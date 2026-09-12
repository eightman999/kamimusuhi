"""Determinism tests: same seed -> identical rollout."""

import numpy as np

from experiments.u0.env.u0_env import IGNORE, STORE, U0Config, U0Env


def rollout(seed, steps=80):
    env = U0Env(U0Config(seed=seed))
    env.reset()
    rng = np.random.default_rng(seed + 1)
    traj = []
    for _ in range(steps):
        a = int(rng.integers(6))
        obs, r, done, info = env.step(a)
        traj.append((obs.copy(), r, done))
        if done:
            break
    return traj


def test_identical_seed_identical_rollout():
    t1 = rollout(7)
    t2 = rollout(7)
    assert len(t1) == len(t2)
    for (o1, r1, d1), (o2, r2, d2) in zip(t1, t2):
        assert np.allclose(o1, o2)
        assert r1 == r2 and d1 == d2


def test_different_seed_diverges():
    t1 = rollout(7)
    t2 = rollout(8)
    diffs = [not np.allclose(o1, o2)
             for (o1, _, _), (o2, _, _) in zip(t1, t2)]
    assert any(diffs)


def test_reset_reproduces_schedule():
    env = U0Env(U0Config(seed=5))
    env.reset(seed=123)
    sched1 = [(e.kind, e.func, e.loc) for e in env.schedule]
    needs1 = list(env.need_plan)
    env.reset(seed=123)
    sched2 = [(e.kind, e.func, e.loc) for e in env.schedule]
    assert sched1 == sched2
    assert list(env.need_plan) == needs1
