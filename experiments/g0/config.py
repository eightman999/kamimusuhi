"""Config loading for G0 experiments (strict key validation).

A config is a YAML file with optional top-level keys:

    seed: 0
    env:   {episode_len: 64, ...}    # EnvConfig overrides
    data:  {episodes: 256, ...}      # collection params
    model: {hidden: 32, ...}         # model hyper-params (per-run name
                                   #  is chosen via --model)
    train: {max_steps: 2000, ...}
    eval:  {episodes: 48, ood: {...}}

Unknown keys raise an error so typos cannot silently pass.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional

import yaml

from .env.dynamics import EnvConfig


@dataclass
class DataConfig:
    episodes: int = 256
    val_episodes: int = 32
    policy: str = "explore"          # random | explore
    ctx_switch_frac: float = 0.5     # fraction of train episodes with a
                                   # mid-episode train-context switch
                                   # (the invariance-learning signal)


@dataclass
class ModelConfig:
    hidden: int = 32                 # GRU hidden / latent dim for pred models
    ae_hidden: int = 64              # encoder width for AE models
    ae_latent: int = 8               # AE bottleneck dim
    codebook_size: int = 24          # VQ codebook entries
    commitment: float = 0.25         # VQ commitment cost
    window: int = 8                  # history window for windowed reps
    pca_dim: int = 8                 # PCA latent dim (per-step)
    pca_win_dim: int = 16            # PCA latent dim (windowed)
    kmeans_k: int = 16               # k-means-on-latent bottleneck


@dataclass
class TrainConfig:
    batch_size: int = 32             # episodes per batch
    lr: float = 1e-3
    max_steps: int = 2000
    time_budget_sec: float = 0.0     # 0 = unlimited; G0_TIME_BUDGET overrides
    target_delta: bool = True        # predict (next_obs - obs) for pred models


@dataclass
class OODConfig:
    noise_mul: float = 3.0           # read-noise scale
    action_gain_mul: float = 1.5     # intensity-range shift
    dropout_frac: float = 0.5        # fraction of obs dims zeroed
    ood_ctx_id: int = 6              # held-out permutation context
    ood_dense_ctx_id: int = 8        # held-out dense-mix context
    pair_prob: float = 0.6           # pair density in combo-OOD data


@dataclass
class EvalConfig:
    episodes: int = 48               # per dataset variant
    warmup: int = 4                  # leading steps of each ep ignored
    seg_warmup: int = 2              # leading steps of each segment
                                   # excluded from cause probes
    probe_steps: int = 800
    probe_lr: float = 0.05
    loco: bool = True                # leave-one-context-out probe
    fewshot: tuple = (1, 5, 20)      # shots per cause
    ood: OODConfig = field(default_factory=OODConfig)


@dataclass
class Config:
    seed: int = 0
    env: EnvConfig = field(default_factory=EnvConfig)
    data: DataConfig = field(default_factory=DataConfig)
    model: ModelConfig = field(default_factory=ModelConfig)
    train: TrainConfig = field(default_factory=TrainConfig)
    eval: EvalConfig = field(default_factory=EvalConfig)
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
        cfg.env = EnvConfig(**_filter(EnvConfig, dict(raw["env"]), "env"))
    if "data" in raw:
        cfg.data = DataConfig(
            **_filter(DataConfig, dict(raw["data"]), "data"))
    if "model" in raw:
        cfg.model = ModelConfig(
            **_filter(ModelConfig, dict(raw["model"]), "model"))
    if "train" in raw:
        cfg.train = TrainConfig(
            **_filter(TrainConfig, dict(raw["train"]), "train"))
    if "eval" in raw:
        ev = dict(raw["eval"])
        ood_d = ev.pop("ood", None)
        if "fewshot" in ev and ev["fewshot"] is not None:
            ev["fewshot"] = tuple(ev["fewshot"])
        cfg.eval = EvalConfig(**_filter(EvalConfig, ev, "eval"))
        if ood_d is not None:
            cfg.eval.ood = OODConfig(
                **_filter(OODConfig, dict(ood_d), "eval.ood"))
    return cfg


def save_config(cfg: Config, path: str | Path) -> None:
    d = {
        "name": cfg.name,
        "seed": cfg.seed,
        "env": dataclasses.asdict(cfg.env),
        "data": dataclasses.asdict(cfg.data),
        "model": dataclasses.asdict(cfg.model),
        "train": dataclasses.asdict(cfg.train),
        "eval": dataclasses.asdict(cfg.eval),
    }
    with open(path, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False)
