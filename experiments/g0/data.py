"""Dataset collection for G0.

Training data uses obs / next_obs / actions only. cause_a, cause_b,
ctx_id, intensities, segment ids and canonical signals are stored
alongside for EVALUATION ONLY.
"""

from __future__ import annotations

from pathlib import Path
from typing import Optional, Sequence

import numpy as np

from .env import ExplorePolicy, LatentCauseEnv, random_policy, rollout

# factories: a fresh policy per collection so no state leaks between
# episodes/datasets
POLICIES = {"random": lambda: random_policy, "explore": ExplorePolicy}

# seeds used for data/eval must not collide with env param seeds
DATA_SEED_OFFSET = 100_000
EVAL_SEED_OFFSET = 500_000

KEYS = ("obs", "next_obs", "actions", "cause_a", "cause_b", "x_a", "x_b",
        "seg_id", "switch", "canonical", "signal", "ctx")


def collect_dataset(env_cfg, n_episodes: int, seed: int,
                    policy: str = "explore",
                    env_seed: Optional[int] = None,
                    env: Optional[LatentCauseEnv] = None,
                    ctx_ids: Optional[Sequence[int]] = None,
                    pair_set: Optional[str] = None,
                    ctx_schedule: Optional[dict] = None,
                    ctx_switch_frac: float = 0.0) -> dict:
    """Roll out episodes; returns stacked arrays.

    env_seed: dynamics parameter seed (defaults to `seed`). The env's
    noise/schedule rng is seeded from the DATA seed (`seed`), so datasets
    sharing an env_seed are the same world but independent episode rolls
    (train/val/eval are decorrelated). If `env` is given it is used
    directly (runtime hooks already applied).
    ctx_ids: optional per-episode context assignment; if None, contexts
    are assigned round-robin over the env's context pool so every
    context is covered deterministically.
    pair_set: override env pair set ("train" | "ood" | "all" | "off").
    ctx_switch_frac: fraction of episodes that additionally get one
    mid-episode switch to a DIFFERENT context drawn from the same pool.
    This is the invariance signal: the cause persists across an
    appearance discontinuity.
    """
    env = env or LatentCauseEnv(
        env_cfg, seed=env_seed if env_seed is not None else seed,
        rng_seed=seed * 1_000_033 + 811)
    if pair_set is not None:
        env.set_pair_set(pair_set)
    pol = POLICIES[policy]()

    pool = (env._ctx_pool if env._ctx_pool is not None
            else list(range(env_cfg.n_train_contexts)))
    if ctx_ids is None:
        ctx_ids = [pool[i % len(pool)] for i in range(n_episodes)]

    eps = []
    for i in range(n_episodes):
        ctx = int(ctx_ids[i % len(ctx_ids)])
        sched = ctx_schedule
        if ctx_switch_frac > 0.0 and sched is None:
            s_rng = np.random.default_rng(seed + 77_000 + i)
            if s_rng.random() < ctx_switch_frac:
                T = env_cfg.episode_len
                n_sw = 1 + int(s_rng.random() < 0.4)  # 1 or 2 switches
                sched = {}
                cur = ctx
                for _ in range(n_sw):
                    t_sw = int(s_rng.integers(T // 5, 4 * T // 5))
                    others = [c for c in pool if c != cur]
                    if not others:
                        break
                    cur = int(others[int(s_rng.integers(len(others)))])
                    sched[t_sw] = cur
        eps.append(rollout(env, pol, env_cfg.episode_len,
                           seed=seed + DATA_SEED_OFFSET + i, ctx_id=ctx,
                           ctx_schedule=sched))
    out = {k: np.stack([e[k] for e in eps]) for k in KEYS}
    out["ctx_id"] = np.asarray([e["ctx_id"] for e in eps], dtype=np.int64)
    out["env_seed"] = env_seed if env_seed is not None else seed
    out["cause_names"] = env.cause_names
    return out


def train_val_datasets(cfg, seed: int) -> tuple[dict, dict]:
    """Train + val share the same env_seed: val is a held-out set of
    episodes IN THE SAME WORLD (same W, contexts, dim layout) — not a
    different environment. Data/noise seeds still differ."""
    train_ds = collect_dataset(cfg.env, cfg.data.episodes, seed,
                               cfg.data.policy, env_seed=seed,
                               ctx_switch_frac=cfg.data.ctx_switch_frac)
    val_ds = collect_dataset(cfg.env, cfg.data.val_episodes,
                             seed + 777, cfg.data.policy, env_seed=seed,
                             ctx_switch_frac=cfg.data.ctx_switch_frac)
    return train_ds, val_ds
