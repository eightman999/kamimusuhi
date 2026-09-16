"""A3.1 focused tests — Drosophila physiological calibration.

Covers: fly channel independence + Q10, generic A3 channels kept,
receptor-aware sign (Glu strictness, UNKNOWN policies), AIS node
insertion/emission, active_fly_v1 single-cell behaviour, channel
ablation, gate-bound guard, and backward compatibility of
active_hh_v0/passive_lif.
"""
import numpy as np
import pandas as pd
import pytest

torch = pytest.importorskip("torch")

from experiments.mioba.anatomy.compartments import EntityReduction
from experiments.mioba.anatomy.provenance import Provenance
from experiments.mioba.fba.torch_backend import TorchBackend
from experiments.mioba.physio.channels import GenericNa, build_channel
from experiments.mioba.physio.fly_channels import (FLY_CHANNELS,
                                                   ParaNa, ShabK)
from experiments.mioba.physio.receptors import (receptor_map_for,
                                               resolve_sign)
from experiments.mioba.physio.fly_compiler import (
    AIS, compile_reduced_graph_v4)
from experiments.mioba.physio.fly_overlay import (
    FLY_PROFILES, fly_profile_for)
from experiments.mioba.physio.calibrate import (TARGETS,
                                                single_node_phys)
from experiments.mioba.physio.benchmark import sim_single


# --------------------------------------------------------------- channels
def test_fly_channels_independent_not_renamed_generic():
    """§4: para_Na must not be a renamed generic_Na."""
    para = build_channel({"channel_model": "para_Na", "e_rev": 50.0})
    gen = build_channel({"channel_model": "generic_Na", "e_rev": 50.0})
    assert type(para) is not type(gen)
    V = torch.tensor([[-60.0, -30.0, 0.0]])
    s_p, s_g = para.initial_state(V), gen.initial_state(V)
    # different steady-state gating at the same voltages
    assert not torch.allclose(s_p, s_g)
    # independent temperature handling (§19): Para accepts it,
    # generic does not
    para22 = build_channel({"channel_model": "para_Na",
                            "e_rev": 50.0, "temperature_C": 22.0})
    para32 = build_channel({"channel_model": "para_Na",
                            "e_rev": 50.0, "temperature_C": 32.0})
    st22 = para22.initial_state(V)
    assert torch.allclose(
        st22, para32.initial_state(V))           # same steady state
    # Q10 changes kinetics — must advance from a non-equilibrium state
    off = torch.zeros_like(st22)
    a22 = para22.advance(0.1, V, off)
    a32 = para32.advance(0.1, V, off)
    assert not torch.allclose(a22, a32)          # faster at 32 °C
    assert (a32 >= a22).all()                    # toward x_inf > 0


def test_generic_channels_still_registered():
    """§1: A3 freeze — generic models untouched and constructible."""
    assert isinstance(build_channel(
        {"channel_model": "generic_Na", "e_rev": 50.0}), GenericNa)
    # the frozen A3.1 set must all still be present (the registry is
    # allowed to grow with later versioned sets — A3.2 *_v11/KCa)
    assert {"para_Na", "shab_K", "shaker_K", "shal_K"} \
        <= set(FLY_CHANNELS)
    for name in ("para_Na", "shab_K", "shaker_K", "shal_K"):
        assert FLY_CHANNELS[name].kinetics_provenance == \
            "LITERATURE_PRIOR"


# --------------------------------------------------------------- receptors
def test_receptor_sign_model():
    """§27-§29: sign = f(NT, receptor) — never NT alone."""
    kc = receptor_map_for("kenyon_cell")
    assert resolve_sign("acetylcholine", kc)[0] == "exc"
    assert resolve_sign("gaba", kc)[0] == "inh"
    # Glu + GluCl-like → inh on KC …
    assert resolve_sign("glutamate", kc)[0] == "inh"
    # … but motor carries an iGluR-like prior → exc (§29: Glu is not
    # hard-coded inhibitory)
    assert resolve_sign("glutamate",
                        receptor_map_for("motor"))[0] == "exc"
    # generic_fly declares no Glu receptor → UNKNOWN_SIGN
    s, prov, note = resolve_sign("glutamate",
                                 receptor_map_for("generic_fly"))
    assert prov == "UNKNOWN"


def test_unknown_sign_policies():
    gf = receptor_map_for("generic_fly")
    s, p, _ = resolve_sign("glutamate", gf, "strict")
    assert s is None                                # dropped
    s, p, _ = resolve_sign("glutamate", gf, "drop")
    assert s is None
    s, p, _ = resolve_sign("glutamate", gf, "neutral")
    assert s == "exc" and p == "UNKNOWN"            # kept + flagged
    s, p, _ = resolve_sign("glutamate", gf, "prior")
    assert s == "exc" and p == "UNKNOWN"            # §29: no Glu prior
    # ACh *is* declared on generic_fly → resolves via the receptor,
    # not the prior. The NT prior path needs a stripped map.
    s, p, _ = resolve_sign("acetylcholine", {}, "prior")
    assert s == "exc" and p == "MODEL_INFERENCE"    # ACh has a prior


