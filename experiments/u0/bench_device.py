"""Device benchmark for U0: ~100 PPO updates on cpu vs mps (if present),
reporting updates/sec, wall time, and loss parity. Used to pick --device
for the overnight sweep.

    python -m experiments.u0.bench_device \
        --config experiments/u0/configs/smoke.yaml --updates 8
"""

from __future__ import annotations

import argparse
import time

import numpy as np
import torch

from .config import env_config, load_config, train_config
from .env.u0_env import VecU0Env
from .models.nets import build_policy
from .train import collect_rollout, ppo_update


def bench_device(device: str, cfg: dict, updates: int,
                 model_name: str = "gru64") -> dict:
    ecfg = env_config(cfg, seed=0)
    tc = train_config(cfg)
    dev = torch.device(device)
    torch.manual_seed(0)
    np.random.seed(0)
    vec = VecU0Env(ecfg, tc["num_envs"], seed=13)
    policy = build_policy(model_name, vec.obs_dim).to(dev)
    opt = torch.optim.Adam(policy.parameters(), lr=tc["lr"])
    losses = []
    t0 = time.time()
    for _ in range(updates):
        bufs = collect_rollout(policy, vec, dev)
        losses.append(ppo_update(policy, opt,
                                 (*bufs[:5], bufs[7]), tc, dev))
    wall = time.time() - t0
    return {"device": device, "updates": updates,
            "wall_sec": round(wall, 2),
            "updates_per_sec": round(updates / wall, 3),
            "final_pi_loss": losses[-1]["pi_loss"],
            "final_v_loss": losses[-1]["v_loss"]}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config",
                    default="experiments/u0/configs/smoke.yaml")
    ap.add_argument("--updates", type=int, default=8)
    ap.add_argument("--model", default="gru64")
    ap.add_argument("--devices", nargs="+", default=None)
    args = ap.parse_args()
    cfg = load_config(args.config)
    devices = args.devices or ["cpu"] + (
        ["mps"] if torch.backends.mps.is_available() else [])
    results = [bench_device(d, cfg, args.updates, args.model)
               for d in devices]
    for r in results:
        print(r)
    if len(results) == 2:
        dl = abs(results[0]["final_pi_loss"] - results[1]["final_pi_loss"])
        print(f"pi_loss parity |cpu-mps| = {dl:.2e}")


if __name__ == "__main__":
    main()
