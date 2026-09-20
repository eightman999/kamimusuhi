"""Final evidence recomputed from on-disk checkpoints and completed runs."""
import argparse,hashlib,json,platform,subprocess
from pathlib import Path
import torch
from .train import atomic


def digest_rng(cp):
    r=cp['rng'];h=hashlib.sha256()
    for value in [r['torch']]+(r['cuda'] or [])+[r['env']['rng_state'],r['env']['noise_stream'],r['env']['latent']]:h.update(value.cpu().numpy().tobytes())
    h.update(repr(r['python']).encode());h.update(repr(r['numpy']).encode());return h.hexdigest()


def build(root):
    root=Path(root);runs=[];initials={};parents={};failed=[];nonfinite=[]
    for d in sorted((root/'runs').iterdir()):
        if not (d/'config.json').exists():continue
        config=json.loads((d/'config.json').read_text());status=json.loads((d/'status.json').read_text())
        if status['status']!='complete':failed.append(d.name)
        cps={}
        for p in sorted(d.glob('*.pt')):
            cp=torch.load(p,map_location='cpu',weights_only=False)
            if not all(torch.isfinite(v).all() for v in cp['model'].values()):nonfinite.append(str(p.relative_to(root)))
            stage='initial' if p.stem=='initial' else ('imitation' if p.stem.startswith('imitation') else 'ppo')
            assert cp['stage']==stage,(p,cp['stage'])
            cps[p.name]=dict(sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size,stage=cp['stage'],update=cp['update'],model_sha256=cp['model_sha256'],parent_sha256=cp.get('parent_sha256'))
            if p.name=='initial.pt':initials[d.name]=dict(model=cp['model_sha256'],rng=digest_rng(cp),actual_env_config=cp['rng']['env']['config'])
            if p.name=='imitation_best.pt':parents[d.name]=dict(model=cp['model_sha256'],sha256=cps[p.name]['sha256'])
        metrics=[json.loads(x) for x in (d/'metrics.jsonl').read_text().splitlines()]
        warm=json.loads((d/'critic_warmup.json').read_text()) if (d/'critic_warmup.json').exists() else None
        runs.append(dict(run_id=d.name,config=config,status=status,checkpoints=cps,metric_rows=len(metrics),train_transitions=status.get('transitions'),
                         training_update_seconds=sum(m['elapsed'] for m in metrics),critic_warmup=warm,
                         evaluation_files=[str(p.relative_to(root)) for p in (root/'evaluations').glob(d.name+'*/*.json')]))
    pairing=[]
    for seed in range(8):
        name=f'gru128-s{seed}-B0';parent=parents[name]
        branches=[r for r in runs if r['config']['architecture']=='gru128' and r['config']['seed']==seed and r['config']['arm']!='B0']
        models=all(initials[r['run_id']]['model']==parent['model'] for r in branches)
        rngs={initials[r['run_id']]['rng'] for r in branches}
        hashes=all(r['checkpoints']['initial.pt']['parent_sha256']==parent['sha256'] for r in branches)
        gpu={r['config']['gpu_uuid'] for r in branches}
        pairing.append(dict(seed=seed,arms=[r['config']['arm'] for r in branches],same_initial_model=models,same_parent_checkpoint=hashes,same_rollout_rng=len(rngs)==1,same_gpu=len(gpu)==1))
    matrices={arm:sorted(r['config']['seed'] for r in runs if r['config']['arm']==arm and r['config']['architecture']=='gru128') for arm in sorted({r['config']['arm'] for r in runs})}
    result=dict(runs=runs,run_count=len(runs),failed_runs=failed,nonfinite_checkpoints=nonfinite,paired_checks=pairing,
                seed_coverage={arch:sorted(r['config']['seed'] for r in runs if r['config']['architecture']==arch and r['config']['arm']=='B0') for arch in ['mlp','gru64','gru128']},ppo_coverage=matrices,
                total_training_transitions=sum(r['train_transitions'] or 0 for r in runs),additional_critic_transitions=sum((r['critic_warmup'] or {}).get('transitions',0) for r in runs),
                runtime=dict(python=platform.python_version(),pytorch=torch.__version__,cuda=torch.version.cuda),
                source_commits=sorted({r['config']['source_commit'] for r in runs}),
                final_source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip())
    assert not failed and not nonfinite
    assert all(all(p[k] for k in ['same_initial_model','same_parent_checkpoint','same_rollout_rng','same_gpu']) for p in pairing)
    atomic(root/'run_evidence.json',result)
    files={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('experiments/k0_e2_active_info').glob('*.py')}
    atomic(root/'evaluation_source_manifest.json',dict(files=files,commit=result['final_source_commit']))
    print(json.dumps({k:v for k,v in result.items() if k!='runs'},indent=2))
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();build(a.artifacts)
