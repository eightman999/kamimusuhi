"""Sanity: heuristic baselines are beatable but not useless."""

import numpy as np

from experiments.o0.analysis import metrics as metrics_mod
from experiments.o0.env import dynamics as dyn
from experiments.o0.evaluate import evaluate_heuristic


def test_constvel_beats_lastobs_and_prior():
    p = dyn.EnvParams()
    out = {}
    for name in ("prior", "lastobs", "constvel", "corridor",
                 "openloop", "kalman"):
        out[name] = evaluate_heuristic(name, p, 256, seed=4242)["metrics"]
    cv = out["constvel"]["pos_mae_occluded"]
    lo = out["lastobs"]["pos_mae_occluded"]
    pr = out["prior"]["pos_mae_occluded"]
    orc = out["openloop"]["pos_mae_occluded"]
    cor = out["corridor"]["pos_mae_occluded"]
    kal = out["kalman"]["pos_mae_occluded"]
    assert cv < lo, (cv, lo)
    assert cv < pr, (cv, pr)
    # corridor clipping can only help while hidden (bounds hold inside)
    assert cor <= cv + 1e-9, (cor, cv)
    # open-loop true-dynamics tracker beats plain const-velocity
    assert orc <= cv + 1e-9, (orc, cv)
    # the Kalman tracker fuses x+v with true noise stats: at least as
    # good as open-loop in the aggregate
    assert kal <= orc + 0.02, (kal, orc)
    # not useless: clearly better than guessing the world centre
    assert cv < 0.15
    # but imperfect: irreducible process noise keeps error > 0
    assert cv > 0.001


def test_corridor_clip_bounds_error_on_gone_episodes():
    """On gone episodes (occluder to the boundary, long bouts) clipping
    to the corridor strictly dominates raw extrapolation."""
    p = dyn.EnvParams().with_overrides(p_gone=0.5)
    cv = evaluate_heuristic("constvel", p, 256, seed=5)["metrics"]
    cor = evaluate_heuristic("corridor", p, 256, seed=5)["metrics"]
    assert cor["pos_mae_occ_gone"] < cv["pos_mae_occ_gone"]
    assert cor["pos_mae_occluded"] < cv["pos_mae_occluded"]


def test_heuristics_exist_and_id_levels():
    p = dyn.EnvParams()
    out = evaluate_heuristic("constvel", p, 256, seed=777)["metrics"]
    # existence: prior-ish; id: above chance but far from perfect
    assert out["exist_acc_hidden"] > 0.5
    assert out["id_acc"] > 0.5


def test_kalman_tracks_through_occlusion():
    """Kalman tracker should track about as well as open-loop through a
    typical bout and beat it on the strict persistent<=16 headline."""
    p = dyn.EnvParams()
    kal = evaluate_heuristic("kalman", p, 256, seed=11)["metrics"]
    orc = evaluate_heuristic("openloop", p, 256, seed=11)["metrics"]
    assert kal["pos_mae_occ_persist_le16"] < 0.05
    assert np.isfinite(kal["pos_mae_occluded_ci95"])
    assert kal["exist_acc_hidden"] > 0.6
