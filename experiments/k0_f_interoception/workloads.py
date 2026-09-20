"""Bounded real matrix jobs and optional background contention; no privileged operations."""
import argparse, json, math, multiprocessing as mp, os, subprocess, time
from pathlib import Path

ACTIONS = ['RUN_CPU', 'RUN_RTX3060', 'RUN_P100']
GPU_NAMES = ['3060', 'P100']


def devices():
    import torch
    result=['cpu']
    for name in GPU_NAMES:
        matches=[i for i in range(torch.cuda.device_count()) if name in torch.cuda.get_device_name(i)]
        if len(matches)!=1: raise RuntimeError('Required GPU identity is not unique')
        result.append('cuda:'+str(matches[0]))
    return result


def gpu_temperatures():
    r=subprocess.run(['nvidia-smi','--query-gpu=temperature.gpu','--format=csv,noheader,nounits'],capture_output=True,text=True,timeout=5,check=True)
    vals=[float(v) for v in r.stdout.splitlines()]
    if not vals or not all(math.isfinite(v) for v in vals): raise RuntimeError('GPU thermal sensor unavailable')
    return vals


def job_worker(connection, device):
    import torch
    torch.set_num_threads(2)
    torch.backends.cuda.matmul.allow_tf32=False
    torch.backends.cudnn.allow_tf32=False
    cache={}
    def job(n,reps):
        if n not in cache:
            g=torch.Generator().manual_seed(n)
            a=torch.randn((n,n),generator=g)*.02
            b=torch.randn((n,n),generator=g)*.02
            cache[n]=(a.to(device),b.to(device),torch.empty((n,n),device=device))
        a,b,c=cache[n]
        if device!='cpu':torch.cuda.synchronize(device)
        start=time.perf_counter()
        for _ in range(reps):torch.mm(a,b,out=c)
        if device!='cpu':torch.cuda.synchronize(device)
        elapsed=time.perf_counter()-start
        finite=bool(torch.isfinite(c[0,:]).all())
        return {'compute_seconds':elapsed,'finite':finite,'checksum':float(c[0,:].double().sum()),'allocated_bytes':int(torch.cuda.memory_allocated(device)) if device!='cpu' else n*n*4*3}
    for n in (128,512,1024,2048):job(n,1)
    connection.send({'ready':True,'device':device})
    while True:
        message=connection.recv()
        if message is None:break
        try:
            start=time.time()
            out=job(int(message['matrix_dimension']),int(message['repetitions']))
            out.update(start_timestamp=start,end_timestamp=time.time(),ok=True)
        except Exception as exc:out={'ok':False,'error_type':type(exc).__name__}
        connection.send(out)
    connection.close()


class JobPool:
    def __init__(self):
        self.ctx=mp.get_context('spawn'); self.workers=[]; self.pipes=[]
        try:
            for device in devices():
                parent,child=self.ctx.Pipe()
                p=self.ctx.Process(target=job_worker,args=(child,device),daemon=True);p.start();child.close()
                self.workers.append(p);self.pipes.append(parent)
            for c in self.pipes:
                if not c.poll(90):raise TimeoutError('worker initialization timed out')
                if not c.recv().get('ready'):raise RuntimeError('worker not ready')
        except BaseException:self.close();raise
    def run(self, action, task):
        c=self.pipes[action];start=time.perf_counter();c.send(task)
        if not c.poll(20):raise TimeoutError('bounded foreground job exceeded 20 seconds')
        out=c.recv();out['latency_seconds']=time.perf_counter()-start
        return out
    def close(self):
        for p,c in zip(self.workers,self.pipes):
            if p.is_alive():
                try:c.send(None)
                except (BrokenPipeError,EOFError):pass
        for p in self.workers:
            p.join(timeout=3)
            if p.is_alive():p.terminate();p.join(timeout=3)
            if p.is_alive():p.kill();p.join(timeout=3)
            if p.is_alive():raise RuntimeError('Owned job worker did not exit')
        for c in self.pipes:c.close()
    def __enter__(self):return self
    def __exit__(self,*_):self.close()


def background(device, seconds, duty=.4, stop_path=None, ready_path=None):
    """At most 50% wall-time duty, < 256 MiB matrices, stop before 75 Celsius."""
    import torch
    if not 0 < duty <= .5:raise ValueError('Duty must be in (0,.5]')
    torch.set_num_threads(2);torch.backends.cuda.matmul.allow_tf32=False
    n=2048 if device!='cpu' else 512
    a=torch.full((n,n),.01,device=device);b=a.clone();out=a.clone()
    if device != "cpu" and max(gpu_temperatures()) >= 75:raise RuntimeError("Thermal safety stop before workload")
    if ready_path:Path(ready_path).write_text(json.dumps({"ready":True,"pid":os.getpid(),"timestamp":time.time(),"duty_limit":duty}))
    deadline=time.monotonic()+min(seconds,1800);next_guard=0.;cycles=0;active=0.
    while time.monotonic()<deadline:
        if stop_path and Path(stop_path).exists():break
        if device!='cpu' and time.monotonic()>=next_guard:
            if max(gpu_temperatures())>=75:raise RuntimeError('Thermal safety stop at 75C')
            next_guard=time.monotonic()+2
        started=time.monotonic()
        while time.monotonic()-started<.04:
            torch.mm(a,b,out=out)
            if device!='cpu':torch.cuda.synchronize(device)
        duration=time.monotonic()-started;active+=duration;cycles+=1
        time.sleep(duration*(1-duty)/duty)
    return {'cycles':cycles,'active_seconds':active,'duty_limit':duty,'matrix_bytes':3*n*n*4}


def main():
    p=argparse.ArgumentParser();s=p.add_subparsers(dest='cmd',required=True)
    b=s.add_parser('background');b.add_argument('--device',required=True);b.add_argument('--seconds',type=float,default=30);b.add_argument('--duty',type=float,default=.4);b.add_argument('--stop-path');b.add_argument('--ready-path')
    q=s.add_parser('pilot');q.add_argument('--busy-action',type=int,choices=[1,2]);q.add_argument('--output',type=Path,required=True);q.add_argument('--repeats',type=int,default=5)
    args=p.parse_args()
    if args.cmd=='background':print(json.dumps(background(args.device,args.seconds,args.duty,args.stop_path,args.ready_path)));return
    rows=[]; busy=None
    if args.busy_action is not None:
        import sys
        busy=subprocess.Popen([sys.executable,'-m',__name__ if __name__!='__main__' else 'experiments.k0_f_interoception.workloads','background','--device',devices()[args.busy_action],'--seconds','40','--duty','.5'],stdout=subprocess.DEVNULL)
        time.sleep(5)
    with JobPool() as pool:
        for n,reps in [(128,8),(512,16),(1024,16),(2048,8)]:
            for repeat in range(args.repeats):
                for action in range(3):
                    result=pool.run(action,{'matrix_dimension':n,'repetitions':reps})
                    rows.append(dict(matrix_dimension=n,repetitions=reps,repeat=repeat,action=action,**result))
    if busy is not None:
        busy.terminate();busy.wait(timeout=5)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(rows,indent=2,allow_nan=False)+'\n')
    for n in sorted({x['matrix_dimension'] for x in rows}):
        print(n,{ACTIONS[a]:sum(x['latency_seconds'] for x in rows if x['action']==a and x['matrix_dimension']==n)/args.repeats for a in range(3)})

if __name__=='__main__':main()
