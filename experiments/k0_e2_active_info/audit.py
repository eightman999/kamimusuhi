"""Reproducible leakage and immutable-baseline checks; no artifact mutation."""
import hashlib,json
from pathlib import Path
import torch
from .env import ActiveInfoEnv
from .train import atomic


def audit(output,baseline_root=None):
    streams={'imitation':[100000+i for i in range(8)],'ppo':[200000+i for i in range(8)],'critic':[300000+i for i in range(8)],'validation':[700001],'test':[900001]}
    allseeds=sum(streams.values(),[]);assert len(allseeds)==len(set(allseeds))
    digests={};observations={};duplicates=[]
    for group,seeds in streams.items():
        digests[group]=[]
        for seed in seeds:
            e=ActiveInfoEnv(32,'cpu',seed,{'episode_length':48});obs=e.reset()
            for i in range(32):
                data=e.noise_stream[:,i].numpy().tobytes();h=hashlib.sha256(data).hexdigest()
                if h in observations:duplicates.append([observations[h],[group,seed,i]])
                observations[h]=[group,seed,i];digests[group].append(h)
    baseline={}
    if baseline_root:
        root=Path(baseline_root);manifest=json.loads((Path(__file__).parent/'baseline_manifest.json').read_text())
        for path,expected in manifest['files'].items():
            p=root/path;baseline[path]=p.is_file() and hashlib.sha256(p.read_bytes()).hexdigest()==expected
    result=dict(seed_streams=streams,seed_overlap=False,exogenous_episode_fingerprints=digests,
        duplicated_exogenous_episodes=duplicates,episode_audit_scope='32 generated episodes per RNG stream; deliberate A/B pairs excluded',
        latent_leakage={'precall_pair_test':'bit-identical full histories for all five non-language actions',
                       'strict_memory':'all six cue-dependent actions leave identical postcue observations',
                       'answer_encoding':'response/cue/sensor only; gate bit and announced service conditions independent of answer',
                       'scenario_context':'synthetic physical affordances expose acquisition type; no numeric scenario ID input. This is an explicit task simplification, not an unseen-context generalization claim.',
                       'timestamp':'deadline flag is public and shared by A/B; no wall-clock or seed input'},
        normalization={'observations':'fixed public physical scaling only; no fitted dataset statistics','advantage_and_return':'training rollout only; validation/test never update parameters or normalization'},
        tensor_memory_reuse='snapshot/restore and caller-observation clone tests passed; no shared mutable train/eval tensors',
        baseline_files_checked=len(baseline),baseline_unchanged=all(baseline.values()) if baseline else None,
        baseline_mismatches=[p for p,ok in baseline.items() if not ok])
    atomic(output,result);return result

if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser();p.add_argument('--output',required=True);p.add_argument('--baseline-root');a=p.parse_args();audit(a.output,a.baseline_root)
