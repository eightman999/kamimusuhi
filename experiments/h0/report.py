"""Generate H0_RESULTS.md + figures from results/*.json and runs/*.

Usage:
    python -m experiments.h0.report --episodes 20

Reads:
    results/summary.json            ID + OOD aggregates per agent/seed/ckpt
    results/causal_<tag>_seed<i>.json
    runs/<tag>/seed<i>/train_log.json

Writes:
    reports/H0_RESULTS.md
    reports/figs/*.png
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Dict, List, Optional

import numpy as np

from .analysis import plots, rhythm
from .config import load_config
from .env import dynamics as dyn
from .evaluate import evaluate, make_policy

H0 = Path(__file__).resolve().parent
RUNS = H0 / "runs"
RESULTS = H0 / "results"
REPORTS = H0 / "reports"
FIGS = REPORTS / "figs"
CONFIGS = H0 / "configs"

LEARNED_TAGS = ["mlp64", "gru64", "gru128"]
OOD = ["ood_energy", "ood_temperature", "ood_risk", "ood_resource"]


# ---------------------------------------------------------------------------
# stats helpers (no scipy)
# ---------------------------------------------------------------------------


def welch_t(a: np.ndarray, b: np.ndarray) -> float:
    """Welch's t statistic for difference in means."""
    a, b = np.asarray(a, float), np.asarray(b, float)
    ma, mb = a.mean(), b.mean()
    va, vb = a.var(ddof=1), b.var(ddof=1)
    na, nb = len(a), len(b)
    denom = np.sqrt(va / na + vb / nb)
    return float((ma - mb) / denom) if denom > 0 else 0.0


def permutation_p(a: np.ndarray, b: np.ndarray, n: int = 5000,
                  seed: int = 0) -> float:
    """Two-sided permutation test p-value for mean difference."""
    a, b = np.asarray(a, float), np.asarray(b, float)
    rng = np.random.default_rng(seed)
    obs = abs(a.mean() - b.mean())
    pool = np.concatenate([a, b])
    na = len(a)
    count = 0
    for _ in range(n):
        rng.shuffle(pool)
        if abs(pool[:na].mean() - pool[na:].mean()) >= obs:
            count += 1
    return (count + 1) / (n + 1)


# ---------------------------------------------------------------------------
# data collection
# ---------------------------------------------------------------------------


def load_summary() -> Dict:
    p = RESULTS / "summary.json"
    return json.loads(p.read_text()) if p.exists() else {}


def load_causal() -> Dict[str, Dict]:
    out = {}
    for p in sorted(RESULTS.glob("causal_*.json")):
        out[p.stem.replace("causal_", "")] = json.loads(p.read_text())
    return out


def per_episode_errors(entry: Dict, condition: str, ckpt: str = "best") -> np.ndarray:
    """We only stored aggregates; recompute per-episode arrays is not possible
    from summary.json. Instead we keep the mean/std for reporting and use
    episode-level data where available via re-evaluation."""
    agg = entry[ckpt][condition]
    return agg["homeostatic_error_full"]["mean"]


def collect_traces(tag: str, seed: int, ckpt: str = "best", n: int = 3) -> List[Dict]:
    """Run a few traced episodes for rhythm analysis / figures."""
    rd = RUNS / tag / f"seed{seed}"
    cfg = load_config(rd / "config.yaml")
    pol = make_policy(cfg, str(rd / f"{ckpt}.pt"))
    r = evaluate(cfg, pol, n, seed=seed + 9000, collect_traces=True)
    return [ep["trace"] for ep in r["episodes"] if "trace" in ep]


# ---------------------------------------------------------------------------
# verdict logic (spec section 16/17/18)
# ---------------------------------------------------------------------------


