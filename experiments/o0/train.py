"""Supervised training for O0 object-permanence predictors.

The task is pure prediction, so instead of RL we do multi-task
supervised learning on freshly generated episodes (online samples, no
dataset reuse):

    pos head   : MSE on (x, v) at steps where the object exists
                 (extra weight on occluded steps -- the core skill)
    exist head : BCE at every step (0 before appearance, 1 while the
                 object is in the world -- including invisibly, 0 after
                 boundary absorption)
    id head    : BCE on visible steps after the first occlusion
                 ("is the object you see now the one that disappeared?")

Artifacts per run dir under <artifacts>/runs/<run_id>/:
    config.json, status.json, metrics.jsonl, initial.pt, best.pt,
    final.pt, selection.json
"""

from __future__ import annotations

import argparse
import copy
import dataclasses
import hashlib
import json
import os
import random
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from .analysis import metrics as metrics_mod
from .config import Config, load_config
from .env import dynamics as dyn
from .evaluate import evaluate_model
from .models import make_model


def atomic(path, obj):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(
        json.dumps(metrics_mod.json_safe(obj), indent=2, allow_nan=False)
        + "\n"
    )
    tmp.replace(path)


def tensor_hash(model):
    h = hashlib.sha256()
    for name, value in model.state_dict().items():
        h.update(name.encode())
        h.update(value.detach().cpu().numpy().tobytes())
    return h.hexdigest()


def git_commit():
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"


def save_checkpoint(path, model, optimizer, cfg: Config, update, **extra):
    p = Path(path)
    tmp = p.with_suffix(".tmp")
    torch.save(
        dict(
            model=model.state_dict(),
            optimizer=optimizer.state_dict() if optimizer else None,
            config=dataclasses.asdict(cfg.env) | {"train": dataclasses.asdict(cfg.train)},
            model_spec=dataclasses.asdict(cfg.model),
            seed=cfg.seed,
            git=git_commit(),
            update=update,
            model_sha256=tensor_hash(model),
            **extra,
        ),
        tmp,
    )
    tmp.replace(p)


def batch_to_torch(data, device):
    out = {}
    for k in ("obs", "exist", "pos", "vel", "same"):
        out[k] = torch.as_tensor(data[k], dtype=torch.float32, device=device)
    for k in ("occluded", "visible", "id_mask"):
        out[k] = torch.as_tensor(data[k], dtype=torch.bool, device=device)
    return out


def supervised_loss(model, data, cfg: Config, device):
    """One whole-sequence update on a fresh batch of episodes."""
    b = batch_to_torch(data, device)
    state = model.initial_state(b["obs"].shape[1], device)
    pos, exist, idl, _ = model.forward_sequence(b["obs"], state)

    exists = b["exist"] > 0.5
    # position/velocity: supervised wherever the object is in the world
    w = torch.where(b["occluded"], cfg.train.w_occ, 0.0)
    w = torch.where(b["visible"], torch.full_like(w, cfg.train.w_vis), w)
    pos_sq = (pos[..., 0] - b["pos"]).square() + cfg.train.w_vel * (
        pos[..., 1] - b["vel"]
    ).square()
    pos_loss = (pos_sq * w).sum() / w.sum().clamp_min(1.0)

    exist_loss = nn.functional.binary_cross_entropy_with_logits(
        exist, b["exist"]
    )
    idm = b["id_mask"]
    if bool(idm.any()):
        # "different" (same=0) is the minority class; balance classes so
        # the head cannot collapse onto the always-same prior
        same = b["same"][idm]
        per = nn.functional.binary_cross_entropy_with_logits(
            idl[idm], same, reduction="none"
        )
        frac_same = same.mean().clamp(1e-3, 1.0 - 1e-3)
        w_same = (1.0 - frac_same) / frac_same
        cw = torch.where(same > 0.5, w_same.expand_as(same),
                         torch.ones_like(same))
        id_loss = (per * cw).sum() / cw.sum().clamp_min(1.0)
    else:
        id_loss = torch.zeros((), device=device)
    total = (
        cfg.train.w_pos * pos_loss
        + cfg.train.w_exist * exist_loss
        + cfg.train.w_id * id_loss
    )
    return total, {
        "pos_loss": float(pos_loss.detach()),
        "exist_loss": float(exist_loss.detach()),
        "id_loss": float(id_loss.detach()),
    }


def validation(model, cfg: Config, n_episodes):
    """Fixed validation distribution (separate seed stream)."""
    return evaluate_model(
        model, cfg.env, n_episodes, seed=700001, device=cfg.train.device
    )["metrics"]


