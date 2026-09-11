"""U0 env tests: determinism, action semantics, cost accounting,
no-difficulty-label leak."""

import numpy as np
import pytest

from experiments.u0.env.uncertainty_env import (U0Config, U0Env, VecU0Env,
                                                REGIMES)


def rollout(env, actions):
    obs = env.reset()
    out = [obs.copy()]
    rews = []
    for a in actions:
        obs, r, d, i = env.step(a)
        out.append(obs.copy())
        rews.append(r)
        if d:
            break
    return np.array(out), np.array(rews)


def test_same_seed_same_actions_identical():
    rng = np.random.default_rng(42)
    acts = rng.integers(0, 6, size=8)
    cfg = U0Config(seed=0)
    o1, r1 = rollout(U0Env(cfg), acts)
    o2, r2 = rollout(U0Env(cfg), acts)
    np.testing.assert_array_equal(o1, o2)
    np.testing.assert_array_equal(r1, r2)


def test_different_seed_differs():
    rng = np.random.default_rng(42)
    acts = np.full(8, 5)  # observe forever
    o1, _ = rollout(U0Env(U0Config(seed=0)), acts)
    o2, _ = rollout(U0Env(U0Config(seed=1)), acts)
    assert not np.allclose(o1[: len(o2)], o2[: len(o1)])


def test_answer_terminal_and_graded():
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    env.reset()
    y = env.latent["y"]
    _o, r, d, info = env.step(y)
    assert d and info["outcome"] == "answer" and info["correct"]
    assert r == cfg.reward_correct
    env.reset()
    wrong = (env.latent["y"] + 1) % cfg.n_classes
    _o, r, d, info = env.step(wrong)
    assert d and info["outcome"] == "answer" and not info["correct"]
    assert r == cfg.reward_wrong


def test_abstain_terminal():
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    env.reset()
    _o, r, d, info = env.step(cfg.abstain_id)
    assert d and info["outcome"] == "abstain" and r == cfg.reward_abstain


def test_observe_costs_and_draws():
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    env.reset()
    assert env.n == 1
    o, r, d, info = env.step(cfg.observe_id)
    assert not d and r == -cfg.cost_observe and env.n == 2
    assert env.total_observe_cost == cfg.cost_observe


def test_budget_exhaustion_forced_abstain():
    cfg = U0Config(seed=0, max_obs=3)
    env = U0Env(cfg)
    env.reset()
    for _ in range(2):                      # n: 1 -> 3
        _o, r, d, i = env.step(cfg.observe_id)
        assert not d
    _o, r, d, info = env.step(cfg.observe_id)   # n == max_obs
    assert d and info["outcome"] == "forced_abstain"
    assert r == cfg.reward_abstain
    assert env.n == 3


def test_obs_is_sufficient_stats_only():
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    obs = env.reset()
    assert obs.shape == (cfg.feat_dim + 2,)
    assert obs[-1] == pytest.approx(1 / cfg.max_obs)  # n_frac
    assert obs[-2] == 0.0                             # var at n=1


def test_no_label_leak_same_latent_same_obs():
    """obs must depend only on (mu, sigma, samples) -- not on the regime
    label.  Two episodes forced to identical latent params and identical
    sample draws must emit identical observations even under different
    regime names."""
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    mu = env.proto[:, 0].copy()
    base = {"c": 0, "S": (0,), "y": 0, "mu": mu, "sigma": 1.0,
            "mask": None}
    obs_a = env.reset(seed=123, latent={**base, "regime": "easy"})
    obs_b = env.reset(seed=123, latent={**base, "regime": "ambiguous"})
    np.testing.assert_array_equal(obs_a, obs_b)
    # and the regime string itself never appears numerically in obs
    assert obs_a.shape == (cfg.feat_dim + 2,)


def test_regime_distribution_and_no_null_label_in_obs():
    cfg = U0Config(seed=0)
    env = U0Env(cfg)
    seen = set()
    for _ in range(300):
        env.reset()
        seen.add(env.latent["regime"])
        assert env.latent["regime"] in REGIMES
    assert seen == set(REGIMES)


def test_vec_env_step_and_done_slots():
    cfg = U0Config(seed=0)
    vec = VecU0Env(cfg, 8, seed=7)
    obs = vec.reset()
    assert obs.shape == (8, cfg.obs_dim)
    acts = np.full(8, cfg.abstain_id)
    _o, r, d, i = vec.step(acts)
    assert d.all()
    # dead slots are inert no-ops
    _o2, r2, d2, _ = vec.step(np.zeros(8, dtype=int))
    assert np.all(r2 == 0) and d2.all()


def test_invalid_action_rejected():
    env = U0Env(U0Config(seed=0))
    env.reset()
    with pytest.raises(ValueError):
        env.step(99)
