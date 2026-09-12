"""P0 sweep v2: multi-seed evaluation, causal ablations, OOD, aggregation.

Phases:
    baselines  eval all non-learned agents in-distribution, including the
               pure-clock frontier clock1..clock5 (C1: periodic polling must
               not reach the detection ceiling cheaply)
    causal     learned (best+final ckpt), belief-only control, and the
               surprise-consuming references under the full ablation battery
               (3 permutations each for pe_shuffle / ch_permute, pe_mask,
               D-channel pe_mask_ch, mask_belief, fresh_pred) (M2/M3)
    ood        eval learned + controls + references under OOD configs
    controls   round-3 additions (2nd adversarial review): belief-reactive
               baselines (vtrigger/vtrigger22/cwatch) causal+OOD, predictor
               provenance battery (trained/fresh x frozen predictor),
               stochastic eval of final ckpts
    all        run everything (expects trained ckpts under runs/)

Results land in reports/json/*.json and reports/results.jsonl.

Usage:
    python -m experiments.p0.sweep --seeds 0 1 2 3 4 --phase all
    P0_SMOKE=1 python -m experiments.p0.sweep --seeds 0 --phase all
"""

from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path
from typing import Dict, List, Optional

import numpy as np

from .agents.baselines import make_baseline
from .config import load_config
from .env import dynamics as dyn
from .evaluate import evaluate, load_predictor, make_policy

BASE = Path(__file__).resolve().parent
REPORTS = BASE / "reports"
CLOCK_KINDS = [f"clock{k}" for k in (1, 2, 3, 4, 5)]
BASELINE_KINDS = ["never", "random", "roundrobin", "always", "fixed",
                  "pe_heuristic", "pe_thresh", "trigger",
                  "vtrigger", "vtrigger22", "cwatch"] + CLOCK_KINDS
# Baselines added in round 3 (belief-reactive, no predictor).
CONTROL_KINDS = ["vtrigger", "vtrigger22", "cwatch"]
OOD_CONFIGS = [
    "ood_event_freq_up", "ood_event_freq_down", "ood_noise_up",
    "ood_dynamics", "ood_cost_up",
]

# Three distinct non-identity channel permutations, fixed across the whole
# sweep (M2: a single fixed permutation is not enough evidence).
def _perms() -> List[List[int]]:
    rng = np.random.default_rng(123)
    out = []
    while len(out) < 3:
        p = rng.permutation(dyn.N_CHANNELS)
        if np.all(p == np.arange(dyn.N_CHANNELS)):
            continue
        if any(np.array_equal(p, q) for q in out):
            continue
        out.append(p.tolist())
    return out

PERMS = _perms()

# Full battery applied to the surprise-consuming agents.
def causal_ablations() -> List[Dict]:
    ab = [{"type": "none", "tag": "id"}]
    for i, p in enumerate(PERMS):
        ab.append({"type": "pe_shuffle", "permutation": p, "tag": f"pe_shuffle_p{i}"})
    ab.append({"type": "pe_mask", "tag": "pe_mask"})
    ab.append({"type": "pe_mask_ch", "channel": dyn.CH_D, "tag": "pe_mask_D"})
    # M3: a permutation that MUST move D's e,d elsewhere AND give D a foreign
    # channel's surprise (p1 = [1,0,2,3] leaves channel D untouched).
    ab.append({"type": "pe_shuffle", "permutation": [3, 1, 2, 0],
               "tag": "pe_shuffle_D"})
    for i, p in enumerate(PERMS):
        ab.append({"type": "ch_permute", "permutation": p, "tag": f"ch_permute_p{i}"})
    ab.append({"type": "mask_belief", "channel": dyn.CH_D, "tag": "mask_belief"})
    return ab


def _episodes() -> int:
    return int(os.environ.get("P0_EVAL_EPISODES", "12"))


def _run_dir(seed: int, run: str = "base_mlp") -> Path:
    return BASE / "runs" / f"{run}_s{seed}"


def _ckpt(seed: int, run: str = "base_mlp", which: str = "best") -> Path:
    d = _run_dir(seed, run)
    best = d / f"ckpt_{which}.pt"
    if best.exists():
        return best
    return d / ("ckpt_final.pt" if which == "best" else "ckpt_best.pt")


def _save(row: Dict, name: str):
    (REPORTS / "json").mkdir(parents=True, exist_ok=True)
    (REPORTS / "json" / f"{name}.json").write_text(json.dumps(row, indent=2))


