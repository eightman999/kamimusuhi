"""Config loading for O0 experiments.

A config is a YAML file with optional top-level keys:

    seed: 0
    env:    {occ_lo: 4, occ_hi: 16, ...}     # EnvParams overrides
    train:  {updates: 2000, ...}             # TrainConfig overrides
    eval:   {episodes: 512, ...}             # EvalConfig overrides
    model:  {arch: gru64, hidden: 64}        # model spec

Unknown keys raise an error so typos cannot silently pass.
Budgets are also overrideable via env vars O0_UPDATES, O0_BATCH_SIZE,
O0_EPISODES and via CLI flags on train.py / evaluate.py / sweep.py.
"""

from __future__ import annotations

import dataclasses
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional

import yaml

from .env.dynamics import EnvParams


@dataclass
class TrainConfig:
    updates: int = 2000            # supervised updates (fresh batch each)
    batch_size: int = 64
    lr: float = 1e-3
    w_pos: float = 5.0             # weight of the position/velocity loss
    w_occ: float = 4.0             # extra weight multiplier on occluded steps
    w_vis: float = 0.25            # weight multiplier on visible steps
    w_exist: float = 1.0
    w_id: float = 1.0
    w_vel: float = 0.5             # velocity term inside the pos loss
    val_every: int = 25
    val_episodes: int = 256
    grad_clip: float = 1.0
    threads: int = 4
    device: str = "cpu"


@dataclass
class EvalConfig:
    episodes: int = 512
    seed: int = 910000             # base seed for evaluation rollouts


@dataclass
class ModelSpec:
    arch: str = "gru64"            # mlp | gru64 | gru128 | lstm64 | ssm
    hidden: int = 64               # used when arch leaves it unspecified


@dataclass
class Config:
    seed: int = 0
    env: EnvParams = field(default_factory=EnvParams)
    train: TrainConfig = field(default_factory=TrainConfig)
    eval: EvalConfig = field(default_factory=EvalConfig)
    model: ModelSpec = field(default_factory=ModelSpec)
    name: str = "run"


def _filter(cls, d: Dict[str, Any], where: str) -> Dict[str, Any]:
    valid = {f.name for f in dataclasses.fields(cls)}
    bad = set(d) - valid
    if bad:
        raise ValueError(f"unknown {where} config keys: {sorted(bad)}")
    return d


def load_config(path: str | Path) -> Config:
    path = Path(path)
    with open(path) as f:
        raw: Dict[str, Any] = yaml.safe_load(f) or {}

    cfg = Config()
    cfg.name = path.stem
    if "seed" in raw:
        cfg.seed = int(raw["seed"])
    if "name" in raw:
        cfg.name = str(raw["name"])
    if "env" in raw:
        env_d = dict(raw["env"])
        for k in ("distractor_probs",):
            if k in env_d and env_d[k] is not None:
                env_d[k] = tuple(env_d[k])
        cfg.env = EnvParams(**_filter(EnvParams, env_d, "env"))
    if "train" in raw:
        cfg.train = TrainConfig(**_filter(TrainConfig, dict(raw["train"]), "train"))
    if "eval" in raw:
        cfg.eval = EvalConfig(**_filter(EvalConfig, dict(raw["eval"]), "eval"))
    if "model" in raw:
        cfg.model = ModelSpec(**_filter(ModelSpec, dict(raw["model"]), "model"))

    # env-var budget overrides
    if os.environ.get("O0_UPDATES"):
        cfg.train.updates = int(os.environ["O0_UPDATES"])
    if os.environ.get("O0_BATCH_SIZE"):
        cfg.train.batch_size = int(os.environ["O0_BATCH_SIZE"])
    if os.environ.get("O0_EPISODES"):
        cfg.eval.episodes = int(os.environ["O0_EPISODES"])
    return cfg


def save_config(cfg: Config, path: str | Path) -> None:
    d = {
        "name": cfg.name,
        "seed": cfg.seed,
        "env": dataclasses.asdict(cfg.env),
        "train": dataclasses.asdict(cfg.train),
        "eval": dataclasses.asdict(cfg.eval),
        "model": dataclasses.asdict(cfg.model),
    }
    with open(path, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False)
