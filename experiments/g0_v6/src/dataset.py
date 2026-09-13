"""Strict loaders for the frozen G0-v6 observation/evaluation dataset.

The training boundary accepts archives containing exactly ``obs``. Oracle
arrays are read only by the evaluator and never enter the training process.
"""

from pathlib import Path
import hashlib
import json

import numpy as np


REQUIRED_EVALUATION = (
    "iid_test.npz",
    "match_ood.npz",
    "dynseg_ood.npz",
    "combo_oodctx.npz",
    "midctx.npz",
    "intervention.npz",
)


def sha256(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_training(path: Path) -> np.ndarray:
    """Load one training archive while enforcing the observation firewall."""
    with np.load(path, allow_pickle=False) as archive:
        if set(archive.files) != {"obs"}:
            raise ValueError("DATA_LEAK: training archive must contain only obs")
        observations = np.asarray(archive["obs"], dtype=np.float32)
    if observations.ndim != 3 or min(observations.shape) < 2:
        raise ValueError("observations must be nonempty [N,T,D]")
    if not np.isfinite(observations).all():
        raise FloatingPointError("nonfinite observations")
    return observations


def load_evaluation(path: Path) -> dict[str, np.ndarray]:
    """Load an evaluator archive; this function is never imported by training."""
    with np.load(path, allow_pickle=False) as archive:
        return {key: archive[key].copy() for key in archive.files}


def verify_frozen(data_dir: Path, reference_path: Path | None = None) -> dict:
    """Verify the copied frozen data and its byte-level file manifest."""
    data_dir = Path(data_dir)
    manifest_path = data_dir / "dataset_manifest.json"
    manifest = json.loads(manifest_path.read_text())
    for rel, expected in manifest["files"].items():
        actual = sha256(data_dir / rel)
        if actual != expected:
            raise RuntimeError(f"DATA_LEAK: dataset hash mismatch: {rel}")
    for name in ("train.npz", "validation.npz"):
        loaded = load_training(data_dir / "training" / name)
        if loaded.shape[-1] != int(manifest["obs_dim"]):
            raise RuntimeError(f"dataset dimension mismatch: {name}")
    for name in REQUIRED_EVALUATION:
        if not (data_dir / "evaluation" / name).exists():
            raise RuntimeError(f"missing evaluation split: {name}")
    if reference_path is not None:
        reference = json.loads(Path(reference_path).read_text())
        actual_manifest_hash = sha256(manifest_path)
        if actual_manifest_hash != reference["dataset_manifest_sha256"]:
            raise RuntimeError("G0-v5 equivalence audit failed: manifest hash")
        for rel, expected in reference["dataset_files"].items():
            if sha256(data_dir / rel) != expected:
                raise RuntimeError(f"G0-v5 equivalence audit failed: {rel}")
    return manifest
