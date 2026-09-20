"""Publication-style E2 plots from saved seed-level metrics; never recompute models."""
import argparse
import json
import re
from pathlib import Path

import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from .evaluate import seed_statistics

ARCHES = ("mlp", "gru64", "gru128")
COLORS = {"mlp":"#64748b","gru64":"#087e8b","gru128":"#b54c2e"}
LABELS = {"mlp":"MLP","gru64":"GRU-64","gru128":"GRU-128"}


def read(root,name):
    path=root/name
    if not path.exists():raise FileNotFoundError(f"Required saved metrics missing: {path}")
    value=json.loads(path.read_text())
    if not isinstance(value,list):raise ValueError(f"Expected seed-record list: {path}")
    return value


def values(records, metric):
    # Each record already represents one independent training seed.
    return [r["metrics"].get(metric) for r in records]


def draw_curve(axis,records,x_key,metric,label,color):
    if not records:
        return
    xs=sorted({r[x_key] for r in records})
    mean=[];lo=[];hi=[]
    for x in xs:
        rows=[r for r in records if r[x_key]==x]
        seeds=[r["seed"] for r in rows]
        if len(seeds)!=len(set(seeds)):
            raise ValueError(f"Duplicate seed in curve {label}/{metric}/{x}")
        stats=seed_statistics(values(rows,metric))
        mean.append(stats["mean"] if stats["mean"] is not None else np.nan)
        lo.append(stats["ci95"][0] if stats["ci95"] else np.nan)
        hi.append(stats["ci95"][1] if stats["ci95"] else np.nan)
    axis.plot(xs,mean,"o-",label=label,color=color,linewidth=1.6,markersize=4)
    axis.fill_between(xs,lo,hi,color=color,alpha=.12)


def finish(fig,path):
    for ax in fig.axes:
        if getattr(ax,"name","")=="rectilinear":
            if "(fraction)" in ax.get_ylabel():
                ax.set_ylim(0,1.05)
            ax.grid(alpha=.18)
            ax.spines["top"].set_visible(False);ax.spines["right"].set_visible(False)
    fig.tight_layout()
    fig.savefig(path,dpi=180,bbox_inches="tight")
    plt.close(fig)


def categorical(axis,records,conditions,metric,ylabel):
    positions=np.arange(len(conditions));offset=.22
    for i,arch in enumerate(ARCHES):
        means=[];low=[];high=[]
        for condition in conditions:
            rows=[r for r in records if r["architecture"]==arch and r["condition"]==condition]
            stats=seed_statistics(values(rows,metric));m=stats["mean"]
            means.append(np.nan if m is None else m)
            low.append(0 if m is None else max(0,m-stats["ci95"][0]))
            high.append(0 if m is None else max(0,stats["ci95"][1]-m))
        axis.errorbar(positions+(i-1)*offset,means,yerr=[low,high],fmt="o",capsize=3,label=LABELS[arch],color=COLORS[arch])
    axis.set_xticks(positions,[c.replace("matched_rate_random","matched random").replace("plausible_wrong","plausible wrong").replace("_"," ") for c in conditions],rotation=25,ha="right")
    axis.set_ylabel(ylabel);axis.legend(frameon=False)


def nondominated(xs,ys):
    return [i for i in range(len(xs)) if not any(xs[j]<=xs[i] and ys[j]>=ys[i] and (xs[j]<xs[i] or ys[j]>ys[i]) for j in range(len(xs)))]


