"""Known-training-delay control, separate from the prespecified temporal OOD sweep."""
import argparse,json,hashlib
from pathlib import Path
import torch
from .env import ActiveInfoEnv
from .evaluate import load_checkpoint,evaluate_episode,STATES
from .train import atomic


def run(root):
    torch.set_num_threads(4);root=Path(root);records=[];pairs=[]
    for p in sorted(root.glob('runs/*-B0/imitation_best.pt')):
        model,cp=load_checkpoint(p);cfg={'scenario':'memory','memory_delay':47,'strict_memory':True};meta=dict(architecture=cp['config']['architecture'],seed=cp['config']['seed'],run_id=p.parent.name,delay=47,checkpoint_sha256=hashlib.sha256(p.read_bytes()).hexdigest())
        for state_mode in STATES:
            metrics,_=evaluate_episode(model,cfg,n_envs=64,seed=900001,state_mode=state_mode)
            records.append(dict(**meta,state_mode=state_mode,metrics=metrics))
        ea=ActiveInfoEnv(64,'cpu',900001,cfg);eb=ActiveInfoEnv(64,'cpu',900001,cfg)
        ea.reset(torch.zeros(64,dtype=torch.long));eb.reset(torch.ones(64,dtype=torch.long))
        ma,a=evaluate_episode(model,cfg,n_envs=64,env=ea,record_observations=True)
        mb,b=evaluate_episode(model,cfg,n_envs=64,env=eb,record_observations=True)
        pairs.append(dict(**meta,postcue_bit_identical=bool(torch.equal(a['observations'][1:],b['observations'][1:])),
                          paired_success=(ma['task_success']+mb['task_success'])/2,
                          final_action_change_rate=float((a['actions'][-1]!=b['actions'][-1]).float().mean()),episodes_per_latent=64))
    atomic(root/'trained_delay_ablation.json',dict(records=records,memory_pairs=pairs,complete=True,
        purpose='Known training delay 47 is an in-distribution control; original 8..640 timing-OOD sweep unchanged; no model selection or retraining.',environment_seed=900001))
    horizons=json.loads((root/'retention_horizon.json').read_text()) if (root/'retention_horizon.json').exists() else None
    atomic(root/'retention_horizon_interpretation.json',dict(effective_memory_horizon=None,
        reason='Non-monotonic success across delays and short-delay failure reveal deadline/timing OOD. A first 50% crossing does not estimate memory decay.',
        original_first_crossing_diagnostic=horizons,training_delay_control=47))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();run(a.artifacts)
