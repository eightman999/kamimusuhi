"""Finite record-only workloads, isolated scratch and pre-existing local LLM weights."""
import argparse,json,os,runpy,time,signal
from pathlib import Path


def main():
    p=argparse.ArgumentParser();p.add_argument('kind',choices=['disk','llm']);p.add_argument('--seconds',type=float,default=20);p.add_argument('--scratch',type=Path);p.add_argument('--model-loader');p.add_argument('--device',default='cpu');p.add_argument('--output',type=Path);p.add_argument('--ready-path',type=Path)
    a=p.parse_args();rows=[]
    def stop(*_):raise KeyboardInterrupt("owned auxiliary interrupted")
    signal.signal(signal.SIGTERM,stop)
    if a.kind=='disk':
        if not a.scratch:raise ValueError('Dedicated scratch path required')
        a.scratch.parent.mkdir(parents=True,exist_ok=True)
        if a.scratch.exists():raise FileExistsError('Refusing to overwrite existing scratch')
        data=b'k0f-scratch-2026\0'*(1024*1024)
        until=time.monotonic()+a.seconds
        try:
            with a.scratch.open('xb') as f:f.write(data)
            if a.ready_path:a.ready_path.write_text(json.dumps({'ready':True,'pid':os.getpid(),'timestamp':time.time()}))
            while time.monotonic()<until:
                with a.scratch.open('wb') as f:f.write(data);f.flush();os.fsync(f.fileno())
                with a.scratch.open('rb') as f:f.read()
                time.sleep(.4)
        finally:a.scratch.unlink(missing_ok=True)
    else:
        from .workloads import gpu_temperatures
        os.environ.update(J72_DEVICE=a.device,J72_HOST='127.0.0.1',J72_PORT='8081',J72_MAX_NEW_TOKENS='32')
        scope=runpy.run_path(a.model_loader)
        import torch
        torch.set_num_threads(2)
        deadline=time.monotonic()+a.seconds
        while time.monotonic()<deadline:
            if a.device!='cpu' and max(gpu_temperatures())>=75:raise RuntimeError('LLM thermal safety stop')
            start=time.time();text=scope['generate']('[user]\nThe machine is ready.\n[assistant]\n',32,0,1)
            rows.append({'timestamp':start,'latency_seconds':time.time()-start,'requested_new_tokens':32,'nonempty':bool(text),'source_kind':'real','workload':'existing_j72_generation'})
            time.sleep(1)
        if a.output:a.output.write_text(json.dumps(rows,indent=2,allow_nan=False)+'\n')

if __name__=='__main__':main()
