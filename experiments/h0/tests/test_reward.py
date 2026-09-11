"""Reward-shaping tests: reward must be -homeostatic_error only."""

import numpy as np

from experiments.h0.env import dynamics as dyn
from experiments.h0.env.homeostasis_env import HomeostasisEnv


def test_reward_is_negative_error():
    env = HomeostasisEnv(dyn.EnvParams(), seed=0)
    env.reset()
    _, reward, done, info = env.step(dyn.WAIT)
    if not done:
        assert reward == -info["homeostatic_error"]


def test_error_zero_inside_ranges():
    s = np.array([0.6, 0.2, 0.5, 0.65, 0.5])
    assert dyn.homeostatic_error(s) == 0.0
    assert dyn.stable_mask(s)


def test_error_grows_with_deviation():
    near = np.array([0.35, 0.2, 0.5, 0.65, 0.5])   # energy slightly low
    far = np.array([0.10, 0.2, 0.5, 0.65, 0.5])   # energy very low
    assert dyn.homeostatic_error(far) > dyn.homeostatic_error(near) > 0.0


def test_no_action_specific_reward():
    """Same resulting state -> same reward regardless of action taken."""
    env = HomeostasisEnv(dyn.EnvParams(state_noise=0.0), seed=0)
    env.reset()
    # Force identical post-state by setting internal before step and using
    # actions whose net effect we then overwrite: instead verify reward only
    # depends on info['homeostatic_error'] across different actions.
    rewards = {}
    for a in range(dyn.N_ACTIONS):
        env2 = HomeostasisEnv(dyn.EnvParams(state_noise=0.0), seed=7)
        env2.reset()
        _, r, done, info = env2.step(a)
        rewards[a] = (r, info["homeostatic_error"], done)
    for a, (r, e, done) in rewards.items():
        if not done:
            assert abs(r - (-e)) < 1e-9


def test_death_adds_penalty():
    env = HomeostasisEnv(dyn.EnvParams(death_penalty=2.0), seed=0)
    env.reset()
    env.internal[dyn.TEMPERATURE] = 0.99
    _, reward, done, info = env.step(dyn.WAIT)
    assert done
    assert reward == -info["homeostatic_error"] - 2.0
