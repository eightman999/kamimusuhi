"""nvidia-smi GPU inventory. Returns [] when nvidia-smi is absent."""
from __future__ import annotations

import csv
import io
import shutil
import subprocess

_FIELDS = ("index", "uuid", "name", "memory.total", "memory.used",
           "utilization.gpu", "temperature.gpu", "driver_version")


def _num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def query_gpus() -> list[dict]:
    if not shutil.which("nvidia-smi"):
        return []
    try:
        out = subprocess.run(
            ["nvidia-smi", f"--query-gpu={','.join(_FIELDS)}",
             "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=5, check=True)
    except (OSError, subprocess.SubprocessError):
        return []
    gpus = []
    for row in csv.reader(io.StringIO(out.stdout), skipinitialspace=True):
        if len(row) != len(_FIELDS):
            continue
        gpus.append({
            "index": int(row[0]),
            "uuid": row[1],
            "name": row[2],
            "memory_total_mb": _num(row[3]),
            "memory_used_mb": _num(row[4]),
            "utilization_pct": _num(row[5]),
            "temperature_c": _num(row[6]),
            "driver_version": row[7],
        })
    return gpus
