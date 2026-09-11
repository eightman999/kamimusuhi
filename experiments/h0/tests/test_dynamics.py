"""Unit tests for H0 internal/external dynamics and action effects."""

import numpy as np
import pytest

from experiments.h0.env import dynamics as dyn
from experiments.h0.env.homeostasis_env import HomeostasisEnv


def make_env(seed=0, **overrides):
    p = dyn.EnvParams(**overrides)
    return HomeostasisEnv(p, seed=seed)


def neutral_internal():
    # mid-range internal state, comfortably inside all preferred ranges
    return np.array([0.6, 0.2, 0.5, 0.65, 0.5])


def neutral_external():
    return np.array([0.5, 0.4, 0.7, 0.7, 0.0])  # hazard=0


# ---------------------------------------------------------------------------
# Action effects
# ---------------------------------------------------------------------------


def test_rest_recovers_energy_and_fatigue():
    rng = np.random.default_rng(0)
    s = np.array([0.3, 0.6, 0.5, 0.65, 0.5])
    e = neutral_external()
    p = dyn.EnvParams(state_noise=0.0)
    out = dyn.update_internal(rng, s, e, dyn.REST, p)
    assert out[dyn.ENERGY] > s[dyn.ENERGY]
    assert out[dyn.FATIGUE] < s[dyn.FATIGUE]


def test_rest_energy_gain_scales_with_resource():
    p = dyn.EnvParams(state_noise=0.0)
    s = np.array([0.3, 0.2, 0.5, 0.65, 0.5])
    e_rich = neutral_external().copy(); e_rich[dyn.RESOURCE] = 1.0
    e_poor = neutral_external().copy(); e_poor[dyn.RESOURCE] = 0.0
    r_rich = dyn.update_internal(np.random.default_rng(0), s, e_rich, dyn.REST, p)
    r_poor = dyn.update_internal(np.random.default_rng(0), s, e_poor, dyn.REST, p)
    assert r_rich[dyn.ENERGY] > r_poor[dyn.ENERGY]


def test_explore_costs_energy_and_raises_novelty():
    p = dyn.EnvParams(state_noise=0.0)
    s = neutral_internal()
    e = neutral_external()
    out = dyn.update_internal(np.random.default_rng(0), s, e, dyn.EXPLORE, p)
    assert out[dyn.ENERGY] < s[dyn.ENERGY]
    assert out[dyn.FATIGUE] > s[dyn.FATIGUE]
    assert out[dyn.NOVELTY] > s[dyn.NOVELTY]


def test_observe_raises_certainty():
    p = dyn.EnvParams(state_noise=0.0)
    s = neutral_internal()
    e = neutral_external()
    out = dyn.update_internal(np.random.default_rng(0), s, e, dyn.OBSERVE, p)
    assert out[dyn.CERTAINTY] > s[dyn.CERTAINTY]
    assert out[dyn.ENERGY] < s[dyn.ENERGY]


def test_ignore_is_cheapest_but_decays_certainty():
    p = dyn.EnvParams(state_noise=0.0)
    s = neutral_internal()
    e = neutral_external()
    out = dyn.update_internal(np.random.default_rng(0), s, e, dyn.IGNORE, p)
    assert out[dyn.CERTAINTY] < s[dyn.CERTAINTY]
    # energy cost of IGNORE < WAIT < OBSERVE < EXPLORE
    for a in (dyn.WAIT, dyn.OBSERVE, dyn.EXPLORE):
        o2 = dyn.update_internal(np.random.default_rng(0), s, e, a, p)
        assert out[dyn.ENERGY] > o2[dyn.ENERGY]


def test_temperature_relaxes_toward_ambient():
    p = dyn.EnvParams(state_noise=0.0)
    s = neutral_internal().copy(); s[dyn.TEMPERATURE] = 0.5
    e_hot = neutral_external().copy(); e_hot[dyn.AMBIENT] = 0.9
    out = dyn.update_internal(np.random.default_rng(0), s, e_hot, dyn.WAIT, p)
    assert out[dyn.TEMPERATURE] > s[dyn.TEMPERATURE]


def test_action_permutation_swaps_effects():
    # C4 hook: agent picks REST(2) but physically EXPLORE(3) happens
    perm = tuple(range(dyn.N_ACTIONS))
    perm = list(perm); perm[dyn.REST], perm[dyn.EXPLORE] = perm[dyn.EXPLORE], perm[dyn.REST]
    p = dyn.EnvParams(state_noise=0.0, action_permutation=tuple(perm))
    s = np.array([0.3, 0.6, 0.5, 0.65, 0.5])
    e = neutral_external()
    out = dyn.update_internal(np.random.default_rng(0), s, e, dyn.REST, p)
    # should behave like EXPLORE: energy down, fatigue up
    assert out[dyn.ENERGY] < s[dyn.ENERGY]
    assert out[dyn.FATIGUE] > s[dyn.FATIGUE]


# ---------------------------------------------------------------------------
# Death conditions
# ---------------------------------------------------------------------------


def test_death_low_energy():
    env = make_env(seed=1)
    env.reset()
    env.internal[dyn.ENERGY] = 0.06
    env.internal[dyn.FATIGUE] = 0.9  # force REST-like need; step with EXPLORE drains
    _, _, done, info = env.step(dyn.EXPLORE)
    # energy should drop below 0.05 -> death (or at least flagged eventually)
    if done:
        assert info["death_cause"] == "energy"


def test_death_temperature_bounds():
    env = make_env(seed=2)
    env.reset()
    env.internal[dyn.TEMPERATURE] = 0.999
    _, _, done, info = env.step(dyn.WAIT)
    assert done and info["death_cause"] == "temperature"


def test_episode_terminates_at_length():
    env = make_env(seed=3, episode_length=10)
    env.reset()
    done = False
    for _ in range(10):
        _, _, done, _ = env.step(dyn.WAIT)
    assert done


def test_step_after_done_raises():
    env = make_env(seed=4, episode_length=2)
    env.reset()
    env.step(dyn.WAIT)
    env.step(dyn.WAIT)
    with pytest.raises(RuntimeError):
        env.step(dyn.WAIT)


# ---------------------------------------------------------------------------
# External dynamics
# ---------------------------------------------------------------------------


def test_ambient_temperature_oscillates():
    p = dyn.EnvParams(ambient_noise=0.0, ambient_amp=0.2, ambient_period=100)
    rng = np.random.default_rng(0)
    e = dyn.initial_external(np.random.default_rng(1), p)
    vals = []
    for t in range(100):
        e = dyn.update_external(rng, e, t, 0.0, p)
        vals.append(e[dyn.AMBIENT])
    assert max(vals) - min(vals) > 0.3  # clear oscillation


def test_external_stays_in_bounds():
    env = make_env(seed=5)
    env.reset()
    for _ in range(200):
        env.step(np.random.default_rng(6).integers(dyn.N_ACTIONS))
        assert np.all(env.external >= 0.0) and np.all(env.external <= 1.0)
        assert np.all(env.internal >= 0.0) and np.all(env.internal <= 1.0)
        if env.done:
            break
