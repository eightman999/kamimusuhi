"""Separate record-only execution of the already installed J72 model."""
import argparse,json,signal,subprocess,sys,time
from pathlib import Path
from .acquire import Aligner,runtime_state,save
from .workloads import devices


def collect(a):
    out=a.output;out.mkdir(parents=True,exist_ok=True)
    if (out/'driver_identity.json').exists():raise FileExistsError('Use a fresh LLM recording directory')
    save(out/'driver_identity.json',dict(pid=__import__('os').getpid(),started_at=time.time(),owned_by='k0-f-acquire'))
    save(out/'initial_runtime_state.json',runtime_state())
    align=Aligner(out);sensor=None;worker=None;manifest=[];ok=False
    try:
        sensor=subprocess.Popen([sys.executable,'-m','experiments.k0_f_interoception.sensors','--duration','240','--interval','1','--output',str(out/'raw_master_telemetry.jsonl')],stdout=subprocess.DEVNULL)
        align.start();until=time.monotonic()+45
        while (not align.mac or not align.master) and time.monotonic()<until:time.sleep(.5)
        align.guard()
        for index,device in enumerate(devices()[1:]):
            started=time.time();label=['rtx3060_inference','p100_inference'][index];output=out/(label+'.json')
            print(json.dumps(dict(event='block_start',block=index,block_id=label,label=label,seconds=20)),flush=True)
            with (out/(label+'.log')).open('w') as log:
                worker=subprocess.Popen([a.model_python,'-m','experiments.k0_f_interoception.auxiliary','llm','--seconds','20','--model-loader',a.model_loader,'--device',device,'--output',str(output)],stdout=log,stderr=log)
                deadline=time.monotonic()+90
                while worker.poll() is None:
                    align.guard()
                    if time.monotonic()>deadline:raise TimeoutError('LLM workload exceeded finite runtime')
                    time.sleep(.5)
            if worker.returncode!=0:raise RuntimeError('J72 record-only workload failed; inspect dedicated log')
            measurements=json.loads(output.read_text());manifest.append(dict(workload_label=label,start_timestamp=started,end_timestamp=time.time(),source_kind='real',model='existing_j72',requests=len(measurements),record_only=True,pid=worker.pid,returncode=worker.returncode))
            worker=None;print(json.dumps(dict(event='block_end',block_id=label)),flush=True)
            for _ in range(5):align.guard();time.sleep(1)
        ok=True
    finally:
        if worker and worker.poll() is None:worker.terminate();worker.wait(timeout=5)
        align.close()
        if sensor and sensor.poll() is None:sensor.terminate();sensor.wait(timeout=5)
        save(out/'workload_manifest.json',manifest)
        state=runtime_state();state.update(collection_complete=ok,owned_sensor_stopped=sensor is None or sensor.poll() is not None,owned_background_stopped=worker is None or worker.poll() is not None)
        save(out/'collection_runtime_state.json',state)
    print(json.dumps(dict(event='collection_complete',blocks=len(manifest))),flush=True)


def main():
    p=argparse.ArgumentParser();p.add_argument('cmd',choices=['collect']);p.add_argument('--output',type=Path,required=True);p.add_argument('--source-commit',required=True);p.add_argument('--blocks',type=int,default=2);p.add_argument('--block-seconds',type=float,default=60);p.add_argument('--model-python',required=True);p.add_argument('--model-loader',required=True);a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt('LLM record-only interrupted')
    signal.signal(signal.SIGTERM,stop);collect(a)

if __name__=='__main__':main()
