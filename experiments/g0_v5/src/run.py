"""Stage runner: controls -> evaluated sanity -> pilot; never silently overwrite."""
import argparse
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import numpy as np
import torch
from .controls import PCAControl, raw_encode, temporal_readout
from .protocol import METHODS, PROTOCOL, REQUIRED_METRICS, sha, same_metrics, pilot_gate, verify_lock

ROOT = Path(__file__).resolve().parents[1]


def write(path, data):
    with Path(path).open('x') as f: json.dump(data,f,indent=2,allow_nan=False)


def data_check(data):
    manifest = json.loads((data/'dataset_manifest.json').read_text())
    for rel, expected in manifest['files'].items():
        if sha(data/rel) != expected: raise RuntimeError(f'DATA_LEAK: dataset hash mismatch {rel}')
    for name in ('train','validation'):
        with np.load(data/'training'/f'{name}.npz') as a:
            if set(a.files) != {'obs'}: raise RuntimeError('DATA_LEAK: training keys')
    return manifest


def train_call(config, data, output, seed, device, initialize=False):
    command=[sys.executable,'-m','experiments.g0_v5.src.train','--config',str(config),
             '--data-dir',str(data/'training'),'--output',str(output),'--seed',str(seed),'--device',device]
    if initialize: command.append('--initialize-only')
    env=os.environ.copy()
    env['CUBLAS_WORKSPACE_CONFIG']=':4096:8'
    return subprocess.run(command,env=env,check=False).returncode


def isolation_audit():
    """Structural allowlist plus observation archive hashes. Not an OS sandbox."""
    files = [ROOT/'src/train.py', ROOT/'src/telemetry.py'] + list((ROOT/'src/models').glob('*.py')) + list((ROOT/'src/losses').glob('*.py'))
    rejected=[]
    for p in files:
        if not p.exists(): continue
        tree=ast.parse(p.read_text())
        for node in ast.walk(tree):
            if isinstance(node,ast.ImportFrom) and any(t in (node.module or '').split('.') for t in ('evaluate','dataset')):
                rejected.append(f'{p.name}: imports oracle-capable module')
            if isinstance(node,ast.Subscript) and isinstance(node.slice,ast.Constant) and node.slice.value in ('cause','context','composition_id','ood_category','pair_label'):
                rejected.append(f'{p.name}: oracle key access')
    return {'passed':not rejected,'findings':rejected,
            'boundary':'separate trainer subprocess receives training/ only, strict obs-only archives; evaluator imported only in orchestration process',
            'limitation':'same OS user can access evaluation directory; code/data-boundary audit, not adversarial filesystem sandbox'}


def evaluate(encode,data,out):
    from .evaluate import evaluate_representation
    out.mkdir(parents=True,exist_ok=False)
    metrics=evaluate_representation(encode,data,out)
    write(out/'metrics.json',metrics)
    again=evaluate_representation(encode,data,None)
    write(out/'repeat_check.json',{'passed':same_metrics(metrics,again),'atol':1e-7,'rtol':1e-6})
    return metrics, same_metrics(metrics,again)


def controls(args):
    from .train import load_encoder
    data_check(args.data)
    lock=verify_lock(ROOT,args.lock,args.data/'dataset_manifest.json')
    base=ROOT/'results/controls'
    base.mkdir(parents=True,exist_ok=True)
    train=np.load(args.data/'training/train.npz')['obs']
    val=np.load(args.data/'training/validation.npz')['obs']
    for name, encoder in [('raw',raw_encode),('pca',PCAControl(train,16))]:
        metrics,repeat=evaluate(encoder,args.data,base/name)
        write(base/name/'temporal_readout.json',temporal_readout(encoder,train,val))
        write(base/name/'manifest.json',{'experiment_id':'G0-v5','control':name,'lock_hash':lock['lock_hash'],'dataset_manifest_sha256':sha(args.data/'dataset_manifest.json'),'fit_scope':'train observations only' if name=='pca' else 'no fit'})
        if name=='pca': encoder.save(base/name/'pca.npz')
        if not repeat: raise RuntimeError('EVAL_ERROR: repeat failed for '+name)
    for method in METHODS:
        device='cuda:1' if method in ('gru','vicreg') else 'cuda:0'
        if args.device: device=args.device
        out=base/f'{method}_seed0'
        if train_call(ROOT/'configs'/f'{method}.yaml',args.data,out,0,device,True): raise RuntimeError('initialization failed '+method)
        encoder=load_encoder(out/'checkpoint_initial.pt',device)
        metrics,repeat=evaluate(encoder,args.data,out/'evaluation')
        write(out/'temporal_readout.json',temporal_readout(encoder,train,val))
        if not repeat: raise RuntimeError('EVAL_ERROR: repeat failed for '+method)
    audit=isolation_audit()
    if not audit['passed']: raise RuntimeError(str(audit))
    write(base/'sanity.json',{'experiment_id':'G0-v5','repeat_all':True,'isolation':audit,'lock_hash':lock['lock_hash'],'dataset_manifest_sha256':sha(args.data/'dataset_manifest.json'),
                            'old_g0':'SKIP: historical results prohibited; incompatible obs/action input contract'})


