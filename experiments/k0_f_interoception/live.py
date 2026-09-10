"""Frozen policies choose resources for new real hardware jobs; no replayed reward."""
import argparse,copy,json,random,signal,subprocess,sys,time
from pathlib import Path
import numpy as np
import torch
from .acquire import Aligner,Background,TASKS,LABELS,emit,save,runtime_state
from .workloads import JobPool
from .normalize import canonical_identity,FRAME_SCHEMA_ID
from .policy import load_dataset,load_model,encode_inputs,infer,PRIMARY_MODES,sha256
from .statistics import describe,paired_comparison


def aggregate(rows):
    entries=[]
    for seed in sorted({r['seed'] for r in rows}):
        for mode in list(PRIMARY_MODES)+['TRAINED_BLIND']:
            rs=[r for r in rows if r['seed']==seed and r['mode']==mode]
            if not rs:continue
            entries.append(dict(seed=seed,mode=mode,n_jobs=len(rs),utility=float(np.mean([r['utility'] for r in rs])),latency_seconds=float(np.mean([r['measurement']['latency_seconds'] for r in rs])),deadline_success_rate=float(np.mean([r['success'] for r in rs])),failure_rate=float(np.mean([not r['measurement']['ok'] or not r['measurement']['finite'] for r in rs]))))
    table={m:sorted([r for r in entries if r['mode']==m],key=lambda r:r['seed']) for m in list(PRIMARY_MODES)+['TRAINED_BLIND']}
    comparisons={}
    for mode in table:
        if mode=='BODY':continue
        if [r['seed'] for r in table['BODY']]!=[r['seed'] for r in table[mode]]:raise ValueError('Unpaired live seeds')
        comparisons['BODY_vs_'+mode]=paired_comparison([r['utility'] for r in table['BODY']],[r['utility'] for r in table[mode]])
    return dict(schema_version='k0-f-live-v1',rows=entries,comparisons=comparisons,statistics={m:{metric:describe([r[metric] for r in entries]) for metric in ['utility','latency_seconds','deadline_success_rate','failure_rate']} for m,entries in table.items()},outcome_provenance='new_real_jobs_executed_after_frozen_policy_choice',independent_unit='training_seed',limitations='Sequential randomized mode execution within workload blocks; shared physical environment and fixed workload cohort, no claim of population uncertainty from telemetry points.')


