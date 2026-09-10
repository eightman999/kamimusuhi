"""Two heterogeneous GPUs, independent processes, durable 24-run queue."""
import argparse,json,os,subprocess,sys,time,fcntl
from pathlib import Path
import yaml
from .trainer import atomic


def inventory():
    result=subprocess.check_output(['nvidia-smi','--query-gpu=index,name,uuid,memory.free','--format=csv,noheader,nounits'],text=True)
    return [dict(zip(['index','name','uuid','free_mb'],[v.strip() for v in line.split(',')])) for line in result.splitlines()]


def main():
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);p.add_argument('--configs',default='experiments/k0_brainstem/configs');p.add_argument('--min-free-mb',type=int,default=5000);a=p.parse_args()
    root=Path(a.artifacts).resolve();root.mkdir(parents=True,exist_ok=True)
    lock=(root/'launcher.lock').open('a')
    try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    except BlockingIOError:raise SystemExit('Launcher already active')
    if not (root/'smoke_gate.json').exists():raise SystemExit('Smoke evidence gate missing')
    gate=json.loads((root/'smoke_gate.json').read_text())
    if not gate.get('passed'):raise SystemExit('Smoke gate did not pass')
    gpus=[g for g in inventory() if '3060' in g['name'] or 'P100' in g['name']]
    if len(gpus)!=2:raise SystemExit('Expected RTX 3060 + P100')
    atomic(root/'launcher_inventory.json',gpus)
    pending=[]
    for arch in ['mlp','gru64','gru128','sparse_rnn','softlogic_rnn','fly_modular']:
        for seed in range(4):
            run=f'{arch}-s{seed}';directory=root/'runs'/run;directory.mkdir(parents=True,exist_ok=True)
            c=yaml.safe_load((Path(a.configs)/f'{arch}.yaml').read_text());c['seed']=seed
            # Prespecified density allocation: two seeds each, report as subgroups.
            if arch=='sparse_rnn':c['density']=.1 if seed<2 else .25
            cfg=directory/'launch.yaml';cfg.write_text(yaml.safe_dump(c))
            status=directory/'status.json'
            if status.exists():
                previous=json.loads(status.read_text())
                if previous['status']=='complete':continue
                if previous['status'] in ['running','paused']:
                    try:os.kill(previous['pid'],0);raise SystemExit(f'{run}: existing live process, refusing duplicate')
                    except ProcessLookupError:pass
                if previous['status'] in ['failed','stopped']:continue
            atomic(status,{'run_id':run,'architecture':arch,'seed':seed,'status':'queued','training_step':0})
            pending.append((run,cfg))
    active={}
    while pending or active:
        for uuid,(proc,log,run) in list(active.items()):
            if proc.poll() is not None:
                log.close();del active[uuid]
                if proc.returncode:
                    path=root/'runs'/run/'status.json';s=json.loads(path.read_text());s.update(status='failed',exit_code=proc.returncode);atomic(path,s)
        for g in inventory():
            if g['uuid'] not in {x['uuid'] for x in gpus} or g['uuid'] in active or not pending:continue
            if int(g['free_mb'])<a.min_free_mb:continue
            # Do not launch on an externally occupied compute GPU.
            apps=subprocess.check_output(['nvidia-smi','--query-compute-apps=gpu_uuid','--format=csv,noheader'],text=True)
            if g['uuid'] in apps:continue
            run,cfg=pending.pop(0);env=dict(os.environ,CUDA_VISIBLE_DEVICES=g['uuid'],K0_CPU_THREADS='4')
            log=(root/'runs'/run/'train.log').open('a')
            proc=subprocess.Popen([sys.executable,'-m','experiments.k0_brainstem.train.trainer','--config',str(cfg),'--artifacts',str(root),'--run-id',run,'--device','cuda:0']+(['--resume'] if (root/'runs'/run/'checkpoint.pt').exists() else []),env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
            atomic(root/'runs'/run/'gpu.json',g);active[g['uuid']]=(proc,log,run)
            print(f'{run} -> {g["name"]} PID {proc.pid}',flush=True)
        atomic(root/'queue.json',{'pending':[x[0] for x in pending],'active':{u:v[2] for u,v in active.items()},'timestamp':time.time()})
        time.sleep(2)
    statuses=[json.loads(p.read_text()) for p in (root/'runs').glob('*/status.json') if not p.parent.name.startswith('smoke')]
    counts={k:sum(s.get('status')==k for s in statuses) for k in ['complete','failed','stopped','queued','running','paused']}
    atomic(root/'queue.json',{'pending':[],'active':{},'timestamp':time.time(),'finished':True,'counts':counts,'all_24_complete':counts['complete']==24 and len(statuses)==24})

if __name__=='__main__':main()
