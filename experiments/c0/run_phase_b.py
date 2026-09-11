"""C0 phase B: fresh process. Rebuild the episode (same ep seed supplied by
the orchestrator -- the artifact carries no episode id), fast-forward the
env to the resume step, decode the state artifact, restore the agent, and
continue to the next stop or the episode end.

    python -m experiments.c0.run_phase_b --checkpoint CKPT \
        --config experiments/c0/configs/default.yaml \
        --episodes 64 --ep-seed-base 900000 \
        --artifact-in art.json --stop-index 0 --metrics-out segB.json
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np

from .config_util import build_runner, git_commit, load_env_cfg
from .env.c0_env import C0Env
from .persistence.artifact import read_artifact, write_artifact
from .persistence.codec import state_spec_from, decode_payload
from .persistence.interrupt import run_leg, capture_payloads
from .run_phase_a import runner_spec


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--set", nargs="*", default=None)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--checkpoint")
    src.add_argument("--heuristic", choices=["random", "oracle", "storeall"])
    ap.add_argument("--model-override", default=None,
                    help="load weights from a different checkpoint "
                         "(model-swap stretch test)")
    ap.add_argument("--episodes", type=int, required=True)
    ap.add_argument("--ep-seed-base", type=int, required=True)
    ap.add_argument("--artifact-in", required=True)
    ap.add_argument("--stop-index", type=int, required=True,
                    help="resume at stops[k] + gap_k")
    ap.add_argument("--artifact-out", default=None,
                    help="write a new artifact at the next stop (chained)")
    ap.add_argument("--metrics-out", required=True)
    ap.add_argument("--sample", action="store_true")
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()

    t0 = time.time()
    env_cfg, env_d = load_env_cfg(args.config, args.set)
    doc, payloads = read_artifact(args.artifact_in)
    assert len(payloads) == args.episodes, "artifact/episode count mismatch"
    condition, budget = doc["condition"], doc.get("budget_bytes")

    runner, meta = build_runner(env_cfg, checkpoint=args.checkpoint,
                                heuristic=args.heuristic,
                                model_override=args.model_override,
                                device=args.device)
    spec = runner_spec(runner, env_cfg)
    states = [decode_payload(p, condition, spec) for p in payloads]

    k = args.stop_index
    resume = env_cfg.stops[k][0] + env_cfg.stops[k][1]
    envs = [C0Env(env_cfg) for _ in range(args.episodes)]
    ep_seeds = [args.ep_seed_base + i for i in range(args.episodes)]
    runner.reset(args.episodes)
    for e, s in zip(envs, ep_seeds):
        e.reset(seed=s)
        e.advance_to(resume)
    runner.set_states(states)
    h_resume = runner.hidden()

    next_stop = (env_cfg.stops[k + 1][0] if k + 1 < env_cfg.n_stops
                 else env_cfg.episode_len)
    run_leg(envs, runner, next_stop, greedy=not args.sample)

    if args.artifact_out:
        out_payloads = capture_payloads(runner, condition, budget, spec)
        write_artifact(args.artifact_out, agent=meta["agent"],
                       condition=condition, payloads=out_payloads,
                       budget=budget, env_cfg_dict=env_d)

    h_end = runner.hidden()
    episodes = []
    for i, s in enumerate(ep_seeds):
        rec = {"i": i, "ep_seed": s, "t_range": [resume, envs[i].t],
               "queries": envs[i].query_records,
               "ep_stats": envs[i].episode_stats(),
               "hidden_resume": (None if h_resume is None else
                                 np.asarray(h_resume[i]).round(4).tolist()),
               "hidden_end": (None if h_end is None else
                              np.asarray(h_end[i]).round(4).tolist())}
        episodes.append(rec)
    metrics = {"phase": "b", "agent": meta["agent"], "meta": meta,
               "condition": condition, "budget": budget,
               "stop_index": k, "resume": resume, "next_stop": next_stop,
               "artifact_in": str(args.artifact_in),
               "artifact_condition": condition,
               "ep_seed_base": args.ep_seed_base, "env": env_d,
               "episodes": episodes, "wall_sec": round(time.time() - t0, 3),
               "git": git_commit()}
    Path(args.metrics_out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.metrics_out).write_text(json.dumps(metrics))
    print(f"[phase_b] resumed@{resume} x{args.episodes} eps -> "
          f"{args.metrics_out}")


if __name__ == "__main__":
    main()
