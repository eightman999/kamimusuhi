"""Frozen F2 policies choose actions for independent, new numerical jobs."""
from __future__ import annotations
import argparse, copy, json, os, random, signal, subprocess, sys, time
from pathlib import Path
import numpy as np
import torch
from .acquire import Aligner, Background, emit, save, runtime_state, row_inputs, body_snapshot
from .workloads import JobPool
from .normalize import canonical_identity, FRAME_SCHEMA_ID
from .protocol import TASKS, LABELS, load_lock, lock_sha256, lock_receipt
from .data import load_split, encode_inputs, sha256
from .policy import load_model, infer
from .probe import features, predict
from .statistics import describe, paired_comparison

MODES = ('BODY', 'TRAINED_BLIND', 'SHUFFLED', 'STALE')


def aggregate(rows, require_complete=True):
    groups = {}
    for r in rows:
        key = (r['session_id'], r['block_id'], r['seed'], r['task_id'])
        groups.setdefault(key, []).append(r)
    for entries in groups.values():
        if sorted(r['mode'] for r in entries) != sorted(MODES):
            raise ValueError('Live group missing or duplicate mode')
    if require_complete:
        if len(rows) != 3072 or {r['session_id'] for r in rows} != {'L1', 'L2'}:
            raise ValueError('Both fixed live sessions with 3072 jobs required')
        for sid in ('L1', 'L2'):
            subset = [r for r in rows if r['session_id'] == sid]
            if len({r['block_id'] for r in subset}) != 8:
                raise ValueError('Live block coverage differs')
    entries=[]
    for seed in sorted({r['seed'] for r in rows}):
        for mode in MODES:
            rs=[r for r in rows if r['seed']==seed and r['mode']==mode]
            if not rs: raise ValueError('Unpaired live seeds')
            entries.append(dict(seed=seed, mode=mode, n_jobs=len(rs), utility=float(np.mean([r['utility'] for r in rs])),
                latency_seconds=float(np.mean([r['measurement']['latency_seconds'] for r in rs])),
                deadline_success_rate=float(np.mean([r['success'] for r in rs])),
                failure_rate=float(np.mean([not r['measurement']['ok'] or not r['measurement']['finite'] for r in rs])),
                action_rates={str(a):float(np.mean([r['action']==a for r in rs])) for a in range(4)}))
    table={m:sorted([r for r in entries if r['mode']==m],key=lambda r:r['seed']) for m in MODES}
    comparisons={}
    for mode in MODES[1:]:
        if [r['seed'] for r in table['BODY']] != [r['seed'] for r in table[mode]]:
            raise ValueError('Unpaired live seeds')
        label='P4' if mode=='TRAINED_BLIND' else 'D_LIVE_'+mode
        comparisons[label]=paired_comparison([r['utility'] for r in table['BODY']], [r['utility'] for r in table[mode]])
    return dict(schema_version='k0-f2-live-v1', rows=entries, comparisons=comparisons,
        statistics={m:{metric:describe([r[metric] for r in rs]) for metric in ('utility','latency_seconds','deadline_success_rate','failure_rate')} for m,rs in table.items()},
        outcome_provenance='new_real_jobs_executed_after_frozen_policy_choice', independent_unit='training_seed',
        limitations='Shared physical environment and fixed workload cohort; randomized sequential modes. Seed intervals do not estimate population uncertainty over machines or workloads.')


def order_effect(rows):
    mode_means={m:float(np.mean([r['measurement']['latency_seconds'] for r in rows if r['mode']==m])) for m in MODES}
    by_position=[]
    for i in range(4):
        rs=[r for r in rows if r['order_index']==i]
        if not rs: raise ValueError('Missing live execution order position')
        by_position.append(dict(order_index=i,n_jobs=len(rs),mean_latency_seconds=float(np.mean([r['measurement']['latency_seconds'] for r in rs])),
            mean_utility=float(np.mean([r['utility'] for r in rs])),
            within_mode_centered_latency=float(np.mean([r['measurement']['latency_seconds']-mode_means[r['mode']] for r in rs])),
            mode_counts={m:sum(r['mode']==m for r in rs) for m in MODES}))
    return dict(schema_version='k0-f2-live-order-v1', rows=by_position, role='descriptive_secondary_only',
                limitation='Sequential actions may carry thermal effects despite randomized order; no independent hardware cohort inference.')


