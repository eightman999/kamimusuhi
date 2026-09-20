"""Shared config / runner construction for the C0 phase scripts."""
from __future__ import annotations

import dataclasses
import hashlib
import json
import subprocess
from dataclasses import asdict
from pathlib import Path

import numpy as np
import torch
import yaml

from .agents.policies import MODEL_SPECS, build_policy
from .agents.runner import Runner
from .agents.heuristics import HEURISTICS, HeuristicRunner
from .env.c0_env import C0Config


def parse_overrides(pairs: list[str] | None) -> dict:
    """--set key=value ; stops use 'a:g,b:g' (stop:gap pairs)."""
    out = {}
    for kv in pairs or []:
        k, _, v = kv.partition("=")
        if k == "stops":
            out[k] = tuple(tuple(int(x) for x in p.split(":"))
                           for p in v.split(",") if p)
        else:
            try:
                out[k] = json.loads(v)
            except json.JSONDecodeError:
                out[k] = v
    return out


def load_env_cfg(config_path: str, overrides: list[str] | None = None):
    raw = yaml.safe_load(open(config_path)) or {}
    env_d = raw.get("env", {})
    env_d.update(parse_overrides(overrides))
    if "stops" in env_d:
        env_d["stops"] = tuple(tuple(s) for s in env_d["stops"])
    cfg = C0Config(**{k: v for k, v in env_d.items()
                      if k in {f.name for f in dataclasses.fields(C0Config)}})
    return cfg, env_d


def load_full_cfg(config_path: str) -> dict:
    return yaml.safe_load(open(config_path)) or {}


def git_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"], capture_output=True,
            text=True, cwd=Path(__file__).parents[2]).stdout.strip()
    except Exception:
        return "unknown"


def tensor_hash(model) -> str:
    h = hashlib.sha256()
    for name, value in model.state_dict().items():
        h.update(name.encode())
        h.update(value.detach().cpu().numpy().tobytes())
    return h.hexdigest()


def load_policy(ckpt_path: str, env_cfg: C0Config, device="cpu"):
    ck = torch.load(ckpt_path, map_location=device, weights_only=False)
    policy = build_policy(ck["model_name"], env_cfg)
    policy.load_state_dict(ck["model"])
    policy.to(device).eval()
    return policy, ck


def build_runner(env_cfg: C0Config, *, checkpoint: str | None = None,
                 heuristic: str | None = None, device: str = "cpu",
                 model_override: str | None = None):
    """Returns (runner, meta) for either a learned checkpoint or a heuristic."""
    if heuristic is not None:
        assert heuristic in HEURISTICS
        return HeuristicRunner(heuristic, env_cfg), {"agent": f"heur_{heuristic}"}
    ckpt = model_override or checkpoint
    policy, ck = load_policy(ckpt, env_cfg, device)
    runner = Runner(policy, ck["model_name"], env_cfg, device)
    meta = {"agent": ck["model_name"], "checkpoint": str(ckpt),
            "model_sha256": ck.get("model_sha256", tensor_hash(policy)),
            "train_seed": ck.get("seed")}
    if model_override:
        meta["model_override"] = model_override
    return runner, meta
