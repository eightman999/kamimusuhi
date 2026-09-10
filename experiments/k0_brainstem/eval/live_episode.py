"""Run bounded real J72 E2E evaluation after training only."""
import argparse,json,time
from pathlib import Path
import torch
from .evaluator import load
from .j72_bridge import RealJ72LanguageBackend
from ..env.vector_env import VectorEnv
from ..train.trainer import atomic


def run(checkpoint,endpoint,episodes=50,timeout=10):
    torch.set_num_threads(4);model,c=load(checkpoint);backend=RealJ72LanguageBackend(endpoint,timeout=timeout)
    env=VectorEnv(episodes,c['episode_length'],'cpu',990123);obs=env.reset();state=model.initial_state(episodes,'cpu')
    log=Path(checkpoint).parent/'language_events.jsonl';calls=failures=missed=false=required=0
    with log.open('a') as f,torch.no_grad():
        for t in range(env.episode_length):
            logits,_,state=model(obs,state);prob=logits.softmax(-1);act=logits.argmax(-1);target=env.oracle_actions()
            required+=int((target==5).sum());missed+=int(((target==5)&(act!=5)).sum());false+=int(((target!=5)&(act==5)).sum())
            for i in (act==5).nonzero().flatten().tolist():
                response=backend.invoke(obs[i].tolist(),model.state_metrics(state[i:i+1]))
                event={'timestamp':time.time(),'run_id':Path(checkpoint).parent.name,'episode':i,'time':t,'scenario':int(env.scenario[i]),'signals':obs[i].tolist(),'core_confidence':float(prob[i,5]),'oracle_required':bool(target[i]==5),'j72_called':True,'j72_latency':response['latency'],**response}
                f.write(json.dumps(event,ensure_ascii=False)+'\n');f.flush();calls+=1;failures+=response['status']!='ok'
            obs,_,_,_=env.step(act)
    result={'episodes':episodes,'calls':calls,'successful_calls':calls-failures,'http_failures':failures,'required':required,'missed':missed,'false_calls':false,'gate_verified':calls>0,'complete':calls>0 and failures==0}
    atomic(log.parent/'j72_evaluation.json',result);return result

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--checkpoint',required=True);p.add_argument('--endpoint',required=True);p.add_argument('--episodes',type=int,default=50);p.add_argument('--timeout',type=float,default=10);a=p.parse_args();print(json.dumps(run(a.checkpoint,a.endpoint,a.episodes,a.timeout)))
