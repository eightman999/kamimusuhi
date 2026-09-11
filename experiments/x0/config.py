"""Config loading for X0 experiments.

A config is a YAML file with optional top-level keys:

    seed: 0
    env:   {n_events: 6, T: 16, lag: 2, ...}   # SceneParams + cause-space keys
    model: {kind: binder, d_z: 32, ...}        # model spec
    train: {steps: 400, batch_scenes: 128, ...}
    eval:  {scenes: 512, ...}

Unknown keys raise an error so typos cannot silently pass.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

import yaml

from .env.latent_cause import SceneParams


@dataclass
class EnvConfig:
    # cause space
    factors: Tuple[int, ...] = (4, 4, 4)
    holdout: Tuple[int, ...] = (2, 2)   # f0>=h0 AND f1>=h1 -> held-out combos
    modalities: Tuple[str, ...] = ("vis", "aud", "temp", "mot")
    # scene generator (SceneParams fields)
    n_events: int = 6
    T: int = 16
    lag: int = 2
    fixed_shift: Optional[int] = None
    token_drop_p: float = 0.1
    modality_drop_p: float = 0.1
    noise: float = 0.1
    conflict_delta: int = 2
    conflict_pair: Tuple[str, str] = ("vis", "aud")
    # dataset sizes
    n_train_scenes: int = 8192
    n_val_scenes: int = 512

    def scene_params(self) -> SceneParams:
        return SceneParams(n_events=self.n_events, T=self.T, lag=self.lag,
                           fixed_shift=self.fixed_shift,
                           token_drop_p=self.token_drop_p,
                           modality_drop_p=self.modality_drop_p,
                           noise=self.noise, conflict_delta=self.conflict_delta,
                           conflict_pair=tuple(self.conflict_pair))


@dataclass
class ModelSpec:
    kind: str = "binder"          # binder | bottleneck | indep_ae
    d_z: int = 32
    hidden: int = 64
    tau: float = 0.2              # InfoNCE temperature
    window: int = 3               # in-window half-width for positives/cands
    contra_weight: float = 1.0    # binder: multi-positive InfoNCE weight
    recon_weight: float = 0.5     # binder: soft cross-modal recon weight
    ae_weight: float = 1.0        # bottleneck/indep_ae: within-modal recon
    cross_weight: float = 1.0     # bottleneck: min-window cross recon weight


@dataclass
class TrainConfig:
    steps: int = 400
    batch_scenes: int = 128
    lr: float = 1e-3
    val_every: int = 20
    threads: int = 4
    device: str = "cpu"


@dataclass
class EvalConfig:
    scenes: int = 512
    lags: Tuple[int, ...] = (0, 1, 2, 4, 8)
    conflict_deltas: Tuple[int, ...] = (1, 2)
    drop_rates: Tuple[float, ...] = (0.0, 0.25, 0.5)
    alt_noise: float = 0.3


@dataclass
class Config:
    seed: int = 0
    env: EnvConfig = field(default_factory=EnvConfig)
    model: ModelSpec = field(default_factory=ModelSpec)
    train: TrainConfig = field(default_factory=TrainConfig)
    eval: EvalConfig = field(default_factory=EvalConfig)
    name: str = "run"


def _filter(cls, d: Dict[str, Any], where: str) -> Dict[str, Any]:
    valid = {f.name for f in dataclasses.fields(cls)}
    bad = set(d) - valid
    if bad:
        raise ValueError(f"unknown {where} config keys: {sorted(bad)}")
    return d


def _tuples(d: Dict[str, Any], keys) -> Dict[str, Any]:
    for k in keys:
        if k in d and d[k] is not None:
            d[k] = tuple(d[k])
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
        d = _tuples(dict(raw["env"]), ("factors", "holdout", "modalities",
                                       "conflict_pair"))
        cfg.env = EnvConfig(**_filter(EnvConfig, d, "env"))
    if "model" in raw:
        cfg.model = ModelSpec(**_filter(ModelSpec, dict(raw["model"]), "model"))
    if "train" in raw:
        cfg.train = TrainConfig(**_filter(TrainConfig, dict(raw["train"]), "train"))
    if "eval" in raw:
        cfg.eval = EvalConfig(**_filter(EvalConfig,
                                        _tuples(dict(raw["eval"]),
                                                ("lags", "conflict_deltas",
                                                 "drop_rates")), "eval"))
    return cfg


def save_config(cfg: Config, path: str | Path) -> None:
    d = {"name": cfg.name, "seed": cfg.seed,
         "env": dataclasses.asdict(cfg.env),
         "model": dataclasses.asdict(cfg.model),
         "train": dataclasses.asdict(cfg.train),
         "eval": dataclasses.asdict(cfg.eval)}
    with open(path, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False)
