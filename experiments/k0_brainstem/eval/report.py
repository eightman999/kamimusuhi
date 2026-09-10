"""Aggregate independent seeds without fabricating experimental success."""
import csv,json,argparse,statistics
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def make_report(root):
    root=Path(root);rows=[];evaluations={}
    fields=['run_id','architecture','seed','density','task_success','llm_call_rate','missed_llm_rate','false_llm_call_rate','OOD_score','habituation_score','habituation_sequence_score','state_retention_score','params','active_connections','gates','cpu_inference_latency_ms','steps_per_second']
    for path in sorted((root/'runs').glob('*/evaluation.json')):
        e=json.loads(path.read_text());c=e['config'];run=path.parent.name
        if run.startswith('smoke'):continue
        logs=[json.loads(s) for s in (path.parent/'metrics.jsonl').read_text().splitlines()]
        row=dict(run_id=run,architecture=c['architecture'],seed=c['seed'],density=c.get('density'))
        row.update({k:e.get(k) for k in fields if k not in row})
        row['steps_per_second']=statistics.mean(m['steps_per_second'] for m in logs if m['stage']=='ppo')
        rows.append(row);evaluations[run]=e
    (root/'run_summary.json').write_text(json.dumps(rows,indent=2))
    with (root/'run_summary.csv').open('w',newline='') as f:
        w=csv.DictWriter(f,fields,lineterminator='\n');w.writeheader();w.writerows(rows)
    arches=sorted(set(r['architecture'] for r in rows));fig,ax=plt.subplots(figsize=(8,5))
    for arch in arches:
        subset=[r for r in rows if r['architecture']==arch]
        ax.scatter([r['llm_call_rate'] for r in subset],[r['task_success'] for r in subset],label=arch)
        for r in subset:
            curve=evaluations[r['run_id']]['pareto'];ax.plot([x['llm_call_rate'] for x in curve],[x['task_success'] for x in curve],alpha=.25)
    ax.set(xlabel='LLM call rate',ylabel='Task success',title='K0 held-out policy tradeoffs (points: trained gate; lines: logit bias sweep)');ax.legend();fig.tight_layout();fig.savefig(root/'pareto.png',dpi=150);plt.close(fig)
    curves=root/'learning_curves';curves.mkdir(exist_ok=True)
    for arch in arches:
        fig,axs=plt.subplots(1,2,figsize=(10,3))
        for r in rows:
            if r['architecture']!=arch:continue
            ms=[json.loads(s) for s in (root/'runs'/r['run_id']/'metrics.jsonl').read_text().splitlines()]
            for ax,k in zip(axs,['task_success','llm_call_rate']):ax.plot([m['training_step'] for m in ms],[m[k] for m in ms],label=f"seed {r['seed']}");ax.set(xlabel='environment steps',ylabel=k)
        axs[0].legend();fig.tight_layout();fig.savefig(curves/f'{arch}.png',dpi=120);plt.close(fig)
    return rows

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();print(json.dumps(make_report(a.artifacts),indent=2))