def collect(args):
    torch.set_num_threads(2)
    out=args.output;out.mkdir(parents=True,exist_ok=True)
    if (out/'driver_identity.json').exists():raise FileExistsError('Use a fresh live directory')
    training_config=json.loads((args.training_artifacts/'training_config.json').read_text())
    scope=training_config.get('experiment_scope','confirmatory');eligible=training_config.get('confirmatory_eligible',True)
    models={};identities={}
    for seed in range(8):
        for mode in ['BODY','BLIND']:
            path=args.training_artifacts/'runs'/f'gru128-s{seed}-{mode.lower()}'/'best.pt'
            model,meta=load_model(path)
            status=json.loads((path.parent/'status.json').read_text())
            if sha256(path)!=status['checkpoint_sha256']['best']:raise ValueError('Live checkpoint hash changed')
            if meta['seed']!=seed or meta['training_mode']!=mode or meta['checkpoint_stage']!='best' or meta['architecture']!='GRU128':raise ValueError('Live checkpoint identity mismatch')
            if meta['identities']['normalization_identity']!=canonical_identity() or meta['identities']['sensor_schema_identity']!=FRAME_SCHEMA_ID:raise ValueError('Live sensor normalization differs from training')
            models[seed,mode]=model;identities[f'{seed}_{mode}']=sha256(path)
    # Only archived body observations are sampled. Costs/labels from this pool are never used live.
    donors=[r for r in load_dataset(args.dataset) if r['split']=='test']
    save(out/'initial_runtime_state.json',runtime_state())
    save(out/'driver_identity.json',dict(pid=__import__('os').getpid(),started_at=time.time(),owned_by='k0-f-acquire'))
    save(out/'live_config.json',dict(experiment_scope=scope,confirmatory_eligible=eligible,decision_sha256=training_config.get('decision_sha256'),seeds=list(range(8)),modes=list(PRIMARY_MODES)+['TRAINED_BLIND'],blocks=args.blocks,tasks=TASKS,checkpoint_sha256=identities,source_commit=args.source_commit,shuffle='different archived episode with identical history length; costs and labels excluded',timing='randomized task/seed/mode order per block; common pre-action body for each 5-mode group',success_rule='live BODY vs BLIND and independently trained BLIND paired utility difference mean>0, CI lower>0, exact p<=.05',body_kind='real',reward='1-min(real_latency/deadline,2); failure=-1'))
    aligner=Aligner(out);sensor=None;bg=None;rows=[];manifest=[];ok=False
    try:
        sensor=subprocess.Popen([sys.executable,'-m','experiments.k0_f_interoception.sensors','--duration','2300','--interval','1','--output',str(out/'raw_master_telemetry.jsonl')],stdout=subprocess.DEVNULL)
        aligner.start();deadline=time.monotonic()+45
        while (not aligner.mac or not aligner.master) and time.monotonic()<deadline:time.sleep(.5)
        aligner.guard()
        with JobPool() as pool:
            for _ in range(35):aligner.guard();time.sleep(1)
            labels=LABELS.copy();random.Random(981731).shuffle(labels)
            for block in range(args.blocks):
                label=labels[block%len(labels)];bid=f'live-{block:03d}';started=time.time()
                bg=Background(label,240,out,bid)
                print(json.dumps(dict(event='block_start',block=block,block_id=bid,label=label,seconds=240)),flush=True)
                for _ in range(5):aligner.guard();bg.check();time.sleep(1)
                rng=random.Random(982000+block);cases=[(s,t) for s in range(8) for t in range(4)];rng.shuffle(cases)
                for group,(seed,task_id) in enumerate(cases):
                    aligner.guard();bg.check();task=TASKS[task_id];when=time.time();length=1+rng.randrange(8)
                    seq=aligner.sequence(length=length);stale=copy.deepcopy(aligner.sequence(when-30,length=length))
                    for f in stale:f['values'][18]=min(1.,f['values'][18]+max(0,when-f['timestamp'])/60.)
                    if not stale:stale=[dict(values=[0.]*20,mask=[0.]*20,timestamp=when-30,frame_id='missing')]
                    row=dict(schema_version='k0-f-policy-v1',episode_id=f'{bid}-{group:02d}',block_id=bid,split='test',task_features=[task['matrix_dimension']/3072,task['repetitions']/20,task['deadline_seconds']/5,0.],body_sequence=[f['values'] for f in seq],body_mask_sequence=[f['mask'] for f in seq],stale_body_sequence=[f['values'] for f in stale],stale_body_mask_sequence=[f['mask'] for f in stale])
                    candidates=[r for r in donors if len(r['body_sequence'])==len(seq)]
                    if not candidates:raise ValueError('No same-length archived SHUFFLED body donor')
                    donor=candidates[rng.randrange(len(candidates))]
                    modes=list(PRIMARY_MODES)+['TRAINED_BLIND'];rng.shuffle(modes)
                    for mode in modes:
                        observed=copy.deepcopy(row)
                        if mode=='SHUFFLED':
                            observed['body_sequence']=donor['body_sequence'];observed['body_mask_sequence']=donor['body_mask_sequence']
                        intervention='BODY' if mode=='SHUFFLED' else ('BLIND' if mode=='TRAINED_BLIND' else mode)
                        model=models[seed,'BLIND' if mode=='TRAINED_BLIND' else 'BODY']
                        inputs=encode_inputs([observed],intervention,seed)
                        tick=time.perf_counter();actions,norms=infer(model,inputs);policy_time=time.perf_counter()-tick
                        measurement=pool.run(actions[0],task);failure=not measurement.get('ok') or not measurement.get('finite')
                        utility=-1. if failure else 1-min(measurement['latency_seconds']/task['deadline_seconds'],2)
                        record=dict(schema_version='k0-f-live-job-v1',seed=seed,mode=mode,block_id=bid,group_id=row['episode_id'],task_id=task_id,timestamp=when,action=actions[0],hidden_norm=norms[0],policy_seconds=policy_time,model_input_sequence=inputs[0].tolist(),current_base_frame_ids=[f['frame_id'] for f in seq],actual_input_frame_ids=(donor['provenance']['frame_ids'] if mode=='SHUFFLED' else [f['frame_id'] for f in stale] if mode=='STALE' else [f['frame_id'] for f in seq]),body_masked=mode in ('BLIND','TRAINED_BLIND'),donor_episode_id=donor['episode_id'] if mode=='SHUFFLED' else None,deadline_seconds=task['deadline_seconds'],measurement=measurement,utility=utility,success=not failure and measurement['latency_seconds']<=task['deadline_seconds'],source_kind='real_new_hardware_job')
                        rows.append(record);emit(out/'live_jobs.jsonl',record)
                bg.check();bg.close();manifest.append(dict(block_id=bid,workload_label=label,start_timestamp=started,end_timestamp=time.time(),workload_processes=bg.evidence()));bg=None;save(out/'workload_manifest.json',manifest)
                print(json.dumps(dict(event='block_end',block_id=bid)),flush=True)
        for _ in range(10):aligner.guard();time.sleep(1)
        result=aggregate(rows);result.update(experiment_scope=scope,confirmatory_eligible=eligible,decision_sha256=training_config.get('decision_sha256'))
        save(out/'live_results.json',result);ok=True
    finally:
        if bg:bg.close()
        aligner.close()
        if sensor and sensor.poll() is None:
            sensor.terminate()
            try:sensor.wait(timeout=5)
            except subprocess.TimeoutExpired:sensor.kill();sensor.wait(timeout=5)
        state=runtime_state();state.update(collection_complete=ok,owned_sensor_stopped=sensor is None or sensor.poll() is not None,owned_background_stopped=bg is None or all(p.poll() is not None for p in bg.children))
        save(out/'collection_runtime_state.json',state)
        save(out/'workload_manifest.json',manifest)
    print(json.dumps(dict(event='collection_complete',rows=len(rows),blocks=len(manifest))),flush=True)


def main():
    p=argparse.ArgumentParser();p.add_argument('cmd',choices=['collect']);p.add_argument('--output',required=True,type=Path);p.add_argument('--blocks',type=int,default=8);p.add_argument('--block-seconds',type=float,default=240);p.add_argument('--source-commit',required=True);p.add_argument('--training-artifacts',type=Path,required=True);p.add_argument('--dataset',type=Path,required=True)
    a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt('live acquisition interrupted')
    signal.signal(signal.SIGTERM,stop);collect(a)

if __name__=='__main__':main()
