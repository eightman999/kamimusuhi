"""U0 Protocol Sanity Gate (Phase C).

Runs the baseline-only matrix on the production environment and enforces
the P0 gates. No PPO training is allowed until every gate passes: a
PROTOCOL_FAIL means the environment itself cannot distinguish memory use
from luck, so learning curves would be uninformative.

Gates (thresholds fixed in analysis/metrics.py — do not relax after the
fact):
    P0-1  oracle need_resolution >= 0.90
    P0-2  no_memory need_resolution < 0.40
    P0-3  oracle crisis_error_auc < no_memory crisis_error_auc
    P0-4  oracle clearly not worse than fifo/random on primary metrics
          (higher need_resolution AND lower crisis_error_auc)
    P0-5  oracle with its memory erased degrades clearly
          (need_resolution drops by >= 0.30)

Usage:
    python -m experiments.u0.protocol_check [--episodes 512]

Writes:
    experiments/u0/reports/U0_PROTOCOL_CHECK.md
    <artifacts>/results/protocol_check.json   (machine-readable verdict)

Exit code: 0 = all gates pass, 2 = PROTOCOL_FAIL.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

from .analysis.metrics import GATE_THRESHOLDS
from .config import env_config, load_config
from .evaluate import evaluate_baseline
from .policies.baselines import BASELINES

SANITY_BASELINES = ["oracle", "no_memory", "random", "fifo", "lru",
                    "store_all", "heuristic_current_need"]


def _git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"],
            stderr=subprocess.DEVNULL).decode().strip()
    except Exception:
        return "unknown"


def check_protocol(config_path: str, episodes: int = 512,
                   seed: int = 900001, verbose: bool = True) -> dict:
    """Run the baseline matrix and evaluate the P0 gates.

    Returns {"all_pass": bool, "gates": {...}, "metrics": {...}}.
    """
    raw = load_config(config_path)
    cfg = env_config(raw, seed=seed)
    t0 = time.time()

    metrics: dict[str, dict] = {}
    for name in SANITY_BASELINES:
        metrics[f"{name}/clean"] = evaluate_baseline(
            name, cfg, episodes=episodes, seed=seed)
        if verbose:
            m = metrics[f"{name}/clean"]
            print(f"[protocol] {name:22s} res={m['need_resolution']:.3f} "
                  f"auc={m['crisis_error_auc']:.2f} "
                  f"ttr={m['time_to_resolution']:.1f} "
                  f"surv={m['survival_after_need']:.3f}", flush=True)
    metrics["oracle/erase"] = evaluate_baseline(
        "oracle", cfg, episodes=episodes, seed=seed, causal="erase")
    if verbose:
        m = metrics["oracle/erase"]
        print(f"[protocol] {'oracle+erase':22s} res={m['need_resolution']:.3f} "
              f"auc={m['crisis_error_auc']:.2f}", flush=True)

    g = GATE_THRESHOLDS
    orc, nom = metrics["oracle/clean"], metrics["no_memory/clean"]
    fifo, rnd = metrics["fifo/clean"], metrics["random/clean"]
    era = metrics["oracle/erase"]

    gates = {
        "P0-1": {
            "desc": "oracle need_resolution >= "
                    f"{g['P0_oracle_resolution']}",
            "value": orc["need_resolution"],
            "pass": bool(orc["need_resolution"]
                         >= g["P0_oracle_resolution"]),
        },
        "P0-2": {
            "desc": "no_memory need_resolution < "
                    f"{g['P0_no_memory_resolution']}",
            "value": nom["need_resolution"],
            "pass": bool(nom["need_resolution"]
                         < g["P0_no_memory_resolution"]),
        },
        "P0-3": {
            "desc": "oracle crisis_error_auc < no_memory crisis_error_auc",
            "value": {"oracle": orc["crisis_error_auc"],
                      "no_memory": nom["crisis_error_auc"]},
            "pass": bool(orc["crisis_error_auc"]
                         < nom["crisis_error_auc"]),
        },
        "P0-4": {
            "desc": "oracle not worse than fifo/random on primary "
                    "metrics (resolution AND crisis_error_auc)",
            "value": {"oracle_res": orc["need_resolution"],
                      "fifo_res": fifo["need_resolution"],
                      "random_res": rnd["need_resolution"],
                      "oracle_auc": orc["crisis_error_auc"],
                      "fifo_auc": fifo["crisis_error_auc"],
                      "random_auc": rnd["crisis_error_auc"]},
            "pass": bool(
                orc["need_resolution"] >= max(fifo["need_resolution"],
                                              rnd["need_resolution"])
                and orc["crisis_error_auc"] <= fifo["crisis_error_auc"]
                and orc["crisis_error_auc"] <= rnd["crisis_error_auc"]),
        },
        "P0-5": {
            "desc": "erased oracle degrades clearly (resolution drop "
                    f">= {g['P0_oracle_erase_drop']})",
            "value": {"clean_res": orc["need_resolution"],
                      "erased_res": era["need_resolution"],
                      "drop": orc["need_resolution"]
                              - era["need_resolution"]},
            "pass": bool(orc["need_resolution"] - era["need_resolution"]
                         >= g["P0_oracle_erase_drop"]),
        },
    }
    all_pass = all(gg["pass"] for gg in gates.values())
    return {"all_pass": all_pass,
            "verdict": "PASS" if all_pass else "PROTOCOL_FAIL",
            "gates": gates, "metrics": metrics,
            "episodes": episodes, "seed": seed,
            "elapsed_sec": round(time.time() - t0, 1),
            "git_commit": _git_commit()}


def write_report(result: dict, out: Path) -> None:
    L = ["# U0 Protocol Sanity Check\n",
         f"- verdict: **{result['verdict']}**\n",
         f"- episodes per condition: {result['episodes']} "
         f"(seed {result['seed']})\n",
         f"- git commit: `{result['git_commit']}`\n",
         f"- elapsed: {result['elapsed_sec']} s\n",
         "\n## Baseline matrix (clean)\n",
         "| baseline | need_resolution | crisis_error_auc | "
         "time_to_resolution | survival_after_need | retention |\n",
         "|---|---|---|---|---|---|\n"]
    for name in SANITY_BASELINES:
        m = result["metrics"][f"{name}/clean"]
        L.append(f"| {name} | {m['need_resolution']:.3f} | "
                 f"{m['crisis_error_auc']:.3f} | "
                 f"{m['time_to_resolution']:.1f} | "
                 f"{m['survival_after_need']:.3f} | "
                 f"{m['important_retention']:.3f} |\n")
    m = result["metrics"]["oracle/erase"]
    L.append(f"| oracle + erase | {m['need_resolution']:.3f} | "
             f"{m['crisis_error_auc']:.3f} | "
             f"{m['time_to_resolution']:.1f} | "
             f"{m['survival_after_need']:.3f} | "
             f"{m['important_retention']:.3f} |\n")
    L.append("\n## Gates\n| gate | criterion | value | verdict |\n"
             "|---|---|---|---|\n")
    for name, gg in result["gates"].items():
        val = gg["value"]
        if isinstance(val, float):
            val_s = f"{val:.3f}"
        elif isinstance(val, dict):
            val_s = ", ".join(f"{k}={v:.3f}" for k, v in val.items())
        else:
            val_s = str(val)
        L.append(f"| {name} | {gg['desc']} | {val_s} | "
                 f"{'PASS' if gg['pass'] else 'FAIL'} |\n")
    if result["all_pass"]:
        L.append("\nAll gates passed — PPO training is allowed.\n")
    else:
        L.append("\n**PROTOCOL_FAIL** — the environment cannot "
                 "distinguish memory use from luck; do not train.\n")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(L))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="experiments/u0/configs/default.yaml")
    ap.add_argument("--episodes", type=int, default=512)
    ap.add_argument("--seed", type=int, default=900001)
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--report",
                    default="experiments/u0/reports/U0_PROTOCOL_CHECK.md")
    args = ap.parse_args()

    result = check_protocol(args.config, args.episodes, args.seed)
    write_report(result, Path(args.report))
    res_dir = Path(args.artifacts) / "results"
    res_dir.mkdir(parents=True, exist_ok=True)
    (res_dir / "protocol_check.json").write_text(
        json.dumps(result, indent=2))
    print(f"[protocol] verdict={result['verdict']} "
          f"({sum(g['pass'] for g in result['gates'].values())}/"
          f"{len(result['gates'])} gates) -> {args.report}")
    return 0 if result["all_pass"] else 2


if __name__ == "__main__":
    sys.exit(main())