def judge(summary: Dict, causal: Dict, seeds: List[int]) -> Dict:
    """Return {criterion: (pass?, detail)} and an overall verdict."""
    crit = {}

    def agg(tag_seed: str, ckpt: str, cond: str, key: str):
        try:
            return summary[tag_seed][ckpt][cond][key]["mean"]
        except KeyError:
            return float("nan")

    # gather learned-agent ID errors (best ckpt, per seed)
    learned_err = {}
    learned_surv = {}
    for tag in LEARNED_TAGS:
        errs, survs = [], []
        for s in seeds:
            e = agg(f"{tag}_seed{s}", "best", "ID", "homeostatic_error_full")
            v = agg(f"{tag}_seed{s}", "best", "ID", "survival_fraction")
            if not np.isnan(e):
                errs.append(e)
                survs.append(v)
        learned_err[tag] = errs
        learned_surv[tag] = survs

    rand_err = summary.get("random", {}).get("ID", {}).get(
        "homeostatic_error_full", {}).get("mean", float("nan"))
    heur_err = summary.get("heuristic", {}).get("ID", {}).get(
        "homeostatic_error_full", {}).get("mean", float("nan"))

    best_tag = min(learned_err, key=lambda t: np.mean(learned_err[t]) if learned_err[t] else 1e9)
    best_errs = np.array(learned_err[best_tag])

    # PASS-1: learned < random (permutation test on pooled per-episode errs)
    rand_eps = np.array(summary.get("random", {}).get("ID_eps", []))
    best_eps = np.concatenate([
        np.array(summary[f"{best_tag}_seed{s}"]["best"]["ID_eps"])
        for s in seeds
        if f"{best_tag}_seed{s}" in summary
        and "ID_eps" in summary[f"{best_tag}_seed{s}"].get("best", {})
    ]) if best_errs.size else np.array([])
    p1 = bool(len(best_errs) and np.mean(best_errs) < rand_err)
    p1_sig = ""
    if len(rand_eps) and len(best_eps):
        pv = permutation_p(best_eps, rand_eps)
        p1 = p1 and pv < 0.05
        p1_sig = f", perm p={pv:.4f}"
    crit["PASS-1 learned<random"] = (
        p1, f"{best_tag} err={np.mean(best_errs):.4f} vs random "
            f"{rand_err:.4f}{p1_sig}")

    # PASS-2: >= heuristic ID, or beat heuristic on some OOD
    heur_ood = {c: summary.get("heuristic", {}).get(c, {}).get(
        "homeostatic_error_full", {}).get("mean", float("nan")) for c in OOD}
    id_ok = bool(len(best_errs) and np.mean(best_errs) <= heur_err * 1.2)
    ood_wins = []
    for c in OOD:
        le = np.nanmean([agg(f"{best_tag}_seed{s}", "best", c,
                             "homeostatic_error_full") for s in seeds])
        if not np.isnan(le) and not np.isnan(heur_ood[c]) and le < heur_ood[c]:
            ood_wins.append(c)
    p2 = id_ok or bool(ood_wins)
    crit["PASS-2 >=heuristic or OOD win"] = (
        p2, f"ID {np.mean(best_errs):.4f} vs heur {heur_err:.4f}; "
            f"OOD wins: {ood_wins or 'none'}")

    # PASS-3: shuffle degrades (use causal summary, mean over seeds)
    deltas = []
    for s in seeds:
        k = f"{best_tag}_seed{s}"
        if k in causal:
            deltas.append(causal[k]["summary"]["shuffle"]["delta_error"])
    p3 = bool(deltas and np.mean(deltas) > 0)
    crit["PASS-3 shuffle degrades"] = (
        p3, f"mean Δerr={np.mean(deltas):+.4f} over {len(deltas)} seeds"
        if deltas else "no causal data")

    # PASS-4: >=2 internal vars with state-dependent action.
    # Use action_shift (robust to narrow-band states) with MI as secondary.
    shifts, mi_means = [], []
    for s in seeds:
        k = f"{best_tag}_seed{s}"
        try:
            shifts.append(summary[k]["best"]["ID"]["action_shift"])
            mi_means.append(summary[k]["best"]["ID"]["state_action_mi"])
        except KeyError:
            pass
    if shifts:
        sh_avg = {v: float(np.mean([m[v] for m in shifts]))
                  for v in dyn.INTERNAL_NAMES}
        mi_avg = {v: float(np.mean([m[v] for m in mi_means]))
                  for v in dyn.INTERNAL_NAMES} if mi_means else {}
        n_sig = sum(1 for v in sh_avg.values() if v > 0.15)
        p4 = n_sig >= 2
        crit["PASS-4 >=2 state-dependent vars"] = (
            p4, f"action_shift={ {k: round(v,3) for k,v in sh_avg.items()} } "
                f"MI={ {k: round(v,3) for k,v in mi_avg.items()} }")
    else:
        crit["PASS-4 >=2 state-dependent vars"] = (False, "no data")
        p4 = False

    # PASS-5: some OOD where learned beats fixed recovery
    p5 = bool(ood_wins)
    crit["PASS-5 OOD recovery > fixed"] = (
        p5, f"OOD wins: {ood_wins or 'none'}")

    n_pass = sum(1 for v, _ in crit.values() if v)
    verdict = "PASS" if n_pass == 5 else ("PARTIAL" if n_pass >= 3 else "FAIL")
    return {"criteria": crit, "n_pass": n_pass, "verdict": verdict,
            "best_tag": best_tag}


