"""P0 plots: reads reports/results.jsonl + run metrics, writes PNGs.

    python -m experiments.p0.analysis.plots
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Dict, List

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from ..env import dynamics as dyn

BASE = Path(__file__).resolve().parent.parent
REPORTS = BASE / "reports"


def load_rows() -> List[Dict]:
    rows = []
    p = REPORTS / "results.jsonl"
    if p.exists():
        for line in p.read_text().splitlines():
            if line.strip():
                rows.append(json.loads(line))
    # group key collapsing per-permutation rows (causal:pe_shuffle_p0..p2
    # -> causal:pe_shuffle) for aggregate plotting
    import re
    for r in rows:
        r["condition_group"] = re.sub(r"_p\d+$", "", r["condition"])
    return rows


def _group(rows, key):
    out = {}
    for r in rows:
        out.setdefault(r[key], []).append(r)
    return out


def _meanstd(rows, field):
    vals = [r[field] for r in rows if r[field] == r[field]]  # drop nan
    if not vals:
        return float("nan"), float("nan")
    return float(np.mean(vals)), float(np.std(vals))


ID_ORDER = ["never", "random", "roundrobin", "always",
            "clock1", "clock2", "clock3", "clock4", "clock5",
            "fixed", "pe_heuristic", "pe_thresh", "trigger",
            "vtrigger", "vtrigger22", "cwatch",
            "bonly", "learned_final", "learned"]
LEARNED_HUES = {"learned": "#2a7", "learned_final": "#74b",
                "bonly": "#c60"}
# belief-reactive frontier policies (no predictor) get their own hue
BELIEF_HUES = {"vtrigger": "#a52a8a", "vtrigger22": "#a52a8a",
               "cwatch": "#a52a8a"}


def _color(a):
    if a in LEARNED_HUES:
        return LEARNED_HUES[a]
    if a in BELIEF_HUES:
        return BELIEF_HUES[a]
    if a.startswith("clock"):
        return "#57a"
    return "#888"


def plot_id_baselines(rows, out: Path):
    """Bar chart: detection / cost / efficiency per agent, ID only."""
    rows = [r for r in rows if r["condition"] == "id"]
    g = _group(rows, "agent")
    order = [a for a in ID_ORDER if a in g]
    fig, axes = plt.subplots(1, 3, figsize=(16, 3.8))
    for ax, field, title in zip(
        axes,
        ["detection_rate", "mean_cost", "info_efficiency"],
        ["D-event detection rate", "observation cost / step",
         "info efficiency (det/cost)"],
    ):
        means = [_meanstd(g[a], field)[0] for a in order]
        stds = [_meanstd(g[a], field)[1] for a in order]
        ax.bar(order, means, yerr=stds, capsize=3,
               color=[_color(a) for a in order])
        ax.set_title(title)
        ax.tick_params(axis="x", rotation=45)
    fig.suptitle("P0 v2 in-distribution agents (mean±sd over seeds)")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def plot_frontier(rows, out: Path):
    """Detection-vs-cost frontier: the pure-clock frontier line vs all agents.

    This is the central C1 plot: if the env is not clock-solvable, learned /
    surprise-driven points should lie ABOVE the clock frontier (better
    detection per unit cost).
    """
    rows = [r for r in rows if r["condition"] == "id"]
    g = _group(rows, "agent")
    fig, ax = plt.subplots(figsize=(7, 5))
    # clock frontier
    clocks = sorted([a for a in g if a.startswith("clock")],
                    key=lambda a: int(a[5:]))
    cx = [_meanstd(g[a], "mean_cost")[0] for a in clocks]
    cy = [_meanstd(g[a], "detection_rate")[0] for a in clocks]
    ax.plot(cx, cy, "o-", color="#57a", label="pure-clock frontier")
    for a, x, y in zip(clocks, cx, cy):
        ax.annotate(a, (x, y), textcoords="offset points", xytext=(6, -8),
                    fontsize=8, color="#57a")
    for a in [x for x in ID_ORDER if not x.startswith("clock")]:
        if a not in g:
            continue
        m = _meanstd(g[a], "mean_cost"); d = _meanstd(g[a], "detection_rate")
        ax.errorbar(m[0], d[0], xerr=m[1], yerr=d[1], fmt="o",
                    color=_color(a), capsize=3, label=a)
        ax.annotate(a, (m[0], d[0]), textcoords="offset points",
                    xytext=(6, 6), fontsize=8)
    ax.set_xlabel("mean observation cost / step")
    ax.set_ylabel("D-event detection rate")
    ax.set_title("P0 v2 detection-cost frontier (ID, mean±sd over seeds)")
    ax.legend(fontsize=7, loc="lower right")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def plot_causal(rows, out: Path):
    """Learned + belief-only agents under causal ablations (detection)."""
    conds = ["id", "causal:pe_shuffle", "causal:pe_mask", "causal:pe_mask_D",
             "causal:ch_permute", "causal:mask_belief", "causal:fresh_pred"]
    labels = ["ID", "PE-shuffle\n(3 perms)", "PE-mask", "PE-mask\nD only",
              "ch-permute\n(3 perms)", "D-belief\nmask", "fresh\npredictor"]
    fig, ax = plt.subplots(figsize=(10, 4))
    w = 0.35
    x = np.arange(len(conds))
    for i, (agent, color) in enumerate((("learned", "#2a7"),
                                      ("bonly", "#c60"))):
        sub = [r for r in rows if r["agent"] == agent]
        g = _group(sub, "condition_group")
        means = [_meanstd(g.get(c, []), "detection_rate")[0] for c in conds]
        stds = [_meanstd(g.get(c, []), "detection_rate")[1] for c in conds]
        ax.bar(x + i * w, means, w, yerr=stds, capsize=3, color=color,
               label=agent)
    ax.set_xticks(x + w / 2)
    ax.set_xticklabels(labels, fontsize=8)
    ax.set_ylabel("detection rate")
    ax.set_title("Causal interventions (mean±sd over seeds×perms)")
    ax.legend()
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def plot_ood(rows, out: Path):
    """OOD: efficiency per condition for learned vs references + attention."""
    rows = [r for r in rows if r["condition"].startswith("ood:")]
    conds = sorted(set(r["condition"] for r in rows))
    agents = [a for a in ["learned", "bonly", "learned_final", "trigger",
                          "vtrigger22", "cwatch", "fixed", "clock2"]
              if any(r["agent"] == a for r in rows)]
    fig, axes = plt.subplots(1, 2, figsize=(14, 4))
    ax = axes[0]
    w = 0.8 / max(len(agents), 1)
    x = np.arange(len(conds))
    for i, a in enumerate(agents):
        sub = [r for r in rows if r["agent"] == a]
        g = _group(sub, "condition")
        means = [_meanstd(g.get(c, []), "info_efficiency")[0] for c in conds]
        ax.bar(x + i * w, means, w, label=a, color=_color(a))
    ax.set_xticks(x + w * len(agents) / 2)
    ax.set_xticklabels([c.replace("ood:", "") for c in conds], rotation=20)
    ax.set_title("info efficiency under OOD")
    ax.legend(fontsize=7)

    ax = axes[1]
    # attention adaptation: OBSERVE_D fraction ID vs OOD for learned
    lr = [r for r in rows if r["agent"] == "learned"]
    g = _group(lr, "condition")
    dfrac = [_meanstd(g.get(c, []), "attD")[0] if "attD" in
             (g.get(c, [{}])[0] or {}) else
             np.mean([r["attention"]["OBSERVE_D"] for r in g.get(c, [])] or [0])
             for c in conds]
    idD = [r["attention"]["OBSERVE_D"] for r in load_rows()
           if r["agent"] == "learned" and r["condition"] == "id"]
    ax.bar(range(len(conds) + 1), [np.mean(idD or [0])] + dfrac,
           color=["#555"] + ["#27a"] * len(conds))
    ax.set_xticks(range(len(conds) + 1))
    ax.set_xticklabels(["ID"] + [c.replace("ood:", "") for c in conds],
                      rotation=20)
    ax.set_title("learned OBSERVE_D share: ID vs OOD")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def plot_training(out: Path):
    """Training curves from runs/*/metrics.jsonl."""
    fig, axes = plt.subplots(1, 3, figsize=(13, 3.6))
    for mf in sorted((BASE / "runs").glob("base_*/metrics.jsonl")):
        rows = [json.loads(l) for l in mf.read_text().splitlines() if l.strip()]
        if not rows:
            continue
        steps = [r["steps"] for r in rows]
        seed = mf.parent.name
        axes[0].plot(steps, [r.get("eval_detection", np.nan) for r in rows],
                     label=seed, alpha=0.8)
        axes[1].plot(steps, [r.get("eval_cost", np.nan) for r in rows],
                     label=seed, alpha=0.8)
        axes[2].plot(steps, [r.get("eval_reward", np.nan) for r in rows],
                     label=seed, alpha=0.8)
    axes[0].set_title("eval detection"); axes[1].set_title("eval cost")
    axes[2].set_title("eval reward/ep")
    for ax in axes:
        ax.legend(fontsize=7)
        ax.set_xlabel("steps")
    fig.suptitle("P0 training curves (periodic eval)")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def plot_surprise_signature(out: Path):
    """Per-channel predictor error (pred_err_mean) for the learned agent --
    shows the intended signature: A low, C high, D spiky."""
    rows = load_rows()
    rows = [r for r in rows if r["agent"] == "learned" and r["condition"] == "id"]
    if not rows:
        return
    chans = dyn.CHANNEL_NAMES
    e = {c: [r["pred_err_mean"][c] for r in rows] for c in chans}
    fig, ax = plt.subplots(figsize=(6, 3.6))
    ax.bar(chans, [np.mean(e[c]) for c in chans],
           yerr=[np.std(e[c]) for c in chans], capsize=4, color="#a55")
    ax.set_title("predictor |xhat - x| per channel (learned agent, ID)")
    fig.tight_layout()
    fig.savefig(out, dpi=140)
    plt.close(fig)


def main():
    rows = load_rows()
    REPORTS.mkdir(exist_ok=True)
    plot_id_baselines(rows, REPORTS / "fig_baselines.png")
    plot_frontier(rows, REPORTS / "fig_frontier.png")
    plot_causal(rows, REPORTS / "fig_causal.png")
    plot_ood(rows, REPORTS / "fig_ood.png")
    plot_training(REPORTS / "fig_training.png")
    plot_surprise_signature(REPORTS / "fig_surprise.png")
    print(f"wrote figures to {REPORTS}")


if __name__ == "__main__":
    main()
