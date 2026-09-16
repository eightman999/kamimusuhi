"""Canonical anatomy store — the dataset-agnostic interchange format.

Importers (``importers/banc.py`` etc.) translate dataset-specific files
into this store; the runtime backend and Observatory read only this
format and never learn about BANC/FAFB specifics (AFC §3).

Layout of a store directory::

    manifest.json         dataset kind, source file hashes, fidelity
                          summary, per-field provenance declaration
    neurons.parquet       one row per real neuron (canonical schema)
    connectivity.parquet  pre_idx, post_idx, anatomical_count, weight,
                          weight_provenance

Column-level provenance lives in the manifest: each canonical column is
declared with a provenance tag and source description. A field with no
real data is present as an all-null column declared UNKNOWN — the schema
keeps the *slot* so downstream code can distinguish "not applicable"
from "absent", and the fidelity manifest counts how much of each field
is actually populated.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

from .provenance import Provenance

STORE_VERSION = "canonical-anatomy-v0"

#: Canonical neuron columns → declared provenance. Importers must fill
#: exactly these columns (extra dataset-specific columns are allowed but
#: never required by downstream code).
NEURON_COLUMNS = {
    # canonical int index, contiguous 0..n-1 — the runtime node index
    "neuron_idx": Provenance.EXACT_EM,
    # namespaced dataset id, e.g. "banc:720575940614..."
    "dataset_id": Provenance.EXACT_EM,
    # soma coordinates in dataset space (nanometres where reported)
    "soma_x": Provenance.EXACT_EM,
    "soma_y": Provenance.EXACT_EM,
    "soma_z": Provenance.EXACT_EM,
    "cell_type": Provenance.EXACT_EM,
    # functional class: sensory / intrinsic / motor / ascending /
    # descending — whatever the dataset actually annotated
    "flow_class": Provenance.EXACT_EM,
    "super_class": Provenance.EXACT_EM,
    # neuropil / region membership of the soma
    "neuropil": Provenance.EXACT_EM,
    "side": Provenance.EXACT_EM,            # left | right | mid | null
    # predicted neurotransmitter (a model output — never EXACT_EM)
    "nt_top": Provenance.MODEL_INFERENCE,
    "nt_confidence": Provenance.MODEL_INFERENCE,
    # per-neuron synapse totals (EM-derived counts)
    "syn_pre_total": Provenance.EXACT_EM,
    "syn_post_total": Provenance.EXACT_EM,
    # morphology detail is a *reference*, not inlined geometry: the
    # canonical store keeps the slot so nothing pretends point-neurons
    # are all there is (AFC §6.6)
    "morphology_ref": Provenance.UNKNOWN,
    # developmental/lineage info when the dataset carries it
    "hemilineage": Provenance.UNKNOWN,
    "birth_time": Provenance.UNKNOWN,
}

CONNECTIVITY_COLUMNS = {
    "pre_idx": Provenance.EXACT_EM,
    "post_idx": Provenance.EXACT_EM,
    "anatomical_count": Provenance.EXACT_EM,
    # physiological weight: in v0 this is a count→weight model output —
    # always tagged, never passed off as measured conductance (AFC §6.5)
    "weight": Provenance.MODEL_INFERENCE,
    "weight_provenance": Provenance.EXACT_EM,  # provenance *tag* of weight
}


def manifest_template() -> dict:
    return {
        "store_version": STORE_VERSION,
        "dataset_kind": None,          # e.g. "banc_888", "flywire_v783"
        "source_files": {},            # filename -> sha256
        "n_neurons": 0,
        "n_connections": 0,
        "field_provenance": {k: v.value for k, v in NEURON_COLUMNS.items()},
        "connectivity_provenance": {k: v.value
                                    for k, v in CONNECTIVITY_COLUMNS.items()},
        "fidelity": {},                # ANATOMY_FIDELITY counters
        "notes": [],
    }


def write_store(out_dir: str | Path, neurons_df, conn_df,
                manifest: dict) -> Path:
    """Write a canonical store. ``neurons_df``/``conn_df`` are pandas
    DataFrames already carrying the canonical columns."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    neurons_df.to_parquet(out / "neurons.parquet", index=False)
    conn_df.to_parquet(out / "connectivity.parquet", index=False)
    manifest = dict(manifest)
    manifest["store_version"] = STORE_VERSION
    manifest["n_neurons"] = int(len(neurons_df))
    manifest["n_connections"] = int(len(conn_df))
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1,
                                                default=str))
    return out


def load_store(store_dir: str | Path):
    """Read a canonical store; returns (manifest, neurons_df, conn_df).
    Validates the version and declares-missing-vs-present sanity."""
    import pandas as pd
    d = Path(store_dir)
    manifest = json.loads((d / "manifest.json").read_text())
    if manifest.get("store_version") != STORE_VERSION:
        raise ValueError(f"{d}: unsupported store_version "
                         f"{manifest.get('store_version')!r}")
    return manifest, pd.read_parquet(d / "neurons.parquet"), \
        pd.read_parquet(d / "connectivity.parquet")


def sha256_file(path: str | Path, _bs=1 << 22) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(_bs), b""):
            h.update(chunk)
    return h.hexdigest()
