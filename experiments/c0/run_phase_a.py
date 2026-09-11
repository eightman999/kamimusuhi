"""C0 phase A: run env+agent from t=0 to the interruption, serialize the
agent state artifact to disk, exit. The process boundary is real: nothing
survives except the artifact file.

    python -m experiments.c0.run_phase_a --checkpoint CKPT \
        --config experiments/c0/configs/default.yaml \
        --episodes 64 --ep-seed-base 900000 \
        --condition full --artifact-out art.json --metrics-out segA.json
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np

from .config_util import build_runner, git_commit, load_env_cfg
from .env.c0_env import C0Env
from .persistence.artifact import write_artifact
from .persistence.codec import state_spec_from
from .persistence.interrupt import run_leg, capture_payloads


def runner_spec(runner, env_cfg) -> list[tuple]:
    hid = getattr(getattr(runner, "policy", None), "hidden", 0)
    return state_spec_from(runner.state_fields(), env_cfg, hid)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--set", nargs="*", default=None,
                    help="env overrides k=v ; stops as 's:g,s:g'")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--checkpoint")
    src.add_argument("--heuristic", choices=["random", "oracle", "storeall"])
    ap.add_argument("--episodes", type=int, required=True)
    ap.add_argument("--ep-seed-base", type=int, required=True)
    ap.add_argument("--condition", required=True,
                    choices=["cold", "hidden", "memory", "compressed", "full"])
    ap.add_argument("--budget", type=int, default=None)
    ap.add_argument("--artifact-out", required=True)
    ap.add_argument("--metrics-out", required=True)
    ap.add_argument("--sample", action="store_true",
                    help="sample actions instead of greedy argmax")
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()

    t0 = time.time()
    env_cfg, env_d = load_env_cfg(args.config, args.set)
    runner, meta = build_runner(env_cfg, checkpoint=args.checkpoint,
                                heuristic=args.heuristic, device=args.device)
    spec = runner_spec(runner, env_cfg)

    envs = [C0Env(env_cfg) for _ in range(args.episodes)]
    ep_seeds = [args.ep_seed_base + i for i in range(args.episodes)]
    for e, s in zip(envs, ep_seeds):
        e.reset(seed=s)
    runner.reset(args.episodes)

    stop0 = env_cfg.stops[0][0]
    run_leg(envs, runner, stop0, greedy=not args.sample)

    payloads = capture_payloads(runner, args.condition, args.budget, spec)
    write_artifact(args.artifact_out, agent=meta["agent"],
                   condition=args.condition, payloads=payloads,
                   budget=args.budget, env_cfg_dict=env_d)

    h = runner.hidden()
    episodes = [{"i": i, "ep_seed": s, "t_end": envs[i].t,
                 "queries": envs[i].query_records,
                 "hidden_boundary": (None if h is None else
                                     np.asarray(h[i]).round(4).tolist())}
                for i, s in enumerate(ep_seeds)]
    metrics = {"phase": "a", "agent": meta["agent"], "meta": meta,
               "condition": args.condition, "budget": args.budget,
               "ep_seed_base": args.ep_seed_base, "env": env_d,
               "episodes": episodes, "wall_sec": round(time.time() - t0, 3),
               "git": git_commit()}
    Path(args.metrics_out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.metrics_out).write_text(json.dumps(metrics))
    print(f"[phase_a] {args.episodes} eps -> {args.artifact_out} "
          f"({sum(len(p) for p in payloads)} payload bytes)")


if __name__ == "__main__":
    main()
