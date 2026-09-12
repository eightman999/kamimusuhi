"""Sweep orchestrator for R0: trains the model x seed matrix, then runs the
evaluation matrix (clean, causal R-C1..R-C4, OOD) for every subject and
writes aggregate JSON/CSV under artifacts/results/.

    python -m experiments.r0.sweep --config experiments/r0/configs/default.yaml \
        --models mlp gru64 gru128 --seeds 0 1 2 [--skip-train] [--eval-only]
"""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
import sys
from pathlib import Path

import yaml

CAUSAL_MODES = ["none", "erase", "shuffle", "permute", "noise"]
OOD_MODES = ["delay96", "delay128", "highnoise", "slots2"]
BASELINES = ["random", "fifo", "lru", "reservoir", "oracle"]


def run(cmd: list[str]) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="experiments/r0/configs/default.yaml")
    ap.add_argument("--models", nargs="+", default=["mlp", "gru64", "gru128"])
    ap.add_argument("--seeds", nargs="+", type=int, default=[0, 1, 2])
    ap.add_argument("--artifacts", default="experiments/r0/artifacts")
    ap.add_argument("--episodes", type=int, default=256)
    ap.add_argument("--eval-seed", type=int, default=900001)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--skip-train", action="store_true")
    args = ap.parse_args()

    art = Path(args.artifacts)
    res_dir = art / "results"
    res_dir.mkdir(parents=True, exist_ok=True)
    env_cfg = yaml.safe_load(open(args.config)).get("env", {})

    subjects = []  # (tag, checkpoint_path_or_None, baseline_name_or_None)
    if not args.skip_train:
        for model in args.models:
            for seed in args.seeds:
                run_id = f"{model}_s{seed}"
                if not (art / "runs" / run_id / "done.json").exists():
                    run([sys.executable, "-m", "experiments.r0.train",
                         "--config", args.config, "--model", model,
                         "--seed", str(seed), "--artifacts", args.artifacts,
                         "--run-id", run_id, "--device", args.device])
                subjects.append((run_id, str(art / "runs" / run_id / "best.pt"),
                                 None))
    else:
        for model in args.models:
            for seed in args.seeds:
                run_id = f"{model}_s{seed}"
                subjects.append((run_id, str(art / "runs" / run_id / "best.pt"),
                                 None))

    for b in BASELINES:
        subjects.append((f"baseline_{b}", None, b))

    rows = []
    for tag, ckpt, baseline in subjects:
        for mode in CAUSAL_MODES:
            for ood in [None] + OOD_MODES:
                if mode != "none" and ood is not None:
                    continue  # causal x OOD cross-product not required
                out = res_dir / f"{tag}_c-{mode}_o-{ood or 'none'}.json"
                if out.exists():
                    m = json.loads(out.read_text())["metrics"]
                    rows.append({"subject": tag, "causal": mode,
                                 "ood": ood or "none", **m})
                    continue
                cmd = [sys.executable, "-m", "experiments.r0.evaluate",
                       "--config", args.config, "--episodes", str(args.episodes),
                       "--seed", str(args.eval_seed), "--out", str(out)]
                cmd += ["--baseline", baseline] if baseline else \
                    ["--checkpoint", ckpt, "--device", args.device]
                if mode != "none":
                    cmd += ["--causal", mode]
                if ood:
                    cmd += ["--ood", ood]
                run(cmd)
                m = json.loads(out.read_text())["metrics"]
                rows.append({"subject": tag, "causal": mode,
                             "ood": ood or "none", **m})

    with open(res_dir / "summary.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
        w.writeheader()
        w.writerows(rows)
    print(f"wrote {res_dir / 'summary.csv'} ({len(rows)} rows)")


if __name__ == "__main__":
    main()
