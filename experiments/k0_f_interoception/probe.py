"""Train-only standardized ridge probes and a validation-only policy gate."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

from .policy import PRIMARY_MODES, encode_inputs, json_write, load_dataset, sha256

TARGETS = ("future_rtx3060_util", "future_p100_util", "next_job_latency_seconds")
RIDGE_LAMBDA = 10.0
GATE_RELATIVE_IMPROVEMENT = .10


def features(rows, mode, seed=0):
    """Task, final masked body/mask and history mean; no timestamps or IDs."""
    encoded = encode_inputs(rows, mode, seed)
    return np.stack([np.concatenate((x[-1].numpy(), x[:, 4:].mean(0).numpy())) for x in encoded]).astype(np.float64)


def fit_ridge(x, y, ridge_lambda=RIDGE_LAMBDA):
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    center, scale = x.mean(0), x.std(0)
    scale[scale < 1e-8] = 1
    y_center, y_scale = float(y.mean()), float(y.std())
    if y_scale < 1e-8:
        y_scale = 1.0
    standardized = (x - center) / scale
    design = np.column_stack((np.ones(len(x)), standardized))
    penalty = np.eye(design.shape[1]) * ridge_lambda
    penalty[0, 0] = 0
    coefficients = np.linalg.solve(design.T @ design + penalty, design.T @ ((y - y_center) / y_scale))
    return {"x_center": center, "x_scale": scale, "y_center": y_center,
            "y_scale": y_scale, "coefficients": coefficients, "ridge_lambda": ridge_lambda}


def predict(model, x):
    design = np.column_stack((np.ones(len(x)), (x - model["x_center"]) / model["x_scale"]))
    return (design @ model["coefficients"]) * model["y_scale"] + model["y_center"]


def score(y, estimate, y_train_scale):
    y, estimate = np.asarray(y), np.asarray(estimate)
    mae = float(np.mean(np.abs(y - estimate)))
    variance = float(np.sum((y - y.mean()) ** 2))
    return {"n": len(y), "mae": mae, "normalized_mae": mae / y_train_scale,
            "rmse": float(np.sqrt(np.mean((y - estimate) ** 2))),
            "r2": 1 - float(np.sum((y - estimate) ** 2)) / variance if variance > 1e-12 else None}


def run_probe(rows, *, include_test=False, seed=0):
    selected_splits = ("train", "validation", "test") if include_test else ("train", "validation")
    splits = {s: [r for r in rows if r["split"] == s] for s in selected_splits}
    result, fitted, predictions = [], {}, []
    targets_available = []
    for target in TARGETS:
        subsets = {split: [r for r in entries if r.get("probe_targets", {}).get(target) is not None
            and np.isfinite(r["probe_targets"][target])] for split, entries in splits.items()}
        if len(subsets["train"]) < 4 or len(subsets["validation"]) < 2:
            continue
        train_y = np.array([r["probe_targets"][target] for r in subsets["train"]], dtype=float)
        if np.std(train_y) < 1e-8:
            continue
        targets_available.append(target)
        for mode in PRIMARY_MODES:
            train_x = features(subsets["train"], mode, seed)
            model = fit_ridge(train_x, train_y)
            fitted[f"{mode}/{target}"] = {k: v.tolist() if isinstance(v, np.ndarray) else v for k, v in model.items()}
            for split in ("validation", "test") if include_test else ("validation",):
                if not subsets[split]:
                    continue
                x = features(subsets[split], mode, seed)
                y = [r["probe_targets"][target] for r in subsets[split]]
                estimated = predict(model, x)
                result.append({"mode": mode, "target": target, "split": split,
                    "target_kind": "future_10_second_observation" if target.startswith("future_") else "measured_next_job_completion_cost",
                    **score(y, estimated, model["y_scale"])})
                predictions.extend({"mode": mode, "target": target, "split": split,
                    "episode_id": r["episode_id"], "block_id": r["block_id"],
                    "observed": float(actual), "predicted": float(prediction)}
                    for r, actual, prediction in zip(subsets[split], y, estimated))
    errors = {mode: np.mean([r["normalized_mae"] for r in result if r["split"] == "validation" and r["mode"] == mode])
              for mode in PRIMARY_MODES} if targets_available else {}
    improvement = (1 - errors["BODY"] / errors["BLIND"]) if errors and errors["BLIND"] > 1e-12 else 0.0
    gate = {"pass": bool(len(targets_available) >= 2 and improvement >= GATE_RELATIVE_IMPROVEMENT),
            "split": "validation", "threshold_relative_mae_improvement": GATE_RELATIVE_IMPROVEMENT,
            "relative_mae_improvement": float(improvement), "targets": targets_available,
            "normalized_mae": {k: float(v) for k, v in errors.items()},
            "rule": "at least 2 nonconstant targets, BODY aggregate normalized MAE <= 0.9*BLIND; validation only",
            "inferential_claim": False,
            "independence_note": "descriptive probe gate; telemetry samples are not independent n for significance"}
    return {"schema_version": "k0-f-probe-v1", "rows": result, "gate": gate,
            "ridge_lambda": RIDGE_LAMBDA, "seed": seed, "fit_split": "train",
            "feature_standardization": "train only", "test_evaluated": include_test,
            "target_weighting": "equal average after train-target standard deviation scaling",
            "limitations": "Frozen ridge diagnostic; gate is validation screening, not primary causal policy success."}, fitted, predictions


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--include-test", action="store_true")
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args()
    rows = load_dataset(args.dataset)
    args.output.mkdir(parents=True, exist_ok=True)
    result, models, predictions = run_probe(rows, include_test=args.include_test, seed=args.seed)
    result["dataset_sha256"] = sha256(args.dataset)
    result["source_sha256"] = sha256(Path(__file__))
    result_path = args.output / "prediction_probe.json"
    if result_path.exists():
        previous = json.loads(result_path.read_text())
        if not args.include_test or previous.get("test_evaluated"):
            raise FileExistsError("probe result already exists")
        if previous["dataset_sha256"] != result["dataset_sha256"] or previous["gate"] != result["gate"] or previous["source_sha256"] != result["source_sha256"]:
            raise ValueError("held-out evaluation may not change dataset or validation gate")
        if not previous["gate"]["pass"]:
            raise RuntimeError("validation gate failed; held-out probe remains unexamined")
        json_write(args.output / "prediction_probe_validation_gate.json", previous)
    elif args.include_test:
        raise RuntimeError("first run and freeze validation-only probe before evaluating test")
    json_write(result_path, result)
    json_write(args.output / "prediction_probe_models.json", models)
    (args.output / "prediction_probe_predictions.jsonl").write_text("".join(json.dumps(r, allow_nan=False) + "\n" for r in predictions))
    print(json.dumps(result["gate"]), flush=True)


if __name__ == "__main__":
    main()
