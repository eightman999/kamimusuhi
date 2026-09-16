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


# ================================================================ A0.1 v2
#: Entity classification (directive §2) — the union of meta + edgelist
#: ids yields *anatomical entities*; which of them are biological
#: neurons is a classification decision, not an assumption.
_FRAGMENT_STATUS = ("UNROOTED", "TOO_SMALL", "TRACING_ISSUE")


def _classify_entity(super_class, status, proofread, roughly_proofread,
                     has_meta: bool) -> str:
    if not has_meta:
        return "UNKNOWN_SEGMENT"
    sc = str(super_class or "")
    st = str(status or "")
    if sc == "trachea" or "TRACHEA" in st:
        return "TRACHEA"
    if sc == "glia" or "GLIA" in st:
        return "GLIA"
    if sc == "not_a_neuron" or "NOT_A_NEURON" in st:
        return "OTHER_CELL"
    if any(f in st for f in _FRAGMENT_STATUS):
        return "ORPHAN_FRAGMENT"
    if str(proofread).upper() == "TRUE":
        return "BIOLOGICAL_NEURON"
    if str(roughly_proofread).upper() == "TRUE" or sc:
        return "ROUGH_NEURON"
    return "UNKNOWN_SEGMENT"


#: extra meta columns the v2 entity table carries (beyond _META_MAP)
_META_MAP_V2_EXTRA = {
    "cell_class": "cell_class",
    "proofread": "proofread_flag",
    "roughly_proofread": "roughly_proofread_flag",
    "status": "status",
}

#: annotation columns compared when a root_id has several meta rows
_AUDIT_COLS = ["neuropil", "flow_class", "super_class", "cell_type",
               "cell_class", "side", "soma_x", "soma_y", "soma_z"]


def _class_from_label(lbl: str) -> str:
    return {"axon": "AXON", "dendrite": "DENDRITE",
            "primary.dendrite": "DENDRITE",
            "primary.neurite": "PRIMARY_NEURITE"}.get(str(lbl), "UNKNOWN")


def _load_edgelist_split(cache: Path):
    """v3 *split* edgelist: one row per (pre, post, compartment combo).
    The aggregated runtime graph keeps the dominant compartment label
    per side; the full rows land in the raw Layer-A table."""
    import pyarrow.feather as feather
    df = feather.read_table(
        cache / "banc_888_edgelist_split_v3.feather").to_pandas()
    return df[["pre", "post", "pre_label", "post_label", "count"]]


def _audit_duplicates(meta: pd.DataFrame, out_dir: Path) -> pd.DataFrame:
    """A0.1 §4: duplicated root_ids are audited, not just first-won.
    Every duplicate group's rows are written to duplicate_rows.jsonl and
    the surviving canonical row is flagged annotation_conflict=True when
    the duplicates disagree on any audited annotation."""
    dup_ids = meta.loc[meta["root_id"].astype(str).duplicated(keep=False),
                       "root_id"].astype(str).unique()
    audit, conflict_of = [], {}
    for rid in sorted(dup_ids):
        rows = meta[meta["root_id"].astype(str) == rid]
        diffs = {c: sorted({str(v) for v in rows[c]})
                 for c in _AUDIT_COLS
                 if rows[c].astype(str).nunique() > 1}
        conflict_of[rid] = bool(diffs)
        for _, r in rows.iterrows():
            audit.append({"root_id": rid, "group_rows": len(rows),
                          "conflicts": diffs,
                          "row": {c: (None if pd.isna(r[c]) else
                                      (float(r[c]) if c.startswith("soma_")
                                       else str(r[c])))
                                  for c in _AUDIT_COLS}})
    with open(out_dir / "duplicate_rows.jsonl", "w") as fh:
        for rec in audit:
            fh.write(json.dumps(rec, default=str) + "\n")
    return conflict_of


