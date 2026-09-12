"""Post-hoc controls for G0 — no model retraining required.

  * untrained discrete-bottleneck reps (`ae_vq_untrained`,
    `gru_untrained_km`): code-stability / MI controls that attribute
    "stable discrete symbols" to reservoir temporal smoothness vs
    learning. (Untrained `gru`/`ae` are already covered by the sweep's
    --untrained flag.)
  * paired per-seed diffs: trained rep minus its untrained reservoir
    twin on every summary metric, computed from sweep_summary.json
    merged with the control evals produced here.

Usage:
    python -m experiments.g0.controls --seeds 0 1 2 3 4
    python -m experiments.g0.controls --diffs-only   # skip evals
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from .config import load_config
from .data import train_val_datasets
from .evaluate import _eval_datasets, eval_representation
from .models import build_model
from .representations import TorchRep, build_kmeans_rep
from .sweep import METRIC_PATHS, _get, mean_std

REPORTS = Path(__file__).parent / "reports"

PAIRS = (("gru", "gru_untrained"), ("ae", "ae_untrained"),
         ("ae_vq", "ae_vq_untrained"), ("gru_km", "gru_untrained_km"))

# metrics where a positive diff would indicate learned structure
DIFF_METRICS = ("acc_in", "acc_loco", "acc_ood_ctx", "acc_dense_ctx",
                "seg_acc_in", "seg_acc_loco", "seg_acc_ood_ctx",
                "dynseg_acc_in", "dynseg_acc_loco", "dynseg_acc_ood_ctx",
                "match_ood", "midctx_acc", "best_action_delta",
                "nmi_pooled", "combo_oodctx_auc",
                "mi_code_cause_norm", "code_stability")


def run_untrained_discrete(cfg, seeds, device="cpu"):
    """Evaluate untrained ae_vq and gru-kmeans reservoir controls."""
    out = {}
    for seed in seeds:
        train_ds, _ = train_val_datasets(cfg, seed)
        dss = _eval_datasets(cfg, seed, seed)

        torch.manual_seed(seed)
        np.random.seed(seed)
        m = build_model("ae_vq", cfg.env.obs_dim, 4, cfg.model,
                        cfg.train.target_delta)
        rep = TorchRep("ae_vq_untrained", m, device)
        ev = eval_representation(rep, cfg, seed, device, model=m,
                                 train_ds=train_ds, env_seed=seed,
                                 dss=dss)
        out.setdefault("ae_vq_untrained", {})[seed] = ev
        d = ev.get("discrete") or {}
        print(f"== ae_vq_untrained s{seed}: stability "
              f"{d.get('code_stability', float('nan')):.3f} "
              f"mi_norm {d.get('mi_code_cause_norm', float('nan')):.3f}",
              flush=True)

        torch.manual_seed(seed)
        np.random.seed(seed)
        m = build_model("gru", cfg.env.obs_dim, 4, cfg.model,
                        cfg.train.target_delta)
        km = build_kmeans_rep("gru_untrained", m, device, cfg.model,
                              train_ds, seed)
        km.name = "gru_untrained_km"
        ev = eval_representation(km, cfg, seed, device, model=None,
                                 train_ds=train_ds, env_seed=seed,
                                 dss=dss)
        out.setdefault("gru_untrained_km", {})[seed] = ev
        d = ev.get("discrete") or {}
        print(f"== gru_untrained_km s{seed}: stability "
              f"{d.get('code_stability', float('nan')):.3f} "
              f"mi_norm {d.get('mi_code_cause_norm', float('nan')):.3f}",
              flush=True)
    return out


def paired_diffs(per_run: dict) -> dict:
    """trained minus untrained per seed, per metric."""
    res = {}
    for trained, untrained in PAIRS:
        if trained not in per_run or untrained not in per_run:
            continue
        seeds = sorted(set(per_run[trained]) & set(per_run[untrained]),
                       key=int)
        diffs = {}
        for m in DIFF_METRICS:
            path = METRIC_PATHS.get(m)
            if not path:
                continue
            ds = []
            for s in seeds:
                a = _get(per_run[trained][s], path)
                b = _get(per_run[untrained][s], path)
                if a is None or b is None:
                    continue
                if isinstance(a, float) and np.isnan(a):
                    continue
                if isinstance(b, float) and np.isnan(b):
                    continue
                ds.append(float(a - b))
            if ds:
                diffs[m] = {"mean_diff": float(np.mean(ds)),
                            "std_diff": float(np.std(ds)),
                            "n_pos": int(sum(d > 0 for d in ds)),
                            "n": len(ds)}
        res[f"{trained} - {untrained}"] = diffs
    return res


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--seeds", type=int, nargs="+",
                    default=[0, 1, 2, 3, 4])
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--diffs-only", action="store_true",
                    help="skip untrained evals; diffs from existing "
                         "sweep_summary.json only")
    ap.add_argument("--out", default=str(REPORTS /
                                         "controls_untrained.json"))
    args = ap.parse_args()

    cfg = load_config(args.config)
    new_evals = {}
    if not args.diffs_only:
        new_evals = run_untrained_discrete(cfg, args.seeds, args.device)

    summ_path = REPORTS / "sweep_summary.json"
    per_run = {}
    if summ_path.exists():
        per_run = json.load(open(summ_path)).get("per_run", {})
    for rep, by_seed in new_evals.items():
        per_run.setdefault(rep, {}).update(
            {str(s): v for s, v in by_seed.items()})

    payload = {"untrained_evals": new_evals,
               "paired_diffs": paired_diffs(per_run),
               "seeds": args.seeds}
    Path(args.out).write_text(json.dumps(payload, indent=2,
                                         default=float))
    print(json.dumps(payload["paired_diffs"], indent=2))


if __name__ == "__main__":
    main()
