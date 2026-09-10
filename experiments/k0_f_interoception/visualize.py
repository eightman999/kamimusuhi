"""K0-F scientific figures from saved evidence; no model runs or invented values."""
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
    rng = np.random.default_rng(8101)
    means = rng.choice(values, size=(10000, len(values))).mean(axis=1)
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
    fig.tight_layout(rect=(0, 0, 1, .96))
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
    root = Path(artifacts)
    root.mkdir(parents=True, exist_ok=True)
    plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 9, "axes.titlesize": 11})
    names = ("raw_mac_telemetry.jsonl", "raw_master_telemetry.jsonl", "interoceptive_frames.jsonl", "policy_traces.jsonl",
             "prediction_probe.json", "ablation_results.json", "counterfactual_body.json", "ood_results.json", "run_summary.json")
    inputs = {name: read(root, name) for name in names}
    mac, master, frames, traces = (rows(inputs[name]) for name in names[:4])
    probe, ablation, counter, ood = (rows(inputs[name]) for name in names[4:8])
    # A separately trained BLIND model is a distinct control, not a duplicate seed
    # of the BODY model's BLIND input intervention.
    ablation = [dict(row, mode="TRAINED_BLIND") if row.get("training_mode") == "BLIND" else row for row in ablation]
    timestamps = [record["timestamp"] for record in mac + master if finite(record.get("timestamp"))]
    origin = min(timestamps) if timestamps else 0
    manifest = {"schema_version": "k0f.plots.v1", "files": [], "inputs": {},
                "uncertainty": "95% percentile bootstrap over independent training seeds; 1 Hz telemetry is descriptive only",
                "missing_data": "Unavailable panels explicitly marked; no substitute success values",
                "raw_source_kinds": sorted({record.get("source_kind", "unknown") for record in mac + master}),
                "trace_source_kinds": sorted({record.get("source_kind", "unknown") for record in traces}),
                "policy_evidence": "Trace replay / counterfactual estimates are not proof of live chosen-job execution"}
    for name in names:
        if (root / name).exists():
            manifest["inputs"][name] = hashlib.sha256((root / name).read_bytes()).hexdigest()
    def save(fig, name, available):
        finish(fig, root / name)
        manifest["files"].append({"name": name, "status": "data_present" if available else "not_collected",
                                  "sha256": hashlib.sha256((root / name).read_bytes()).hexdigest()})

    fig, axes = plt.subplots(3, 2, figsize=(13, 10))
    specs = (
        ("Temperature", "Degrees C", ((master, "cpu_temperature_c", "Master CPU", 1), (master, "rtx3060_temperature_c", "RTX 3060", 1), (master, "p100_temperature_c", "P100", 1))),
        ("Utilization", "Percent", ((mac, "cpu_utilization", "Mac CPU", 100), (master, "cpu_utilization", "Master CPU", 100), (master, "rtx3060_utilization", "RTX 3060", 100), (master, "p100_utilization", "P100", 100))),
        ("GPU memory", "GiB used", ((master, "rtx3060_vram_used_bytes", "RTX 3060", 1/2**30), (master, "p100_vram_used_bytes", "P100", 1/2**30))),
        ("Memory and Mac thermal pressure", "Fraction", ((mac, "memory_pressure", "Mac memory proxy", 1), (master, "memory_pressure", "Master RAM", 1), (mac, "thermal_pressure", "Mac thermal", 1))),
        ("Network RTT", "Milliseconds", ((mac, "network_rtt_ms", "Mac to master", 1), (master, "network_rtt_ms", "Master to Mac", 1))))
    present = False
    for axis, (title, ylabel, series) in zip(axes.flat, specs):
        plotted = [plot_raw(axis, records, key, origin, label, COLORS[index], scale) for index, (records, key, label, scale) in enumerate(series)]
        axis.set(title=title, xlabel="Seconds since recording start", ylabel=ylabel)
        if any(plotted):
            axis.legend(frameon=False, fontsize=7)
            present = True
        else:
            missing(axis)
    action_names = sorted({row.get("action") for row in traces if isinstance(row.get("action"), str)})
    if action_names:
        axis = axes.flat[5]
        axis.plot(range(len(traces)), [action_names.index(row["action"]) if row.get("action") in action_names else np.nan for row in traces], ".", color=COLORS[0], markersize=2)
        axis.set_yticks(range(len(action_names)), action_names)
        axis.set(title="Core actions (saved policy replay)", xlabel="Trace record index", ylabel="Action")
    else:
        missing(axes.flat[5], "Core action trace not collected")
    fig.suptitle("K0-F body telemetry; solid = real, dashed = synthetic; gaps = missing sensors")
    save(fig, "body_timeseries.png", present)

    fig, axes = plt.subplots(1, 3, figsize=(13, 4))
    present = False
    for axis, device in zip(axes, ("cpu", "rtx3060", "p100")):
        pairs = [(row["metrics"].get(device + "_utilization"), row["metrics"].get(device + "_temperature_c")) for row in master if row.get("source_kind") == "real" and isinstance(row.get("metrics"), dict)
                 and row.get("quality", {}).get(device + "_utilization", 1) > 0 and row.get("quality", {}).get(device + "_temperature_c", 1) > 0]
        pairs = [(x, y) for x, y in pairs if finite(x) and finite(y)]
        if pairs:
            axis.scatter([x*100 for x, _ in pairs], [y for _, y in pairs], c=COLORS[0], alpha=.22, s=8)
            present = True
        else:
            missing(axis)
        axis.set(title={"cpu": "Master CPU", "rtx3060": "RTX 3060", "p100": "P100"}[device], xlabel="Utilization (%)", ylabel="Temperature (C)")
    fig.suptitle("Real temperature / load association; telemetry points are not independent n")
    save(fig, "thermal_load_relationship.png", present)

    fig, axes = plt.subplots(2, 2, figsize=(12, 7))
    present = False
    for axis, indices, title in zip(axes.flat, ((1, 2, 3), (5, 6, 7), (9, 10, 11), (12, 13, 14, 15)),
                                   ("Master CPU / RAM / I/O", "RTX 3060", "P100", "Mac peripheral")):
        plotted = False
        for color_index, index in enumerate(indices):
            selected = [frame for frame in frames if frame.get("source_kind") == "real" and finite(frame.get("timestamp"))]
            x = [frame["timestamp"] - origin for frame in selected]
            y = [frame["values"][index] if len(frame.get("values", [])) == 20 and len(frame.get("mask", [])) == 20 and frame["mask"][index] == 1 else np.nan for frame in selected]
            if any(np.isfinite(y)):
                axis.plot(x, y, label=FRAME_NAMES[index], color=COLORS[color_index], linewidth=1)
                plotted = True
        axis.set(title=title, xlabel="Seconds since recording start", ylabel="Normalized pressure", ylim=(-.02, 1.03))
        if plotted:
            axis.legend(frameon=False, fontsize=7)
            present = True
        else:
            missing(axis, "Valid real normalized frames not collected")
    fig.suptitle("Interoceptive frame: real telemetry, fixed normalization, masked values excluded")
    save(fig, "resource_pressure.png", present)

    fig, axes = plt.subplots(2, 2, figsize=(12, 7))
    present = False
    for axis, metric, label, scale in zip(axes.flat, ("network_rtt_ms", "network_loss", "network_connectivity", "daemon_cpu_fraction"),
                                        ("RTT (ms)", "Packet loss (fraction)", "Connectivity observation", "Sensor daemon CPU (fraction)"), (1, 1, 1, 1)):
        plotted = [plot_raw(axis, records, metric, origin, name, COLORS[index], scale) for index, (records, name) in enumerate(((mac, "Mac"), (master, "Master")))]
        axis.set(xlabel="Seconds since recording start", ylabel=label)
        if any(plotted):
            axis.legend(frameon=False, fontsize=8)
            present = True
        else:
            missing(axis)
    fig.suptitle("Network body state and collection overhead; connection failures remain missing RTT")
    save(fig, "network_body_state.png", present)

    targets = sorted({str(row.get("target")) for row in probe})
    fig, axes = plt.subplots(1, max(1, len(targets)), figsize=(max(7, len(targets)*4), 4), squeeze=False)
    present = False
    for axis, target in zip(axes.flat, targets or [None]):
        if target is None:
            missing(axis, "Prediction probe not evaluated")
            continue
        selected = [row for row in probe if str(row.get("target")) == target and row.get("split") in ("test", "heldout", "held_out")]
        if not selected:
            missing(axis, "Held-out probe results not collected")
            continue
        for index, mode in enumerate(MODES):
            group = [row for row in selected if row.get("mode") == mode and finite(row.get("normalized_mae", row.get("mae")))]
            if len(group) == 1:
                axis.bar(index, group[0].get("normalized_mae", group[0].get("mae")), color=COLORS[index], width=.65)
                present = True
            elif len(group) > 1:
                raise ValueError(f"Ambiguous probe aggregation for {target}/{mode}")
        axis.set_xticks(range(len(MODES)), MODES, rotation=25, ha="right")
        axis.set(title=target.replace("_", " "), ylabel="Normalized MAE" if any("normalized_mae" in row for row in selected) else "MAE")
    fig.suptitle("Held-out sensor informativeness; lower error is better; descriptive probe estimates")
    save(fig, "prediction_probe.png", present)

    fig, axes = plt.subplots(1, 3, figsize=(14, 4.7))
    modes = list(MODES) + sorted({row.get("mode", row.get("condition")) for row in ablation} - set(MODES) - {None})
    present = False
    for axis, metric, ylabel in zip(axes, ("utility", "latency_seconds", "deadline_success_rate"), ("Task utility", "Completion latency (seconds)", "Deadline success (fraction)")):
        present |= grouped_seed_plot(axis, ablation, metric, modes, ylabel)
    fig.suptitle("Matched body ablations; dots = training seeds, error bars = seed bootstrap 95% CI")
    save(fig, "body_ablation.png", present)

    fig, axes = plt.subplots(1, 3, figsize=(13, 4.5))
    present = False
    for axis, metric, label in zip(axes, ("action_change_rate", "utility_gain", "latency_improvement_seconds"),
                                   ("Action change rate (fraction)", "Matched minus frozen utility", "Frozen minus matched latency (seconds)")):
        augmented = []
        for row in counter:
            value = dict(row)
            if finite(row.get("matched_latency_seconds")) and finite(row.get("frozen_latency_seconds")):
                value["latency_improvement_seconds"] = row["frozen_latency_seconds"] - row["matched_latency_seconds"]
            augmented.append(value)
        for index, architecture in enumerate(sorted({row.get("architecture", "unknown") for row in counter})):
            stat = seed_summary([row for row in augmented if row.get("architecture", "unknown") == architecture], metric)
            if stat is None:
                continue
            mean, (lo, hi) = stat["mean"], stat["ci95"]
            axis.scatter(np.full(stat["n"], index), stat["values"], color=COLORS[index], alpha=.4)
            axis.errorbar(index, mean, yerr=[[max(0, mean-lo)], [max(0, hi-mean)]], fmt="o", color=COLORS[index], capsize=4)
            axis.set_xticks(range(index+1), sorted({row.get("architecture", "unknown") for row in counter})[:index+1])
            present = True
        if not axis.lines and not axis.collections:
            missing(axis)
        axis.set_ylabel(label)
        if metric != "action_change_rate":
            axis.axhline(0, color="#777", linewidth=.8)
    fig.suptitle("Counterfactual body fork: matched versus frozen action on saved measured outcomes")
    save(fig, "counterfactual_body.png", present)

    architectures = sorted({row.get("architecture", "unknown") for row in ood})
    conditions = sorted({str(row.get("mode", row.get("condition"))) for row in ood})
    fig, axis = plt.subplots(figsize=(max(10, len(conditions)*.7), 4))
    present = bool(architectures and conditions)
    if present:
        matrix = np.full((len(architectures), len(conditions)), np.nan)
        for i, architecture in enumerate(architectures):
            for j, condition in enumerate(conditions):
                selected = [row for row in ood if row.get("architecture", "unknown") == architecture and str(row.get("mode", row.get("condition"))) == condition]
                stat = seed_summary(selected, "utility")
                if stat:
                    matrix[i, j] = stat["mean"]
        if np.isfinite(matrix).any():
            im = axis.imshow(matrix, aspect="auto", cmap="viridis")
            axis.set_xticks(range(len(conditions)), [condition.replace("_", " ") for condition in conditions], rotation=40, ha="right")
            axis.set_yticks(range(len(architectures)), architectures)
            for i in range(len(architectures)):
                for j in range(len(conditions)):
                    if np.isfinite(matrix[i, j]):
                        axis.text(j, i, f"{matrix[i,j]:.3f}", ha="center", va="center", color="white", fontsize=8,
                                  bbox={"facecolor": "black", "alpha": .2, "edgecolor": "none", "pad": 1})
            fig.colorbar(im, ax=axis, label="Mean task utility across training seeds")
        else:
            missing(axis)
            present = False
    else:
        missing(axis, "Temporal / sensor OOD not evaluated")
    fig.suptitle("Controlled synthetic observation perturbations over held-out real telemetry")
    save(fig, "ood_heatmap.png", present)

    fig, axes = plt.subplots(1, 3, figsize=(13, 4.7))
    present = False
    actions = ("RUN_CPU", "RUN_RTX3060", "RUN_P100")
    for axis, index, label in zip(axes, (1, 5, 9), ("Master CPU busy", "RTX 3060 busy", "P100 busy")):
        grouped = [[] for _ in range(4)]
        for row in traces:
            if row.get("mode", row.get("condition")) != "BODY":
                continue
            body = row.get("body_values", row.get("body", []))
            value = row.get(FRAME_NAMES[index])
            if not finite(value) and isinstance(body, list) and len(body) >= 20:
                value = body[index]
            if finite(value) and row.get("action") in actions:
                grouped[min(3, max(0, int(value*4)))].append(row["action"])
        bottom = np.zeros(4)
        for action_index, action in enumerate(actions):
            proportions = np.array([group.count(action)/len(group) if group else np.nan for group in grouped])
            if any(np.isfinite(proportions)):
                axis.bar(range(4), proportions, bottom=bottom, color=COLORS[action_index], label=action)
                bottom += np.nan_to_num(proportions)
                present = True
        if not any(grouped):
            missing(axis, "BODY trace with current body values not collected")
        else:
            axis.set_xticks(range(4), [f"{lo:.2f}-{lo+.25:.2f}\nn={len(grouped[i])}" for i, lo in enumerate((0, .25, .5, .75))])
            axis.legend(frameon=False, fontsize=7)
        axis.set(xlabel=label + " (fraction)", ylabel="Saved action proportion", ylim=(0, 1.05))
    trace_origin = ", ".join(manifest["trace_source_kinds"]) or "not collected"
    fig.suptitle("Policy actions by body load; descriptive counts, not independent n\nSource: " + trace_origin, fontsize=10)
    save(fig, "policy_action_by_body_state.png", present)

    fig, axis = plt.subplots(figsize=(8, 5))
    present = False
    for index, mode in enumerate(modes):
        for architecture in sorted({row.get("architecture", "unknown") for row in ablation}):
            selected = [row for row in ablation if row.get("mode", row.get("condition")) == mode and row.get("architecture", "unknown") == architecture and finite(row.get("latency_seconds")) and finite(row.get("utility"))]
            if not selected:
                continue
            sx, sy = seed_summary(selected, "latency_seconds"), seed_summary(selected, "utility")
            x, y = sx["mean"], sy["mean"]
            axis.scatter(sx["values"], sy["values"], alpha=.3, s=20, color=COLORS[index % len(COLORS)])
            axis.errorbar(x, y, xerr=[[max(0, x-sx["ci95"][0])], [max(0, sx["ci95"][1]-x)]],
                          yerr=[[max(0, y-sy["ci95"][0])], [max(0, sy["ci95"][1]-y)]], fmt="o", capsize=3,
                          color=COLORS[index % len(COLORS)], label=f"{architecture} {mode}")
            present = True
    if present:
        axis.legend(frameon=False, fontsize=8)
    else:
        missing(axis, "Paired task utility and latency not evaluated")
    axis.set(xlabel="Mean completion latency (seconds; lower is better)", ylabel="Mean task utility (higher is better)")
    fig.suptitle("Utility / latency tradeoff on measured-outcome replay; seed bootstrap 95% CI")
    save(fig, "pareto.png", present)
    manifest["plot_count"] = len(manifest["files"])
    (root / "plot_manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", required=True)
    arguments = parser.parse_args()
    print(json.dumps(render(arguments.artifacts), indent=2))
