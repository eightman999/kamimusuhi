"""A3.1 §6-§12, §27-§31: ``reduce-v3-ais-fly`` runtime compiler.

Deltas vs reduce-v2-biophysical (which stays frozen):

- **AIS**: entities carrying an AXON compartment gain a
  runtime-derived ``AXON_INITIATION_ZONE`` node inserted between
  SOMA and AXON in the coupling chain (§7-§9). Location is
  ``soma_axon_boundary`` — MODEL_INFERENCE, never claimed measured.
- **Spike emission moves to the AIS** (§10): pre side of every edge
  is the AIS node when present, else SOMA. The axonal delay stays a
  deterministic relay (§11 allowed); per-edge morphology-derived
  delay is future work (recorded as gap).
- **Receptor-aware sign** (§27-§31): edge effect resolves
  presynaptic NT × postsynaptic receptor profile; unresolved →
  UNKNOWN_SIGN handled by ``unknown_sign_mode`` and audited.

Returns ``(n, post, pre, w, esyn, coup, node_map_rows, manifest,
edge_audit, physiology)`` — same shape as v3.
"""
from __future__ import annotations

import numpy as np
import pandas as pd

from experiments.mioba.anatomy.provenance import Provenance
from experiments.mioba.physio.compiler import (
    E_EXC_MV, E_INH_MV, _POST_TO_RUNTIME, NT_CLASS)
from experiments.mioba.physio.geometry import axial_coupling
from experiments.mioba.physio.receptors import (
    RECEPTOR_MODEL_VERSION, UNKNOWN_SIGN_MODES, resolve_sign)

REDUCTION_VERSION_V3 = "reduce-v3-ais-fly"
AIS = "AXON_INITIATION_ZONE"
# coupling adjacency (v3 chain + AIS between soma and axon)
_CHAIN = ("AXON", AIS, "SOMA", "DENDRITE_PROX", "DENDRITE_DIST")