# --------------------------------------------------------------- profiles
def test_profile_families_and_fallback():
    """§13, §37-§38: coarse classes; fallback is generic_fly, never
    squid HH."""
    assert fly_profile_for(None, "sensory")[0] == "sensory"
    assert fly_profile_for("acetylcholine", "descending")[0] == \
        "descending"
    assert fly_profile_for("dopamine", "central")[0] == "modulatory"
    pid, prov = fly_profile_for("acetylcholine", "optic")
    assert pid == "generic_fly" and prov == "GENERIC_FALLBACK"
    required = {"sensory", "projection_neuron", "kenyon_cell",
                "central_complex", "descending", "motor",
                "modulatory", "generic_fly"}
    assert required <= set(FLY_PROFILES)
    # every active profile carries Para + Shab (§3 minimum)
    for pid in required:
        chs = FLY_PROFILES[pid]["channels"]
        assert "para_Na" in chs and "shab_K" in chs


# --------------------------------------------------------------- compiler
def _toy_circuit():
    """3 entities, each with SOMA+DENDRITE_DIST+AXON compartments."""
    reductions = {e: EntityReduction(
        entity_idx=e, original_nodes=10, node_map={}, valid=True,
        compartments=["SOMA", "PRIMARY_NEURITE", "AXON",
                      "DENDRITE_PROX", "DENDRITE_DIST"])
        for e in (0, 1, 2)}
    split = pd.DataFrame({
        "pre_idx": [0, 1, 0], "post_idx": [1, 2, 2],
        "pre_compartment": ["AXON", "AXON", "AXON"],
        "post_compartment": ["DENDRITE_DIST", "SOMA", "UNKNOWN"],
        "anatomical_count": [10, 5, 3]})
    ep = {e: {"profile_id": "kenyon_cell",
              "assignment_provenance": "MODEL_INFERENCE",
              "membrane": {k: {"value": v} for k, v in
                           FLY_PROFILES["kenyon_cell"]["membrane"]
                           .items()},
              "channels": [{"channel": n, "g_density": c["g"],
                            "E_rev_mV": c["e"], "compartments":
                            c["comps"], "provenance":
                            "MODEL_INFERENCE", "source": "t",
                            "confidence": 0.3}
                           for n, c in
                           FLY_PROFILES["kenyon_cell"]["channels"]
                           .items()]}
          for e in (0, 1, 2)}
    rec = {e: receptor_map_for("kenyon_cell") for e in (0, 1, 2)}
    return split, reductions, ep, rec


def test_v4_ais_insertion_and_emission():
    """§7-§10: AIS nodes inserted; edges emit from AIS not SOMA."""
    split, red, ep, rec = _toy_circuit()
    nt = {0: "acetylcholine", 1: "gaba", 2: "glutamate"}
    n, post, pre, w, esyn, coup, rows, man, audit, phys = \
        compile_reduced_graph_v4(split, red, ep, rec, nt_map=nt)
    ais_idx = {r["runtime_idx"] for r in rows
               if r["compartment"] == AIS}
    assert len(ais_idx) == 3                        # one per entity
    assert man["ais"]["count"] == 3
    assert all(r["ais_provenance"] == Provenance.MODEL_INFERENCE
               for r in man["ais"]["rows"])
    soma_idx = {r["runtime_idx"]: r["entity_idx"] for r in rows
                if r["compartment"] == "SOMA"}
    # every edge's pre must be an AIS node (emission moved off SOMA)
    assert set(pre.tolist()) <= ais_idx
    assert not (set(pre.tolist()) & set(soma_idx))
    assert phys["runtime_mode"] == "active_fly_v1"
    # channel g_bar aligned to active-node count
    na = len(phys["active_idx"])
    for spec in phys["channels"].values():
        assert len(spec["g_bar"]) == na


def test_v4_receptor_audit_counts():
    """§51-§52: manifest audit fields exist and are consistent."""
    split, red, ep, rec = _toy_circuit()
    # nt_map is keyed on the PRESYNAPTIC entity — edges are
    # 0→1 (ACh), 1→2 (unknown_nt), 0→2 (ACh)
    nt = {0: "acetylcholine", 1: "unknown_nt"}
    *_, man, audit, phys = compile_reduced_graph_v4(
        split, red, ep, rec, nt_map=nt,
        unknown_sign_mode="neutral")
    syn = man["synapse_effect"]
    for k in ("synapses_total", "synapses_receptor_resolved",
              "synapses_unknown_sign", "synapses_using_prior",
              "synapses_dropped_strict"):
        assert k in syn
    assert syn["synapses_total"] == 3
    assert syn["synapses_receptor_resolved"] == 2
    assert syn["synapses_unknown_sign"] == 1


