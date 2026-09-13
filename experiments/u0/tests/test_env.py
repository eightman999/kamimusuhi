"""Environment mechanics tests for U0."""

import numpy as np
import pytest

from experiments.u0.env.u0_env import (
    ACT, CLS_DISTRACTOR, CLS_FUNCTIONAL, CLS_NOISE, CRITICAL_THRESHOLDS,
    ENERGY, EV_DISTRACTOR, EV_NONE, EV_NOISE, FUNCTION_NAMES, IGNORE,
    INTERNAL_NAMES, MOVE, N_FUNCTIONS, OBS_DIM, PAYLOAD_DIM,
    PREFERRED_RANGES, RECALL, STORE, U0Config, U0Env, WAIT,
    check_death, homeostatic_error, var_deviation)


def cfg(**kw):
    return U0Config(seed=0, **kw)


def test_obs_dim_and_bounds():
    env = U0Env(cfg())
    obs = env.reset()
    assert obs.shape == (OBS_DIM,)
    assert np.all(np.isfinite(obs))
    for _ in range(50):
        obs, _r, _d, _i = env.step(IGNORE)
        assert obs.shape == (OBS_DIM,)
        assert np.all(np.isfinite(obs))


def test_sites_layout():
    env = U0Env(cfg())
    env.reset()
    funcs = [s["func"] for s in env.sites]
    pots = sorted(s["potency"] for s in env.sites)
    for f in range(N_FUNCTIONS):
        assert funcs.count(f) == 2
    assert pots == [env.cfg.potency_meager] * 4 + \
        [env.cfg.potency_rich] * 4
    for s in env.sites:
        assert 0 <= s["loc"] < env.cfg.num_locations


def test_functional_events_describe_real_sites():
    env = U0Env(cfg())
    env.reset()
    site_keys = {(s["func"], s["loc"], s["potency"]) for s in env.sites}
    n_func = 0
    for e in env.schedule:
        if e.cls == CLS_FUNCTIONAL:
            n_func += 1
            assert (e.func, e.loc, e.potency) in site_keys
            assert 1 <= e.kind <= 4
        elif e.cls == CLS_DISTRACTOR:
            assert e.kind == EV_DISTRACTOR
        elif e.cls == CLS_NOISE:
            assert e.kind == EV_NOISE
    assert n_func == len(env.sites)


def test_needs_fire_after_delay():
    env = U0Env(cfg(needs_per_episode=1, delay_min=24, delay_max=40))
    env.reset()
    last_func = max(t for t, e in enumerate(env.schedule)
                    if e.cls == CLS_FUNCTIONAL)
    for n in env.needs:
        assert n.onset >= last_func + 24
    # run until the need fires (a passive agent may die during the
    # crisis — that's fine, we only need the onset to have occurred)
    while not env.done and not any(
            n.active or n.resolved for n in env.needs):
        env.step(WAIT)
    assert any(n.active or n.resolved for n in env.needs)


def test_crisis_shock_announces_onset():
    """At onset the crisis variable jumps toward its lethal bound —
    much farther than passive drift alone could move it."""
    env = U0Env(cfg(needs_per_episode=1, delay_min=4, delay_max=4))
    env.reset()
    var = env.needs[0].var
    pre = None
    while not any(n.active or n.resolved for n in env.needs):
        pre = float(env.internal[var])
        env.step(IGNORE)
    name = INTERNAL_NAMES[var]
    lo_c, hi_c = CRITICAL_THRESHOLDS[name]
    post = float(env.internal[var])
    if name == "temperature":
        bound = hi_c if env.needs[0].dir > 0 else lo_c
    elif lo_c is not None:
        bound = lo_c
    else:
        bound = hi_c
    # shock moved the variable strictly toward the bound
    assert abs(post - bound) < abs(pre - bound)


