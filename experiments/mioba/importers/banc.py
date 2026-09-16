"""BANC materialization-888 importer → canonical anatomy store (AFC A0).

Reads the public ``banc_888`` release files and emits the
dataset-agnostic store defined in ``anatomy/schema.py``. Every mapped
field carries its provenance in the manifest; fields the dataset does
not provide stay null and are declared UNKNOWN — nothing is fabricated.

Only public bulk files are used (no CAVE tokens, no live API calls), so
the import is fully offline-reproducible once the cache is populated.
"""
from __future__ import annotations

import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

from ..anatomy import fidelity as fid_mod
from ..anatomy import schema as A
from ..anatomy.provenance import Provenance
from ..datasets import banc_888 as ds
from ..graft.namespaces import host_id

#: BANC meta column → canonical column.
_META_MAP = {
    "root_id": "root_id",
    "root_position_nm": "pos_nm",
    "region": "neuropil",
    "flow": "flow_class",
    "super_class": "super_class",
    "cell_type": "cell_type",
    "side": "side",
    "hemilineage": "hemilineage",
    "neurotransmitter_predicted": "nt_top",
    "neurotransmitter_score": "nt_confidence",
    "input_connections": "syn_post_total",
    "output_connections": "syn_pre_total",
}


def _parse_pos_nm(s: str) -> tuple[float, float, float] | tuple[None, None, None]:
    """'390720, 93568, 86220' → floats; empty/NaN → (None,)*3."""
    if not isinstance(s, str) or not s.strip():
        return (None, None, None)
    try:
        x, y, z = (float(v) for v in s.split(","))
        return x, y, z
    except (ValueError, TypeError):
        return (None, None, None)


def _load_meta(cache: Path) -> pd.DataFrame:
    import pyarrow.feather as feather
    df = feather.read_table(cache / "banc_888_meta.feather").to_pandas()
    out = pd.DataFrame(index=df.index)
    for src, dst in _META_MAP.items():
        out[dst] = df[src] if src in df.columns else None
    pos = out["pos_nm"].map(_parse_pos_nm)
    out["soma_x"] = [p[0] for p in pos]
    out["soma_y"] = [p[1] for p in pos]
    out["soma_z"] = [p[2] for p in pos]
    out["dataset_id"] = [host_id(ds.DATASET_PREFIX, r) for r in out["root_id"]]
    return out


def _load_edgelist(cache: Path) -> pd.DataFrame:
    """v3 simple edgelist: per directed neuron pair the summed synapse
    count. Column names differ between releases — detect them."""
    import pyarrow.feather as feather
    df = feather.read_table(
        cache / "banc_888_edgelist_simple_v3.feather").to_pandas()
    cols = {c.lower(): c for c in df.columns}
    pre = next((cols[c] for c in ("pre", "pre_root_id", "source",
                                  "presynaptic", "pre_pt_root_id")
                if c in cols), None)
    post = next((cols[c] for c in ("post", "post_root_id", "target",
                                   "postsynaptic", "post_pt_root_id")
                 if c in cols), None)
    size = next((cols[c] for c in ("size", "syn_count", "n_syn",
                                   "synapses", "count", "weight")
                 if c in cols), None)
    if not (pre and post and size):
        raise ValueError(f"unrecognised edgelist columns: {list(df.columns)}")
    return df[[pre, post, size]].rename(
        columns={pre: "pre_root", post: "post_root", size: "n_syn"})


