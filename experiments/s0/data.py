"""Dataset collection and config loading for S0."""

from __future__ import annotations

import copy
from pathlib import Path
from typing import Optional

import numpy as np
import yaml

from env import AgencyEnv, EnvConfig
from env.agency_env import habitual_policy, random_policy, rollout

POLICIES = {"random": random_policy, "habitual": habitual_policy}

# seeds used for data/eval must not collide with env param seeds
DATA_SEED_OFFSET = 100_000
EVAL_SEED_OFFSET = 500_000


def load_config(path: str | Path) -> dict:
    with open(path) as f:
        return yaml.safe_load(f)


def make_env_config(cfg_dict: dict, **overrides) -> EnvConfig:
    d = copy.deepcopy(cfg_dict)
    d.update(overrides)
    return EnvConfig(**d)


def collect_dataset(env_cfg: EnvConfig, n_episodes: int, seed: int,
                    policy: str = "random",
                    env_seed: Optional[int] = None,
                    env: Optional[AgencyEnv] = None) -> dict:
    """Roll out episodes; returns stacked arrays.

    env_seed: dynamics parameter seed. Defaults to `seed`. Data noise is
    driven by DATA_SEED_OFFSET + seed so train/eval differ. If `env` is
    given it is used directly (for runtime interventions like S-C4).
    """
    env = env or AgencyEnv(env_cfg,
                           seed=env_seed if env_seed is not None else seed)
    pol = POLICIES[policy]
    eps = []
    for i in range(n_episodes):
        eps.append(rollout(env, pol, env_cfg.episode_len,
                           seed=seed + DATA_SEED_OFFSET + i))
    keys = ("obs", "next_obs", "actions", "world", "action_comp", "noise",
            "disturbance")
    out = {k: np.stack([e[k] for e in eps]) for k in keys}
    out["cause_labels"] = env.cause_labels
    out["env_seed"] = env_seed if env_seed is not None else seed
    return out
