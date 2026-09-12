"""Unit tests for P0 channel dynamics, costs, detection bookkeeping."""

import numpy as np
import pytest

from experiments.p0.env import dynamics as dyn
from experiments.p0.env.attention_env import AttentionEnv


def make_env(seed=0, **overrides):
    return AttentionEnv(dyn.EnvParams(**overrides), seed=seed)


# ---------------------------------------------------------------------------
# Channels
# ---------------------------------------------------------------------------


def test_channel_a_is_nearly_constant():
    p = dyn.EnvParams(a_noise=0.005)
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    d = dyn.DState()
    vals = []
    for t in range(200):
        x, _, _ = dyn.update_channels(rng, x, t, 0.0, d, p)
        vals.append(x[dyn.CH_A])
    assert np.std(vals) < 0.05


def test_channel_b_oscillates():
    p = dyn.EnvParams(b_noise=0.0, b_period=50)
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    d = dyn.DState()
    vals = []
    for t in range(100):
        x, _, _ = dyn.update_channels(rng, x, t, 0.0, d, p)
        vals.append(x[dyn.CH_B])
    assert max(vals) - min(vals) > 0.4


def test_channel_c_has_jumps():
    p = dyn.EnvParams(c_jump_prob=0.1)
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    d = dyn.DState()
    deltas = []
    for t in range(300):
        prev = x[dyn.CH_C]
        x, _, _ = dyn.update_channels(rng, x, t, 0.0, d, p)
        deltas.append(abs(x[dyn.CH_C] - prev))
    assert max(deltas) > 0.2  # at least one real jump in 300 steps


def test_channel_d_events_rare_and_bounded():
    p = dyn.EnvParams()
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    d = dyn.DState()
    event_steps = 0
    for t in range(5000):
        x, _, _ = dyn.update_channels(rng, x, t, 0.0, d, p)
        event_steps += int(d.phase == dyn.D_EVENT)
    frac = event_steps / 5000
    assert 0.001 < frac < 0.15        # rare but present (1-step windows)
    assert d.n_events > 0


def test_event_window_is_single_step():
    """v2 design: the detection window is exactly 1 step, so a periodic
    clock cannot cover events cheaply."""
    p = dyn.EnvParams()
    assert p.d_event_steps == 1
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    # force EVENT then step once with no observation: event must expire missed
    d = dyn.DState(phase=dyn.D_EVENT, phase_t=0, detected=False, n_events=1)
    x, det, missed = dyn.update_channels(rng, x, 0, 0.0, d, p,
                                         d_observed=False)
    assert not det and missed and d.phase == dyn.D_QUIET


def test_pure_clock_cannot_reach_ceiling():
    """A poll-k step clock (k>=2) must miss most 1-step events; this is the
    property the v2 env was redesigned to have (C1)."""
    from experiments.p0.agents.baselines import ClockAgent
    from experiments.p0.config import load_config
    from experiments.p0.evaluate import evaluate

    cfg = load_config(
        __import__("pathlib").Path(__file__).resolve().parents[1]
        / "configs" / "base.yaml")
    cfg.eval.episodes = 4
    for poll, det_ceiling in ((2, 0.75), (4, 0.5)):
        res = evaluate(cfg, ClockAgent(poll=poll), 4, seed=0)
        det = res["aggregate"]["detection_rate_pooled"]
        assert det == det and det < det_ceiling, (poll, det)


def test_channels_stay_in_bounds():
    env = make_env(seed=5)
    env.reset()
    rng = np.random.default_rng(6)
    for _ in range(200):
        env.step(int(rng.integers(dyn.N_ACTIONS)))
        assert np.all(env.x >= 0.0) and np.all(env.x <= 1.0)
        if env.done:
            break


