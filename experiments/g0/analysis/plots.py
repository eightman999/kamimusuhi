"""Plots for G0 reports. All functions save PNGs; no display needed."""

from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402


def _ms(entry):
    if entry is None:
        return 0.0, 0.0
    return entry["mean"], entry["std"]


def plot_probe_acc(summary: dict, out: Path,
                   keys=("acc_in", "acc_loco", "acc_ood_ctx",
                         "acc_dense_ctx")) -> Path:
    names = list(summary)
    x = np.arange(len(keys))
    w = 0.8 / max(1, len(names))
    fig, ax = plt.subplots(figsize=(9, 4.5))
    for i, n in enumerate(names):
        means, stds = zip(*[_ms(summary[n].get(k)) for k in keys])
        ax.bar(x + i * w, means, w, yerr=stds, label=n, capsize=2)
    ax.axhline(1 / 6, color="k", ls="--", lw=0.8, label="chance")
    ax.set_xticks(x + w * len(names) / 2,
                  [k.replace("acc_", "").replace("_", "\n") for k in keys])
    ax.set_ylabel("linear probe accuracy")
    ax.set_title("G0 cause probes: in-context vs held-out contexts")
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_nmi_match(summary: dict, out: Path) -> Path:
    keys = ("nmi_pooled", "nmi_worst_ctx", "nmi_ood", "match_train",
            "match_ood")
    names = list(summary)
    x = np.arange(len(keys))
    w = 0.8 / max(1, len(names))
    fig, ax = plt.subplots(figsize=(9, 4.5))
    for i, n in enumerate(names):
        means, stds = zip(*[_ms(summary[n].get(k)) for k in keys])
        ax.bar(x + i * w, means, w, yerr=stds, label=n, capsize=2)
    ax.axhline(1 / 6, color="k", ls="--", lw=0.8)
    ax.set_xticks(x + w * len(names) / 2,
                  [k.replace("_", "\n") for k in keys])
    ax.set_ylabel("score")
    ax.set_title("G0 clustering NMI + cross-context cause matching")
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_causal(summary: dict, out: Path) -> Path:
    keys = ("acc_in", "shuffle_aligned_acc", "perm_acc", "dropout_acc",
            "midctx_acc", "ood_noise", "ood_gain")
    names = list(summary)
    x = np.arange(len(keys))
    w = 0.8 / max(1, len(names))
    fig, ax = plt.subplots(figsize=(10, 4.5))
    for i, n in enumerate(names):
        means, stds = zip(*[_ms(summary[n].get(k)) for k in keys])
        ax.bar(x + i * w, means, w, yerr=stds, label=n, capsize=2)
    ax.axhline(1 / 6, color="k", ls="--", lw=0.8)
    ax.set_xticks(x + w * len(names) / 2,
                  [k.replace("_", "\n") for k in keys])
    ax.set_ylabel("probe accuracy")
    ax.set_title("G0 causal/OOD robustness: aligned-shuffle, sensor-perm, "
                 "dropout, ctx-switch, noise, gain")
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_ood_combo(summary: dict, out: Path) -> Path:
    keys = ("combo_auc", "combo_set", "fewshot5_ood")
    names = list(summary)
    x = np.arange(len(keys))
    w = 0.8 / max(1, len(names))
    fig, ax = plt.subplots(figsize=(8, 4.5))
    for i, n in enumerate(names):
        means, stds = zip(*[_ms(summary[n].get(k)) for k in keys])
        ax.bar(x + i * w, means, w, yerr=stds, label=n, capsize=2)
    ax.axhline(0.5, color="k", ls="--", lw=0.8, label="chance(auc)")
    ax.set_xticks(x + w * len(names) / 2,
                  ["presence\nAUC", "pair set\nacc", "5-shot\nOOD ctx"])
    ax.set_ylabel("score")
    ax.set_title("G0 compositional OOD + few-shot context adaptation")
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_confusion(conf: list, names, out: Path,
                   title="OOD-context confusion") -> Path:
    conf = np.asarray(conf)
    fig, ax = plt.subplots(figsize=(5.5, 4.5))
    im = ax.imshow(conf, vmin=0, vmax=1, cmap="viridis")
    ax.set_xticks(range(len(names)), names, rotation=45, ha="right")
    ax.set_yticks(range(len(names)), names)
    ax.set_xlabel("predicted cause")
    ax.set_ylabel("true cause")
    ax.set_title(title)
    for i in range(len(names)):
        for j in range(len(names)):
            ax.text(j, i, f"{conf[i, j]:.2f}", ha="center",
                    va="center", fontsize=7,
                    color="w" if conf[i, j] < 0.6 else "k")
    fig.colorbar(im, fraction=0.046)
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def plot_code_usage(ev: dict, out: Path) -> Path:
    """Placeholder-free: plots VQ code stats if present in eval result."""
    d = ev.get("discrete")
    if not d:
        return out
    fig, ax = plt.subplots(figsize=(5, 3.5))
    ax.bar(["MI(cause;code)\n(norm)", "code\nstability",
            "codes\nused"],
           [d["mi_code_cause_norm"], d["code_stability"],
            d["n_codes_used"] / max(d["codebook_size"], 1)])
    ax.set_ylim(0, 1)
    ax.set_title(f"G0 discrete codes ({ev['rep']})")
    fig.tight_layout()
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def make_report_plots(sweep_summary: dict, out_dir: Path) -> list:
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = [
        plot_probe_acc(sweep_summary, out_dir / "probe_acc.png"),
        plot_nmi_match(sweep_summary, out_dir / "nmi_match.png"),
        plot_causal(sweep_summary, out_dir / "causal.png"),
        plot_ood_combo(sweep_summary, out_dir / "ood_combo.png"),
    ]
    return paths
