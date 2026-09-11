"""Environment tests: determinism, context structure, leakage guards."""

import numpy as np

from ..env import (LatentCauseEnv, EnvConfig, N_ACTIONS, N_CAUSES,
                   CAUSE_TABLE, TAP, NOOP)
from ..env.dynamics import make_dynamics_params


def test_deterministic_replay():
    a = LatentCauseEnv(EnvConfig(), seed=3)
    b = LatentCauseEnv(EnvConfig(), seed=3)
    rng = np.random.default_rng(0)
    acts = rng.integers(N_ACTIONS, size=48)
    oa, ob = [a.reset()], [b.reset()]
    for act in acts:
        oa.append(a.step(int(act))[0])
        ob.append(b.step(int(act))[0])
    np.testing.assert_allclose(np.stack(oa), np.stack(ob))


def test_context_is_exact_signed_transform():
    """With obs_noise=0, the same latent trajectory rendered in two
    contexts satisfies obs_j = C_j @ inv(C_i) @ obs_i exactly."""
    cfg = EnvConfig(obs_noise=0.0)
    ea = LatentCauseEnv(cfg, seed=11)
    eb = LatentCauseEnv(cfg, seed=11)
    ea.set_context(0)
    eb.set_context(1)
    rng = np.random.default_rng(1)
    acts = rng.integers(N_ACTIONS, size=40)
    pa = ea.params.contexts[0]
    pb = eb.params.contexts[1]
    oa, ob = [ea.reset(ctx_id=0)], [eb.reset(ctx_id=1)]
    for act in acts:
        oa.append(ea.step(int(act))[0])
        ob.append(eb.step(int(act))[0])
    oa, ob = np.stack(oa), np.stack(ob)
    np.testing.assert_allclose(ob, oa @ np.linalg.inv(pa).T @ pb.T,
                               atol=1e-10)


def test_same_cause_many_appearances():
    """One cause must produce many different observations."""
    cfg = EnvConfig(obs_noise=0.0, switch_prob=0.0, min_segment=100,
                    max_segment=100)
    env = LatentCauseEnv(cfg, seed=5)
    env.reset(ctx_id=0)
    env.set_segment(1)  # HEAT
    rng = np.random.default_rng(2)
    obs = []
    for _ in range(30):
        o, _, _, info = env.step(int(rng.integers(N_ACTIONS)))
        obs.append(o)
        assert info["cause_a"] == 1
    obs = np.stack(obs)
    spread = obs.std(axis=0).mean()
    assert spread > 0.02, f"same-cause obs too similar: {spread}"
    # and appearance is genuinely variable, not a fixed point
    assert np.linalg.matrix_rank(obs - obs.mean(0)) > 4


def test_appearance_decorrelated_across_contexts():
    """Same latent stream, different context -> low instantaneous
    similarity (the difficulty the experiment relies on)."""
    cfg = EnvConfig(obs_noise=0.0)
    rng = np.random.default_rng(4)
    acts = rng.integers(N_ACTIONS, size=60)
    obs_by_ctx = []
    for k in (0, 1, 4):
        env = LatentCauseEnv(cfg, seed=7)
        env.reset(ctx_id=k)
        buf = []
        for a in acts:
            buf.append(env.step(int(a))[0])
        obs_by_ctx.append(np.stack(buf))
    o0, o1, o4 = obs_by_ctx
    cos01 = np.abs((o0 * o1).sum(1) / (np.linalg.norm(o0, axis=1)
                                      * np.linalg.norm(o1, axis=1)
                                      + 1e-12)).mean()
    cos04 = np.abs((o0 * o4).sum(1) / (np.linalg.norm(o0, axis=1)
                                      * np.linalg.norm(o4, axis=1)
                                      + 1e-12)).mean()
    assert cos01 < 0.7 and cos04 < 0.7, (cos01, cos04)


def test_pair_signal_is_superposition():
    """Pair-segment canonical signal == sum of the two causes'
    individual contributions (compositional structure)."""
    cfg = EnvConfig(obs_noise=0.0, min_segment=100, max_segment=100)
    env = LatentCauseEnv(cfg, seed=9)
    env.reset(ctx_id=0)
    env.set_segment(1, 3)  # HEAT + OSC pair
    p = env.params
    rng = np.random.default_rng(0)
    for _ in range(10):
        _, _, _, info = env.step(int(rng.integers(N_ACTIONS)))
        xa, xb = info["x_a"], info["x_b"]
        expect = p.w @ (xa * p.v_table[1] + xb * p.v_table[3])
        np.testing.assert_allclose(info["signal"], expect, atol=1e-10)


