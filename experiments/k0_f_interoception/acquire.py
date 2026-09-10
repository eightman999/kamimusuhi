"""Real telemetry alignment and bounded, block-separated hardware job measurement."""
import argparse,bisect,copy,hashlib,json,os,random,signal,subprocess,sys,threading,time
from pathlib import Path
from .workloads import JobPool,devices,gpu_temperatures
from .normalize import align_records,DEFAULT_CONFIG

TASKS=[dict(matrix_dimension=n,repetitions=r,deadline_seconds=d) for n,r,d in [(128,8,.001),(512,16,.004),(1024,16,.015),(2048,8,.05)]]
LABELS=['idle','cpu_light','rtx3060_matrix','p100_matrix','dual_gpu','mixed','disk_io','network_transfer']


def save(path,data):
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True)
    tmp=path.with_suffix(path.suffix+'.tmp');tmp.write_text(json.dumps(data,indent=2,allow_nan=False)+'\n');tmp.replace(path)


def emit(path,data):
    with Path(path).open('a') as f:f.write(json.dumps(data,allow_nan=False,separators=(',',':'))+'\n')


def read_new(file,position):
    if not file.exists():return [],position
    records=[]
    with file.open() as f:
        f.seek(position)
        while True:
            start=f.tell();line=f.readline()
            if not line or not line.endswith('\n'):return records,start
            records.append(json.loads(line))


def runtime_state():
    import psutil
    gpu=subprocess.run(['nvidia-smi','--query-gpu=name,temperature.gpu,utilization.gpu,memory.used,memory.total,power.draw','--format=csv,noheader'],capture_output=True,text=True,timeout=10)
    proc=subprocess.run(['nvidia-smi','--query-compute-apps=pid','--format=csv,noheader'],capture_output=True,text=True,timeout=10)
    services={}
    for s in ['llama-master.service','k0-j72-eval.service','k0-e2-j72-eval.service']:
        r=subprocess.run(['systemctl','--user','is-active',s],capture_output=True,text=True);services[s]=r.stdout.strip()
    return dict(timestamp=time.time(),gpu_summary=gpu.stdout.strip().splitlines(),compute_processes=proc.stdout.strip().splitlines(),services=services,cpu_percent=psutil.cpu_percent(interval=.2),load_average=list(os.getloadavg()),memory_available_bytes=psutil.virtual_memory().available)


class Aligner:
    def __init__(self,out):
        self.out=out;self.frames=[];self.master=[];self.mac=[];self.done=threading.Event();self.error=None
        self.thread=threading.Thread(target=self.loop,daemon=True)
    def loop(self):
        mp=ap=0
        try:
            while not self.done.is_set():
                ms,mp=read_new(self.out/'raw_master_telemetry.jsonl',mp);self.master.extend(ms)
                xs,ap=read_new(self.out/'raw_mac_telemetry.jsonl',ap);self.mac.extend(xs)
                now=time.time();m=self.master[-1] if self.master else None;x=self.mac[-1] if self.mac else None
                if m is not None:
                    a=align_records(m,x,now=now);frame=a['frame'];frame['frame_id']='frame-'+str(len(self.frames));self.frames.append(frame)
                    emit(self.out/'aligned_body_telemetry.jsonl',a['aligned'])
                    emit(self.out/'interoceptive_frames.jsonl',frame)
                    emit(self.out/'policy_inputs.jsonl',{'frame_id':frame['frame_id'],'timestamp':now,'input':a['policy_input']['values'],'source_kind':'real'})
                self.done.wait(1)
        except BaseException as e:self.error=e
    def start(self):self.thread.start()
    def close(self):self.done.set();self.thread.join(timeout=5)
    def sequence(self,when=None,length=8):
        fs=self.frames.copy();when=time.time() if when is None else when
        end=bisect.bisect_right([f['timestamp'] for f in fs],when)
        return fs[max(0,end-length):end]
    def guard(self):
        if (self.out/'STOP').exists():raise RuntimeError('Acquisition stop requested')
        if self.error:raise RuntimeError('Alignment failed') from self.error
        if not self.master or time.time()-self.master[-1]['timestamp']>5:raise RuntimeError('Master telemetry stale; workload stopped')
        if not self.mac or time.time()-self.mac[-1].get('receipt_timestamp',self.mac[-1]['timestamp'])>8:raise RuntimeError('Mac telemetry stale; workload stopped')
        m=self.master[-1]['metrics'];a=self.mac[-1]['metrics']
        for name in ['rtx3060_temperature_c','p100_temperature_c']:
            if m.get(name) is None or m[name]>=75:raise RuntimeError('GPU thermal guard')
        if m.get('cpu_temperature_c') is not None and m['cpu_temperature_c']>=80:raise RuntimeError('CPU thermal guard')
        if a.get('thermal_pressure',0)>=2/3:raise RuntimeError('Mac thermal guard')
        if m.get('memory_available_bytes',0)<4*1024**3:raise RuntimeError('RAM headroom guard')


