"""Temporary-service evaluation: API -> strict parser -> numeric Core feedback."""
import argparse,json,time,urllib.error
from pathlib import Path
import torch
from experiments.k0_brainstem.models import make_model
from .env import ActiveInfoEnv
from .language import query
from .train import atomic


def fault_transport(kind,fact):
    def transport(body,timeout):
        if kind=='timeout':raise TimeoutError('controlled timeout fixture')
        if kind=='http_503':raise urllib.error.HTTPError('fixture',503,'controlled unavailable',None,None)
        if kind=='slow':time.sleep(.01)
        text={'malformed':'{not-json','empty':'','contradiction':json.dumps({'category':'A' if fact else 'B','confidence':1.,'evidence_id':'record-1'})}.get(kind,
             json.dumps({'category':'B' if fact else 'A','confidence':1.,'evidence_id':'record-1'}))
        return {'choices':[{'message':{'content':text}}]}
    return transport


@torch.no_grad()
def evaluate_closed_loop(model,endpoint,n=24,seed=920001,backend='j72',fault=None,force_gate=False):
    config=dict(scenario='language',episode_length=48,language_cost=.05,language_reliability=1.,language_latency=16 if fault=='slow' else 2)
    if backend!='scripted':config['language_backend']='external'
    env=ActiveInfoEnv(n,'cpu',seed,config);obs=env.reset();state=model.initial_state(n,'cpu')
    episodes=[dict(episode_id=i,steps=[],calls=[],success=False) for i in range(n)]
    reward=torch.zeros(n);allfinite=True;accepted_gate_calls=0
    for t in range(env.episode_length):
        logits,_,state=model(obs,state);action=logits.argmax(-1)
        if force_gate and t==0:action.fill_(5)
        before=obs.clone();obs,r,done,info=env.step(action);reward+=r
        accepted=info['call_accepted'].nonzero().flatten().tolist();accepted_gate_calls+=len(accepted)
        for i in accepted:
            if backend=='scripted':continue
            result=query(endpoint,int(env.latent[i]),before[i].tolist(),transport=fault_transport(fault,int(env.latent[i])) if fault else None)
            result.update(t=t,source='controlled_fault_fixture' if fault else 'live_j72')
            episodes[i]['calls'].append(result)
            env.inject_language_response([i],[result['category']],[result['confidence']],[result['valid']])
        obs=env.observation.clone();allfinite &= bool(torch.isfinite(obs).all() and torch.isfinite(state).all())
        for i in range(n):
            status='CALL' if i in accepted else ('WAITING' if bool(env.pending[i]) else ('RESPONSE' if bool(info['language_delivered'][i]) else 'NEXT_ACTION'))
            episodes[i]['steps'].append(dict(t=t,observation=before[i].tolist(),action=int(action[i]),hidden_norm=float(state[i].norm()) if state.numel() else 0.,information=float(obs[i,9]) if obs[i,10]>0 else None,status=status,language_latency=int(env.latency[i]),cost=float(info['language_cost'][i]),resource=float(env.resource[i])))
            if bool(done[i]):episodes[i]['success']=bool(info['success'][i]);episodes[i]['reward']=float(reward[i])
    calls=[c for e in episodes for c in e['calls']]
    def rate(key):return sum(bool(c[key]) for c in calls)/len(calls) if calls else None
    return dict(backend=backend,fault=fault,force_gate=force_gate,episodes=episodes,num_episodes=n,calls=len(calls),accepted_gate_calls=accepted_gate_calls,
                http_attempts=sum(len(c['attempts']) for c in calls) if backend=='j72' else 0,
                api_success=rate('api_success'),parse_success=rate('parse_success'),semantic_correctness=rate('semantic_correct'),
                downstream_success=sum(e['success'] for e in episodes)/n,reward=float(reward.mean()),
                response_latency_seconds=sum(c['latency_seconds'] for c in calls)/len(calls) if calls else None,
                finite_core_state=allfinite,environment_seed=seed,
                timing_scope='HTTP elapsed measured in wall seconds; simulated response delay is fixed exogenous steps, not wall-clock feedback')


def run(artifacts,endpoint):
    torch.set_num_threads(4);root=Path(artifacts);representatives=[]
    for arch in ['gru64','gru128']:
        candidates=[]
        for p in (root/'runs').glob(f'{arch}-s*-B0/selection.json'):
            selection=json.loads(p.read_text());candidates.append((selection['score'],p.parent))
        score,d=max(candidates,key=lambda x:x[0]);cp=torch.load(d/'imitation_best.pt',map_location='cpu',weights_only=False)
        model=make_model(arch);model.load_state_dict(cp['model']);model.eval()
        real=evaluate_closed_loop(model,endpoint)
        baseline=evaluate_closed_loop(model,endpoint,backend='scripted')
        failures={kind:evaluate_closed_loop(model,endpoint,n=16,backend='fixture',fault=kind,force_gate=True) for kind in ['timeout','http_503','malformed','empty','contradiction','slow']}
        item=dict(run_id=d.name,validation_score=score,selection_seed=700001,**real,scripted=baseline,faults=failures)
        # If gate learned nothing, preserve that failure and exercise interface separately.
        if real['calls']==0:item['forced_gate_diagnostic']=evaluate_closed_loop(model,endpoint,n=8,force_gate=True)
        representatives.append(item)
        atomic(root/'j72_results.json',dict(representatives=representatives,complete=False))
    atomic(root/'j72_results.json',dict(representatives=representatives,complete=True,selection='validation only',fault_scope='controlled deterministic transport fixtures; live J72 metrics separately marked'))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);p.add_argument('--endpoint',required=True);a=p.parse_args();run(a.artifacts,a.endpoint)