def test_no_label_leakage_single_dim():
    """No single observation dim may take a small discrete set of values
    tracking the cause id (dims are continuous mixtures + permuted)."""
    env = LatentCauseEnv(EnvConfig(obs_noise=0.0), seed=13)
    env.reset()
    rng = np.random.default_rng(5)
    obs, causes = [], []
    for _ in range(200):
        o, _, _, info = env.step(int(rng.integers(N_ACTIONS)))
        obs.append(o)
        causes.append(info["cause_a"])
    obs = np.stack(obs)
    for d in range(env.cfg.obs_dim):
        assert len(np.unique(obs[:, d])) > 50, f"dim {d} looks discrete"


def test_cross_context_raw_probe_near_chance():
    """Guard on intended difficulty: a linear probe on raw obs trained
    in one context must fail in another (no free lunch from raw
    appearance alone)."""
    from ..data import collect_dataset
    from ..probes.linear import logistic_probe
    cfg = EnvConfig()
    ds0 = collect_dataset(cfg, 16, 0, ctx_ids=[0], pair_set="off")
    ds1 = collect_dataset(cfg, 16, 1, ctx_ids=[1], pair_set="off")
    x0 = ds0["obs"][:, 4:].reshape(-1, cfg.obs_dim)
    y0 = ds0["cause_a"][:, 4:].ravel()
    x1 = ds1["obs"][:, 4:].reshape(-1, cfg.obs_dim)
    y1 = ds1["cause_a"][:, 4:].ravel()
    r = logistic_probe(x0, y0, x1, y1, n_classes=N_CAUSES, steps=300)
    assert r["acc"] < 0.45, f"raw obs leak cause cross-context: {r['acc']}"


def test_state_roundtrip():
    env = LatentCauseEnv(EnvConfig(), seed=8)
    env.reset()
    for a in [0, 2, 1]:
        env.step(a)
    st = env.get_state()
    o1, _, _, i1 = env.step(0)
    env.set_state(st)
    o2, _, _, i2 = env.step(0)
    np.testing.assert_allclose(o1, o2)
    assert i1["cause_a"] == i2["cause_a"]


def test_segment_switch_flag():
    env = LatentCauseEnv(EnvConfig(min_segment=3, max_segment=6,
                                   switch_prob=0.5), seed=4)
    env.reset()
    prev = None
    saw_switch = False
    for _ in range(40):
        _, _, _, info = env.step(3)
        if prev is not None and info["cause_a"] != prev:
            assert info["switch"], "cause change must flag switch"
        if info["switch"]:
            saw_switch = True
        prev = info["cause_a"]
    assert saw_switch


def test_action_gain_hook():
    cfg = EnvConfig(obs_noise=0.0, min_segment=100, max_segment=100)
    env = LatentCauseEnv(cfg, seed=6)
    env.set_action_gain(3.0)
    env.reset(ctx_id=0)
    env.set_segment(1)
    xs = []
    for _ in range(8):
        _, _, _, info = env.step(0)  # APPROACH
        xs.append(info["x_a"])
    assert xs[-1] > xs[0] + 0.5


def test_mid_episode_context_switch_hook():
    env = LatentCauseEnv(EnvConfig(), seed=10)
    env.reset(ctx_id=0)
    for _ in range(5):
        env.step(3)
    env.set_context(4)
    _, _, _, info = env.step(3)
    assert info["ctx_id"] == 4


def test_params_deterministic_per_seed():
    p1 = make_dynamics_params(EnvConfig(), 17)
    p2 = make_dynamics_params(EnvConfig(), 17)
    np.testing.assert_allclose(p1.w, p2.w)
    for c1, c2 in zip(p1.contexts, p2.contexts):
        np.testing.assert_allclose(c1, c2)


def test_cause_table_invariants():
    assert len(CAUSE_TABLE) == N_CAUSES
    for c in CAUSE_TABLE:
        assert len(c.v) == 4 and len(c.resp) == N_ACTIONS
        assert c.resp[NOOP] == 0.0
