"""G0-v4 trainer — self-supervised objectives only.

Artifacts per run dir (v4 spec §23):

    latest.pt      most recent weights (+ spec + cfg)
    best.pt        best-val checkpoint
    config.yaml    resolved config
    meta.json      seed, git, device/GPU, batch size, OOM retries,
                   data keys seen by the trainer (label-access audit)
    metrics.jsonl  appended every `log_every` updates: step, loss terms,
                   val loss, latent health summary

Fault tolerance: on CUDA OOM the batch size is halved and retried ONCE;
a second OOM marks the run failed. Silent CPU fallback is forbidden —
when --device=cuda* is requested, a non-CUDA run aborts, and meta.json
records the actual device used.

Usage:
    python -m experiments.g0_v4.train --method cpc --seed 0 --device cuda
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

from experiments.g0.data import train_val_datasets
from experiments.g0.representations import TorchRep

from .config import Config, load_config, save_config
from .health import representation_health
from .models import MODEL_REGISTRY_V4, build_model, compute_loss, \
    model_spec

RUNS_DIR = Path(__file__).parent / "runs"

# keys the trainer is allowed to see — everything else in the dataset
# dict is eval-only metadata (cause ids, ctx ids, canonical, ...)
TRAIN_KEYS = ("obs", "next_obs", "actions")


def git_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, cwd=Path(__file__).parent,
        ).stdout.strip()
    except Exception:
        return "unknown"


def sanitize(ds: dict) -> dict:
    """Strip eval-only labels before the trainer touches the data."""
    return {k: ds[k] for k in TRAIN_KEYS}


def to_tensors(ds: dict, device) -> tuple[torch.Tensor, ...]:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    nxt = torch.as_tensor(ds["next_obs"], dtype=torch.float32,
                          device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    return obs, nxt, act


def _ckpt_payload(method: str, cfg: Config, seed: int, env_seed: int,
                  model) -> dict:
    return {"model_name": method, "obs_dim": cfg.env.obs_dim,
            "n_actions": 4, "seed": seed, "env_seed": env_seed,
            "model_spec": model_spec(method, cfg), "config": cfg,
            "v4": True}


def train_one(method: str, seed: int, cfg: Config, device: str = "cpu",
              out_dir: Path | None = None, time_budget: float = 0.0,
              max_steps: int | None = None, quiet: bool = False,
              datasets: tuple[dict, dict] | None = None,
              run_name: str | None = None) -> dict:
    if method not in MODEL_REGISTRY_V4:
        raise KeyError(f"unknown v4 method {method!r}")
    run_name = run_name or method
    if device.startswith("cuda"):
        if not torch.cuda.is_available():
            raise RuntimeError(
                f"--device={device} requested but CUDA unavailable — "
                "silent CPU fallback is forbidden by the v4 spec")
        dev = torch.device(device)
        if dev.index is None:
            dev = torch.device("cuda", 0)
            device = str(dev)
        torch.cuda.set_device(dev)

    torch.manual_seed(seed)
    np.random.seed(seed)

    if datasets is None:
        train_ds, val_ds = train_val_datasets(cfg, seed)
    else:
        train_ds, val_ds = datasets
    env_seed = int(train_ds["env_seed"])
    # label firewall: the trainer only ever sees obs/next_obs/actions
    train_s = sanitize(train_ds)
    val_s = sanitize(val_ds)

    model = build_model(method, cfg, cfg.env.obs_dim, 4).to(device)
    params = [p for p in model.parameters() if p.requires_grad]
    opt = torch.optim.Adam(params, lr=cfg.train.lr)

    obs, nxt, act = to_tensors(train_s, device)
    vobs, vnxt, vact = to_tensors(val_s, device)
    n_ep = obs.shape[0]
    bs = min(cfg.train.batch_size, n_ep)

    max_steps = max_steps or cfg.train.max_steps
    budget = float(os.environ.get("G0V4_TIME_BUDGET", 0)) \
        or cfg.train.time_budget_sec or time_budget
    log_every = max(1, int(cfg.v4.log_every))

    if out_dir is not None:
        out_dir.mkdir(parents=True, exist_ok=True)
        save_config(cfg, out_dir / "config.yaml")
    metrics_path = out_dir / "metrics.jsonl" if out_dir else None
    meta = {
        "method": method, "run": run_name, "seed": seed,
        "env_seed": env_seed,
        "git": git_commit(), "device": device,
        "gpu_name": (torch.cuda.get_device_name(device)
                     if device.startswith("cuda") else None),
        "torch": torch.__version__,
        "cuda": torch.version.cuda if device.startswith("cuda") else None,
        "data_keys": list(train_s.keys()),
        "eval_only_keys_stripped": sorted(set(train_ds) - set(train_s)),
        "oom_retries": 0, "status": "running",
    }

    t0 = time.time()
    step = 0
    best_val = np.inf
    best_state = None
    best_step = 0
    oom_seen = 0
    gen = torch.Generator(device="cpu").manual_seed(seed)
    mj = open(metrics_path, "a") if metrics_path else None
    try:
        while step < max_steps:
            idx = torch.randint(0, n_ep, (bs,), generator=gen).to(device)
            try:
                model.train()
                loss, logs = compute_loss(model, method, obs[idx],
                                          nxt[idx], act[idx], cfg, gen)
                opt.zero_grad()
                loss.backward()
                opt.step()
                if hasattr(model, "ema_update"):
                    model.ema_update()
            except torch.cuda.OutOfMemoryError:
                oom_seen += 1
                meta["oom_retries"] = oom_seen
                if oom_seen > 1:
                    raise RuntimeError("second OOM — run FAIL")
                bs = max(1, bs // 2)
                torch.cuda.empty_cache()
                if not quiet:
                    print(f"[{method} s{seed}] OOM -> batch halved to "
                          f"{bs} (retry {oom_seen}/1)", flush=True)
                continue
            step += 1
            if step % log_every == 0 or step == max_steps:
                model.eval()
                with torch.no_grad():
                    vloss, _ = compute_loss(model, method, vobs, vnxt,
                                            vact, cfg, gen)
                    _, zlat, _ = model(vobs, vact)
                h = representation_health(zlat.cpu().numpy())
                rec = {"step": step, "train": float(loss.item()),
                       "val": float(vloss.item()),
                       "var_mean": h["var_mean"],
                       "eff_rank": h["effective_rank"],
                       "cos_abs": h["cos_abs_mean"], **logs}
                if mj:
                    mj.write(json.dumps(rec) + "\n")
                    mj.flush()
                if vloss.item() < best_val:
                    best_val = vloss.item()
                    best_step = step
                    best_state = {k: v.detach().clone()
                                  for k, v in model.state_dict().items()}
                    if out_dir is not None:
                        torch.save({**_ckpt_payload(method, cfg, seed,
                                                    env_seed, model),
                                    "state_dict": best_state},
                                   out_dir / "best.pt")
                if out_dir is not None:
                    torch.save({**_ckpt_payload(method, cfg, seed,
                                                env_seed, model),
                                "state_dict": model.state_dict()},
                               out_dir / "latest.pt")
                if not quiet:
                    print(f"[{method} s{seed}] step {step} "
                          f"train {loss.item():.4f} val {vloss:.4f} "
                          f"rank {h['effective_rank']:.1f}", flush=True)
            if budget and time.time() - t0 > budget:
                break
    finally:
        if mj:
            mj.close()

    if best_state is not None:
        model.load_state_dict(best_state)
    model.eval()
    with torch.no_grad():
        vloss, _ = compute_loss(model, method, vobs, vnxt, vact, cfg, gen)
        val_loss = float(vloss.item())

    wall = time.time() - t0
    meta.update({"status": "ok", "steps": step, "best_step": best_step,
                 "best_val_loss": best_val, "val_loss": val_loss,
                 "batch_size_final": bs, "wall_sec": wall})
    if out_dir is not None:
        with open(out_dir / "meta.json", "w") as f:
            json.dump(meta, f, indent=2)

    return {"model": model, "val_loss": val_loss, "best_val": best_val,
            "steps": step, "train_ds": train_ds, "val_ds": val_ds,
            "env_seed": env_seed, "wall_sec": wall, "meta": meta}


def load_ckpt(path: str | Path, device: str = "cpu"):
    ck = torch.load(path, map_location=device, weights_only=False)
    cfg = ck["config"]
    if not isinstance(cfg, Config):
        cfg = load_config(cfg)
    model = build_model(ck["model_name"], cfg, ck["obs_dim"],
                        ck["n_actions"])
    model.load_state_dict(ck["state_dict"])
    model.eval().to(device)
    return model, ck


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--method", required=True,
                    choices=list(MODEL_REGISTRY_V4))
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--max-steps", type=int, default=None)
    ap.add_argument("--time-budget", type=float, default=0.0)
    ap.add_argument("--out", default=None)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    cfg = load_config(args.config)
    out = Path(args.out) if args.out else \
        RUNS_DIR / f"{args.method}__seed{args.seed}"
    res = train_one(args.method, args.seed, cfg, args.device, out,
                    args.time_budget, args.max_steps, args.quiet)
    print(f"done: val_loss={res['val_loss']:.5f} steps={res['steps']} "
          f"-> {out}")


if __name__ == "__main__":
    main()