def import_banc_v2(cache_dir: str | Path | None = None,
                   out_dir: str | Path | None = None,
                   weight_map: str = "count_scaled_v0") -> Path:
    """A0.1 importer: entity taxonomy, split-edgelist compartments,
    3-layer manifest, strict dataset identity (§5–§8)."""
    t0 = time.time()
    cache = Path(cache_dir or ds.default_cache_dir())
    out = Path(out_dir or (Path(__file__).resolve().parents[1]
                           / "datasets" / "banc_888" / "store_v2"))
    out.mkdir(parents=True, exist_ok=True)

    meta_raw = _load_meta(cache)
    meta_extra_src = None
    import pyarrow.feather as feather
    raw_meta = feather.read_table(cache / "banc_888_meta.feather").to_pandas()
    for src, dst in _META_MAP_V2_EXTRA.items():
        meta_raw[dst] = (raw_meta[src] if src in raw_meta.columns
                         else None)

    # §4: audit duplicates before dedupe — keep-first for the canonical
    # row, but flag conflicts and dump every duplicate row to jsonl
    conflict_of = _audit_duplicates(meta_raw, out)
    meta_raw["annotation_conflict"] = (
        meta_raw["root_id"].astype(str).map(conflict_of).fillna(False))
    n_dup_groups = len(conflict_of)
    n_conflicts = sum(conflict_of.values())
    if n_dup_groups:
        meta_raw = (meta_raw.assign(_rid=meta_raw["root_id"].astype(str))
                    .drop_duplicates("_rid", keep="first")
                    .drop(columns="_rid"))

    split = _load_edgelist_split(cache)
    meta_ids = set(meta_raw["root_id"].astype(str))
    edge_ids = set(split["pre"].astype(str)) | set(split["post"].astype(str))
    ids = np.sort(np.array(list(meta_ids | edge_ids)))
    idx_of = {rid: i for i, rid in enumerate(ids)}

    ent = (meta_raw.set_index(meta_raw["root_id"].astype(str))
           .reindex(ids).reset_index(drop=True))
    ent["root_id"] = ids
    ent["dataset_id"] = [host_id(ds.DATASET_PREFIX, r) for r in ids]
    ent.insert(0, "entity_idx", np.arange(len(ent), dtype=np.int64))
    has_meta = ent["root_id"].astype(str).isin(meta_ids)
    # entity classification (§2) — no id is silently called a neuron
    ent["entity_class"] = [
        _classify_entity(sc, st, pf, rpf, hm)
        for sc, st, pf, rpf, hm in zip(
            ent["super_class"], ent["status"], ent["proofread_flag"],
            ent["roughly_proofread_flag"], has_meta)]
    # morphology slots (§12) — skeleton availability checked on disk;
    # unlabeled pcg-skel skeletons still count as AVAILABLE geometry,
    # compartment labels come from the split labels instead
    skel_dir = cache / "skeletons" / "swcs-from-pcg-skel"
    have_skel = (set(p.stem for p in skel_dir.glob("*.swc"))
                 if skel_dir.is_dir() else set())
    ent["morphology_status"] = np.where(
        ent["root_id"].astype(str).isin(have_skel), "AVAILABLE", "UNKNOWN")
    ent["morphology_ref"] = [
        f"banc888://swcs-from-pcg-skel/{r}.swc" if r in have_skel else None
        for r in ent["root_id"].astype(str)]
    ent["morphology_version"] = np.where(
        ent["morphology_status"] == "AVAILABLE",
        "pcg-skel-2025-07", None)
    ent["morphology_validation"] = None      # filled by ingest/validate step
    ent["annotation_conflict"] = ent["annotation_conflict"].fillna(False)

    # Layer C: aggregated runtime edges + dominant compartment per side
    split["_pc"] = split["pre_label"].map(_class_from_label)
    split["_qc"] = split["post_label"].map(_class_from_label)
    grp = split.groupby(["pre", "post"], sort=False)
    conn = grp["count"].sum().reset_index()
    conn.columns = ["pre_root", "post_root", "n_syn"]
    # dominant compartment = label of the pair's largest-count row
    dom = (split.loc[grp["count"].idxmax(),
                     ["pre", "post", "_pc", "_qc"]]
           .rename(columns={"pre": "pre_root", "post": "post_root"}))
    conn = conn.merge(dom, on=["pre_root", "post_root"], how="left")
    conn["pre_idx"] = conn["pre_root"].map(idx_of).astype(np.int64)
    conn["post_idx"] = conn["post_root"].map(idx_of).astype(np.int64)
    conn = conn.rename(columns={"_pc": "pre_compartment",
                                "_qc": "post_compartment"})
    conn["anatomical_count"] = conn["n_syn"].astype(np.int64)
    conn["weight"] = (conn["n_syn"].astype(np.float64) / 32.0
                      ).astype(np.float32)
    conn["weight_provenance"] = Provenance.MODEL_INFERENCE.value
    conn = conn[["pre_idx", "post_idx", "anatomical_count", "weight",
                 "weight_provenance", "pre_compartment",
                 "post_compartment"]]

    # Layer A: split detail (compartment-resolved connection records)
    split_out = pd.DataFrame({
        "pre_idx": split["pre"].map(idx_of).astype(np.int64),
        "post_idx": split["post"].map(idx_of).astype(np.int64),
        "pre_compartment": split["_pc"],
        "post_compartment": split["_qc"],
        "anatomical_count": split["count"].astype(np.int64),
    })

    src_hashes = {}
    for name in list(ds.SOURCE_FILES) + [
            "banc_888_edgelist_split_v3.feather", "neuron_skeletons.zip"]:
        p = cache / name
        if p.is_file():
            src_hashes[name] = A.sha256_file(p)
    manifest = A.manifest_template_v2()
    manifest.update({
        "dataset": {
            "kind": ds.DATASET_KIND,
            "materialization": ds.MATERIALIZATION,
            "synapse_version": "edgelist_split_v3",
            "morphology_version": "pcg-skel-2025-07",
        },
        "source": {"paper": ds.PAPER, "dataverse": ds.DATAVERSE_DOI,
                   "base_url": ds.BASE_URL},
        "source_files": src_hashes,
        "entity_class_counts":
            ent["entity_class"].value_counts().to_dict(),
        "weight_mapping": {
            "name": weight_map,
            "rule": "weight = anatomical_count / 32.0",
            "provenance": Provenance.MODEL_INFERENCE.value,
            "note": "count→weight prior; NOT measured conductance",
        },
        "compartment_mapping": {
            "rule": "dominant split-edgelist label per aggregated pair",
            "provenance": Provenance.MODEL_INFERENCE.value,
            "raw_labels": "CURATED_ANNOTATION (upstream axon/dendrite "
                          "split)",
        },
        "importer": "experiments.mioba.importers.banc v2 (A0.1)",
        "deduplicated_root_ids": {
            "groups": n_dup_groups,
            "conflicting_groups": int(n_conflicts),
            "audit_file": "duplicate_rows.jsonl",
            "rule": "keep-first for the canonical row; conflict flagged",
        },
        "notes": [
            "entities = meta ∪ split-edgelist ids — an anatomical-entity "
            "count, not a neuron count; entity_class decides",
            "gap junctions: no whole-CNS dataset — status UNKNOWN",
            "nt_top/nt_confidence come from BANC's NT classifier — "
            "MODEL_INFERENCE, never a direct measurement",
            "per-synapse positions not ingested (v3 enriched parquet "
            "held upstream); compartment detail is at connection "
            "granularity via the split edgelist",
        ],
    })
    A.write_store_v2(out, ent, conn, manifest, split_df=split_out)
    # v2 fidelity manifest (§31)
    from ..anatomy import fidelity as _fid
    _fid.write_fidelity_v2(out, ent, conn, split_out, manifest)
    print(f"[banc-import-v2] {len(ent)} entities "
          f"({manifest['entity_class_counts']}) / {len(conn)} runtime "
          f"edges -> {out} ({time.time()-t0:.0f}s)")
    return out


def main() -> int:
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=None)
    ap.add_argument("--out", default=None)
    ap.add_argument("--v2", action="store_true",
                    help="emit the A0.1 canonical-anatomy-v2 store")
    args = ap.parse_args()
    if args.v2:
        import_banc_v2(args.cache, args.out)
    else:
        import_banc(args.cache, args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
