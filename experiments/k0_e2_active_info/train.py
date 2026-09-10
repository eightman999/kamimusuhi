"""Full-sequence imitation and paired single-factor PPO experiments."""
import argparse, copy, hashlib, json, os, random, subprocess, time
from pathlib import Path
import numpy as np
import torch
from torch import nn
from experiments.k0_brainstem.models import make_model
from experiments.k0_brainstem.train.ppo import advantages
from .env import ActiveInfoEnv


def atomic(path, obj):
    path=Path(path); path.parent.mkdir(parents=True,exist_ok=True)
    temp=path.with_suffix(path.suffix+'.tmp'); temp.write_text(json.dumps(obj,indent=2,allow_nan=False)+'\n');temp.replace(path)


def rng_state(env=None):
    return dict(python=random.getstate(), numpy=np.random.get_state(), torch=torch.get_rng_state(),
                cuda=torch.cuda.get_rng_state_all() if torch.cuda.is_available() else None,
                env=env.snapshot() if env else None)


def restore_rng(state,env=None):
    random.setstate(state['python']);np.random.set_state(state['numpy']);torch.set_rng_state(state['torch'].cpu())
    if state['cuda'] is not None and torch.cuda.is_available():torch.cuda.set_rng_state_all([s.cpu() for s in state['cuda']])
    if env is not None and state['env'] is not None:env.restore(state['env'])


def tensor_hash(model):
    h=hashlib.sha256()
    for name,value in model.state_dict().items():h.update(name.encode());h.update(value.detach().cpu().numpy().tobytes())
    return h.hexdigest()


def save_checkpoint(path, model, optimizer, config, env, stage, update, **extra):
    assert stage in ('initial','imitation','ppo','critic_warmup')
    p=Path(path);tmp=p.with_suffix('.tmp')
    torch.save(dict(model=model.state_dict(),optimizer=optimizer.state_dict(),config=config,
                    rng=rng_state(env),stage=stage,update=update,model_sha256=tensor_hash(model),**extra),tmp)
    tmp.replace(p)


def collect(model,env,teacher_probability=0.,greedy=False):
    observation=env.reset();state=model.initial_state(env.num_envs,env.device)
    columns=[[] for _ in range(6)];infos=[]
    with torch.no_grad():
        for t in range(env.episode_length):
            logits,value,state=model(observation,state)
            dist=torch.distributions.Categorical(logits=logits)
            target=env.oracle_actions()
            action=logits.argmax(-1) if greedy else dist.sample()
            if teacher_probability:
                action=torch.where(torch.rand(env.num_envs,device=env.device)<teacher_probability,target,action)
            nxt,reward,done,info=env.step(action)
            for col,item in zip(columns,(observation,action,dist.log_prob(action),value,reward,target)):col.append(item.clone())
            infos.append(info);observation=nxt
    return [torch.stack(col) for col in columns],infos


def metrics(rollout,infos):
    obs,actions,_,_,rewards,targets=rollout
    terminal=infos[-1];success=terminal['success'].float();scenarios=terminal['scenario']
    means=[float(success[scenarios==s].mean()) for s in range(6) if bool((scenarios==s).any())]
    accepted=torch.stack([i['call_accepted'] for i in infos])
    return dict(task_success=float(success.mean()),scenario_macro_success=sum(means)/len(means),
                reward=float(rewards.sum(0).mean()),call_rate=float(accepted.any(0).float().mean()),
                action_agreement=float((actions==targets).float().mean()))


def validation(model,device,n=256):
    env=ActiveInfoEnv(n,device,700001,dict(episode_length=48,language_cost=.05,language_reliability=1.,language_latency=2))
    return metrics(*collect(model,env,greedy=True))


