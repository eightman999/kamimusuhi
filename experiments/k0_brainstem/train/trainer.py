"""Standalone trainer. Monitoring is optional and cannot own training lifetime."""
import argparse,json,os,random,time,traceback,signal
from pathlib import Path
import numpy as np
import torch
import yaml
from ..env.vector_env import VectorEnv
from ..models import make_model
from ..telemetry.store import MetricStore
from ..telemetry.system_stats import snapshot
from . import imitation,ppo


def atomic(path, obj):
    path=Path(path); temp=path.with_suffix(path.suffix+'.tmp'); temp.write_text(json.dumps(obj,indent=2,allow_nan=False)); temp.replace(path)


def collect(model,env,device,teacher=False,greedy=False):
    observation=env.reset(); state=model.initial_state(env.num_envs,device)
    obs=[]; actions=[]; logs=[]; values=[]; rewards=[]; targets=[]; infos=[]; states=[]
    with torch.no_grad():
        for _ in range(env.episode_length):
            logits,value,state=model(observation,state)
            dist=torch.distributions.Categorical(logits=logits)
            target=env.oracle_actions()
            action=target if teacher else (logits.argmax(-1) if greedy else dist.sample())
            obs.append(observation); targets.append(target); actions.append(action)
            logs.append(dist.log_prob(action)); values.append(value)
            observation,reward,done,info=env.step(action)
            rewards.append(reward); infos.append(info); states.append(state)
    return tuple(torch.stack(v) for v in (obs,actions,logs,values,rewards)),torch.stack(targets),infos,states[-1]


def metric_values(rollout,targets,infos,state,model):
    _,actions,_,_,rewards=rollout
    required=targets==5; called=actions==5
    def rate(mask,value): return float(value[mask].float().mean()) if bool(mask.any()) else None
    out={'reward_mean':float(rewards.mean()),'reward_std':float(rewards.std()),'task_success':float((actions==targets).float().mean()),'llm_call_rate':float(called.float().mean()),'required_llm_call_rate':float(required.float().mean()),'missed_llm_rate':rate(required,~called),'false_llm_call_rate':rate(~required,called),'action_distribution':torch.bincount(actions.flatten(),minlength=6).float().div(actions.numel()).tolist()}
    for key,name in [('retention_mask','state_retention_score'),('habituation_mask','habituation_score'),('novelty_mask','novelty_response_score')]:
        mask=torch.stack([i[key] for i in infos]);out[name]=rate(mask,actions==targets)
    if 'first_stimulus_mask' in infos[0]:
        first=torch.stack([i['first_stimulus_mask'] for i in infos]); repeated=torch.stack([i['habituation_mask'] for i in infos]); renewed=torch.stack([i['renewed_stimulus_mask'] for i in infos])
        eligible=first.any(0)&repeated.any(0)&renewed.any(0)
        matched=actions==targets
        joint=((~first|matched).all(0)&(~repeated|matched).all(0)&(~renewed|matched).all(0))
        out['habituation_sequence_score']=rate(eligible,joint)
    out.update(model.state_metrics(state));return out