def pilot(args):
    from .train import load_encoder
    data_check(args.data)
    lock=verify_lock(ROOT,args.lock,args.data/'dataset_manifest.json')
    control=ROOT/'results/controls'
    sanity=json.loads((control/'sanity.json').read_text())
    if not sanity['repeat_all'] or not sanity['isolation']['passed']: raise RuntimeError('control sanity gate failed')
    if sanity['lock_hash']!=lock['lock_hash'] or sanity['dataset_manifest_sha256']!=sha(args.data/'dataset_manifest.json'): raise RuntimeError('control identity mismatch')
    method=args.method
    out=ROOT/'results/pilot'/f'{method}_seed0'
    code=train_call(ROOT/'configs'/f'{method}.yaml',args.data,out,0,args.device)
    if code:
        manifest=json.loads((out/'manifest.json').read_text())
        write(out/'sanity_gate.json',{'admitted':False,'stop_reasons':[manifest['failure_class']],'training_status':manifest['status']})
        empty={k:{} for k in REQUIRED_METRICS}; empty['failure_class']=manifest['failure_class']
        write(out/'metrics.json',empty)
        print(f'{method}: trainer failed ({code}); preserved {out}',flush=True)
        return code
    before=torch.load(control/f'{method}_seed0/checkpoint_initial.pt',map_location='cpu',weights_only=False)
    after=torch.load(out/'checkpoint_initial.pt',map_location='cpu',weights_only=False)
    def state(c): return c.get('state_dict',c.get('model_state',c.get('model')))
    a,b=state(before),state(after)
    twin_identical=a is not None and b is not None and set(a)==set(b) and all(torch.equal(a[k],b[k]) for k in a)
    if not twin_identical: raise RuntimeError('INVALID: control is not exact initialization twin')
    encoder=load_encoder(out/'checkpoint_best.pt',args.device)
    metrics,repeat=evaluate(encoder,args.data,out/'evaluation')
    train=np.load(args.data/'training/train.npz')['obs']; val=np.load(args.data/'training/validation.npz')['obs']
    readout=temporal_readout(encoder,train,val)
    write(out/'temporal_readout.json',readout)
    twin=json.loads((control/f'{method}_seed0/evaluation/metrics.json').read_text())
    twin_readout=json.loads((control/f'{method}_seed0/temporal_readout.json').read_text())
    logs=[json.loads(line) for line in (out/'train_log.jsonl').read_text().splitlines() if line.strip()]
    def finite(x):
        if isinstance(x,dict): return all(finite(v) for v in x.values())
        if isinstance(x,list): return all(finite(v) for v in x)
        return bool(np.isfinite(x)) if isinstance(x,(float,int)) else True
    gate=pilot_gate(metrics,twin,readout,twin_readout,bool(logs) and all(finite(x) for x in logs),repeat,isolation_audit()['passed'] and twin_identical)
    training_manifest=json.loads((out/'manifest.json').read_text())
    epoch_failures=[x['latent']['failure_class'] for x in logs if x.get('latent',{}).get('failure_class','NONE')!='NONE']
    if training_manifest['status']!='complete' or training_manifest['failure_class']!='NONE' or epoch_failures:
        gate['admitted']=False
        gate['stop_reasons'] += sorted(set(epoch_failures + [training_manifest['failure_class']]) - {'NONE'})
    verify_lock(ROOT,args.lock,args.data/'dataset_manifest.json')
    data_check(args.data)
    gate['lock_hash']=lock['lock_hash']
    gate['training_status']=training_manifest['status']
    gate['initial_twin_identical']=twin_identical
    write(out/'sanity_gate.json',gate)
    write(out/'metrics.json',metrics)
    with (out/'metrics.jsonl').open('x') as f: f.write(json.dumps(metrics,allow_nan=False)+'\n')
    print(json.dumps({'method':method,**gate}),flush=True)
    return 0


def main():
    p=argparse.ArgumentParser(); p.add_argument('stage',choices=['controls','pilot'])
    p.add_argument('--data',type=Path,default=ROOT/'data/frozen')
    p.add_argument('--lock',type=Path,default=ROOT/'manifests/protocol_lock.json')
    p.add_argument('--method',choices=METHODS);p.add_argument('--device')
    args=p.parse_args()
    if args.stage=='controls': controls(args)
    else:
        if args.method is None or args.device is None: p.error('pilot requires method and device')
        try:
            sys.exit(pilot(args))
        except Exception as exc:
            out=ROOT/'results/pilot'/f'{args.method}_seed0'
            if out.exists():
                record={k:{} for k in REQUIRED_METRICS}
                record['failure_class']='DATA_LEAK' if 'initialization twin' in str(exc) else 'EVAL_ERROR'
                record['error']=str(exc)
                if not (out/'metrics.json').exists(): write(out/'metrics.json',record)
                if not (out/'sanity_gate.json').exists(): write(out/'sanity_gate.json',{'admitted':False,'stop_reasons':[record['failure_class']],'error':str(exc)})
                write(out/'evaluation_failure.json',record)
            raise

if __name__=='__main__':main()
