"""Mac-side private SSH transport and owned-process lifecycle for finite acquisition."""
import argparse,json,os,signal,subprocess,sys,threading,time
from pathlib import Path


def main():
    p=argparse.ArgumentParser();p.add_argument('--ssh-alias',required=True);p.add_argument('--peer-host',required=True);p.add_argument('--remote-root',required=True);p.add_argument('--remote-python',required=True);p.add_argument('--sensor-binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--remote-output',required=True);p.add_argument('--blocks',type=int,default=48);p.add_argument('--block-seconds',type=float,default=20);p.add_argument('--source-commit',required=True)
    a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
    if (a.output/'transport_state.json').exists():raise FileExistsError('Use fresh transport output')
    import shlex
    cmd=lambda words:' '.join(shlex.quote(str(w)) for w in words)
    remote=lambda tail:'cd '+shlex.quote(a.remote_root)+' && '+cmd(tail)
    processes=[];threads=[];network_done=threading.Event();network_rows=[];success=False
    def stop(*_):raise KeyboardInterrupt('finite collection stopped')
    signal.signal(signal.SIGTERM,stop)
    def transfer(seconds):
        end=time.monotonic()+seconds;count=0;payload=bytes(4*1024*1024)
        while time.monotonic()<end and not network_done.is_set():
            started=time.time()
            r=subprocess.run(['ssh','-o','ConnectTimeout=5',a.ssh_alias,'cat > /dev/null'],input=payload,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
            network_rows.append(dict(timestamp=started,bytes=len(payload),ok=r.returncode==0,duration_seconds=time.time()-started,source_kind='real'))
            count+=1;network_done.wait(.5)
    source=None;receiver=None;driver=None;mac_log=None;remote_log=None
    try:
        receiver=subprocess.Popen(['ssh',a.ssh_alias,remote([a.remote_python,'-m','experiments.k0_f_interoception.acquire','receive','--output',a.remote_output+'/raw_mac_telemetry.jsonl'])],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE);processes.append(receiver)
        source=subprocess.Popen([str(a.sensor_binary),'--duration',str(a.blocks*a.block_seconds+350),'--interval','1','--peer-host',a.peer_host],stdout=subprocess.PIPE,stderr=subprocess.PIPE);processes.append(source)
        mac_log=(a.output/'raw_mac_source.jsonl').open('wb')
        def forward():
            for line in source.stdout:
                mac_log.write(line);mac_log.flush();receiver.stdin.write(line);receiver.stdin.flush()
        t=threading.Thread(target=forward,daemon=True);t.start();threads.append(t)
        driver=subprocess.Popen(['ssh',a.ssh_alias,remote([a.remote_python,'-m','experiments.k0_f_interoception.acquire','collect','--output',a.remote_output,'--blocks',a.blocks,'--block-seconds',a.block_seconds,'--source-commit',a.source_commit])],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);processes.append(driver)
        remote_log=(a.output/'collection.log').open('w')
        for line in driver.stdout:
            remote_log.write(line);remote_log.flush();print(line.strip(),flush=True)
            try:event=json.loads(line)
            except json.JSONDecodeError:continue
            if event.get('label')=='network_transfer':
                t=threading.Thread(target=transfer,args=(event['seconds'],),daemon=True);t.start();threads.append(t)
        code=driver.wait();success=code==0
        if not success:raise RuntimeError('Remote collection failed; see local log')
    finally:
        if driver and driver.poll() is None:
            request=remote([a.remote_python,'-c',"from pathlib import Path; import sys; Path(sys.argv[1]).write_text('owned acquisition stop\\n')",a.remote_output+'/STOP'])
            subprocess.run(['ssh','-o','ConnectTimeout=5',a.ssh_alias,request],capture_output=True,timeout=10)
            try:driver.wait(timeout=30)
            except subprocess.TimeoutExpired:
                force=remote([a.remote_python,'-c',"import json,os,signal,sys; from pathlib import Path; d=json.loads(Path(sys.argv[1]).read_text()); p=d['pid']; c=Path('/proc/'+str(p)+'/cmdline').read_bytes(); assert d['owned_by']=='k0-f-acquire' and b'experiments.k0_f_interoception.acquire' in c; os.kill(p,signal.SIGTERM)",a.remote_output+'/driver_identity.json'])
                subprocess.run(['ssh','-o','ConnectTimeout=5',a.ssh_alias,force],capture_output=True,timeout=10)
                driver.wait(timeout=15)
        network_done.set()
        if source and source.poll() is None:source.terminate()
        if source:
            try:source.wait(timeout=5)
            except subprocess.TimeoutExpired:source.kill();source.wait()
        for t in threads:t.join(timeout=12)
        if receiver and receiver.stdin:
            try:receiver.stdin.close()
            except BrokenPipeError:pass
        for proc in processes:
            if proc.poll() is None:
                try:proc.wait(timeout=5)
                except subprocess.TimeoutExpired:proc.terminate();proc.wait(timeout=5)
        if mac_log:mac_log.close()
        if remote_log:remote_log.close()
        (a.output/'network_workloads.json').write_text(json.dumps(network_rows,indent=2)+'\n')
        verification=subprocess.run(['ssh','-o','ConnectTimeout=5',a.ssh_alias,remote([a.remote_python,'-c',"from pathlib import Path; import sys; print(Path(sys.argv[1]).read_text())",a.remote_output+'/collection_runtime_state.json'])],capture_output=True,text=True,timeout=10)
        remote_state=json.loads(verification.stdout) if verification.returncode==0 else None
        (a.output/'transport_state.json').write_text(json.dumps(dict(timestamp=time.time(),success=success,remote_cleanup_state=remote_state,owned_processes=[dict(pid=proc.pid,returncode=proc.poll()) for proc in processes],sensor_source_retained=True,connection_values_excluded=True),indent=2)+'\n')

if __name__=='__main__':main()