def collect(args):
    lock=load_lock(args.protocol_lock);identity=lock_sha256(args.protocol_lock);receipt=lock_receipt(args.protocol_lock)
    spec=lock['live']
    if args.session_id not in spec['sessions'] or args.blocks!=spec['blocks_per_session'] or args.block_seconds!=spec['block_max_seconds']:
        raise ValueError('Live budget or session differs from pre-data lock')
    gate=json.loads((args.study_root/'validation/probe_validation.json').read_text())
    freeze=json.loads((args.training_artifacts/'checkpoint_manifest.json').read_text())
    if gate.get('gate',{}).get('pass') is not True or freeze.get('complete') is not True:
        raise RuntimeError('Live requires passed probe and completed checkpoint freeze')
    if gate.get('protocol_lock_sha256')!=identity or freeze.get('protocol_lock_sha256')!=identity:
        raise ValueError('Live frozen identities differ')
    torch.set_num_threads(lock['policy']['torch_threads'])
    models={};identities={}
    for seed in lock['policy']['seeds']:
        for mode in ('BODY','BLIND'):
            path=args.training_artifacts/'runs'/f'gru128-s{seed}-{mode.lower()}'/'best.pt'
            model,meta=load_model(path);status=json.loads((path.parent/'status.json').read_text())
            if not status['complete'] or sha256(path)!=status['checkpoint_sha256']['best']:
                raise ValueError('Live checkpoint hash changed')
            if (meta['seed'],meta['training_mode'],meta['checkpoint_stage'],meta['architecture'])!=(seed,mode,'best','GRU128'):
                raise ValueError('Live checkpoint identity mismatch')
            if meta['identities']['protocol_lock_sha256']!=identity or meta['identities']['normalization_identity']!=canonical_identity() or meta['identities']['sensor_schema_identity']!=FRAME_SCHEMA_ID:
                raise ValueError('Live checkpoint protocol or normalization differs')
            models[seed,mode]=model;identities[f'{seed}_{mode}']=sha256(path)
    # The test file is reachable only after gate and checkpoint verification above.
    donors=load_split(args.dataset,'test',identity,lock['normalization_identity'])
    donor_groups={}
    for r in donors: donor_groups.setdefault((r['task_id'],len(r['body_sequence'])),[]).append(r)
    probe_file=args.study_root/'validation/probe_models.json'
    probe_meta=json.loads(probe_file.read_text())
    if probe_meta['protocol_lock_sha256']!=identity: raise ValueError('Live probe identity mismatch')
    probe_model=probe_meta['models']['BODY/utility']
    for key in ('x_center','x_scale','y_center','y_scale','coefficients'): probe_model[key]=np.asarray(probe_model[key])
    out=args.output;out.mkdir(parents=True,exist_ok=True)
    if (out/'driver_identity.json').exists():raise FileExistsError('Use a fresh live session directory')
    before=runtime_state()
    if before['compute_processes']:raise RuntimeError('Unrelated GPU compute active; live not started')
    save(out/'initial_runtime_state.json',before)
    save(out/'driver_identity.json',dict(pid=os.getpid(),started_at=time.time(),owned_by='k0-f2-acquire'))
    save(out/'live_config.json',dict(schema_version='k0-f2-live-config-v1',session_id=args.session_id,seeds=list(range(12)),modes=list(MODES),blocks=args.blocks,tasks=TASKS,checkpoint_sha256=identities,probe_models_sha256=sha256(probe_file),donor_dataset_sha256=sha256(args.dataset),**receipt))
    aligner=Aligner(out,args.session_id,identity);sensor=None;bg=None;rows=[];manifest=[];ok=False;started=time.time()
    seed_base=spec['sessions'][args.session_id]['seed']
    try:
        sensor=subprocess.Popen([sys.executable,'-m','experiments.k0_f2_interoception_confirmatory.sensors','--duration',str(args.blocks*args.block_seconds+360),'--interval','1','--output',str(out/'raw_master_telemetry.jsonl')],stdout=subprocess.DEVNULL)
        aligner.start();deadline=time.monotonic()+45
        while (not aligner.mac or not aligner.master) and time.monotonic()<deadline:time.sleep(.5)
        aligner.guard()
        with JobPool() as pool:
            for _ in range(45):aligner.guard();time.sleep(1)
            labels=LABELS.copy();random.Random(seed_base).shuffle(labels)
            for block,label in enumerate(labels):
                bid=f'{args.session_id}-{block:03d}';begin=time.time()
                bg=Background(label,spec['background_max_seconds'],out,bid)
                print(json.dumps(dict(event='block_start',session_id=args.session_id,block=block,block_id=bid,label=label,seconds=spec['background_max_seconds'])),flush=True)
                for _ in range(5):aligner.guard();bg.check();time.sleep(1)
                rng=random.Random(seed_base+1000+block);cases=[(s,t) for s in range(12) for t in range(4)];rng.shuffle(cases)
                for group,(seed,task_id) in enumerate(cases):
                    if time.time()-begin>args.block_seconds:raise RuntimeError('Locked live block maximum duration exceeded')
                    aligner.guard();bg.check();task=TASKS[task_id];when=time.time();length=1+rng.randrange(8)
                    row=row_inputs(aligner,when=when,length=length,task=task,task_id=task_id,session_id=args.session_id,split='live',block_id=bid,episode_id=f'{bid}-{group:02d}',lock_identity=identity)
                    candidates=donor_groups[(task_id,length)];donor=candidates[rng.randrange(len(candidates))]
                    mode_order=list(MODES);rng.shuffle(mode_order)
                    predictions={};prepared={}
                    for mode in MODES:
                        observed=copy.deepcopy(row)
                        if mode=='SHUFFLED':
                            observed['body_sequence']=donor['body_sequence'];observed['body_mask_sequence']=donor['body_mask_sequence']
                        intervention='BLIND' if mode=='TRAINED_BLIND' else 'BODY' if mode=='SHUFFLED' else mode
                        inputs=encode_inputs([observed],intervention,seed)
                        model=models[seed,'BLIND' if mode=='TRAINED_BLIND' else 'BODY']
                        tick=time.perf_counter();actions,norms=infer(model,inputs);policy_seconds=time.perf_counter()-tick
                        prepared[mode]=(inputs,actions[0],norms[0],policy_seconds)
                    x,_=features([row],'BODY');predicted_utility=predict(probe_model,x,'utility')[0].tolist()
                    for order,mode in enumerate(mode_order):
                        aligner.guard();bg.check();inputs,action,hidden_norm,policy_seconds=prepared[mode]
                        decision_at=time.time();snapshot=body_snapshot(aligner);measurement=pool.run(action,task)
                        measurement.update(decision_timestamp=decision_at,body_timestamp=row['provenance']['body_timestamp'],order_index=order,body_at_dispatch=snapshot)
                        if not measurement.get('ok') or not measurement.get('finite'):raise RuntimeError('Invalid live foreground result; stop safely')
                        utility=1-min(measurement['latency_seconds']/task['deadline_seconds'],2)
                        input_frames=donor['provenance']['frame_ids'] if mode=='SHUFFLED' else row['provenance']['stale_frame_ids'] if mode=='STALE' else row['provenance']['frame_ids']
                        record=dict(schema_version='k0-f2-live-job-v1',protocol_lock_sha256=identity,session_id=args.session_id,seed=seed,mode=mode,block_id=bid,group_id=row['episode_id'],task_id=task_id,task=task,timestamp=when,decision_timestamp=decision_at,body_timestamp=row['provenance']['body_timestamp'],order_index=order,mode_order=mode_order,action=action,hidden_norm=hidden_norm,policy_seconds=policy_seconds,model_input_sequence=inputs[0].tolist(),base_row=row,current_base_frame_ids=row['provenance']['frame_ids'],actual_input_frame_ids=input_frames,body_masked=mode=='TRAINED_BLIND',donor_episode_id=donor['episode_id'] if mode=='SHUFFLED' else None,deadline_seconds=task['deadline_seconds'],measurement=measurement,utility=utility,success=measurement['latency_seconds']<=task['deadline_seconds'],source_kind='real_new_hardware_job',probe_predicted_utility=predicted_utility)
                        rows.append(record);emit(out/'live_jobs.jsonl',record)
                        save(out/'current_decision.json',dict(stage='live',timestamp=decision_at,task=task,task_id=task_id,seed=seed,mode=mode,body_action=prepared['BODY'][1],blind_action=prepared['TRAINED_BLIND'][1],probe_predicted_utility=predicted_utility,actual_selected_resource=measurement['actual_resource'],actual_latency=measurement['latency_seconds'],hidden_norm=hidden_norm,body_age_seconds=decision_at-row['provenance']['body_timestamp'],body_mask=row['body_mask_sequence'][-1]))
                bg.check();bg.close();manifest.append(dict(session_id=args.session_id,block_id=bid,workload_label=label,start_timestamp=begin,end_timestamp=time.time(),workload_processes=bg.evidence()));bg=None;save(out/'workload_manifest.json',manifest)
                print(json.dumps(dict(event='block_end',session_id=args.session_id,block_id=bid)),flush=True)
        save(out/'live_session_results.json',dict(aggregate(rows,require_complete=False),session_id=args.session_id,protocol_lock_sha256=identity));ok=True
    finally:
        if bg:bg.close()
        aligner.close()
        if sensor and sensor.poll() is None:
            sensor.terminate()
            try:sensor.wait(timeout=5)
            except subprocess.TimeoutExpired:sensor.kill();sensor.wait(timeout=5)
        state=runtime_state();state.update(collection_complete=ok,session_id=args.session_id,start_timestamp=started,end_timestamp=time.time(),protocol_lock_sha256=identity,owned_sensor_stopped=sensor is None or sensor.poll() is not None,owned_background_stopped=bg is None or all(p.poll() is not None for p in bg.children),scratch_files=[str(p.relative_to(out)) for p in (out/'scratch').glob('*')])
        save(out/'collection_runtime_state.json',state);save(out/'workload_manifest.json',manifest)
    print(json.dumps(dict(event='collection_complete',session_id=args.session_id,rows=len(rows),blocks=len(manifest))),flush=True)


