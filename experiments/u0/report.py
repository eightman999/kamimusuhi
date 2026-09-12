"""U0 report generator: consolidates artifacts/results into
reports/U0_RESULTS.md with pre-registered gate verdicts and figures.

    python -m experiments.u0.report --artifacts experiments/u0/artifacts

Gates (fixed before seeing results — see README):
    U-H1  learned error_full >= 20% below min(random, fifo)        [primary]
    U-H2  store_precision >= .75 AND important_retention >= .75
    U-H3  erase OR shuffle degrades error_full >= 30%
    U-H4  need intervention mean |dP(STORE)| >= 0.20
    U-H5  main conclusions sign-consistent over >= 3 seeds
    Strong PASS = all of U-H1..U-H5 AND delay128 within 20% degradation
                  AND distractor4x precision >= .70 AND oracle_gap <= .15
"""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

import numpy as np

from .analysis.metrics import GATE_THRESHOLDS
from .analysis.plots import (plot_causal_ablation, plot_homeostatic_error,
                             plot_learning_curve, plot_memory_retention,
                             plot_need_intervention, plot_ood_delay,
                             plot_store_precision)

BASELINES = ["random", "fifo", "lru", "store_all",
             "heuristic_current_need", "oracle"]
CAUSAL_MODES = ["none", "erase", "shuffle"]
OOD_MODES = ["delay96", "delay128", "delay160",
             "distractor2x", "distractor4x", "need_mapping_shift",
             "event_perm"]


def _git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"],
            stderr=subprocess.DEVNULL).decode().strip()
    except Exception:
        return "unknown"


def collect(art: Path) -> dict:
    """subject -> condition -> metrics dict."""
    res_dir = art / "results"
    table: dict[str, dict] = {}
    probes: dict[str, dict] = {}
    missing: list[str] = []
    if not res_dir.exists():
        return {"table": {}, "probes": {}, "missing": ["no results dir"]}
    for p in sorted(res_dir.glob("*_c-*_o-*.json")):
        r = json.loads(p.read_text())
        table.setdefault(r["subject"], {})[
            f"c-{r['causal']}_o-{r['ood']}"] = r["metrics"]
    for p in sorted(res_dir.glob("*_probe-need_intervention.json")):
        probes[p.name.split("_probe-")[0]] = json.loads(p.read_text())
    return {"table": table, "probes": probes, "missing": missing}


def run_statuses(art: Path, models: list[str], seeds: list[int]) -> dict:
    out = {}
    for m in models:
        for s in seeds:
            rid = f"{m}_s{s}"
            d = art / "runs" / rid
            if (d / "done.json").exists():
                done = json.loads((d / "done.json").read_text())
                out[rid] = {"status": "completed", **done}
            elif (d / "last.pt").exists():
                out[rid] = {"status": "partial"}
            else:
                out[rid] = {"status": "missing"}
    return out


def load_run_metrics(art: Path) -> dict:
    runs = {}
    runs_dir = art / "runs"
    if runs_dir.exists():
        for d in sorted(runs_dir.iterdir()):
            mj = d / "metrics.jsonl"
            if mj.exists():
                runs[d.name] = [json.loads(l) for l in
                                mj.read_text().splitlines() if l.strip()]
    return runs


# ---------------------------------------------------------------------------
# Gates
# ---------------------------------------------------------------------------


