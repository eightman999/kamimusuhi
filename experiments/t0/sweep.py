"""Sequential run driver: architectures x seeds, skipping completed runs."""
import argparse
import json
import subprocess
import sys
from pathlib import Path

from experiments.t0.env.interval_env import INTERP_HOLDOUT
from experiments.t0.models import ARCHITECTURES


def run_id_for(arch, seed):
    return f"{arch}-s{seed}"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--artifacts", required=True)
    p.add_argument("--seeds", default="0,1,2")
    p.add_argument("--architectures", default=",".join(ARCHITECTURES))
    p.add_argument("--imitation-updates", type=int, default=120)
    p.add_argument("--ppo-updates", type=int, default=80)
    p.add_argument("--num-envs", type=int, default=512)
    p.add_argument("--device", default="cpu")
    p.add_argument("--excluded-delays", default=",".join(map(str, INTERP_HOLDOUT)),
                   help="delays removed from the training support (v2 holdout)")
    p.add_argument("--smoke", action="store_true")
    p.add_argument("--evaluate", action="store_true",
                   help="run evaluate on each completed run's best checkpoint")
    a = p.parse_args()
    seeds = [int(s) for s in a.seeds.split(",")]
    archs = a.architectures.split(",")
    excluded = [int(d) for d in a.excluded_delays.split(",") if d]
    if a.smoke:
        a.imitation_updates, a.ppo_updates, a.num_envs = 4, 2, 64
    artifacts = Path(a.artifacts)
    cfg_dir = artifacts / "generated_configs"
    cfg_dir.mkdir(parents=True, exist_ok=True)
    for arch in archs:
        for seed in seeds:
            run_id = run_id_for(arch, seed)
            status = Path(artifacts) / "runs" / run_id / "status.json"
            if status.exists() and json.loads(status.read_text()).get("status") == "complete":
                print(f"skip complete {run_id}", flush=True)
            else:
                config = {"architecture": arch, "seed": seed, "task": "interval",
                          "num_envs": a.num_envs, "minibatch_envs": min(256, a.num_envs),
                          "imitation_updates": a.imitation_updates,
                          "ppo_updates": a.ppo_updates, "ppo_epochs": 2,
                          "validation_envs": 128,
                          "env": {"delay_distribution": "mixed", "horizon": 96,
                                  "delays": [8, 16, 24, 32, 48, 64],
                                  "delay_min": 8, "delay_max": 64,
                                  "excluded_training_delays": excluded,
                                  "window": 2, "distractor_rate": 0.02,
                                  "noise": 0.1}}
                cfg = cfg_dir / f"{run_id}.json"
                cfg.write_text(json.dumps(config, indent=2) + "\n")
                print(f"train {run_id}", flush=True)
                subprocess.run(
                    [sys.executable, "-m", "experiments.t0.train",
                     "--config", str(cfg), "--artifacts", str(artifacts),
                     "--run-id", run_id, "--device", a.device],
                    check=True)
            if a.evaluate:
                ckpt = Path(artifacts) / "runs" / run_id / "ppo_best.pt"
                if not ckpt.exists():
                    ckpt = Path(artifacts) / "runs" / run_id / "imitation_best.pt"
                if ckpt.exists():
                    print(f"evaluate {run_id}", flush=True)
                    subprocess.run(
                        [sys.executable, "-m", "experiments.t0.evaluate",
                         "--checkpoint", str(ckpt), "--artifacts", str(artifacts),
                         "--episodes", "128", "--device", a.device],
                        check=True)
    if a.evaluate:
        subprocess.run([sys.executable, "-m", "experiments.t0.evaluate",
                        "--consolidate-only", "--artifacts", str(artifacts)],
                       check=True)


if __name__ == "__main__":
    main()
