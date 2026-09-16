"""Morphology ingestion + validation (A2, directive §11–§13).

Parses upstream SWC skeletons into the canonical store's
``morphology_nodes.parquet`` and records per-entity validation status.
Nothing is repaired silently: a broken skeleton keeps its data and gets
a ``morphology_validation`` flag; any repair would be a separate,
provenance-tagged step.

SWC columns: ``id type x y z radius parent``. The BANC pcg-skel
skeletons use ``type=0`` throughout (no compartment labels); the
per-cell ``*_split.swc`` files carry axon/dendrite/primary-neurite/soma
labels (2/3/4/7) — ingestion records whichever the file has.
"""
from __future__ import annotations

import math
import numpy as np
import pandas as pd
from pathlib import Path

from .provenance import Provenance

#: SWC type code → canonical compartment (§14)
SWC_TYPE_TO_COMPARTMENT = {
    1: "SOMA", 7: "SOMA",
    2: "AXON",
    3: "DENDRITE",
    4: "PRIMARY_NEURITE",
    0: "UNKNOWN", 5: "UNKNOWN", 6: "UNKNOWN",
}


def parse_swc(path: str | Path) -> np.ndarray:
    """SWC → (n,7) float array [id, type, x, y, z, r, parent]."""
    a = np.loadtxt(path, comments="#", dtype=np.float64)
    if a.ndim == 1:
        a = a[None, :]
    if a.shape[1] < 7:
        raise ValueError(f"{path}: {a.shape[1]} columns, expected 7")
    return a[:, :7]


def validate_swc(nodes: np.ndarray) -> dict:
    """§13 integrity checks — report only, never repair."""
    issues = []
    ids = nodes[:, 0]
    if len(ids) != len(set(ids.tolist())):
        issues.append("duplicate_node_ids")
    idset = set(ids.tolist())
    roots = int((nodes[:, 6] < 0).sum())
    if roots != 1:
        issues.append(f"roots:{roots}")
    if np.any(~np.isin(nodes[:, 6], list(idset) + [-1.0])):
        issues.append("broken_parent_refs")
    # connectivity: nodes reachable downward from a root
    children = {}
    for i, p in zip(ids, nodes[:, 6]):
        children.setdefault(int(p), []).append(int(i))
    seen, stack = set(), [int(i) for i in ids[nodes[:, 6] < 0]]
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        stack.extend(children.get(cur, ()))
    if len(seen) != len(ids):
        issues.append(f"disconnected:{len(ids) - len(seen)}")
    if np.isnan(nodes[:, 2:6]).any():
        issues.append("nan_coordinates")
    if np.any(np.abs(nodes[:, 2:5]) > 1e8):
        issues.append("impossible_coordinates")
    d = nodes[:, 2:5]
    pl = {int(i): j for i, j in zip(ids, range(len(ids)))}
    dz = np.array([np.linalg.norm(d[pl[int(i)]] - d[pl[p]])
                   for i, p in zip(ids, nodes[:, 6])
                   if p >= 0 and int(i) in pl and int(p) in pl])
    if len(dz) and float((dz == 0).mean()) > 0.2:
        issues.append("many_zero_length_edges")
    return {"status": "VALID" if not issues else "INVALID",
            "issues": issues,
            "n_nodes": int(len(nodes)),
            "n_roots": roots}


def skeleton_stats(nodes: np.ndarray) -> dict:
    """Cable length (µm if coords are µm), branch points, soma flag."""
    ids = nodes[:, 0].astype(np.int64)
    pos = nodes[:, 2:5]
    pl = {int(i): j for j, i in enumerate(ids)}
    length = float(sum(
        np.linalg.norm(pos[pl[int(i)]] - pos[pl[int(p)]])
        for i, p in zip(ids, nodes[:, 6].astype(np.int64))
        if p >= 0 and int(p) in pl))
    child_count = pd.Series(
        nodes[nodes[:, 6] >= 0, 6].astype(np.int64)).value_counts()
    types = nodes[:, 1].astype(int)
    comps = {SWC_TYPE_TO_COMPARTMENT.get(int(t), "UNKNOWN")
             for t in np.unique(types)}
    return {"cable_length": length,
            "n_branch_points": int((child_count >= 2).sum()),
            "has_soma": bool((types == 1).any() or (types == 7).any()),
            "compartments_present": sorted(comps)}