class Background:
    def __init__(self,label,duration,out,block_id='pilot'):
        self.children=[];self.files=[];self.ready=[];self.closed=False
        logs=out/'workload_logs';logs.mkdir(exist_ok=True)
        ds=devices();indices={'cpu_light':[0],'rtx3060_matrix':[1],'p100_matrix':[2],'dual_gpu':[1,2],'mixed':[0,1]}.get(label,[])
        commands=[]
        for index in indices:
            commands.append([sys.executable,'-m','experiments.k0_f_interoception.workloads','background','--device',ds[index],'--seconds',str(duration),'--duty','.5'])
        if label=='disk_io':
            scratch=out/'scratch'/('k0f-'+block_id+'.bin');scratch.parent.mkdir(exist_ok=True)
            if scratch.exists():raise FileExistsError('Refusing existing scratch')
            commands.append([sys.executable,'-m','experiments.k0_f_interoception.auxiliary','disk','--seconds',str(duration),'--scratch',str(scratch)])
        try:
            for i,cmd in enumerate(commands):
                ready=logs/(block_id+'-'+str(i)+'-ready.json')
                if ready.exists():raise FileExistsError('Refusing reused workload identity')
                log=(logs/(block_id+'-'+str(i)+'.log')).open('w');self.files.append(log);self.ready.append(ready)
                self.children.append(subprocess.Popen(cmd+['--ready-path',str(ready)],stdout=log,stderr=log))
            deadline=time.monotonic()+15
            while not all(p.exists() for p in self.ready):
                self.check()
                if time.monotonic()>deadline:raise TimeoutError('Background readiness timeout')
                time.sleep(.1)
        except BaseException:self.close();raise
    def check(self):
        for p in self.children:
            if p.poll() is not None:raise RuntimeError('Background exited before block completion; consult workload log')
    def evidence(self):
        return [dict(pid=p.pid,returncode=p.poll(),ready=json.loads(f.read_text()) if f.exists() else None) for p,f in zip(self.children,self.ready)]
    def close(self):
        if self.closed:return
        self.closed=True
        for p in self.children:
            if p.poll() is None:p.terminate()
        for p in self.children:
            try:p.wait(timeout=5)
            except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
        for f in self.files:f.close()


def receive(args):
    args.output.parent.mkdir(parents=True,exist_ok=True)
    for line in sys.stdin:
        raw=json.loads(line)
        if raw.get('schema_version')!='k0f.raw.v1' or raw.get('node_id')!='mac':raise ValueError('Unexpected telemetry schema')
        raw['receipt_timestamp']=time.time();emit(args.output,raw)


