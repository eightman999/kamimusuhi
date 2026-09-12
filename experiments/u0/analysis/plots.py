"""Figure generation for U0 (matplotlib, Agg backend).

All functions take plain data structures produced by sweep/report and
write PNGs under the given directory. Seven figures are required by the
protocol: learning_curve, homeostatic_error, store_precision,
memory_retention, need_intervention, causal_ablation, ood_delay.
"""

from __future__ import annotations

from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


def _save(fig, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)


def plot_learning_curve(run_metrics: dict[str, list[dict]], path: Path,
                        metric: str = "val_error_full") -> None:
    """run_metrics: run_id -> list of metrics.jsonl records."""
    fig, ax = plt.subplots(figsize=(7, 4))
    for run_id, recs in sorted(run_metrics.items()):
        xs = [r["iter"] for r in recs if metric in r]
        ys = [r[metric] for r in recs if metric in r]
        if xs:
            ax.plot(xs, ys, label=run_id, alpha=0.8)
    ax.set_xlabel("PPO iteration")
    ax.set_ylabel(metric)
    ax.set_title("U0 learning curve (validation)")
    ax.legend(fontsize=7)
    _save(fig, path)


def _bar_panel(rows: list[dict], metric: str, title: str, path: Path,
               ylabel: str | None = None) -> None:
    """rows: {subject, value, sd} sorted by value."""
    rows = [r for r in rows if r.get("value") is not None
            and not np.isnan(r.get("value", float("nan")))]
    rows.sort(key=lambda r: r["value"])
    fig, ax = plt.subplots(figsize=(max(6, len(rows) * 0.7), 4))
    names = [r["subject"] for r in rows]
    vals = [r["value"] for r in rows]
    sds = [r.get("sd", 0.0) for r in rows]
    ax.bar(range(len(rows)), vals, yerr=sds, capsize=3,
           color=["#d95f02" if n.startswith(("mlp", "gru")) else "#7570b3"
                  for n in names])
    ax.set_xticks(range(len(rows)))
    ax.set_xticklabels(names, rotation=60, ha="right", fontsize=7)
    ax.set_ylabel(ylabel or metric)
    ax.set_title(title)
    _save(fig, path)


def plot_homeostatic_error(rows: list[dict], path: Path) -> None:
    _bar_panel(rows, "error_full", "U0 homeostatic error (full episode)",
               path, ylabel="error_full")


def plot_store_precision(rows: list[dict], path: Path) -> None:
    _bar_panel(rows, "store_precision", "U0 store precision", path,
               ylabel="store_precision")


def plot_memory_retention(rows: list[dict], path: Path) -> None:
    _bar_panel(rows, "important_retention", "U0 important retention",
               path, ylabel="important_retention")


def plot_need_intervention(probe: dict, path: Path) -> None:
    """probe: need_intervention_probe() output. Shows P(STORE) per
    function under its adverse vs safe counterfactual internal state."""
    pairs = {
        "resource": ("energy_low", "energy_safe"),
        "shelter": ("temperature_low", "temperature_safe"),
        "safe_zone": ("risk_high", "risk_low"),
        "obs_point": ("certainty_low", "certainty_safe"),
    }
    conds = probe["conditions"]
    fig, ax = plt.subplots(figsize=(8, 4))
    x = np.arange(len(pairs))
    w = 0.35
    for i, (fname, (adv, safe)) in enumerate(pairs.items()):
        pa = conds[adv]["store_prob_by_func"].get(fname, float("nan"))
        ps = conds[safe]["store_prob_by_func"].get(fname, float("nan"))
        ax.bar(i - w / 2, pa, w, color="#d95f02", label="adverse" if i == 0 else None)
        ax.bar(i + w / 2, ps, w, color="#7570b3", label="safe" if i == 0 else None)
        ax.text(i, max(pa, ps) + 0.03, f"Δ={abs(pa - ps):.2f}",
                ha="center", fontsize=8)
    ax.set_xticks(x)
    ax.set_xticklabels([f"{f}\n({a} vs {s})" for f, (a, s) in pairs.items()],
                       fontsize=7)
    ax.set_ylabel("P(STORE | functional event)")
    ax.set_ylim(0, 1.15)
    ax.set_title(f"U0 need intervention (mean |ΔP| = "
                 f"{probe.get('mean_abs_delta', float('nan')):.3f})")
    ax.legend()
    _save(fig, path)


def plot_causal_ablation(rows: list[dict], path: Path) -> None:
    """rows: {subject, clean, erase, shuffle} error_full values."""
    subs = [r["subject"] for r in rows]
    fig, ax = plt.subplots(figsize=(max(6, len(subs) * 0.9), 4))
    x = np.arange(len(subs))
    w = 0.28
    for j, cond in enumerate(("clean", "erase", "shuffle")):
        vals = [r.get(cond, float("nan")) for r in rows]
        ax.bar(x + (j - 1) * w, vals, w, label=cond)
    ax.set_xticks(x)
    ax.set_xticklabels(subs, rotation=45, ha="right", fontsize=7)
    ax.set_ylabel("error_full")
    ax.set_title("U0 causal ablation (error change under memory attack)")
    ax.legend()
    _save(fig, path)


def plot_ood_delay(rows: list[dict], path: Path) -> None:
    """rows: {subject, clean, delay96, delay128, delay160} error_full."""
    conds = ["clean", "delay96", "delay128", "delay160"]
    fig, ax = plt.subplots(figsize=(7, 4))
    for r in rows:
        ys = [r.get(c, float("nan")) for c in conds]
        ax.plot(conds, ys, marker="o", label=r["subject"], alpha=0.8)
    ax.set_ylabel("error_full")
    ax.set_title("U0 OOD delay robustness")
    ax.legend(fontsize=7)
    _save(fig, path)