def test_c_jump_boosts_d_hazard():
    """Events should be more likely inside the post-jump boost window."""
    p = dyn.EnvParams()
    rng = np.random.default_rng(0)
    x = dyn.initial_channels(np.random.default_rng(1), p)
    d = dyn.DState()
    n_boosted, n_unboosted = 0, 0
    for t in range(20000):
        x, _, _ = dyn.update_channels(rng, x, t, 0.0, d, p)
        if d.phase == dyn.D_RAMP and d.phase_t == 1:
            pass
    # statistical version: run two populations
    counts = {}
    for boosted, trials in ((True, 4000), (False, 4000)):
        hits = 0
        for _ in range(trials):
            dd = dyn.DState()
            dd.boost = 8 if boosted else 0
            x2 = np.array([0.5, 0.5, 0.5, 0.15])
            dyn.update_channels(rng, x2, 0, 0.0, dd, p)
            hits += int(dd.phase == dyn.D_RAMP)
        counts[boosted] = hits / trials
    assert counts[True] > 3 * counts[False]


# ---------------------------------------------------------------------------
# Detection / reward
# ---------------------------------------------------------------------------


def test_detection_requires_observing_d():
    env = make_env(seed=0)
    env.reset()
    env.d.phase = dyn.D_EVENT
    env.d.phase_t = 0
    env.d.detected = False
    # step with IGNORE: cannot detect
    _, r, _, info = env.step(dyn.IGNORE)
    assert not info["detected"]
    # OBSERVE_D during EVENT detects
    env.d.phase = dyn.D_EVENT
    env.d.phase_t = 0
    _, r2, _, info2 = env.step(dyn.OBS_D)
    assert info2["detected"] and r2 > 0


def test_missed_event_penalized():
    env = make_env(seed=0)
    env.reset()
    env.d.phase = dyn.D_EVENT
    env.d.phase_t = env.params.d_event_steps - 1  # last event step
    env.d.detected = False
    _, r, _, info = env.step(dyn.IGNORE)
    assert info["missed"] and r < 0


def test_detection_bonus_once():
    env = make_env(seed=0)
    env.reset()
    env.d.phase = dyn.D_EVENT
    env.d.phase_t = 0
    _, r1, _, _ = env.step(dyn.OBS_D)   # detects
    _, r2, _, info = env.step(dyn.OBS_D)  # already detected: no 2nd bonus
    assert r1 > r2
    assert not info["detected"]


def test_costs_ordered():
    p = dyn.EnvParams()
    assert dyn.action_cost(dyn.IGNORE, p) == 0.0
    for a in (dyn.OBS_A, dyn.OBS_B, dyn.OBS_C, dyn.OBS_D):
        assert dyn.action_cost(a, p) == p.cost_observe
    assert dyn.action_cost(dyn.OBS_ALL, p) == p.cost_observe_all
    assert p.cost_observe < p.cost_observe_all < 4 * p.cost_observe


def test_episode_terminates_and_step_raises():
    env = make_env(seed=3, episode_length=10)
    env.reset()
    for _ in range(10):
        _, _, done, _ = env.step(dyn.IGNORE)
    assert done
    with pytest.raises(RuntimeError):
        env.step(dyn.IGNORE)


# ---------------------------------------------------------------------------
# Belief bookkeeping
# ---------------------------------------------------------------------------


def test_staleness_grows_and_resets():
    env = make_env(seed=0)
    obs = env.reset()
    assert np.all(obs[4:8] == 1.0)  # staleness cap
    env.step(dyn.IGNORE)
    env.step(dyn.IGNORE)
    obs, _, _, _ = env.step(dyn.OBS_D)
    assert obs[4 + dyn.CH_D] == 0.0          # D fresh
    assert obs[4 + dyn.CH_A] == 1.0          # A never observed
    obs, _, _, _ = env.step(dyn.IGNORE)
    assert obs[4 + dyn.CH_D] == 1.0 / dyn.STALENESS_CAP


def test_observed_value_close_to_truth():
    env = make_env(seed=0, obs_noise=0.0)
    env.reset()
    _, _, _, info = env.step(dyn.OBS_D)
    obs = env._obs()
    assert obs[dyn.CH_D] == pytest.approx(info["x"][dyn.CH_D])
