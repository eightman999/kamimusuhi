"""G0-v4 sweep driver.

Per seed: collect train/val + the G0 eval datasets ONCE (shared across
methods), train each method, evaluate the trained rep AND its untrained
twin (same arch + init seed, no training), plus the analytic/raw
controls and the dynfeat headroom references — all under metric names
identical to G0.

Artifacts:
    runs/<method>__seed<i>/{latest,best}.pt, config.yaml, meta.json,
        metrics.jsonl, eval.json
    reports/sweep_summary.json      per-run + mean±std (G0 schema)
    reports/controls_untrained.json paired trained−untrained diffs

Usage:
    python -m experiments.g0_v4.sweep --seeds 0 --device cuda
    python -m experiments.g0_v4.sweep --seeds 0 1 2 3 4 \
        --methods cpc jepa --device cuda --quiet
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch

from experiments.g0.data import train_val_datasets
from experiments.g0.evaluate import _eval_datasets, eval_dynfeat
from experiments.g0.representations import build_analytic_rep
from experiments.g0.sweep import METRIC_PATHS, _get, mean_std, summarize

from .config import load_config
from .evaluate import eval_model
from .models import MODEL_REGISTRY_V4, build_model
from .train import RUNS_DIR, load_ckpt, train_one

REPORTS = Path(__file__).parent / "reports"

ANALYTIC = ("raw", "raw_win", "pca", "pca_win")

# metrics where trained−untrained > 0 would indicate learned structure
DIFF_METRICS = ("acc_in", "acc_loco", "acc_ood_ctx", "acc_dense_ctx",
                "seg_acc_in", "seg_acc_loco", "seg_acc_ood_ctx",
                "dynseg_acc_in", "dynseg_acc_loco", "dynseg_acc_ood_ctx",
                "match_ood", "midctx_acc", "best_action_delta",
                "nmi_pooled", "combo_oodctx_auc",
                "intervention_selectivity")


def paired_diffs(per_run: dict, pairs) -> dict:
    """trained − untrained per seed, per metric (paired sign check)."""
    res = {}
    for trained, untrained in pairs:
        if trained not in per_run or untrained not in per_run:
            continue
        seeds = sorted(set(per_run[trained]) & set(per_run[untrained]),
                       key=int)
        diffs = {}
        for m in DIFF_METRICS:
            path = METRIC_PATHS.get(m)
            if not path and m != "intervention_selectivity":
                continue
            ds = []
            for s in seeds:
                if m == "intervention_selectivity":
                    a = _get(per_run[trained][s],
                             ("intervention", "selectivity"))
                    b = _get(per_run[untrained][s],
                             ("intervention", "selectivity"))
                else:
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


def run_sweep(cfg, seeds, methods, device="cpu", eval_only=False,
              max_steps=None, time_budget=0.0, quiet=False,
              analytic=ANALYTIC) -> dict:
    all_results: dict[str, dict] = {}
    for seed in seeds:
        train_ds, val_ds = train_val_datasets(cfg, seed)
        dss = _eval_datasets(cfg, seed, seed)

        # analytic controls (no learning)
        for name in analytic:
            rep = build_analytic_rep(name, cfg.env.obs_dim, cfg.model,
                                     train_ds)
            ev = eval_model_no_model(rep, cfg, seed, device, train_ds,
                                     seed, dss)
            all_results.setdefault(name, {})[seed] = ev
            print(f"== {name} s{seed}: acc_in "
                  f"{ev['probes']['acc_in']:.3f} ood "
                  f"{ev['probes']['acc_ood_ctx']:.3f}", flush=True)

        # headroom references
        for source in ("canonical", "obs"):
            name = f"dynfeat_{source}"
            all_results.setdefault(name, {})[seed] = \
                eval_dynfeat(cfg, seed, seed, dss, source)

        for method in methods:
            out_dir = RUNS_DIR / f"{method}__seed{seed}"
            if eval_only:
                ckpt = out_dir / "best.pt"
                if not ckpt.exists():
                    ckpt = out_dir / "latest.pt"
                model, ck = load_ckpt(ckpt, device)
                env_seed = int(ck["env_seed"])
                res = {"model": model, "val_loss": None,
                       "env_seed": env_seed, "train_ds": train_ds}
            else:
                res = train_one(method, seed, cfg, device, out_dir,
                                time_budget, max_steps, quiet,
                                datasets=(train_ds, val_ds))
            model, env_seed = res["model"], res["env_seed"]
            ev = eval_model(model, method, cfg, seed, device,
                            train_ds, env_seed, dss)
            ev["val_loss"] = res["val_loss"]
            all_results.setdefault(method, {})[seed] = ev
            with open(out_dir / "eval.json", "w") as f:
                json.dump(ev, f, indent=2, default=float)
            print(f"== {method} s{seed}: acc_in "
                  f"{ev['probes']['acc_in']:.3f} ood "
                  f"{ev['probes']['acc_ood_ctx']:.3f} midctx "
                  f"{ev['causal']['midctx_acc']:.3f} match "
                  f"{ev['matching']['match_ood_ctx']:.3f}", flush=True)

            # untrained twin: identical arch + init seed, no training
            torch.manual_seed(seed)
            np.random.seed(seed)
            mu = build_model(method, cfg, cfg.env.obs_dim, 4)
            evu = eval_model(mu, f"{method}_untrained", cfg, seed,
                             device, train_ds, env_seed, dss)
            all_results.setdefault(f"{method}_untrained", {})[seed] = evu
            with open(out_dir / "eval_untrained.json", "w") as f:
                json.dump(evu, f, indent=2, default=float)
            print(f"== {method}_untrained s{seed}: acc_in "
                  f"{evu['probes']['acc_in']:.3f} ood "
                  f"{evu['probes']['acc_ood_ctx']:.3f} midctx "
                  f"{evu['causal']['midctx_acc']:.3f} match "
                  f"{evu['matching']['match_ood_ctx']:.3f}", flush=True)

    summary = summarize(all_results)
    return {"per_run": all_results, "summary": summary}


def eval_model_no_model(rep, cfg, seed, device, train_ds, env_seed, dss):
    """Battery for non-torch reps (raw/pca): same eval, model=None."""
    from experiments.g0.evaluate import eval_representation
    return eval_representation(rep, cfg, seed, device, model=None,
                               train_ds=train_ds, env_seed=env_seed,
                               dss=dss)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--methods", nargs="+",
                    default=list(MODEL_REGISTRY_V4))
    ap.add_argument("--analytic", nargs="+", default=list(ANALYTIC))
    ap.add_argument("--eval-only", action="store_true")
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--max-steps", type=int, default=None)
    ap.add_argument("--time-budget", type=float, default=0.0)
    ap.add_argument("--quiet", action="store_true")
    ap.add_argument("--out", default=None,
                    help="summary path (default reports/"
                         "sweep_summary.json)")
    args = ap.parse_args()

    cfg = load_config(args.config)
    t0 = time.time()
    res = run_sweep(cfg, args.seeds, args.methods, args.device,
                    args.eval_only, args.max_steps, args.time_budget,
                    args.quiet, tuple(args.analytic))

    REPORTS.mkdir(exist_ok=True)
    out = Path(args.out) if args.out else REPORTS / "sweep_summary.json"
    payload = {**res, "config": args.config, "seeds": args.seeds,
               "methods": args.methods, "wall_sec": time.time() - t0,
               "protocol": "g0_v4"}
    with open(out, "w") as f:
        json.dump(payload, f, indent=2, default=float)

    pairs = [(m, f"{m}_untrained") for m in args.methods]
    diffs = paired_diffs(res["per_run"], pairs)
    with open(REPORTS / "controls_untrained.json", "w") as f:
        json.dump({"paired_diffs": diffs, "seeds": args.seeds}, f,
                  indent=2)
    print(json.dumps(res["summary"], indent=2, default=float))


if __name__ == "__main__":
    main()
