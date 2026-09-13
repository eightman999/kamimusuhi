"""CPC trainer with observation-only input and per-epoch health telemetry."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import time

import numpy as np

os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")
import torch
import yaml

from .health import latent_statistics, perturbation_sensitivity
from .losses import info_nce_loss
from .models import build_model
from .retrieval import model_retrieval


def sha256(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def command_output(arguments):
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def load_observations(path: Path) -> np.ndarray:
    with np.load(path, allow_pickle=False) as archive:
        if set(archive.files) != {"obs"}:
            raise ValueError("DATA_LEAK: training archive must contain only obs")
        observations = np.asarray(archive["obs"], dtype=np.float32)
    if observations.ndim != 3 or min(observations.shape) < 2:
        raise ValueError("observations must be nonempty [N,T,D]")
    if not np.isfinite(observations).all():
        raise FloatingPointError("nonfinite observations")
    return observations


def _write_json(path: Path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def _checkpoint(path: Path, model, config, epoch, seed):
    torch.save({"config": config, "state_dict": model.state_dict(), "epoch": epoch, "seed": seed}, path)


def load_model(checkpoint: Path, device="cpu"):
    saved = torch.load(checkpoint, map_location=device, weights_only=True)
    model = build_model(saved["config"]).to(device)
    model.load_state_dict(saved["state_dict"])
    model.eval()
    return model, saved


def load_encoder(checkpoint: Path, device="cpu"):
    model, saved = load_model(checkpoint, device)

    @torch.no_grad()
    def encode(observations):
        values = np.asarray(observations, dtype=np.float32)
        if values.ndim != 3 or values.shape[-1] != int(saved["config"]["obs_dim"]):
            raise ValueError("expected observations [N,T,D]")
        encoded = []
        for start in range(0, len(values), 64):
            batch = torch.from_numpy(values[start:start + 64]).to(device)
            encoded.append(model.encoder(batch).cpu().numpy())
        return np.concatenate(encoded)

    return encode


@torch.no_grad()
def validation_loss(model, data, config, device):
    model.eval()
    total, count = 0.0, 0
    for start in range(0, len(data), int(config["batch_size"])):
        batch = torch.from_numpy(data[start:start + int(config["batch_size"])]).to(device)
        loss, _ = info_nce_loss(model, batch, config["temperature"])
        if not torch.isfinite(loss):
            raise FloatingPointError("nonfinite validation loss")
        total += float(loss) * len(batch)
        count += len(batch)
    return total / max(count, 1)


@torch.no_grad()
def _latent_statistics(model, data, config, device):
    model.eval()
    encoded = []
    for start in range(0, len(data), 64):
        batch = torch.from_numpy(data[start:start + 64]).to(device)
        encoded.append(model.encoder(batch).cpu().numpy())
    latent = np.concatenate(encoded)
    return latent_statistics(
        latent,
        float(config["collapse_std_threshold"]),
        float(config["collapse_active_fraction"]),
    )


def run(config_path: Path, data_dir: Path, output: Path, seed=0, device="cpu", max_epochs=None, initialize_only=False):
    config_path, data_dir, output = Path(config_path), Path(data_dir), Path(output)
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    config_bytes = config_path.read_bytes()
    (output / "config.yaml").write_bytes(config_bytes)
    os.chmod(output / "config.yaml", 0o444)
    config = yaml.safe_load(config_bytes)
    config_hash = hashlib.sha256(config_bytes).hexdigest()
    source_root = Path(__file__).parent
    manifest = {
        "experiment_id": "G0-v6",
        "config_hash": config_hash,
        "seed": int(seed),
        "device": device,
        "encoder": "GRU plus linear projection",
        "predictor": "per-horizon MLP",
        "objective": "InfoNCE",
        "hostname": socket.gethostname(),
        "platform": platform.platform(),
        "git_commit": command_output(["git", "rev-parse", "HEAD"]),
        "source_hashes": {
            str(path.relative_to(source_root)): sha256(path)
            for path in sorted(source_root.rglob("*.py"))
        },
        "provenance": "independent G0-v6 run; no G0-v5 checkpoint/result reuse",
        "pytorch": str(torch.__version__),
        "cuda": torch.version.cuda,
        "driver": command_output(["nvidia-smi", "--query-gpu=index,name,uuid,driver_version", "--format=csv,noheader"]),
        "gpu_name": None,
        "gpu_uuid": None,
        "start_time": datetime.now(timezone.utc).isoformat(),
        "status": "running",
        "failure_class": "NONE",
        "max_epochs_override": max_epochs,
        "initialize_only": bool(initialize_only),
        "training_selection": "validation InfoNCE loss only",
        "retrieval_selection": "telemetry only; never checkpoint selection",
    }
    _write_json(output / "manifest.json", manifest)
    try:
        if config.get("experiment_id") != "G0-v6" or config.get("method") != "cpc":
            raise ValueError("incorrect fixed G0-v6 CPC configuration")
        if device.startswith("cuda"):
            if not torch.cuda.is_available():
                raise RuntimeError("requested CUDA unavailable")
            torch.cuda.set_device(torch.device(device))
            props = torch.cuda.get_device_properties(torch.device(device))
            manifest.update(
                gpu_name=props.name,
                gpu_uuid=str(getattr(props, "uuid", "unavailable")),
            )
        torch.manual_seed(int(seed))
        np.random.seed(int(seed))
        torch.use_deterministic_algorithms(True)
        train = load_observations(data_dir / "train.npz")
        validation = load_observations(data_dir / "validation.npz")
        manifest["dataset_hashes"] = {
            name: sha256(data_dir / name) for name in ("train.npz", "validation.npz")
        }
        manifest["dataset_version"] = hashlib.sha256(
            json.dumps(manifest["dataset_hashes"], sort_keys=True).encode()
        ).hexdigest()
        for values in (train, validation):
            if values.shape[-1] != int(config["obs_dim"]):
                raise ValueError("observation dimension incompatible")
            if max(int(x) for x in config["horizons"]) >= values.shape[1]:
                raise ValueError("horizon incompatible")
        model = build_model(config).to(device)
        _checkpoint(output / "checkpoint_initial.pt", model, config, 0, seed)
        manifest["initial_checkpoint_hash"] = sha256(output / "checkpoint_initial.pt")
        if initialize_only:
            manifest.update(status="initialized", epochs_completed=0)
            return manifest

        optimizer = torch.optim.AdamW(
            model.parameters(),
            lr=float(config["learning_rate"]),
            weight_decay=float(config["weight_decay"]),
        )
        initial_loss = validation_loss(model, validation, config, device)
        manifest["initial_heldout_ssl_loss"] = initial_loss
        _write_json(output / "manifest.json", manifest)
        best_loss, best_epoch = float("inf"), 0
        rng = np.random.default_rng(int(seed))
        epochs = int(max_epochs if max_epochs is not None else config["epochs"])
        if epochs < 1:
            raise ValueError("epochs must be positive")
        records = []
        with (output / "train_log.jsonl").open("x") as log:
            for epoch in range(1, epochs + 1):
                model.train()
                order = rng.permutation(len(train))
                total, count = 0.0, 0
                epoch_started = time.monotonic()
                for start in range(0, len(order), int(config["batch_size"])):
                    batch = torch.from_numpy(train[order[start:start + int(config["batch_size"])]])
                    batch = batch.to(device)
                    optimizer.zero_grad(set_to_none=True)
                    loss, _ = info_nce_loss(model, batch, config["temperature"])
                    if not torch.isfinite(loss):
                        raise FloatingPointError("nonfinite training loss")
                    loss.backward()
                    torch.nn.utils.clip_grad_norm_(
                        model.parameters(), float(config["grad_clip"]), error_if_nonfinite=True
                    )
                    optimizer.step()
                    total += float(loss.detach()) * len(batch)
                    count += len(batch)
                heldout_loss = validation_loss(model, validation, config, device)
                latent = _latent_statistics(model, validation, config, device)
                sensitivity = perturbation_sensitivity(
                    model,
                    validation,
                    device,
                    float(config["perturbation_scale"]),
                )
                retrieval = model_retrieval(
                    model,
                    validation,
                    device,
                    config["horizons"],
                    int(config["retrieval_stride"]),
                    int(config["retrieval_max_episodes"]),
                    predictor=True,
                )
                record = {
                    "epoch": epoch,
                    "train_loss": total / max(count, 1),
                    "validation_loss": heldout_loss,
                    "info_nce_loss": heldout_loss,
                    "wall_clock": time.monotonic() - epoch_started,
                    "latent": latent,
                    "perturbation_sensitivity": sensitivity,
                    "temporal_retrieval": retrieval,
                }
                log.write(json.dumps(record, allow_nan=False) + "\n")
                log.flush()
                records.append(record)
                _checkpoint(output / "checkpoint_final.pt", model, config, epoch, seed)
                if heldout_loss < best_loss:
                    best_loss, best_epoch = heldout_loss, epoch
                    _checkpoint(output / "checkpoint_best.pt", model, config, epoch, seed)
                if latent["failure_class"] == "COLLAPSE_FULL":
                    manifest["failure_class"] = "COLLAPSE_FULL"
                    break
        np.savez_compressed(
            output / "latent_stats.npz",
            epoch=np.asarray([item["epoch"] for item in records]),
            latent_mean=np.asarray([item["latent"]["latent_mean"] for item in records]),
            latent_std=np.asarray([item["latent"]["latent_std"] for item in records]),
            per_dimension_std=np.asarray([item["latent"]["per_dimension_std"] for item in records]),
            covariance_spectrum=np.asarray([item["latent"]["covariance_spectrum"] for item in records]),
            effective_rank=np.asarray([item["latent"]["effective_rank"] for item in records]),
            retrieval_recall_at_1=np.asarray([
                item["temporal_retrieval"]["recall_at_1"] for item in records
            ]),
            retrieval_mrr=np.asarray([item["temporal_retrieval"]["mrr"] for item in records]),
            info_nce_loss=np.asarray([item["info_nce_loss"] for item in records]),
        )
        manifest.update(
            best_epoch=best_epoch,
            epochs_completed=len(records),
            best_heldout_ssl_loss=best_loss,
            final_heldout_ssl_loss=records[-1]["validation_loss"] if records else None,
            status="complete" if manifest["failure_class"] == "NONE" else "failed",
        )
        if sha256(output / "config.yaml") != config_hash or sha256(config_path) != config_hash:
            raise RuntimeError("configuration changed during run")
    except Exception as exc:
        if isinstance(exc, torch.cuda.OutOfMemoryError):
            classification = "OOM"
        elif isinstance(exc, FloatingPointError) or "nonfinite" in str(exc).lower():
            classification = "NAN"
        elif "DATA_LEAK" in str(exc):
            classification = "DATA_LEAK"
        else:
            classification = "SYSTEM_ERROR"
        manifest.update(
            status="failed",
            failure_class=classification,
            error=str(exc),
            error_type=type(exc).__name__,
        )
        raise
    finally:
        manifest.update(
            end_time=datetime.now(timezone.utc).isoformat(),
            wall_clock=time.monotonic() - started,
        )
        _write_json(output / "manifest.json", manifest)
    return manifest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--max-epochs", type=int)
    parser.add_argument("--initialize-only", action="store_true")
    args = parser.parse_args()
    run(
        args.config,
        args.data_dir,
        args.output,
        args.seed,
        args.device,
        args.max_epochs,
        args.initialize_only,
    )


if __name__ == "__main__":
    main()
