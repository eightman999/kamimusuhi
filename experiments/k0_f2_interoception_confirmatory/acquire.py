"""Real telemetry alignment and bounded, block-separated hardware job measurement."""
import argparse,bisect,copy,hashlib,json,os,random,signal,subprocess,sys,threading,time
from pathlib import Path
from .workloads import JobPool,devices,gpu_temperatures
from .normalize import align_records,DEFAULT_CONFIG

from .protocol import TASKS,LABELS,SESSIONS,task_features,load_lock,lock_sha256,lock_receipt


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
    def __init__(self,out,session_id,lock_identity):
        self.out=out;self.session_id=session_id;self.lock_identity=lock_identity;self.frames=[];self.master=[];self.mac=[];self.done=threading.Event();self.error=None
        self.thread=threading.Thread(target=self.loop,daemon=True)
    def loop(self):
        mp=ap=0
        try:
            while not self.done.is_set():
                ms,mp=read_new(self.out/'raw_master_telemetry.jsonl',mp);self.master.extend(ms)
                xs,ap=read_new(self.out/'raw_mac_telemetry.jsonl',ap);self.mac.extend(xs)
                now=time.time();m=self.master[-1] if self.master else None;x=self.mac[-1] if self.mac else None
                if m is not None:
                    a=align_records(m,x,now=now);frame=a['frame'];frame['frame_id']=self.session_id+'-frame-'+str(len(self.frames));frame['session_id']=self.session_id;frame['protocol_lock_sha256']=self.lock_identity;self.frames.append(frame)
                    emit(self.out/'aligned_telemetry.jsonl',a['aligned'])
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
            commands.append([sys.executable,'-m','experiments.k0_f2_interoception_confirmatory.workloads','background','--device',ds[index],'--seconds',str(duration),'--duty','.5'])
        if label=='disk_io':
            scratch=out/'scratch'/('k0f2-'+block_id+'.bin');scratch.parent.mkdir(exist_ok=True)
            if scratch.exists():raise FileExistsError('Refusing existing scratch')
            commands.append([sys.executable,'-m','experiments.k0_f2_interoception_confirmatory.auxiliary','disk','--seconds',str(duration),'--scratch',str(scratch)])
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


def observed_sequence(aligner, when, length):
    seq=aligner.sequence(when,length);stale=copy.deepcopy(aligner.sequence(when-30,length))
    if len(seq)!=length or len(stale)!=length:raise RuntimeError('Insufficient current/stale history')
    for f in stale:f['values'][18]=min(1.,f['values'][18]+max(0,when-f['timestamp'])/60.)
    return seq,stale


def row_inputs(aligner, *, when, length, task, task_id, session_id, split, block_id, episode_id, lock_identity):
    seq,stale=observed_sequence(aligner,when,length)
    return dict(schema_version='k0-f2-policy-v1',source_kind='real',episode_id=episode_id,block_id=block_id,session_id=session_id,split=split,timestamp=when,protocol_lock_sha256=lock_identity,task_id=task_id,task=task,task_features=task_features(task),body_sequence=[f['values'] for f in seq],body_mask_sequence=[f['mask'] for f in seq],stale_body_sequence=[f['values'] for f in stale],stale_body_mask_sequence=[f['mask'] for f in stale],deadline_seconds=task['deadline_seconds'],provenance=dict(telemetry_kind='real',frame_ids=[f['frame_id'] for f in seq],stale_frame_ids=[f['frame_id'] for f in stale],frame_timestamps=[f['timestamp'] for f in seq],stale_frame_timestamps=[f['timestamp'] for f in stale],normalization_identity=seq[-1]['normalization_identity'],body_timestamp=seq[-1]['timestamp'],stale_age_seconds=when-stale[-1]['timestamp'],master_source_sequence=[f['provenance']['master']['sequence'] for f in seq],mac_source_sequence=[f['provenance']['mac']['sequence'] for f in seq]))


def body_snapshot(aligner):
    m=aligner.master[-1];a=aligner.mac[-1]
    return dict(master_timestamp=m['timestamp'],mac_timestamp=a['timestamp'],metrics={k:m['metrics'].get(k) for k in ['cpu_temperature_c','cpu_utilization','rtx3060_temperature_c','rtx3060_utilization','p100_temperature_c','p100_utilization']})


