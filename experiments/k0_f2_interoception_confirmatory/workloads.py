"""Bounded real matrix jobs and optional background contention; no privileged operations."""
import argparse, json, math, multiprocessing as mp, os, subprocess, time
from pathlib import Path

ACTIONS = ['RUN_CPU', 'RUN_RTX3060', 'RUN_P100', 'WAIT']
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
            g=torch.Generator().manual_seed(20260911+n)
            a=torch.randn((n,n),generator=g)*.02
            b=torch.randn((n,n),generator=g)*.02
            cache[n]=(a.to(device),b.to(device),torch.empty((n,n),device=device))
        a,b,c=cache[n]
        if device!='cpu':torch.cuda.synchronize(device)
        start=time.perf_counter();start_timestamp=time.time()
        for _ in range(reps):torch.mm(a,b,out=c)
        if device!='cpu':torch.cuda.synchronize(device)
        completion_monotonic=time.perf_counter();completion_timestamp=time.time();elapsed=completion_monotonic-start
        validation_start=time.perf_counter()
        finite=bool(torch.isfinite(c).all());checksum=float(c[0,:].double().sum())
        return {'compute_seconds':elapsed,'start_timestamp':start_timestamp,'completion_timestamp':completion_timestamp,'completion_monotonic':completion_monotonic,'finite':finite,'checksum':checksum,'validation_seconds':time.perf_counter()-validation_start,'validation_scope':'entire_output_matrix_finite_and_first_row_checksum','allocated_bytes':int(torch.cuda.memory_allocated(device)) if device!='cpu' else n*n*4*3}
    for n in (128,512,1024,2048):job(n,1)
    connection.send({'ready':True,'device':device})
    while True:
        message=connection.recv()
        if message is None:break
        try:
            start=time.time()
            out=job(int(message['matrix_dimension']),int(message['repetitions']))
            out.update(end_timestamp=time.time(),ok=True)
            if device!='cpu' and out['allocated_bytes']>256*1024**2:raise RuntimeError('GPU tensor allocation guard')
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
        if action not in range(4):raise ValueError('Unknown fixed F2 action')
        dispatch=time.time();tick=time.perf_counter();wait=.02 if action==3 else 0.
        if wait:time.sleep(wait)
        resource=1 if action==3 else action
        c=self.pipes[resource];c.send(task)
        if not c.poll(20):raise TimeoutError('bounded foreground job exceeded 20 seconds')
        out=c.recv();out['roundtrip_seconds']=time.perf_counter()-tick
        out['latency_seconds']=out.get('completion_monotonic',time.perf_counter())-tick
        out.update(action=action,actual_resource=resource,wait_seconds=wait,dispatch_timestamp=dispatch,dispatch_monotonic=tick)
        if not math.isfinite(out['latency_seconds']) or out['latency_seconds']<=0:raise ValueError('Invalid real completion latency')
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
    p=argparse.ArgumentParser();p.add_argument('cmd',choices=['background']);p.add_argument('--device',required=True);p.add_argument('--seconds',type=float,required=True);p.add_argument('--duty',type=float,default=.5);p.add_argument('--stop-path');p.add_argument('--ready-path')
    a=p.parse_args();print(json.dumps(background(a.device,a.seconds,a.duty,a.stop_path,a.ready_path)))

if __name__=='__main__':main()
