"""Plots for X0 (matplotlib, Agg backend — headless)."""

from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


def _save(fig, path):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    fig.tight_layout()
    fig.savefig(path, dpi=120)
    plt.close(fig)


def plot_retrieval_bars(rows, path):
    """X0-A: macro top-1 retrieval per method (first seed shown; error bars
    = sd across seeds for 'model')."""
    method_names = ["model", "cca", "pca", "concat", "timing", "chance",
                    "oracle"]
    names = [m for m in method_names if m in rows[0]["retrieval"]]
    means, errs = [], []
    for m in names:
        vals = [r["retrieval"][m]["top1"] for r in rows]
        means.append(np.mean(vals))
        errs.append(np.std(vals))
    fig, ax = plt.subplots(figsize=(7, 4))
    ax.bar(names, means, yerr=errs, capsize=4,
           color=["tab:blue"] + ["tab:gray"] * (len(names) - 2)
           + ["tab:green"])
    ax.axhline(1 / 6, ls="--", c="r", lw=1, label="chance (1/G)")
    ax.set_ylabel("cross-modal retrieval top-1")
    ax.set_title("X0-A retrieval vs baselines")
    ax.legend()
    _save(fig, path)


def plot_false_binding(rows, path):
    """X-C2: cotimed-wrong rate under conflict scenes (first delta)."""
    d = sorted(rows[0]["conflict"].keys())[0]
    block = rows[0]["conflict"][d]
    names = [m for m in ("model", "cca", "timing", "chance") if m in block]
    vals = [np.mean([r["conflict"][d][m]["cotimed_wrong"] for r in rows])
            for m in names]
    errs = [np.std([r["conflict"][d][m]["cotimed_wrong"] for r in rows])
            for m in names]
    fig, ax = plt.subplots(figsize=(6, 4))
    ax.bar(names, vals, yerr=errs, capsize=4,
           color=["tab:blue", "tab:orange", "tab:red", "tab:gray"][:len(names)])
    ax.set_ylabel("co-timed different-cause bound rate")
    ax.set_title(f"X-C2 false binding under false synchrony (delta={d})")
    _save(fig, path)


def plot_lag_curve(rows, path):
    """X-C1: retrieval top-1 vs lag for model vs timing."""
    lags = sorted(rows[0]["desync"].keys(), key=int)
    fig, ax = plt.subplots(figsize=(6, 4))
    for name, color in (("model", "tab:blue"), ("timing", "tab:red"),
                        ("cca", "tab:orange"), ("chance", "tab:gray")):
        if name not in rows[0]["desync"][lags[0]]:
            continue
        y = [np.mean([r["desync"][L][name] for r in rows]) for L in lags]
        e = [np.std([r["desync"][L][name] for r in rows]) for L in lags]
        ax.errorbar([int(L) for L in lags], y, yerr=e, marker="o",
                    label=name, color=color, capsize=3)
    ax.set_xlabel("same-cause modality lag (steps)")
    ax.set_ylabel("retrieval top-1")
    ax.set_title("X-C1 temporal desynchronization")
    ax.legend()
    _save(fig, path)


def plot_probe_heatmap(mat, path):
    """X0-C: probe transfer matrix (rows=train modality, cols=test)."""
    if not isinstance(mat, dict) or mat.get("error"):
        return
    mods = list(mat.keys())
    A = np.asarray([[mat[a].get(b, np.nan) for b in mods] for a in mods])
    fig, ax = plt.subplots(figsize=(5, 4))
    im = ax.imshow(A, vmin=0, vmax=1, cmap="viridis")
    ax.set_xticks(range(len(mods)), mods)
    ax.set_yticks(range(len(mods)), mods)
    for i in range(len(mods)):
        for j in range(len(mods)):
            ax.text(j, i, f"{A[i, j]:.2f}", ha="center", va="center",
                    color="w" if A[i, j] < 0.6 else "k", fontsize=8)
    ax.set_title("X0-C cause-probe transfer (train row -> test col)")
    fig.colorbar(im)
    _save(fig, path)


def plot_latent_pca(latent, path):
    proj = np.asarray(latent["proj"])
    cause = np.asarray(latent["cause"])
    mods = np.asarray(latent["mod"])
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.2))
    keep = cause < 16                       # first 16 causes for readability
    sc = axes[0].scatter(proj[keep, 0], proj[keep, 1], c=cause[keep],
                         cmap="tab20", s=8, alpha=0.8)
    axes[0].set_title("latent PCA coloured by TRUE cause")
    fig.colorbar(sc, ax=axes[0])
    uniq = sorted(set(mods))
    for m in uniq:
        sel = mods == m
        axes[1].scatter(proj[sel, 0], proj[sel, 1], s=8, alpha=0.7, label=m)
    axes[1].set_title("coloured by modality")
    axes[1].legend()
    _save(fig, path)