def imitation_update(model,optimizer,rollout,config):
    obs,_,_,_,_,targets=rollout
    freq=torch.bincount(targets.flatten(),minlength=6).float().clamp_min(1)
    classweights=(freq.sum()/freq).sqrt();classweights/=classweights.mean()
    logs=[]
    for ids in torch.randperm(obs.shape[1],device=obs.device).split(config['minibatch_envs']):
        logits,_,_=model.forward_sequence(obs[:,ids],model.initial_state(len(ids),obs.device))
        loss=nn.functional.cross_entropy(logits.flatten(0,1),targets[:,ids].flatten(),weight=classweights,reduction='none').view_as(targets[:,ids])
        weight=torch.ones_like(loss)
        weight[targets[:,ids]>=2]=4
        weight[obs[:,ids,14]>.5]=32
        loss=(loss*weight).sum()/weight.sum()
        optimizer.zero_grad(set_to_none=True);loss.backward();norm=nn.utils.clip_grad_norm_(model.parameters(),1.0);optimizer.step()
        logs.append((float(loss.detach()),float(norm)))
    return dict(loss=float(np.mean([x[0] for x in logs])),grad_norm=float(np.mean([x[1] for x in logs])))


def ppo_update(model,optimizer,rollout,config,anchor):
    obs,actions,oldlog,oldvalues,rewards,_=rollout
    adv,returns=advantages(rewards,oldvalues,.99,.95)
    adv=(adv-adv.mean())/(adv.std()+1e-8)
    scale=returns.std().clamp_min(.1) if config['arm']=='B5' else torch.ones((),device=obs.device)
    logs=[]
    for _ in range(config['ppo_epochs']):
        for ids in torch.randperm(obs.shape[1],device=obs.device).split(config['minibatch_envs']):
            logits,values,_=model.forward_sequence(obs[:,ids],model.initial_state(len(ids),obs.device))
            dist=torch.distributions.Categorical(logits=logits)
            ratio=(dist.log_prob(actions[:,ids])-oldlog[:,ids]).exp();a=adv[:,ids]
            policy=-torch.minimum(ratio*a,ratio.clamp(.8,1.2)*a).mean()
            value=((values-returns[:,ids])/scale).square().mean()
            with torch.no_grad():al,_,_=anchor.forward_sequence(obs[:,ids],anchor.initial_state(len(ids),obs.device))
            kl=torch.distributions.kl_divergence(dist,torch.distributions.Categorical(logits=al)).mean()
            loss=policy+.5*value-.01*dist.entropy().mean()+(.1*kl if config['arm']=='B6' else 0)
            optimizer.zero_grad(set_to_none=True);loss.backward()
            norm=nn.utils.clip_grad_norm_(model.parameters(),.25 if config['arm']=='B7' else 1.0)
            optimizer.step()
            logs.append([float(x.detach()) for x in (loss,policy,value,kl,dist.entropy().mean(),norm)])
    vals=np.mean(logs,axis=0)
    return dict(zip(['loss','policy_loss','value_loss','anchor_kl','entropy','grad_norm'],vals.tolist()),
                return_std=float(returns.std()),explained_variance=float(1-(returns-oldvalues).var()/returns.var().clamp_min(1e-8)))


def critic_warmup(model,env,config):
    before={k:v.clone() for k,v in model.state_dict().items() if not k.startswith('critic.')}
    saved=rng_state(env)
    warm_env=ActiveInfoEnv(env.num_envs,env.device,300000+config['seed'],env.config)
    for p in model.parameters():p.requires_grad_(False)
    for p in model.critic.parameters():p.requires_grad_(True)
    optimizer=torch.optim.Adam(model.critic.parameters(),lr=.001)
    losses=[]
    for _ in range(4):
        rollout,_=collect(model,warm_env);obs,_,_,values,rewards,_=rollout
        _,returns=advantages(rewards,values)
        for ids in torch.arange(env.num_envs,device=env.device).split(config['minibatch_envs']):
            _,prediction,_=model.forward_sequence(obs[:,ids],model.initial_state(len(ids),env.device))
            loss=(prediction-returns[:,ids]).square().mean();optimizer.zero_grad();loss.backward();optimizer.step();losses.append(float(loss.detach()))
    assert all(torch.equal(v,model.state_dict()[k]) for k,v in before.items())
    for p in model.parameters():p.requires_grad_(True)
    restore_rng(saved,env)
    return {'losses':losses,'policy_unchanged':True,'common_rng_restored':True}


