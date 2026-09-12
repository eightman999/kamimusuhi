"""Sweep orchestrator for U0: trains the model x seed matrix, then runs
the evaluation grid (clean, causal U-C1/U-C2, OOD U-C4..U-C6) for every
subject plus the U-C3 need-intervention probe for learned checkpoints,
and consolidates results under artifacts/results/.

Run bookkeeping per spec: each run lives under
    artifacts/runs/{model}_s{seed}/
with config.json, metrics.jsonl, best.pt, last.pt, done.json.
    completed (done.json exists) -> skip training
    partial   (last.pt, no done) -> resume from last.pt
    missing                       -> start

Usage (spec):
    caffeinate -ims python -m experiments.u0.sweep \
        --config experiments/u0/configs/default.yaml \
        --models mlp gru64 gru128 --seeds 0 1 2 --device cpu
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
from pathlib import Path

import torch

BASELINES = ["random", "fifo", "lru", "store_all",
             "heuristic_current_need", "oracle"]
CAUSAL_MODES = ["none", "erase", "shuffle"]
OOD_MODES = ["delay96", "delay128", "delay160",
             "distractor2x", "distractor4x", "need_mapping_shift",
             "event_perm"]


def run(cmd: list[str]) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)


def run_status(run_dir: Path) -> str:
    if (run_dir / "done.json").exists():
        return "completed"
    if (run_dir / "last.pt").exists():
        return "partial"
    return "missing"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="experiments/u0/configs/default.yaml")
    ap.add_argument("--models", nargs="+",
                    default=["mlp", "gru64", "gru128"])
    ap.add_argument("--seeds", nargs="+", type=int, default=[0, 1, 2])
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--episodes", type=int, default=256)
    ap.add_argument("--eval-seed", type=int, default=900001)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--skip-train", action="store_true")
    ap.add_argument("--eval-only", action="store_true")
    args = ap.parse_args()

    torch.set_num_threads(min(8, os.cpu_count() or 1))
    art = Path(args.artifacts)
    res_dir = art / "results"
    res_dir.mkdir(parents=True, exist_ok=True)
    model_of_run = {}

    # ---- phase 1: training -------------------------------------------------
    subjects = []  # (tag, checkpoint, model_name_or_baseline)
    if not args.eval_only:
        for model in args.models:
            for seed in args.seeds:
                run_id = f"{model}_s{seed}"
                run_dir = art / "runs" / run_id
                status = run_status(run_dir)
                if status == "completed" or args.skip_train:
                    print(f"[sweep] {run_id}: {status}, skip train")
                else:
                    cmd = [sys.executable, "-m", "experiments.u0.train",
                           "--config", args.config, "--model", model,
                           "--seed", str(seed), "--artifacts",
                           str(art), "--run-id", run_id,
                           "--device", args.device]
                    if status == "partial":
                        cmd.append("--resume")
                    run(cmd)
                ckpt = run_dir / "best.pt"
                subjects.append((run_id, str(ckpt), model))
                model_of_run[run_id] = model
    else:
        for model in args.models:
            for seed in args.seeds:
                run_id = f"{model}_s{seed}"
                subjects.append((run_id,
                                 str(art / "runs" / run_id / "best.pt"),
                                 model))
                model_of_run[run_id] = model

    for b in BASELINES:
        subjects.append((f"baseline_{b}", None, b))

    # ---- phase 2: clean + causal + OOD eval grid ---------------------------
    rows = []
    for tag, ckpt, kind in subjects:
        for mode in CAUSAL_MODES:
            for ood in [None] + OOD_MODES:
                if mode != "none" and ood is not None:
                    continue      # causal x OOD cross-product not required
                out = res_dir / f"{tag}_c-{mode}_o-{ood or 'none'}.json"
                if out.exists():
                    m = json.loads(out.read_text())["metrics"]
                    rows.append({"subject": tag, "causal": mode,
                                 "ood": ood or "none", **m})
                    continue
                cmd = [sys.executable, "-m", "experiments.u0.evaluate",
                       "--config", args.config,
                       "--episodes", str(args.episodes),
                       "--seed", str(args.eval_seed),
                       "--out", str(out)]
                if kind in BASELINES:
                    cmd += ["--baseline", kind]
                else:
                    cmd += ["--checkpoint", ckpt, "--device", args.device,
                            "--model", model_of_run[tag]]
                if mode != "none":
                    cmd += ["--causal", mode]
                if ood:
                    cmd += ["--ood", ood]
                run(cmd)
                m = json.loads(out.read_text())["metrics"]
                rows.append({"subject": tag, "causal": mode,
                             "ood": ood or "none", **m})

    # ---- phase 3: U-C3 need-intervention probe (learned only) --------------
    for tag, ckpt, kind in subjects:
        if kind in BASELINES or ckpt is None or not Path(ckpt).exists():
            continue
        out = res_dir / f"{tag}_probe-need_intervention.json"
        if out.exists():
            continue
        run([sys.executable, "-m", "experiments.u0.evaluate",
             "--config", args.config, "--checkpoint", ckpt,
             "--model", model_of_run[tag], "--device", args.device,
             "--probe", "need_intervention",
             "--episodes", "128", "--seed", str(args.eval_seed + 10000),
             "--out", str(out)])

    if rows:
        with open(res_dir / "summary.csv", "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"wrote {res_dir / 'summary.csv'} ({len(rows)} rows)")


if __name__ == "__main__":
    main()
