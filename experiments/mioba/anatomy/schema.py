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
#: A0.1 (directive §1): entity taxonomy + 3-layer separation +
#: compartment-labelled connectivity. v0/v1 stores stay loadable.
STORE_VERSION_V2 = "canonical-anatomy-v2"
SUPPORTED_STORE_VERSIONS = {STORE_VERSION, STORE_VERSION_V2}

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


# ---------------------------------------------------------------- A0.1 v2
#: What a row in ``entities.parquet`` *is* (directive §2). The union of
#: meta + edgelist ids is a set of anatomical entities, not a neuron
#: count — classification decides which ones are biological neurons.
ENTITY_CLASSES = [
    "BIOLOGICAL_NEURON",     # proofread neuron
    "ROUGH_NEURON",          # roughly-proofread / annotated but unproofread
    "ORPHAN_FRAGMENT",       # status flags: unrooted / too small / tracing issue
    "GLIA",
    "TRACHEA",
    "OTHER_CELL",            # not_a_neuron etc.
    "UNKNOWN_SEGMENT",       # edgelist-only or unclassified
]

#: Compartment vocabulary (directive §14) — mirrors the BANC split labels.
COMPARTMENTS = ["SOMA", "PRIMARY_NEURITE", "DENDRITE", "AXON", "UNKNOWN"]

#: entities.parquet column provenance (directive §10 — no blanket
#: EXACT_EM: ids are segmentation output, coordinates are EM-derived,
#: annotations are curated, NT is a model output).
ENTITY_COLUMNS_V2 = {
    "entity_idx": Provenance.PROOFREAD_SEGMENTATION,
    "dataset_id": Provenance.PROOFREAD_SEGMENTATION,
    "entity_class": Provenance.CURATED_ANNOTATION,
    "annotation_conflict": Provenance.CURATED_ANNOTATION,
    "soma_x": Provenance.RAW_EM_DERIVED,
    "soma_y": Provenance.RAW_EM_DERIVED,
    "soma_z": Provenance.RAW_EM_DERIVED,
    "cell_type": Provenance.CURATED_ANNOTATION,
    "cell_class": Provenance.CURATED_ANNOTATION,
    "flow_class": Provenance.CURATED_ANNOTATION,
    "super_class": Provenance.CURATED_ANNOTATION,
    "neuropil": Provenance.CURATED_ANNOTATION,
    "side": Provenance.CURATED_ANNOTATION,
    "hemilineage": Provenance.CURATED_ANNOTATION,
    "proofread_status": Provenance.PROOFREAD_SEGMENTATION,
    "nt_top": Provenance.MODEL_INFERENCE,
    "nt_confidence": Provenance.MODEL_INFERENCE,
    "syn_pre_total": Provenance.RAW_EM_DERIVED,
    "syn_post_total": Provenance.RAW_EM_DERIVED,
    # morphology slots — never silently absent (§12)
    "morphology_status": Provenance.RAW_EM_DERIVED,   # AVAILABLE|UNKNOWN
    "morphology_ref": Provenance.RAW_EM_DERIVED,
    "morphology_version": Provenance.RAW_EM_DERIVED,
    "morphology_validation": Provenance.MODEL_INFERENCE,
}

#: connectivity.parquet (runtime layer, Layer C) — one row per
#: aggregated directed pair. Compartments are the pair's *dominant*
#: split labels; the full decomposition stays in connections_split.
CONNECTIVITY_COLUMNS_V2 = {
    "pre_idx": Provenance.PROOFREAD_SEGMENTATION,
    "post_idx": Provenance.PROOFREAD_SEGMENTATION,
    "anatomical_count": Provenance.RAW_EM_DERIVED,
    "weight": Provenance.MODEL_INFERENCE,
    "weight_provenance": Provenance.CURATED_ANNOTATION,
    "pre_compartment": Provenance.MODEL_INFERENCE,
    "post_compartment": Provenance.MODEL_INFERENCE,
}

#: connections_split.parquet (Layer A): the per-compartment-combo rows
#: exactly as the upstream split edgelist publishes them.
SPLIT_COLUMNS_V2 = {
    "pre_idx": Provenance.PROOFREAD_SEGMENTATION,
    "post_idx": Provenance.PROOFREAD_SEGMENTATION,
    "pre_compartment": Provenance.CURATED_ANNOTATION,
    "post_compartment": Provenance.CURATED_ANNOTATION,
    "anatomical_count": Provenance.RAW_EM_DERIVED,
}


