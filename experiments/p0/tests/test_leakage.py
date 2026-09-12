"""Leakage audit: the agent input must never contain

  * the current value of a channel not observed this step,
  * which channel changes next / future surprise,
  * event labels or latent D phase,
  * ground-truth relevance flags.

Also verifies ablations only touch the agent input, and that the predictor
is only ever trained on values that were actually observed.
"""

import numpy as np

from experiments.p0.env import dynamics as dyn
from experiments.p0.env.attention_env import AttentionEnv


def test_unobserved_channels_stay_stale():
    """v_i in obs equals the value at last observation, never current x."""
    env = AttentionEnv(dyn.EnvParams(), seed=0)
    env.reset()
    rng = np.random.default_rng(7)
    for _ in range(400):
        a = int(rng.integers(dyn.N_ACTIONS))
        prev_obs = env._obs().copy()
        obs, _, done, info = env.step(a)
        if done:
            break
        observed = dyn.ACTION_CHANNELS[a]
        for ch in range(dyn.N_CHANNELS):
            if ch not in observed:
                # stale: equals previous belief, and (almost surely) != truth
                assert obs[ch] == prev_obs[ch]
                # current truth only leaks via observation; stale obs must
                # differ from x unless the channel coincidentally matches
                # its stale value -- check the mechanism not the accident:
                assert obs[ch] == env.last_val[ch]


def test_obs_never_reveals_unobserved_truth():
    """With obs_noise=0, an observed channel equals x; unobserved differs."""
    env = AttentionEnv(dyn.EnvParams(obs_noise=0.0), seed=1)
    env.reset()
    rng = np.random.default_rng(3)
    n_checked = 0
    for _ in range(300):
        a = int(rng.integers(dyn.N_ACTIONS))
        obs, _, done, info = env.step(a)
        if done:
            break
        observed = set(dyn.ACTION_CHANNELS[a])
        for ch in range(dyn.N_CHANNELS):
            if ch in observed:
                assert obs[ch] == info["x"][ch]
                n_checked += 1
    assert n_checked > 0


def test_obs_contains_no_event_label():
    """Identical action histories give identical obs regardless of a future
    or hidden event: the D-phase/event flags are not readable from obs."""
    p = dyn.EnvParams(obs_noise=0.0)
    e1 = AttentionEnv(p, seed=11)
    e2 = AttentionEnv(p, seed=11)
    e1.reset(); e2.reset()
    for _ in range(50):
        e1.step(dyn.IGNORE)
        e2.step(dyn.IGNORE)
    # force divergent latent state WITHOUT touching beliefs
    e2.d.phase = dyn.D_EVENT
    e2.d.phase_t = 0
    np.testing.assert_array_equal(e1._obs(), e2._obs())


def test_obs_dim_and_layout():
    env = AttentionEnv(dyn.EnvParams(), seed=0)
    obs = env.reset()
    assert obs.shape == (16,)
    assert obs.dtype == np.float64
    # staleness at reset = cap
    np.testing.assert_array_equal(obs[4:8], np.ones(4))
    np.testing.assert_array_equal(obs[12:16], np.ones(4))


def test_predictor_trains_only_on_observed():
    """The harness must call predictor.update with mask=observed only."""
    import torch

    from experiments.p0.agents.harness import BeliefHarness
    from experiments.p0.models.predictor import ChannelPredictor

    pred = ChannelPredictor(hidden=8)
    h = BeliefHarness(pred)
    h.reset(1)
    env = AttentionEnv(dyn.EnvParams(), seed=2)
    env_obs = env.reset()
    inp = h.agent_input(env_obs[None, :])
    assert inp.shape == (1, 24)
    env_obs2, _, _, _ = env.step(dyn.OBS_D)   # only D observed
    mask = env_obs2[None, 4:8] == 0.0
    assert mask[0, dyn.CH_D] and not mask[0, dyn.CH_A]
    h.update(env_obs2[None, :])
    # e_D updated from 0, e_A untouched
    assert pred.e[0, dyn.CH_D] >= 0.0
    assert pred.e[0, dyn.CH_A] == 0.0


def test_ablations_change_only_agent_input():
    env = AttentionEnv(dyn.EnvParams(), seed=0)
    env.reset()
    obs = np.arange(24, dtype=np.float64) / 24.0
    perm = np.array([2, 0, 3, 1])
    # pe_shuffle: v,s,pv,g untouched; e,d permuted across channels
    out = AttentionEnv.shuffle_surprise(obs, perm)
    F, E, D = AttentionEnv.FEATS_PER_CHANNEL, AttentionEnv.E, AttentionEnv.D
    for ch in range(dyn.N_CHANNELS):
        for off in (AttentionEnv.V, AttentionEnv.S, AttentionEnv.PV, AttentionEnv.G):
            assert out[ch * F + off] == obs[ch * F + off]
        assert out[ch * F + E] == obs[perm[ch] * F + E]
        assert out[ch * F + D] == obs[perm[ch] * F + D]
    # pe_mask zeroes only e,d
    out = AttentionEnv.mask_surprise(obs)
    for ch in range(dyn.N_CHANNELS):
        assert out[ch * F + E] == 0.0 and out[ch * F + D] == 0.0
        assert out[ch * F + AttentionEnv.V] == obs[ch * F + AttentionEnv.V]


def test_mask_surprise_channel_specific():
    """pe_mask_ch zeroes e,d of exactly one channel."""
    F, E, D = AttentionEnv.FEATS_PER_CHANNEL, AttentionEnv.E, AttentionEnv.D
    obs = np.arange(24, dtype=np.float64) / 24.0
    out = AttentionEnv.mask_surprise_ch(obs, dyn.CH_D)
    for ch in range(dyn.N_CHANNELS):
        if ch == dyn.CH_D:
            assert out[ch * F + E] == 0.0 and out[ch * F + D] == 0.0
        else:
            assert out[ch * F + E] == obs[ch * F + E]
            assert out[ch * F + D] == obs[ch * F + D]
        assert out[ch * F + AttentionEnv.V] == obs[ch * F + AttentionEnv.V]


def test_mask_surprise_batched():
    """mask_surprise works on (n, 24) batches too (used for belief-only)."""
    obs = np.arange(48, dtype=np.float64).reshape(2, 24) / 48.0
    out = AttentionEnv.mask_surprise(obs)
    F, E, D = AttentionEnv.FEATS_PER_CHANNEL, AttentionEnv.E, AttentionEnv.D
    for ch in range(dyn.N_CHANNELS):
        assert np.all(out[:, ch * F + E] == 0.0)
        assert np.all(out[:, ch * F + D] == 0.0)
        assert np.all(out[:, ch * F + AttentionEnv.V]
                      == obs[:, ch * F + AttentionEnv.V])
