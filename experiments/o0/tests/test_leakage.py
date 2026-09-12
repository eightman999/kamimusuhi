"""Occlusion-leakage audit.

While the target is inside the occluder the observation must not carry:
object position, velocity, appearance, object id, visibility-hidden
ground truth, or time-until-reappearance.  The occluder bounds are
observable world geometry and *do* weakly constrain the hidden position
(the object must be inside them) -- that is legitimate, so the probe
checks that nothing beyond the bounds helps decode it.
"""

import numpy as np

from experiments.o0.env import dynamics as dyn


def make(n=512, seed=0):
    p = dyn.EnvParams()
    return p, dyn.generate_batch(p, n, np.random.default_rng(seed))


def test_target_channels_zero_while_occluded():
    p, d = make()
    occ = d["occluded"]
    assert occ.sum() > 1000
    assert np.abs(d["obs"][occ][:, dyn.T_VIS]).max() == 0.0
    assert np.abs(d["obs"][occ][:, dyn.T_X]).max() == 0.0
    assert np.abs(d["obs"][occ][:, dyn.T_V]).max() == 0.0
    assert np.abs(d["obs"][occ][:, dyn.T_APP]).max() == 0.0


def test_no_channel_decodes_hidden_position():
    """Linear probe: corr(obs_channel, hidden x residual) ~ 0 for every
    channel except the occluder bounds (legitimate geometry)."""
    p, d = make(seed=1)
    occ = d["occluded"]
    x_true = d["pos"][occ]                    # normalized hidden position
    lo = d["obs"][occ][:, dyn.OCC_LO_IDX]
    hi = d["obs"][occ][:, dyn.OCC_HI_IDX]
    mid = 0.5 * (lo + hi)
    resid = x_true - mid                      # what the bounds cannot tell
    for c in range(dyn.OBS_DIM):
        if c in (dyn.OCC_LO_IDX, dyn.OCC_HI_IDX):
            continue
        chan = d["obs"][occ][:, c]
        if chan.std() < 1e-9:
            continue
        r = abs(np.corrcoef(chan, resid)[0, 1])
        assert r < 0.1, (
            f"channel {dyn.OBS_NAMES[c]} correlates with hidden position "
            f"residual (r={r:.3f})"
        )


def test_no_time_until_reappearance_channel():
    """During occlusion, no channel may encode steps-until-reappearance."""
    p, d = make(seed=2)
    occ = d["occluded"]
    T = d["obs"].shape[0]
    t = np.arange(T)[:, None]
    # steps remaining until reappearance (only where it happens)
    remaining = (d["t_reapp"][None, :] - t)
    has = (d["t_reapp"][None, :] > 0) & occ
    rem = np.broadcast_to(remaining, d["obs"].shape[:2])[has]
    for c in range(dyn.OBS_DIM):
        if c in (dyn.OCC_LO_IDX, dyn.OCC_HI_IDX):
            # bounds legitimately co-vary with remaining time (geometry)
            continue
        chan = d["obs"][has][:, c]
        if chan.std() < 1e-9:
            continue
        r = abs(np.corrcoef(chan, rem)[0, 1])
        assert r < 0.15, (
            f"channel {dyn.OBS_NAMES[c]} encodes time-until-reappearance "
            f"(r={r:.3f})"
        )


def test_counterfactual_hidden_state_same_obs():
    """Two worlds differing only in the hidden position produce identical
    observations during occlusion.  Simulated by re-generating with a
    velocity flip applied while hidden: obs must be identical at steps
    where both variants are occluded."""
    p = dyn.EnvParams().with_overrides(v_flip_prob=0.0)
    d1 = dyn.generate_batch(p, 64, np.random.default_rng(5))
    p2 = dyn.EnvParams().with_overrides(v_flip_prob=1.0)
    d2 = dyn.generate_batch(p2, 64, np.random.default_rng(5))
    # the only difference between variants is the hidden velocity flip.
    # note: once trajectories diverge, per-step RNG draw counts differ, so
    # stochastic channels (ambient, noise, obs jitter) legitimately
    # differ between variants.  What must NOT differ is anything that
    # could carry the hidden position: target channels (identically 0),
    # occluder bounds, and distractor visibility flags (deterministic
    # trajectories, identical draws).
    both_occ = d1["occluded"] & d2["occluded"]
    o1, o2 = d1["obs"], d2["obs"]
    assert np.abs(o1[both_occ][:, :4]).max() == 0.0
    assert np.abs(o2[both_occ][:, :4]).max() == 0.0
    np.testing.assert_array_equal(
        o1[both_occ][:, dyn.OCC_LO_IDX], o2[both_occ][:, dyn.OCC_LO_IDX])
    np.testing.assert_array_equal(
        o1[both_occ][:, dyn.OCC_HI_IDX], o2[both_occ][:, dyn.OCC_HI_IDX])
    for s in range(dyn.D_SLOTS):
        np.testing.assert_array_equal(
            o1[both_occ][:, dyn.D0 + 3 * s],
            o2[both_occ][:, dyn.D0 + 3 * s])


def test_distractor_channels_also_hidden():
    p, d = make(seed=3)
    obs = d["obs"]
    for s in range(dyn.D_SLOTS):
        base = dyn.D0 + 3 * s
        # whenever a distractor's visible flag is 0, its x/app are 0 too
        invis = obs[..., base] < 0.5
        assert np.abs(obs[..., base + 1][invis]).max() == 0.0
        assert np.abs(obs[..., base + 2][invis]).max() == 0.0