def evaluate_gates(table: dict, probes: dict, learned: list[str],
                   seeds: list[int]) -> dict:
    g = GATE_THRESHOLDS
    gates: dict[str, dict] = {}

    def clean(subj):
        return table.get(subj, {}).get("c-none_o-none", {})

    # per-seed learned error
    seed_err = {}
    for s in seeds:
        errs = [clean(f"{m}_s{s}").get("error_full")
                for m in ("mlp", "gru64", "gru128") if f"{m}_s{s}" in table]
        errs = [e for e in errs if e is not None]
        if errs:
            seed_err[s] = min(errs)          # best model per seed
    best_learned = min(seed_err.values()) if seed_err else float("nan")
    base_err = min(clean("baseline_random").get("error_full", np.inf),
                   clean("baseline_fifo").get("error_full", np.inf))
    improvement = (1.0 - best_learned / base_err) if base_err < np.inf \
        else float("nan")
    per_seed_imp = []
    for s, e in seed_err.items():
        per_seed_imp.append(1.0 - e / base_err if base_err < np.inf
                            else float("nan"))
    gates["U-H1"] = {
        "desc": "learned error_full >= 20% below min(random, fifo)",
        "value": improvement, "threshold": g["U-H1_improvement"],
        "per_seed": per_seed_imp,
        "pass": bool(np.isfinite(improvement)
                     and improvement >= g["U-H1_improvement"])}

    # U-H2: store quality for the best learned subject
    best_subj = None
    if seed_err:
        best_s = min(seed_err, key=seed_err.get)
        cands = [clean(f"{m}_s{best_s}")
                 for m in ("mlp", "gru64", "gru128")]
        cands = [(f"{m}_s{best_s}", c) for m, c in
                 zip(("mlp", "gru64", "gru128"), cands) if c]
        if cands:
            best_subj = min(cands, key=lambda kv: kv[1]["error_full"])[0]
    bm = clean(best_subj) if best_subj else {}
    prec = bm.get("store_precision", float("nan"))
    ret = bm.get("important_retention", float("nan"))
    gates["U-H2"] = {
        "desc": "store_precision >= .75 AND important_retention >= .75",
        "value": {"store_precision": prec, "important_retention": ret},
        "threshold": [g["U-H2_store_precision"],
                      g["U-H2_important_retention"]],
        "subject": best_subj,
        "pass": bool(prec >= g["U-H2_store_precision"]
                     and ret >= g["U-H2_important_retention"])}

    # U-H3: causal degradation for the best learned subject
    degrad = {}
    for mode in ("erase", "shuffle"):
        c = table.get(best_subj, {}).get(f"c-{mode}_o-none", {}) \
            if best_subj else {}
        base = bm.get("error_full")
        attacked = c.get("error_full")
        if base and attacked:
            degrad[mode] = attacked / base - 1.0
    best_deg = max(degrad.values()) if degrad else float("nan")
    gates["U-H3"] = {
        "desc": "erase OR shuffle degrades error_full >= 30%",
        "value": degrad, "threshold": g["U-H3_delta_err"],
        "pass": bool(np.isfinite(best_deg)
                     and best_deg >= g["U-H3_delta_err"])}

    # U-H4: need intervention on the best learned subject
    probe = probes.get(best_subj or "", {})
    delta = probe.get("mean_abs_delta", float("nan"))
    gates["U-H4"] = {
        "desc": "need intervention mean |dP(STORE)| >= 0.20",
        "value": delta, "threshold": g["U-H4_mean_abs_delta"],
        "detail": probe.get("delta_by_func"),
        "pass": bool(np.isfinite(delta)
                     and delta >= g["U-H4_mean_abs_delta"])}

    # U-H5: sign consistency across seeds for H1 & H4 directions
    h4_ok = all(probes.get(f"{m}_s{s}", {}).get("mean_abs_delta", 0) > 0
                for m in ("mlp", "gru64", "gru128") for s in seeds
                if f"{m}_s{s}" in probes)
    h1_signs = [i > 0 for i in per_seed_imp if np.isfinite(i)]
    gates["U-H5"] = {
        "desc": "main conclusions sign-consistent over >= 3 seeds",
        "value": {"seeds_with_improvement": sum(h1_signs),
                  "n_seeds": len(seed_err), "h4_positive": h4_ok},
        "threshold": g["U-H5_min_seeds"],
        "pass": bool(len(seed_err) >= g["U-H5_min_seeds"]
                     and sum(h1_signs) >= g["U-H5_min_seeds"] and h4_ok)}

    # Strong PASS extensions
    ext = {}
    if best_subj:
        c = clean(best_subj)
        d128 = table.get(best_subj, {}).get("c-none_o-delay128", {})
        ext["delay128_degradation"] = (
            d128.get("error_full", np.inf)
            / max(c.get("error_full", np.nan), 1e-9) - 1.0)
        dx = table.get(best_subj, {}).get("c-none_o-distractor4x", {})
        ext["distractor4x_precision"] = dx.get("store_precision",
                                              float("nan"))
        oc = clean("baseline_oracle").get("error_full")
        ext["oracle_gap"] = (c.get("error_full", np.inf) - oc
                             if oc is not None else float("nan"))
    strong = all(gg["pass"] for gg in gates.values()) and \
        ext.get("delay128_degradation", np.inf) <= \
        g["strong_delay128_degradation"] and \
        ext.get("distractor4x_precision", 0) >= \
        g["strong_distractor4x_precision"] and \
        ext.get("oracle_gap", np.inf) <= g["strong_oracle_gap"]
    gates["Strong"] = {"desc": "U-H1..U-H5 + delay128 <=20% degradation "
                               "+ distractor4x precision >= .70 "
                               "+ oracle_gap <= .15",
                       "value": ext, "pass": bool(strong)}
    return gates


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def make_figures(art: Path, table: dict, probes: dict,
                 learned: list[str]) -> None:
    fig = Path("experiments/u0/reports/figs")

    def cond_of(subj, key):
        return table.get(subj, {}).get(key, {})

    subs = learned + [f"baseline_{b}" for b in BASELINES]
    err_rows = [{"subject": s,
                 "value": cond_of(s, "c-none_o-none").get("error_full")}
                for s in subs]
    prec_rows = [{"subject": s,
                  "value": cond_of(s, "c-none_o-none").get("store_precision")}
                 for s in subs]
    ret_rows = [{"subject": s,
                 "value": cond_of(s, "c-none_o-none").get("important_retention")}
                for s in subs]
    plot_homeostatic_error(err_rows, fig / "homeostatic_error.png")
    plot_store_precision(prec_rows, fig / "store_precision.png")
    plot_memory_retention(ret_rows, fig / "memory_retention.png")
    plot_learning_curve(load_run_metrics(art), fig / "learning_curve.png")

    abl_rows = []
    for s in subs:
        r = {"subject": s}
        for m, key in (("clean", "c-none_o-none"),
                       ("erase", "c-erase_o-none"),
                       ("shuffle", "c-shuffle_o-none")):
            r[m] = cond_of(s, key).get("error_full", float("nan"))
        abl_rows.append(r)
    plot_causal_ablation(abl_rows, fig / "causal_ablation.png")

    ood_rows = []
    for s in learned:
        r = {"subject": s}
        r["clean"] = cond_of(s, "c-none_o-none").get("error_full")
        for m in ("delay96", "delay128", "delay160"):
            r[m] = cond_of(s, f"c-none_o-{m}").get("error_full")
        ood_rows.append(r)
    if ood_rows:
        plot_ood_delay(ood_rows, fig / "ood_delay.png")

    best_probe_subj = max(
        probes, key=lambda s: probes[s].get("mean_abs_delta", -1),
        default=None)
    if best_probe_subj:
        plot_need_intervention(probes[best_probe_subj],
                               fig / "need_intervention.png")