def import_banc(cache_dir: str | Path | None = None,
                out_dir: str | Path | None = None,
                weight_map: str = "count_scaled_v0") -> Path:
    """Build the canonical store. Returns the store directory.

    ``weight_map`` names the versioned synapse-count → runtime-weight
    mapping; v0 scales count/32 and tags every weight MODEL_INFERENCE
    (the mapping is a model prior, not a measurement — AFC §6.5).
    """
    t0 = time.time()
    cache = Path(cache_dir or ds.default_cache_dir())
    out = Path(out_dir or (Path(__file__).resolve().parents[1]
                           / "datasets" / "banc_888" / "store"))
    out.mkdir(parents=True, exist_ok=True)

    meta = _load_meta(cache)
    edges = _load_edgelist(cache)

    # BANC meta has a handful of duplicate root_ids (split/merge
    # proofreading artefacts): keep the first annotated row per id and
    # record the dedupe in the manifest rather than failing or
    # silently dropping data
    n_dup = int(meta["root_id"].astype(str).duplicated().sum())
    if n_dup:
        meta = (meta.assign(_rid=meta["root_id"].astype(str))
                .drop_duplicates("_rid", keep="first")
                .drop(columns="_rid"))

    # Canonical index assignment: deterministic order = root_id sort of
    # the union of meta and edgelist ids. Neurons present only in the
    # edgelist get a canonical row with null annotations — dropping them
    # would silently fabricate a smaller connectome.
    ids = np.sort(np.unique(np.concatenate([
        meta["root_id"].astype(str).unique(),
        edges["pre_root"].astype(str).unique(),
        edges["post_root"].astype(str).unique()])))
    idx_of = {rid: i for i, rid in enumerate(ids)}
    neurons = (meta.set_index("root_id").reindex(ids)
               .reset_index())
    # reindex preserves index name 'root_id' as a column; rows that only
    # existed in the edgelist have NaN annotations — kept, not dropped
    neurons["dataset_id"] = [host_id(ds.DATASET_PREFIX, r) for r in ids]
    neurons.insert(0, "neuron_idx", np.arange(len(neurons),
                                              dtype=np.int64))
    conn = pd.DataFrame({
        "pre_idx": edges["pre_root"].map(idx_of).astype(np.int64),
        "post_idx": edges["post_root"].map(idx_of).astype(np.int64),
        "anatomical_count": edges["n_syn"].astype(np.int64),
        # count→weight prior v0: w = count/32 — a MODEL_INFERENCE,
        # versioned here and recorded in the manifest
        "weight": (edges["n_syn"].astype(np.float64) / 32.0)
                  .astype(np.float32),
        "weight_provenance": Provenance.MODEL_INFERENCE.value,
    })

    src_hashes = {}
    for name in ds.SOURCE_FILES:
        p = cache / name
        if p.is_file():
            src_hashes[name] = A.sha256_file(p)
    manifest = A.manifest_template()
    manifest.update({
        "dataset_kind": ds.DATASET_KIND,
        "materialization": ds.MATERIALIZATION,
        "source": {"paper": ds.PAPER, "dataverse": ds.DATAVERSE_DOI,
                   "base_url": ds.BASE_URL},
        "source_files": src_hashes,
        "weight_mapping": {
            "name": weight_map,
            "rule": "weight = anatomical_count / 32.0",
            "provenance": Provenance.MODEL_INFERENCE.value,
            "note": "count→weight prior; NOT measured conductance",
        },
        "importer": "experiments.mioba.importers.banc v0",
        "deduplicated_meta_rows": n_dup,
        "notes": [
            "morphology_ref left null: L2 skeletons exist upstream but "
            "are not ingested in v0 (declared UNKNOWN, not fabricated)",
            "gap junctions: no whole-CNS dataset — status UNKNOWN",
            "nt_top/nt_confidence come from BANC's NT classifier — "
            "MODEL_INFERENCE, never a direct measurement",
        ],
    })
    A.write_store(out, neurons, conn, manifest)
    doc = fid_mod.write_fidelity(out, neurons, conn, manifest)
    print(f"[banc-import] {len(neurons)} neurons, {len(conn)} "
          f"connections -> {out} ({time.time()-t0:.0f}s)")
    c = doc["counts"]
    print(f"[banc-import] fidelity: coords={c['neurons_with_soma_coordinate']}"
          f" cell_type={c['neurons_with_cell_type']}"
          f" nt={c['neurons_with_nt_annotation']}"
          f" synapses={c['chemical_synapses_imported']}")
    return out


def main() -> int:
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=None)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    import_banc(args.cache, args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
