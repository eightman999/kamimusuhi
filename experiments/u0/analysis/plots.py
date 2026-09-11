"""U0 report figures: risk-coverage, reliability diagram, action usage vs
difficulty, OOD abstention bars.  All functions take plain dicts/records
and write PNGs (matplotlib, Agg backend)."""

from __future__ import annotations

from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from .metrics import risk_coverage


def plot_risk_coverage(subj_records: dict, path: str | Path) -> None:
    """subj_records: {subject_name: records list (answered episodes)}."""
    fig, ax = plt.subplots(figsize=(5, 4))
    for name, recs in subj_records.items():
        ans = [r for r in recs if r["outcome"] == "answer"]
        if not ans:
            continue
        conf = np.asarray([r["conf"] for r in ans])
        cor = np.asarray([r["correct"] for r in ans], dtype=float)
        rc = risk_coverage(conf, cor, n_points=21)
        ax.plot(rc["coverage"], rc["risk"], marker=".", label=name)
    ax.set_xlabel("coverage")
    ax.set_ylabel("selective risk (1 - accuracy)")
    ax.set_title("Risk–coverage")
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.grid(alpha=0.3)
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)


def plot_reliability(conf: np.ndarray, correct: np.ndarray, path,
                     n_bins: int = 10, title: str = "Reliability diagram"):
    edges = np.linspace(0, 1, n_bins + 1)
    idx = np.clip(np.digitize(conf, edges[1:-1]), 0, n_bins - 1)
    centers, accs, counts = [], [], []
    for b in range(n_bins):
        m = idx == b
        if m.any():
            centers.append(conf[m].mean())
            accs.append(correct[m].mean())
            counts.append(m.sum())
    fig, ax = plt.subplots(figsize=(4.5, 4))
    ax.plot([0, 1], [0, 1], "k--", lw=1, label="perfect")
    sizes = 20 + 80 * np.asarray(counts) / max(1, max(counts))
    ax.scatter(centers, accs, s=sizes)
    ax.plot(centers, accs, "-", alpha=0.5)
    ax.set_xlabel("confidence")
    ax.set_ylabel("accuracy")
    ax.set_title(title)
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.grid(alpha=0.3)
    ax.legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)


def plot_action_by_regime(subj_records: dict, path,
                          regimes=None, title="Action usage by regime"):
    """Stacked bars: per regime, fraction of episodes ending in
    answer / observe-then-answer / abstain(+forced)."""
    if regimes is None:
        regimes = sorted({r["regime"] for recs in subj_records.values()
                          for r in recs})
    n_sub = len(subj_records)
    fig, axes = plt.subplots(1, n_sub, figsize=(2.6 * n_sub, 4),
                           sharey=True)
    if n_sub == 1:
        axes = [axes]
    for ax, (name, recs) in zip(axes, subj_records.items()):
        fr_ans, fr_obs, fr_abs = [], [], []
        for reg in regimes:
            sub = [r for r in recs if r["regime"] == reg]
            if not sub:
                fr_ans.append(0); fr_obs.append(0); fr_abs.append(0)
                continue
            n = len(sub)
            fr_ans.append(sum(r["outcome"] == "answer" and r["n_obs"] == 1
                              for r in sub) / n)
            fr_obs.append(sum(r["outcome"] == "answer" and r["n_obs"] > 1
                              for r in sub) / n)
            fr_abs.append(sum(r["outcome"] != "answer" for r in sub) / n)
        x = np.arange(len(regimes))
        ax.bar(x, fr_ans, label="answer@1", color="#2ca02c")
        ax.bar(x, fr_obs, bottom=fr_ans, label="observe->answer",
               color="#1f77b4")
        ax.bar(x, fr_abs, bottom=[a + b for a, b in zip(fr_ans, fr_obs)],
               label="abstain", color="#d62728")
        ax.set_xticks(x)
        ax.set_xticklabels(regimes, rotation=45, ha="right", fontsize=7)
        ax.set_title(name, fontsize=9)
        ax.set_ylim(0, 1)
    axes[0].set_ylabel("fraction of episodes")
    axes[-1].legend(fontsize=7, loc="upper right")
    fig.suptitle(title)
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)


def plot_ood_bars(ood_stats: dict, path,
                  title="OOD abstention"):
    """ood_stats: {subject: {ood_mode: abstain_rate}} + clean reference."""
    modes = sorted({m for s in ood_stats.values() for m in s})
    subjects = list(ood_stats)
    x = np.arange(len(modes))
    w = 0.8 / max(1, len(subjects))
    fig, ax = plt.subplots(figsize=(6.5, 4))
    for i, s in enumerate(subjects):
        vals = [ood_stats[s].get(m, np.nan) for m in modes]
        ax.bar(x + i * w, vals, w, label=s)
    ax.set_xticks(x + w * (len(subjects) - 1) / 2)
    ax.set_xticklabels(modes)
    ax.set_ylabel("abstain rate")
    ax.set_title(title)
    ax.grid(alpha=0.3, axis="y")
    ax.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)


def plot_conf_vs_error(conf: np.ndarray, correct: np.ndarray, path,
                       n_bins: int = 10,
                       title="Confidence vs error rate"):
    edges = np.linspace(0, 1, n_bins + 1)
    idx = np.clip(np.digitize(conf, edges[1:-1]), 0, n_bins - 1)
    centers, errs, counts = [], [], []
    for b in range(n_bins):
        m = idx == b
        if m.any():
            centers.append(conf[m].mean())
            errs.append(1 - correct[m].mean())
            counts.append(m.sum())
    fig, ax = plt.subplots(figsize=(4.5, 4))
    ax.bar(centers, errs, width=0.08)
    for cx, e, c in zip(centers, errs, counts):
        ax.text(cx, e + 0.02, str(int(c)), ha="center", fontsize=7)
    ax.set_xlabel("confidence bin")
    ax.set_ylabel("error rate")
    ax.set_title(title)
    ax.set_ylim(0, 1)
    ax.grid(alpha=0.3, axis="y")
    fig.tight_layout()
    fig.savefig(path, dpi=140)
    plt.close(fig)
