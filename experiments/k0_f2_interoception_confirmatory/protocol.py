"""Read a pre-data confirmatory lock and verify its scientific source identities."""
from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NAMESPACE = "experiments/k0_f2_interoception_confirmatory"
TASKS = [dict(matrix_dimension=n, repetitions=r, deadline_seconds=d)
         for n, r, d in ((128, 8, .003), (512, 16, .008), (1024, 16, .025), (2048, 8, .07))]
ACTIONS = ["RUN_CPU", "RUN_RTX3060", "RUN_P100", "WAIT"]
LABELS = ["idle", "cpu_light", "rtx3060_matrix", "p100_matrix", "dual_gpu", "disk_io", "network_transfer", "mixed"]
SESSIONS = {"A": {"split": "train", "seed": 911001}, "B": {"split": "train", "seed": 912003},
            "C": {"split": "validation", "seed": 913007}, "D": {"split": "test", "seed": 914009}}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def lock_sha256(path):
    return sha256(path)


def load_lock(path, verify_sources=True):
    path = Path(path)
    data = json.loads(path.read_text())
    if data.get("schema_version") != "k0-f2-protocol-lock-v1" or data.get("status") != "locked_before_fresh_data":
        raise ValueError("F2 confirmatory lock is absent or malformed")
    if data.get("actions") != ACTIONS or data.get("tasks") != TASKS or data.get("sessions") != SESSIONS:
        raise ValueError("F2 fixed action/task/session protocol differs")
    if data.get("policy", {}).get("seeds") != list(range(12)):
        raise ValueError("Exactly 12 preregistered seeds required")
    if verify_sources:
        for name, digest in data["source_files_sha256"].items():
            source = ROOT / name
            if not source.is_file() or sha256(source) != digest:
                raise ValueError("Locked scientific source changed: " + name)
        if not data["source_files_sha256"]:
            raise ValueError("No scientific sources frozen")
    return data


def lock_receipt(path):
    data = load_lock(path)
    commit = subprocess.check_output(["git", "rev-parse", "F2_PROTOCOL_LOCK"], cwd=ROOT, text=True).strip()
    return dict(protocol_lock_sha256=lock_sha256(path), protocol_lock_commit=commit,
                source_commit=data["source_commit"], locked_at=data["locked_at"])


def task_features(task):
    n = task["matrix_dimension"]
    return [n / 2048, task["repetitions"] / 16, task["deadline_seconds"] / .1, 3 * n * n * 4 / (64 * 1024**2)]