def train(config,artifacts,run_id,parent=None):
    torch.set_num_threads(4);seed=config['seed'];random.seed(seed);np.random.seed(seed);torch.manual_seed(seed)
    device=config.get('device','cpu');directory=Path(artifacts)/'runs'/run_id
    if (directory/'status.json').exists():raise ValueError('Refusing to overwrite existing run: '+run_id)
    directory.mkdir(parents=True,exist_ok=True)
    config=copy.deepcopy(config);config['gpu_uuid']=os.getenv('CUDA_VISIBLE_DEVICES');config['source_commit']=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
    atomic(directory/'config.json',config)
    status=dict(run_id=run_id,architecture=config['architecture'],seed=seed,arm=config['arm'],status='running',pid=os.getpid(),started_at=time.time(),gpu_uuid=config['gpu_uuid'])
    def state(**kwargs):status.update(kwargs,timestamp=time.time());atomic(directory/'status.json',status)
    state()
    model=make_model(config['architecture']).to(device);optimizer=torch.optim.Adam(model.parameters(),lr=.001)
    envconfig=dict(config['env']);envconfig['mix_conditions']=True
    if config['arm']=='B2':envconfig['zero_call_cost']=True
    env=ActiveInfoEnv(config['num_envs'],device,(100000 if config['arm']=='B0' else 200000)+seed,envconfig)
    parent_hash=None
    if parent:
        cp=torch.load(parent,map_location=device,weights_only=False);model.load_state_dict(cp['model']);optimizer.load_state_dict(cp['optimizer'])
        parent_hash=hashlib.sha256(Path(parent).read_bytes()).hexdigest()
        # Identical global and environment reset streams across every paired arm.
        restore_rng(cp['rng'])
        if config['arm']=='B3':
            for group in optimizer.param_groups:group['lr']=.0001
    anchor=copy.deepcopy(model).eval()
    for p in anchor.parameters():p.requires_grad_(False)
    env.reset()
    save_checkpoint(directory/'initial.pt',model,optimizer,config,env,'initial',0,parent_sha256=parent_hash)
    atomic(directory/'initial_validation.json',validation(model,device))
    if config['arm']=='B4':atomic(directory/'critic_warmup.json',critic_warmup(model,env,config))
    stage='imitation' if config['arm']=='B0' else 'ppo';total=config['imitation_updates'] if stage=='imitation' else config['ppo_updates'];best=-float('inf')
    try:
        for u in range(1,total+1):
            start=time.perf_counter()
            # B2 keeps all announced random conditions matched; only the reward language-cost term is disabled.
            rollout,infos=collect(model,env,teacher_probability=(1. if u<=total//2 else .8) if stage=='imitation' else 0.)
            losses=imitation_update(model,optimizer,rollout,config) if stage=='imitation' else ppo_update(model,optimizer,rollout,config,anchor)
            if str(device).startswith('cuda'):torch.cuda.synchronize()
            elapsed=time.perf_counter()-start
            val=validation(model,device,config.get('validation_envs',256));score=val['scenario_macro_success']+.1*val['reward']
            telemetry={'gpu_memory_bytes':torch.cuda.max_memory_allocated() if str(device).startswith('cuda') else 0}
            row=dict(update=u,stage=stage,validation=val,losses=losses,elapsed=elapsed,steps_per_second=env.num_envs*env.episode_length/elapsed,transitions=u*env.num_envs*env.episode_length,**telemetry)
            with (directory/'metrics.jsonl').open('a') as f:f.write(json.dumps(row,allow_nan=False)+'\n')
            save_checkpoint(directory/(stage+'_final.pt'),model,optimizer,config,env,stage,u,parent_sha256=parent_hash,validation=val)
            if score>best:
                best=score
                import shutil
                shutil.copyfile(directory/(stage+'_final.pt'),directory/(stage+'_best.pt'))
                atomic(directory/'selection.json',dict(validation_seed=700001,update=u,score=score,criterion='scenario_macro_success + 0.1 * episode_reward'))
            state(update=u,stage=stage,validation=val,transitions=row['transitions'])
            print(json.dumps(dict(run_id=run_id,**row)),flush=True)
        state(status='complete',finished_at=time.time())
    except BaseException as exc:
        state(status='failed',error=str(exc));raise


def main():
    p=argparse.ArgumentParser();p.add_argument('--config',required=True);p.add_argument('--artifacts',required=True);p.add_argument('--run-id',required=True);p.add_argument('--parent');p.add_argument('--device',default='cpu');a=p.parse_args()
    c=json.loads(Path(a.config).read_text());c['device']=a.device;train(c,a.artifacts,a.run_id,a.parent)

if __name__=='__main__':main()