def write_report(art: Path, models: list[str], seeds: list[int]) -> Path:
    data = collect(art)
    table, probes = data["table"], data["probes"]
    statuses = run_statuses(art, models, seeds)
    learned = [s for s in table if not s.startswith("baseline_")]
    gates = evaluate_gates(table, probes, learned, seeds)
    make_figures(art, table, probes, learned)

    L: list[str] = []
    L.append("# U0 — Need-Guided Memory: Results\n")
    L.append("## Research question\n")
    L.append("In a finite-memory environment, does an agent learn "
             "need-conditioned memory gating — does the *same* event "
             "become worth storing or not depending on its own internal "
             "state, because stored events have later homeostatic "
             "consequences?\n")
    L.append("## Protocol\n")
    L.append("- Models: mlp, gru64, gru128; seeds: "
             + ", ".join(map(str, seeds)) + "\n"
             "- Eval corpus: seed 900001+, held out from training (val seed "
             "700001) and training seeds\n"
             "- Causal battery: erase (U-C1), shuffle (U-C2), need "
             "intervention (U-C3), event_perm (U-C4)\n"
             "- OOD: delay96/128/160, distractor2x/4x, need_mapping_shift\n"
             "- Checkpoint selection: lowest `error_full` on the validation "
             "corpus\n")
    L.append("## Environment\n")
    L.append("6-location ring, 8 sites = 4 functions x (potent, meager); "
             "4-slot memory; needs sampled ~ reset-time deviation; "
             "crisis resolved via recall -> move -> act. See "
             "`experiments/u0/README.md` for the full spec.\n")
    L.append("## Runs\n")
    L.append("| run | status | iters | best val error_full | wall (s) |\n")
    L.append("|---|---|---|---|---|\n")
    for rid, stt in sorted(statuses.items()):
        L.append(f"| {rid} | {stt['status']} | "
                 f"{stt.get('iters', '-')} | "
                 f"{stt.get('best_val_error_full', float('nan')):.4f} | "
                 f"{stt.get('elapsed_sec', '-')} |\n"
                 if stt["status"] == "completed" else
                 f"| {rid} | {stt['status']} | - | - | - |\n")
    L.append("\n## Main results (clean)\n")
    L.append("| subject | error_full | survival | stable | resolution |"
             " retention | precision |\n")
    L.append("|---|---|---|---|---|---|---|\n")
    for s in learned + [f"baseline_{b}" for b in BASELINES]:
        m = table.get(s, {}).get("c-none_o-none")
        if not m:
            continue
        L.append(f"| {s} | {m.get('error_full', float('nan')):.4f} | "
                 f"{m.get('survival_fraction', float('nan')):.3f} | "
                 f"{m.get('stable_fraction', float('nan')):.3f} | "
                 f"{m.get('need_resolution', float('nan')):.3f} | "
                 f"{m.get('important_retention', float('nan')):.3f} | "
                 f"{m.get('store_precision', float('nan')):.3f} |\n")
    L.append("\n## Causal tests\n")
    for s in learned:
        row = [s]
        for c in CAUSAL_MODES:
            m = table.get(s, {}).get(f"c-{c}_o-none", {})
            row.append(m.get("error_full", float("nan")))
        pr = probes.get(s, {})
        L.append(f"- `{s}`: clean={row[1]:.4f} erase={row[2]:.4f} "
                 f"shuffle={row[3]:.4f} | need-intervention "
                 f"mean|ΔP|={pr.get('mean_abs_delta', float('nan')):.3f}\n")
    L.append("\n## OOD\n")
    for s in learned:
        parts = []
        for m in OOD_MODES:
            mm = table.get(s, {}).get(f"c-none_o-{m}", {})
            parts.append(f"{m}={mm.get('error_full', float('nan')):.4f}")
        L.append(f"- `{s}`: " + ", ".join(parts) + "\n")
    L.append("\n## Per-hypothesis verdict\n")
    L.append("| gate | criterion | value | verdict |\n|---|---|---|---|\n")
    for name, gg in gates.items():
        val = gg["value"]
        val_s = f"{val:.3f}" if isinstance(val, float) else str(val)
        L.append(f"| {name} | {gg['desc']} | {val_s} | "
                 f"{'PASS' if gg['pass'] else 'FAIL'} |\n")
    overall = ("**STRONG PASS**" if gates["Strong"]["pass"] else
               "**PASS**" if all(gates[n]["pass"] for n in
                                 ("U-H1", "U-H2", "U-H3", "U-H4"))
               else "**FAIL/PARTIAL**")
    L.append(f"\n## Overall verdict\n\n{overall}\n")
    L.append("\n## Limitations\n")
    L.append("- Single-agent, single-room abstraction: the result bounds "
             "need-conditioned gating in a finite slot memory, not "
             "biological memory.\n"
             "- Need targets are sampled from the reset-time deviation "
             "profile; the learnable signal is the correlation between "
             "observed internal state and subsequent crisis identity.\n"
             "- The recall readout is a derived feature (the experiment "
             "tests gating, not readout mechanics).\n")
    L.append("\n## Reproduce\n")
    L.append("```bash\n"
             "caffeinate -ims python -m experiments.u0.sweep \\\n"
             "  --config experiments/u0/configs/default.yaml \\\n"
             "  --models mlp gru64 gru128 --seeds 0 1 2 --device cpu\n"
             "python -m experiments.u0.report\n"
             "```\n")
    L.append(f"\n_Generated from `{art}` at commit `{_git_commit()}`._\n")

    out = Path("experiments/u0/reports/U0_RESULTS.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(L))
    print(f"wrote {out}")
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--models", nargs="+",
                    default=["mlp", "gru64", "gru128"])
    ap.add_argument("--seeds", nargs="+", type=int, default=[0, 1, 2])
    args = ap.parse_args()
    write_report(Path(args.artifacts), args.models, args.seeds)


if __name__ == "__main__":
    main()