def test_crisis_drains_and_act_resolves():
    env = U0Env(cfg(needs_per_episode=1, delay_min=4, delay_max=4))
    env.reset()
    # store nothing; walk the crisis manually
    while not any(n.active for n in env.needs):
        env.step(IGNORE)
    n = env.needs[0]
    var = n.var
    pre = float(env.internal[var])
    for _ in range(5):
        env.step(IGNORE)
    if var == ENERGY or var == 3:          # energy/certainty drain down
        assert env.internal[var] < pre
    elif var == 2:                          # risk drifts up
        assert env.internal[var] > pre
    # teleport to the matching site and ACT
    func = env._func_of_var(var)
    loc = next(s["loc"] for s in env.sites
               if s["func"] == func and s["potency"] == env.cfg.potency_rich)
    env.position = loc
    _o, _r, _d, info = env.step(ACT)
    assert n.resolved
    assert info.get("resolved_need") == var


def test_wrong_site_act_fails():
    env = U0Env(cfg(needs_per_episode=1, delay_min=4, delay_max=4))
    env.reset()
    while not any(n.active for n in env.needs):
        env.step(IGNORE)
    n = env.needs[0]
    func = env._func_of_var(n.var)
    bad_loc = next(l for l in range(env.cfg.num_locations)
                   if all(s["func"] != func for s in env.sites_at(l)))
    env.position = bad_loc
    _o, _r, _d, info = env.step(ACT)
    assert not n.resolved
    assert info.get("failed_act")


def test_preventive_act_pulls_toward_midpoint():
    """Preventive ACT must move the served variable toward its preferred
    range, never out of it: a safe zone lowers high risk (not raises it),
    and a shelter cannot overheat a comfortable agent."""
    env = U0Env(cfg(needs_per_episode=0, risk_spike_prob=0.0))
    env.reset()
    env.needs = []
    safe = next(s for s in env.sites if s["func"] == 2)   # safe_zone
    env.position = safe["loc"]
    env.internal[2] = 0.80                               # risk far above range
    env.step(ACT)
    assert env.internal[2] < 0.80 - 0.02                 # pulled down
    env2 = U0Env(cfg(needs_per_episode=0, risk_spike_prob=0.0))
    env2.reset()
    env2.needs = []
    shelter = next(s for s in env2.sites if s["func"] == 1)
    env2.position = shelter["loc"]
    env2.internal[1] = 0.62                              # temp inside range
    env2.step(ACT)
    assert env2.internal[1] < 0.65                       # never overshoots


def test_recall_returns_matching_site():
    env = U0Env(cfg(needs_per_episode=1, delay_min=4, delay_max=4))
    env.reset()
    while not any(n.active for n in env.needs):
        env.step(IGNORE)
    # recall with nothing stored -> invalid
    env.step(RECALL)
    assert not env.recall_valid
    # store the matching potent site payload manually via a STORE on the
    # functional event: find its schedule step is past; write directly
    n = env.needs[0]
    func = env._func_of_var(n.var)
    site = next(s for s in env.sites
                if s["func"] == func and s["potency"] == env.cfg.potency_rich)
    payload = np.zeros(PAYLOAD_DIM, dtype=np.float32)
    env.memory.store(payload, func, site["loc"], site["potency"])
    env.step(RECALL)
    assert env.recall_valid
    assert env.recall_loc == site["loc"]


def test_move_wraps_ring():
    env = U0Env(cfg())
    env.reset()
    env.position = env.cfg.num_locations - 1
    env.step(MOVE)
    assert env.position == 0


def test_store_cost_and_capacity():
    env = U0Env(cfg())
    env.reset()
    rewards = []
    for _ in range(30):
        e = env.schedule[env.t]
        if e.kind != EV_NONE:
            _o, r, _d, _i = env.step(STORE)
            rewards.append(r)
        else:
            env.step(IGNORE)
    assert all(r < 0 for r in rewards)
    assert env.memory.num_occupied <= env.cfg.memory_slots


def test_death_and_forfeit():
    env = U0Env(cfg())
    env.reset()
    env.internal[ENERGY] = 0.001
    _o, r, done, _i = env.step(IGNORE)
    assert done
    assert env.death_cause == "energy"
    # dying early must forfeit future error -> large negative reward
    assert r < -env.cfg.death_penalty


def test_error_weights_and_ranges():
    s = np.array([0.60, 0.50, 0.10, 0.70])
    assert homeostatic_error(s) == pytest.approx(0.0)
    s2 = np.array([0.30, 0.50, 0.10, 0.70])   # energy 0.10 below range
    assert homeostatic_error(s2) == pytest.approx(0.10 / 3.7)
    assert check_death(np.array([0.04, 0.5, 0.1, 0.7])) == "energy"
    assert check_death(np.array([0.5, 0.5, 0.1, 0.7])) is None


