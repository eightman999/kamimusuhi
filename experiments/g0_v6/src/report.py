"""Generate the G0-v6 five-seed report and required point-preserving plots."""

from pathlib import Path
import json

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from .dataset import load_evaluation
from .protocol import PROTOCOL, verify_lock
from .run import DATA, FULL, LOCK, REFERENCE, ROOT


def read_json(path):
    return json.loads(Path(path).read_text())


def _plot_paired(values_trained, values_twin, path, title, ylabel):
    x = np.arange(len(values_trained))
    fig, ax = plt.subplots(figsize=(7, 4.5))
    ax.plot(x, values_twin, "o--", label="exact untrained twin")
    ax.plot(x, values_trained, "o-", label="CPC trained")
    for i, (trained, twin) in enumerate(zip(values_trained, values_twin)):
        ax.plot([i, i], [twin, trained], color="0.65", linewidth=0.8)
    ax.set_xticks(x, [f"seed{s}" for s in PROTOCOL["seeds"]])
    ax.set_title(title)
    ax.set_ylabel(ylabel)
    ax.grid(axis="y", alpha=0.25)
    ax.legend()
    fig.tight_layout()
    fig.savefig(path, dpi=160)
    plt.close(fig)


def _plot_trajectory(summaries, field, path, title, ylabel):
    fig, ax = plt.subplots(figsize=(7, 4.5))
    for summary in summaries:
        seed = summary["seed"]
        stats_path = FULL / f"seed{seed}" / "trained" / "latent_stats.npz"
        if not stats_path.exists():
            continue
        with np.load(stats_path) as stats:
            if field not in stats.files:
                continue
            ax.plot(stats["epoch"], stats[field], marker=".", label=f"seed{seed}")
    ax.set_title(title)
    ax.set_xlabel("epoch")
    ax.set_ylabel(ylabel)
    ax.grid(alpha=0.25)
    ax.legend(ncol=2)
    fig.tight_layout()
    fig.savefig(path, dpi=160)
    plt.close(fig)


def _plot_intervention(summaries, path):
    factors = ("cause_distance", "context_distance", "nuisance_distance")
    x = np.arange(len(summaries))
    width = 0.18
    fig, ax = plt.subplots(figsize=(8, 4.5))
    for offset, factor in enumerate(factors):
        trained = [s["trained"]["intervention"][factor] for s in summaries]
        twin = [s["twin"]["intervention"][factor] for s in summaries]
        ax.scatter(x + (offset - 1) * width, trained, label=f"trained {factor.removesuffix('_distance')}")
        ax.scatter(x + (offset - 1) * width, twin, marker="x", label=f"twin {factor.removesuffix('_distance')}")
    ax.set_xticks(x, [f"seed{s['seed']}" for s in summaries])
    ax.set_ylabel("latent distance")
    ax.set_title("Intervention distances (raw values; points preserved)")
    ax.grid(axis="y", alpha=0.25)
    ax.legend(ncol=2, fontsize=8)
    fig.tight_layout()
    fig.savefig(path, dpi=160)
    plt.close(fig)


def _plot_geometry(seed, path):
    geometry_path = FULL / f"seed{seed}" / "trained" / "geometry.npz"
    if not geometry_path.exists():
        return
    with np.load(geometry_path) as values:
        latent = values["latent"].astype(np.float64)
        if latent.ndim == 3:
            latent = latent.reshape(-1, latent.shape[-1])
        centered = latent - latent.mean(axis=0)
        _, _, vh = np.linalg.svd(centered, full_matrices=False)
        xy = centered @ vh[:2].T
        factors = {
            "cause": values["cause"],
            "context": values["context"],
            "time": values["time"],
            "segment": values["segment"],
        }
    for factor, colors in factors.items():
        fig, ax = plt.subplots(figsize=(5, 4.2))
        scatter = ax.scatter(xy[:, 0], xy[:, 1], c=colors, s=5, cmap="tab10" if factor in {"cause", "context"} else "viridis", alpha=0.65)
        fig.colorbar(scatter, ax=ax, label=factor)
        ax.set_title(f"seed{seed} trained latent PCA / {factor}")
        ax.set_xlabel("PC1")
        ax.set_ylabel("PC2")
        fig.tight_layout()
        fig.savefig(path.parent / f"pca_seed{seed}_{factor}.png", dpi=160)
        plt.close(fig)