def combine(args):
    lock=load_lock(args.protocol_lock);identity=lock_sha256(args.protocol_lock)
    gate=json.loads((args.study_root/'validation/probe_validation.json').read_text())
    if not gate.get('gate',{}).get('pass') or gate.get('protocol_lock_sha256')!=identity:
        raise RuntimeError('No live data may be opened after a failed probe')
    root=args.study_root/'live'
    if (root/'live_results.json').exists(): raise FileExistsError('Live combined result is immutable')
    rows=[]
    for sid in ('L1','L2'):
        state=json.loads((root/sid/'collection_runtime_state.json').read_text())
        if not state['collection_complete'] or state['protocol_lock_sha256']!=identity:
            raise ValueError('Incomplete or mismatched live collection')
        rows.extend(json.loads(line) for line in (root/sid/'live_jobs.jsonl').read_text().splitlines())
    result=aggregate(rows);result.update(protocol_lock_sha256=identity,source_commit=lock['source_commit'])
    save(root/'live_results.json',result);save(root/'live_order_effect.json',order_effect(rows))
    with (root/'live_jobs.jsonl').open('x') as handle:
        for row in rows:handle.write(json.dumps(row,allow_nan=False)+'\n')


def main():
    p=argparse.ArgumentParser();p.add_argument('cmd',choices=['collect','combine']);p.add_argument('--output',type=Path);p.add_argument('--blocks',type=int,default=8);p.add_argument('--block-seconds',type=float,default=120);p.add_argument('--protocol-lock',type=Path,required=True);p.add_argument('--session-id');p.add_argument('--study-root',type=Path,required=True);p.add_argument('--training-artifacts',type=Path);p.add_argument('--dataset',type=Path)
    a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt('live acquisition interrupted')
    signal.signal(signal.SIGTERM,stop)
    if a.cmd=='collect':
        if any(x is None for x in (a.output,a.session_id,a.training_artifacts,a.dataset)):p.error('collect requires output, session-id, training-artifacts, dataset')
        collect(a)
    else:combine(a)

if __name__=='__main__':main()
