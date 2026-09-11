"""Sanity: heuristic baselines are beatable but not useless."""

import numpy as np

from experiments.o0.analysis import metrics as metrics_mod
from experiments.o0.env import dynamics as dyn
from experiments.o0.evaluate import evaluate_heuristic


def test_constvel_beats_lastobs_and_prior():
    p = dyn.EnvParams()
    out = {}
    for name in ("prior", "lastobs", "constvel", "oracle"):
        out[name] = evaluate_heuristic(name, p, 256, seed=4242)["metrics"]
    cv = out["constvel"]["pos_mae_occluded"]
    lo = out["lastobs"]["pos_mae_occluded"]
    pr = out["prior"]["pos_mae_occluded"]
    orc = out["oracle"]["pos_mae_occluded"]
    assert cv < lo, (cv, lo)
    assert cv < pr, (cv, pr)
    assert orc <= cv + 1e-9, (orc, cv)
    # not useless: clearly better than guessing the world centre
    assert cv < 0.15
    # but imperfect: irreducible process noise + drag keep error > 0
    assert cv > 0.005


def test_heuristics_exist_and_id_levels():
    p = dyn.EnvParams()
    out = evaluate_heuristic("constvel", p, 256, seed=777)["metrics"]
    # existence: prior-ish; id: above chance but far from perfect
    assert out["exist_acc_hidden"] > 0.5
    assert out["id_acc"] > 0.5
