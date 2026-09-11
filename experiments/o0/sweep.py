"""O0 sweep driver: multi-seed training + evaluation suite + aggregation.

    python -m experiments.o0.sweep --config experiments/o0/configs/default.yaml \
        --seeds 0 1 2 3 4 --models mlp gru64 gru128 lstm64

For every (model, seed) it trains a predictor, then evaluates:
    * ID eval                     (PRESETS["id"])
    * causal: hidden reset mid-occlusion, mid-visible control, noise
    * OOD:  longer occlusions (24/32/48), unseen velocities, drag shift,
            velocity flip, 4 distractors, ambush decoys, app jitter
Heuristics (no training) are evaluated on the same episode distributions.

All raw per-run results go to reports/raw/*.json; aggregates and plots to
reports/.  Budgets are config/env/CLI overrideable.
"""

from __future__ import annotations

import argparse
import copy
import dataclasses
import json
import time
from pathlib import Path

import numpy as np
import torch

from .analysis import plots
from .config import Config, load_config
from .env import dynamics as dyn
from .evaluate import (PRESETS, evaluate_heuristic, evaluate_model,
                       preset_params)
from .models import make_model
from .train import atomic, train

CAUSAL = [
    {"type": "hidden_reset", "at": "mid_occl"},
    {"type": "hidden_reset", "at": "mid_vis"},
    {"type": "hidden_reset", "at": "occl_start"},
    {"type": "hidden_noise", "at": "mid_occl", "sigma": 0.5},
    {"type": "hidden_noise", "at": "mid_occl", "sigma": 2.0},
]

OOD_PRESET_LIST = [
    "occ24", "occ32", "occ48",
    "fast_v", "slow_v",
    "drag_x3", "v_flip",
    "distractors4", "ambush",
    "app_jitter", "swap_half",
]

HEURISTIC_NAMES = ["prior", "lastobs", "constvel", "oracle"]


def run_id_for(arch, seed):
    return f"{arch}_s{seed}"


def load_trained(cfg: Config, artifacts, arch, seed):
    run_dir = Path(artifacts) / "runs" / run_id_for(arch, seed)
    ckpt = run_dir / "best.pt"
    if not ckpt.exists():
        ckpt = run_dir / "final.pt"
    model = make_model(arch)
    cp = torch.load(ckpt, map_location="cpu", weights_only=True)
    model.load_state_dict(cp["model"])
    model.eval()
    return model, str(ckpt)