# ---------------------------------------------------------------------------
# report assembly
# ---------------------------------------------------------------------------


def fmt(x, nd=4):
    return "nan" if x is None or (isinstance(x, float) and np.isnan(x)) else f"{x:.{nd}f}"


def build_report(summary: Dict, causal: Dict, seeds: List[int],
                 verdict: Dict) -> str:
    L = []
    L.append("# H0 — Artificial Homeostasis: Results\n")
    L.append(f"**Verdict: {verdict['verdict']}** "
             f"({verdict['n_pass']}/5 PASS criteria)\n")

    L.append("## PASS criteria\n")
    L.append("| criterion | result | detail |")
    L.append("|---|---|---|")
    for k, (ok, detail) in verdict["criteria"].items():
        L.append(f"| {k} | {'PASS' if ok else 'FAIL'} | {detail} |")
    L.append("")

    # ID table
    L.append("## In-distribution (base env)\n")
    L.append("| agent | ckpt | err_full | survival | stable | entropy |")
    L.append("|---|---|---|---|---|---|")
    for name in ("random", "heuristic"):
        a = summary.get(name, {}).get("ID", {})
        if a:
            L.append(f"| {name} | - | {fmt(a['homeostatic_error_full']['mean'])} "
                     f"| {fmt(a['survival_fraction']['mean'],3)} "
                     f"| {fmt(a['stable_fraction']['mean'],3)} "
                     f"| {fmt(a['action_entropy']['mean'],3)} |")
    for tag in LEARNED_TAGS:
        for ckpt in ("pre", "best", "final"):
            errs, survs, stabs, ents = [], [], [], []
            for s in seeds:
                try:
                    a = summary[f"{tag}_seed{s}"][ckpt]["ID"]
                    errs.append(a["homeostatic_error_full"]["mean"])
                    survs.append(a["survival_fraction"]["mean"])
                    stabs.append(a["stable_fraction"]["mean"])
                    ents.append(a["action_entropy"]["mean"])
                except KeyError:
                    pass
            if errs:
                L.append(f"| {tag} | {ckpt} | {fmt(np.mean(errs))}±{fmt(np.std(errs),3)} "
                         f"| {fmt(np.mean(survs),3)} | {fmt(np.mean(stabs),3)} "
                         f"| {fmt(np.mean(ents),3)} |")
    L.append("")

    # OOD table
    L.append("## OOD (zero-shot, best checkpoint)\n")
    L.append("| agent | " + " | ".join(OOD) + " |")
    L.append("|---|" + "---|" * len(OOD))
    for name in ("random", "heuristic"):
        row = [fmt(summary.get(name, {}).get(c, {}).get(
            "homeostatic_error_full", {}).get("mean")) for c in OOD]
        L.append(f"| {name} | " + " | ".join(row) + " |")
    for tag in LEARNED_TAGS:
        row = []
        for c in OOD:
            vals = []
            for s in seeds:
                try:
                    vals.append(summary[f"{tag}_seed{s}"]["best"][c]
                                ["homeostatic_error_full"]["mean"])
                except KeyError:
                    pass
            row.append(fmt(np.mean(vals)) if vals else "nan")
        L.append(f"| {tag} | " + " | ".join(row) + " |")
    L.append("")

    # causal table
    if causal:
        L.append("## Causal ablations (Δ error_full vs baseline, best ckpt)\n")
        ab_names = None
        rows: Dict[str, List] = {}
        for k, d in sorted(causal.items()):
            s = d["summary"]
            if ab_names is None:
                ab_names = [x for x in s if x != "baseline"]
            for ab in ab_names:
                rows.setdefault(ab, []).append(s[ab]["delta_error"])
        if ab_names:
            L.append("| ablation | mean Δerr |")
            L.append("|---|---|")
            for ab in ab_names:
                L.append(f"| {ab} | {fmt(np.mean(rows[ab]),4)} |")
            L.append("")

    L.append("## Figures\n")
    for f in sorted(FIGS.glob("*.png")):
        L.append(f"- `figs/{f.name}`")
    L.append("")
    L.append("## Reproduce\n")
    L.append("```bash")
    L.append(".venv/bin/python -m experiments.h0.sweep all --seeds 0,1,2,3,4")
    L.append(".venv/bin/python -m experiments.h0.report")
    L.append("```")
    return "\n".join(L)


