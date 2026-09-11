"""The immutable FBA0 reference description.

FBA0 is the FlyWire v783 connectome simulated with the LIF + alpha
synapse model of Shiu et al. (2024) as released by eonsystemspbc in the
``fly-brain`` repository (GPL-2.0). This module never vendors code or
data from that repo; it only records identity metadata and, when the
data directory is available, a content hash of the data files.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path

DATA_FILES = ("2025_Completeness_783.csv", "2025_Connectivity_783.parquet")


def _data_dir() -> Path | None:
    env = os.environ.get("MIOBA_FLY_BRAIN_DATA")
    if env:
        p = Path(env)
        return p if p.is_dir() else None
    return None


def _hash_files() -> str | None:
    d = _data_dir()
    if not d:
        return None
    h = hashlib.sha256()
    found = False
    for name in sorted(DATA_FILES):
        f = d / name
        if f.is_file():
            found = True
            h.update(name.encode())
            h.update(f.read_bytes())
    return h.hexdigest() if found else None


def _neuron_count() -> int | None:
    d = _data_dir()
    if not d:
        return None
    comp = d / "2025_Completeness_783.csv"
    if not comp.is_file():
        return None
    try:
        # one data row per FlyWire root id; header is the first line
        with comp.open("rb") as fh:
            return max(0, sum(1 for _ in fh) - 1)
    except OSError:
        return None


def _build() -> dict:
    return {
        "name": "flywire-v783-shiu-lif",
        "neurons": _neuron_count(),
        "source": "eonsystemspbc/fly-brain data (FlyWire v783)",
        "license_note": (
            "FlyWire v783 connectivity derived data per fly-brain repo "
            "(GPL-2.0 code); data used read-only, never vendored."
        ),
        "data_hash": _hash_files(),
    }


FBA0_REFERENCE = _build()


def fba0_neuron_count() -> int | None:
    return FBA0_REFERENCE["neurons"]
