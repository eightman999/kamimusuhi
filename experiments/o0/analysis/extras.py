"""Round-2 reviewer extras: cheap analyses that need no retraining.

Computes, on the fixed eval stream (seed 910000):

1. post-window none baseline: hidden-alive pos MAE restricted to steps
   after each episode's mid_occl anchor, with NO intervention -- the
   fair denominator for post-reset degradation ratios (M-c).
2. post-reset state reversion: mean predicted pos/exist/vel on hidden
   steps after reset@mid_occl vs un-perturbed pre-appearance steps.
3. decoy-takeover episode stats: steps per affected episode
   (mean/p90/max), decoy-step pos error and identity rejection.
4. paired bootstrap CIs: per-preset delta of pos_mae_occluded between
   each learned model and the corridor/constvel baselines, resampling
   EPISODES (the independent unit), pooled over seeds.
5. hidden-state linear probe: ridge-decode pos/vel/exist from the
   recurrent state at each occlusion depth k (2-fold across episodes).

Usage:
    python -m experiments.o0.analysis.extras --config configs/full.yaml \
        --artifacts artifacts --out reports/extras.json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from ..env import dynamics as dyn
from ..evaluate import (evaluate_heuristic, intervention_steps,
                        predict_heuristic, predict_model, preset_params)
from ..models import make_model
from ..train import atomic
from . import metrics as metrics_mod

MODELS = ["mlp", "ssm", "gru64", "gru128", "lstm64"]
HEURS = ["prior", "lastobs", "constvel", "corridor", "openloop",
         "corridordyn", "kalman"]
SEEDS = [0, 1, 2, 3, 4]
EVAL_SEED = 910000
N_EP = 512


def load_trained(artifacts, arch, seed):
    run = Path(artifacts) / "runs" / f"{arch}_s{seed}"
    ck = run / "best.pt"
    if not ck.exists():
        ck = run / "final.pt"
    m = make_model(arch)
    cp = torch.load(ck, map_location="cpu", weights_only=True)
    m.load_state_dict(cp["model"])
    m.eval()
    return m


def hidden_alive_mask(data):
    bout = metrics_mod.first_bout_masks(data)
    return bout & ~data["visible"] & (data["exist"] > 0.5)


def ep_mae(pred, data, mask):
    """Per-episode mean pos error over `mask` (nan if empty)."""
    err = np.abs(pred["pos"] - data["pos"])
    T, B = err.shape
    out = np.full(B, np.nan)
    for b in range(B):
        mb = mask[:, b]
        if mb.any():
            out[b] = err[mb, b].mean()
    return out


# ---------------------------------------------------------------------------
# 1-2. post-window baselines and reset reversion
# ---------------------------------------------------------------------------

def post_window_stats(artifacts, p):
    """For every predictor: hidden-alive pos MAE on steps after mid_occl,
    with and without reset@mid_occl.  5-seed means for learned models."""
    data = dyn.generate_batch(p, N_EP, np.random.default_rng(EVAL_SEED))
    rs = intervention_steps(data, "mid_occl")
    T = data["obs"].shape[0]
    t = np.arange(T)[:, None]
    post = hidden_alive_mask(data) & (t > rs[None, :]) & (rs[None, :] >= 0)
    post_hid = metrics_mod.first_bout_masks(data) & ~data["visible"] & (
        t > rs[None, :]) & (rs[None, :] >= 0)
    out = {}
    for name in HEURS:
        b = predict_heuristic(name, data, p, {"type": "none"})
        r = predict_heuristic(name, data, p,
                              {"type": "hidden_reset", "at": "mid_occl"})
        e = lambda pr: float(np.abs(pr["pos"] - data["pos"])[post].mean())
        ex = lambda pr: float(
            ((pr["exist"] > 0.5) == (data["exist"] > 0.5))[post_hid].mean())
        out[f"heur:{name}"] = {
            "post_mae_none": e(b), "post_mae_reset": e(r),
            "post_exist_acc_reset": ex(r),
        }
    for arch in MODELS:
        none_e, reset_e, reset_ex, post_exist_prob = [], [], [], []
        for s in SEEDS:
            m = load_trained(artifacts, arch, s)
            b = predict_model(m, data, {"type": "none"})
            r = predict_model(m, data, {"type": "hidden_reset",
                                        "at": "mid_occl"})
            e = lambda pr: float(
                np.abs(pr["pos"] - data["pos"])[post].mean())
            none_e.append(e(b))
            reset_e.append(e(r))
            reset_ex.append(float(
                ((r["exist"] > 0.5) == (data["exist"] > 0.5))
                [post_hid].mean()))
            post_exist_prob.append(float(r["exist"][post_hid].mean()))
        out[arch] = {
            "post_mae_none": float(np.mean(none_e)),
            "post_mae_none_seeds": none_e,
            "post_mae_reset": float(np.mean(reset_e)),
            "post_mae_reset_seeds": reset_e,
            "post_exist_acc_reset": float(np.mean(reset_ex)),
            "post_exist_prob_reset": float(np.mean(post_exist_prob)),
            "degradation": float(np.mean(reset_e) / np.mean(none_e)),
        }
    # reset->prior reversion check (gru128_s0 vs pre-appearance prior)
    m = load_trained(artifacts, "gru128", 0)
    r = predict_model(m, data, {"type": "hidden_reset", "at": "mid_occl"})
    b0 = predict_model(m, data, {"type": "none"})
    pre = t < data["t_appear"][None, :]
    out["_prior_reversion_gru128_s0"] = {
        "post_reset_hidden": {
            "pos": float(r["pos"][post].mean()),
            "exist_prob": float(r["exist"][post_hid].mean()),
            "vel": float(r["vel"][post].mean()),
        },
        "pre_appearance": {
            "pos": float(b0["pos"][pre].mean()),
            "exist_prob": float(b0["exist"][pre].mean()),
            "vel": float(b0["vel"][pre].mean()),
        },
    }
    return out


# ---------------------------------------------------------------------------
# 3. takeover stats
# ---------------------------------------------------------------------------

def takeover_stats(artifacts, p):
    pt = preset_params(p, "decoytk")
    data = dyn.generate_batch(pt, N_EP, np.random.default_rng(EVAL_SEED))
    tk = data["decoy"]
    per_ep = tk.sum(axis=0)
    aff = per_ep[per_ep > 0]
    stats = {
        "episodes_affected_frac": float((per_ep > 0).mean()),
        "steps_mean": float(aff.mean()) if aff.size else 0.0,
        "steps_p90": float(np.percentile(aff, 90)) if aff.size else 0.0,
        "steps_max": int(aff.max()) if aff.size else 0,
        "steps_total": int(tk.sum()),
    }
    err = lambda pred: float(
        np.abs(pred["pos"] - data["pos"])[tk & (data["exist"] > 0.5)]
        .mean())
    same = lambda pred: float(
        ((pred["same"] > 0.5) == (data["same"] > 0.5))[tk].mean())
    for name in HEURS:
        pr = predict_heuristic(name, data, pt, {"type": "none"})
        stats[f"heur:{name}"] = {"pos_err_decoy": err(pr),
                                 "id_acc_decoy": same(pr)}
    for arch in MODELS:
        e_, i_ = [], []
        for s in SEEDS:
            m = load_trained(artifacts, arch, s)
            pr = predict_model(m, data, {"type": "none"})
            e_.append(err(pr))
            i_.append(same(pr))
        stats[arch] = {"pos_err_decoy": float(np.mean(e_)),
                       "pos_err_decoy_seeds": e_,
                       "id_acc_decoy": float(np.mean(i_)),
                       "id_acc_decoy_seeds": i_}
    return stats


# ---------------------------------------------------------------------------
# 4. paired bootstrap CIs
# ---------------------------------------------------------------------------

def paired_bootstrap(artifacts, p, presets, compare=(), n_boot=2000,
                     seed=123):
    """Paired episode-resampled bootstrap of the pos_mae_occluded delta
    between each learned model (pooled over seeds) and each baseline in
    `compare`.  Episodes are the independent unit; the eval stream is
    shared so episode b is the SAME episode for every predictor.
    """
    rng = np.random.default_rng(seed)
    out = {}
    for preset in presets:
        pp = preset_params(p, preset)
        data = dyn.generate_batch(pp, N_EP,
                                  np.random.default_rng(EVAL_SEED))
        mask = hidden_alive_mask(data)
        per_ep = {}
        for name in compare:
            pr = predict_heuristic(name, data, pp, {"type": "none"})
            per_ep[name] = ep_mae(pr, data, mask)
        for arch in MODELS:
            mats = []
            for s in SEEDS:
                m = load_trained(artifacts, arch, s)
                pr = predict_model(m, data, {"type": "none"})
                mats.append(ep_mae(pr, data, mask))
            per_ep[arch] = np.nanmean(np.stack(mats), axis=0)  # seed-mean/ep
        ok = np.isfinite(per_ep[compare[0]])
        for k in per_ep:
            ok &= np.isfinite(per_ep[k])
        idx = np.where(ok)[0]
        entry = {}
        for arch in MODELS:
            for base in compare:
                d = per_ep[arch][idx] - per_ep[base][idx]
                boots = d[rng.integers(0, len(idx), (n_boot, len(idx)))].mean(1)
                entry[f"{arch}-{base}"] = {
                    "delta": float(d.mean()),
                    "ci95_lo": float(np.percentile(boots, 2.5)),
                    "ci95_hi": float(np.percentile(boots, 97.5)),
                    "p_model_wins": float((boots < 0).mean()),
                    "n_episodes": int(len(idx)),
                }
        out[preset] = entry
    return out


# ---------------------------------------------------------------------------
# 5. hidden-state linear probe
# ---------------------------------------------------------------------------

def _ridge_r2(Xtr, ytr, Xte, yte, lam=1e-2):
    Xtr = np.concatenate([Xtr, np.ones((Xtr.shape[0], 1))], 1)
    Xte = np.concatenate([Xte, np.ones((Xte.shape[0], 1))], 1)
    A = Xtr.T @ Xtr + lam * np.trace(Xtr.T @ Xtr) / Xtr.shape[1] * np.eye(
        Xtr.shape[1])
    w = np.linalg.solve(A, Xtr.T @ ytr)
    pred = Xte @ w
    denom = np.sum((yte - yte.mean()) ** 2)
    if denom < 1e-9:                      # degenerate constant target
        return float("nan")
    return float(1.0 - np.sum((pred - yte) ** 2) / denom)


def hidden_probe(artifacts, p, arch="gru128", seed=0, n_ep=256, max_k=40,
                 preset="id", targets=("pos", "vel")):
    """Decode target quantities from the recurrent state at occlusion
    depth k.  2-fold across episodes; R² vs k for steps inside the first
    occlusion bout (alive steps for pos/vel; for exist, run on the
    gone20 preset so dead-hidden steps supply variance).
    """
    pp = preset_params(p, preset)
    data = dyn.generate_batch(pp, n_ep, np.random.default_rng(EVAL_SEED))
    obs = torch.as_tensor(data["obs"], dtype=torch.float32)
    T, B, _ = obs.shape
    m = load_trained(artifacts, arch, seed)
    H = m.hidden_size
    if arch == "lstm64":
        H = 2 * m.hidden_size
    states = torch.zeros(T, B, H)
    state = m.initial_state(B, "cpu")
    with torch.no_grad():
        for tt in range(T):
            _, _, _, state = m(obs[tt], state)
            states[tt] = state
    states = states.numpy()
    kidx = np.arange(T)[:, None] - data["t_occl"][None, :] + 1
    bout = metrics_mod.first_bout_masks(data)
    alive_hid = bout & ~data["visible"] & (data["exist"] > 0.5)
    dead_hid = bout & ~data["visible"] & (data["exist"] <= 0.5)
    res = {"arch": arch, "seed": seed, "preset": preset,
           "targets": list(targets), "by_k": []}
    tgt = {"pos": data["pos"], "vel": data["vel"], "exist": data["exist"]}
    for k in range(1, max_k + 1):
        row = {"k": k}
        sel = alive_hid & (kidx == k)
        if sel.sum() >= 40:
            X = states[sel]
            eps = np.tile(np.arange(B), (T, 1))[sel]
            fold = eps % 2 == 0
            row["n"] = int(sel.sum())
            for name in targets:
                if name == "exist":
                    continue
                y = tgt[name]
                row[f"r2_{name}"] = _ridge_r2(X[fold], y[sel][fold],
                                              X[~fold], y[sel][~fold])
        if "exist" in targets:
            sel_e = (alive_hid | dead_hid) & (kidx == k)
            if sel_e.sum() >= 40 and dead_hid[sel_e].sum() >= 10:
                X = states[sel_e]
                eps = np.tile(np.arange(B), (T, 1))[sel_e]
                fold = eps % 2 == 0
                y = tgt["exist"]
                row["n_exist"] = int(sel_e.sum())
                row["frac_dead"] = float(dead_hid[sel_e].mean())
                row["r2_exist"] = _ridge_r2(X[fold], y[sel_e][fold],
                                            X[~fold], y[sel_e][~fold])
        if "n" in row or "n_exist" in row:
            res["by_k"].append(row)
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="experiments/o0/configs/full.yaml")
    ap.add_argument("--artifacts", default="experiments/o0/artifacts")
    ap.add_argument("--out", default="experiments/o0/reports/extras.json")
    ap.add_argument("--quick", action="store_true",
                    help="skip the bootstrap (slowest section)")
    args = ap.parse_args()
    from ..config import load_config

    cfg = load_config(args.config)
    p = cfg.env
    out = {"eval_seed": EVAL_SEED, "n_episodes": N_EP}

    print("[extras] post-window + reset-reversion stats", flush=True)
    out["post_window"] = post_window_stats(args.artifacts, p)

    print("[extras] takeover stats", flush=True)
    out["takeover"] = takeover_stats(args.artifacts, p)

    if not args.quick:
        print("[extras] paired bootstrap (close calls)", flush=True)
        out["paired_bootstrap"] = paired_bootstrap(
            args.artifacts, p,
            presets=["id", "slow_v", "v_flip", "drag_x3", "app_jitter",
                     "swap_half", "plen48"],
            compare=["corridor", "constvel"])

    print("[extras] hidden-state linear probe", flush=True)
    out["probe_pos"] = [
        hidden_probe(args.artifacts, p, arch=a)
        for a in ("gru128", "gru64", "lstm64", "ssm", "mlp")
    ]
    out["probe_exist"] = [
        hidden_probe(args.artifacts, p, arch=a, preset="gone20",
                     targets=("exist",))
        for a in ("gru128", "gru64", "lstm64", "ssm", "mlp")
    ]

    atomic(args.out, out)
    print(f"wrote {args.out}")

    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt

        fig, ax = plt.subplots(figsize=(6, 3.5))
        for pr in out["probe_pos"]:
            ks = [r["k"] for r in pr["by_k"] if "r2_pos" in r]
            r2 = [r["r2_pos"] for r in pr["by_k"] if "r2_pos" in r]
            ax.plot(ks, r2, marker="o", ms=3, label=pr["arch"])
        ax.axvspan(4, 16, alpha=0.08, color="k")
        ax.set_xlabel("occlusion depth k")
        ax.set_ylabel("probe R² (linear decode of true pos)")
        ax.set_title("Hidden state encodes target position, decaying with depth")
        ax.set_ylim(-0.1, 1.05)
        ax.legend(fontsize=8)
        fig.tight_layout()
        fig.savefig(Path(args.out).parent / "probe_pos_r2.png", dpi=150)
        print("wrote probe_pos_r2.png")
    except Exception as ex:  # pragma: no cover - plotting is optional
        print(f"probe plot skipped: {ex}")


if __name__ == "__main__":
    main()
