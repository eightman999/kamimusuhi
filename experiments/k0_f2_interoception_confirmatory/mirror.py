"""Finite read-only artifact mirror for the Mac body dashboard."""
import argparse,json,subprocess,time
from pathlib import Path


def main():
    p=argparse.ArgumentParser();p.add_argument('--ssh-alias',required=True);p.add_argument('--remote-artifacts',required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--duration',type=float,default=1800);p.add_argument('--interval',type=float,default=5);a=p.parse_args()
    a.output.mkdir(parents=True,exist_ok=True);end=time.monotonic()+a.duration;count=0;errors=0
    try:
        while time.monotonic()<end:
            started=time.monotonic()
            r=subprocess.run(['rsync','-az','--include=/*.jsonl','--include=/*.json','--exclude=*',a.ssh_alias+':'+a.remote_artifacts.rstrip('/')+'/',str(a.output)+'/'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=20)
            count+=1;errors+=int(r.returncode!=0)
            if (a.output/'collection_runtime_state.json').exists():break
            time.sleep(max(0,a.interval-(time.monotonic()-started)))
    finally:
        (a.output/'dashboard_mirror_state.json').write_text(json.dumps(dict(timestamp=time.time(),iterations=count,errors=errors,complete=True,read_only_remote=True,connection_values_excluded=True),indent=2)+'\n')

if __name__=='__main__':main()
