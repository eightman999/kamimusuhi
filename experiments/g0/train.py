"""Train a G0 representation model (self-supervised only).

Objectives: next-observation prediction (gru, gru_vq) or masked/plain
reconstruction (ae, ae_vq). Cause/context labels are NEVER used.

Usage:
    python -m experiments.g0.train --model gru_vq --seed 0
    G0_TIME_BUDGET=30 python -m experiments.g0.train --model gru --seed 0 \
        --max-steps 200

Artifacts land in runs/<model>__seed<seed>/:
    ckpt.pt        final weights + spec + config + metadata
    ckpt_best.pt   best-val checkpoint
    metrics.json   losses/history
    run.json       seed, git commit, device, steps, wall time
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np
import torch

from .config import Config, load_config
from .data import train_val_datasets
from .models import build_model, model_spec

RUNS_DIR = Path(__file__).parent / "runs"


def git_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, cwd=Path(__file__).parent,
        ).stdout.strip()
    except Exception:
        return "unknown"


def to_tensors(ds: dict, device) -> tuple[torch.Tensor, ...]:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    nxt = torch.as_tensor(ds["next_obs"], dtype=torch.float32, device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    return obs, nxt, act


def batch_loss(model, obs, nxt, act) -> torch.Tensor:
    pred, _, aux = model(obs, act)
    target = obs if model.kind == "recon" else nxt
    loss = torch.nn.functional.mse_loss(pred, target)
    if "loss_extra" in aux:
        loss = loss + aux["loss_extra"]
    return loss


def train_one(model_name: str, seed: int, cfg: Config, device: str = "cpu",
              out_dir: Path | None = None, time_budget: float = 0.0,
              max_steps: int | None = None, quiet: bool = False,
              datasets: tuple[dict, dict] | None = None) -> dict:
    torch.manual_seed(seed)
    np.random.seed(seed)

    if datasets is None:
        train_ds, val_ds = train_val_datasets(cfg, seed)
    else:
        train_ds, val_ds = datasets
    env_seed = int(train_ds["env_seed"])

    model = build_model(model_name, cfg.env.obs_dim, 4, cfg.model,
                        cfg.train.target_delta).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=cfg.train.lr)

    obs, nxt, act = to_tensors(train_ds, device)
    vobs, vnxt, vact = to_tensors(val_ds, device)
    n_ep = obs.shape[0]
    bs = min(cfg.train.batch_size, n_ep)

    max_steps = max_steps or cfg.train.max_steps
    budget = float(os.environ.get("G0_TIME_BUDGET", 0)) \
        or cfg.train.time_budget_sec or time_budget
    t0 = time.time()
    step = 0
    hist = []
    best_val = np.inf
    best_state = None
    best_step = 0
    gen = torch.Generator().manual_seed(seed)
    while step < max_steps:
        idx = torch.randint(0, n_ep, (bs,), generator=gen).to(device)
        model.train()
        loss = batch_loss(model, obs[idx], nxt[idx], act[idx])
        opt.zero_grad()
        loss.backward()
        opt.step()
        step += 1
        if step % 50 == 0 or step == max_steps:
            model.eval()
            with torch.no_grad():
                vloss = batch_loss(model, vobs, vnxt, vact).item()
            hist.append({"step": step, "train": loss.item(),
                         "val": vloss})
            if vloss < best_val:
                best_val = vloss
                best_step = step
                best_state = {k: v.detach().clone()
                              for k, v in model.state_dict().items()}
            if not quiet:
                print(f"[{model_name} s{seed}] step {step} "
                      f"train {loss.item():.5f} val {vloss:.5f}")
        if budget and time.time() - t0 > budget:
            break

    # restore best-val weights: val shares env dynamics; selection
    # guards against late-training episode memorization, so the *best*
    # checkpoint is the meaningful model to evaluate
    if best_state is not None:
        model.load_state_dict(best_state)
    model.eval()
    with torch.no_grad():
        val_loss = batch_loss(model, vobs, vnxt, vact).item()

    wall = time.time() - t0
    if out_dir is not None:
        out_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "model_name": model_name,
            "obs_dim": cfg.env.obs_dim,
            "n_actions": 4,
            "seed": seed,
            "env_seed": env_seed,
            "model_spec": model_spec(model_name, cfg.model,
                                     cfg.train.target_delta),
            "config": cfg,
        }
        torch.save({**payload, "state_dict": model.state_dict()},
                   out_dir / "ckpt.pt")
        if best_state is not None:
            torch.save({**payload, "state_dict": best_state},
                       out_dir / "ckpt_best.pt")
        with open(out_dir / "metrics.json", "w") as f:
            json.dump({"val_loss": val_loss, "best_val_loss": best_val,
                       "best_step": best_step, "steps": step,
                       "wall_sec": wall, "history": hist}, f)
        with open(out_dir / "run.json", "w") as f:
            json.dump({"model": model_name, "seed": seed,
                       "env_seed": env_seed, "git": git_commit(),
                       "device": device, "steps": step,
                       "best_step": best_step,
                       "wall_sec": wall,
                       "config_name": cfg.name}, f, indent=2)

    return {"model": model, "val_loss": val_loss, "best_val": best_val,
            "steps": step, "train_ds": train_ds, "val_ds": val_ds,
            "env_seed": env_seed, "wall_sec": wall}


def load_ckpt(path: str | Path, device: str = "cpu"):
    ck = torch.load(path, map_location=device, weights_only=False)
    cfg = ck["config"]
    if not isinstance(cfg, Config):  # pickled older configs
        cfg = load_config(cfg)
    model = build_model(ck["model_name"], ck["obs_dim"], ck["n_actions"],
                        cfg.model, ck["model_spec"]["target_delta"])
    model.load_state_dict(ck["state_dict"])
    model.eval().to(device)
    return model, ck


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--model", required=True)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--max-steps", type=int, default=None)
    ap.add_argument("--time-budget", type=float, default=0.0)
    ap.add_argument("--out", default=None)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    cfg = load_config(args.config)
    out = Path(args.out) if args.out else \
        RUNS_DIR / f"{args.model}__seed{args.seed}"
    res = train_one(args.model, args.seed, cfg, args.device, out,
                    args.time_budget, args.max_steps, args.quiet)
    print(f"done: val_loss={res['val_loss']:.5f} steps={res['steps']} "
          f"-> {out}")


if __name__ == "__main__":
    main()
