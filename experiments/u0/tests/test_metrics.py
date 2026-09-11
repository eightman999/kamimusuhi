"""U0 calibration-metric tests on synthetic data with known answers."""

import numpy as np
import pytest

from experiments.u0.analysis import metrics as met


def test_ece_perfect_calibration_is_zero():
    rng = np.random.default_rng(0)
    conf = rng.uniform(0, 1, 20000)
    correct = (rng.uniform(0, 1, 20000) < conf).astype(float)
    assert met.ece(conf, correct, n_bins=10) < 0.02


def test_ece_miscalibrated_known_value():
    # conf all 0.9, accuracy all 0.5 -> ECE = 0.4
    conf = np.full(1000, 0.9)
    correct = np.tile([1.0, 0.0], 500)
    assert met.ece(conf, correct, n_bins=10) == pytest.approx(0.4)


def test_nll_and_brier_handcheck():
    probs = np.array([[0.8, 0.2], [0.4, 0.6]])
    y = np.array([0, 1])
    nll = -(np.log(0.8) + np.log(0.6)) / 2
    assert met.nll(probs, y) == pytest.approx(nll)
    brier = (((0.8 - 1) ** 2 + 0.2 ** 2) +
             ((0.4 ** 2) + (0.6 - 1) ** 2)) / 2
    assert met.brier(probs, y) == pytest.approx(brier)


def test_risk_coverage_perfect_confidence():
    conf = np.array([0.9, 0.8, 0.7, 0.6, 0.5])
    correct = np.array([1.0, 1.0, 0.0, 1.0, 0.0])
    rc = met.risk_coverage(conf, correct, n_points=6)
    # the most confident 40% are all correct -> risk 0
    assert rc["risk"][1] == pytest.approx(0.0)
    # monotone non-decreasing-ish as coverage grows past the errors
    assert rc["risk"][-1] == pytest.approx(1 - 3 / 5)


def test_selective_accuracy():
    conf = np.array([0.9, 0.1, 0.8, 0.2])
    correct = np.array([1.0, 0.0, 1.0, 0.0])
    assert met.selective_accuracy(conf, correct, 0.5) == pytest.approx(1.0)


def test_auroc_perfect_and_chance():
    conf = np.array([0.9, 0.8, 0.3, 0.2])
    correct = np.array([1.0, 1.0, 0.0, 0.0])
    assert met.error_detection_auroc(conf, correct) == pytest.approx(1.0)
    rng = np.random.default_rng(1)
    conf = rng.uniform(size=4000)
    correct = (rng.uniform(size=4000) < 0.5).astype(float)
    assert abs(met.error_detection_auroc(conf, correct) - 0.5) < 0.05


def test_episode_metrics_keys():
    recs = [
        {"outcome": "answer", "reward": 1.0, "n_obs": 1, "y": 0,
         "probs": [0.9, 0.05, 0.05, 0.0], "conf": 0.9, "correct": True,
         "observe_cost": 0.0, "action_counts": [1, 0, 0, 0, 0, 0],
         "regime": "easy"},
        {"outcome": "abstain", "reward": 0.05, "n_obs": 1, "y": 2,
         "probs": [0.25] * 4, "conf": 0.4, "correct": False,
         "observe_cost": 0.0, "action_counts": [0, 0, 0, 0, 1, 0],
         "regime": "ambiguous"},
    ]
    m = met.episode_metrics(recs)
    assert m["episodes"] == 2
    assert m["answer_rate"] == 0.5
    assert m["abstain_rate"] == 0.5
    assert m["accuracy"] == 1.0
    assert m["ece_conf"] >= 0
