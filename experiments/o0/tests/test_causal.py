"""Mechanical causal-test checks + a tiny end-to-end training run.

These tests verify the intervention machinery works; the *empirical*
hidden-reset result lives in reports/O0_RESULTS.md.
"""

import numpy as np
import pytest
import torch

from experiments.o0.env import dynamics as dyn
from experiments.o0.evaluate import (intervention_steps, predict_heuristic,
                                     predict_model)
from experiments.o0.models import make_model


def make_data(n=64, seed=0):
    p = dyn.EnvParams()
    return p, dyn.generate_batch(p, n, np.random.default_rng(seed))


def test_intervention_steps():
    p, d = make_data()
    mid = intervention_steps(d, "mid_occl")
    occluded_eps = d["t_occl"] >= 0
    assert (mid[occluded_eps] > d["t_occl"][occluded_eps]).all()
    assert (mid[~occluded_eps] == -1).all()
    start = intervention_steps(d, "occl_start")
    assert (start[occluded_eps] == d["t_occl"][occluded_eps] + 1).all()


def test_hidden_reset_changes_predictions_recurrent():
    p, d = make_data()
    torch.manual_seed(0)
    model = make_model("gru64")
    base = predict_model(model, d, {"type": "none"})
    reset = predict_model(
        model, d, {"type": "hidden_reset", "at": "mid_occl"})
    occ = d["occluded"]
    # predictions after the reset step must differ on occluded episodes
    diff = np.abs(base["pos"] - reset["pos"])
    t = np.arange(d["obs"].shape[0])[:, None]
    after = occ & (t >= d["mid_occl"][None, :]) & (d["mid_occl"][None, :] > 0)
    assert diff[after].max() > 1e-4


def test_hidden_reset_noop_for_mlp():
    p, d = make_data()
    torch.manual_seed(0)
    model = make_model("mlp")
    base = predict_model(model, d, {"type": "none"})
    reset = predict_model(
        model, d, {"type": "hidden_reset", "at": "mid_occl"})
    np.testing.assert_array_equal(base["pos"], reset["pos"])


def test_heuristic_hidden_reset_changes_prediction():
    p, d = make_data()
    base = predict_heuristic("constvel", d, p, {"type": "none"})
    reset = predict_heuristic(
        "constvel", d, p, {"type": "hidden_reset", "at": "mid_occl"})
    occ = d["occluded"]
    t = np.arange(d["obs"].shape[0])[:, None]
    after = occ & (t > d["mid_occl"][None, :]) & (d["mid_occl"][None, :] > 0)
    assert np.abs(base["pos"] - reset["pos"])[after].max() > 1e-4


def test_tiny_training_run(tmp_path):
    """End-to-end smoke: a few updates produce artifacts, finite loss."""
    from experiments.o0.config import Config, TrainConfig
    from experiments.o0.train import train

    cfg = Config()
    cfg.name = "pytest"
    cfg.model.arch = "gru64"
    cfg.train = TrainConfig(updates=12, batch_size=16, val_every=6,
                            val_episodes=32, device="cpu")
    run_dir = train(cfg, tmp_path, "pytest_gru64_s0")
    assert (run_dir / "final.pt").exists()
    assert (run_dir / "best.pt").exists()
    status = __import__("json").loads((run_dir / "status.json").read_text())
    assert status["status"] == "complete"
    lines = (run_dir / "metrics.jsonl").read_text().strip().splitlines()
    assert len(lines) == 12