def compile_reduced_graph_v4(
        split_df: pd.DataFrame,
        reductions: dict,
        entity_params: dict[int, dict],
        receptor_maps: dict[int, dict],
        nt_map: dict[int, str] | None = None,
        mapping_mode: str = "permissive",
        weight_div: float = 32.0,
        unknown_sign_mode: str = "neutral",
        coupling_c: float = 1.0,
        geometries: dict[int, dict] | None = None,
        ra_ohmcm: float = 100.0,
        ais_density_boost: float = 3.0,
        weight_to_g: float = 0.02,
        tau_syn: float = 5.0,
        drive_g: float = 5.0,
        dt: float = 0.1,
        temperature_C: float = 25.0):
    if mapping_mode not in ("permissive", "strict"):
        raise ValueError(f"mapping_mode {mapping_mode!r}")
    if unknown_sign_mode not in UNKNOWN_SIGN_MODES:
        raise ValueError(f"unknown_sign_mode {unknown_sign_mode!r}")

    node_of, node_map_rows = {}, []
    ais_rows = []

    def comp_list(ei):
        r = reductions.get(ei)
        return r.compartments if r else ["SOMA"]

    def node(ei: int, comp: str) -> int:
        key = (ei, comp)
        if key not in node_of:
            node_of[key] = len(node_of)
            node_map_rows.append(
                {"runtime_idx": node_of[key], "entity_idx": ei,
                 "compartment": comp})
        return node_of[key]

    entities = sorted({*split_df["pre_idx"], *split_df["post_idx"],
                       *reductions.keys()})
    for ei in entities:
        cs = comp_list(ei)
        for c in cs:
            node(ei, c)
        # §7-9: AIS inserted iff the entity carries an AXON
        # compartment — runtime-derived, MODEL_INFERENCE position
        if "AXON" in cs:
            node(ei, AIS)
            ais_rows.append({
                "entity_idx": ei,
                "ais_present": True,
                "ais_location": "soma_axon_boundary",
                "ais_source": "runtime_derived",
                "ais_mapping_rule": "entity has AXON compartment -> "
                                    "AIS node inserted between SOMA "
                                    "and AXON",
                "ais_provenance": Provenance.MODEL_INFERENCE,
            })

    # ---- intra-entity coupling chain with AIS -----------------------
    crow, ccol, cval = [], [], []
    coupling_model = "scalar MODEL_INFERENCE coupling_c"
    for ei in entities:
        cs = [c for c in _CHAIN if c in comp_list(ei)
              or (c == AIS and (ei, AIS) in node_of)]
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

    # ---- split-synapse edges, receptor-aware sign (§2/§27-31) -------
    post_l, pre_l, w_l, esyn_l = [], [], [], []
    audit = {k: [] for k in
             ("pre_entity", "post_entity", "pre_compartment",
              "post_compartment", "anatomical_count", "runtime_weight",
              "weight_provenance", "fallback_used", "fallback_reason",
              "mapping_provenance", "nt", "receptor", "sign",
              "sign_provenance")}
    n_exact = n_unknown = n_fallback = n_skipped = 0
    n_sign_unknown = n_sign_prior = n_sign_resolved = n_dropped = 0
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
        sign, sprov, rec = resolve_sign(
            nt, receptor_maps.get(qe) or {}, unknown_sign_mode)
        if sign is None:
            n_dropped += 1
            continue
        if sprov == Provenance.UNKNOWN.value \
                or sprov == Provenance.UNKNOWN:
            n_sign_unknown += 1
        elif rec == "nt_prior_no_receptor":
            n_sign_prior += 1
        else:
            n_sign_resolved += 1
        esyn = E_EXC_MV if sign == "exc" else E_INH_MV
        # §10-11: emission node is AIS when present, else SOMA
        emitter = AIS if (pe, AIS) in node_of else "SOMA"
        wv = float(r["anatomical_count"]) / weight_div
        post_l.append(node_of[(qe, tgt)])
        pre_l.append(node_of[(pe, emitter)])
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
                      ("nt", nt or "UNKNOWN"), ("receptor", rec),
                      ("sign", sign), ("sign_provenance", sprov)):
            audit[k].append(v_)

    phys, _act = _fly_physiology_block(
        node_map_rows, entity_params, ais_density_boost, weight_to_g,
        tau_syn, drive_g, dt, temperature_C, unknown_sign_mode)
    man_rows = [reductions[e].manifest_row() for e in sorted(reductions)]
    total = int(len(split_df))
    manifest = {
        "algorithm": REDUCTION_VERSION_V3,
        "input_layer": "connections_split",
        "mapping_mode": mapping_mode,
        "weight_rule": f"weight = anatomical_count / {weight_div}",
        "weight_provenance": "MODEL_INFERENCE",
        "compartment_reduction": "reduce-v1 chain + AIS node "
                                 "(runtime-derived)",
        "spike_emission": "AIS when present else SOMA "
                          "(MODEL_INFERENCE); axonal output remains a "
                          "deterministic delayed relay (§11-12)",
        "delay_model": {"model": "delayed_spike_relay",
                        "provenance": "MODEL_INFERENCE",
                        "note": "uniform tDelay; per-edge "
                                "morphology-derived delay not yet "
                                "implemented"},
        "ais": {"count": len(ais_rows), "rows": ais_rows,
                "density_boost": ais_density_boost,
                "density_provenance": "MODEL_INFERENCE"},
        "coupling_model": coupling_model,
        "coupling_provenance": "MODEL_INFERENCE",
        "synapse_effect": {
            "receptor_model": RECEPTOR_MODEL_VERSION,
            "unknown_sign_mode": unknown_sign_mode,
            "synapses_total": total,
            "synapses_emitted": len(w_l),
            "synapses_receptor_resolved": n_sign_resolved,
            "synapses_unknown_sign": n_sign_unknown,
            "synapses_using_prior": n_sign_prior,
            "synapses_dropped_strict": n_dropped,
        },
        "split_edges_total": total,
        "split_edges_exact_compartment": n_exact,
        "split_edges_unknown_compartment": n_unknown,
        "split_edges_fallback_to_soma": n_fallback,
        "split_edges_skipped_strict": n_skipped,
        "fallback_fraction": round(n_fallback / total, 6)
            if total else 0.0,
        "runtime_edges_out": len(w_l),
        "entities_expanded": len(man_rows),
        "runtime_fidelity": "L3+",
        "rows": man_rows,
    }
    return (len(node_of), np.asarray(post_l), np.asarray(pre_l),
            np.asarray(w_l), np.asarray(esyn_l),
            (np.asarray(crow), np.asarray(ccol), np.asarray(cval)),
            node_map_rows, manifest, pd.DataFrame(audit), phys)


