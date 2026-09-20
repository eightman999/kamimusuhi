"""Sequential run driver: models x seeds, skipping completed runs.

Generates one JSON run config per (kind, seed), trains, then optionally
evaluates each run's best checkpoint and consolidates.
"""
import argparse
import dataclasses
import json
import subprocess
import sys
from pathlib import Path

import yaml

MODEL_KINDS = ["binder", "bottleneck", "indep_ae"]


def run_id_for(kind, seed):
    return f"{kind}-s{seed}"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--artifacts", required=True)
    p.add_argument("--config", default=None,
                   help="yaml base config; defaults to built-in base")
    p.add_argument("--seeds", default="0,1,2")
    p.add_argument("--models", default=",".join(MODEL_KINDS))
    p.add_argument("--smoke", action="store_true")
    p.add_argument("--evaluate", action="store_true")
    p.add_argument("--device", default="cpu")
    a = p.parse_args()
    seeds = [int(s) for s in a.seeds.split(",")]
    kinds = a.models.split(",")

    from experiments.x0.config import Config, load_config
    base: Config = load_config(a.config) if a.config else Config()
    if a.smoke:
        base.env.n_train_scenes = 1024
        base.env.n_val_scenes = 128
        base.train.steps = 60
        base.train.batch_scenes = 64
        base.train.val_every = 20
        base.eval.scenes = 128

    artifacts = Path(a.artifacts)
    cfg_dir = artifacts / "generated_configs"
    cfg_dir.mkdir(parents=True, exist_ok=True)

    for kind in kinds:
        for seed in seeds:
            run_id = run_id_for(kind, seed)
            status = artifacts / "runs" / run_id / "status.json"
            if status.exists() and json.loads(status.read_text()).get(
                    "status") == "complete":
                print(f"skip complete {run_id}", flush=True)
            else:
                cfg = dataclasses.replace(base)
                cfg.seed = seed
                cfg.model = dataclasses.replace(base.model, kind=kind)
                cfg.name = run_id
                cfg.train.device = a.device
                d = {"seed": seed,
                     "env": dataclasses.asdict(cfg.env),
                     "model": dataclasses.asdict(cfg.model),
                     "train": dataclasses.asdict(cfg.train),
                     "eval": dataclasses.asdict(cfg.eval)}
                cfg_path = cfg_dir / f"{run_id}.json"
                cfg_path.write_text(json.dumps(d, indent=2) + "\n")
                print(f"train {run_id}", flush=True)
                subprocess.run(
                    [sys.executable, "-m", "experiments.x0.train",
                     "--config", str(cfg_path), "--artifacts", str(artifacts),
                     "--run-id", run_id], check=True)
            if a.evaluate:
                ckpt = artifacts / "runs" / run_id / "best.pt"
                if not ckpt.exists():
                    ckpt = artifacts / "runs" / run_id / "final.pt"
                if ckpt.exists():
                    print(f"evaluate {run_id}", flush=True)
                    subprocess.run(
                        [sys.executable, "-m", "experiments.x0.evaluate",
                         "--checkpoint", str(ckpt), "--artifacts",
                         str(artifacts), "--device", a.device,
                         "--scenes", str(base.eval.scenes)], check=True)
    if a.evaluate:
        subprocess.run([sys.executable, "-m", "experiments.x0.evaluate",
                        "--consolidate-only", "--artifacts", str(artifacts)],
                       check=True)


if __name__ == "__main__":
    main()