def _row(cfg, agent, condition, ablation, result, wall) -> Dict:
    agg = result["aggregate"]
    return {
        "seed": cfg.seed, "agent": agent, "condition": condition,
        "ablation": ablation.get("type", "none"), "config": cfg.name,
        "perm_tag": ablation.get("tag", ""),
        "detection_rate": agg["detection_rate_pooled"],
        "mean_cost": agg["mean_cost"]["mean"],
        "info_efficiency": agg["info_efficiency"]["mean"],
        "total_reward": agg["total_reward"]["mean"],
        "action_entropy": agg["action_entropy"]["mean"],
        "n_detected": agg["n_detected"], "n_missed": agg["n_missed"],
        "attention": agg["attention"], "surprise_mean": agg["surprise_mean"],
        "pred_err_mean": agg["pred_err_mean"],
        "n_episodes": agg["n_episodes"], "wall_time_s": wall,
    }


def eval_baselines(seeds: List[int], episodes: int,
                   kinds: Optional[List[str]] = None) -> List[Dict]:
    rows = []
    cfg = load_config(BASE / "configs" / "base.yaml")
    cfg.eval.episodes = episodes
    for seed in seeds:
        cfg.seed = seed
        for kind in (kinds or BASELINE_KINDS):
            t0 = time.time()
            res = evaluate(cfg, make_baseline(kind, seed=seed), episodes,
                           seed=seed)
            row = _row(cfg, kind, "id", {"type": "none"}, res,
                       round(time.time() - t0, 2))
            rows.append(row)
            _save(row, f"s{seed}_{kind}_id")
            print(f"  s{seed} {kind:>12}: det={row['detection_rate']:.2f} "
                  f"cost={row['mean_cost']:.4f} eff={row['info_efficiency']:.1f}",
                  flush=True)
    return rows


def _learned_specs(cfg, seed: int):
    """(label, policy, ckpt_path) for the learned agents.

    The predictor is (re)loaded from ckpt at every eval so its weights are
    identical across conditions -- no cross-condition training drift.
    """
    for label, which in (("learned", "best"), ("learned_final", "final")):
        ckpt = str(_ckpt(seed, "base_mlp", which))
        yield label, make_policy(cfg, ckpt), ckpt


def _bonly_spec(base_cfg, seed: int):
    cfg = load_config(BASE / "configs" / "base_bonly.yaml")
    cfg.seed = seed
    ckpt = str(_ckpt(seed, "base_bonly_mlp", "best"))
    pol = make_policy(cfg, ckpt)
    return pol, ckpt


def eval_causal(seeds: List[int], episodes: int) -> List[Dict]:
    """Causal ablations on learned agents + controls."""
    ablations = causal_ablations()
    short = [a for a in ablations if a["tag"] in ("id", "pe_mask", "mask_belief")]
    rows = []
    cfg = load_config(BASE / "configs" / "base.yaml")
    cfg.eval.episodes = episodes
    for seed in seeds:
        cfg.seed = seed
        # (label, policy, ckpt-or-None, ablation-list)
        agents = []
        for label, pol, ckpt in _learned_specs(cfg, seed):
            abl = ablations if label == "learned" else short
            agents.append([label, pol, ckpt, abl])
        bpol, bckpt = _bonly_spec(cfg, seed)
        agents.append(["bonly", bpol, bckpt, ablations])
        for label in ("fixed", "trigger"):
            agents.append([label, make_baseline(label, seed=seed), None, short])
        for label, pol, ckpt, abl in agents:
            for ab in abl:
                cond = "id" if ab["type"] == "none" else f"causal:{ab['tag']}"
                t0 = time.time()
                res = evaluate(cfg, pol, episodes, seed=seed, ablation=ab,
                               predictor=(load_predictor(cfg, ckpt, seed=seed)
                                          if ckpt else None))
                row = _row(cfg, label, cond, ab, res, round(time.time() - t0, 2))
                rows.append(row)
                _save(row, f"s{seed}_{label}_{cond.replace(':', '_')}")
                print(f"  s{seed} {label:>13} {cond:>24}: "
                      f"det={row['detection_rate']:.2f} "
                      f"cost={row['mean_cost']:.4f} "
                      f"eff={row['info_efficiency']:.1f}", flush=True)
            # fresh-predictor variant (M3): policy kept, predictor re-init.
            # Meaningful only for agents that consume e,d.
            if label in ("learned", "learned_final", "fixed", "trigger"):
                t0 = time.time()
                res = evaluate(cfg, pol, episodes, seed=seed,
                               ablation={"type": "none"}, predictor=None)
                row = _row(cfg, label, "causal:fresh_pred",
                           {"type": "fresh_pred"}, res,
                           round(time.time() - t0, 2))
                rows.append(row)
                _save(row, f"s{seed}_{label}_causal_fresh_pred")
                print(f"  s{seed} {label:>13} {'causal:fresh_pred':>24}: "
                      f"det={row['detection_rate']:.2f} "
                      f"cost={row['mean_cost']:.4f} "
                      f"eff={row['info_efficiency']:.1f}", flush=True)
    return rows