def test_v4_strict_drops_unknown_sign():
    split, red, ep, rec = _toy_circuit()
    nt = {0: "acetylcholine", 1: "unknown_nt"}
    n, post, pre, w, esyn, coup, rows, man, audit, phys = \
        compile_reduced_graph_v4(split, red, ep, rec, nt_map=nt,
                                 unknown_sign_mode="strict")
    assert man["synapse_effect"]["synapses_dropped_strict"] == 1
    assert len(w) == 2


# --------------------------------------------------------------- runtime
def test_fly_single_cell_rest_and_spike():
    """§39, §56: quiet at rest, fires on injection, repolarises."""
    phys = single_node_phys("projection_neuron")
    b = sim_single(phys, 120.0, trace=True)
    assert int(b.spike_counts[0, 0]) == 0
    assert b._num_failure is None
    b = sim_single(phys, 120.0, i_ext=40.0, trace=True)
    assert int(b.spike_counts[0, 0]) >= 1
    v = np.asarray(b.get_trace()[0]["v"])
    assert float(v[np.argmax(v):].min()) < -50.0   # repolarises


def test_para_ablation_abolishes_spikes():
    """§25-§26, §56: Para off → spike generation strongly reduced."""
    base = single_node_phys("motor")
    # 25 µA/cm²: suprathreshold with Para, but below the passive
    # leak-driven crossing so an off-para cell can't fake a "spike"
    full = sim_single(base, 120.0, i_ext=25.0)
    assert int(full.spike_counts[0, 0]) >= 1
    import json
    phys = json.loads(json.dumps(base))
    phys["channels"]["para_Na"]["g_bar"] = [0.0]
    off = sim_single(phys, 120.0, i_ext=25.0)
    assert int(off.spike_counts[0, 0]) == 0


def test_fly_deterministic():
    ph = single_node_phys("kenyon_cell")
    a = sim_single(ph, 80.0, i_ext=30.0, seed=7)
    b = sim_single(ph, 80.0, i_ext=30.0, seed=7)
    assert torch.equal(a.spike_counts, b.spike_counts)
    assert torch.equal(a.v, b.v)


def test_gate_bound_guard_flags():
    """§55: an impossible channel config surfaces as
    NUMERICAL_FAILURE, never silently clipped."""
    phys = single_node_phys("motor")
    phys["channels"]["para_Na"]["g_bar"] = [1e7]   # absurd density
    phys["stability"]["v_max"] = 90.0
    b = sim_single(phys, 60.0, i_ext=40.0)
    assert b._num_failure is not None


def test_passive_and_generic_modes_unchanged():
    """§1/§53: active_hh_v0 + passive_lif still work side by side."""
    post = np.array([0]); pre = np.array([0])
    w = np.array([0.0]); esyn = np.array([0.0])
    for mode, ph in (("passive_lif", None),
                     ("active_hh_v0", {
                         "runtime_mode": "active_hh_v0",
                         "active_idx": [0],
                         "membrane": {"Cm": [1.0], "g_leak": [0.3],
                                      "E_leak": [-60.0],
                                      "V_rest": [-60.0]},
                         "channels": {
                             "na": {"channel_model": "generic_Na",
                                    "e_rev": 50.0, "g_bar": [60.0]},
                             "kd": {"channel_model": "generic_Kdr",
                                    "e_rev": -77.0, "g_bar": [18.0]}},
                         "synapse": {"E_exc": 0.0, "E_inh": -80.0,
                                     "tau_syn": 5.0,
                                     "weight_to_g": 0.5},
                         "v_spike": -20.0,
                         "stability": {"v_min": -110.0,
                                       "v_max": 90.0}})):
        b = TorchBackend(
            synthetic=False,
            base_override=(1, post, pre, w, esyn), runtime_mode=mode)
        b.initialize(phenotype={"physiology": ph} if ph else {},
                     batch_size=1, seed=0, device="cpu")
        b.run(50.0)
        assert b._num_failure is None


def test_calibration_targets_schema():
    """§16: targets carry the required metadata columns."""
    for pid, t in TARGETS.items():
        for k in ("stage", "sex", "temperature_C", "method",
                  "location", "source", "metrics"):
            assert k in t, (pid, k)
        assert t["stage"] == "adult"
    # ≥3 calibratable profiles (§60)
    assert len(TARGETS) >= 3
    # cross-stage flag present on the motor table (§17)
    assert TARGETS["motor"].get("cross_stage_prior")
