"""Evaluation for R0: learned checkpoints and heuristic baselines.

Runs a fixed, CPU-generated evaluation corpus (seed 900001 by default,
held out from all training and validation seeds) and reports the primary
metrics: task accuracy, important-item retention, store precision /
irrelevant retention, and memory efficiency.

Causal tests (--causal):
    erase    R-C2: clear memory the moment each QUERY event fires
    shuffle  R-C1: swap memory contents with a parallel donor episode
    permute  R-C3: permute slot order at query time
    noise    R-C4: regenerate env with high noise + extra distractors

OOD (--ood): delay96, delay128 (longer episodes), highnoise, slots2.

Usage:
    python -m experiments.r0.evaluate --checkpoint RUN/best.pt --episodes 256
    python -m experiments.r0.evaluate --baseline fifo --episodes 256
"""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from pathlib import Path

import numpy as np
import torch
import yaml

from .agents.baselines import BASELINES, FIFOGate
from .agents.gates import build_policy
from .env.r0_env import QUERY, R0Config, R0Env

OOD_OVERRIDES = {
    "delay96": {"delay_min": 96, "delay_max": 96, "episode_len": 192},
    "delay128": {"delay_min": 128, "delay_max": 128, "episode_len": 224},
    "highnoise": {"noise_rate": 0.9, "num_distractors": 24},
    "slots2": {"memory_slots": 2},
}


def _apply_causal(env: R0Env, donor: R0Env | None, causal: str | None,
                  rng: np.random.Generator) -> None:
    """Fire the causal manipulation at the moment a QUERY event opens."""
    if causal is None:
        return
    e = env.schedule[env.t]
    if e.kind != QUERY or env.pending is not None:
        return
    if causal == "erase":
        env.erase_memory()
    elif causal == "permute":
        env.permute_memory(rng)
    elif causal == "shuffle" and donor is not None:
        env.swap_memory(donor.memory.clone_state())


def _mean_stats(records: list[dict]) -> dict:
    keys = records[0].keys()
    return {k: float(np.mean([r[k] for r in records])) for k in keys}


def evaluate_learned(policy, cfg: R0Config, device, episodes: int = 256,
                     seed: int = 900001, causal: str | None = None,
                     batch: int = 32) -> dict:
    policy.eval()
    envs = [R0Env(R0Config(**{**cfg.__dict__, "seed": seed + i}))
          for i in range(batch)]
    donors = None
    donor_gates = None
    if causal == "shuffle":
        donors = [R0Env(R0Config(**{**cfg.__dict__, "seed": seed + 500000 + i}))
                  for i in range(batch)]
        donor_gates = [FIFOGate(seed=seed + i) for i in range(batch)]
    rng = np.random.default_rng(seed + 999)
    records = []
    for ep0 in range(0, episodes, batch):
        for e in envs:
            e.reset()
        if donors:
            for d, g in zip(donors, donor_gates):
                d.reset()
                g.reset()
        h = policy.initial_state(batch, device)
        T = cfg.episode_len
        with torch.no_grad():
            for _t in range(T):
                for i, e in enumerate(envs):
                    _apply_causal(e, donors[i] if donors else None, causal, rng)
                if donors:
                    for d, g in zip(donors, donor_gates):
                        a, ans = g.decide(d)
                        d.step(a, ans)
                obs = np.stack([e._obs() for e in envs])
                al, nl, _v, h = policy(torch.as_tensor(obs, device=device), h)
                act = al.argmax(-1).cpu().numpy()     # greedy eval
                ans = nl.argmax(-1).cpu().numpy()
                for i, e in enumerate(envs):        # forced answer at deadline
                    if e.pending is not None and e.t == e.pending["deadline"]:
                        act[i] = 3                  # ANSWER
                for i, e in enumerate(envs):
                    _o, _r, done, info = e.step(int(act[i]), int(ans[i]))
                    if done and "ep_stats" in info:
                        records.append(info["ep_stats"])
    return _mean_stats(records)


def evaluate_baseline(name: str, cfg: R0Config, episodes: int = 256,
                      seed: int = 900001, causal: str | None = None) -> dict:
    gate = BASELINES[name](seed=seed)
    env = R0Env(R0Config(**{**cfg.__dict__, "seed": seed,
                           "evict_policy": gate.evict_policy}))
    donor = donor_gate = None
    if causal == "shuffle":
        donor = R0Env(R0Config(**{**cfg.__dict__, "seed": seed + 500000}))
        donor_gate = FIFOGate(seed=seed)
    rng = np.random.default_rng(seed + 999)
    records = []
    for ep in range(episodes):
        env.reset()
        gate.reset()
        if donor is not None:
            donor.reset()
            donor_gate.reset()
        for _t in range(cfg.episode_len):
            _apply_causal(env, donor, causal, rng)
            if donor is not None:
                a, ans = donor_gate.decide(donor)
                donor.step(a, ans)
            act, ans = gate.decide(env)
            _o, _r, done, info = env.step(act, ans)
            if done:
                records.append(info["ep_stats"])
    return _mean_stats(records)


def main() -> None:
    ap = argparse.ArgumentParser()
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--checkpoint")
    src.add_argument("--baseline", choices=sorted(BASELINES))
    ap.add_argument("--config", default="experiments/r0/configs/default.yaml")
    ap.add_argument("--episodes", type=int, default=256)
    ap.add_argument("--seed", type=int, default=900001)
    ap.add_argument("--causal", choices=[None, "erase", "shuffle", "permute", "noise"],
                    default=None)
    ap.add_argument("--ood", choices=sorted(OOD_OVERRIDES), default=None)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    with open(args.config) as f:
        cfg = R0Config(**yaml.safe_load(f).get("env", {}))
    if args.causal == "noise":
        cfg = R0Config(**{**cfg.__dict__, **OOD_OVERRIDES["highnoise"]})
    if args.ood:
        cfg = R0Config(**{**cfg.__dict__, **OOD_OVERRIDES[args.ood]})

    if args.baseline:
        res = evaluate_baseline(args.baseline, cfg, args.episodes, args.seed,
                                args.causal)
        tag = f"baseline/{args.baseline}"
    else:
        ck = torch.load(args.checkpoint, map_location=args.device,
                        weights_only=False)
        policy = build_policy(ck["model_name"], cfg.obs_dim, cfg.num_values)
        policy.load_state_dict(ck["model"])
        policy.to(args.device)
        res = evaluate_learned(policy, cfg, args.device, args.episodes,
                               args.seed, args.causal)
        tag = f"checkpoint/{Path(args.checkpoint).parent.name}"

    out = {"subject": tag, "causal": args.causal, "ood": args.ood,
           "episodes": args.episodes, "seed": args.seed,
           "env": asdict(cfg), "metrics": res}
    print(json.dumps(out, indent=2))
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
