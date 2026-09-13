"""Config loading for G0-v4 (strict key validation).

Reuses the G0 dataclasses for env/data/model/train/eval so the v4
Config object is attribute-compatible with every `experiments.g0`
function (train_val_datasets, _eval_datasets, eval_representation, ...).

v4-specific hyperparameters live in a `v4:` section:

    v4:
      hidden: 64            # recurrent latent size (all v4 methods)
      horizon: 8            # CPC/JEPA prediction horizon (steps)
      temperature: 0.1      # InfoNCE temperature (cpc)
      z_dim: 64             # CPC per-step embedding dim
      readout_weight: 0.1   # aux linear latent->obs readout (enables the
                          # intervention/base_mse metrics without turning
                          # the objective into reconstruction)
      vicreg_sim: 10.0      # invariance weight
      vicreg_var: 10.0      # variance hinge weight (gamma=1)
      vicreg_cov: 1.0       # covariance penalty weight
      vicreg_pred: 1.0      # latent forward-prediction weight
      ema_decay: 0.99       # JEPA target encoder momentum
      jepa_var: 1.0         # JEPA anti-collapse variance guard weight
      jepa_cov: 0.1         # JEPA covariance guard weight
      aug_jitter: 0.10      # obs Gaussian jitter std (view augmentation)
      aug_drop: 0.15        # sensor-dim dropout prob (view augmentation)
      log_every: 15         # metrics.jsonl append interval (updates)
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict

import yaml

from experiments.g0.config import (DataConfig, EvalConfig, ModelConfig,
                                   OODConfig, TrainConfig, _filter)
from experiments.g0.env.dynamics import EnvConfig


@dataclass
class V4Config:
    hidden: int = 64               # recurrent latent dim (all methods)
    horizon: int = 8               # prediction horizon (cpc/jepa)
    temperature: float = 0.1       # InfoNCE temperature (cpc)
    z_dim: int = 64                # per-step embedding dim (cpc)
    readout_weight: float = 0.1    # aux linear latent->obs readout weight
    vicreg_sim: float = 10.0       # invariance MSE weight
    vicreg_var: float = 10.0       # variance hinge weight
    vicreg_cov: float = 1.0        # covariance penalty weight
    vicreg_pred: float = 1.0       # latent forward-prediction weight
    vicreg_gamma: float = 1.0      # variance hinge target std
    ema_decay: float = 0.99        # JEPA target encoder momentum
    jepa_var: float = 1.0          # JEPA variance guard weight
    jepa_cov: float = 0.1          # JEPA covariance guard weight
    aug_jitter: float = 0.10       # obs jitter std for views
    aug_drop: float = 0.15         # sensor-dim dropout prob for views
    log_every: int = 15            # metrics.jsonl interval (updates)


@dataclass
class Config:
    seed: int = 0
    env: EnvConfig = field(default_factory=EnvConfig)
    data: DataConfig = field(default_factory=DataConfig)
    model: ModelConfig = field(default_factory=ModelConfig)
    train: TrainConfig = field(default_factory=TrainConfig)
    eval: EvalConfig = field(default_factory=EvalConfig)
    v4: V4Config = field(default_factory=V4Config)
    name: str = "run"


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
    if "v4" in raw:
        cfg.v4 = V4Config(**_filter(V4Config, dict(raw["v4"]), "v4"))
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
        "v4": dataclasses.asdict(cfg.v4),
    }
    with open(path, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False)
