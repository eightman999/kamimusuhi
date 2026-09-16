"""A3.2 + Body0 focused tests.

Covers: v1_1 channel registration/freeze separation, KCa-lite
provenance, active_fly_v1_1 runtime detection, F-I hardening on a
short sim, RNG-aligned zero-rate drives (no fake causal effect),
sensorimotor circuit path selection, port mapping, and the closed-loop
mechanics (encode/decode/body) on synthetic fixtures.
"""
import numpy as np
import pandas as pd
import pytest

torch = pytest.importorskip("torch")

from experiments.mioba.body.body0 import (Environment, BodyState,
                                          bearing, decode_motor,
                                          encode_sensory,
                                          pick_sensorimotor_circuit,
                                          run_episode, run_oracle,
                                          select_ports, step_body,
                                          wrap_pi)
from experiments.mioba.fba.torch_backend import TorchBackend
from experiments.mioba.physio.channels import build_channel
from experiments.mioba.physio.fly_channels import (FLY_CHANNELS,
                                                   KcaKV11, ParaNa,
                                                   ParaNaV11)
from experiments.mioba.physio.calibrate import single_node_phys
from experiments.mioba.physio.benchmark import sim_single
from experiments.mioba.physio.fly_compiler import _fly_physiology_block


# ----------------------------------------------------------- channels
def test_v11_channels_registered_and_v1_frozen():
    for n in ("para_Na_v11", "shab_K_v11", "shaker_K_v11",
              "shal_K_v11", "kca_K_v11"):
        assert n in FLY_CHANNELS
    # A3.1 classes keep their exact kinetics — freeze check
    assert (ParaNa.M_TMIN, ParaNa.H_TMIN) == (0.04, 0.4)
    assert (ParaNaV11.M_TMIN, ParaNaV11.H_TMIN) == (0.15, 1.2)
    assert type(build_channel({"channel_model": "para_Na_v11"})) \
        is ParaNaV11


def test_kca_is_model_inference_not_molecular_claim():
    k = KcaKV11()
    assert k.kinetics_provenance == "MODEL_INFERENCE"
    # gate opens only above spike range
    s = k.initial_state(torch.tensor([[-60.0, -10.0]]))
    assert s[0, 0, 0] < 0.05 < s[0, 1, 0]


def test_compiler_detects_v11_runtime():
    """_fly_physiology_block must label *_v11 channel sets as
    active_fly_v1_1 and leave v1 sets as active_fly_v1."""
    rows = [{"entity_idx": 1, "compartment": "SOMA", "runtime_idx": 0}]

    def ep(chname):
        return {1: {"membrane": {"Cm": {"value": 1.0},
                                 "g_leak": {"value": 0.1},
                                 "E_leak": {"value": -60.0},
                                 "V_rest": {"value": -60.0}},
                    "channels": [{"channel": chname, "g_density": 10.0,
                                  "E_rev_mV": 50.0,
                                  "compartments": ["SOMA"],
                                  "provenance": "LITERATURE_PRIOR",
                                  "source": "t"}]}}

    for chname, want in (("para_Na", "active_fly_v1"),
                         ("para_Na_v11", "active_fly_v1_1")):
        phys, act = _fly_physiology_block(
            rows, ep(chname), 1.0, 0.02, 5.0, 1.5, 0.1, 25.0, "neutral")
        assert phys["runtime_mode"] == want
        assert act == [0]


def test_v11_fi_is_suppressed_vs_v1():
    """A3.2 core claim on the *fitted* model: the A3.2-calibrated
    projection_neuron does not saturate at ~250 Hz the way v1 did,
    and does not depolarization-block at strong current either."""
    fitted = {"g_para": 39.19, "g_shab": 40.14, "g_shal": 42.18,
              "g_shaker": 10.21, "g_kca": 20.61, "g_leak": 0.49,
              "E_leak": -69.08, "Cm": 0.58}
    v1 = single_node_phys("projection_neuron", variant="v1")
    v11 = single_node_phys("projection_neuron", fitted=fitted,
                           variant="v1_1")
    b1 = sim_single(v1, 200.0, i_ext=60.0)
    b11 = sim_single(v11, 200.0, i_ext=60.0)
    # v1 depolarization-blocks (spike count collapses) at i=60;
    # v1_1 sustains bounded firing
    assert int(b11.spike_counts[0, 0]) > int(b1.spike_counts[0, 0])
    assert int(b11.spike_counts[0, 0]) < 40   # <200 Hz equivalent
    assert not b11._num_failure