def render(artifacts):
    root=Path(artifacts);root.mkdir(parents=True,exist_ok=True)
    plt.rcParams.update({"font.family":"DejaVu Sans","font.size":10,"axes.titlesize":12})
    voi=read(root,"voi_results.json")
    policy=read(root,"policy_interventions.json")
    responses=read(root,"language_response_ablation.json")
    retention=read(root,"retention_results.json")
    ood=read(root,"ood_results.json")
    main=read(root,"run_summary.json")
    generated=[]
    def save(fig,name):finish(fig,root/name);generated.append(name)
    fig,ax=plt.subplots(figsize=(7,4))
    for arch in ARCHES:
        rows=[r for r in voi if r["architecture"]==arch and r["dimension"]=="cost"]
        draw_curve(ax,rows,"value","llm_call_rate",LABELS[arch],COLORS[arch])
    ax.axvline(.4,color="#777",linestyle="--",linewidth=.8,label="Requested base grid upper cost")
    ax.set(xlabel="Language cost / accepted call (reward units)",ylabel="Episodes with accepted call (fraction)",title="Value of information: cost response")
    ax.legend(frameon=False);save(fig,"voi_cost_curve.png")
    fig,axes=plt.subplots(1,2,figsize=(11,4))
    for arch in ARCHES:
        rows=[r for r in voi if r["architecture"]==arch and r["dimension"]=="cost"]
        for ax,metric in zip(axes,["task_success","reward"]):draw_curve(ax,rows,"value",metric,LABELS[arch],COLORS[arch])
    axes[0].set(ylabel="Task success (fraction)");axes[1].set(ylabel="Reward / episode")
    for ax in axes:ax.set_xlabel("Language cost / accepted call");ax.legend(frameon=False)
    fig.suptitle("Cost-performance tradeoff; bands: seed bootstrap 95% CI")
    save(fig,"voi_performance_curve.png")
    fig,ax=plt.subplots(figsize=(7,4))
    for arch in ARCHES:
        draw_curve(ax,[r for r in voi if r["architecture"]==arch and r["dimension"]=="reliability"],"value","task_success",LABELS[arch],COLORS[arch])
    ax.set(xlabel="Scripted response reliability (probability)",ylabel="Task success (fraction)",title="Reliability sensitivity");ax.legend(frameon=False);save(fig,"reliability_curve.png")
    fig,axes=plt.subplots(1,2,figsize=(11,4))
    for arch in ARCHES:
        rows=[r for r in voi if r["architecture"]==arch and r["dimension"]=="latency"]
        for ax,metric in zip(axes,["llm_call_rate","reward"]):draw_curve(ax,rows,"value",metric,LABELS[arch],COLORS[arch])
    axes[0].set_ylabel("Episodes with accepted call (fraction)");axes[1].set_ylabel("Reward / episode")
    for ax in axes:ax.set_xlabel("Additional response latency (environment steps)");ax.legend(frameon=False)
    save(fig,"latency_curve.png")
    fig,axes=plt.subplots(1,2,figsize=(12,4.5))
    conditions=["never","random","matched_rate_random","learned","always","oracle"]
    categorical(axes[0],policy,conditions,"task_success","Task success (fraction)")
    categorical(axes[1],policy,conditions,"reward","Reward / episode")
    fig.suptitle("Gate interventions on paired episodes; oracle gate retains learned downstream policy")
    save(fig,"policy_intervention.png")
    fig,ax=plt.subplots(figsize=(12,4.5))
    categorical(ax,responses,["correct","shuffled","random","inverted","delayed","missing","plausible_wrong","contradictory","uncertain"],"task_success","Task success (fraction)")
    ax.set_title("Returned-information dependence, same learned policy")
    save(fig,"response_ablation.png")
    fig,axes=plt.subplots(1,3,figsize=(14,4.5),sharey=True)
    modes=["normal","reset_step","reset_8","noise","quantize"]
    colors=["#087e8b","#ba3b46","#d39b28","#7e57a4","#64748b"]
    for ax,arch in zip(axes,ARCHES):
        for mode,color in zip(modes,colors):
            rows=[r for r in retention if r["architecture"]==arch and r["state_mode"]==mode]
            draw_curve(ax,rows,"delay","memory_retention",mode.replace("_"," "),color)
        ax.axhline(.5,color="#999",linestyle="--",linewidth=.8)
        all_delays=[r["delay"] for r in retention]
        ax.set_xscale("log",base=2)
        ax.set_xlim(min(all_delays)*.9 if all_delays else 8,max(all_delays)*1.1 if all_delays else 640)
        ax.set(title=LABELS[arch],xlabel="Cue-to-decision delay (steps)",ylim=(0,1.05))
    axes[0].set_ylabel("Strict-memory decision success (fraction)")
    for ax in reversed(axes):
        if ax.get_legend_handles_labels()[0]:
            ax.legend(frameon=False,fontsize=8)
            break
    episode_counts=sorted({r["metrics"]["episodes"] for r in retention})
    fig.suptitle(f"Memory retention; {'/'.join(map(str,episode_counts))} episodes / condition, independent seed uncertainty")
    save(fig,"retention_curve.png")
    fig,axes=plt.subplots(1,2,figsize=(11,4.5))
    ppo=[r for r in main if r["architecture"]=="gru128" and (r["checkpoint_name"]=="imitation_best.pt" or r["checkpoint_name"]=="ppo_final.pt")]
    arms=sorted({r["run_id"].split("-")[-1] for r in ppo})
    for ax,metric,ylabel in zip(axes,["task_success","reward"],["Task success (fraction)","Reward / episode"]):
        summaries=[seed_statistics([r.get(metric) for r in ppo if r["run_id"].endswith("-"+arm)]) for arm in arms]
        ys=[s["mean"] for s in summaries]
        if ys:
            ax.errorbar(range(len(arms)),ys,yerr=[[m-s["ci95"][0] for m,s in zip(ys,summaries)],[s["ci95"][1]-m for m,s in zip(ys,summaries)]],fmt="o-",color=COLORS["gru128"],capsize=4)
        for seed in sorted({r["seed"] for r in ppo}):
            by_arm={r["run_id"].split("-")[-1]:r[metric] for r in ppo if r["seed"]==seed}
            ax.plot(range(len(arms)),[by_arm.get(a,np.nan) for a in arms],color="#999",alpha=.25,linewidth=.8)
        ax.set_xticks(range(len(arms)),arms);ax.set_ylabel(ylabel)
    fig.suptitle("Paired GRU-128 PPO ablations; B0 imitation-best, B1–B8 PPO-final")
    save(fig,"ppo_ablation.png")
    conditions=sorted({r["condition"] for r in ood})
    matrix=np.full((len(ARCHES),len(conditions)),np.nan)
    for i,arch in enumerate(ARCHES):
        for j,condition in enumerate(conditions):
            vals=values([r for r in ood if r["architecture"]==arch and r["condition"]==condition],"task_success")
            if vals:matrix[i,j]=np.mean(vals)
    fig,ax=plt.subplots(figsize=(14,4))
    im=ax.imshow(matrix,vmin=0,vmax=1,cmap="viridis",aspect="auto")
    ax.set_xticks(range(len(conditions)),[x.replace("_"," ") for x in conditions],rotation=40,ha="right")
    ax.set_yticks(range(3),[LABELS[a] for a in ARCHES]);ax.set_title("OOD task success; architecture mean across independent seeds")
    for i in range(len(ARCHES)):
        for j in range(len(conditions)):
            if np.isfinite(matrix[i,j]):ax.text(j,i,f"{matrix[i,j]:.2f}",ha="center",va="center",fontsize=8,color="white" if matrix[i,j]<.6 else "black")
    fig.colorbar(im,ax=ax,label="Task success (fraction)");save(fig,"ood_heatmap.png")
    baseline=[r for r in main if r["checkpoint_name"]=="imitation_best.pt"]
    fig,axes=plt.subplots(2,2,figsize=(11,8))
    for ax,key,xlabel in zip(axes.flat,["llm_call_rate","cpu_inference_ms_median","params","estimated_compute_proxy_active_connections"],
        ["Episodes with accepted call (fraction)","CPU inference median (ms, batch 1, 4 threads)","Parameter count","Logical active connections (compute proxy, not FLOPs)"]):
        xs=[];ys=[]
        for arch in ARCHES:
            rows=[r for r in baseline if r["architecture"]==arch and r.get(key) is not None]
            if not rows:continue
            x=np.mean([r[key] for r in rows]);stats=seed_statistics([r["task_success"] for r in rows]);y=stats["mean"]
            ax.scatter([r[key] for r in rows],[r["task_success"] for r in rows],alpha=.3,s=18,color=COLORS[arch])
            ax.errorbar(x,y,yerr=[[y-stats["ci95"][0]],[stats["ci95"][1]-y]],fmt="o",color=COLORS[arch],capsize=3,label=LABELS[arch])
            xs.append(x);ys.append(y)
        indices=sorted(nondominated(xs,ys),key=lambda i:xs[i])
        ax.plot([xs[i] for i in indices],[ys[i] for i in indices],"--",color="#222",linewidth=1)
        ax.set(xlabel=xlabel,ylabel="Task success (fraction)");ax.legend(frameon=False)
    fig.suptitle("Pareto comparisons: lower cost, higher success; small dots are independent seeds")
    save(fig,"pareto.png")
    manifest={"files":generated,"inputs":["voi_results.json","policy_interventions.json","language_response_ablation.json","retention_results.json","ood_results.json","run_summary.json"],
        "uncertainty":"95% percentile bootstrap of independent training-seed means; episode count is not n",
        "call_rate_unit":"fraction of episodes containing one accepted language call","reward_unit":"sum of step rewards per episode",
        "cpu_latency_unit":"milliseconds, batch=1, four CPU threads","plot_count":len(generated)}
    (root/"plot_manifest.json").write_text(json.dumps(manifest,indent=2))
    return manifest


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--artifacts",required=True)
    args=parser.parse_args();print(json.dumps(render(args.artifacts),indent=2))
