"""H0 experiment sweep orchestrator.

Runs the full experiment matrix (spec section 19):

    agents:   random, heuristic, mlp64, gru64, gru128
    seeds:    0..4 (learned agents only)
    envs:     base (ID) + 4 OOD conditions
    ablation: none, shuffle, mask x5, permute, action_perm, hidden_reset

Subcommands:
    train     train all learned agents x seeds
    evaluate  ID + OOD eval for every agent/checkpoint
    causal    causal ablation battery per learned checkpoint
    all       train -> evaluate -> causal

Results land in experiments/h0/runs/<agent>/seed<i>/ and
experiments/h0/results/*.json.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Dict, Optional

import numpy as np

from .analysis.causality import run_causal_battery, summarize_causal
from .config import Config, load_config
from .env import dynamics as dyn
from .evaluate import TorchPolicy, evaluate, make_policy
from .train import train

H0_DIR = Path(__file__).resolve().parent
RUNS = H0_DIR / "runs"
RESULTS = H0_DIR / "results"
CONFIGS = H0_DIR / "configs"

LEARNED = [("mlp", 64), ("gru", 64), ("gru", 128)]
OOD_CONFIGS = ["ood_energy", "ood_temperature", "ood_risk", "ood_resource"]


def run_dir(agent: str, hidden: int, seed: int) -> Path:
    tag = f"{agent}{hidden}" if agent in ("mlp", "gru") else agent
    return RUNS / tag / f"seed{seed}"


def cmd_train(cfg: Config, seeds, agents, steps: Optional[int]):
    for kind, hidden in agents:
        for seed in seeds:
            out = run_dir(kind, hidden, seed)
            if (out / "final.pt").exists():
                print(f"skip {out} (final.pt exists)")
                continue
            c = load_config(CONFIGS / "base.yaml")
            c.agent.kind, c.agent.hidden, c.seed = kind, hidden, seed
            if steps:
                c.train.total_steps = steps
            print(f"=== train {kind}{hidden} seed{seed} -> {out}")
            train(c, out)


def _policy_factory(cfg: Config, checkpoint: Path):
    def make():
        return make_policy(cfg, str(checkpoint))
    return make


def cmd_evaluate(cfg: Config, seeds, agents, episodes: int):
    RESULTS.mkdir(parents=True, exist_ok=True)
    summary: Dict = {}

    def _ep_errs(r):
        return [ep["homeostatic_error_full"] for ep in r["episodes"]]

    # non-learned baselines
    for kind in ("random", "heuristic"):
        c = load_config(CONFIGS / "base.yaml")
        c.agent.kind = kind
        r = evaluate(c, make_policy(c), episodes, seed=0)
        summary[kind] = {"ID": r["aggregate"],
                         "ID_eps": _ep_errs(r)}
        for ood in OOD_CONFIGS:
            co = load_config(CONFIGS / f"{ood}.yaml")
            co.agent.kind = kind
            ro = evaluate(co, make_policy(co), episodes, seed=0)
            summary[kind][ood] = ro["aggregate"]
            summary[kind][f"{ood}_eps"] = _ep_errs(ro)
        (RESULTS / f"{kind}.json").write_text(json.dumps(summary[kind], indent=2))
        print(f"eval {kind}: ID err={summary[kind]['ID']['homeostatic_error_full']['mean']:.4f}")

    # learned agents: evaluate pre/best/final on ID + OOD
    for kind, hidden in agents:
        tag = f"{kind}{hidden}"
        for seed in seeds:
            rd = run_dir(kind, hidden, seed)
            if not (rd / "final.pt").exists():
                print(f"skip {tag} seed{seed}: no checkpoint")
                continue
            entry: Dict = {}
            for ckpt in ("pre", "best", "final"):
                p = rd / f"{ckpt}.pt"
                if not p.exists():
                    continue
                c = load_config(rd / "config.yaml")
                pol = _policy_factory(c, p)
                r = evaluate(c, pol(), episodes, seed=seed)
                entry[ckpt] = {"ID": r["aggregate"], "ID_eps": _ep_errs(r)}
                for ood in OOD_CONFIGS:
                    co = load_config(CONFIGS / f"{ood}.yaml")
                    co.agent = c.agent
                    ro = evaluate(co, pol(), episodes, seed=seed)
                    entry[ckpt][ood] = ro["aggregate"]
                    entry[ckpt][f"{ood}_eps"] = _ep_errs(ro)
                print(f"eval {tag} seed{seed} {ckpt}: "
                      f"ID err={entry[ckpt]['ID']['homeostatic_error_full']['mean']:.4f}")
            out = RESULTS / f"{tag}_seed{seed}.json"
            out.write_text(json.dumps(entry, indent=2))
            summary[f"{tag}_seed{seed}"] = entry

    (RESULTS / "summary.json").write_text(json.dumps(summary, indent=2))


def cmd_causal(cfg: Config, seeds, agents, episodes: int):
    RESULTS.mkdir(parents=True, exist_ok=True)
    for kind, hidden in agents:
        tag = f"{kind}{hidden}"
        for seed in seeds:
            rd = run_dir(kind, hidden, seed)
            ckpt = rd / "best.pt"
            if not ckpt.exists():
                continue
            c = load_config(rd / "config.yaml")
            print(f"=== causal {tag} seed{seed}")
            res = run_causal_battery(
                c, _policy_factory(c, ckpt), n_episodes=episodes, seed=seed
            )
            out = {
                "raw": res,
                "summary": summarize_causal(res),
            }
            (RESULTS / f"causal_{tag}_seed{seed}.json").write_text(
                json.dumps(out, indent=2)
            )
            s = out["summary"]
            print(f"  shuffle dErr={s['shuffle']['delta_error']:+.4f} "
                  f"permute dErr={s['permute']['delta_error']:+.4f} "
                  f"actperm dErr={s['action_perm_rest_explore']['delta_error']:+.4f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["train", "evaluate", "causal", "all"])
    ap.add_argument("--config", default=str(CONFIGS / "base.yaml"))
    ap.add_argument("--seeds", default="0,1,2,3,4")
    ap.add_argument("--agents", default="mlp64,gru64,gru128")
    ap.add_argument("--steps", type=int, default=None)
    ap.add_argument("--episodes", type=int, default=20)
    args = ap.parse_args()

    seeds = [int(s) for s in args.seeds.split(",")]
    agents = []
    for a in args.agents.split(","):
        if a.startswith("gru"):
            agents.append(("gru", int(a[3:])))
        elif a.startswith("mlp"):
            agents.append(("mlp", int(a[3:])))
        else:
            raise ValueError(a)

    cfg = load_config(args.config)
    if args.cmd in ("train", "all"):
        cmd_train(cfg, seeds, agents, args.steps)
    if args.cmd in ("evaluate", "all"):
        cmd_evaluate(cfg, seeds, agents, args.episodes)
    if args.cmd in ("causal", "all"):
        cmd_causal(cfg, seeds, agents, max(8, args.episodes // 2))


if __name__ == "__main__":
    main()