def train(config,artifacts,run_id,resume=False):
    torch.set_num_threads(int(os.getenv('K0_CPU_THREADS','4')))
    seed=int(config['seed']);random.seed(seed);np.random.seed(seed);torch.manual_seed(seed)
    device=torch.device(config.get('device','cpu'))
    directory=Path(artifacts)/'runs'/run_id;directory.mkdir(parents=True,exist_ok=True)
    if (directory/'checkpoint.pt').exists() and not resume: raise ValueError('Existing checkpoint: use --resume or a new run ID')
    atomic(directory/'config.json',config)
    status={'run_id':run_id,'architecture':config['architecture'],'seed':seed,'device':str(device),'gpu_uuid':os.getenv('CUDA_VISIBLE_DEVICES'),'pid':os.getpid(),'status':'running','training_step':0,'started_at':time.time()}
    def save_status(**extra): status.update(extra,timestamp=time.time());atomic(directory/'status.json',status)
    save_status()
    model=make_model(config['architecture'],hidden_size=config.get('hidden_size',128),density=config.get('density',.1)).to(device)
    optimizer=torch.optim.Adam(model.parameters(),lr=config.get('learning_rate',.001))
    env=VectorEnv(config['num_envs'],config['episode_length'],str(device),seed)
    env.llm_cost=config.get('llm_cost',.15)
    store=MetricStore(artifacts,run_id)
    best_score=-float('inf')
    start=0;total=config['imitation_updates']+config['ppo_updates']
    stopped=False
    def request_stop(*_):
        nonlocal stopped
        stopped=True
    signal.signal(signal.SIGTERM,request_stop);signal.signal(signal.SIGINT,request_stop)
    if resume:
        cp=torch.load(directory/'checkpoint.pt',map_location=device,weights_only=False)
        model.load_state_dict(cp['model']);optimizer.load_state_dict(cp['optimizer']);start=cp['update']
        torch.set_rng_state(cp['torch_rng'].cpu());np.random.set_state(cp['numpy_rng']);random.setstate(cp['python_rng'])
        if device.type=='cuda' and cp.get('cuda_rng') is not None:torch.cuda.set_rng_state(cp['cuda_rng'].cpu(),device)
        env.generator.set_state(cp['env_rng'].cpu())
        best_score=cp.get('best_score',-float('inf'))
        save_status(resumed_from_update=start,environment_rng_reset=False)
    def checkpoint(update):
        tmp=directory/'checkpoint.tmp'
        torch.save({'model':model.state_dict(),'optimizer':optimizer.state_dict(),'update':update,'stage':'imitation' if update<config['imitation_updates'] else 'ppo','config':config,'env_rng':env.generator.get_state(),'best_score':best_score,'torch_rng':torch.get_rng_state(),'numpy_rng':np.random.get_state(),'python_rng':random.getstate(),'cuda_rng':torch.cuda.get_rng_state(device) if device.type=='cuda' else None},tmp)
        tmp.replace(directory/'checkpoint.pt')
    try:
        for update in range(start,total):
            control=directory/'control.json'
            while control.exists():
                command=json.loads(control.read_text()).get('command')
                if command=='stop': stopped=True;break
                if command!='pause':break
                save_status(status='paused');time.sleep(.2)
                if stopped:break
            if stopped:checkpoint(update);save_status(status='stopped');return
            save_status(status='running')
            began=time.perf_counter();teacher=update<config['imitation_updates']
            rollout,targets,infos,state=collect(model,env,device,teacher)
            if teacher:loss=imitation.update(model,optimizer,rollout[0],targets,config,infos)
            else:loss=ppo.update(model,optimizer,rollout,config)
            if device.type=='cuda':torch.cuda.synchronize(device)
            elapsed=time.perf_counter()-began
            # Measure learned policy, never report oracle demonstration accuracy as learning.
            eval_env=VectorEnv(min(256,config['num_envs']),config['episode_length'],str(device),700001)
            eval_env.llm_cost=env.llm_cost
            er,et,ei,es=collect(model,eval_env,device,greedy=True)
            metric=metric_values(er,et,ei,es,model)
            with (directory/'language_events.jsonl').open('a') as events:
                for t,n in (er[1]==5).nonzero()[:16].tolist():
                    events.write(json.dumps({'timestamp':time.time(),'run_id':run_id,'scenario':int(ei[t]['scenario'][n]),'signals':er[0][t,n].tolist(),'core_confidence':float(er[2][t,n].exp()),'oracle_required':bool(et[t,n]==5),'j72_called':False,'j72_latency':None,'status':'VIRTUAL_ORACLE','update':update+1})+'\n')
            metric.update(timestamp=time.time(),run_id=run_id,architecture=config['architecture'],seed=seed,training_step=(update+1)*config['num_envs']*config['episode_length'],episodes=(update+1)*config['num_envs'],update=update+1,stage='imitation' if teacher else 'ppo',loss=loss,steps_per_second=rollout[0].shape[0]*rollout[0].shape[1]/elapsed,episodes_per_second=config['num_envs']/elapsed,system=snapshot(),OOD_score=None,**model.stats())
            store.write(metric)
            components=[metric['task_success'],metric['state_retention_score'],metric['habituation_score'],metric['novelty_response_score']]
            if metric['missed_llm_rate'] is not None:components.append(1-metric['missed_llm_rate'])
            selection=sum(v for v in components if v is not None)/sum(v is not None for v in components)-.15*metric['llm_call_rate']
            improved=selection>best_score
            if improved:best_score=selection
            checkpoint(update+1)
            if improved:
                import shutil
                shutil.copyfile(directory/'checkpoint.pt',directory/'best.pt')
                atomic(directory/'best_selection.json',{'update':update+1,'score':selection,'validation_seed':700001})
            save_status(training_step=metric['training_step'],update=update+1,stage=metric['stage'],reward=metric['reward_mean'],task_success=metric['task_success'],llm_call_rate=metric['llm_call_rate'])
            print(json.dumps({k:metric[k] for k in ['run_id','update','stage','loss','task_success','state_retention_score','steps_per_second']}),flush=True)
            if update+1==config['imitation_updates']:
                import shutil
                shutil.copyfile(directory/'checkpoint.pt',directory/'warmup.pt')
        save_status(status='complete',finished_at=time.time())
    except Exception as exc:
        save_status(status='failed',error=str(exc));raise
    finally:store.close()


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--config',required=True);parser.add_argument('--artifacts',required=True);parser.add_argument('--run-id',required=True);parser.add_argument('--device');parser.add_argument('--resume',action='store_true');args=parser.parse_args()
    config=yaml.safe_load(Path(args.config).read_text())
    if args.device:config['device']=args.device
    train(config,args.artifacts,args.run_id,args.resume)

if __name__=='__main__':main()
