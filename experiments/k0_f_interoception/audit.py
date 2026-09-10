"""Audit actual recorded telemetry, reconstructed inputs, checkpoints, and prior artifacts."""
import argparse,hashlib,json,math,statistics
from pathlib import Path
from .normalize import normalize_pair,canonical_identity,DEFAULT_CONFIG
from .acquire import save


def read_jsonl(path):return [json.loads(s) for s in path.read_text().splitlines() if s.strip()]
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def summarize(numbers):
    a=[float(x) for x in numbers if isinstance(x,(int,float)) and math.isfinite(x)]
    return dict(n=len(a),minimum=min(a) if a else None,maximum=max(a) if a else None,mean=statistics.mean(a) if a else None,median=statistics.median(a) if a else None)


def audit(root,primary,policy=None,live=None,llm=None):
    initial=json.loads((root/'baseline_manifest.json').read_text())
    repo=root.parents[1]
    failures=[name for name,value in initial['sha256'].items() if not (repo/name).exists() or digest(repo/name)!=value]
    save(primary/'baseline_integrity.json',dict(pass_=not failures,**{'pass':not failures},files_checked=len(initial['sha256']),changed=failures))
    nodes={};norm_errors=[];sources={};sensor_missing={}
    collections={'primary':primary}
    if live and (live/'raw_master_telemetry.jsonl').exists():collections['live']=live
    if llm and (llm/'raw_master_telemetry.jsonl').exists():collections['llm_record_only']=llm
    for kind,folder in collections.items():
        sources[kind]={}
        for node in ['mac','master']:
            path=folder/('raw_'+node+'_telemetry.jsonl');raw=read_jsonl(path)
            if not raw:raise ValueError('Empty actual telemetry')
            ts=[r['timestamp'] for r in raw];gaps=[b-a for a,b in zip(ts,ts[1:])]
            assert all(r['source_kind']=='real' and r['node_id']==node for r in raw)
            assert len({r['sequence'] for r in raw})==len(raw)
            metrics=sorted({k for r in raw for k in r['metrics']})
            sensor_missing[kind+'_'+node]=[k for k in metrics if all(r['metrics'].get(k) is None for r in raw)]
            sources[kind][node]=dict(samples=len(raw),duration_seconds=ts[-1]-ts[0],interval_seconds=summarize(gaps),daemon_cpu_fraction=summarize([r['metrics'].get('daemon_cpu_fraction') for r in raw]),daemon_rss_bytes=summarize([r['metrics'].get('daemon_rss_bytes') for r in raw]),raw_sha256=digest(path))
            nodes[kind+'_'+node]=raw
        frames=read_jsonl(folder/'interoceptive_frames.jsonl');aligned=read_jsonl(folder/'aligned_body_telemetry.jsonl')
        if len(frames)!=len(aligned):norm_errors.append(kind+': row counts mismatch')
        for frame,row in zip(frames,aligned):
            expected=normalize_pair(row['master'],row['mac'],now=row['timestamp'])
            if any(frame[k]!=expected[k] for k in ['values','mask','quality','normalization_identity']):norm_errors.append(kind+': frame mismatch '+frame['frame_id'])
        sources[kind]['frames']=dict(samples=len(frames),dimensions=sorted({len(f['values']) for f in frames}),availability=summarize([f['availability'] for f in frames]),normalization_identity=canonical_identity())
    inputs=read_jsonl(primary/'policy_dataset.jsonl');frames=read_jsonl(primary/'interoceptive_frames.jsonl');lookup={f['frame_id']:f for f in frames}
    byblock={};input_errors=[];target_lags=[]
    for row in inputs:
        byblock.setdefault(row['block_id'],[]).append(row)
        for key,target in [('frame_ids',row['timestamp']),('stale_frame_ids',row['timestamp']-30)]:
            if any(lookup[i]['timestamp']>target for i in row['provenance'][key]):input_errors.append('future frame: '+row['episode_id'])
        if row['provenance'].get('future_target_source_timestamp') is not None:target_lags.append(row['provenance']['future_target_source_timestamp']-row['timestamp'])
    balanced=all(sorted(len(r['body_sequence']) for r in block)==list(range(1,9)) for block in byblock.values())
    split_counts={s:sum(r['split']==s for r in inputs) for s in ['train','validation','test']}
    checkpts=[]
    if policy:
        import torch
        for status_path in sorted((policy/'runs').glob('*/status.json')):
            status=json.loads(status_path.read_text())
            for stage,h in status['checkpoint_sha256'].items():
                path=status_path.parent/(stage+'.pt');checkpoint=torch.load(path,map_location='cpu',weights_only=True)
                finite=all(bool(torch.isfinite(t).all()) for t in checkpoint['state_dict'].values())
                match=digest(path)==h
                checkpts.append(dict(run=status_path.parent.name,stage=stage,finite=finite,hash_matches=match,sha256=h,seed=checkpoint['metadata']['seed']))
    masters=[r for key,rs in nodes.items() if key.endswith('_master') for r in rs]
    macs=[r for key,rs in nodes.items() if key.endswith('_mac') for r in rs]
    temp={k:summarize([r['metrics'].get(k) for r in masters]) for k in ['cpu_temperature_c','rtx3060_temperature_c','p100_temperature_c']}
    thermal=summarize([r['metrics'].get('thermal_pressure') for r in macs])
    ram=summarize([r['metrics'].get('memory_available_bytes') for r in masters])
    safety=all(temp[k]['maximum'] is not None and temp[k]['maximum']<75 for k in ['rtx3060_temperature_c','p100_temperature_c']) and (temp['cpu_temperature_c']['maximum'] is None or temp['cpu_temperature_c']['maximum']<80) and thermal['maximum'] is not None and thermal['maximum']<2/3 and ram['minimum']>=4*1024**3
    continuous=all(sources['primary'][n]['samples']>=900 and sources['primary'][n]['interval_seconds']['maximum']<=5 for n in ['mac','master'])
    reproducible=not failures and not norm_errors and not input_errors and balanced and len(checkpts)==32 and all(c['finite'] and c['hash_matches'] for c in checkpts)
    result=dict(schema_version='k0-f-audit-v1',telemetry_continuous=continuous,fixed_frame_valid=not norm_errors,safety_pass=safety,reproducibility_pass=reproducible,telemetry=sources,always_missing_sensors=sensor_missing,temperature_c=temp,mac_thermal_pressure=thermal,master_ram_available_bytes=ram,normalization_errors=norm_errors,input_errors=input_errors,balanced_history_lengths=balanced,split_counts=split_counts,future_target_lag_seconds=summarize(target_lags),checkpoint_count=len(checkpts),checkpoints=checkpts,safety_configuration={'gpu_stop_c':75,'cpu_stop_c':80,'mac_stop_thermal_pressure':2/3,'gpu_background_duty_limit':.5,'background_cpu_threads':2,'minimum_free_ram_bytes':4*1024**3},limitations=['daemon CPU reflects collector process; external CLI subprocess overhead is not fully included','thermal sensor sample maximum is observed evidence, not a proof about every instant between samples','fixed heldout workload: training-seed CI does not quantify arbitrary deployment workloads'])
    save(primary/'resource_summary.json',result)
    return result


def main():
    p=argparse.ArgumentParser();p.add_argument('--artifacts',type=Path,required=True);p.add_argument('--policy-artifacts',type=Path);p.add_argument('--live-artifacts',type=Path);p.add_argument('--llm-artifacts',type=Path);a=p.parse_args();r=audit(Path(__file__).parent,a.artifacts,a.policy_artifacts,a.live_artifacts,a.llm_artifacts);print(json.dumps({k:r[k] for k in ['telemetry_continuous','fixed_frame_valid','safety_pass','reproducibility_pass','checkpoint_count']}))

if __name__=='__main__':main()