def collect(args):
    lock=load_lock(args.protocol_lock);identity=lock_sha256(args.protocol_lock);receipt=lock_receipt(args.protocol_lock)
    if args.session_id not in SESSIONS:raise ValueError('Unregistered collection session')
    spec=SESSIONS[args.session_id];split=spec['split'];seed=spec['seed']
    if args.blocks!=lock['collection']['blocks_per_session'] or args.block_seconds!=lock['collection']['block_seconds']:raise ValueError('Collection budget differs from lock')
    if args.session_id=='D':
        gate=json.loads((args.study_root/'validation/probe_validation.json').read_text())
        freeze=json.loads((args.study_root/'train/checkpoint_manifest.json').read_text())
        if gate.get('gate',{}).get('pass') is not True or freeze.get('complete') is not True:raise RuntimeError('Test collection requires passed probe and frozen checkpoints')
        if gate.get('protocol_lock_sha256')!=identity or freeze.get('protocol_lock_sha256')!=identity:raise ValueError('Test collection identities differ')
    out=args.output;out.mkdir(parents=True,exist_ok=True)
    if (out/'driver_identity.json').exists():raise FileExistsError('Use a fresh locked collection identity')
    before=runtime_state()
    if before['compute_processes']:raise RuntimeError('Unrelated GPU compute active; collection not started')
    save(out/'initial_runtime_state.json',before);save(out/'driver_identity.json',{'pid':os.getpid(),'started_at':time.time(),'owned_by':'k0-f2-acquire'})
    save(out/'normalization_config.json',DEFAULT_CONFIG)
    save(out/'collection_config.json',dict(schema_version='k0-f2-collection-v1',session_id=args.session_id,split=split,tasks=TASKS,block_seconds=args.block_seconds,blocks=args.blocks,seed=seed,**receipt))
    sensor=None;aligner=Aligner(out,args.session_id,identity);background=None;manifest=[];rows=[];ok=False;started=time.time()
    try:
        sensor=subprocess.Popen([sys.executable,'-m','experiments.k0_f2_interoception_confirmatory.sensors','--duration',str(args.blocks*args.block_seconds+360),'--interval','1','--output',str(out/'raw_master_telemetry.jsonl')],stdout=subprocess.DEVNULL)
        aligner.start();until=time.monotonic()+45
        while (not aligner.mac or not aligner.master) and time.monotonic()<until:time.sleep(.5)
        aligner.guard()
        with JobPool() as pool:
            for _ in range(45):aligner.guard();time.sleep(1)
            labels=[]
            for round_index in range(2):
                order=LABELS.copy();random.Random(seed+round_index*101).shuffle(order);labels.extend(order)
            for block,label in enumerate(labels):
                aligner.guard();rng=random.Random(seed+1000+block);bid=f'{args.session_id}-{block:03d}';begin=time.time()
                background=Background(label,args.block_seconds+60,out,bid)
                entry=dict(session_id=args.session_id,block_id=bid,split=split,workload_label=label,start_timestamp=begin,source_kind='real',tasks=[])
                print(json.dumps(dict(event='block_start',session_id=args.session_id,block=block,block_id=bid,label=label,seconds=args.block_seconds)),flush=True)
                for _ in range(5):aligner.guard();background.check();time.sleep(1)
                cases=[(t,length) for t in range(4) for length in range(1,9)];rng.shuffle(cases)
                for decision,(task_id,length) in enumerate(cases):
                    aligner.guard();background.check();task=TASKS[task_id];when=time.time()
                    row=row_inputs(aligner,when=when,length=length,task=task,task_id=task_id,session_id=args.session_id,split=split,block_id=bid,episode_id=f'{bid}-{decision:02d}',lock_identity=identity)
                    action_order=list(range(4));rng.shuffle(action_order);results={}
                    for index,action in enumerate(action_order):
                        aligner.guard();snapshot=body_snapshot(aligner);result=pool.run(action,task)
                        result.update(decision_timestamp=when,body_timestamp=row['provenance']['body_timestamp'],order_index=index,body_at_dispatch=snapshot)
                        if not result.get('ok') or not result.get('finite'):raise RuntimeError('Foreground result invalid; stop collection safely')
                        results[action]=result
                    row.update(costs_seconds=[results[a]['latency_seconds'] for a in range(4)],failures=[False]*4,action_order=action_order,measurements=[results[a] for a in range(4)])
                    rows.append(row);emit(out/'dataset.jsonl',row);emit(out/'measured_jobs.jsonl',row);entry['tasks'].append(row['episode_id'])
                    save(out/'current_decision.json',dict(stage='record_only',timestamp=when,task=task,task_id=task_id,actual_selected_resource=results[action_order[-1]]['actual_resource'],actual_latency=results[action_order[-1]]['latency_seconds'],body_action=None,blind_action=None,probe_predicted_utility=None,hidden_norm=None))
                    target=begin+5+(decision+1)*(args.block_seconds-5)/len(cases)
                    while time.time()<target:aligner.guard();background.check();time.sleep(max(0,min(.25,target-time.time())))
                background.check();background.close();entry['workload_processes']=background.evidence();background=None;entry['end_timestamp']=time.time();manifest.append(entry);save(out/'workload_manifest.json',manifest)
                print(json.dumps(dict(event='block_end',session_id=args.session_id,block_id=bid)),flush=True)
            for _ in range(5):aligner.guard();time.sleep(1)
        ok=True
    finally:
        if background:background.close()
        aligner.close()
        if sensor and sensor.poll() is None:
            sensor.terminate()
            try:sensor.wait(timeout=5)
            except subprocess.TimeoutExpired:sensor.kill();sensor.wait(timeout=5)
        after=runtime_state();after.update(collection_complete=ok,owned_sensor_stopped=sensor is None or sensor.poll() is not None,owned_background_stopped=background is None or all(p.poll() is not None for p in background.children),scratch_files=[str(p.relative_to(out)) for p in (out/'scratch').glob('*')],session_id=args.session_id,split=split,start_timestamp=started,end_timestamp=time.time(),protocol_lock_sha256=identity)
        save(out/'collection_runtime_state.json',after);save(out/'workload_manifest.json',manifest)
    print(json.dumps(dict(event='collection_complete',session_id=args.session_id,rows=len(rows),blocks=len(manifest))),flush=True)


def main():
    p=argparse.ArgumentParser();s=p.add_subparsers(dest='cmd',required=True)
    r=s.add_parser('receive');r.add_argument('--output',required=True,type=Path)
    c=s.add_parser('collect');c.add_argument('--output',required=True,type=Path);c.add_argument('--blocks',type=int,default=16);c.add_argument('--block-seconds',type=float,default=45);c.add_argument('--protocol-lock',type=Path,required=True);c.add_argument('--session-id',choices=list(SESSIONS),required=True);c.add_argument('--study-root',type=Path,required=True)
    a=p.parse_args()
    def stop(*_):raise KeyboardInterrupt('bounded acquisition interrupted')
    signal.signal(signal.SIGTERM,stop)
    receive(a) if a.cmd=='receive' else collect(a)

if __name__=='__main__':main()
