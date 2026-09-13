"""Sweep orchestrator for U0: enforces the protocol sanity gate, trains
the model x seed matrix, then runs the evaluation grid (clean, causal
targeted_erase/donor_shuffle/targeted_mediation, OOD battery) for every
subject plus the need-intervention probe for learned checkpoints, and
consolidates results under artifacts/results/.

The P0 protocol sanity gate runs first: if the environment cannot
separate oracle from no_memory the sweep stops with PROTOCOL_FAIL and
no PPO training happens. A cached passing verdict in
artifacts/results/protocol_check.json is reused; --recheck-protocol
forces a rerun.

Run bookkeeping per spec: each run lives under
    artifacts/runs/{model}_s{seed}/
with config.yaml, metrics.jsonl, best.pt, latest.pt, meta.json.
    completed (meta.json exists)    -> skip training
    partial   (latest.pt, no meta)  -> resume from latest.pt
    missing                         -> start

Failures are never swallowed: every failed stage is recorded to
artifacts/results/failures.json and the sweep continues with the next
subject so one crashed run cannot kill the overnight batch.

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

from .config import resolve_device

BASELINES = ["random", "fifo", "lru", "store_all",
             "heuristic_current_need", "no_memory", "oracle"]
CAUSAL_MODES = ["none", "targeted_erase", "donor_shuffle",
                "targeted_mediation"]
OOD_MODES = ["delay96", "delay128", "delay160",
             "distractor2x", "distractor4x", "need_mapping_shift",
             "event_permutation", "capacity2"]


def run(cmd: list[str], failures: list[dict], subject: str,
        stage: str) -> bool:
    print("+", " ".join(cmd), flush=True)
    try:
        subprocess.run(cmd, check=True)
        return True
    except subprocess.CalledProcessError as e:
        failures.append({"subject": subject, "stage": stage,
                         "reason": f"exit code {e.returncode}"})
        print(f"[sweep] {subject} {stage} FAILED "
              f"(exit {e.returncode})", flush=True)
        return False


def run_status(run_dir: Path) -> str:
    if (run_dir / "meta.json").exists():
        return "completed"
    if (run_dir / "latest.pt").exists():
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
    ap.add_argument("--device", default="cpu",
                    choices=["cpu", "mps", "auto"])
    ap.add_argument("--protocol-episodes", type=int, default=512)
    ap.add_argument("--skip-protocol-check", action="store_true",
                    help="debug only: bypass the P0 sanity gate")
    ap.add_argument("--recheck-protocol", action="store_true",
                    help="re-run the sanity gate even if a pass is cached")
    ap.add_argument("--teacher-iters", type=int, default=0,
                    help="add a second arm per model with mechanics-only "
                         "teacher bootstrapping (run ids get a T suffix; "
                         "teacher settings come from train.* config)")
    ap.add_argument("--scaffold", action="store_true",
                    help="add a curriculum arm per model (C suffix): "
                         "crisis action-mask + functional-only stores + "
                         "delay annealing — disclosed in run config")
    ap.add_argument("--skip-train", action="store_true")
    ap.add_argument("--eval-only", action="store_true")
    args = ap.parse_args()

    torch.set_num_threads(min(8, os.cpu_count() or 1))
    device = resolve_device(args.device)
    art = Path(args.artifacts)
    res_dir = art / "results"
    res_dir.mkdir(parents=True, exist_ok=True)
    model_of_run = {}
    failures: list[dict] = []

    # ---- phase 0: P0 protocol sanity gate ---------------------------------
    # No PPO may run until the environment provably separates oracle from
    # no_memory. A cached PASS is reused across invocations.
    pc_json = res_dir / "protocol_check.json"
    if args.skip_protocol_check:
        print("[sweep] WARNING: protocol sanity gate bypassed "
              "(--skip-protocol-check)", flush=True)
    else:
        verdict = None
        if pc_json.exists() and not args.recheck_protocol:
            try:
                verdict = json.loads(pc_json.read_text())
            except Exception:
                verdict = None
        if verdict is None or not verdict.get("all_pass"):
            print("[sweep] running protocol sanity gate "
                  f"({args.protocol_episodes} episodes)...", flush=True)
            subprocess.run(
                [sys.executable, "-m", "experiments.u0.protocol_check",
                 "--config", args.config,
                 "--episodes", str(args.protocol_episodes),
                 "--artifacts", str(art)])
            verdict = json.loads(pc_json.read_text()) \
                if pc_json.exists() else {"all_pass": False}
        if not verdict.get("all_pass"):
            print("[sweep] PROTOCOL_FAIL — environment does not "
                  "separate oracle from no_memory; not training.",
                  flush=True)
            (res_dir / "failures.json").write_text(json.dumps(
                [{"subject": "*", "stage": "protocol_check",
                  "reason": "P0 gate failed"}], indent=2))
            sys.exit(2)
        print(f"[sweep] protocol gate: {verdict['verdict']}", flush=True)

    # ---- phase 1: training -------------------------------------------------
    # --teacher-iters N adds a second, clearly-tagged arm per model
    # ("{model}T") that gets mechanics-only teacher bootstrapping; the
    # main arm ({model}) is always pure reward learning.
    subjects = []  # (tag, checkpoint, model_name_or_baseline)
    if not args.eval_only:
        arms = [(m, "", ()) for m in args.models]
        if args.teacher_iters:
            arms += [(m, "T", ("--imitation-iters",
                              str(args.teacher_iters)))
                     for m in args.models]
        if args.scaffold:
            arms += [(m, "C", ("--scaffold",)) for m in args.models]
        for model, tag, extra in arms:
            for seed in args.seeds:
                run_id = f"{model}{tag}_s{seed}"
                run_dir = art / "runs" / run_id
                status = run_status(run_dir)
                if status == "completed" or args.skip_train:
                    print(f"[sweep] {run_id}: {status}, skip train")
                else:
                    cmd = [sys.executable, "-m", "experiments.u0.train",
                           "--config", args.config, "--model", model,
                           "--seed", str(seed), "--artifacts",
                           str(art), "--run-id", run_id,
                           "--device", device, *extra]
                    if status == "partial":
                        cmd.append("--resume")
                    run(cmd, failures, run_id, "train")
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
        if kind not in BASELINES and not Path(ckpt).exists():
            failures.append({"subject": tag, "stage": "eval",
                             "reason": "missing checkpoint (best.pt)"})
            print(f"[sweep] {tag}: no best.pt, skipping eval grid",
                  flush=True)
            continue
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
                    cmd += ["--checkpoint", ckpt, "--device", device,
                            "--model", model_of_run[tag]]
                if mode != "none":
                    cmd += ["--causal", mode]
                if ood:
                    cmd += ["--ood", ood]
                stage = f"eval c-{mode} o-{ood or 'none'}"
                if run(cmd, failures, tag, stage):
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
             "--model", model_of_run[tag], "--device", device,
             "--probe", "need_intervention",
             "--episodes", "128", "--seed", str(args.eval_seed + 10000),
             "--out", str(out)], failures, tag, "probe-need_intervention")

    (res_dir / "failures.json").write_text(
        json.dumps(failures, indent=2))
    if rows:
        with open(res_dir / "summary.csv", "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"wrote {res_dir / 'summary.csv'} ({len(rows)} rows)")
    print(f"[sweep] done: {len(rows)} eval rows, "
          f"{len(failures)} failures", flush=True)


if __name__ == "__main__":
    main()