def _fly_physiology_block(node_map_rows, entity_params,
                          ais_boost, weight_to_g, tau_syn, drive_g,
                          dt, temperature_C, unknown_sign_mode):
    """Per-node fly physiology; AIS gets boosted para_Na density
    (MODEL_INFERENCE §8: spike initiation zone enrichment)."""
    # pass 1: union of channel names + static meta (first record wins)
    ch_meta: dict[str, dict] = {}
    for ep in entity_params.values():
        for ch in ep["channels"]:
            if ch["channel"] == "leak":
                continue
            ch_meta.setdefault(
                ch["channel"],
                {"channel_model": ch["channel"], "e_rev": ch["E_rev_mV"],
                 "provenance": ch["provenance"], "source": ch["source"],
                 "temperature_C": temperature_C})
    # pass 2: aligned g_bar over every node row (0.0 where absent)
    active_idx, cm, gleak, eleak, vrest = [], [], [], [], []
    ch_gbar: dict[str, list[float]] = {n: [] for n in ch_meta}
    for row in node_map_rows:
        ei, comp = row["entity_idx"], row["compartment"]
        ep = entity_params.get(ei)
        if ep is None:
            continue
        active_idx.append(row["runtime_idx"])
        mem = ep["membrane"]
        cm.append(mem["Cm"]["value"])
        gleak.append(mem["g_leak"]["value"])
        eleak.append(mem["E_leak"]["value"])
        vrest.append(mem["V_rest"]["value"])
        by_name = {c["channel"]: c for c in ep["channels"]}
        for name in ch_meta:
            ch = by_name.get(name)
            g = 0.0
            if ch is not None and comp in ch["compartments"]:
                g = ch["g_density"]
            if comp == AIS and name.startswith("para_Na") \
                    and ch is not None:
                g = ch["g_density"] * ais_boost
            ch_gbar[name].append(g)
    channels = {n: {**ch_meta[n], "g_bar": ch_gbar[n]}
                for n in ch_meta}
    return {
        "active_idx": active_idx,
        "membrane": {"Cm": cm, "g_leak": gleak, "E_leak": eleak,
                     "V_rest": vrest},
        "channels": channels,
        "synapse": {"E_exc": E_EXC_MV, "E_inh": E_INH_MV,
                    "tau_syn": tau_syn, "weight_to_g": weight_to_g,
                    "model": "receptor_aware_v0",
                    "unknown_sign_mode": unknown_sign_mode},
        "v_spike": -20.0,
        "stability": {"v_min": -110.0, "v_max": 90.0},
        "drive": {"g_exc": drive_g},
        "integration": {"method": "exponential_euler_V+cnexp_gates",
                        "dt": dt},
        "simulation_temperature_C": temperature_C,
        "temperature_provenance": Provenance.MODEL_INFERENCE,
        # v1_1 overlays carry *_v11 channel models → matching runtime
        "runtime_mode": ("active_fly_v1_1" if any(
            n.endswith("_v11") for n in ch_meta) else
            "active_fly_v1"),
    }, active_idx