def eval_controls(seeds: List[int], episodes: int) -> List[Dict]:
    """Round-3 additions demanded by the second adversarial review.

    1. Belief-reactive baselines (vtrigger/vtrigger22/cwatch) under the short
       causal battery + all OOD configs (ID rows come from `phase baselines`).
    2. Predictor-provenance battery for the e/d-consuming agents:
         trained_pred    trained predictor (from the learned ckpt), still
                         learning during eval (= training-time semantics)
         trained_frozen  trained predictor, SGD frozen during eval
         fresh_frozen    fresh seeded predictor, frozen during eval
       The v2 `fresh_pred` rows on baselines were VACUOUS: baseline agents
       never had a trained predictor, so `id` already used a fresh one.
       frozen variants isolate eval-time predictor learning.
    3. Stochastic (sampled-action) eval of final ckpts: separates a truly
       collapsed policy from an argmax artifact (C-2).
    """
    rows = []
    cfg = load_config(BASE / "configs" / "base.yaml")
    bonly_cfg = load_config(BASE / "configs" / "base_bonly.yaml")
    cfg.eval.episodes = episodes
    bonly_cfg.eval.episodes = episodes
    short = [a for a in causal_ablations()
             if a["tag"] in ("id", "pe_mask", "mask_belief", "pe_shuffle_D",
                             "ch_permute_p0")]

    def emit(label, cond, ab, res, wall, seed, row_cfg):
        row = _row(row_cfg, label, cond, ab, res, round(wall, 2))
        row["agent"] = label
        rows.append(row)
        _save(row, f"s{seed}_{label}_{cond.replace(':', '_')}")
        print(f"  s{seed} {label:>20} {cond:>24}: "
              f"det={row['detection_rate']:.2f} "
              f"cost={row['mean_cost']:.4f} "
              f"eff={row['info_efficiency']:.1f}", flush=True)

    for seed in seeds:
        cfg.seed = seed
        bonly_cfg.seed = seed
        learned_ckpt = str(_ckpt(seed, "base_mlp", "best"))

        # --- 1. new baselines: short causal + OOD ---
        for kind in CONTROL_KINDS:
            pol = make_baseline(kind, seed=seed)
            for ab in short:
                cond = "id" if ab["type"] == "none" else f"causal:{ab['tag']}"
                t0 = time.time()
                res = evaluate(cfg, pol, episodes, seed=seed, ablation=ab)
                emit(kind, cond, ab, res, time.time() - t0, seed, cfg)
            for ood_name in OOD_CONFIGS:
                ocfg = load_config(BASE / "configs" / f"{ood_name}.yaml")
                ocfg.seed = seed
                ocfg.eval.episodes = episodes
                t0 = time.time()
                res = evaluate(ocfg, pol, episodes, seed=seed)
                emit(kind, f"ood:{ood_name}", {"type": "none"}, res,
                     time.time() - t0, seed, ocfg)

        # --- 2. predictor provenance battery ---
        for label in ("trigger", "fixed"):
            pol = make_baseline(label, seed=seed)
            for tag, pred, frz in (
                ("trained_pred",
                 load_predictor(cfg, learned_ckpt, seed=seed), True),
                ("trained_frozen",
                 load_predictor(cfg, learned_ckpt, seed=seed), False),
                ("fresh_frozen", None, False),
            ):
                t0 = time.time()
                res = evaluate(cfg, pol, episodes, seed=seed,
                               ablation={"type": "none"}, predictor=pred,
                               pred_train=frz)
                emit(label, f"causal:{tag}", {"type": tag}, res,
                     time.time() - t0, seed, cfg)

        # learned best ckpt: trained-frozen + fresh-frozen + D-moving shuffle
        # (M3: perms that must move D's e,d, unlike p1 which leaves it put)
        lpol = make_policy(cfg, learned_ckpt)
        for tag, pred in (
            ("trained_frozen", load_predictor(cfg, learned_ckpt, seed=seed)),
            ("fresh_frozen", None),
        ):
            t0 = time.time()
            res = evaluate(cfg, lpol, episodes, seed=seed,
                           ablation={"type": "none"}, predictor=pred,
                           pred_train=False)
            emit("learned", f"causal:{tag}", {"type": tag}, res,
                 time.time() - t0, seed, cfg)
        ab_d = {"type": "pe_shuffle", "permutation": [3, 1, 2, 0],
                "tag": "pe_shuffle_D"}
        t0 = time.time()
        res = evaluate(cfg, lpol, episodes, seed=seed, ablation=ab_d,
                       predictor=load_predictor(cfg, learned_ckpt, seed=seed))
        emit("learned", "causal:pe_shuffle_D", ab_d, res,
             time.time() - t0, seed, cfg)

        # --- 3. stochastic eval of final ckpts ---
        for label, pcfg, which in (
            ("learned_final_stoch", cfg, "final"),
            ("bonly_final_stoch", bonly_cfg, "final"),
        ):
            run = "base_mlp" if label.startswith("learned") else "base_bonly_mlp"
            ckpt = str(_ckpt(seed, run, which))
            pol = make_policy(pcfg, ckpt, deterministic=False)
            for ab in ({"type": "none", "tag": "id"},
                       {"type": "pe_mask", "tag": "pe_mask"}):
                cond = "id" if ab["type"] == "none" else f"causal:{ab['tag']}"
                t0 = time.time()
                res = evaluate(pcfg, pol, episodes, seed=seed, ablation=ab,
                               predictor=load_predictor(pcfg, ckpt, seed=seed))
                emit(label, cond, ab, res, time.time() - t0, seed, pcfg)
    return rows


