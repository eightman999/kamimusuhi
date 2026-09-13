"""Immutable G0-v6 protocol, identity lock, and scientific comparisons."""

from pathlib import Path
import ast
import hashlib
import json

import numpy as np


ROOT = Path(__file__).resolve().parents[1]
PROTOCOL = {
    "experiment_id": "G0-v6",
    "revision": 2,
    "revision_label": "G0-v6.1 corrective rerun: fixed unseen-horizon retrieval negative scoring and PCA plot reshape",
    "dataset_reference": "G0-v5 frozen dataset, byte-identical copy",
    "dataset_seed": 20260913,
    "n_train": 1024,
    "n_eval": 256,
    "obs_dim": 24,
    "episode_length": 48,
    "latent_dim": 32,
    "pca_dimensions": 16,
    "seeds": [0, 1, 2, 3, 4],
    "epochs": 20,
    "horizons": [1, 4],
    "unseen_horizons": [2, 8],
    "retrieval_primary": "iid/train_like/recall_at_1",
    "retrieval_stride": 4,
    "retrieval_max_episodes": 128,
    "eval_repeat_atol": 1e-7,
    "eval_repeat_rtol": 1e-6,
    "checkpoint_selection": "validation SSL loss only; no oracle labels",
    "strong_pass": "match and combo improve in >=4/5; primary retrieval improves in >=4/5; IS or midctx improves in >=4/5; no invalidity",
    "partial": "match and combo improve in >=4/5 while dynseg deteriorates in >=4/5",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_files(root: Path = ROOT):
    excluded = {"results", "data", "plots", "manifests", "__pycache__"}
    paths = []
    for path in Path(root).rglob("*"):
        if not path.is_file() or any(part in excluded for part in path.relative_to(root).parts):
            continue
        if path.name in {"G0_V6_REPORT.md", "protocol_lock.json"}:
            continue
        if path.suffix in {".py", ".yaml", ".sh", ".md", ".gitignore"}:
            paths.append(path)
    return sorted(paths)


def snapshot(root: Path = ROOT):
    return {str(path.relative_to(root)): sha256(path) for path in source_files(root)}


def freeze(root: Path, destination: Path, dataset_manifest: Path, reference: Path):
    destination = Path(destination)
    record = {
        "protocol": PROTOCOL,
        "source_files": snapshot(root),
        "dataset_manifest_sha256": sha256(dataset_manifest),
        "g0_v5_reference_sha256": sha256(reference),
    }
    record["lock_hash"] = hashlib.sha256(json.dumps(record, sort_keys=True).encode()).hexdigest()
    with destination.open("x") as stream:
        json.dump(record, stream, indent=2, sort_keys=True)
        stream.write("\n")
    return record


def verify_lock(root: Path, lock_path: Path, dataset_manifest: Path, reference: Path):
    record = json.loads(Path(lock_path).read_text())
    expected = record.get("lock_hash")
    unsigned = {key: value for key, value in record.items() if key != "lock_hash"}
    actual = hashlib.sha256(json.dumps(unsigned, sort_keys=True).encode()).hexdigest()
    if actual != expected:
        raise RuntimeError("protocol lock hash mismatch")
    if snapshot(root) != record["source_files"]:
        raise RuntimeError("frozen G0-v6 source changed; fresh protocol required")
    if sha256(dataset_manifest) != record["dataset_manifest_sha256"]:
        raise RuntimeError("frozen dataset manifest changed")
    if sha256(reference) != record["g0_v5_reference_sha256"]:
        raise RuntimeError("G0-v5 reference audit changed")
    return record


def same_metrics(a, b):
    """Compare scientific JSON fields while excluding runtime durations."""
    if isinstance(a, dict):
        if not isinstance(b, dict) or set(a) != set(b):
            return False
        return all(same_metrics(a[key], b[key]) for key in a if key not in {"runtime", "evaluation_seconds"})
    if isinstance(a, (list, tuple)):
        return isinstance(b, (list, tuple)) and len(a) == len(b) and all(
            same_metrics(x, y) for x, y in zip(a, b)
        )
    if isinstance(a, (float, int)) and not isinstance(a, bool):
        return bool(np.isclose(a, b, atol=1e-7, rtol=1e-6, equal_nan=False))
    return a == b


def finite_json(value):
    if isinstance(value, dict):
        return all(finite_json(item) for item in value.values())
    if isinstance(value, (list, tuple)):
        return all(finite_json(item) for item in value)
    if isinstance(value, (float, int)) and not isinstance(value, bool):
        return bool(np.isfinite(value))
    return True


def leakage_audit(root: Path = ROOT):
    """AST audit the complete training-side source boundary."""
    training_parts = [
        Path(root) / "src" / "train.py",
        Path(root) / "src" / "health.py",
        Path(root) / "src" / "retrieval.py",
        Path(root) / "src" / "models" / "__init__.py",
        Path(root) / "src" / "losses" / "__init__.py",
    ]
    forbidden = {
        "cause",
        "context",
        "nuisance",
        "composition_id",
        "ood_category",
        "pair_label",
        "intervention_type",
        "oracle",
    }
    findings = []
    for path in training_parts:
        tree = ast.parse(path.read_text(), filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom) and any(
                part in {"evaluate", "dataset"} for part in (node.module or "").split(".")
            ):
                findings.append(f"{path.name}: imports evaluator/dataset")
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                lowered = node.value.lower()
                if any(token in lowered for token in ("cause", "context", "nuisance", "pair_label", "ood_label")):
                    findings.append(f"{path.name}: oracle token in training string")
            if isinstance(node, ast.Name) and node.id in forbidden:
                findings.append(f"{path.name}: oracle identifier {node.id}")
    return {
        "passed": not findings,
        "findings": sorted(set(findings)),
        "boundary": "trainer subprocess receives training/*.npz only; evaluation archives are orchestration-only",
        "limitation": "static/code boundary, not an OS sandbox against the same user",
    }