def collect(args):
    out=args.output;out.mkdir(parents=True,exist_ok=True)
    if (out/'workload_manifest.json').exists():raise FileExistsError('Use a fresh collection directory')
    before=runtime_state();save(out/'initial_runtime_state.json',before)
    save(out/'driver_identity.json',{'pid':os.getpid(),'started_at':time.time(),'owned_by':'k0-f-acquire'})
    save(out/'normalization_config.json',DEFAULT_CONFIG)
    save(out/'collection_config.json',dict(schema='k0-f-collection-v1',tasks=TASKS,block_seconds=args.block_seconds,blocks=args.blocks,seed=260910,split_blocks={'train':24,'validation':8,'test':16},source_commit=args.source_commit))
    sensor=None;aligner=Aligner(out);background=None;manifest=[];rows=[];ok=False
    stop_file=out/'STOP'
    try:
        sensor=subprocess.Popen([sys.executable,'-m','experiments.k0_f_interoception.sensors','--duration',str(args.blocks*args.block_seconds+300),'--interval','1','--output',str(out/'raw_master_telemetry.jsonl')],stdout=subprocess.DEVNULL)
        aligner.start()
        until=time.monotonic()+45
        while (not aligner.mac or not aligner.master) and time.monotonic()<until:time.sleep(.5)
        aligner.guard()
        with JobPool() as pool:
            # Record-only warm-up provides actual history before first decision.
            start=time.monotonic()
            while time.monotonic()-start<35:aligner.guard();time.sleep(1)
            for block in range(args.blocks):
                if stop_file.exists():raise RuntimeError('User stop request')
                if block in (24,32):
                    for _ in range(35):aligner.guard();time.sleep(1)
                split='train' if block<24 else ('validation' if block<32 else 'test')
                rng=random.Random(260910+block)
                labels=LABELS.copy();random.Random(260910+block//8).shuffle(labels)
                label=labels[block%8];block_id=f'{split}-{block:03d}'
                start=time.time();background=Background(label,args.block_seconds+20,out,block_id)
                entry=dict(block_id=block_id,split=split,workload_label=label,start_timestamp=start,source_kind='real',tasks=[])
                print(json.dumps(dict(event='block_start',block=block,block_id=block_id,label=label,seconds=args.block_seconds)),flush=True)
                # Allow one sensor window after process/model startup.
                for _ in range(5):aligner.guard();background.check();time.sleep(1)
                task_order=list(range(4))*2;rng.shuffle(task_order)
                for decision,task_id in enumerate(task_order):
                    aligner.guard();background.check();task=TASKS[task_id];when=time.time()
                    seq=aligner.sequence(length=1+rng.randrange(8));stale=copy.deepcopy(aligner.sequence(when-30,length=len(seq)))
                    for f in stale:f['values'][18]=min(1.,f['values'][18]+max(0,when-f['timestamp'])/60.)
                    if not seq:raise RuntimeError('No policy frame')
                    if not stale:stale=[dict(values=[0.]*20,mask=[0.]*20,frame_id='missing',timestamp=when-30)]
                    action_order=list(range(3));rng.shuffle(action_order);results={}
                    for action in action_order:
                        results[action]=pool.run(action,task)
                    lat=[results[a]['latency_seconds'] for a in range(3)]
                    fail=[not results[a].get('ok') or not results[a].get('finite') for a in range(3)]
                    row=dict(schema_version='k0-f-policy-v1',episode_id=f'{block_id}-{decision:02d}',block_id=block_id,split=split,timestamp=when,task_features=[task['matrix_dimension']/3072,task['repetitions']/20,task['deadline_seconds']/5,0.],body_sequence=[f['values'] for f in seq],body_mask_sequence=[f['mask'] for f in seq],stale_body_sequence=[f['values'] for f in stale],stale_body_mask_sequence=[f['mask'] for f in stale],costs_seconds=lat,failures=fail,deadline_seconds=task['deadline_seconds'],probe_targets={},provenance=dict(telemetry_kind='real',frame_ids=[f['frame_id'] for f in seq],stale_frame_ids=[f['frame_id'] for f in stale],stale_age_seconds=when-stale[-1]['timestamp']),task=task,action_order=action_order,measurements=[results[a] for a in range(3)])
                    rows.append(row);emit(out/'measured_jobs.jsonl',row);entry['tasks'].append(row['episode_id'])
                    target=start+5+(decision+1)*(args.block_seconds-5)/len(task_order)
                    while time.time()<target:aligner.guard();background.check();time.sleep(max(0,min(.25,target-time.time())))
                background.check();background.close();entry['workload_processes']=background.evidence();background=None;entry['end_timestamp']=time.time();manifest.append(entry);save(out/'workload_manifest.json',manifest)
            # Future-target observation; no extra training blocks.
            for _ in range(12):aligner.guard();time.sleep(1)
        fs=aligner.frames
        for row in rows:
            future=next((f for f in fs if f['provenance']['master']['timestamp']>=row['timestamp']+10),None)
            row['probe_targets']={'future_rtx3060_util':future['values'][5] if future and future['mask'][5] else None,'future_p100_util':future['values'][9] if future and future['mask'][9] else None,'next_job_latency_seconds':min(row['costs_seconds'])}
            row['provenance']['future_target_source_timestamp']=future['provenance']['master']['timestamp'] if future else None
            row['provenance']['probe_target_kinds']={'future_rtx3060_util':'real_body_at_least_10s_after_decision','future_p100_util':'real_body_at_least_10s_after_decision','next_job_latency_seconds':'minimum_of_three_real_jobs_started_after_current_body_observation'}
            emit(out/'dataset.jsonl',row)
        ok=True
    finally:
        if background:background.close()
        aligner.close()
        if sensor and sensor.poll() is None:
            sensor.terminate()
            try:sensor.wait(timeout=5)
            except subprocess.TimeoutExpired:sensor.kill();sensor.wait(timeout=5)
        after=runtime_state();after.update(collection_complete=ok,owned_sensor_stopped=sensor is None or sensor.poll() is not None,owned_background_stopped=background is None or all(p.poll() is not None for p in background.children),scratch_files=list(str(p.relative_to(out)) for p in (out/'scratch').glob('*')) if (out/'scratch').exists() else [])
        save(out/'collection_runtime_state.json',after)
        save(out/'workload_manifest.json',manifest)
    print(json.dumps(dict(event='collection_complete',rows=len(rows),blocks=len(manifest))),flush=True)


def main():
    p=argparse.ArgumentParser();s=p.add_subparsers(dest='cmd',required=True)
    r=s.add_parser('receive');r.add_argument('--output',required=True,type=Path)
    c=s.add_parser('collect');c.add_argument('--output',required=True,type=Path);c.add_argument('--blocks',type=int,default=48);c.add_argument('--block-seconds',type=float,default=20);c.add_argument('--source-commit',required=True)
    a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt('bounded acquisition interrupted')
    signal.signal(signal.SIGTERM,stop)
    receive(a) if a.cmd=='receive' else collect(a)

if __name__=='__main__':main()
