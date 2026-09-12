"""Deterministic replay: same seed + same actions -> identical trajectories."""

import numpy as np

from experiments.p0.env import dynamics as dyn
from experiments.p0.env.attention_env import AttentionEnv


def rollout(seed, actions):
    env = AttentionEnv(dyn.EnvParams(), seed=seed)
    obs = env.reset()
    obs_list = [obs.copy()]
    rewards, infos = [], []
    for a in actions:
        obs, r, done, info = env.step(int(a))
        obs_list.append(obs.copy())
        rewards.append(r)
        infos.append((info["d_phase"], info["detected"], info["missed"]))
        if done:
            break
    return np.array(obs_list), np.array(rewards), infos


def test_same_seed_same_actions_identical():
    rng = np.random.default_rng(42)
    actions = rng.integers(0, dyn.N_ACTIONS, size=300)
    o1, r1, i1 = rollout(0, actions)
    o2, r2, i2 = rollout(0, actions)
    np.testing.assert_array_equal(o1, o2)
    np.testing.assert_array_equal(r1, r2)
    assert i1 == i2


def test_different_seed_differs():
    rng = np.random.default_rng(42)
    actions = rng.integers(0, dyn.N_ACTIONS, size=300)
    o1, _, _ = rollout(0, actions)
    o2, _, _ = rollout(1, actions)
    n = min(len(o1), len(o2))
    assert not np.allclose(o1[:n], o2[:n])


def test_reseed_reproduces():
    env = AttentionEnv(dyn.EnvParams(), seed=0)
    env.reset()
    for _ in range(50):
        if env.step(1)[2]:
            break
    env.reseed(0)
    env2 = AttentionEnv(dyn.EnvParams(), seed=0)
    env2.reset()
    np.testing.assert_array_equal(env._obs(), env2._obs())
