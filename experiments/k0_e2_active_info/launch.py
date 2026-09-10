"""Finite, durable, one-process-per-physical-GPU runner; no service management."""
import argparse, concurrent.futures, fcntl, json, os, subprocess, sys, time
from pathlib import Path
from .train import atomic

GPU_UUIDS=['GPU-1d6afaad-67b7-b965-46c7-efe6c9c395f3','GPU-d11b6f7f-002d-b252-1921-149ed2d84f15']

def base_config(arch,seed,arm,smoke=False):
    return dict(architecture=arch,seed=seed,arm=arm,env={'episode_length':48},num_envs=32 if smoke else 1024,
                imitation_updates=2 if smoke else 256,ppo_updates=1 if smoke else 8,
                ppo_epochs=2,minibatch_envs=16 if smoke else 256,validation_envs=32 if smoke else 256)

def run_queue(artifacts,phase,smoke=False):
    root=Path(artifacts).resolve();root.mkdir(parents=True,exist_ok=True)
    lock=(root/'launcher.lock').open('w');fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    apps=subprocess.check_output(['nvidia-smi','--query-compute-apps=gpu_uuid,pid','--format=csv,noheader'],text=True).strip()
    if apps:raise RuntimeError('GPUs occupied; will not terminate processes: '+apps)
    seeds=range(2) if smoke else range(8)
    jobs=[]
    if phase=='imitation':
        for seed in seeds:
            for arch in ['mlp','gru64','gru128']:jobs.append((arch,seed,'B0'))
    elif phase=='ppo':
        for seed in seeds:
            for arm in ['B1','B2','B3','B4','B5','B6','B7']:jobs.append(('gru128',seed,arm))
    else:raise ValueError(phase)
    began=time.time();results=[]
    def worker(gpu,assigned):
        rows=[]
        for arch,seed,arm in assigned:
            run_id=f'{arch}-s{seed}-{arm}';d=root/'runs'/run_id
            if (d/'status.json').exists():
                status=json.loads((d/'status.json').read_text())
                if status['status']=='complete':rows.append(dict(run_id=run_id,exit_code=0,skipped=True));continue
                rows.append(dict(run_id=run_id,exit_code=-1,error='existing incomplete run preserved'));continue
            c=base_config(arch,seed,arm,smoke);p=root/'configs'/(run_id+'.json');atomic(p,c)
            cmd=[sys.executable,'-m','experiments.k0_e2_active_info.train','--config',str(p),'--artifacts',str(root),'--run-id',run_id,'--device','cuda:0']
            if arm!='B0':cmd+=['--parent',str(root/'runs'/f'{arch}-s{seed}-B0'/'imitation_best.pt')]
            d.mkdir(parents=True,exist_ok=True)
            with (d/'train.log').open('a') as log:
                process=subprocess.Popen(cmd,env=dict(os.environ,CUDA_VISIBLE_DEVICES=gpu),stdout=log,stderr=subprocess.STDOUT)
                code=process.wait()
            row=dict(run_id=run_id,exit_code=code,gpu_uuid=gpu);rows.append(row)
            atomic(root/f'worker-{GPU_UUIDS.index(gpu)}-{phase}.json',rows)
        return rows
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures=[pool.submit(worker,gpu,[job for job in jobs if job[1]%2==i]) for i,gpu in enumerate(GPU_UUIDS)]
        while not all(f.done() for f in futures):
            telemetry=subprocess.check_output(['nvidia-smi','--query-gpu=uuid,utilization.gpu,memory.used,temperature.gpu,power.draw','--format=csv,noheader'],text=True).strip()
            with (root/'gpu_telemetry.jsonl').open('a') as f:f.write(json.dumps(dict(timestamp=time.time(),phase=phase,rows=telemetry.splitlines()))+'\n')
            time.sleep(5)
        for future in futures:results.extend(future.result())
    atomic(root/(phase+'_queue.json'),dict(phase=phase,started_at=began,finished_at=time.time(),jobs=results,complete=all(r['exit_code']==0 for r in results)))
    return all(r['exit_code']==0 for r in results)

def main():
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);p.add_argument('--phase',choices=['imitation','ppo'],required=True);p.add_argument('--smoke',action='store_true');a=p.parse_args()
    raise SystemExit(0 if run_queue(a.artifacts,a.phase,a.smoke) else 1)
if __name__=='__main__':main()