def select_score(val: dict) -> float:
    """Checkpoint-selection score: tracking + existence + identity.

    Non-finite metric values (e.g. NaN from an empty mask) are treated as
    worst-case so a degenerate validation can never win selection."""

    def finite(key, worst):
        v = val.get(key)
        if v is None or not np.isfinite(v):
            return worst
        return float(v)

    return (
        -10.0 * finite("pos_mae_occluded", 0.5)
        + finite("exist_acc_hidden", 0.0)
        + finite("id_acc", 0.0)
        - 2.0 * finite("pos_mae_visible", 0.5)
    )


def train(cfg: Config, artifacts: str | Path, run_id: str) -> Path:
    torch.set_num_threads(cfg.train.threads)
    seed = cfg.seed
    random.seed(seed)
    np.random.seed(seed)
    torch.manual_seed(seed)
    device = cfg.train.device

    directory = Path(artifacts) / "runs" / run_id
    if (directory / "status.json").exists():
        raise ValueError("Refusing to overwrite existing run: " + run_id)
    directory.mkdir(parents=True, exist_ok=True)

    cfg_dict = {
        "name": cfg.name,
        "seed": cfg.seed,
        "env": dataclasses.asdict(cfg.env),
        "train": dataclasses.asdict(cfg.train),
        "eval": dataclasses.asdict(cfg.eval),
        "model": dataclasses.asdict(cfg.model),
        "source_commit": git_commit(),
    }
    atomic(directory / "config.json", cfg_dict)
    status = dict(run_id=run_id, arch=cfg.model.arch, seed=seed,
                  status="running", pid=os.getpid(), started_at=time.time())

    def state(**kw):
        status.update(kw, timestamp=time.time())
        atomic(directory / "status.json", status)

    state()
    model = make_model(cfg.model.arch, hidden=cfg.model.hidden).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=cfg.train.lr)
    train_rng = np.random.default_rng(100000 + seed)

    save_checkpoint(directory / "initial.pt", model, opt, cfg, 0)
    val0 = validation(model, cfg, cfg.train.val_episodes)
    atomic(directory / "initial_validation.json", val0)
    best = -float("inf")
    t_start = time.time()

    try:
        for u in range(1, cfg.train.updates + 1):
            t0 = time.perf_counter()
            data = dyn.generate_batch(cfg.env, cfg.train.batch_size, train_rng)
            loss, parts = supervised_loss(model, data, cfg, device)
            opt.zero_grad(set_to_none=True)
            loss.backward()
            norm = nn.utils.clip_grad_norm_(model.parameters(), cfg.train.grad_clip)
            opt.step()
            row = dict(update=u, losses=parts,
                       grad_norm=float(norm),
                       elapsed=time.perf_counter() - t0)
            if u % cfg.train.val_every == 0 or u == cfg.train.updates:
                model.eval()
                val = validation(model, cfg, cfg.train.val_episodes)
                model.train()
                row["validation"] = val
                row["score"] = select_score(val)
                save_checkpoint(directory / "final.pt", model, opt, cfg, u,
                                validation=val)
                if row["score"] > best:
                    best = row["score"]
                    import shutil

                    shutil.copyfile(directory / "final.pt",
                                    directory / "best.pt")
                    atomic(directory / "selection.json",
                           dict(update=u, score=best,
                                criterion="-10*pos_mae_occ + exist + id "
                                          "- 2*pos_mae_vis"))
                state(update=u, validation=val,
                      wall_time_s=round(time.time() - t_start, 1))
            with (directory / "metrics.jsonl").open("a") as f:
                f.write(
                    json.dumps(
                        metrics_mod.json_safe(row), allow_nan=False,
                        default=float,
                    )
                    + "\n"
                )
            if u % max(1, cfg.train.val_every) == 0 or u <= 3:
                print(json.dumps(dict(run_id=run_id, **row), default=float),
                      flush=True)
        state(status="complete", finished_at=time.time(),
              wall_time_s=round(time.time() - t_start, 1))
    except BaseException as exc:
        state(status="failed", error=str(exc))
        raise
    return directory


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--artifacts", default="experiments/o0/artifacts")
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--arch", default=None)
    ap.add_argument("--updates", type=int, default=None)
    ap.add_argument("--batch-size", type=int, default=None)
    args = ap.parse_args()
    cfg = load_config(args.config)
    if args.seed is not None:
        cfg.seed = args.seed
    if args.arch:
        cfg.model.arch = args.arch
    if args.updates:
        cfg.train.updates = args.updates
    if args.batch_size:
        cfg.train.batch_size = args.batch_size
    train(cfg, args.artifacts, args.run_id)


if __name__ == "__main__":
    main()