# ------------------------------------------------------- RNG alignment
def test_zero_rate_drive_keeps_rng_stream():
    """§45 integrity: listing a node at 0 Hz must still consume its
    Poisson draw — otherwise ablating a drive silently perturbs the
    whole RNG stream and fakes a causal effect."""
    b = TorchBackend(synthetic=True, synthetic_neurons=8)
    b.initialize({"artificial_organs": [], "attachments": [],
                    "n_extra_neurons": 0, "params": {}},
                 batch_size=1, seed=3, device="cpu")
    b.set_inputs({"rates_hz": {"0": 0.0, "1": 50.0}})
    assert b._drive_idx is not None
    assert 0 in b._drive_idx.tolist()
    assert float(b._drive_p[0]) == 0.0


# ------------------------------------------------- circuit selection
def _tiny_connectome():
    """Sensory L→i1→DN1 (left) and sensory R→i2→DN2 (right)."""
    rows = [
        # entity, super_class, side
        (1, "sensory", "left"), (2, "sensory", "right"),
        (3, "central_brain_intrinsic", "left"),
        (4, "central_brain_intrinsic", "right"),
        (5, "descending", "left"), (6, "descending", "right"),
        (7, "descending", "left"), (8, "descending", "right"),
    ]
    ent = pd.DataFrame([(e, sc, s) for e, sc, s in rows],
                       columns=["entity_idx", "super_class", "side"])
    conn = pd.DataFrame(
        [(1, 3, 20), (3, 5, 15), (3, 7, 5),      # L path
         (2, 4, 20), (4, 6, 15), (4, 8, 5)],     # R path
        columns=["pre_idx", "post_idx", "anatomical_count"])
    return ent, conn


def test_sensorimotor_circuit_finds_paths():
    ent, conn = _tiny_connectome()
    p = pick_sensorimotor_circuit(ent, conn, 8, 0, depth=2, n_motor=2)
    assert 5 in p["motor_left"] and 6 in p["motor_right"]
    assert 1 in p["sensory_left"] and 2 in p["sensory_right"]
    assert set(p["targets"]) == set(p["motor_left"]) | \
        set(p["motor_right"])
    for e in p["motor_left"] + p["motor_right"] + p["sensory_left"] \
            + p["sensory_right"]:
        assert e in p["circuit"]


def test_select_ports_maps_picked_entities():
    ent, conn = _tiny_connectome()
    comp_of = {1: {"SOMA": 10}, 2: {"SOMA": 20},
               5: {"SOMA": 50, "AXON_INITIATION_ZONE": 51},
               6: {"SOMA": 60, "AXON_INITIATION_ZONE": 61}}
    picked = {"sensory_left": [1], "sensory_right": [2],
              "motor_left": [5], "motor_right": [6]}
    ports = select_ports(ent, [1, 2, 5, 6], comp_of, picked=picked)
    assert ports["sensory_left"] == [10]
    assert ports["motor_left"] == [51]   # AIS preferred over SOMA


# ---------------------------------------------------------- body loop
def test_encode_decode_body_mechanics():
    sL, sR = encode_sensory(0.5, 1.0, 5.0, 60.0)
    assert sL > sR                       # left target → left drive
    sL2, sR2 = encode_sensory(-0.5, 1.0, 5.0, 60.0)
    assert sR2 > sL2
    v_lin, v_ang = decode_motor(50.0, 10.0)
    assert v_ang > 0 and v_lin > 0       # L pop → left turn
    b = BodyState(heading=0.0)
    step_body(b, 1.0, 0.5, 0.1)
    assert b.heading > 0 and b.x > 0
    assert abs(wrap_pi(bearing(b, (1.0, 0.0)))) < np.pi


def test_oracle_orients_without_cns():
    env = Environment(target=(10.0, 0.0))
    r = run_oracle(env, 1.0, initial_heading=1.2)
    assert r["metrics"]["mean_abs_heading_err_rad_tail"] < 0.6
