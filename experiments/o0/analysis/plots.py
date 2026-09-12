"""Plotting for O0 reports (matplotlib, Agg backend)."""

from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


def plot_err_vs_occstep(curves: dict, out: str | Path, title: str):
    """curves: {label: (xs, ys, yerr_or_None)} -> line plot."""
    fig, ax = plt.subplots(figsize=(7, 4.2))
    for label, (xs, ys, yerr) in curves.items():
        xs, ys = np.asarray(xs), np.asarray(ys, dtype=np.float64)
        ok = ~np.isnan(ys)
        if yerr is not None:
            yerr = np.asarray(yerr, dtype=np.float64)[ok]
            ax.fill_between(xs[ok], ys[ok] - yerr, ys[ok] + yerr, alpha=0.15)
        ax.plot(xs[ok], ys[ok], marker="o", ms=3, lw=1.5, label=label)
    ax.set_xlabel("occluded step index k (steps since disappearance)")
    ax.set_ylabel("position MAE (normalized)")
    ax.set_title(title)
    ax.grid(alpha=0.3)
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)


def plot_bars(groups: dict, out: str | Path, title: str, ylabel: str,
              lower_better: bool = True):
    """groups: {group_label: {model: (mean, std)}} -> grouped bar chart."""
    models = sorted({m for g in groups.values() for m in g})
    labels = list(groups)
    x = np.arange(len(labels))
    width = 0.8 / max(1, len(models))
    fig, ax = plt.subplots(figsize=(max(7, 1.6 * len(labels)), 4.2))
    for i, m in enumerate(models):
        means = [groups[g].get(m, (np.nan, 0.0))[0] for g in labels]
        stds = [groups[g].get(m, (np.nan, 0.0))[1] for g in labels]
        ax.bar(x + i * width - 0.4 + width / 2, means, width * 0.9,
               yerr=stds, capsize=3, label=m)
    ax.set_xticks(x)
    ax.set_xticklabels(labels, fontsize=8)
    ax.set_ylabel(ylabel + (" (lower is better)" if lower_better else " (higher is better)"))
    ax.set_title(title)
    ax.grid(alpha=0.3, axis="y")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)


def plot_metric_vs_boutlen(table: dict, out: str | Path, title: str):
    """table: {model: [(mid, mean, std), ...]}"""
    fig, ax = plt.subplots(figsize=(7, 4.2))
    for label, rows in table.items():
        if not rows:
            continue
        mid = np.array([r[0] for r in rows])
        mean = np.array([r[1] for r in rows])
        std = np.array([r[2] for r in rows])
        ax.plot(mid, mean, marker="o", ms=3, lw=1.5, label=label)
        ax.fill_between(mid, mean - std, mean + std, alpha=0.15)
    ax.set_xlabel("occlusion bout length (steps hidden)")
    ax.set_ylabel("position MAE (normalized)")
    ax.set_title(title)
    ax.grid(alpha=0.3)
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
