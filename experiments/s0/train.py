"""Train a dynamics model for S0 (self-supervised next-obs prediction).

Usage:
    python train.py --model gru_state_action --seed 0
    S0_TIME_BUDGET=60 python train.py --model mlp_state_action --seed 0

Artifacts land in runs/<model>__seed<seed>/:
    ckpt.pt     model weights + spec + config
    metrics.json  final train/val losses
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).parent))

from data import collect_dataset, load_config, make_env_config  # noqa: E402
from models import build_model  # noqa: E402

RUNS_DIR = Path(__file__).parent / "runs"


def to_tensors(ds: dict, device) -> tuple[torch.Tensor, ...]:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    nxt = torch.as_tensor(ds["next_obs"], dtype=torch.float32, device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    return obs, nxt, act


def train_one(model_name: str, seed: int, cfg: dict, device: str = "cpu",
              out_dir: Path | None = None,
              time_budget: float = 0.0,
              max_steps: int | None = None,
              quiet: bool = False) -> dict:
    torch.manual_seed(seed)
    np.random.seed(seed)

    env_cfg = make_env_config(cfg["env"])
    train_ds = collect_dataset(env_cfg, cfg["data"]["episodes"], seed,
                               cfg["data"]["policy"])
    val_ds = collect_dataset(env_cfg, cfg["data"]["val_episodes"],
                             seed + 777, cfg["data"]["policy"],
                             env_seed=int(train_ds["env_seed"]))

    model = build_model(model_name, env_cfg.obs_dim, 4,
                        cfg["train"]["target_delta"]).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=cfg["train"]["lr"])

    obs, nxt, act = to_tensors(train_ds, device)
    vobs, vnxt, vact = to_tensors(val_ds, device)
    n_ep = obs.shape[0]
    bs = cfg["train"]["batch_size"]
    loss_fn = torch.nn.MSELoss()

    max_steps = max_steps or cfg["train"]["max_steps"]
    budget = float(os.environ.get("S0_TIME_BUDGET", 0)) or \
        cfg["train"]["time_budget_sec"] or time_budget
    t0 = time.time()
    step = 0
    hist = []
    gen = torch.Generator().manual_seed(seed)
    while step < max_steps:
        idx = torch.randint(0, n_ep, (bs,), generator=gen).to(device)
        model.train()
        pred, _, _ = model(obs[idx], act[idx])
        loss = loss_fn(pred, nxt[idx])
        opt.zero_grad()
        loss.backward()
        opt.step()
        step += 1
        if step % 100 == 0 or step == max_steps:
            model.eval()
            with torch.no_grad():
                vpred, _, _ = model(vobs, vact)
                vloss = loss_fn(vpred, vnxt).item()
            hist.append({"step": step, "train": loss.item(), "val": vloss})
            if not quiet:
                print(f"[{model_name} s{seed}] step {step} "
                      f"train {loss.item():.5f} val {vloss:.5f}")
        if budget and time.time() - t0 > budget:
            break

    model.eval()
    with torch.no_grad():
        vpred, _, _ = model(vobs, vact)
        val_loss = loss_fn(vpred, vnxt).item()

    if out_dir is not None:
        out_dir.mkdir(parents=True, exist_ok=True)
        torch.save({
            "state_dict": model.state_dict(),
            "model_name": model_name,
            "obs_dim": env_cfg.obs_dim,
            "n_actions": 4,
            "target_delta": cfg["train"]["target_delta"],
            "seed": seed,
            "env_seed": int(train_ds["env_seed"]),
            "config": cfg,
        }, out_dir / "ckpt.pt")
        with open(out_dir / "metrics.json", "w") as f:
            json.dump({"val_loss": val_loss, "steps": step,
                       "wall_sec": time.time() - t0, "history": hist}, f)

    return {"model": model, "val_loss": val_loss, "steps": step,
            "train_ds": train_ds, "val_ds": val_ds}


def load_ckpt(path: str | Path, device: str = "cpu"):
    ck = torch.load(path, map_location=device, weights_only=False)
    model = build_model(ck["model_name"], ck["obs_dim"], ck["n_actions"],
                        ck["target_delta"]).to(device)
    model.load_state_dict(ck["state_dict"])
    model.eval()
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
    args = ap.parse_args()

    cfg = load_config(args.config)
    out = Path(args.out) if args.out else \
        RUNS_DIR / f"{args.model}__seed{args.seed}"
    res = train_one(args.model, args.seed, cfg, args.device, out,
                    args.time_budget, args.max_steps)
    print(f"done: val_loss={res['val_loss']:.5f} steps={res['steps']} "
          f"-> {out}")


if __name__ == "__main__":
    main()
