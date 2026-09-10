"""Held-out sequence evaluation, gate tradeoffs and explicit decision masks."""
import argparse,json,time,hashlib
from pathlib import Path
import torch
from ..models import make_model
from ..env.vector_env import VectorEnv
from ..train.trainer import metric_values,atomic


def load(checkpoint,device='cpu'):
    cp=torch.load(checkpoint,map_location=device,weights_only=False);c=cp['config']
    model=make_model(c['architecture'],hidden_size=c.get('hidden_size',128),density=c.get('density',.1)).to(device)
    model.load_state_dict(cp['model']);model.eval();return model,c


@torch.no_grad()
def evaluate(model,config,episodes=512,seed=900001,ood=None,device='cpu',gate_bias=0.,save_episode=False,ablate_state=False):
    env=VectorEnv(episodes,max(160,config['episode_length']) if ood in ('long_delay','longer_delay') else config['episode_length'],device,seed,ood=ood)
    env.llm_cost=config.get('llm_cost',.15)
    obs=env.reset();state=model.initial_state(episodes,device)
    actions=[];targets=[];rewards=[];infos=[];records=[]
    for t in range(env.episode_length):
        if ablate_state:state=model.initial_state(episodes,device)
        logits,_,state=model(obs,state);logits[:,5]+=gate_bias;chosen=logits.argmax(-1)
        target=env.oracle_actions();nxt,r,_,info=env.step(chosen)
        if save_episode:
            records.append({'time':t,'sensor_vector':obs[0].tolist(),'internal_state':state[0].tolist(),'chosen_action':int(chosen[0]),'oracle_action':int(target[0]),'reward':float(r[0]),'scenario':int(info['scenario'][0])})
        actions.append(chosen);targets.append(target);rewards.append(r);infos.append(info);obs=nxt
    a=torch.stack(actions);target=torch.stack(targets);r=torch.stack(rewards)
    result=metric_values((None,a,None,None,r),target,infos,state,model)
    scenario=torch.stack([i['scenario'] for i in infos]);correct=a==target
    result['scenario_success']={str(s):float(correct[scenario==s].float().mean()) if bool((scenario==s).any()) else None for s in range(8)}
    valid=[v for v in result['scenario_success'].values() if v is not None]
    result['macro_task_success']=sum(valid)/len(valid)
    result['decision_count']={k:int(torch.stack([i[k] for i in infos]).sum()) for k in ['retention_mask','habituation_mask','novelty_mask']}
    result['episodes']=episodes;result['seed']=seed;result['gate_bias']=gate_bias
    return result,records


def evaluate_checkpoint(checkpoint,device='cpu',episodes=512):
    torch.set_num_threads(4);model,c=load(checkpoint,device)
    result,records=evaluate(model,c,episodes=episodes,device=device,save_episode=True)
    modes=['noise','dropout','inversion','long_delay','combination','resource_drop','sensor_fault']
    oods={m:evaluate(model,c,episodes=episodes,device=device,ood=m)[0] for m in modes}
    result.update(OOD_score=sum(v['macro_task_success'] for v in oods.values())/len(oods),ood=oods,**model.stats())
    if hasattr(model,'set_discretized'):
        model.set_discretized(True)
        result['discretized_gates']=evaluate(model,c,episodes=episodes,device=device)[0]
        model.set_discretized(False)
    result['state_reset_ablation']=evaluate(model,c,episodes=episodes,device=device,ablate_state=True)[0]
    result['pareto']=[evaluate(model,c,episodes=episodes,device=device,gate_bias=b)[0] for b in [-2.,-1.,0.,1.,2.]]
    cpu=model.to('cpu');x=torch.zeros(1,16);s=cpu.initial_state(1,'cpu')
    with torch.no_grad():
        for _ in range(20):cpu(x,s)
        start=time.perf_counter()
        for _ in range(500):cpu(x,s)
    result['cpu_inference_latency_ms']=(time.perf_counter()-start)*2
    cp=torch.load(checkpoint,map_location='cpu',weights_only=False)
    result['checkpoint']={'filename':Path(checkpoint).name,'sha256':hashlib.sha256(Path(checkpoint).read_bytes()).hexdigest(),'update':cp['update'],'stage':cp['stage']}
    result['config']=c
    directory=Path(checkpoint).parent
    atomic(directory/'evaluation.json',result);atomic(directory/'episodes.json',records)
    warm=directory/'warmup.pt'
    if warm.exists() and Path(checkpoint).name!='warmup.pt':
        wm,wc=load(warm,device);before,_=evaluate(wm,wc,episodes=episodes,device=device)
        atomic(directory/'warmup_evaluation.json',before)
    final=directory/'checkpoint.pt'
    if final.exists():
        fm,fc=load(final,device);fe,_=evaluate(fm,fc,episodes=episodes,device=device)
        fe['checkpoint']='checkpoint.pt';atomic(directory/'final_evaluation.json',fe)
    return result


def main():
    p=argparse.ArgumentParser();p.add_argument('--checkpoint',required=True);p.add_argument('--device',default='cpu');p.add_argument('--episodes',type=int,default=512);args=p.parse_args()
    print(json.dumps(evaluate_checkpoint(args.checkpoint,args.device,args.episodes),indent=2))

if __name__=='__main__':main()