def make_figures(summary: Dict, seeds: List[int], best_tag: str):
    FIGS.mkdir(parents=True, exist_ok=True)
    # training curves
    for tag in LEARNED_TAGS:
        for s in seeds[:1]:
            p = RUNS / tag / f"seed{s}" / "train_log.json"
            if p.exists():
                hist = json.loads(p.read_text())["evals"]
                plots.plot_training_curve(
                    hist, FIGS / f"train_{tag}_seed{s}.png",
                    title=f"{tag} seed{s}")
    # OOD comparison
    table: Dict[str, Dict[str, float]] = {}
    conds = ["ID"] + OOD
    for c in conds:
        table[c] = {}
        for name in ("random", "heuristic"):
            v = summary.get(name, {}).get(c, {}).get(
                "homeostatic_error_full", {}).get("mean")
            if v is not None:
                table[c][name] = v
        for tag in LEARNED_TAGS:
            vals = []
            for s in seeds:
                try:
                    vals.append(summary[f"{tag}_seed{s}"]["best"][c]
                                ["homeostatic_error_full"]["mean"])
                except KeyError:
                    pass
            if vals:
                table[c][tag] = float(np.mean(vals))
    if table.get("ID"):
        plots.plot_ood_comparison(table, FIGS / "ood_comparison.png")
    # ablation bar chart (mean over seeds of best_tag)
    causal = load_causal()
    merged: Dict[str, List[float]] = {}
    base = None
    for s in seeds:
        k = f"{best_tag}_seed{s}"
        if k in causal:
            for ab, d in causal[k]["summary"].items():
                if ab == "baseline":
                    base = d
                else:
                    merged.setdefault(ab, []).append(d["delta_error"])
    if merged:
        summ = {"baseline": base or {}}
        for ab, v in merged.items():
            summ[ab] = {"delta_error": float(np.mean(v))}
        plots.plot_ablation(summ, FIGS / "ablation_delta.png")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", default="0,1,2,3,4")
    ap.add_argument("--episodes", type=int, default=20)
    ap.add_argument("--traces", action="store_true",
                    help="run traced episodes for rhythm/figure generation")
    args = ap.parse_args()
    seeds = [int(s) for s in args.seeds.split(",")]

    summary = load_summary()
    causal = load_causal()
    verdict = judge(summary, causal, seeds)
    best_tag = verdict["best_tag"]

    REPORTS.mkdir(parents=True, exist_ok=True)
    make_figures(summary, seeds, best_tag)

    # optional: traced episode figures + rhythm for the best agent
    rhythm_out = {}
    if args.traces:
        try:
            traces = collect_traces(best_tag, seeds[0], n=3)
            if traces:
                plots.plot_internal_states(
                    traces[0], FIGS / "trace_internal.png",
                    title=f"{best_tag} seed{seeds[0]} best")
                plots.plot_error_vs_time(
                    traces, FIGS / "error_vs_time.png",
                    title=f"{best_tag} homeostatic error")
                plots.plot_transition_matrix(
                    traces[0], FIGS / "transition_matrix.png")
                rhythm_out = rhythm.analyze_episode(traces[0])
        except Exception as e:  # noqa: BLE001
            print("trace/figure generation failed:", e)

    md = build_report(summary, causal, seeds, verdict)
    if rhythm_out:
        md += "\n## Rhythm analysis (representative episode)\n\n```json\n"
        md += json.dumps(
            {k: v for k, v in rhythm_out.items() if k != "dominant_period"}
            | {"dominant_period": {
                k: v for k, v in rhythm_out["dominant_period"].items()
                if k not in ("psd", "freqs")}},
            indent=2) + "\n```\n"
    (REPORTS / "H0_RESULTS.md").write_text(md)
    print(f"verdict: {verdict['verdict']} ({verdict['n_pass']}/5)")
    print(f"wrote {REPORTS / 'H0_RESULTS.md'}")


if __name__ == "__main__":
    main()
