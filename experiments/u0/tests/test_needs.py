"""Need-sampling tests: deviation-conditioned targets, determinism."""

import numpy as np

from experiments.u0.env.u0_env import (ENERGY, U0Config, U0Env,
                                       INTERNAL_NAMES, PREFERRED_RANGES,
                                       var_deviation)


def cfg(**kw):
    return U0Config(**kw)


def test_need_biased_toward_depleted_variable():
    """The planned need should hit the initially-depleted variable far
    more often than chance, and rarely hit a comfortable one."""
    match_worst = 0
    n = 200
    for seed in range(n):
        env = U0Env(cfg(seed=seed, init_perturb_prob=1.0,
                        init_perturb_lo=0.12, init_perturb_hi=0.12,
                        needs_per_episode=1))
        env.reset()
        devs = [var_deviation(env.internal, i) for i in range(4)]
        worst = int(np.argmax(devs))
        match_worst += int(env.need_plan[0][0] == worst)
    # uniform guessing gives 25%; deviation weighting must clearly beat it
    assert match_worst / n > 0.55


def test_need_targets_most_deviated_when_forced():
    """With a hard-coded depleted var and high beta, needs almost always
    target that variable."""
    hits = 0
    n = 60
    for seed in range(n):
        env = U0Env(cfg(seed=seed, init_perturb_prob=0.0,
                        needs_per_episode=1, need_beta=40.0))
        env.reset()
        env.internal[ENERGY] = 0.20   # far below range
        # re-run need sampling on the modified state
        rng = np.random.default_rng(seed + 7)
        last_func = max(t for t, e in enumerate(env.schedule)
                        if e.cls == "functional")
        needs = env._make_needs(rng, last_func)
        hits += int(needs[0].var == ENERGY)
    assert hits / n > 0.90


def test_need_plan_deterministic_per_seed():
    env1 = U0Env(cfg(seed=42))
    env2 = U0Env(cfg(seed=42))
    env1.reset()
    env2.reset()
    assert env1.need_plan == env2.need_plan
    assert [(e.kind, e.loc) for e in env1.schedule] == \
           [(e.kind, e.loc) for e in env2.schedule]
