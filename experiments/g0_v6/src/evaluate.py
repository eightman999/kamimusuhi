"""Fixed distance-based evaluation battery for G0-v6.

This module is evaluator-only. Oracle arrays are used to define the declared
pair labels and intervention factors, never to fit a representation, select a
checkpoint, or enter the CPC objective.
"""

from pathlib import Path
import time

import numpy as np

from .dataset import load_evaluation
from .health import latent_statistics


EPS = 1e-12


def _cosine(a, b):
    a, b = np.asarray(a, dtype=np.float64), np.asarray(b, dtype=np.float64)
    denom = np.linalg.norm(a, axis=-1) * np.linalg.norm(b, axis=-1)
    return np.sum(a * b, axis=-1) / np.maximum(denom, EPS)


def auc_score(labels, scores):
    """Mann-Whitney AUC with exact half-credit for ties."""
    y, s = np.asarray(labels), np.asarray(scores)
    if not np.isfinite(s).all():
        raise ValueError("nonfinite scores")
    positive, negative = s[y == 1], s[y == 0]
    if not len(positive) or not len(negative):
        raise ValueError("AUC requires both classes")
    ordered = np.sort(negative)
    lower = np.searchsorted(ordered, positive, side="left")
    upper = np.searchsorted(ordered, positive, side="right")
    return float(np.mean((lower + upper) / (2 * len(negative))))


def _shortcut_diagnostics(pooled, endpoint_cause, endpoint_context):
    x = np.asarray(pooled, dtype=np.float64)

    def explained(labels):
        mean = x.mean(axis=0)
        total = np.sum((x - mean) ** 2)
        between = sum(
            np.count_nonzero(labels == key)
            * np.sum((x[labels == key].mean(axis=0) - mean) ** 2)
            for key in np.unique(labels)
        )
        return float(between / max(float(total), EPS))

    return {
        "context_variance_fraction": explained(endpoint_context),
        "cause_variance_fraction": explained(endpoint_cause),
    }


def evaluate_representation(encode, data_dir: Path, output_dir: Path | None = None):
    """Evaluate one deterministic observation-only representation."""
    started = time.monotonic()
    data_dir = Path(data_dir)
    predictions, stats_arrays = {}, {}
    result = {
        key: {}
        for key in (
            "iid",
            "midctx",
            "match_ood",
            "dynseg_ood",
            "combo_ood",
            "intervention",
            "collapse",
            "runtime",
        )
    }
    result["failure_class"] = "NONE"

    def encoded(data):
        observations = data["obs"]
        latent = np.asarray(encode(observations.copy()))
        if latent.ndim != 3 or latent.shape[:2] != observations.shape[:2] or latent.shape[-1] < 1:
            raise ValueError("encoder must return [N,T,Z]")
        if not np.isfinite(latent).all():
            raise FloatingPointError("nonfinite latent")
        return latent.astype(np.float64)

    shortcuts = []
    split_map = (
        ("iid_test", "iid"),
        ("match_ood", "match_ood"),
        ("dynseg_ood", "dynseg_ood"),
        ("combo_oodctx", "combo_ood"),
    )
    for filename, key in split_map:
        data = load_evaluation(data_dir / "evaluation" / f"{filename}.npz")
        latent = encoded(data)
        pooled = latent[:, -4:].mean(axis=1)
        indices, labels = data["pair_index"], data["pair_label"]
        distances = np.linalg.norm(pooled[indices[:, 0]] - pooled[indices[:, 1]], axis=1)
        auc = auc_score(labels, -distances)
        endpoint_cause = data["cause"][:, -1]
        endpoint_context = data["context"][:, -1]
        diagnostic = _shortcut_diagnostics(pooled, endpoint_cause, endpoint_context)
        shortcut = bool(diagnostic["context_variance_fraction"] >= 0.95 and auc <= 0.55)
        shortcuts.append(shortcut)
        result[key] = {
            "auc": auc,
            "n_pairs": int(len(labels)),
            "same_distance": float(distances[labels == 1].mean()),
            "different_distance": float(distances[labels == 0].mean()),
            "shortcut_diagnostics": diagnostic,
        }
        for name, values in (
            ("score", -distances),
            ("label", labels),
            ("pooled", pooled),
            ("cause", endpoint_cause),
            ("context", endpoint_context),
        ):
            predictions[f"{key}_{name}"] = values
        if key == "iid":
            collapse = latent_statistics(latent)
            result["collapse"] = collapse
            stats_arrays.update(
                latent_mean=np.asarray(collapse["latent_mean"]),
                per_dimension_std=np.asarray(collapse["per_dimension_std"]),
                covariance_spectrum=np.asarray(collapse["covariance_spectrum"]),
                effective_rank=np.asarray(collapse["effective_rank"]),
                cosine_quantiles=np.asarray(collapse["pairwise_cosine_distribution"]["quantiles"]),
                latent_std=np.asarray(collapse["latent_std"]),
            )

    midctx = load_evaluation(data_dir / "evaluation" / "midctx.npz")
    mid_latent = encoded(midctx)
    switch = int(midctx["switch_time"])
    immediate = _cosine(mid_latent[:, switch - 1], mid_latent[:, switch])
    pre = mid_latent[:, max(0, switch - 4):switch].mean(axis=1)
    offsets = list(range(0, min(9, mid_latent.shape[1] - switch)))
    recovery_curve = [_cosine(pre, mid_latent[:, switch + offset]).mean() for offset in offsets]
    recovery = _cosine(
        mid_latent[:, switch - 4:switch].mean(axis=1),
        mid_latent[:, switch:switch + 4].mean(axis=1),
    )
    result["midctx"] = {
        "stability": float(immediate.mean()),
        "recovery_stability": float(recovery.mean()),
        "recovery_curve": {str(offset): float(value) for offset, value in zip(offsets, recovery_curve)},
        "n": int(len(mid_latent)),
    }
    predictions["midctx_stability"] = immediate
    predictions["midctx_recovery_stability"] = recovery
    predictions["midctx_recovery_curve"] = np.asarray(recovery_curve)

    intervention = load_evaluation(data_dir / "evaluation" / "intervention.npz")
    intervention_latent = encoded(intervention).mean(axis=1)
    groups = intervention["intervention_index"]
    distances = np.linalg.norm(intervention_latent[groups[:, 1:]] - intervention_latent[groups[:, 0, None]], axis=-1)
    cause_distance, context_distance, nuisance_distance = distances.mean(axis=0)
    result["intervention"] = {
        "selectivity": float(cause_distance / (context_distance + nuisance_distance + EPS)),
        "cause_distance": float(cause_distance),
        "context_distance": float(context_distance),
        "nuisance_distance": float(nuisance_distance),
        "n": int(len(groups)),
    }
    predictions["intervention_distances"] = distances

    result["collapse"]["shortcut_context"] = bool(shortcuts[0] and shortcuts[1])
    if result["collapse"]["full_collapse"]:
        result["failure_class"] = "COLLAPSE_FULL"
    elif result["collapse"]["shortcut_context"]:
        result["failure_class"] = "SHORTCUT_CONTEXT"
    result["runtime"] = {"evaluation_seconds": time.monotonic() - started}

    if output_dir is not None:
        output_dir = Path(output_dir)
        output_dir.mkdir(parents=True, exist_ok=True)
        for filename in ("eval_predictions.npz", "latent_stats.npz"):
            if (output_dir / filename).exists():
                raise FileExistsError(f"refusing to overwrite {filename}")
        np.savez_compressed(output_dir / "eval_predictions.npz", **predictions)
        np.savez_compressed(output_dir / "latent_stats.npz", **stats_arrays)
    return result