def manifest_template_v2() -> dict:
    return {
        "store_version": STORE_VERSION_V2,
        "schema_version": 2,
        "dataset": {                    # strict identity (§6/§7)
            "kind": None,               # e.g. "banc_888"
            "materialization": None,    # e.g. 888
            "synapse_version": None,    # e.g. "edgelist_split_v3"
            "morphology_version": None,
        },
        "layers": {                     # 3-layer separation (§5)
            "A_raw": ["entities.parquet", "connections_split.parquet",
                      "morphology_nodes.parquet", "duplicate_rows.jsonl"],
            "B_curated": ["entities.parquet"],
            "C_runtime": ["connectivity.parquet", "reduction.parquet"],
        },
        "source_files": {},
        "n_entities": 0,
        "n_runtime_edges": 0,
        "entity_class_counts": {},
        "field_provenance": {k: v.value
                             for k, v in ENTITY_COLUMNS_V2.items()},
        "connectivity_provenance": {k: v.value
                                    for k, v in CONNECTIVITY_COLUMNS_V2.items()},
        "split_provenance": {k: v.value
                             for k, v in SPLIT_COLUMNS_V2.items()},
        "fidelity": {},
        "notes": [],
    }


def write_store_v2(out_dir: str | Path, entities_df, conn_df,
                   manifest: dict, split_df=None, morph_nodes_df=None,
                   reduction_df=None) -> Path:
    """Write a canonical v2 store. Raw-layer tables are optional so a
    store can be built incrementally (morphology ingested later)."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    entities_df.to_parquet(out / "entities.parquet", index=False)
    conn_df.to_parquet(out / "connectivity.parquet", index=False)
    if split_df is not None:
        split_df.to_parquet(out / "connections_split.parquet",
                            index=False)
    if morph_nodes_df is not None:
        morph_nodes_df.to_parquet(out / "morphology_nodes.parquet",
                                  index=False)
    if reduction_df is not None:
        reduction_df.to_parquet(out / "reduction.parquet", index=False)
    manifest = dict(manifest)
    manifest["store_version"] = STORE_VERSION_V2
    manifest["schema_version"] = 2
    manifest["n_entities"] = int(len(entities_df))
    manifest["n_runtime_edges"] = int(len(conn_df))
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1,
                                                default=str))
    return out


def load_store_v2(store_dir: str | Path):
    """Read a v2 store; returns (manifest, entities_df, conn_df).
    ``split``/``morphology``/``reduction`` tables load on demand via
    :func:`load_layer`."""
    import pandas as pd
    d = Path(store_dir)
    manifest = json.loads((d / "manifest.json").read_text())
    if manifest.get("store_version") != STORE_VERSION_V2:
        raise ValueError(f"{d}: not a v2 store "
                         f"({manifest.get('store_version')!r})")
    return manifest, pd.read_parquet(d / "entities.parquet"), \
        pd.read_parquet(d / "connectivity.parquet")


def load_layer(store_dir: str | Path, name: str):
    """Load an optional v2 layer table; ``None`` when absent (absence is
    itself meaningful — the field is UNKNOWN, not empty)."""
    import pandas as pd
    p = Path(store_dir) / f"{name}.parquet"
    return pd.read_parquet(p) if p.is_file() else None


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
    """Read a canonical store; returns (manifest, entities_df, conn_df).

    Accepts both v0/v1 stores (``neurons.parquet``) and v2 stores
    (``entities.parquet``). For v2 the canonical index column is
    ``entity_idx``; a ``neuron_idx`` alias is added so v1-era consumers
    keep working during the migration window. The entity table itself
    stays honest: it is a set of anatomical entities, not a neuron
    count (§2)."""
    import pandas as pd
    d = Path(store_dir)
    manifest = json.loads((d / "manifest.json").read_text())
    ver = manifest.get("store_version")
    if ver == STORE_VERSION_V2:
        ent = pd.read_parquet(d / "entities.parquet")
        if "neuron_idx" not in ent.columns:
            ent = ent.assign(neuron_idx=ent["entity_idx"])
        return manifest, ent, pd.read_parquet(d / "connectivity.parquet")
    if ver != STORE_VERSION:
        raise ValueError(f"{d}: unsupported store_version {ver!r}")
    return manifest, pd.read_parquet(d / "neurons.parquet"), \
        pd.read_parquet(d / "connectivity.parquet")


def sha256_file(path: str | Path, _bs=1 << 22) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(_bs), b""):
            h.update(chunk)
    return h.hexdigest()
