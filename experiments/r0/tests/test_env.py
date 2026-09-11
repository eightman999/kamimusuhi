import numpy as np

from ..env.r0_env import (ANSWER, IGNORE, ITEM, NOISE, QUERY, RECALL, STORE,
                          R0Config, R0Env)


def cfg(**kw):
    base = dict(episode_len=96, delay_min=8, delay_max=32, num_distractors=8,
                queries_per_episode=3, seed=7)
    base.update(kw)
    return R0Config(**base)


def test_schedule_counts_and_delays():
    env = R0Env(cfg())
    env.reset(seed=123)
    kinds = [e.kind for e in env.schedule]
    assert kinds.count(QUERY) == 3
    n_dist = sum(1 for e in env.schedule
                 if e.kind == ITEM and e.cls == "distractor")
    assert n_dist == 8
    for k, t_q in env._queries:
        t_p = next(s for kk, s, v in env._items if kk == k)
        assert 8 <= t_q - t_p <= 32


def test_obs_shape_and_determinism():
    c = cfg()
    o1 = R0Env(c).reset(seed=5)
    o2 = R0Env(c).reset(seed=5)
    assert o1.shape == (c.obs_dim,)
    assert np.allclose(o1, o2)


def test_store_recall_answer_flow():
    env = R0Env(cfg())
    env.reset(seed=11)
    # walk to the first ITEM, store it, then answer its query correctly
    while env.schedule[env.t].kind != ITEM:
        env.step(IGNORE)
    env.step(STORE)
    k = env.schedule[env.t - 1].key
    while not (env.schedule[env.t].kind == QUERY
               and env.schedule[env.t].key == k):
        env.step(IGNORE)
    env.step(RECALL)
    s = env.memory.find_key(k)
    off = 4 + env.cfg.num_keys
    true_val = int(np.argmax(env.memory.payloads[s, off:off + env.cfg.num_values]))
    _o, r, _d, _i = env.step(ANSWER, true_val)
    assert r > 0
    assert env.query_records[-1]["correct"]


def test_wrong_and_expired_queries_penalized():
    env = R0Env(cfg())
    env.reset(seed=3)
    while env.schedule[env.t].kind != QUERY:
        env.step(IGNORE)
    qk = env.schedule[env.t].key
    true = next(v for k, s, v in env._items if k == qk)
    _o, r, _d, _i = env.step(ANSWER, (true + 1) % env.cfg.num_values)
    assert r < 0
    env.reset(seed=4)
    while env.schedule[env.t].kind != QUERY:
        env.step(IGNORE)
    for _ in range(env.cfg.query_window):
        _o, r, _d, _i = env.step(IGNORE)
    assert env.query_records[-1]["answered"] is False


def test_noise_payload_has_no_key():
    env = R0Env(cfg())
    env.reset(seed=9)
    while env.schedule[env.t].kind != NOISE:
        env.step(IGNORE)
    env.step(STORE)
    assert -1 in env.memory.keys


def test_causal_ops():
    env = R0Env(cfg())
    env.reset(seed=21)
    while env.schedule[env.t].kind != ITEM:
        env.step(IGNORE)
    env.step(STORE)
    assert env.memory.num_occupied == 1
    env.permute_memory(np.random.default_rng(0))
    assert env.memory.num_occupied == 1
    env.erase_memory()
    assert env.memory.num_occupied == 0


def test_episode_stats_keys():
    env = R0Env(cfg())
    env.reset(seed=15)
    done = False
    while not done:
        _o, _r, done, info = env.step(IGNORE)
    st = info["ep_stats"]
    for k in ["accuracy", "important_retention", "memory_efficiency",
              "store_precision", "stores", "recalls"]:
        assert k in st
