"""Multi-seed sweep for G0: train models x seeds, evaluate every
representation (trained + analytic + k-means), aggregate to reports/.

Usage:
    python -m experiments.g0.sweep --seeds 0 1 2
    G0_TIME_BUDGET=60 python -m experiments.g0.sweep --seeds 0 --max-steps 300
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch

from .analysis.plots import make_report_plots
from .config import load_config
from .data import train_val_datasets
from .evaluate import _eval_datasets, eval_dynfeat, eval_representation
from .models import MODEL_REGISTRY, build_model
from .representations import (TorchRep, build_analytic_rep,
                              build_kmeans_rep)
from .train import RUNS_DIR, train_one

REPORTS = Path(__file__).parent / "reports"
ANALYTIC = ("raw", "raw_win", "pca", "pca_win")
TRAINED = tuple(MODEL_REGISTRY)          # ae, ae_vq, gru, gru_vq
KMEANS_ON = ("gru",)                     # add <name>_km reps

METRIC_PATHS = {
    "acc_in": ("probes", "acc_in"),
    "acc_loco": ("probes", "acc_loco"),
    "acc_ood_ctx": ("probes", "acc_ood_ctx"),
    "acc_ood_ctx_dm": ("probes", "acc_ood_ctx_dm"),
    "acc_dense_ctx": ("probes", "acc_dense_ctx"),
    "acc_dense_ctx_dm": ("probes", "acc_dense_ctx_dm"),
    "seg_acc_in": ("probes", "seg_acc_in"),
    "seg_acc_loco": ("probes", "seg_acc_loco"),
    "seg_acc_ood_ctx": ("probes", "seg_acc_ood_ctx"),
    "seg_acc_dense_ctx": ("probes", "seg_acc_dense_ctx"),
    "dynseg_acc_in": ("probes", "dynseg_acc_in"),
    "dynseg_acc_loco": ("probes", "dynseg_acc_loco"),
    "dynseg_acc_ood_ctx": ("probes", "dynseg_acc_ood_ctx"),
    "dynseg_acc_dense_ctx": ("probes", "dynseg_acc_dense_ctx"),
    "fewshot5_in": ("probes", "fewshot_in", "5"),
    "fewshot5_ood": ("probes", "fewshot_ood", "5"),
    "best_action": ("probes", "best_action_acc"),
    "boundary_auc": ("probes", "boundary_auc"),
    "x_r2": ("probes", "x_r2"),
    "nmi_pooled": ("clustering", "nmi_pooled"),
    "nmi_worst_ctx": ("clustering", "nmi_worst_ctx"),
    "nmi_ood": ("clustering", "nmi_ood_ctx"),
    "match_train": ("matching", "match_train_ctx"),
    "match_ood": ("matching", "match_ood_ctx"),
    "match_ood_dm": ("matching", "match_ood_ctx_dm"),
    "margin_ood": ("matching", "margin_ood_ctx"),
    "margin_ood_dm": ("matching", "margin_ood_ctx_dm"),
    "shuffle_acc": ("causal", "shuffle_acc"),
    "shuffle_delta": ("causal", "shuffle_delta"),
    "shuffle_aligned_acc": ("causal", "shuffle_aligned_acc"),
    "shuffle_aligned_delta": ("causal", "shuffle_aligned_delta"),
    "perm_acc": ("causal", "perm_acc"),
    "perm_delta": ("causal", "perm_delta"),
    "dropout_acc": ("causal", "dropout_acc"),
    "midctx_acc": ("causal", "midctx_acc"),
    "decoy_rate": ("causal", "decoy_rate"),
    "ood_noise": ("ood", "acc_noise"),
    "ood_gain": ("ood", "acc_gain"),
    "combo_auc": ("ood", "combo_presence_auc"),
    "combo_oodctx_auc": ("ood", "combo_oodctx_auc"),
    "combo_set": ("ood", "combo_set_acc"),
    "base_mse": ("base_mse",),
}


def _get(d: dict, path):
    for k in path:
        if not isinstance(d, dict) or k not in d:
            return None
        d = d[k]
    return d


def mean_std(vals):
    vals = [v for v in vals if v is not None
            and not (isinstance(v, float) and np.isnan(v))]
    if not vals:
        return None
    return {"mean": float(np.mean(vals)), "std": float(np.std(vals)),
            "n": len(vals)}


def summarize(all_results: dict) -> dict:
    summary = {}
    for rep, by_seed in all_results.items():
        res = list(by_seed.values())
        s = {"n_seeds": len(res)}
        for m, path in METRIC_PATHS.items():
            s[m] = mean_std([_get(r, path) for r in res])
        # mean carryover acc across post-switch lags
        lag_means = []
        for r in res:
            lags = _get(r, ("causal", "midctx_by_lag"))
            if lags:
                lag_means.append(float(np.mean(list(lags.values()))))
        s["midctx_lag_mean"] = mean_std(lag_means)
        disc = [_get(r, ("discrete", "mi_code_cause_norm")) for r in res]
        if any(v is not None for v in disc):
            s["mi_code_cause_norm"] = mean_std(disc)
            s["code_stability"] = mean_std(
                [_get(r, ("discrete", "code_stability")) for r in res])
            s["n_codes_used"] = mean_std(
                [_get(r, ("discrete", "n_codes_used")) for r in res])
        sel = [_get(r, ("intervention", "selectivity")) for r in res]
        if any(v is not None for v in sel):
            s["intervention_selectivity"] = mean_std(sel)
        summary[rep] = s
    return summary


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--models", nargs="+", default=list(TRAINED))
    ap.add_argument("--analytic", nargs="+", default=list(ANALYTIC))
    ap.add_argument("--untrained", nargs="+", default=["gru", "ae"],
                    help="models to also evaluate WITHOUT training "
                         "(reservoir controls)")
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--max-steps", type=int, default=None)
    ap.add_argument("--time-budget", type=float, default=0.0)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    cfg = load_config(args.config)
    all_results: dict[str, dict] = {}
    t0 = time.time()

    for seed in args.seeds:
        train_ds, val_ds = train_val_datasets(cfg, seed)
        # collect the eval battery once per seed — every rep sees the
        # same episodes (env params are seeded by `seed`)
        dss = _eval_datasets(cfg, seed, seed)
        # analytic reps (no training) — evaluated against same env_seed
        for name in args.analytic:
            all_results.setdefault(name, {})
            rep = build_analytic_rep(name, cfg.env.obs_dim, cfg.model,
                                     train_ds)
            ev = eval_representation(rep, cfg, seed, args.device,
                                     train_ds=train_ds, env_seed=seed,
                                     dss=dss)
            all_results[name][seed] = ev
            print(f"== {name} s{seed}: acc_in "
                  f"{ev['probes']['acc_in']:.3f} ood "
                  f"{ev['probes']['acc_ood_ctx']:.3f} nmi "
                  f"{ev['clustering']['nmi_pooled']:.3f}", flush=True)

        # dynamical-signature references: "canonical" = headroom bound
        # (eval-only true signal), "obs" = no-learning baseline
        for source in ("canonical", "obs"):
            name = f"dynfeat_{source}"
            evd = eval_dynfeat(cfg, seed, seed, dss, source)
            all_results.setdefault(name, {})[seed] = evd
            print(f"== {name} s{seed}: seg_acc "
                  f"{evd['probes'].get('seg_acc_in', float('nan')):.3f} "
                  f"loco {evd['probes'].get('seg_acc_loco', float('nan')):.3f} "
                  f"ood {evd['probes'].get('seg_acc_ood_ctx', float('nan')):.3f}",
                  flush=True)

        for model_name in args.models:
            out_dir = RUNS_DIR / f"{model_name}__seed{seed}"
            res = train_one(model_name, seed, cfg, args.device, out_dir,
                            args.time_budget, args.max_steps,
                            quiet=args.quiet,
                            datasets=(train_ds, val_ds))
            model, env_seed = res["model"], res["env_seed"]
            all_results.setdefault(model_name, {})
            rep = TorchRep(model_name, model, args.device)
            ev = eval_representation(rep, cfg, seed, args.device,
                                     model=model, train_ds=train_ds,
                                     env_seed=env_seed, dss=dss)
            ev["val_loss"] = res["val_loss"]
            all_results[model_name][seed] = ev
            with open(out_dir / "eval.json", "w") as f:
                json.dump(ev, f, indent=2, default=float)
            print(f"== {model_name} s{seed}: acc_in "
                  f"{ev['probes']['acc_in']:.3f} ood "
                  f"{ev['probes']['acc_ood_ctx']:.3f} nmi "
                  f"{ev['clustering']['nmi_pooled']:.3f}", flush=True)

            if model_name in KMEANS_ON:
                km = build_kmeans_rep(model_name, model, args.device,
                                      cfg.model, train_ds, seed)
                ev2 = eval_representation(km, cfg, seed, args.device,
                                          model=model,
                                          train_ds=train_ds,
                                          env_seed=env_seed, dss=dss)
                all_results.setdefault(km.name, {})[seed] = ev2
                with open(out_dir / "eval_km.json", "w") as f:
                    json.dump(ev2, f, indent=2, default=float)
                print(f"== {km.name} s{seed}: acc_in "
                      f"{ev2['probes']['acc_in']:.3f} ood "
                      f"{ev2['probes']['acc_ood_ctx']:.3f}", flush=True)

        # untrained reservoir controls: same arch, random init, same
        # eval battery — attributes learned-vs-random-feature content
        for mname in args.untrained:
            torch.manual_seed(seed)
            np.random.seed(seed)
            m = build_model(mname, cfg.env.obs_dim, 4, cfg.model,
                            cfg.train.target_delta)
            rep = TorchRep(f"{mname}_untrained", m, args.device)
            evu = eval_representation(rep, cfg, seed, args.device,
                                      model=m, train_ds=train_ds,
                                      env_seed=seed, dss=dss)
            all_results.setdefault(rep.name, {})[seed] = evu
            print(f"== {rep.name} s{seed}: acc_in "
                  f"{evu['probes']['acc_in']:.3f} ood "
                  f"{evu['probes']['acc_ood_ctx']:.3f}", flush=True)

    summary = summarize(all_results)
    REPORTS.mkdir(exist_ok=True)
    payload = {"per_run": all_results, "summary": summary,
               "config": args.config, "seeds": args.seeds,
               "wall_sec": time.time() - t0}
    with open(REPORTS / "sweep_summary.json", "w") as f:
        json.dump(payload, f, indent=2, default=float)
    try:
        figs = make_report_plots(summary, REPORTS / "figs")
        print("wrote figs:", [f.name for f in figs])
    except Exception as exc:  # plots are nice-to-have, never fatal
        print(f"plot generation failed: {exc}")
    print(json.dumps(summary, indent=2, default=float))


if __name__ == "__main__":
    main()
