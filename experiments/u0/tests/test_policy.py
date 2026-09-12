"""Policy/model and intervention-probe plumbing tests."""

import numpy as np
import torch

from experiments.u0.env.u0_env import OBS_DIM, U0Config
from experiments.u0.evaluate import (NEED_CONDITIONS, PolicyWrapper,
                                     need_intervention_probe)
from experiments.u0.models.nets import build_policy
from experiments.u0.policies.baselines import BASELINES


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