def eval_suite(cfg: Config, artifacts: Path, reports: Path, tag: str,
               predict_fn, raw: list):
    """Run ID + causal + OOD evals for one predictor. Append rows to raw."""
    n = cfg.eval.episodes
    seed = cfg.eval.seed
    rows = []
    for preset_name in ["id"] + OOD_PRESET_LIST:
        p = preset_params(cfg.env, preset_name)
        for iv in ([{"type": "none"}] if preset_name != "id"
                   else [{"type": "none"}] + CAUSAL):
            res = predict_fn(p, n, seed, iv)
            m = res["metrics"]
            row = dict(tag=tag, preset=preset_name, intervention=iv, **m)
            row["boutlen_curve"] = res.get("boutlen_curve", [])
            rows.append(row)
            raw.append(row)
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--artifacts", default="experiments/o0/artifacts")
    ap.add_argument("--reports", default="experiments/o0/reports")
    ap.add_argument("--seeds", type=int, nargs="+", default=[0, 1, 2])
    ap.add_argument("--models", nargs="+",
                    default=["mlp", "gru64", "gru128", "lstm64"])
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--updates", type=int, default=None)
    ap.add_argument("--skip-train", action="store_true")
    ap.add_argument("--heuristics-only", action="store_true")
    args = ap.parse_args()

    cfg = load_config(args.config)
    if args.episodes:
        cfg.eval.episodes = args.episodes
    if args.updates:
        cfg.train.updates = args.updates
    artifacts = Path(args.artifacts)
    reports = Path(args.reports)
    (reports / "raw").mkdir(parents=True, exist_ok=True)

    t_start = time.time()
    raw: list = []

    # ---------------- heuristics ----------------
    for name in HEURISTIC_NAMES:
        p0 = cfg.env

        def hf(p, n, seed, iv, name=name):
            return evaluate_heuristic(name, p, n, seed, iv)

        eval_suite(cfg, artifacts, reports, f"heur:{name}", hf, raw)
        print(f"[heur:{name}] done", flush=True)

    if not args.heuristics_only:
        # ---------------- train + eval learned models ----------------
        for arch in args.models:
            for seed in args.seeds:
                rid = run_id_for(arch, seed)
                if not args.skip_train:
                    c = copy.deepcopy(cfg)
                    c.model.arch = arch
                    c.seed = seed
                    c.name = rid
                    print(f"=== training {rid} ===", flush=True)
                    train(c, artifacts, rid)
                model, ckpt = load_trained(cfg, artifacts, arch, seed)

                def mf(p, n, seed_, iv, model=model):
                    return evaluate_model(model, p, n, seed_, iv,
                                          device=cfg.train.device)

                eval_suite(cfg, artifacts, reports, rid, mf, raw)
                print(f"[{rid}] eval suite done", flush=True)

    # ---------------- aggregate ----------------
    # Merge with any existing raw rows so incremental invocations (extra
    # seeds/models) accumulate instead of clobbering.  New rows win on
    # key collision (re-runs replace stale results for the same cell).
    raw_path = reports / "raw" / "all_results.json"
    merged = {}
    if raw_path.exists():
        for old in json.loads(raw_path.read_text()):
            merged[_row_key(old)] = old
    for row in raw:
        merged[_row_key(row)] = row
    raw = list(merged.values())
    atomic(raw_path, raw)
    summary = aggregate(raw)
    atomic(reports / "summary.json", summary)
    make_plots(raw, reports)
    print(json.dumps(summary["id_table"], indent=2, default=float))
    print(f"sweep wall time: {round(time.time() - t_start, 1)}s")


def _row_key(row) -> tuple:
    """Dedupe key for a raw result row: predictor x condition."""
    iv = row["intervention"]
    return (row["tag"], row["preset"], iv["type"], iv.get("at", ""),
            iv.get("sigma", None))


def aggregate(raw: list) -> dict:
    """Mean/std across seeds per model per preset/intervention."""
    by = {}
    for row in raw:
        by.setdefault(_row_key(row), []).append(row)
    table = {}
    for (tag, preset, itype, iat, sigma), rows in sorted(
            by.items(), key=lambda kv: tuple(str(x) for x in kv[0])):
        entry = {}
        for k in ("pos_mae_occluded", "pos_err_pre_reappear",
                  "pos_mae_visible", "exist_acc_hidden", "exist_acc_all",
                  "exist_acc_absorbed", "id_acc", "mean_bout_len"):
            vals = np.array([r.get(k, np.nan) for r in rows], dtype=np.float64)
            with np.errstate(all="ignore"):
                entry[k + "_mean"] = float(np.nanmean(vals))
                entry[k + "_std"] = float(np.nanstd(vals))
        label = f"{tag}|{itype}@{iat}"
        if sigma is not None:
            label += f"(s={sigma})"
        table.setdefault(preset, {})[label] = entry
    # compact ID table for printing
    id_table = {}
    for tag in sorted({r["tag"] for r in raw}):
        rows = [r for r in raw
                if r["tag"] == tag and r["preset"] == "id"
                and r["intervention"]["type"] == "none"]
        if not rows:
            continue
        id_table[tag] = {
            k: round(float(np.nanmean([r.get(k, np.nan) for r in rows])), 4)
            for k in ("pos_mae_occluded", "pos_err_pre_reappear",
                      "exist_acc_hidden", "id_acc")
        }
    return {"per_cell": table, "id_table": id_table}


