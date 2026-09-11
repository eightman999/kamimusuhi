import numpy as np
import pytest

from ..env.c0_env import (C0Config, C0Env, IGNORE, STORE, ANSWER,
                          ITEM, QUERY, NOISE)


def cfg(**kw):
    base = dict(seed=7)
    base.update(kw)
    return C0Config(**base)


def test_schedule_classes_and_positions():
    env = C0Env(cfg())
    env.reset(seed=123)
    stop, gap = env.cfg.stops[0]
    resume = stop + gap
    qcls = env._q_classes
    for k, t_q in env._queries:
        t_i = next(s for kk, s, v in env._items if kk == k)
        cls = qcls[t_q]
        if cls == "pre":
            assert t_i < stop and t_q + env.cfg.query_window <= stop
        elif cls == "post":
            assert t_i < stop and t_q >= resume
        else:
            assert t_i >= resume and t_q >= resume
    # no ITEM/QUERY inside a gap
    for s, g in env.cfg.stops:
        for t in range(s, s + g):
            assert env.schedule[t].kind not in (ITEM, QUERY)


def test_determinism_same_seed():
    c = cfg()
    e1, e2 = C0Env(c), C0Env(c)
    e1.reset(seed=5); e2.reset(seed=5)
    assert [(x.kind, x.key, x.value) for x in e1.schedule] == \
           [(x.kind, x.key, x.value) for x in e2.schedule]
    obs1, obs2 = [], []
    while e1.t < c.episode_len:
        obs1.append(e1._obs()); obs2.append(e2._obs())
        e1.step(IGNORE); e2.step(IGNORE)
    assert all(np.allclose(a, b) for a, b in zip(obs1, obs2))


def test_obs_contains_no_answer():
    """QUERY events carry key only -- the value is never in the obs."""
    env = C0Env(cfg())
    env.reset(seed=3)
    c = env.cfg
    while env.t < c.episode_len:
        e = env.schedule[env.t]
        if e.kind == QUERY:
            assert e.value == -1            # no answer in the event
            obs = env._obs()
            vseg = obs[4 + c.num_keys:4 + c.num_keys + c.num_values]
            assert vseg.sum() == 0          # and no value one-hot
        env.step(IGNORE)


def test_gap_advance_and_resume():
    c = cfg(stops=((80, 30),))           # gap of 30
    env = C0Env(c)
    env.reset(seed=9)
    while env.t < 80:
        env.step(IGNORE)
    obs = env.advance_to(110)
    assert env.t == 110
    # queries must not have been skipped
    assert all(e.kind != QUERY for e in env.schedule[80:110])


def test_answer_flow_and_rewards():
    env = C0Env(cfg())
    env.reset(seed=11)
    while env.schedule[env.t].kind != QUERY:
        env.step(IGNORE)
    key = env.schedule[env.t].key
    true_v = next(v for k, s, v in env._items if k == key)
    _o, r, _d, _i = env.step(ANSWER, true_v)
    assert r > 0
    assert env.query_records[-1]["correct"]
    assert env.query_records[-1]["cls"] in ("pre", "post", "ctrl")


def test_multistop_schedule():
    c = cfg(stops=((50, 0), (100, 0)), n_mid=1)
    env = C0Env(c)
    env.reset(seed=21)
    crossed = [r for r in env._q_classes.items()]
    post = [t for t, cls in env._q_classes.items() if cls == "post"]
    assert len(post) >= 2          # bridge + mid queries
    for k, t_q in env._queries:
        t_i = next(s for kk, s, v in env._items if kk == k)
        if env._q_classes[t_q] == "post":
            assert t_q >= 100
            assert t_i < 50 or 50 <= t_i < 100
