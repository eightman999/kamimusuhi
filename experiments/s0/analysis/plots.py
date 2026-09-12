"""Plots for S0 reports. All functions save PNGs; no display needed."""

from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

GROUP_ORDER = ("self", "external", "mixed", "noise")


def plot_group_mse(results: dict[str, dict], out: Path,
                   key: str = "base_mse") -> Path:
    """Bar chart of per-cause-group MSE per model.

    results: {run_name: eval_battery_result}
    """
    names = list(results)
    x = np.arange(len(GROUP_ORDER))
    w = 0.8 / max(1, len(names))
    fig, ax = plt.subplots(figsize=(7, 4))
    for i, n in enumerate(names):
        mse = results[n][key]
        ax.bar(x + i * w, [mse[g] for g in GROUP_ORDER], w, label=n)
    ax.set_xticks(x + w * len(names) / 2, GROUP_ORDER)
    ax.set_ylabel("next-obs MSE")
    ax.set_title(f"S0 prediction error by sensor cause group ({key})")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_sensitivity(attribution: dict, out: Path) -> Path:
    """Model action-sensitivity per obs dim vs true cause labels."""
    sens = np.asarray(attribution["sensitivity"])
    labels = np.asarray(attribution["labels"])
    names = ("self", "external", "mixed", "noise")
    fig, ax = plt.subplots(figsize=(7, 4))
    for code, name in enumerate(names):
        idx = np.where(labels == code)[0]
        ax.scatter(idx, sens[idx], label=name, s=40)
    ax.set_xlabel("observation dim")
    ax.set_ylabel("action sensitivity")
    ax.set_title("S0 causal attribution: model sensitivity vs true cause")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_probe_r2(probes: dict, cause_labels, out: Path) -> Path:
    r2s = np.asarray(probes["r2_self_by_dim"])
    r2w = np.asarray(probes["r2_world_by_dim"])
    labels = np.asarray(cause_labels)
    x = np.arange(len(r2s))
    fig, ax = plt.subplots(figsize=(8, 4))
    ax.bar(x - 0.2, r2s, 0.4, label="self component R2")
    ax.bar(x + 0.2, r2w, 0.4, label="world component R2")
    for code, name in enumerate(("self", "ext", "mix", "noise")):
        idx = np.where(labels == code)[0]
        if len(idx):
            ax.axvspan(idx.min() - 0.5, idx.max() + 0.5, alpha=0.06)
    ax.set_xlabel("observation dim (canonical order)")
    ax.set_ylabel("R2")
    ax.set_title("S0 latent probes: decodability of self/world components")
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out
