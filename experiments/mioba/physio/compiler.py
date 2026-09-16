"""A3 §15: ``reduce-v2-biophysical`` runtime compiler.

Same graph construction as ``reduce-v1-split-synapse`` (split edges
are never collapsed, UNKNOWN fallbacks audited) plus:

- per-edge reversal class ``esyn`` (mV) from the *presynaptic*
  entity's neurotransmitter label (§20-21) — ``NT_ESYN`` is
  MODEL_INFERENCE, UNKNOWN transmitters follow
  ``unknown_nt_mode`` and are always audited, never silently
  excitatory;
- per-node physiology from the overlay (§6 inheritance), exposing
  ``physiology`` for the backend's ``active_hh_v0`` mode;
- optional SWC-derived axial coupling (§13-14). Without geometry the
  legacy scalar ``coupling_c`` is used and recorded as
  MODEL_INFERENCE.

Returns ``(n, post, pre, w, esyn, coup, node_map_rows, manifest,
edge_audit, physiology)``.
"""
from __future__ import annotations

import numpy as np
import pandas as pd

from experiments.mioba.anatomy.provenance import Provenance
from experiments.mioba.physio.geometry import axial_coupling

REDUCTION_VERSION_V2 = "reduce-v2-biophysical"

E_EXC_MV = 0.0
E_INH_MV = -80.0
NT_CLASS = {  # presynaptic NT → reversal class (MODEL_INFERENCE, §20)
    "acetylcholine": "exc",
    "gaba": "inh",
    "glutamate": "inh",       # Drosophila GluCl is inhibitory
}
UNKNOWN_NT_MODES = ("neutral", "generic_prior", "strict")

# canonical runtime coupling order (same chain as reduce-v0/v1)
_CHAIN = ("AXON", "SOMA", "DENDRITE_PROX", "DENDRITE_DIST")
_POST_TO_RUNTIME = {   # anatomical split label → runtime compartment
    "SOMA": "SOMA", "DENDRITE": "DENDRITE_PROX",
    "DENDRITE_PROX": "DENDRITE_PROX", "DENDRITE_DIST": "DENDRITE_DIST",
    "AXON": "AXON", "PRIMARY_NEURITE": "DENDRITE_PROX"}


