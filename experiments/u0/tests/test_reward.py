"""Reward-shaping tests for U0.

The reward must be `-homeostatic_error - memory_op_costs - survival`;
nothing may mark an event as relevant or pay for correct storage.
"""

import numpy as np

from experiments.u0.env.u0_env import (IGNORE, RECALL, STORE, U0Config,
                                       U0Env, homeostatic_error)


def test_reward_equals_error_minus_costs():
    env = U0Env(U0Config(seed=0))
    env.reset()
    for _ in range(60):
        _o, r, _d, info = env.step(IGNORE)
        assert abs(r + info["homeostatic_error"]) < 1e-5


def test_store_relevance_is_not_rewarded():
    """Storing a to-be-needed event must pay the same cost as storing
    junk: relevance is invisible to the reward."""
    # every store incurs exactly store_cost on top of the error term,
    # whether the event will be needed or not
    env2 = U0Env(U0Config(seed=1, noise_rate=0.0))
    env2.reset()
    for t in range(60):
        e = env2.schedule[env2.t]
        if e.kind == 0:
            continue
        _o, r, _d, info = env2.step(STORE)
        expected = -info["homeostatic_error"] - env2.cfg.store_cost
        assert abs(r - expected) < 1e-5


def test_recall_cost():
    env = U0Env(U0Config(seed=2))
    env.reset()
    _o, r, _d, info = env.step(RECALL)
    expected = -info["homeostatic_error"] - env.cfg.recall_cost
    assert abs(r - expected) < 1e-5


def test_no_event_no_store_write():
    env = U0Env(U0Config(seed=3))
    env.reset()
    # find a NULL step
    t = next(i for i, e in enumerate(env.schedule) if e.kind == 0)
    env.t = t
    _o, r, _d, _i = env.step(STORE)
    assert env.memory.num_occupied == 0   # nothing storeable
