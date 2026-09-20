"""K0-F2 scientific figures from saved evidence; no model runs or invented values."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

COLORS = ("#1677a5", "#c34b32", "#69882b", "#9462ac", "#bf8530", "#5e7181")
MODES = ("BODY", "BLIND", "SHUFFLED", "STALE")
FRAME_NAMES = (
    "master_cpu_thermal", "master_cpu_busy", "master_ram_pressure", "master_io_pressure",
    "rtx3060_thermal", "rtx3060_compute_busy", "rtx3060_vram_pressure", "rtx3060_power_pressure",
    "p100_thermal", "p100_compute_busy", "p100_vram_pressure", "p100_power_pressure",
    "mac_thermal", "mac_cpu_busy", "mac_memory_pressure", "mac_power_pressure",
    "network_latency", "network_loss", "body_staleness", "body_availability")


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def read(root, name):
    path = root / name
    if not path.exists():
        return None
    if path.suffix == ".jsonl":
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return json.loads(path.read_text())


def rows(data):
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
        for key in ("rows", "records", "results", "runs"):
            if isinstance(data.get(key), list):
                return data[key]
    return []


def seed_summary(records, metric):
    """CIs use independent training seeds, never episodes or 1 Hz samples."""
    valid = [record for record in records if finite(record.get(metric))]
    if not valid:
        return None
    seeds = [record.get("seed") for record in valid]
    if any(seed is None for seed in seeds) or len(set(seeds)) != len(seeds):
        raise ValueError(f"Unique independent seed required for {metric}: {seeds}")
    values = np.array([record[metric] for record in valid], dtype=float)
    rng = np.random.default_rng(1847)
    means = rng.choice(values, size=(20000, len(values))).mean(axis=1)
    return {"mean": float(values.mean()), "ci95": np.quantile(means, [.025, .975]).tolist(), "values": values, "n": len(values)}


def missing(axis, reason="Not collected / not evaluated"):
    axis.text(.5, .5, reason, ha="center", va="center", transform=axis.transAxes, color="#6b7280", wrap=True)
    axis.set_xticks([])
    axis.set_yticks([])


def finish(fig, path):
    for axis in fig.axes:
        axis.spines["top"].set_visible(False)
        axis.spines["right"].set_visible(False)
        axis.grid(alpha=.16)
    fig.tight_layout(rect=(0, .055, 1, .94))
    fig.savefig(path, dpi=170, bbox_inches="tight")
    plt.close(fig)


def raw_series(records, metric, origin, kind="real", scale=1):
    x, y = [], []
    for row in records:
        if not finite(row.get("timestamp")):
            continue
        value = row.get("metrics", {}).get(metric)
        quality = row.get("quality", {}).get(metric, 1 if value is not None else 0)
        valid = row.get("source_kind") == kind and finite(value) and finite(quality) and quality > 0
        x.append(row["timestamp"] - origin)
        y.append(value * scale if valid else np.nan)
    return x, y


def plot_raw(axis, records, metric, origin, label, color, scale=1):
    plotted = False
    for kind in sorted({row.get("source_kind", "unknown") for row in records}):
        x, y = raw_series(records, metric, origin, kind, scale)
        if any(np.isfinite(y)):
            axis.plot(x, y, label=f"{label} ({kind})", color=color, linewidth=1.1, linestyle="-" if kind == "real" else "--")
            plotted = True
    return plotted


def grouped_seed_plot(axis, records, metric, modes, ylabel):
    architectures = sorted({record.get("architecture", "unknown") for record in records})
    present = False
    for arch_index, architecture in enumerate(architectures):
        offset = (arch_index - (len(architectures) - 1) / 2) * .18
        for mode_index, mode in enumerate(modes):
            selected = [record for record in records if record.get("architecture", "unknown") == architecture and record.get("mode", record.get("condition")) == mode]
            stat = seed_summary(selected, metric)
            if stat is None:
                continue
            color = COLORS[arch_index % len(COLORS)]
            x = mode_index + offset
            axis.scatter(np.full(stat["n"], x), stat["values"], color=color, alpha=.35, s=20)
            mean, (lo, hi) = stat["mean"], stat["ci95"]
            axis.errorbar(x, mean, yerr=[[max(0, mean-lo)], [max(0, hi-mean)]], fmt="o", color=color, capsize=4,
                          label=f"{architecture} (n={stat['n']} seeds)" if mode_index == 0 else None)
            present = True
    axis.set_xticks(range(len(modes)), modes, rotation=20, ha="right")
    axis.set_ylabel(ylabel)
    if present:
        axis.legend(frameon=False, fontsize=8)
    else:
        missing(axis)
    return present


def render(artifacts):
    from .report import probe_numerical_gate
    root=Path(artifacts);root.mkdir(parents=True,exist_ok=True)
    plt.rcParams.update({'font.family':'DejaVu Sans','font.size':9,'axes.titlesize':11})
    base_names=('raw_mac_telemetry.jsonl','raw_master_telemetry.jsonl','interoceptive_frames.jsonl','probe_validation.json','protocol_lock.json','fixture_metadata.json')
    inputs={name:read(root,name) for name in base_names}
    probe=inputs['probe_validation.json'] if isinstance(inputs['probe_validation.json'],dict) else {}
    gate_pass,_=probe_numerical_gate(probe)
    blocked=probe.get('gate',{}).get('pass') is False or probe.get('status')=='FAIL_AT_PROBE_GATE' or probe.get('gate',{}).get('pass') is True and not gate_pass
    status='FAIL_AT_PROBE_GATE' if blocked else 'GATE_PASS' if gate_pass else 'GATE_UNRESOLVED'
    downstream_names=('test_results.json','live_results.json','counterfactual_results.json','ood_results.json','policy_traces.jsonl')
    # Do not open policy/test/live outcome files until the fixed gate is valid.
    if gate_pass and not blocked:inputs.update({name:read(root,name) for name in downstream_names})
    mac,master,frames=(rows(inputs[name]) for name in base_names[:3])
    probe_rows=rows(probe);selection=probe.get('selection_rows',[])
    test=rows(inputs.get('test_results.json'));live=rows(inputs.get('live_results.json'))
    counter=rows(inputs.get('counterfactual_results.json'));ood=rows(inputs.get('ood_results.json'))
    traces=rows(inputs.get('policy_traces.jsonl'))
    test=[dict(r,mode='TRAINED_BLIND') if r.get('training_mode')=='BLIND' else r for r in test]
    timestamps=[r['timestamp'] for r in mac+master if finite(r.get('timestamp'))];origin=min(timestamps) if timestamps else 0
    manifest={'schema_version':'k0f2.plots.v1','files':[],'inputs':{},'protocol_scope':'F2_confirmatory',
              'probe_display_split':'validation','probe_test_evaluated':False,'stage_status':status,'n_seeds_locked':12,
              'minimum_effect_locked':.01,'downstream_opened':gate_pass and not blocked,
              'uncertainty':'20,000 bootstrap resamples of independent training-seed means; raw telemetry remains descriptive'}
    fixture=bool(isinstance(inputs['fixture_metadata.json'],dict) and inputs['fixture_metadata.json'].get('synthetic_fixture') is True)
    manifest['synthetic_fixture']=fixture
    for name in inputs:
        if (root/name).exists():manifest['inputs'][name]=hashlib.sha256((root/name).read_bytes()).hexdigest()
    no_downstream='NOT RUN: probe gate failed\ntest / policy / live remain unopened' if blocked else 'Not run / no result at this stage'
    def save(fig,name,available):
        footer='SYNTHETIC FIXTURE ONLY | ' if fixture else ''
        fig.text(.5,.01,footer+'F2 confirmatory | 12 seeds locked | '+status,ha='center',va='bottom',fontsize=9,color='#933d2e' if blocked else '#446171')
        finish(fig,root/name)
        manifest['files'].append({'name':name,'status':'data_present' if available else 'NOT_RUN_PROBE_GATE_FAILED' if blocked else 'not_evaluated',
                                  'sha256':hashlib.sha256((root/name).read_bytes()).hexdigest()})

    fig,axes=plt.subplots(3,2,figsize=(13,10));present=False
    specifications=(('Temperature','Degrees C',((master,'cpu_temperature_c','Master CPU',1),(master,'rtx3060_temperature_c','RTX3060',1),(master,'p100_temperature_c','P100',1))),
                    ('Compute load','Percent',((mac,'cpu_utilization','Mac CPU',100),(master,'cpu_utilization','Master CPU',100),(master,'rtx3060_utilization','RTX3060',100),(master,'p100_utilization','P100',100))),
                    ('VRAM used','GiB',((master,'rtx3060_vram_used_bytes','RTX3060',1/2**30),(master,'p100_vram_used_bytes','P100',1/2**30))),
                    ('Memory / Mac thermal pressure','Fraction',((mac,'memory_pressure','Mac memory',1),(master,'memory_pressure','Master RAM',1),(mac,'thermal_pressure','Mac thermal',1))),
                    ('Network RTT','ms',((mac,'network_rtt_ms','Mac to master',1),(master,'network_rtt_ms','Master to Mac',1))))
    for axis,(title,ylabel,series) in zip(axes.flat,specifications):
        plotted=[plot_raw(axis,data,key,origin,label,COLORS[i],scale) for i,(data,key,label,scale) in enumerate(series)]
        axis.set(title=title,xlabel='Seconds since fresh recording start',ylabel=ylabel)
        if any(plotted):axis.legend(frameon=False,fontsize=7);present=True
        else:missing(axis)
    action_names=('RUN_CPU','RUN_RTX3060','RUN_P100','WAIT')
    values=[action_names.index(r['action']) if r.get('action') in action_names else np.nan for r in traces]
    if values:
        axes.flat[5].plot(range(len(values)),values,'.',markersize=2,color=COLORS[0]);axes.flat[5].set_yticks(range(4),action_names)
        axes.flat[5].set(title='All seed / mode replay records',xlabel='Trace record index',ylabel='Action')
    else:missing(axes.flat[5],no_downstream)
    fig.suptitle('Fresh K0-F2 body telemetry; solid = real, dashed = synthetic; gaps = unavailable sensors')
    save(fig,'body_timeseries.png',present)

    fig,axes=plt.subplots(1,2,figsize=(11,4.5));present=False
    for axis,target in zip(axes,('log_latency','utility')):
        selected=[r for r in probe_rows if r.get('target')==target and r.get('split')=='validation']
        for i,mode in enumerate(MODES):
            group=[r for r in selected if r.get('mode')==mode]
            if len(group)==1 and finite(group[0].get('mae')):axis.bar(i,group[0]['mae'],color=COLORS[i],width=.65);present=True
            elif len(group)>1:raise ValueError('Ambiguous duplicate probe target/mode')
        if not selected:missing(axis,'Validation probe not evaluated')
        else:axis.set_xticks(range(4),MODES,rotation=20,ha='right')
        axis.set(title=target.replace('_',' '),ylabel='Validation MAE (lower is better)')
    fig.suptitle('Fixed ridge action-conditioned outcomes; validation only, no test probe')
    save(fig,'action_conditioned_probe.png',present)

    fig,axes=plt.subplots(1,2,figsize=(10,4.5));present=False
    for axis,metric,label in zip(axes,('selection_regret','accuracy'),('Measured utility regret (lower is better)','Resource accuracy (higher is better)')):
        for i,mode in enumerate(MODES):
            group=[r for r in selection if r.get('mode')==mode]
            if len(group)==1 and finite(group[0].get(metric)):axis.bar(i,group[0][metric],color=COLORS[i]);present=True
            elif len(group)>1:raise ValueError('Ambiguous duplicate probe selection/mode')
        axis.set(ylabel=label)
        if selection:axis.set_xticks(range(4),MODES,rotation=20,ha='right')
        else:missing(axis,'Validation action selection not evaluated')
    fig.suptitle('Argmax predicted utility: validation action selection, not oracle inputs')
    save(fig,'probe_regret.png',present)

    for name,key,control,data,title in (
        ('body_vs_blind.png','P1','TRAINED_BLIND',test,'P1: BODY versus independently trained BLIND'),
        ('body_vs_shuffled.png','P2','SHUFFLED',test,'P2: BODY versus SHUFFLED'),
        ('body_vs_stale.png','P3','STALE',test,'P3: BODY versus primary 30-second STALE'),
        ('live_body_vs_blind.png','P4','TRAINED_BLIND',live,'P4: new live jobs, BODY versus independently trained BLIND')):
        fig,axes=plt.subplots(1,2,figsize=(10,4.5));present=False
        for axis,metric,label in zip(axes,('utility','latency_seconds'),('Mean task utility','Mean latency (seconds)')):
            if data:present|=grouped_seed_plot(axis,data,metric,('BODY',control),label)
            else:missing(axis,no_downstream);axis.set_ylabel(label)
        fig.suptitle(title+'\nLocked primary: mean gain >= 0.01, CI lower > 0, exact sign p <= 0.05',fontsize=11)
        save(fig,name,present)

    fig,axes=plt.subplots(1,3,figsize=(12,4.5));present=False
    for axis,metric,label in zip(axes,('action_change_rate','utility_gain','latency_gain'),('Action change rate','Matched minus frozen utility','Frozen minus matched latency (s)')):
        augmented=[]
        for r in counter:
            entry=dict(r)
            if finite(r.get('frozen_latency_seconds')) and finite(r.get('matched_latency_seconds')):entry['latency_gain']=r['frozen_latency_seconds']-r['matched_latency_seconds']
            augmented.append(entry)
        stat=seed_summary(augmented,metric)
        if stat:
            x=stat['mean'];lo,hi=stat['ci95'];axis.scatter(np.zeros(stat['n']),stat['values'],alpha=.35,color=COLORS[0]);axis.errorbar(0,x,yerr=[[max(0,x-lo)],[max(0,hi-x)]],fmt='o',capsize=4,color=COLORS[0]);axis.set_xticks([0],['GRU128']);present=True
        else:missing(axis,no_downstream)
        axis.set_ylabel(label)
    fig.suptitle('Body-only fork: all eligible pairs, identical task / history / hidden state')
    save(fig,'counterfactual_body.png',present)

    for name,category in (('sensor_ood_heatmap.png','sensor'),('temporal_ood.png','temporal')):
        selected=[r for r in ood if r.get('category')==category]
        conditions=sorted({r.get('mode') for r in selected});fig,axis=plt.subplots(figsize=(max(9,len(conditions)*.65),4.7))
        present=False
        if conditions:
            values=[]
            for condition in conditions:
                stat=seed_summary([r for r in selected if r.get('mode')==condition],'utility');values.append(stat['mean'] if stat else np.nan)
            if any(np.isfinite(values)):
                im=axis.imshow([values],aspect='auto',cmap='viridis');axis.set_xticks(range(len(conditions)),[v.replace('_',' ') for v in conditions],rotation=35,ha='right');axis.set_yticks([0],['GRU128'])
                for i,value in enumerate(values):
                    if np.isfinite(value):axis.text(i,0,f'{value:.3f}',ha='center',va='center',color='white',fontsize=8,bbox={'facecolor':'black','alpha':.25,'pad':1,'edgecolor':'none'})
                fig.colorbar(im,ax=axis,label='Mean utility across training seeds');present=True
            else:missing(axis,no_downstream)
        else:missing(axis,no_downstream)
        fig.suptitle(('Sensor' if category=='sensor' else 'Temporal')+' OOD: controlled perturbations; no induced physical fault')
        save(fig,name,present)

    fig,axes=plt.subplots(1,3,figsize=(12,4.8));present=False
    for axis,index,label in zip(axes,(1,5,9),('CPU busy','RTX3060 busy','P100 busy')):
        groups=[[] for _ in range(4)]
        for row in traces:
            body=row.get('body_values',[]);mask=row.get('body_mask',[])
            if row.get('mode')=='BODY' and row.get('training_mode','BODY')=='BODY' and len(body)==20 and len(mask)==20 and mask[index] and finite(body[index]) and row.get('action') in action_names:
                groups[min(3,int(body[index]*4))].append(row['action'])
        bottom=np.zeros(4)
        for i,action in enumerate(action_names):
            values=np.array([g.count(action)/len(g) if g else np.nan for g in groups])
            if any(np.isfinite(values)):axis.bar(range(4),values,bottom=bottom,color=COLORS[i],label=action);bottom+=np.nan_to_num(values);present=True
        axis.set(xlabel=label+' (fraction)',ylabel='Saved action fraction',ylim=(0,1.05))
        if any(groups):axis.set_xticks(range(4),[f'{i/4:.2f}-{(i+1)/4:.2f}\nn={len(g)}' for i,g in enumerate(groups)])
        else:missing(axis,no_downstream)
    if present:
        handles,labels=axes[0].get_legend_handles_labels()
        fig.legend(handles,labels,loc='upper center',bbox_to_anchor=(.5,.94),ncol=4,frameon=False,fontsize=8)
    fig.suptitle('BODY action map; trace counts are descriptive, not independent n')
    save(fig,'policy_action_map.png',present)

    fig,axis=plt.subplots(figsize=(8,5));present=False
    for i,mode in enumerate(('BODY','TRAINED_BLIND','SHUFFLED','STALE')):
        selected=[r for r in test if r.get('mode')==mode and finite(r.get('utility')) and finite(r.get('latency_seconds'))]
        x=seed_summary(selected,'latency_seconds');y=seed_summary(selected,'utility')
        if x and y:
            axis.scatter(x['values'],y['values'],color=COLORS[i],alpha=.35);axis.errorbar(x['mean'],y['mean'],xerr=[[max(0,x['mean']-x['ci95'][0])],[max(0,x['ci95'][1]-x['mean'])]],yerr=[[max(0,y['mean']-y['ci95'][0])],[max(0,y['ci95'][1]-y['mean'])]],fmt='o',color=COLORS[i],capsize=3,label=mode);present=True
    if present:axis.legend(frameon=False)
    else:missing(axis,no_downstream)
    axis.set(xlabel='Measured completion latency (s; lower is better)',ylabel='Task utility (higher is better)')
    fig.suptitle('Held-out measured-cost replay tradeoff; new live jobs reported separately')
    save(fig,'pareto.png',present)
    manifest['plot_count']=len(manifest['files']);(root/'plot_manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    return manifest


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();print(json.dumps(render(a.artifacts),indent=2))