def main():
    aggregate = read_json(ROOT / "results" / "aggregate.json")
    lock = verify_lock(ROOT, LOCK, DATA / "dataset_manifest.json", REFERENCE)
    summaries = [read_json(FULL / f"seed{seed}" / "seed_summary.json") for seed in PROTOCOL["seeds"]]
    plots = ROOT / "plots"
    plots.mkdir(exist_ok=True)

    for metric, filename, title, ylabel in (
        ("match_ood", "trained_vs_twin_match_ood.png", "Match OOD AUC", "AUC"),
        ("combo_ood", "trained_vs_twin_combo_ood.png", "Composition OOD AUC", "AUC"),
        ("dynseg_ood", "trained_vs_twin_dynseg.png", "Dynamic-segment OOD AUC", "AUC"),
    ):
        trained = [s["trained"][metric]["auc"] for s in summaries]
        twin = [s["twin"][metric]["auc"] for s in summaries]
        _plot_paired(trained, twin, plots / filename, title, ylabel)
    trained_retrieval = [
        s["temporal_retrieval"]["trained"]["iid"]["train_like"]["recall_at_1"] for s in summaries
    ]
    twin_retrieval = [
        s["temporal_retrieval"]["twin"]["iid"]["train_like"]["recall_at_1"] for s in summaries
    ]
    _plot_paired(
        trained_retrieval,
        twin_retrieval,
        plots / "trained_vs_twin_temporal_retrieval.png",
        "IID train-like temporal retrieval Recall@1",
        "Recall@1",
    )
    _plot_paired(
        [s["future_observation_readout"]["trained"]["mse"] for s in summaries],
        [s["future_observation_readout"]["twin"]["mse"] for s in summaries],
        plots / "future_observation_readout_mse.png",
        "Future-observation ridge readout MSE",
        "MSE (lower is better)",
    )
    _plot_trajectory(summaries, "effective_rank", plots / "effective_rank_trajectory.png", "Effective rank trajectory", "effective rank")
    _plot_trajectory(summaries, "info_nce_loss", plots / "infonce_loss_trajectory.png", "InfoNCE validation loss trajectory", "InfoNCE loss")
    _plot_intervention(summaries, plots / "intervention_distances.png")
    for seed in PROTOCOL["seeds"]:
        _plot_geometry(seed, plots / f"pca_seed{seed}_cause.png")

    rows = []
    rows.extend(
        [
            "# G0-v6 Result",
            "",
            "## Verdict",
            "",
            f"**{aggregate['verdict']}**",
            "",
            "G0-v5のFAIL判定は変更していない。G0-v6はそのcheckpoint/resultを再利用せず、固定データ契約と独立初期化で実施した。",
            "",
            "## Research questions",
            "",
            "G0-v5 seed0で観測されたcontext-invariant grounding signalの5-seed再現性を、match/composition AUC、mid-context、intervention、label-free temporal retrievalで検証した。",
            "raw future observationの線形可読性は、temporal retrievalとは独立した診断として同じpaired twin比較に保存した。",
            "",
            "## Fixed protocol",
            "",
            f"seeds={PROTOCOL['seeds']}; epochs={PROTOCOL['epochs']}; horizons={PROTOCOL['horizons']}; unseen_horizons={PROTOCOL['unseen_horizons']}; primary={PROTOCOL['retrieval_primary']}",
            f"lock_hash=`{lock['lock_hash']}`",
            "training input: observation sequence only; checkpoint selection: validation InfoNCE loss only; effective rank: diagnostic only",
            "",
            "## Equivalence and validity",
            "",
            "G0-v5 frozen datasetはmanifest/file hashでbyte-identicalを確認した。G0-v5 checkpoint/result/FAIL判定は不使用。oracle配列はevaluator/plotに限定し、training-side AST leakage auditを通過した。",
            "",
            "## Seed-wise deltas",
            "",
            "deltaはすべて `trained - exact untrained twin`。future MSEのみ負が改善方向。",
            "",
            "|seed|temporal retrieval Recall@1 delta|match delta|combo delta|dynseg delta|midctx delta|IS delta|future MSE delta|",
            "|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for summary in summaries:
        rows.append(
            "|{seed}|{retrieval:.6f}|{match:.6f}|{combo:.6f}|{dyn:.6f}|{mid:.6f}|{is_delta:.6f}|{future:.6f}|".format(
                seed=summary["seed"],
                retrieval=summary["temporal_retrieval"]["trained"]["iid"]["train_like"]["recall_at_1"] - summary["temporal_retrieval"]["twin"]["iid"]["train_like"]["recall_at_1"],
                match=summary["trained"]["match_ood"]["auc"] - summary["twin"]["match_ood"]["auc"],
                combo=summary["trained"]["combo_ood"]["auc"] - summary["twin"]["combo_ood"]["auc"],
                dyn=summary["trained"]["dynseg_ood"]["auc"] - summary["twin"]["dynseg_ood"]["auc"],
                mid=summary["trained"]["midctx"]["stability"] - summary["twin"]["midctx"]["stability"],
                is_delta=summary["trained"]["intervention"]["selectivity"] - summary["twin"]["intervention"]["selectivity"],
                future=summary["future_observation_readout"]["delta_mse"],
            )
        )
    rows.extend(
        [
            "",
            "## Aggregate direction counts",
            "",
            "|metric|mean delta|median|std|improved count|bootstrap 95% CI|",
            "|---|---:|---:|---:|---:|---|",
        ]
    )
    labels = {
        "match_ood": "match_ood AUC",
        "combo_ood": "combo_ood AUC",
        "dynseg_ood": "dynseg_ood AUC",
        "midctx": "midctx stability",
        "intervention_selectivity": "intervention selectivity",
        "temporal_retrieval_iid_train_like_recall_at_1": "temporal retrieval Recall@1",
        "future_observation_mse": "future-observation MSE",
    }
    for key, label in labels.items():
        metric = aggregate["metrics"][key]
        rows.append(
            f"|{label}|{metric['mean']:.6f}|{metric['median']:.6f}|{metric['std']:.6f}|{metric['improved_count']}/5|[{metric['bootstrap_95ci'][0]:.6f}, {metric['bootstrap_95ci'][1]:.6f}]|"
        )
    rows.extend(
        [
            "",
            "## Raw trained/twin values",
            "",
            "|metric|trained seed0|trained seed1|trained seed2|trained seed3|trained seed4|twin seed0|twin seed1|twin seed2|twin seed3|twin seed4|",
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for key, label in (
        ("iid", "IID AUC"),
        ("midctx", "midctx stability"),
        ("match_ood", "match_ood AUC"),
        ("dynseg_ood", "dynseg_ood AUC"),
        ("combo_ood", "combo_ood AUC"),
        ("is", "intervention selectivity"),
    ):
        trained = [s["trained"][key]["stability"] if key == "midctx" else s["trained"]["intervention"]["selectivity"] if key == "is" else s["trained"][key]["auc"] for s in summaries]
        twin = [s["twin"][key]["stability"] if key == "midctx" else s["twin"]["intervention"]["selectivity"] if key == "is" else s["twin"][key]["auc"] for s in summaries]
        rows.append("|" + label + "|" + "|".join(f"{v:.6f}" for v in trained + twin) + "|")
    rows.extend(
        [
            "",
            "## Temporal generalization",
            "",
            "train-like retrieval uses predictor horizons 1,4; unseen-horizon retrieval uses direct current-latent queries at horizons 2,8; altered-segment timing uses dynseg_ood trajectories. All are episode/time correspondence only and label-free.",
            "",
            "|seed|IID train-like trained/twin|IID unseen trained/twin|context-OOD train-like trained/twin|altered-timing train-like trained/twin|",
            "|---:|---|---|---|---|",
        ]
    )
    for s in summaries:
        tr, tw = s["temporal_retrieval"]["trained"], s["temporal_retrieval"]["twin"]
        rows.append(
            f"|{s['seed']}|{tr['iid']['train_like']['recall_at_1']:.6f}/{tw['iid']['train_like']['recall_at_1']:.6f}|{tr['iid']['unseen_horizon']['recall_at_1']:.6f}/{tw['iid']['unseen_horizon']['recall_at_1']:.6f}|{tr['context_ood']['train_like']['recall_at_1']:.6f}/{tw['context_ood']['train_like']['recall_at_1']:.6f}|{tr['altered_segment_timing']['train_like']['recall_at_1']:.6f}/{tw['altered_segment_timing']['train_like']['recall_at_1']:.6f}|"
        )
    rows.extend(
        [
            "",
            "## Hardware and run provenance",
            "",
            "|seed|device|GPU|CUDA|wall clock seconds|epochs|",
            "|---:|---|---|---|---:|---:|",
        ]
    )
    for s in summaries:
        hardware = s["training_manifest"]
        rows.append(f"|{s['seed']}|{hardware.get('device')}|{hardware.get('gpu_name')}|{hardware.get('cuda')}|{hardware.get('wall_clock', float('nan')):.3f}|{hardware.get('epochs_completed')}|")
    rows.extend(
        [
            "",
            "## Required plots",
            "",
            "- `trained_vs_twin_match_ood.png`, `trained_vs_twin_combo_ood.png`, `trained_vs_twin_dynseg.png`",
            "- `trained_vs_twin_temporal_retrieval.png`, `future_observation_readout_mse.png`",
            "- `effective_rank_trajectory.png`, `infonce_loss_trajectory.png`, `intervention_distances.png`",
            "- `pca_seed{0..4}_{cause,context,time,segment}.png` (descriptive only)",
            "",
            "## Decision",
            "",
            f"direction counts: `{json.dumps(aggregate['direction_counts'], ensure_ascii=False)}`",
            "The verdict is mechanical from the locked criteria; seed-wise values and raw controls above are the evidence. Visualization alone is not a success criterion.",
        ]
    )
    (ROOT / "G0_V6_REPORT.md").write_text("\n".join(rows) + "\n")
    print(json.dumps({"stage": "report", "verdict": aggregate["verdict"], "report": str(ROOT / "G0_V6_REPORT.md")}, indent=2))


if __name__ == "__main__":
    main()
