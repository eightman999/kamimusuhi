"""Plot generation for H0 (spec section 22, Phase 7).

All functions take plain dicts/arrays and write PNGs. matplotlib is used
with the Agg backend so no display is required.
"""

from __future__ import annotations

from pathlib import Path
from typing import Dict, List, Optional

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from ..env import dynamics as dyn


def plot_internal_states(trace: Dict, out: Path, title: str = ""):
    internals = np.array(trace["internal"])
    actions = np.array(trace["action"])
    fig, axes = plt.subplots(
        dyn.N_INTERNAL + 1, 1, figsize=(10, 2 * (dyn.N_INTERNAL + 1)), sharex=True
    )
    for i, name in enumerate(dyn.INTERNAL_NAMES):
        ax = axes[i]
        ax.plot(internals[:, i], lw=0.8)
        lo, hi = dyn.PREFERRED_RANGES[name]
        ax.axhspan(lo, hi, color="green", alpha=0.15)
        ax.set_ylabel(name)
        ax.set_ylim(0, 1)
    ax = axes[-1]
    ax.step(range(len(actions)), actions, where="post", lw=0.8)
    ax.set_yticks(range(dyn.N_ACTIONS))
    ax.set_yticklabels(dyn.ACTION_NAMES, fontsize=7)
    ax.set_ylabel("action")
    ax.set_xlabel("t")
    if title:
        axes[0].set_title(title)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_error_vs_time(traces: List[Dict], out: Path, title: str = ""):
    """Mean homeostatic error over time across episodes."""
    fig, ax = plt.subplots(figsize=(8, 4))
    for tr in traces:
        internals = np.array(tr["internal"])
        errs = [dyn.homeostatic_error(s) for s in internals]
        ax.plot(errs, lw=0.5, alpha=0.4)
    ax.set_xlabel("t")
    ax.set_ylabel("homeostatic error")
    if title:
        ax.set_title(title)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_survival_curves(results: Dict[str, List[int]], out: Path):
    """results: {label: [survival_time per episode]}."""
    fig, ax = plt.subplots(figsize=(7, 4))
    for label, times in results.items():
        times = np.sort(np.asarray(times))
        surv = 1.0 - np.arange(1, len(times) + 1) / len(times)
        ax.step(times, surv, where="post", label=label)
    ax.set_xlabel("survival time (steps)")
    ax.set_ylabel("fraction of episodes alive")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_ood_comparison(table: Dict[str, Dict[str, float]], out: Path):
    """table: {condition: {agent: error_full}}."""
    conditions = list(table)
    agents = list(next(iter(table.values())))
    x = np.arange(len(conditions))
    w = 0.8 / len(agents)
    fig, ax = plt.subplots(figsize=(9, 4))
    for i, ag in enumerate(agents):
        vals = [table[c][ag] for c in conditions]
        ax.bar(x + i * w, vals, w, label=ag)
    ax.set_xticks(x + w * (len(agents) - 1) / 2)
    ax.set_xticklabels(conditions, fontsize=8)
    ax.set_ylabel("homeostatic error (full)")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_ablation(summary: Dict, out: Path):
    """summary: output of causality.summarize_causal."""
    names = [k for k in summary if k != "baseline"]
    deltas = [summary[k]["delta_error"] for k in names]
    fig, ax = plt.subplots(figsize=(9, 4))
    colors = ["crimson" if d > 0 else "steelblue" for d in deltas]
    ax.bar(range(len(names)), deltas, color=colors)
    ax.set_xticks(range(len(names)))
    ax.set_xticklabels(names, rotation=45, ha="right", fontsize=8)
    ax.axhline(0, color="k", lw=0.5)
    ax.set_ylabel("Δ homeostatic error vs baseline")
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_transition_matrix(trace: Dict, out: Path):
    from .rhythm import action_transition_matrix

    M = action_transition_matrix(np.array(trace["action"]))
    fig, ax = plt.subplots(figsize=(5, 4))
    im = ax.imshow(M, cmap="viridis", vmin=0, vmax=1)
    ax.set_xticks(range(dyn.N_ACTIONS))
    ax.set_yticks(range(dyn.N_ACTIONS))
    ax.set_xticklabels(dyn.ACTION_NAMES, rotation=45, ha="right", fontsize=8)
    ax.set_yticklabels(dyn.ACTION_NAMES, fontsize=8)
    ax.set_xlabel("a(t+1)")
    ax.set_ylabel("a(t)")
    for i in range(dyn.N_ACTIONS):
        for j in range(dyn.N_ACTIONS):
            ax.text(j, i, f"{M[i,j]:.2f}", ha="center", va="center",
                    fontsize=7, color="w" if M[i, j] < 0.5 else "k")
    fig.colorbar(im)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)


def plot_training_curve(eval_hist: List[Dict], out: Path, title: str = ""):
    steps = [e["steps"] for e in eval_hist]
    err = [e["homeostatic_error_full"] for e in eval_hist]
    surv = [e["survival_fraction"] for e in eval_hist]
    fig, ax = plt.subplots(figsize=(8, 4))
    ax.plot(steps, err, label="error (full)", color="crimson")
    ax.set_xlabel("training steps")
    ax.set_ylabel("homeostatic error", color="crimson")
    ax2 = ax.twinx()
    ax2.plot(steps, surv, label="survival", color="steelblue")
    ax2.set_ylabel("survival fraction", color="steelblue")
    ax2.set_ylim(0, 1.05)
    if title:
        ax.set_title(title)
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    plt.close(fig)
