"""Tests for the causal-ablation machinery (spec section 13)."""

import numpy as np

from experiments.h0.env import dynamics as dyn
from experiments.h0.env.homeostasis_env import HomeostasisEnv
from experiments.h0.evaluate import run_episode
from experiments.h0.agents.heuristic import HeuristicAgent


def test_shuffle_changes_observation():
    obs = np.arange(dyn.OBS_DIM, dtype=np.float64)
    donor = np.full(dyn.N_INTERNAL, 0.5)
    out = HomeostasisEnv.shuffle_internal(obs, donor)
    np.testing.assert_array_equal(out[: dyn.N_INTERNAL], donor)
    np.testing.assert_array_equal(out[dyn.N_INTERNAL :], obs[dyn.N_INTERNAL :])


def test_mask_fixes_channel():
    obs = np.arange(dyn.OBS_DIM, dtype=np.float64)
    out = HomeostasisEnv.mask_internal(obs, 0, 0.5)
    assert out[0] == 0.5
    np.testing.assert_array_equal(out[1:], obs[1:])


def test_permute_reorders_channels():
    obs = np.arange(dyn.OBS_DIM, dtype=np.float64)
    perm = np.arange(dyn.OBS_DIM)[::-1]
    out = HomeostasisEnv.permute_obs(obs, perm)
    np.testing.assert_array_equal(out, obs[::-1])


def test_action_permutation_changes_dynamics():
    """C4: swapping REST<->EXPLORE effects must change the outcome."""
    perm = list(range(dyn.N_ACTIONS))
    perm[dyn.REST], perm[dyn.EXPLORE] = dyn.EXPLORE, dyn.REST
    p_norm = dyn.EnvParams(state_noise=0.0)
    p_perm = dyn.EnvParams(state_noise=0.0, action_permutation=tuple(perm))
    s = np.array([0.3, 0.6, 0.5, 0.65, 0.5])
    e = np.array([0.5, 0.4, 0.7, 0.7, 0.0])
    r_norm = dyn.update_internal(np.random.default_rng(0), s, e, dyn.REST, p_norm)
    r_perm = dyn.update_internal(np.random.default_rng(0), s, e, dyn.REST, p_perm)
    # under permutation, choosing REST produces the EXPLORE effect
    assert r_norm[dyn.ENERGY] > r_perm[dyn.ENERGY]


def test_ablated_episode_still_runs():
    env = HomeostasisEnv(dyn.EnvParams(episode_length=64), seed=0)
    rng = np.random.default_rng(0)
    donor = np.random.default_rng(1).random((50, dyn.N_INTERNAL))
    for ab in (
        {"type": "shuffle"},
        {"type": "mask", "index": 0, "value": 0.5},
        {"type": "permute", "permutation": list(reversed(range(dyn.OBS_DIM)))},
        {"type": "hidden_reset", "at": 0.5},
    ):
        env.reseed(0)
        m = run_episode(env, HeuristicAgent(), rng, ablation=ab,
                        donor_pool=donor, collect_trace=False)
        assert "homeostatic_error" in m