def compile_reduced_graph_v3(
        split_df: pd.DataFrame,
        reductions: dict,
        entity_params: dict[int, dict],
        nt_map: dict[int, str] | None = None,
        mapping_mode: str = "permissive",
        weight_div: float = 32.0,
        unknown_nt_mode: str = "generic_prior",
        coupling_c: float = 1.0,
        geometries: dict[int, dict] | None = None,
        ra_ohmcm: float = 100.0,
        weight_to_g: float = 0.02,
        tau_syn: float = 5.0,
        drive_g: float = 5.0,
        dt: float = 0.1,
        temperature_C: float = 25.0):
    if mapping_mode not in ("permissive", "strict"):
        raise ValueError(f"mapping_mode {mapping_mode!r}")
    if unknown_nt_mode not in UNKNOWN_NT_MODES:
        raise ValueError(f"unknown_nt_mode {unknown_nt_mode!r}")

    node_of, node_map_rows = {}, []

    def node(ei: int, comp: str) -> int:
        key = (ei, comp)
        if key not in node_of:
            node_of[key] = len(node_of)
            node_map_rows.append(
                {"runtime_idx": node_of[key], "entity_idx": ei,
                 "compartment": comp})
        return node_of[key]

    def comp_list(ei):
        r = reductions.get(ei)
        return r.compartments if r else ["SOMA"]

    entities = sorted({*split_df["pre_idx"], *split_df["post_idx"],
                       *reductions.keys()})
    for ei in entities:
        for c in comp_list(ei):
            node(ei, c)

    # ---- intra-entity axial coupling (§13) --------------------------
    crow, ccol, cval = [], [], []
    coupling_model = "scalar MODEL_INFERENCE coupling_c"
    for ei in entities:
        cs = [c for c in _CHAIN if c in comp_list(ei)]
        geom = (geometries or {}).get(ei) or {}
        for a, b in zip(cs, cs[1:]):
            ga, gb = geom.get(a) or {}, geom.get(b) or {}
            if ga.get("area_um2") and gb.get("area_um2"):
                r_link = min(ga["radius_um"], gb["radius_um"]) or 0.5
                l_link = max(1e-3, 0.5 * (ga["length_um"]
                                          + gb["length_um"]))
                gi = axial_coupling(r_link, l_link, ra_ohmcm,
                                    ga["area_um2"])
                gj = axial_coupling(r_link, l_link, ra_ohmcm,
                                    gb["area_um2"])
                coupling_model = ("SWC axial coupling (r,L,Ra,area) "
                                  "— MODEL_INFERENCE reduction")
            else:
                gi = gj = coupling_c
            i, j = node_of[(ei, a)], node_of[(ei, b)]
            crow += [i, j, i, j]
            ccol += [j, i, i, j]
            cval += [gi, gj, -gi, -gj]

    # ---- split-synapse edges with reversal classes (§2/§19-21) ------
    post_l, pre_l, w_l, esyn_l = [], [], [], []
    audit = {k: [] for k in
             ("pre_entity", "post_entity", "pre_compartment",
              "post_compartment", "anatomical_count", "runtime_weight",
              "weight_provenance", "fallback_used", "fallback_reason",
              "mapping_provenance", "nt", "esyn_class",
              "esyn_provenance")}
    n_exact = n_unknown = n_fallback = n_skipped = n_nt_unknown = 0
    nt_map = nt_map or {}
    for _, r in split_df.iterrows():
        pe, qe = int(r["pre_idx"]), int(r["post_idx"])
        anat_pre = str(r.get("pre_compartment") or "UNKNOWN")
        anat_post = str(r.get("post_compartment") or "UNKNOWN")
        tgt = _POST_TO_RUNTIME.get(anat_post)
        fallback = False
        reason = ""
        if tgt is None:
            n_unknown += 1
            if mapping_mode == "strict":
                n_skipped += 1
                continue
            tgt, fallback, reason = "SOMA", True, \
                "UNKNOWN_POST_COMPARTMENT"
            n_fallback += 1
        elif tgt not in comp_list(qe):
            if mapping_mode == "strict":
                n_skipped += 1
                continue
            tgt, fallback, reason = "SOMA", True, \
                "COMPARTMENT_ABSENT"
            n_fallback += 1
        else:
            n_exact += 1
        nt = str(nt_map.get(pe) or "").lower()
        cls = NT_CLASS.get(nt)
        if cls is None:
            n_nt_unknown += 1
            if unknown_nt_mode == "strict":
                n_skipped += 1
                continue
            cls = "exc"          # generic_prior / neutral → exc, audited
        esyn = E_EXC_MV if cls == "exc" else E_INH_MV
        wv = float(r["anatomical_count"]) / weight_div
        post_l.append(node_of[(qe, tgt)])
        pre_l.append(node_of[(pe, "SOMA")])
        w_l.append(wv)
        esyn_l.append(esyn)
        for k, v_ in (("pre_entity", pe), ("post_entity", qe),
                      ("pre_compartment", anat_pre),
                      ("post_compartment", anat_post),
                      ("anatomical_count", int(r["anatomical_count"])),
                      ("runtime_weight", wv),
                      ("weight_provenance", "MODEL_INFERENCE"),
                      ("fallback_used", fallback),
                      ("fallback_reason", reason),
                      ("mapping_provenance",
                       "MODEL_INFERENCE" if fallback
                       else "RAW_EM_DERIVED"),
                      ("nt", nt or "UNKNOWN"),
                      ("esyn_class", cls),
                      ("esyn_provenance",
                       NT_PROV if NT_CLASS.get(nt)
                       else Provenance.UNKNOWN)):
            audit[k].append(v_)

    # ---- physiology block for phenotype["physiology"] ---------------
    phys, _active = _physiology_block(
        node_map_rows, entity_params, weight_to_g, tau_syn,
        drive_g, dt, temperature_C)
    man_rows = [reductions[e].manifest_row() for e in sorted(reductions)]
    total = int(len(split_df))
    manifest = {
        "algorithm": REDUCTION_VERSION_V2,
        "input_layer": "connections_split",
        "mapping_mode": mapping_mode,
        "weight_rule": f"weight = anatomical_count / {weight_div}",
        "weight_provenance": "MODEL_INFERENCE",
        "compartment_reduction": "same chain as reduce-v1; "
                                 "PRIMARY_NEURITE folded to "
                                 "DENDRITE_* (MODEL_INFERENCE)",
        "presynaptic_note": "anatomical pre_compartment is recorded; "
                            "runtime spike emission is always SOMA (§7)",
        "coupling_model": coupling_model,
        "coupling_provenance": "MODEL_INFERENCE",
        "esyn_model": {"E_exc_mV": E_EXC_MV, "E_inh_mV": E_INH_MV,
                       "nt_map_provenance": "MODEL_INFERENCE",
                       "unknown_nt_mode": unknown_nt_mode},
        "split_edges_total": total,
        "split_edges_exact_compartment": n_exact,
        "split_edges_unknown_compartment": n_unknown,
        "split_edges_fallback_to_soma": n_fallback,
        "split_edges_skipped_strict": n_skipped,
        "edges_unknown_nt": n_nt_unknown,
        "fallback_fraction": round(n_fallback / total, 6)
            if total else 0.0,
        "runtime_edges_out": len(w_l),
        "entities_expanded": len(man_rows),
        "runtime_fidelity": "L3",
        "rows": man_rows,
    }
    return (len(node_of), np.asarray(post_l), np.asarray(pre_l),
            np.asarray(w_l), np.asarray(esyn_l),
            (np.asarray(crow), np.asarray(ccol), np.asarray(cval)),
            node_map_rows, manifest, pd.DataFrame(audit), phys)