def eval_ood(seeds: List[int], episodes: int) -> List[Dict]:
    rows = []
    base_cfg = load_config(BASE / "configs" / "base.yaml")
    bonly_cfg = load_config(BASE / "configs" / "base_bonly.yaml")
    for seed in seeds:
        base_cfg.seed = seed
        learned = list(_learned_specs(base_cfg, seed))
        bpol, bckpt = _bonly_spec(bonly_cfg, seed)
        for ood_name in OOD_CONFIGS:
            cfg = load_config(BASE / "configs" / f"{ood_name}.yaml")
            cfg.seed = seed
            cfg.eval.episodes = episodes
            cfg.agent.kind = base_cfg.agent.kind
            cfg.agent.hidden = base_cfg.agent.hidden
            agents = [(label, pol, ckpt) for label, pol, ckpt in learned]
            agents.append(("bonly", bpol, bckpt))
            for label in ("fixed", "trigger", "clock2"):
                agents.append((label, make_baseline(label, seed=seed), None))
            for label, pol, ckpt in agents:
                t0 = time.time()
                res = evaluate(
                    cfg, pol, episodes, seed=seed,
                    predictor=(load_predictor(cfg, ckpt, seed=seed)
                               if ckpt else None))
                row = _row(cfg, label, f"ood:{ood_name}",
                           {"type": "none"}, res, round(time.time() - t0, 2))
                row["agent"] = label
                rows.append(row)
                _save(row, f"s{seed}_{label}_{ood_name}")
                print(f"  s{seed} {label:>13} {ood_name:>20}: "
                      f"det={row['detection_rate']:.2f} "
                      f"cost={row['mean_cost']:.4f} "
                      f"eff={row['info_efficiency']:.1f}", flush=True)
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, nargs="+", default=[0, 1, 2])
    ap.add_argument("--phase", default="all",
                    choices=["baselines", "causal", "ood", "controls", "all"])
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--kinds", type=str, nargs="+", default=None,
                    help="restrict baselines phase to these agent kinds")
    args = ap.parse_args()
    episodes = args.episodes or _episodes()

    rows: List[Dict] = []
    if args.phase in ("baselines", "all"):
        print("== baselines (ID) ==", flush=True)
        rows += eval_baselines(args.seeds, episodes, kinds=args.kinds)
    if args.phase in ("causal", "all"):
        print("== causal ablations ==", flush=True)
        rows += eval_causal(args.seeds, episodes)
    if args.phase in ("ood", "all"):
        print("== OOD ==", flush=True)
        rows += eval_ood(args.seeds, episodes)
    if args.phase in ("controls", "all"):
        print("== round-3 controls ==", flush=True)
        rows += eval_controls(args.seeds, episodes)

    REPORTS.mkdir(parents=True, exist_ok=True)
    with open(REPORTS / "results.jsonl", "a") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print(f"wrote {REPORTS / 'results.jsonl'} ({len(rows)} rows)", flush=True)


if __name__ == "__main__":
    main()
