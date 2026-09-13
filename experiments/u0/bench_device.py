"""Device benchmark for U0: ~100 PPO updates on cpu vs mps (if present),
reporting updates/sec, env steps/sec, wall time, peak memory, and loss
parity. Used to pick --device for the overnight sweep: MPS is adopted
only if it is >= 1.2x faster than CPU.

    python -m experiments.u0.bench_device \
        --config experiments/u0/configs/smoke.yaml --updates 100
"""

from __future__ import annotations

import argparse
import resource
import sys
import time

import numpy as np
import torch

from .config import (env_config, load_config, resolve_device,
                     train_config)
from .env.u0_env import VecU0Env
from .models.nets import build_policy
from .train import collect_rollout, ppo_update


def _peak_memory_mb(device: str) -> float:
    """Peak RSS (cpu) or currently-allocated device memory (mps), in MB."""
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    peak /= 1e6 if sys.platform == "darwin" else 1024.0
    if device == "mps" and torch.backends.mps.is_available():
        peak = max(peak, torch.mps.current_allocated_memory() / 1e6)
    return peak


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
    env_steps = updates * tc["num_envs"] * ecfg.episode_len
    return {"device": device, "updates": updates,
            "wall_sec": round(wall, 2),
            "updates_per_sec": round(updates / wall, 3),
            "env_steps_per_sec": round(env_steps / wall, 1),
            "peak_memory_mb": round(_peak_memory_mb(device), 1),
            "final_pi_loss": losses[-1]["pi_loss"],
            "final_v_loss": losses[-1]["v_loss"]}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config",
                    default="experiments/u0/configs/smoke.yaml")
    ap.add_argument("--updates", type=int, default=100)
    ap.add_argument("--model", default="gru64")
    ap.add_argument("--devices", nargs="+", default=None)
    args = ap.parse_args()
    cfg = load_config(args.config)
    requested = args.devices or ["cpu"] + (
        ["mps"] if torch.backends.mps.is_available() else [])
    devices = list(dict.fromkeys(resolve_device(d) for d in requested))
    results = [bench_device(d, cfg, args.updates, args.model)
               for d in devices]
    for r in results:
        print(r)
    by_dev = {r["device"]: r for r in results}
    if "cpu" in by_dev and "mps" in by_dev:
        dl = abs(by_dev["cpu"]["final_pi_loss"]
                 - by_dev["mps"]["final_pi_loss"])
        speedup = (by_dev["mps"]["updates_per_sec"]
                   / by_dev["cpu"]["updates_per_sec"])
        print(f"pi_loss parity |cpu-mps| = {dl:.2e}")
        verdict = ("ADOPT mps" if speedup >= 1.2
                   else "CPU fixed (mps < 1.2x)")
        print(f"mps/cpu speedup = {speedup:.2f}x -> {verdict}")


if __name__ == "__main__":
    main()
