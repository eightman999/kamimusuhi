"""Bounded disk activity in an owned scratch file only."""
import argparse,json,os,time,signal
from pathlib import Path


def main():
    p=argparse.ArgumentParser();p.add_argument('kind',choices=['disk']);p.add_argument('--seconds',type=float,default=20);p.add_argument('--scratch',type=Path);p.add_argument('--ready-path',type=Path)
    a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt("owned auxiliary interrupted")
    signal.signal(signal.SIGTERM,stop)
    if a.kind=='disk':
        if not a.scratch:raise ValueError('Dedicated scratch path required')
        a.scratch.parent.mkdir(parents=True,exist_ok=True)
        if a.scratch.exists():raise FileExistsError('Refusing to overwrite existing scratch')
        data=b'k0f2-scratch-26\0'*(1024*1024)
        until=time.monotonic()+a.seconds
        try:
            with a.scratch.open('xb') as f:f.write(data)
            if a.ready_path:a.ready_path.write_text(json.dumps({'ready':True,'pid':os.getpid(),'timestamp':time.time()}))
            while time.monotonic()<until:
                with a.scratch.open('wb') as f:f.write(data);f.flush();os.fsync(f.fileno())
                with a.scratch.open('rb') as f:f.read()
                time.sleep(.4)
        finally:a.scratch.unlink(missing_ok=True)

if __name__=='__main__':main()