def ingest_skeletons(store_dir: str | Path, skeleton_dir: str | Path,
                     only_entity_ids: set[str] | None = None,
                     labeled_dir: str | Path | None = None,
                     max_nodes_per_entity: int | None = None) -> dict:
    """Ingest SWC skeletons into the v2 store.

    Writes ``morphology_nodes.parquet`` (entity_idx, node_id, type,
    compartment, x, y, z, r, parent_id) and updates
    ``morphology_validation`` in entities.parquet.

    ``only_entity_ids`` limits ingestion (dataset ids, e.g. the demo
    circuit); ``None`` ingests every skeleton found — ~115k files,
    tens of millions of nodes.
    """
    from . import schema as A
    manifest, ent, conn = A.load_store_v2(store_dir)
    rid_of_idx = {int(i): str(r) for i, r in
                  zip(ent["entity_idx"], ent["root_id"])}
    idx_of_rid = {r: i for i, r in rid_of_idx.items()}

    skel = Path(skeleton_dir)
    files = sorted(skel.glob("*.swc"))
    labeled = Path(labeled_dir) if labeled_dir else None
    rows, val_rows, n_files = [], [], 0
    for f in files:
        rid = f.stem.split("_")[0]
        if rid not in idx_of_rid:
            continue
        did = f"banc:{rid}"
        if only_entity_ids is not None and did not in only_entity_ids:
            continue
        src = f
        # a labeled split file, when downloaded, takes precedence —
        # it carries the compartment labels pcg-skel lacks
        if labeled is not None:
            cand = labeled / f"{rid}_split.swc"
            if cand.is_file():
                src = cand
        try:
            nodes = parse_swc(src)
        except Exception as exc:                     # noqa: BLE001
            val_rows.append({"root_id": rid, "status": "UNREADABLE",
                             "issues": str(exc)})
            continue
        if max_nodes_per_entity and len(nodes) > max_nodes_per_entity:
            nodes = nodes[:max_nodes_per_entity]
        v = validate_swc(nodes)
        st = skeleton_stats(nodes)
        val_rows.append({"root_id": rid, **v, **st})
        ei = idx_of_rid[rid]
        comp = [SWC_TYPE_TO_COMPARTMENT.get(int(t), "UNKNOWN")
                for t in nodes[:, 1]]
        for (nid, t, x, y, z, r, par), c in zip(nodes, comp):
            rows.append((ei, int(nid), int(t), c, float(x), float(y),
                         float(z), float(r), int(par)))
        n_files += 1

    nodes_df = pd.DataFrame(rows, columns=[
        "entity_idx", "node_id", "swc_type", "compartment",
        "x", "y", "z", "r", "parent_id"])
    val_df = pd.DataFrame(val_rows)
    status_of_rid = dict(zip(val_df["root_id"], val_df["status"]))
    ent["morphology_validation"] = [
        status_of_rid.get(str(r), None) for r in ent["root_id"]]
    out = Path(store_dir)
    nodes_df.to_parquet(out / "morphology_nodes.parquet", index=False)
    ent.to_parquet(out / "entities.parquet", index=False)
    val_df.to_parquet(out / "morphology_validation.parquet", index=False)
    return {"files_ingested": n_files, "nodes": len(nodes_df),
            "validated": len(val_df),
            "valid": int((val_df["status"] == "VALID").sum())}


def fetch_split_swcs(root_ids: list[str], out_dir: str | Path) -> int:
    """Download the compartment-labelled ``*_split.swc`` for the given
    BANC root_ids (per-file public URLs; small subsets only — there is
    no bulk archive)."""
    import urllib.request
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    base = ("https://storage.googleapis.com/"
            "lee-lab_brain-and-nerve-cord-fly-connectome/"
            "compiled_data/banc_888/banc_banc_space_split_swc")
    n = 0
    for rid in root_ids:
        p = out / f"{rid}_split.swc"
        if p.is_file() and p.stat().st_size > 0:
            n += 1
            continue
        try:
            urllib.request.urlretrieve(f"{base}/{rid}_split.swc", p)
            n += int(p.stat().st_size > 0)
        except Exception:                            # noqa: BLE001
            p.write_text("")          # record the miss, stay honest
    return n
