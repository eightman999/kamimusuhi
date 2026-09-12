"""Deterministic generation: same seed -> identical episodes."""

import numpy as np

from experiments.o0.env import dynamics as dyn
from experiments.o0.env.occlusion_env import OcclusionEnv


def test_same_seed_identical_batch():
    p = dyn.EnvParams()
    d1 = dyn.generate_batch(p, 32, np.random.default_rng(7))
    d2 = dyn.generate_batch(p, 32, np.random.default_rng(7))
    for k in d1:
        np.testing.assert_array_equal(d1[k], d2[k])


def test_different_seed_differs():
    p = dyn.EnvParams()
    d1 = dyn.generate_batch(p, 32, np.random.default_rng(7))
    d2 = dyn.generate_batch(p, 32, np.random.default_rng(8))
    assert not np.allclose(d1["obs"], d2["obs"])


def test_env_replay_deterministic():
    p = dyn.EnvParams()
    obs_seq = []
    for _ in range(2):
        env = OcclusionEnv(p, seed=3)
        obs = env.reset()
        seq = [obs]
        done = False
        while not done:
            obs, r, done, info = env.step(0)
            seq.append(obs)
            assert r == 0.0
        obs_seq.append(np.array(seq))
    np.testing.assert_array_equal(obs_seq[0], obs_seq[1])


def test_reseed_reproduces():
    p = dyn.EnvParams()
    env = OcclusionEnv(p, seed=11)
    env.reset()
    for _ in range(10):
        env.step(0)
    env.reseed(11)
    env2 = OcclusionEnv(p, seed=11)
    env2.reset()
    np.testing.assert_array_equal(env.episode["obs"], env2.episode["obs"])
