"""Finite candidate selection using validation scores only; never reads held-out data."""
import argparse,json
from pathlib import Path
from .evaluate import seed_statistics,paired_sign_test
from .train import atomic


def select(root):
    root=Path(root);scores={}
    for arm in ['B0','B1','B2','B3','B4','B5','B6','B7']:
        scores[arm]={}
        for seed in range(8):
            directory=root/'runs'/f'gru128-s{seed}-{arm}'
            status=json.loads((directory/'status.json').read_text());assert status['status']=='complete'
            if arm=='B0':score=json.loads((directory/'selection.json').read_text())['score']
            else:
                v=status['validation'];score=v['scenario_macro_success']+.1*v['reward']
            scores[arm][seed]=score
    rows=[];eligible=[]
    for arm in ['B1','B2','B3','B4','B5','B6','B7']:
        delta=[scores[arm][s]-scores['B1'][s] for s in range(8)]
        stat=seed_statistics(delta);test=paired_sign_test(delta)
        good=arm not in ('B1','B2') and stat['mean']>.01 and test['two_sided_exact_p']<.05
        if good:eligible.append(arm)
        rows.append(dict(arm=arm,score=seed_statistics(list(scores[arm].values())),delta_vs_B1=stat,sign_test=test,eligible=good))
    # One combined candidate only; singleton duplicates a completed condition.
    factors=eligible if len(eligible)>=2 else []
    result=dict(factors=factors,complete=True,source='validation seed 700001 only',
                criterion='mean final validation score gain > .01 and two-sided paired exact p < .05 versus B1; cost-free B2 is causal-only, excluded from cost-aware stabilization',
                rows=rows,scores=scores,decision='one combined B8 candidate' if factors else 'no combined run: fewer than two eligible factors')
    atomic(root/'stabilized_selection.json',result);return result

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();print(json.dumps(select(a.artifacts),indent=2))