NT_PROV = Provenance.MODEL_INFERENCE


def _physiology_block(node_map_rows, entity_params, weight_to_g,
                      tau_syn, drive_g, dt, temperature_C):
    """Resolve per-runtime-node membrane/channel params.

    Every compartment node of an entity with a resolved profile is an
    *active* node (it carries the explicit membrane equation — §16
    dendrite voltage is independent state). Channel density is
    non-zero only where the profile's channel spec lists the
    compartment, so dendrites run leak+syn+axial with g_bar=0 unless
    the profile says otherwise."""
    active_idx, cm, gleak, eleak, vrest = [], [], [], [], []
    ch_gbar: dict[str, list[float]] = {}
    ch_meta: dict[str, dict] = {}
    fallback_profiles = 0
    for row in node_map_rows:
        ei, comp = row["entity_idx"], row["compartment"]
        ep = entity_params.get(ei)
        if ep is None:
            continue                     # stays passive LIF (grafts)
        if ep.get("assignment_provenance") == Provenance.GENERIC_FALLBACK:
            fallback_profiles += 1
        active_idx.append(row["runtime_idx"])
        mem = ep["membrane"]
        cm.append(mem["Cm"]["value"])
        gleak.append(mem["g_leak"]["value"])
        eleak.append(mem["E_leak"]["value"])
        vrest.append(mem["V_rest"]["value"])
        for ch in ep["channels"]:
            name = ch["channel"]
            if name == "leak":
                continue     # leak is the g_leak membrane parameter
            meta = ch_meta.setdefault(
                name, {"channel_model": name, "e_rev": ch["E_rev_mV"],
                       "provenance": ch["provenance"],
                       "source": ch["source"]})
            ch_gbar.setdefault(name, []).append(
                ch["g_density"] if comp in ch["compartments"] else 0.0)
    channels = {n: {**ch_meta[n], "g_bar": ch_gbar[n]}
                for n in ch_gbar}
    phys = {
        "active_idx": active_idx,
        "membrane": {"Cm": cm, "g_leak": gleak, "E_leak": eleak,
                     "V_rest": vrest},
        "channels": channels,
        "synapse": {"E_exc": E_EXC_MV, "E_inh": E_INH_MV,
                    "tau_syn": tau_syn, "weight_to_g": weight_to_g,
                    "model": "conductance_v0",
                    "unknown_nt_mode": "generic_prior"},
        "v_spike": -20.0,
        "stability": {"v_min": -110.0, "v_max": 90.0},
        "drive": {"g_exc": drive_g},
        "integration": {"method": "exponential_euler_V+cnexp_gates",
                        "dt": dt},
        "simulation_temperature_C": temperature_C,
        "temperature_provenance": Provenance.MODEL_INFERENCE,
        "generic_fallback_profile_nodes": fallback_profiles,
    }
    return phys, active_idx
