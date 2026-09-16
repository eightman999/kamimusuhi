"""A3 §14: morphology-derived compartment geometry.

From the full SWC + the reduction's node_map, compute per-runtime-
compartment effective geometry: total neurite length, mean radius
(length-weighted), lateral surface area, path distance. All geometry
is RAW_EM_DERIVED; the *effective* aggregates are MODEL_INFERENCE
(the reduction itself is a model).
"""
from __future__ import annotations

import numpy as np


def compartment_geometry(nodes: np.ndarray,
                         node_map: dict[int, str]) -> dict:
    """``nodes``: (id,type,x,y,z,r,parent); ``node_map`` node→compartment.

    Returns ``{compartment: {length_um, radius_um, area_um2,
    n_swc_nodes}}`` — lengths/areas in microns (SWC units are nm in
    BANC; converted by the caller's unit convention).
    """
    ids = nodes[:, 0].astype(int)
    pos = {int(i): nodes[j, 2:5] for j, i in enumerate(ids)}
    rad = {int(i): float(nodes[j, 5]) for j, i in enumerate(ids)}
    par = {int(i): int(p) for i, p in zip(ids, nodes[:, 6])}
    acc = {}
    for i in ids:
        p = par[int(i)]
        if p < 0 or p not in pos:
            continue
        comp = node_map.get(int(i))
        if comp is None:
            continue
        seg = float(np.linalg.norm(pos[int(i)] - pos[p]))
        r = 0.5 * (rad[int(i)] + rad[p])
        a = acc.setdefault(comp, {"length": 0.0, "r_len": 0.0,
                                  "area": 0.0, "n": 0})
        a["length"] += seg
        a["r_len"] += r * seg
        a["area"] += 2.0 * np.pi * r * seg
        a["n"] += 1
    out = {}
    for comp, a in acc.items():
        L = a["length"]
        out[comp] = {
            "length_um": L / 1000.0,          # nm -> µm
            "radius_um": (a["r_len"] / L) / 1000.0 if L else 0.0,
            "area_um2": a["area"] / 1e6,      # nm² -> µm²
            "n_swc_nodes": a["n"],
        }
    return out


def axial_coupling(r_link_um: float, l_link_um: float,
                   ra_ohmcm: float, area_um2: float) -> float:
    """Axial coupling proxy between two adjacent compartments (§13).

    g_ax = (π r² / (Ra · L)) / A_post — the inter-compartment axial
    conductance normalised to the receiving compartment's area, so the
    membrane equation uses it directly as a current density
    coefficient. Units end up folded into the runtime's conductance
    scale — recorded MODEL_INFERENCE, not a measured value.
    """
    if l_link_um <= 0 or area_um2 <= 0 or r_link_um <= 0:
        return 0.0
    g_ax_s = np.pi * (r_link_um * 1e-4) ** 2 / (ra_ohmcm * l_link_um
                                              * 1e-4)   # S
    # normalise by area (cm²) -> mS/cm²-equivalent coefficient
    return float(g_ax_s / (area_um2 * 1e-8) * 1e3)
