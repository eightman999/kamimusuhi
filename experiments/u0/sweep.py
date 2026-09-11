"""U0 sweep orchestrator: train seeds x models, run the eval battery
(clean + causal U-C1..C5 + OOD), consolidate multi-seed stats and write
figures.

    python -m experiments.u0.sweep --config experiments/u0/configs/default.yaml \
        --seeds 0 1 2 [--skip-train] [--which best|final]
"""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import yaml

from .analysis import stats as st
from .analysis.plots import (plot_action_by_regime, plot_conf_vs_error,
                             plot_ood_bars, plot_reliability,
                             plot_risk_coverage)
from .env.uncertainty_env import U0Config
from .evaluate import (CAUSAL_MODES, EVAL_SEED, OOD_MODES, eval_battery,
                       validate_env)

METRICS = ["mean_reward", "accuracy", "answer_rate", "abstain_rate",
           "mean_obs", "ece_conf", "ece_maxprob", "aurc", "sel_acc_60",
           "err_auroc", "nll", "brier"]


def run(cmd: list[str]) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)


def consolidate(all_results: list[dict], out: Path) -> dict:
    """Multi-seed stats per subject x condition."""
    table = {}
    for res in all_results:
        for subj, conds in res["subjects"].items():
            for cond, r in conds.items():
                key = (subj, cond)
                table.setdefault(key, []).append(r["metrics"])
    rng = np.random.default_rng(0)
    summary = {}
    for (subj, cond), mets in sorted(table.items()):
        row = {"subject": subj, "condition": cond,
               "n_seeds": len(mets)}
        for mname in METRICS:
            vals = [m.get(mname, float("nan")) for m in mets]
            d = st.describe(vals)
            row[mname + "_mean"] = d["mean"]
            row[mname + "_sd"] = d["sd"]
            row[mname + "_median"] = d["median"]
            row[mname + "_ci95"] = d["ci95"]
        summary[f"{subj}|{cond}"] = row
    # effect sizes vs always_answer on clean
    aa = [r["subjects"]["always_answer"]["clean"]["metrics"]["mean_reward"]
          for r in all_results]
    for subj in {k[0] for k in table}:
        vals = [r["subjects"][subj]["clean"]["metrics"]["mean_reward"]
                for r in all_results]
        summary[f"{subj}|clean"]["d_vs_always_answer"] = \
            st.cohens_d(vals, aa)
    return summary


def make_figures(all_results: list[dict], fig_dir: Path,
                 cfg: U0Config) -> None:
    fig_dir.mkdir(parents=True, exist_ok=True)
    # merge clean records across seeds per subject
    merged: dict[str, list] = {}
    ood_abstain: dict[str, dict] = {}
    for res in all_results:
        for subj, conds in res["subjects"].items():
            merged.setdefault(subj, []).extend(
                conds["clean"].get("records", []))
            ood_abstain.setdefault(subj, {})
            ood_abstain[subj]["clean"] = \
                conds["clean"]["metrics"]["abstain_rate"]
            for mode in OOD_MODES:
                key = f"ood_{mode}"
                if key in conds:
                    ood_abstain[subj][key] = \
                        conds[key]["metrics"]["abstain_rate"]
    # risk-coverage: learned + key baselines + oracle
    rc_subjects = {s: merged[s] for s in
                   ["learned", "always_answer", "conf_gated", "ensemble",
                    "threshold", "oracle"] if s in merged}
    plot_risk_coverage(rc_subjects, fig_dir / "risk_coverage.png")
    # reliability of the learned agent's confidence
    ans = [r for r in merged.get("learned", []) if r["outcome"] == "answer"]
    if ans:
        conf = np.asarray([r["conf"] for r in ans])
        cor = np.asarray([r["correct"] for r in ans], dtype=float)
        plot_reliability(conf, cor, fig_dir / "reliability_learned.png",
                         title="Learned policy confidence")
        plot_conf_vs_error(conf, cor, fig_dir / "conf_vs_error.png")
    # action usage vs difficulty
    act_subjects = {s: merged[s] for s in
                    ["learned", "always_answer", "conf_gated", "oracle"]
                    if s in merged}
    plot_action_by_regime(act_subjects, fig_dir / "actions_by_regime.png")
    plot_ood_bars({s: ood_abstain[s] for s in
                   ["learned", "always_answer", "conf_gated", "entropy",
                    "oracle"] if s in ood_abstain},
                  fig_dir / "ood_abstain.png")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config",
                    default="experiments/u0/configs/default.yaml")
    ap.add_argument("--seeds", nargs="+", type=int, default=[0, 1, 2])
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--aux-episodes", type=int, default=None)
    ap.add_argument("--eval-seed", type=int, default=EVAL_SEED)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--which", default="best", choices=["best", "final"])
    ap.add_argument("--skip-train", action="store_true")
    ap.add_argument("--validate-env", action="store_true")
    args = ap.parse_args()

    raw = yaml.safe_load(open(args.config))
    cfg = U0Config(**raw.get("env", {}))
    ev = raw.get("eval", {})
    episodes = args.episodes or ev.get("episodes", 512)
    aux = args.aux_episodes or ev.get("aux_episodes", 256)
    art = Path(args.artifacts)
    res_dir = art / "results"
    res_dir.mkdir(parents=True, exist_ok=True)

    if args.validate_env:
        val = validate_env(cfg)
        slim = {s: {"metrics": r["metrics"], "per_regime": r["per_regime"]}
                for s, r in val.items()}
        (res_dir / "env_validation.json").write_text(
            json.dumps(slim, indent=2, allow_nan=False))
        print(json.dumps(
            {s: r["metrics"] for s, r in val.items()},
            indent=2, allow_nan=False))

    run_ids = []
    for seed in args.seeds:
        run_id = f"s{seed}"
        run_dir = art / "runs" / run_id
        if not args.skip_train and not (run_dir / "done.json").exists():
            run([sys.executable, "-m", "experiments.u0.train",
                 "--config", args.config, "--seed", str(seed),
                 "--artifacts", args.artifacts, "--run-id", run_id,
                 "--device", args.device])
        run_ids.append(run_id)

    all_results = []
    for run_id in run_ids:
        run_dir = art / "runs" / run_id
        out = res_dir / f"eval_{run_id}_{args.which}.json"
        if out.exists():
            all_results.append(json.loads(out.read_text()))
            continue
        res = eval_battery(run_dir, cfg, episodes=episodes,
                           aux_episodes=aux, seed=args.eval_seed,
                           device=args.device, out_dir=res_dir,
                           which=args.which)
        all_results.append(res)

    summary = consolidate(all_results, res_dir)
    (res_dir / "consolidated.json").write_text(
        json.dumps(summary, indent=2, allow_nan=False))
    with open(res_dir / "summary.csv", "w", newline="") as f:
        flat = []
        for key, row in summary.items():
            flat_row = {k: v for k, v in row.items()}
            flat.append(flat_row)
        w = csv.DictWriter(f, fieldnames=list(flat[0].keys()))
        w.writeheader()
        w.writerows(flat)
    make_figures(all_results, art / "figures", cfg)
    print(f"wrote {res_dir}/consolidated.json + summary.csv + figures",
          flush=True)


if __name__ == "__main__":
    main()
