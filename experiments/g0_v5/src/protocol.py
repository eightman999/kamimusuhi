"""Fixed pilot admission gates and source identity, before results exist."""
from pathlib import Path
import hashlib
import json
import numpy as np

METHODS = ('gru', 'cpc', 'vicreg', 'jepa')
REQUIRED_METRICS = ('iid','midctx','match_ood','dynseg_ood','combo_oodctx','intervention','collapse','runtime','failure_class')
PROTOCOL = {
    'experiment_id': 'G0-v5', 'revision': 1,
    'dataset_seed': 20260913, 'n_train': 1024, 'n_eval': 256,
    'pca_dimensions': 16, 'pilot_seeds': [0], 'full_seeds': [0,1,2,3,4],
    'eval_repeat_atol': 1e-7, 'eval_repeat_rtol': 1e-6,
    's3_relative_temporal_mse_improvement': .01,
    'selection': 'sanity survivors ranked by label-free validation temporal-readout relative MSE improvement; top 3, require >=2; no OOD ranking',
    'strong_pass': 'same method improves over each raw/PCA/twin on >=3/4 OOD metrics in >=4/5 seeds, plus intervention selectivity improves over all controls in >=4/5 seeds; no collapse/shortcut/leak',
    'execution_scope': 'seed0 pilot; no automatic full run in this turn',
}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_files(root):
    return sorted(p for p in Path(root).rglob('*') if p.is_file() and p.suffix in ('.py','.yaml','.sh','.md')
                  and p.name != 'G0_V5_REPORT.md'
                  and not any(x in p.relative_to(root).parts for x in ('results','data','remote','__pycache__')))


def snapshot(root):
    return {str(p.relative_to(root)): sha(p) for p in source_files(root)}


def freeze(root, destination, dataset_manifest):
    destination = Path(destination)
    record = {'protocol': PROTOCOL, 'source_files': snapshot(root), 'dataset_manifest_sha256': sha(dataset_manifest)}
    record['lock_hash'] = hashlib.sha256(json.dumps(record,sort_keys=True).encode()).hexdigest()
    with destination.open('x') as f: json.dump(record,f,indent=2,sort_keys=True)
    return record


def verify_lock(root, path, dataset_manifest):
    record = json.loads(Path(path).read_text())
    expected_hash=record.pop('lock_hash')
    actual_hash=hashlib.sha256(json.dumps(record,sort_keys=True).encode()).hexdigest()
    record['lock_hash']=expected_hash
    if actual_hash!=expected_hash: raise RuntimeError('Protocol lock hash mismatch')
    if snapshot(root) != record['source_files'] or sha(dataset_manifest) != record['dataset_manifest_sha256']:
        raise RuntimeError('Frozen source/dataset manifest changed; fresh pilot required')
    return record


def same_metrics(a,b):
    """Exclude timing only. Compare all scientific scalar/array fields recursively."""
    if isinstance(a,dict):
        return isinstance(b,dict) and set(a)==set(b) and all(same_metrics(a[k],b[k]) for k in a if k!='runtime')
    if isinstance(a,(list,tuple)):
        return isinstance(b,(list,tuple)) and len(a)==len(b) and all(same_metrics(x,y) for x,y in zip(a,b))
    if isinstance(a,(float,int)) and not isinstance(a,bool):
        return bool(np.isclose(a,b,atol=1e-7,rtol=1e-6,equal_nan=False))
    return a==b


def pilot_gate(trained, twin, readout, twin_readout, finite_losses, repeat_ok, isolation_ok):
    collapse = trained['collapse']
    relative = (twin_readout['mse']-readout['mse']) / max(twin_readout['mse'],1e-12)
    gates = {'S1': bool(finite_losses), 'S2': not collapse.get('full_collapse',True),
             'S3': relative >= PROTOCOL['s3_relative_temporal_mse_improvement'],
             'S4': bool(isolation_ok), 'S5': bool(repeat_ok)}
    stop = []
    if collapse.get('dimensional_collapse',False): stop.append('COLLAPSE_DIM')
    if collapse.get('shortcut_context',False): stop.append('SHORTCUT_CONTEXT')
    if trained.get('failure_class') not in (None,'NONE'): stop.append(trained['failure_class'])
    return {'gates':gates,'admitted':all(gates.values()) and not stop,'stop_reasons':stop,
            'label_free_relative_improvement':relative,
            'S3_definition':'fixed train-only ridge future-observation readout, disjoint validation episodes; >=1% relative MSE reduction over exact twin'}
