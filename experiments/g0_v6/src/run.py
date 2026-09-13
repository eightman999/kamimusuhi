"""G0-v6 stages: audit, lock, controls, training, paired evaluation, aggregate."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

import numpy as np
import torch
import yaml

from .controls import PCAControl, raw_encode, temporal_readout
from .dataset import load_evaluation, load_training, sha256, verify_frozen
from .evaluate import evaluate_representation
from .protocol import (
    PROTOCOL,
    finite_json,
    freeze,
    leakage_audit,
    same_metrics,
    verify_lock,
)
from .retrieval import model_retrieval, retrieval_from_latent
from .train import load_encoder, load_model


ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data" / "frozen"
REFERENCE = ROOT / "manifests" / "g0_v5_reference.json"
LOCK = ROOT / "manifests" / "protocol_lock.json"
CONTROLS = ROOT / "results" / "controls"
FULL = ROOT / "results" / "full"
CONFIG = ROOT / "configs" / "cpc.yaml"


def write_json(path: Path, value, overwrite=False):
    path = Path(path)
    if path.exists() and not overwrite:
        existing = json.loads(path.read_text())
        if existing != value:
            raise FileExistsError(f"refusing to overwrite {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def load_config():
    return yaml.safe_load(CONFIG.read_bytes())


def audit():
    manifest = verify_frozen(DATA, REFERENCE)
    isolation = leakage_audit(ROOT)
    result = {
        "experiment_id": "G0-v6",
        "dataset_manifest_sha256": sha256(DATA / "dataset_manifest.json"),
        "g0_v5_reference_sha256": sha256(REFERENCE),
        "dataset_manifest": manifest,
        "training_input_keys": ["obs"],
        "isolation": isolation,
        "old_results_reused": False,
        "old_checkpoints_reused": False,
        "evaluator_equivalence": "byte-identical frozen dataset and declared split contract",
    }
    write_json(ROOT / "manifests" / "equivalence_audit.json", result)
    if not isolation["passed"]:
        raise RuntimeError(json.dumps(isolation))
    print(json.dumps({"stage": "audit", "passed": True, "files": len(manifest["files"])}, indent=2))


def protocol_lock():
    if LOCK.exists():
        record = verify_lock(ROOT, LOCK, DATA / "dataset_manifest.json", REFERENCE)
        print(json.dumps({"stage": "protocol-lock", "existing": True, "lock_hash": record["lock_hash"]}, indent=2))
        return
    if not (ROOT / "manifests" / "equivalence_audit.json").exists():
        audit()
    record = freeze(ROOT, LOCK, DATA / "dataset_manifest.json", REFERENCE)
    print(json.dumps({"stage": "protocol-lock", "created": True, "lock_hash": record["lock_hash"]}, indent=2))


def require_lock():
    if not LOCK.exists():
        raise RuntimeError("protocol lock missing; run protocol-lock first")
    return verify_lock(ROOT, LOCK, DATA / "dataset_manifest.json", REFERENCE)


def _evaluate_and_repeat(encode, output_dir: Path):
    if output_dir.exists() and any(output_dir.iterdir()):
        raise FileExistsError(f"evaluation output already exists: {output_dir}")
    metrics = evaluate_representation(encode, DATA, output_dir)
    repeat = evaluate_representation(encode, DATA, None)
    passed = same_metrics(metrics, repeat)
    write_json(output_dir / "metrics.json", metrics)
    write_json(
        output_dir / "repeat_check.json",
        {
            "passed": passed,
            "atol": PROTOCOL["eval_repeat_atol"],
            "rtol": PROTOCOL["eval_repeat_rtol"],
        },
    )
    if not passed:
        raise RuntimeError(f"EVAL_ERROR: repeated evaluation differs for {output_dir}")
    return metrics


def _control_retrieval(encode, observations):
    validation = observations
    context = load_evaluation(DATA / "evaluation" / "match_ood.npz")["obs"]
    altered = load_evaluation(DATA / "evaluation" / "dynseg_ood.npz")["obs"]
    return {
        "iid": {
            "train_like": retrieval_from_latent(encode(validation), PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
            "unseen_horizon": retrieval_from_latent(encode(validation), PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
        },
        "context_ood": {
            "train_like": retrieval_from_latent(encode(context), PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
            "unseen_horizon": retrieval_from_latent(encode(context), PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
        },
        "altered_segment_timing": {
            "train_like": retrieval_from_latent(encode(altered), PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
            "unseen_horizon": retrieval_from_latent(encode(altered), PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"]),
        },
    }


def controls():
    lock = require_lock()
    train = load_training(DATA / "training" / "train.npz")
    validation = load_training(DATA / "training" / "validation.npz")
    controls_to_run = [("raw", raw_encode), ("pca", PCAControl(train, PROTOCOL["pca_dimensions"]))]
    for name, encoder in controls_to_run:
        output = CONTROLS / name
        if output.exists() and (output / "metrics.json").exists():
            existing = json.loads((output / "manifest.json").read_text())
            if existing.get("lock_hash") != lock["lock_hash"]:
                raise RuntimeError(f"control identity mismatch: {name}")
            continue
        metrics = _evaluate_and_repeat(encoder, output / "evaluation")
        readout = temporal_readout(encoder, train, validation)
        retrieval = _control_retrieval(encoder, validation)
        write_json(output / "metrics.json", metrics)
        write_json(output / "temporal_readout.json", readout)
        write_json(output / "retrieval.json", retrieval)
        write_json(
            output / "manifest.json",
            {
                "experiment_id": "G0-v6",
                "control": name,
                "lock_hash": lock["lock_hash"],
                "dataset_manifest_sha256": sha256(DATA / "dataset_manifest.json"),
                "fit_scope": "train observations only" if name == "pca" else "no fit",
            },
        )
        if name == "pca":
            encoder.save(output / "pca.npz")
    write_json(
        CONTROLS / "sanity.json",
        {
            "experiment_id": "G0-v6",
            "lock_hash": lock["lock_hash"],
            "dataset_manifest_sha256": sha256(DATA / "dataset_manifest.json"),
            "raw_pca_repeat": True,
            "old_g0": "SKIP: historical results/checkpoints prohibited",
        },
    )
    print(json.dumps({"stage": "controls", "passed": True}, indent=2))


def _run_train(seed: int, device: str, output: Path, max_epochs=None):
    env = os.environ.copy()
    env.update(
        CUBLAS_WORKSPACE_CONFIG=":4096:8",
        OMP_NUM_THREADS="2",
        OPENBLAS_NUM_THREADS="2",
        MKL_NUM_THREADS="2",
    )
    command = [
        sys.executable,
        "-m",
        "experiments.g0_v6.src.train",
        "--config",
        str(CONFIG),
        "--data-dir",
        str(DATA / "training"),
        "--output",
        str(output),
        "--seed",
        str(seed),
        "--device",
        device,
    ]
    if max_epochs is not None:
        command.extend(["--max-epochs", str(max_epochs)])
    return subprocess.run(command, env=env, check=False).returncode


def train(seed: int, device: str, max_epochs=None, smoke=False):
    require_lock()
    if seed not in PROTOCOL["seeds"] and not smoke:
        raise ValueError(f"seed is not in fixed protocol: {seed}")
    output = ROOT / "results" / ("smoke" if smoke else "full") / (f"seed{seed}" if not smoke else f"seed{seed}") / "trained"
    if output.exists():
        manifest_path = output / "manifest.json"
        if manifest_path.exists():
            manifest = json.loads(manifest_path.read_text())
            if manifest.get("seed") == seed and manifest.get("device") == device and (
                smoke or manifest.get("status") == "complete"
            ):
                print(json.dumps({"stage": "train", "skipped": True, "seed": seed, "status": manifest.get("status")}, indent=2))
                return 0
        raise FileExistsError(f"refusing to reuse mismatched training output: {output}")
    code = _run_train(seed, device, output, max_epochs)
    print(json.dumps({"stage": "train", "seed": seed, "device": device, "returncode": code}, indent=2))
    return code


def _state_hash(state_dict):
    digest = hashlib.sha256()
    for key in sorted(state_dict):
        digest.update(key.encode())
        digest.update(state_dict[key].detach().cpu().numpy().tobytes())
    return digest.hexdigest()


def _retrieval_suite(model, device):
    validation = load_training(DATA / "training" / "validation.npz")
    context = load_evaluation(DATA / "evaluation" / "match_ood.npz")["obs"]
    altered = load_evaluation(DATA / "evaluation" / "dynseg_ood.npz")["obs"]
    return {
        "iid": {
            "train_like": model_retrieval(model, validation, device, PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=True),
            "unseen_horizon": model_retrieval(model, validation, device, PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=False),
        },
        "context_ood": {
            "train_like": model_retrieval(model, context, device, PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=True),
            "unseen_horizon": model_retrieval(model, context, device, PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=False),
        },
        "altered_segment_timing": {
            "train_like": model_retrieval(model, altered, device, PROTOCOL["horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=True),
            "unseen_horizon": model_retrieval(model, altered, device, PROTOCOL["unseen_horizons"], PROTOCOL["retrieval_stride"], PROTOCOL["retrieval_max_episodes"], predictor=False),
        },
    }


def _save_geometry(model, device, output: Path):
    data = load_evaluation(DATA / "evaluation" / "dynseg_ood.npz")
    observations = data["obs"]
    encoded = []
    with torch.no_grad():
        model.eval()
        for start in range(0, len(observations), 64):
            batch = torch.from_numpy(observations[start:start + 64].astype(np.float32)).to(device)
            encoded.append(model.encoder(batch).cpu().numpy())
    latent = np.concatenate(encoded)[:, ::4]
    time_index = np.tile(np.arange(latent.shape[1]) * 4, latent.shape[0])
    causes = data["cause"][:, ::4].reshape(-1)
    contexts = data["context"][:, ::4].reshape(-1)
    segments = np.zeros_like(causes)
    for row in range(len(data["cause"])):
        values = data["cause"][row]
        changes = np.cumsum(np.r_[0, values[1:] != values[:-1]])
        segments[row * latent.shape[1]:(row + 1) * latent.shape[1]] = changes[::4]
    np.savez_compressed(output / "geometry.npz", latent=latent, time=time_index, cause=causes, context=contexts, segment=segments)


def evaluate_seed(seed: int):
    lock = require_lock()
    base = FULL / f"seed{seed}"
    trained_dir = base / "trained"
    manifest_path = trained_dir / "manifest.json"
    if not manifest_path.exists():
        raise FileNotFoundError(f"missing training output for seed {seed}")
    training_manifest = json.loads(manifest_path.read_text())
    if training_manifest.get("status") != "complete":
        raise RuntimeError(f"training did not complete for seed {seed}: {training_manifest.get('failure_class')}")
    trained_model, trained_saved = load_model(trained_dir / "checkpoint_best.pt", "cpu")
    twin_model, twin_saved = load_model(trained_dir / "checkpoint_initial.pt", "cpu")
    trained_initial = torch.load(trained_dir / "checkpoint_initial.pt", map_location="cpu", weights_only=True)
    initial_hash = _state_hash(trained_initial["state_dict"])
    twin_hash = _state_hash(twin_saved["state_dict"])
    torch.manual_seed(seed)
    expected_model = __import__("experiments.g0_v6.src.models", fromlist=["build_model"]).build_model(load_config())
    independently_initialized = _state_hash(expected_model.state_dict()) == initial_hash
    if initial_hash != twin_hash:
        raise RuntimeError("INVALID: exact twin initial state hash mismatch")

    trained_eval = _evaluate_and_repeat(load_encoder(trained_dir / "checkpoint_best.pt", "cpu"), base / "trained" / "evaluation")
    twin_eval = _evaluate_and_repeat(load_encoder(trained_dir / "checkpoint_initial.pt", "cpu"), base / "twin" / "evaluation")
    train_obs = load_training(DATA / "training" / "train.npz")
    validation_obs = load_training(DATA / "training" / "validation.npz")
    trained_encoder = load_encoder(trained_dir / "checkpoint_best.pt", "cpu")
    twin_encoder = load_encoder(trained_dir / "checkpoint_initial.pt", "cpu")
    trained_readout = temporal_readout(trained_encoder, train_obs, validation_obs)
    twin_readout = temporal_readout(twin_encoder, train_obs, validation_obs)
    trained_retrieval = _retrieval_suite(trained_model, "cpu")
    twin_retrieval = _retrieval_suite(twin_model, "cpu")
    _save_geometry(trained_model, "cpu", base / "trained")
    summary = {
        "experiment_id": "G0-v6",
        "seed": seed,
        "lock_hash": lock["lock_hash"],
        "training_manifest": {
            "device": training_manifest.get("device"),
            "gpu_name": training_manifest.get("gpu_name"),
            "gpu_uuid": training_manifest.get("gpu_uuid"),
            "cuda": training_manifest.get("cuda"),
            "driver": training_manifest.get("driver"),
            "wall_clock": training_manifest.get("wall_clock"),
            "epochs_completed": training_manifest.get("epochs_completed"),
        },
        "checks": {
            "trained_repeat": True,
            "twin_repeat": True,
            "initial_state_hash": initial_hash,
            "twin_initial_state_hash": twin_hash,
            "initial_state_hash_identical": initial_hash == twin_hash,
            "independently_initialized_hash_matches": independently_initialized,
            "finite": finite_json(trained_eval) and finite_json(twin_eval),
        },
        "trained": trained_eval,
        "twin": twin_eval,
        "future_observation_readout": {
            "trained": trained_readout,
            "twin": twin_readout,
            "delta_mse": trained_readout["mse"] - twin_readout["mse"],
            "delta_percent_trained_vs_twin": 100.0 * (trained_readout["mse"] - twin_readout["mse"]) / max(twin_readout["mse"], 1e-12),
        },
        "temporal_retrieval": {"trained": trained_retrieval, "twin": twin_retrieval},
    }
    write_json(base / "seed_summary.json", summary)
    write_json(base / "trained" / "evaluation" / "retrieval.json", trained_retrieval)
    write_json(base / "twin" / "evaluation" / "retrieval.json", twin_retrieval)
    print(json.dumps({"stage": "evaluate", "seed": seed, "passed": True}, indent=2))


def _value(summary, representation, metric):
    row = summary[representation]
    if metric == "midctx":
        return row["midctx"]["stability"]
    if metric == "is":
        return row["intervention"]["selectivity"]
    return row[metric]["auc"]


def _retrieval_value(summary, representation):
    return summary["temporal_retrieval"][representation]["iid"]["train_like"]["recall_at_1"]


def _bootstrap(values, seed):
    values = np.asarray(values, dtype=float)
    rng = np.random.default_rng(seed)
    samples = values[rng.integers(0, len(values), size=(10000, len(values)))].mean(axis=1)
    return [float(np.quantile(samples, 0.025)), float(np.quantile(samples, 0.975))]


def aggregate():
    lock = require_lock()
    summaries = []
    for seed in PROTOCOL["seeds"]:
        path = FULL / f"seed{seed}" / "seed_summary.json"
        if not path.exists():
            raise FileNotFoundError(f"missing seed summary: {path}")
        summaries.append(json.loads(path.read_text()))
    metrics = {}
    for name in ("iid", "midctx", "match_ood", "dynseg_ood", "combo_ood"):
        values = [_value(summary, "trained", name) - _value(summary, "twin", name) for summary in summaries]
        metrics[name] = {
            "delta_definition": "trained - exact twin",
            "values_by_seed": {str(seed): value for seed, value in zip(PROTOCOL["seeds"], values)},
            "mean": float(np.mean(values)),
            "median": float(np.median(values)),
            "std": float(np.std(values, ddof=1)),
            "bootstrap_95ci": _bootstrap(values, 801 + len(metrics)),
            "improved_count": int(sum(value > 0 for value in values)),
        }
    is_values = [_value(summary, "trained", "is") - _value(summary, "twin", "is") for summary in summaries]
    metrics["intervention_selectivity"] = {
        "delta_definition": "trained - exact twin",
        "values_by_seed": {str(seed): value for seed, value in zip(PROTOCOL["seeds"], is_values)},
        "mean": float(np.mean(is_values)),
        "median": float(np.median(is_values)),
        "std": float(np.std(is_values, ddof=1)),
        "bootstrap_95ci": _bootstrap(is_values, 812),
        "improved_count": int(sum(value > 0 for value in is_values)),
    }
    retrieval_values = [_retrieval_value(summary, "trained") - _retrieval_value(summary, "twin") for summary in summaries]
    metrics["temporal_retrieval_iid_train_like_recall_at_1"] = {
        "delta_definition": "trained - exact twin",
        "values_by_seed": {str(seed): value for seed, value in zip(PROTOCOL["seeds"], retrieval_values)},
        "mean": float(np.mean(retrieval_values)),
        "median": float(np.median(retrieval_values)),
        "std": float(np.std(retrieval_values, ddof=1)),
        "bootstrap_95ci": _bootstrap(retrieval_values, 813),
        "improved_count": int(sum(value > 0 for value in retrieval_values)),
    }
    readout_values = [summary["future_observation_readout"]["delta_mse"] for summary in summaries]
    metrics["future_observation_mse"] = {
        "delta_definition": "trained - exact twin; negative is improved",
        "values_by_seed": {str(seed): value for seed, value in zip(PROTOCOL["seeds"], readout_values)},
        "mean": float(np.mean(readout_values)),
        "median": float(np.median(readout_values)),
        "std": float(np.std(readout_values, ddof=1)),
        "bootstrap_95ci": _bootstrap(readout_values, 814),
        "improved_count": int(sum(value < 0 for value in readout_values)),
    }

    raw_values = {}
    for metric in ("iid", "midctx", "match_ood", "dynseg_ood", "combo_ood", "is"):
        raw_values[metric] = {
            "trained": {str(seed): _value(summary, "trained", metric) for seed, summary in zip(PROTOCOL["seeds"], summaries)},
            "twin": {str(seed): _value(summary, "twin", metric) for seed, summary in zip(PROTOCOL["seeds"], summaries)},
        }
    raw_values["future_observation_mse"] = {
        "trained": {str(seed): summary["future_observation_readout"]["trained"]["mse"] for seed, summary in zip(PROTOCOL["seeds"], summaries)},
        "twin": {str(seed): summary["future_observation_readout"]["twin"]["mse"] for seed, summary in zip(PROTOCOL["seeds"], summaries)},
    }
    raw_values["temporal_retrieval_iid_train_like_recall_at_1"] = {
        "trained": {str(seed): _retrieval_value(summary, "trained") for seed, summary in zip(PROTOCOL["seeds"], summaries)},
        "twin": {str(seed): _retrieval_value(summary, "twin") for seed, summary in zip(PROTOCOL["seeds"], summaries)},
    }
    invalid_reasons = []
    for summary in summaries:
        if summary.get("lock_hash") != lock["lock_hash"]:
            invalid_reasons.append(f"seed{summary['seed']}: lock mismatch")
        if not all(bool(value) for value in summary["checks"].values()):
            invalid_reasons.append(f"seed{summary['seed']}: check failed")
        for representation in ("trained", "twin"):
            if summary[representation].get("failure_class") not in ("NONE", None):
                invalid_reasons.append(f"seed{summary['seed']}/{representation}: {summary[representation].get('failure_class')}")
    match_count = metrics["match_ood"]["improved_count"]
    combo_count = metrics["combo_ood"]["improved_count"]
    retrieval_count = metrics["temporal_retrieval_iid_train_like_recall_at_1"]["improved_count"]
    is_count = metrics["intervention_selectivity"]["improved_count"]
    mid_count = metrics["midctx"]["improved_count"]
    dyn_deterioration = int(sum(value < 0 for value in metrics["dynseg_ood"]["values_by_seed"].values()))
    if invalid_reasons:
        verdict = "G0-V6_INVALID"
    elif match_count >= 4 and combo_count >= 4 and retrieval_count >= 4 and max(is_count, mid_count) >= 4:
        verdict = "G0-V6_STRONG_PASS"
    elif match_count >= 4 and combo_count >= 4 and dyn_deterioration >= 4:
        verdict = "G0-V6_PARTIAL"
    else:
        verdict = "G0-V6_FAIL"
    aggregate_result = {
        "experiment_id": "G0-v6",
        "lock_hash": lock["lock_hash"],
        "seeds": PROTOCOL["seeds"],
        "verdict": verdict,
        "invalid_reasons": invalid_reasons,
        "metrics": metrics,
        "raw_values": raw_values,
        "direction_counts": {
            "match_ood_improved": match_count,
            "combo_ood_improved": combo_count,
            "temporal_retrieval_improved": retrieval_count,
            "intervention_selectivity_improved": is_count,
            "midctx_improved": mid_count,
            "dynseg_deteriorated": dyn_deterioration,
        },
        "source": "five independent CPC runs, paired with saved exact untrained twins",
    }
    write_json(ROOT / "results" / "aggregate.json", aggregate_result)
    print(json.dumps({"stage": "aggregate", "verdict": verdict, "direction_counts": aggregate_result["direction_counts"]}, indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stage", choices=["audit", "protocol-lock", "controls", "smoke", "train", "evaluate", "aggregate"])
    parser.add_argument("--seed", type=int)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--max-epochs", type=int)
    args = parser.parse_args()
    if args.stage == "audit":
        audit()
    elif args.stage == "protocol-lock":
        protocol_lock()
    elif args.stage == "controls":
        controls()
    elif args.stage == "smoke":
        train(args.seed if args.seed is not None else 0, args.device, args.max_epochs or 1, smoke=True)
    elif args.stage == "train":
        if args.seed is None:
            parser.error("train requires --seed")
        raise SystemExit(train(args.seed, args.device, args.max_epochs))
    elif args.stage == "evaluate":
        if args.seed is None:
            parser.error("evaluate requires --seed")
        evaluate_seed(args.seed)
    elif args.stage == "aggregate":
        aggregate()


if __name__ == "__main__":
    main()
