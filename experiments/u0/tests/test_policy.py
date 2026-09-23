"""Policy/model and intervention-probe plumbing tests."""

import numpy as np
import pytest
import torch

from experiments.u0.config import resolve_device
from experiments.u0.env.u0_env import OBS_DIM, PAYLOAD_DIM, U0Config
from experiments.u0.evaluate import (NEED_CONDITIONS, PolicyWrapper,
                                     need_intervention_probe)
from experiments.u0.models.nets import build_policy
from experiments.u0.policies.baselines import BASELINES


def test_resolve_device():
    assert resolve_device("auto") == "cpu"      # CPU-first, never MPS
    assert resolve_device("cpu") == "cpu"
    if torch.backends.mps.is_available():
        assert resolve_device("mps") == "mps"
    else:
        assert resolve_device("mps") == "cpu"   # graceful fallback
    with pytest.raises(ValueError):
        resolve_device("cuda")


def test_model_shapes():
    for name in ("mlp", "gru64", "gru128"):
        m = build_policy(name, OBS_DIM)
        h = m.initial_state(3, torch.device("cpu"))
        obs = torch.zeros(3, OBS_DIM)
        logits, v, h2 = m(obs, h)
        assert logits.shape == (3, 6)
        assert v.shape == (3,)
        if m.recurrent:
            assert h2.shape == h.shape


def test_wrapper_reset_recovers():
    m = build_policy("gru64")
    w = PolicyWrapper(m)
    obs = np.zeros(OBS_DIM, dtype=np.float32)
    a1 = w.act(obs)
    w.reset()
    a2 = w.act(obs)
    assert 0 <= a1 < 6 and 0 <= a2 < 6


def test_baselines_run_episode():
    from experiments.u0.env.u0_env import U0Env
    for name, cls in BASELINES.items():
        env = U0Env(U0Config(seed=0, episode_len=128))
        env.reset()
        gate = cls(seed=0)
        gate.reset()
        for _ in range(80):
            if env.done:
                break
            a = gate.decide(env)
            assert 0 <= a < 6
            env.step(a)


def test_oracle_stores_only_needed_potent_sites():
    from experiments.u0.env.u0_env import STORE, U0Env
    env = U0Env(U0Config(seed=4))
    env.reset()
    gate = BASELINES["oracle"](seed=0)
    gate.reset()
    for _ in range(60):
        a = gate.decide(env)
        env.step(a)
    needed_funcs = {env._func_of_var(v) for v, _ in env.need_plan}
    stored_funcs = set(env.memory.funcs[env.memory.occupied].tolist())
    assert stored_funcs <= needed_funcs
    assert all(p >= env.cfg.potency_rich
               for p in env.memory.potencies[env.memory.occupied])


def test_probe_returns_deltas():
    m = build_policy("gru64")
    cfg = U0Config(seed=0, episode_len=128, delay_min=16, delay_max=24)
    out = need_intervention_probe(m, cfg, episodes=4, seed=910001)
    assert set(out["conditions"]) == set(NEED_CONDITIONS)
    assert set(out["delta_by_func"]) == {"resource", "shelter",
                                        "safe_zone", "obs_point"}
    assert 0.0 <= out["mean_abs_delta"] <= 1.0


def test_no_memory_baseline_never_stores():
    """no_memory is the lower bound: zero STORE/RECALL actions."""
    from experiments.u0.env.u0_env import U0Env
    env = U0Env(U0Config(seed=0, episode_len=128))
    env.reset()
    gate = BASELINES["no_memory"](seed=0)
    gate.reset()
    while not env.done:
        env.step(gate.decide(env))
    st = env.episode_stats()
    assert st["stores"] == 0 and st["recalls"] == 0


def test_oracle_separates_from_no_memory():
    """Protocol sanity (small n): oracle must resolve far more needs
    than the memoryless policy — the environment's causal core."""
    from experiments.u0.evaluate import evaluate_baseline
    cfg = U0Config(seed=0, episode_len=128, delay_min=16, delay_max=32,
                   needs_per_episode=1)
    o = evaluate_baseline("oracle", cfg, episodes=48, seed=12345)
    n = evaluate_baseline("no_memory", cfg, episodes=48, seed=12345)
    assert o["need_resolution"] > n["need_resolution"] + 0.3
    assert o["crisis_error_auc"] < n["crisis_error_auc"]


def test_targeted_erase_removes_relevant_memory():
    """targeted_erase at onset removes only the need-serving items."""
    from experiments.u0.env.u0_env import U0Env
    from experiments.u0.evaluate import _apply_causal
    env = U0Env(U0Config(seed=0, needs_per_episode=1,
                         delay_min=4, delay_max=4))
    env.reset()
    n = env.needs[0]
    func = env._func_of_var(n.var)
    # store the relevant site plus an unrelated one
    site = next(s for s in env.sites if s["func"] == func)
    other = next(s for s in env.sites if s["func"] != func)
    for s in (site, other):
        env.memory.store(np.zeros(PAYLOAD_DIM, np.float32),
                         s["func"], s["loc"], s["potency"])
    while env.t < n.onset:
        env.step(0)                     # IGNORE until onset step
    _apply_causal(env, None, "targeted_erase")
    funcs = env.memory.funcs[env.memory.occupied]
    assert func not in funcs.tolist()
    assert other["func"] in funcs.tolist()
