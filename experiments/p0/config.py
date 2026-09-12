"""Config loading for P0 experiments.

A config is a YAML file with optional top-level keys:

    seed: 0
    env:        {episode_length: 512, ...}    # EnvParams overrides
    train:      {total_steps: 300000, ...}    # TrainConfig overrides
    agent:      {kind: mlp, hidden: 64, ...}  # agent spec
    predictor:  {hidden: 32, lr: 0.003, ...}  # predictor spec
    eval:       {episodes: 12, ...}

Unknown keys raise an error so typos cannot silently pass.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional

import yaml

from .env.dynamics import EnvParams


@dataclass
class TrainConfig:
    total_steps: int = 300_000        # env steps across all vectorized envs
    rollout_steps: int = 128          # per env per rollout
    num_envs: int = 8
    ppo_epochs: int = 4
    minibatch_size: int = 256
    lr: float = 3e-4
    gamma: float = 0.99
    gae_lambda: float = 0.95
    clip_coef: float = 0.2
    ent_coef: float = 0.01
    vf_coef: float = 0.5
    max_grad_norm: float = 0.5
    target_kl: Optional[float] = None  # early-stop PPO epochs past this KL
    eval_every_updates: int = 5       # evaluate every N PPO updates
    eval_episodes: int = 6            # episodes per periodic eval
    device: str = "cpu"


@dataclass
class PredictorConfig:
    hidden: int = 32
    lr: float = 3e-3
    ema: float = 0.5                  # EMA factor for per-channel error feature
                                      # (fast: a ramp obs must register in ~2)
    err_scale: float = 4.0            # squashes e,d features to ~[0,1] via tanh


@dataclass
class EvalConfig:
    episodes: int = 12
    max_steps: Optional[int] = None   # default: env episode_length


@dataclass
class AgentSpec:
    kind: str = "mlp"                 # random|roundrobin|always|fixed|pe_heuristic|pe_thresh|trigger|clock|mlp|gru
    hidden: int = 64                  # MLP/GRU hidden size
    pe_mask: bool = False             # belief-only control: e,d zeroed at
                                      # BOTH train and eval time (M2)


@dataclass
class Config:
    seed: int = 0
    env: EnvParams = field(default_factory=EnvParams)
    train: TrainConfig = field(default_factory=TrainConfig)
    eval: EvalConfig = field(default_factory=EvalConfig)
    agent: AgentSpec = field(default_factory=AgentSpec)
    predictor: PredictorConfig = field(default_factory=PredictorConfig)
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
        cfg.env = EnvParams(**_filter(EnvParams, dict(raw["env"]), "env"))
    if "train" in raw:
        cfg.train = TrainConfig(**_filter(TrainConfig, dict(raw["train"]), "train"))
    if "eval" in raw:
        cfg.eval = EvalConfig(**_filter(EvalConfig, dict(raw["eval"]), "eval"))
    if "agent" in raw:
        cfg.agent = AgentSpec(**_filter(AgentSpec, dict(raw["agent"]), "agent"))
    if "predictor" in raw:
        cfg.predictor = PredictorConfig(
            **_filter(PredictorConfig, dict(raw["predictor"]), "predictor")
        )
    return cfg


def save_config(cfg: Config, path: str | Path) -> None:
    d = {
        "name": cfg.name,
        "seed": cfg.seed,
        "env": dataclasses.asdict(cfg.env),
        "train": dataclasses.asdict(cfg.train),
        "eval": dataclasses.asdict(cfg.eval),
        "agent": dataclasses.asdict(cfg.agent),
        "predictor": dataclasses.asdict(cfg.predictor),
    }
    with open(path, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False)