def make_plots(raw: list, reports: Path):
    """Figures: error-vs-occluded-step curves (ID + OOD), model bars,
    causal degradation, bout-length curves."""
    # --- error vs occlusion step index, ID -----
    curves = {}
    for tag in sorted({r["tag"] for r in raw}):
        rows = [r for r in raw if r["tag"] == tag and r["preset"] == "id"
                and r["intervention"]["type"] == "none"]
        if not rows:
            continue
        n = max(len(r["err_vs_occstep"]) for r in rows)
        pad = np.full((len(rows), n), np.nan)
        for i, r in enumerate(rows):
            pad[i, : len(r["err_vs_occstep"])] = r["err_vs_occstep"]
        curves[tag] = (np.arange(1, n + 1), np.nanmean(pad, axis=0),
                       np.nanstd(pad, axis=0))
    plots.plot_err_vs_occstep(
        curves, reports / "err_vs_occstep_id.png",
        "Position error vs steps since disappearance (ID, occ 4-16)")

    # --- same for OOD occ48 ---
    curves48 = {}
    for tag in sorted({r["tag"] for r in raw}):
        rows = [r for r in raw if r["tag"] == tag and r["preset"] == "occ48"
                and r["intervention"]["type"] == "none"]
        if not rows:
            continue
        n = max(len(r["err_vs_occstep"]) for r in rows)
        pad = np.full((len(rows), n), np.nan)
        for i, r in enumerate(rows):
            pad[i, : len(r["err_vs_occstep"])] = r["err_vs_occstep"]
        curves48[tag] = (np.arange(1, n + 1), np.nanmean(pad, axis=0),
                         np.nanstd(pad, axis=0))
    if curves48:
        plots.plot_err_vs_occstep(
            curves48, reports / "err_vs_occstep_occ48.png",
            "Position error vs steps hidden (OOD, occlusion 48)")

    # --- causal bars: pos_mae_occluded under interventions -----
    groups = {}
    for iv_label, pred in [("none", {"type": "none"}),
                           ("reset@mid_occl", CAUSAL[0]),
                           ("reset@mid_vis", CAUSAL[1]),
                           ("reset@occl_start", CAUSAL[2]),
                           ("noise0.5", CAUSAL[3]),
                           ("noise2.0", CAUSAL[4])]:
        g = {}
        for tag in sorted({r["tag"] for r in raw}):
            rows = [r for r in raw if r["tag"] == tag
                    and r["preset"] == "id"
                    and r["intervention"]["type"] == pred["type"]
                    and r["intervention"].get("at", "") == pred.get("at", "")
                    and r["intervention"].get("sigma", None)
                        == pred.get("sigma", None)]
            if rows:
                g[tag] = (
                    float(np.nanmean([r["pos_mae_occluded"] for r in rows])),
                    float(np.nanstd([r["pos_mae_occluded"] for r in rows])),
                )
        groups[iv_label] = g
    plots.plot_bars(groups, reports / "causal_pos_mae.png",
                    "Hidden-state interventions: occluded position MAE",
                    "pos MAE (normalized)")

    # --- OOD bars: pos_mae across presets -----
    groups = {}
    for preset in OOD_PRESET_LIST:
        g = {}
        for tag in sorted({r["tag"] for r in raw}):
            rows = [r for r in raw if r["tag"] == tag
                    and r["preset"] == preset
                    and r["intervention"]["type"] == "none"]
            if rows:
                g[tag] = (
                    float(np.nanmean([r["pos_mae_occluded"] for r in rows])),
                    float(np.nanstd([r["pos_mae_occluded"] for r in rows])),
                )
        groups[preset] = g
    plots.plot_bars(groups, reports / "ood_pos_mae.png",
                    "OOD position MAE by preset", "pos MAE (normalized)")

    # --- error vs bout length -----
    table = {}
    for tag in sorted({r["tag"] for r in raw}):
        rows = [r for r in raw if r["tag"] == tag and r["preset"] == "id"
                and r["intervention"]["type"] == "none"]
        if not rows or "boutlen_curve" not in rows[0]:
            continue
        # gather per-seed boutlen curves -> (mid, mean, std over seeds)
        bins = {}
        for r in rows:
            for cell in r["boutlen_curve"]:
                lo, hi = cell["range"].split("-")
                mid = (int(lo) + int(hi)) / 2
                bins.setdefault(mid, []).append(cell["pos_mae"])
        table[tag] = [(m, float(np.nanmean(v)), float(np.nanstd(v)))
                      for m, v in sorted(bins.items())]
    if table:
        plots.plot_metric_vs_boutlen(
            table, reports / "err_vs_boutlen.png",
            "Position error vs occlusion bout length (ID)")


if __name__ == "__main__":
    main()
