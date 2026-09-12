"""Config loading for U0 experiments.

A config YAML has top-level keys `env` (U0Config fields) and `train`
(PPO hyperparameters). Unknown env keys raise via the dataclass
constructor; unknown top-level keys raise here.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import yaml

from .env.u0_env import U0Config

TRAIN_KEYS = {
    "iters", "num_envs", "lr", "epochs", "minibatch_episodes", "clip",
    "gamma", "lam", "vf_coef", "ent_coef", "max_grad_norm", "eval_every",
    "eval_episodes", "val_seed", "imitation_iters", "checkpoint_every",
}

TOP_KEYS = {"env", "train", "name", "seed"}


def load_config(path: str | Path) -> dict:
    with open(path) as f:
        raw: dict[str, Any] = yaml.safe_load(f) or {}
    bad = set(raw) - TOP_KEYS
    if bad:
        raise ValueError(f"unknown config keys: {sorted(bad)}")
    raw.setdefault("env", {})
    raw.setdefault("train", {})
    bad_train = set(raw["train"]) - TRAIN_KEYS
    if bad_train:
        raise ValueError(f"unknown train keys: {sorted(bad_train)}")
    return raw


def env_config(cfg: dict, seed: int | None = None) -> U0Config:
    d = dict(cfg.get("env", {}))
    for k in ("function_shift", "obs_func_perm", "obs_loc_perm"):
        if d.get(k) is not None:
            d[k] = tuple(d[k])
    if seed is not None:
        d["seed"] = seed
    return U0Config(**d)


def train_config(cfg: dict) -> dict:
    defaults = {
        "iters": 400, "num_envs": 32, "lr": 3e-4, "epochs": 3,
        "minibatch_episodes": 32, "clip": 0.2, "gamma": 0.99, "lam": 0.95,
        "vf_coef": 0.5, "ent_coef": 0.01, "max_grad_norm": 0.5,
        "eval_every": 10, "eval_episodes": 64, "val_seed": 700001,
        "imitation_iters": 0, "checkpoint_every": 10,
    }
    return {**defaults, **cfg.get("train", {})}
