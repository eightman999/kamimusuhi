"""G0-v4 evaluation — the G0 battery, unchanged, plus collapse health.

`eval_representation` is imported from `experiments.g0.evaluate` so every
metric name/threshold is directly comparable to G0-v3. On top we attach:

  health.main / health.ood_ctx — per-dim variance, effective rank,
      cosine distribution, covariance RMS (collapse detection, §19)

Usage:
    python -m experiments.g0_v4.evaluate --run-dir runs/cpc__seed0
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from experiments.g0.evaluate import _eval_datasets, eval_representation
from experiments.g0.representations import TorchRep

from .config import load_config
from .health import representation_health
from .train import load_ckpt

REPORTS = Path(__file__).parent / "reports"


def eval_model(model, rep_name: str, cfg, seed: int, device: str,
               train_ds, env_seed: int, dss=None) -> dict:
    """Full battery + health for one model instance."""
    if dss is None:
        dss = _eval_datasets(cfg, seed, env_seed)
    rep = TorchRep(rep_name, model, device)
    ev = eval_representation(rep, cfg, seed, device, model=model,
                             train_ds=train_ds, env_seed=env_seed,
                             dss=dss)
    ev["health"] = {
        "main": representation_health(
            rep.encode(dss["main"]["obs"], dss["main"]["actions"]),
            seed=seed),
        "ood_ctx": representation_health(
            rep.encode(dss["ood_ctx"]["obs"], dss["ood_ctx"]["actions"]),
            seed=seed),
    }
    return ev


def eval_run_dir(run_dir: Path, cfg, seed: int, device: str,
                 train_ds=None, dss=None) -> dict:
    ckpt = run_dir / "best.pt"
    if not ckpt.exists():
        ckpt = run_dir / "latest.pt"
    model, ck = load_ckpt(ckpt, device)
    env_seed = int(ck["env_seed"])
    if train_ds is None:
        from experiments.g0.data import train_val_datasets
        train_ds, _ = train_val_datasets(cfg, env_seed)
    if dss is None:
        dss = _eval_datasets(cfg, seed, env_seed)
    ev = eval_model(model, ck["model_name"], cfg, seed, device,
                    train_ds, env_seed, dss)
    out = run_dir / "eval.json"
    out.write_text(json.dumps(ev, indent=2, default=float))
    return ev


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()
    cfg = load_config(args.config)
    ev = eval_run_dir(Path(args.run_dir), cfg, args.seed, args.device)
    print(json.dumps(ev, indent=2, default=float))


if __name__ == "__main__":
    main()
