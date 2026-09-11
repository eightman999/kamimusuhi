"""Multi-seed sweep for S0: train all models x seeds, evaluate each,
aggregate into reports/.

Usage:
    python sweep.py --seeds 0 1 2 [--models gru_state_action ...]
    S0_TIME_BUDGET=120 python sweep.py --seeds 0   # smoke
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from data import load_config  # noqa: E402
from evaluate import eval_battery  # noqa: E402
from models import MODEL_REGISTRY  # noqa: E402
from train import RUNS_DIR, train_one  # noqa: E402

REPORTS = Path(__file__).parent / "reports"


def mean_std(vals):
    vals = [v for v in vals if v is not None and not np.isnan(v)]
    if not vals:
        return None
    return {"mean": float(np.mean(vals)), "std": float(np.std(vals)),
            "n": len(vals)}


def summarize(all_results: dict) -> dict:
    """all_results: {model: {seed: eval_result}} -> aggregated metrics."""
    summary = {}
    for model, by_seed in all_results.items():
        res = list(by_seed.values())
        s = {"n_seeds": len(res)}
        for g in ("overall", "self", "external", "mixed", "noise"):
            s[f"base_mse_{g}"] = mean_std([r["base_mse"][g] for r in res])
        s["sc1_self_ratio"] = mean_std(
            [r["sc1_shuffle"]["self"] / max(r["base_mse"]["self"], 1e-12)
             for r in res])
        s["sc5_self_ratio"] = mean_std(
            [r["sc5_mask"]["self"] / max(r["base_mse"]["self"], 1e-12)
             for r in res])
        s["cf_cos"] = mean_std(
            [r["sc2_counterfactual"]["counterfactual_cos"] for r in res])
        s["attr_auc_self_or_mix"] = mean_std(
            [r["attribution"]["auc_self_or_mix"] for r in res])
        if "probes" in res[0]:
            s["probe_self_r2_self_dims"] = mean_std(
                [r["probes"]["r2_self_on_self_dims"] for r in res])
            s["probe_action_acc"] = mean_std(
                [r["probes"]["action_probe_acc"] for r in res])
            s["probe_disturbance_auc"] = mean_std(
                [r["probes"]["disturbance_probe_auc"] for r in res])
        s["ood_disturbance_self_mse"] = mean_std(
            [r["ood"]["disturbance"]["self"] for r in res])
        s["sc4_self_mse_before"] = mean_std(
            [r["sc4_permutation"]["mse_before"]["self"] for r in res])
        s["sc4_self_mse_after"] = mean_std(
            [r["sc4_permutation"]["mse_after"]["self"] for r in res])
        summary[model] = s
    return summary


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                          "configs/default.yaml"))
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--models", nargs="+", default=list(MODEL_REGISTRY))
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--max-steps", type=int, default=None)
    ap.add_argument("--time-budget", type=float, default=0.0)
    ap.add_argument("--no-probes", action="store_true")
    args = ap.parse_args()

    cfg = load_config(args.config)
    all_results: dict[str, dict] = {}
    for model_name in args.models:
        all_results[model_name] = {}
        for seed in args.seeds:
            out_dir = RUNS_DIR / f"{model_name}__seed{seed}"
            r = train_one(model_name, seed, cfg, args.device, out_dir,
                          args.time_budget, args.max_steps)
            ev = eval_battery(out_dir / "ckpt.pt", device=args.device,
                              probes=not args.no_probes)
            all_results[model_name][seed] = ev
            with open(out_dir / "eval.json", "w") as f:
                json.dump(ev, f, indent=2)
            print(f"== {model_name} seed {seed}: "
                  f"base {ev['base_mse']['overall']:.4f} "
                  f"cf_cos {ev['sc2_counterfactual']['counterfactual_cos']:.3f} "
                  f"attr_auc {ev['attribution']['auc_self_or_mix']:.3f}")

    summary = summarize(all_results)
    REPORTS.mkdir(exist_ok=True)
    with open(REPORTS / "sweep_summary.json", "w") as f:
        json.dump({"per_run": all_results, "summary": summary}, f, indent=2)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