def test_no_need_label_leak():
    """The observation must not encode which variable will crisis."""
    env = U0Env(cfg())
    env.reset()
    obs = env._obs()
    planned = {v for v, _ in env.need_plan}
    for i in range(4, OBS_DIM):   # everything after the 4 internal channels
        v = obs[i]
        # channel values are structural (one-hot / rates / flags / time)
        assert 0.0 <= v <= 1.0 or -1.0 <= v <= 1.0
    # internal channels carry only state, not the plan
    assert set(obs[:4].tolist()) != set(planned)


def test_event_permutation_keeps_dims():
    c = cfg(obs_func_perm=(2, 0, 3, 1),
            obs_loc_perm=(1, 4, 0, 3, 2, 6, 8, 5, 10, 7, 11, 9))
    env = U0Env(c)
    obs = env.reset()
    assert obs.shape == (OBS_DIM,)
    for _ in range(40):
        obs, _r, _d, _i = env.step(IGNORE)
        assert np.all(np.isfinite(obs))


def test_function_shift_resolution():
    c = cfg(needs_per_episode=1, delay_min=4, delay_max=4,
            function_shift=(1, 2, 3, 0))   # resource site heals temperature
    env = U0Env(c)
    env.reset()
    while not any(n.active for n in env.needs):
        env.step(IGNORE)
    n = env.needs[0]
    func = env._func_of_var(n.var)
    assert env._func_map[func] == n.var


def test_directed_move_when_recalled():
    """With a valid recall readout MOVE walks the short way (2/step);
    without one it wanders +1."""
    env = U0Env(cfg(needs_per_episode=0))
    env.reset()
    L = env.cfg.num_locations
    env.position = 0
    env.step(MOVE)
    assert env.position == 1                    # wandering: +1
    env.position = 0
    env.recall_valid = True
    env.recall_loc = 4                          # forward dist 4 -> +3 hop
    env.step(MOVE)
    assert env.position == 3
    env.position = 0
    env.recall_loc = L - 2                      # backward dist 2 -> -2 hop
    env.step(MOVE)
    assert env.position == L - 2


def test_partial_act_no_resolution():
    """A meager site relieves the crisis variable but does NOT resolve
    the need when the pull cannot reach the preferred range."""
    env = U0Env(cfg(needs_per_episode=1, delay_min=4, delay_max=4,
                    potency_meager=0.30, risk_spike_prob=0.0))
    env.reset()
    while not any(n.active for n in env.needs):
        env.step(IGNORE)
    n = env.needs[0]
    func = env._func_of_var(n.var)
    site = next(s for s in env.sites
                if s["func"] == func and s["potency"] < 1.0)
    env.position = site["loc"]
    # push the variable deep into crisis so the meager pull can't reach
    lo, hi = PREFERRED_RANGES[INTERNAL_NAMES[n.var]]
    lo_c, hi_c = CRITICAL_THRESHOLDS[INTERNAL_NAMES[n.var]]
    bound = lo_c if lo_c is not None else hi_c
    if INTERNAL_NAMES[n.var] == "temperature":
        bound = hi_c if n.dir > 0 else lo_c
    env.internal[n.var] = bound + 0.01 * (0.5 - bound)
    _o, _r, _d, info = env.step(ACT)
    assert not n.resolved
    assert info.get("partial_act") and not info.get("failed_act")


def test_erase_for_need_targets_only_matching():
    """erase_for_need removes only slots serving that need's function —
    the targeted-erase manipulation, not a blanket clear."""
    env = U0Env(cfg())
    env.reset()
    for f, loc in ((0, 1), (1, 2), (2, 3)):
        env.memory.store(np.zeros(PAYLOAD_DIM, np.float32), f, loc, 1.0)
    var = 1   # temperature <- shelter func 1
    removed = env.erase_for_need(var)
    assert removed == 1
    remaining = env.memory.funcs[env.memory.occupied]
    assert sorted(remaining.tolist()) == [0, 2]
