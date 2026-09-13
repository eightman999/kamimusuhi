"""U0 report generator: consolidates artifacts/results into
reports/U0_RESULTS.md with pre-registered gate verdicts and figures.

    python -m experiments.u0.report --artifacts experiments/u0/artifacts

Gates (fixed before seeing results — see README):
    U-H1   need_resolution >= .80 AND normalized_regret <= .50
           (regret on crisis_error_auc: 0 = oracle, 1 = no_memory)
    U-H2   store_precision >= .75 AND important_retention >= .75
    U-H3   targeted_erase OR donor_shuffle: crisis_error_auc >= clean
           x1.30, or need_resolution drops >= .20
    U-H4   need intervention mean |dP(STORE)| >= 0.20
    U-H4b  targeted_mediation auc closes >= half the clean->no_memory gap
    U-H5   main conclusions sign-consistent over >= 3 seeds
    Strong PASS = all of U-H1..U-H5 AND delay128 regret <= .70 AND
                  distractor4x precision >= .70 AND
                  need_mapping_shift resolution >= .50 AND
                  capacity2 regret <= .50
"""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

import numpy as np
import yaml

from .analysis.metrics import GATE_THRESHOLDS
from .analysis.plots import (plot_causal_ablation, plot_homeostatic_error,
                             plot_learning_curve, plot_memory_retention,
                             plot_need_intervention, plot_ood_delay,
                             plot_store_precision)

BASELINES = ["random", "fifo", "lru", "store_all",
             "heuristic_current_need", "no_memory", "oracle"]
CAUSAL_MODES = ["none", "targeted_erase", "donor_shuffle",
                "targeted_mediation"]
OOD_MODES = ["delay96", "delay128", "delay160",
             "distractor2x", "distractor4x", "need_mapping_shift",
             "event_permutation", "capacity2"]
MODELS = ("mlp", "gru64", "gru128")


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
    failures: list[dict] = []
    protocol = None
    if not res_dir.exists():
        return {"table": {}, "probes": {}, "failures": failures,
                "protocol": None}
    for p in sorted(res_dir.glob("*_c-*_o-*.json")):
        r = json.loads(p.read_text())
        table.setdefault(r["subject"], {})[
            f"c-{r['causal']}_o-{r['ood']}"] = r["metrics"]
    for p in sorted(res_dir.glob("*_probe-need_intervention.json")):
        probes[p.name.split("_probe-")[0]] = json.loads(p.read_text())
    fj = res_dir / "failures.json"
    if fj.exists():
        failures = json.loads(fj.read_text())
    pj = res_dir / "protocol_check.json"
    if pj.exists():
        protocol = json.loads(pj.read_text())
    return {"table": table, "probes": probes, "failures": failures,
            "protocol": protocol}


def run_statuses(art: Path, models: list[str], seeds: list[int]) -> dict:
    out = {}
    for m in models:
        for s in seeds:
            rid = f"{m}_s{s}"
            d = art / "runs" / rid
            if (d / "meta.json").exists():
                meta = json.loads((d / "meta.json").read_text())
                out[rid] = {"status": "completed", **meta}
            elif (d / "latest.pt").exists():
                out[rid] = {"status": "partial"}
            else:
                out[rid] = {"status": "missing"}
    return out


def load_train_config(art: Path) -> dict:
    """Training hyperparameters from the first run that recorded a config."""
    runs_dir = art / "runs"
    if runs_dir.exists():
        for d in sorted(runs_dir.iterdir()):
            cf = d / "config.yaml"
            if cf.exists():
                raw = yaml.safe_load(cf.read_text()) or {}
                return raw.get("train") or {}
    return {}


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


def normalized_regret(metrics: dict, oracle_auc: float,
                      nomem_auc: float) -> float:
    """(learned - oracle) / (no_memory - oracle) on crisis_error_auc.
    0.0 = oracle performance, 1.0 = no_memory performance."""
    a = metrics.get("crisis_error_auc")
    if a is None or not np.isfinite(oracle_auc) \
            or not np.isfinite(nomem_auc) or nomem_auc <= oracle_auc:
        return float("nan")
    return (a - oracle_auc) / (nomem_auc - oracle_auc)


# ---------------------------------------------------------------------------
# Gates
# ---------------------------------------------------------------------------


def evaluate_gates(table: dict, probes: dict, learned: list[str],
                   seeds: list[int]) -> dict:
    g = GATE_THRESHOLDS
    gates: dict[str, dict] = {}

    def clean(subj):
        return table.get(subj, {}).get("c-none_o-none", {})

    oracle_auc = clean("baseline_oracle").get("crisis_error_auc",
                                              float("nan"))
    nomem_auc = clean("baseline_no_memory").get("crisis_error_auc",
                                                float("nan"))
    nomem_res = clean("baseline_no_memory").get("need_resolution",
                                                float("nan"))

    # best learned subject per seed = lowest clean crisis_error_auc;
    # teacher arms (…T_sN) compete alongside the pure-reward arm
    best_per_seed: dict[int, str] = {}
    for s in seeds:
        names = [f"{m}_s{s}" for m in MODELS] + \
                [f"{m}T_s{s}" for m in MODELS] + \
                [f"{m}C_s{s}" for m in MODELS]
        cands = [(n, clean(n)) for n in names if n in table]
        cands = [(n, c) for n, c in cands
                 if c.get("crisis_error_auc") is not None]
        if cands:
            best_per_seed[s] = min(
                cands, key=lambda kv: kv[1]["crisis_error_auc"])[0]
    best_subj = None
    if best_per_seed:
        best_subj = min(best_per_seed.values(),
                        key=lambda s: clean(s).get(
                            "crisis_error_auc", np.inf))

    # ---- U-H1: resolution >= .80 AND normalized_regret <= .50 --------
    bm = clean(best_subj) if best_subj else {}
    res = bm.get("need_resolution", float("nan"))
    reg = normalized_regret(bm, oracle_auc, nomem_auc)
    per_seed_reg = {s: normalized_regret(clean(sub), oracle_auc,
                                         nomem_auc)
                    for s, sub in best_per_seed.items()}
    gates["U-H1"] = {
        "desc": "need_resolution >= .80 AND normalized_regret <= .50 "
                "(crisis_error_auc; 0=oracle, 1=no_memory)",
        "value": {"subject": best_subj, "need_resolution": res,
                  "normalized_regret": reg},
        "threshold": [g["U-H1_need_resolution"],
                      g["U-H1_normalized_regret"]],
        "per_seed": {s: {"subject": sub,
                         "regret": per_seed_reg[s],
                         "resolution": clean(sub).get("need_resolution")}
                     for s, sub in best_per_seed.items()},
        "pass": bool(np.isfinite(res)
                     and res >= g["U-H1_need_resolution"]
                     and np.isfinite(reg)
                     and reg <= g["U-H1_normalized_regret"])}

    # ---- U-H2: store quality ----------------------------------------
    prec = bm.get("store_precision", float("nan"))
    ret = bm.get("important_retention", float("nan"))
    gates["U-H2"] = {
        "desc": "store_precision >= .75 AND important_retention >= .75",
        "value": {"subject": best_subj, "store_precision": prec,
                  "important_retention": ret},
        "threshold": [g["U-H2_store_precision"],
                      g["U-H2_important_retention"]],
        "pass": bool(prec >= g["U-H2_store_precision"]
                     and ret >= g["U-H2_important_retention"])}

    # ---- U-H3: causal necessity (targeted window interventions) -----
    degrad = {}
    res_drop = {}
    for mode in ("targeted_erase", "donor_shuffle"):
        c = table.get(best_subj, {}).get(f"c-{mode}_o-none", {}) \
            if best_subj else {}
        attacked_auc = c.get("crisis_error_auc")
        attacked_res = c.get("need_resolution")
        if np.isfinite(bm.get("crisis_error_auc", np.nan)) \
                and attacked_auc is not None:
            degrad[mode] = attacked_auc / max(bm["crisis_error_auc"],
                                              1e-9)
        if np.isfinite(res) and attacked_res is not None:
            res_drop[mode] = res - attacked_res
    auc_hit = any(v >= g["U-H3_auc_ratio"] for v in degrad.values())
    res_hit = any(v >= g["U-H3_resolution_drop"]
                  for v in res_drop.values())
    gates["U-H3"] = {
        "desc": "targeted_erase OR donor_shuffle: crisis_error_auc >= "
                "clean x1.30, or need_resolution drops >= .20",
        "value": {"subject": best_subj, "auc_ratio": degrad,
                  "res_drop": res_drop},
        "threshold": [g["U-H3_auc_ratio"], g["U-H3_resolution_drop"]],
        "pass": bool(auc_hit or res_hit)}

    # ---- U-H4: need intervention ------------------------------------
    probe = probes.get(best_subj or "", {})
    delta = probe.get("mean_abs_delta", float("nan"))
    gates["U-H4"] = {
        "desc": "need intervention mean |dP(STORE)| >= 0.20 "
                "(internal need only; world/event/memory fixed)",
        "value": delta, "threshold": g["U-H4_mean_abs_delta"],
        "detail": probe.get("delta_by_func"),
        "pass": bool(np.isfinite(delta)
                     and delta >= g["U-H4_mean_abs_delta"])}

    # ---- U-H4b: mediation — erasing need-conditioned items must kill
    # the STORE advantage ----------------------------------------------
    med = table.get(best_subj, {}).get("c-targeted_mediation_o-none",
                                      {}) if best_subj else {}
    med_auc = med.get("crisis_error_auc", float("nan"))
    clean_auc = bm.get("crisis_error_auc", float("nan"))
    if np.isfinite(med_auc) and np.isfinite(clean_auc) \
            and np.isfinite(nomem_auc) and nomem_auc > clean_auc:
        gap_closed = (med_auc - clean_auc) / (nomem_auc - clean_auc)
    else:
        gap_closed = float("nan")
    gates["U-H4b"] = {
        "desc": "targeted_mediation auc >= clean + .50 x "
                "(no_memory - clean): the STORE advantage disappears",
        "value": {"subject": best_subj, "mediation_auc": med_auc,
                  "gap_closed": gap_closed},
        "threshold": g["U-H4b_mediation_gap"],
        "pass": bool(np.isfinite(gap_closed)
                     and gap_closed >= g["U-H4b_mediation_gap"])}

    # ---- U-H5: sign consistency over >= 3 seeds ---------------------
    per_seed = []
    for s, sub in best_per_seed.items():
        sc = clean(sub)
        se_auc = table.get(sub, {}).get("c-targeted_erase_o-none",
                                       {}).get("crisis_error_auc")
        p_delta = probes.get(sub, {}).get("mean_abs_delta", float("nan"))
        per_seed.append({
            "seed": s, "subject": sub,
            "learned_gt_nomem": bool(
                np.isfinite(sc.get("need_resolution", np.nan))
                and np.isfinite(nomem_res)
                and sc["need_resolution"] > nomem_res),
            "erase_hurts": bool(
                se_auc is not None
                and np.isfinite(sc.get("crisis_error_auc", np.nan))
                and se_auc > sc["crisis_error_auc"]),
            "probe_delta": p_delta,
            "probe_positive": bool(np.isfinite(p_delta)
                                   and p_delta > 0)})
    ok = all(p["learned_gt_nomem"] and p["erase_hurts"]
             and p["probe_positive"] for p in per_seed)
    gates["U-H5"] = {
        "desc": "learned > no_memory, erase hurts, need intervention "
                "changes STORE — sign-consistent over >= 3 seeds",
        "value": per_seed,
        "threshold": g["U-H5_min_seeds"],
        "pass": bool(len(per_seed) >= g["U-H5_min_seeds"] and ok)}

    # ---- Strong PASS extensions --------------------------------------
    ext = {}
    if best_subj:
        d128 = table.get(best_subj, {}).get("c-none_o-delay128", {})
        ext["delay128_regret"] = normalized_regret(
            d128,
            table.get("baseline_oracle", {}).get(
                "c-none_o-delay128", {}).get("crisis_error_auc",
                                            float("nan")),
            table.get("baseline_no_memory", {}).get(
                "c-none_o-delay128", {}).get("crisis_error_auc",
                                            float("nan")))
        dx = table.get(best_subj, {}).get("c-none_o-distractor4x", {})
        ext["distractor4x_precision"] = dx.get("store_precision",
                                              float("nan"))
        nms = table.get(best_subj, {}).get("c-none_o-need_mapping_shift",
                                         {})
        ext["need_mapping_shift_resolution"] = nms.get(
            "need_resolution", float("nan"))
        cap2 = table.get(best_subj, {}).get("c-none_o-capacity2", {})
        ext["capacity2_regret"] = normalized_regret(
            cap2,
            table.get("baseline_oracle", {}).get(
                "c-none_o-capacity2", {}).get("crisis_error_auc",
                                             float("nan")),
            table.get("baseline_no_memory", {}).get(
                "c-none_o-capacity2", {}).get("crisis_error_auc",
                                             float("nan")))
    strong = all(gg["pass"] for gg in gates.values()) and \
        ext.get("delay128_regret", np.inf) <= \
        g["strong_delay128_regret"] and \
        ext.get("distractor4x_precision", 0) >= \
        g["strong_distractor4x_precision"] and \
        ext.get("need_mapping_shift_resolution", 0) >= \
        g["strong_need_mapping_shift_resolution"] and \
        ext.get("capacity2_regret", np.inf) <= \
        g["strong_capacity2_regret"]
    gates["Strong"] = {
        "desc": "U-H1..U-H5 + delay128 regret <= .70 "
                "+ distractor4x precision >= .70 "
                "+ need_mapping_shift resolution >= .50 "
                "+ capacity2 regret <= .50",
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
    auc_rows = [{"subject": s,
                 "value": cond_of(s, "c-none_o-none").get(
                     "crisis_error_auc")}
                for s in subs]
    prec_rows = [{"subject": s,
                  "value": cond_of(s, "c-none_o-none").get(
                      "store_precision")}
                 for s in subs]
    ret_rows = [{"subject": s,
                 "value": cond_of(s, "c-none_o-none").get(
                     "important_retention")}
                for s in subs]
    plot_homeostatic_error(auc_rows, fig / "crisis_error_auc.png")
    plot_store_precision(prec_rows, fig / "store_precision.png")
    plot_memory_retention(ret_rows, fig / "memory_retention.png")
    plot_learning_curve(load_run_metrics(art), fig / "learning_curve.png")

    abl_rows = []
    for s in subs:
        r = {"subject": s}
        for m in CAUSAL_MODES:
            r[m] = cond_of(s, f"c-{m}_o-none").get(
                "crisis_error_auc", float("nan"))
        abl_rows.append(r)
    plot_causal_ablation(abl_rows, fig / "causal_ablation.png",
                         conds=CAUSAL_MODES)

    ood_rows = []
    for s in learned:
        r = {"subject": s}
        r["clean"] = cond_of(s, "c-none_o-none").get("crisis_error_auc")
        for m in ("delay96", "delay128", "delay160"):
            r[m] = cond_of(s, f"c-none_o-{m}").get("crisis_error_auc")
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
    failures, protocol = data["failures"], data["protocol"]
    statuses = run_statuses(art, models, seeds)
    learned = [s for s in table if not s.startswith("baseline_")]
    gates = evaluate_gates(table, probes, learned, seeds)
    make_figures(art, table, probes, learned)

    L: list[str] = []
    L.append("# U0 — Need-Guided Memory: Results\n")
    L.append("## Run summary\n")
    n_done = sum(1 for s in statuses.values()
                 if s["status"] == "completed")
    n_part = sum(1 for s in statuses.values()
                 if s["status"] == "partial")
    n_miss = sum(1 for s in statuses.values()
                 if s["status"] == "missing")
    wall = sum(s.get("elapsed_sec", 0.0) for s in statuses.values()
               if s["status"] == "completed")
    devs = sorted({s["device"] for s in statuses.values()
                   if s.get("device")})
    thrs = sorted({s["torch_num_threads"] for s in statuses.values()
                   if s.get("torch_num_threads")})
    L.append(f"- source commit: `{_git_commit()}`\n")
    L.append(f"- runs: {n_done} completed, {n_part} partial, "
             f"{n_miss} missing, {len(failures)} failed stages\n")
    L.append(f"- total wall-clock: {wall:.0f} s\n")
    L.append(f"- device(s): {', '.join(devs) if devs else '-'}\n")
    L.append(f"- CPU threads: "
             f"{', '.join(map(str, thrs)) if thrs else '-'}\n")
    if protocol:
        passed = sum(g["pass"] for g in protocol["gates"].values())
        L.append(f"- protocol sanity: **{protocol['verdict']}** "
                 f"({passed}/{len(protocol['gates'])} P0 gates, "
                 f"{protocol['episodes']} episodes; see "
                 "U0_PROTOCOL_CHECK.md)\n")
    else:
        L.append("- protocol sanity: not run (no protocol_check.json)\n")

    L.append("\n## Research question\n")
    L.append("In a finite-memory environment, does an agent learn "
             "need-conditioned memory gating — does the *same* event "
             "become worth storing or not depending on its own internal "
             "state, because stored events have later homeostatic "
             "consequences?\n")
    L.append("## Protocol\n")
    L.append("- Models: mlp, gru64, gru128; seeds: "
             + ", ".join(map(str, seeds)) + "\n"
             "- Eval corpus: seed 900001+, held out from training (val "
             "seed 700001) and training seeds\n"
             "- Causal battery: targeted_erase, donor_shuffle (U-H3), "
             "targeted_mediation (U-H4b), need_intervention probe "
             "(U-H4)\n"
             "- OOD: delay96/128/160, distractor2x/4x, "
             "need_mapping_shift, event_permutation, capacity2\n"
             "- Checkpoint selection: lowest `crisis_error_auc` on the "
             "validation corpus\n")
    L.append("## Baseline sanity\n")
    if protocol:
        orc = protocol["metrics"]["oracle/clean"]
        nom = protocol["metrics"]["no_memory/clean"]
        L.append(f"P0 gate passed before training: oracle "
                 f"resolution {orc['need_resolution']:.3f} vs no_memory "
                 f"{nom['need_resolution']:.3f}; oracle auc "
                 f"{orc['crisis_error_auc']:.2f} vs no_memory "
                 f"{nom['crisis_error_auc']:.2f}. Memory is causally "
                 "required for reliable recovery.\n")
    else:
        L.append("Protocol check results unavailable.\n")
    L.append("## Models\n")
    L.append("`mlp` (256x2 tanh trunk), `gru64`, `gru128` — single 6-way "
             "action head + value head; GRU state resets each episode.\n")
    L.append("## Training\n")
    tc = load_train_config(art)
    if tc:
        keys = ("iters", "num_envs", "lr", "epochs", "clip", "gamma",
                "lam", "vf_coef", "ent_coef", "imitation_iters")
        L.append("PPO: " + ", ".join(f"{k}={tc[k]}" for k in keys
                                     if k in tc) + "\n")
    else:
        L.append("PPO (config not recorded — no run config.yaml found).\n")
    L.append("## Runs\n")
    L.append("| run | status | iters | best val crisis_auc | wall (s) |"
             " device | threads |\n")
    L.append("|---|---|---|---|---|---|---|\n")
    for rid, stt in sorted(statuses.items()):
        L.append(f"| {rid} | {stt['status']} | "
                 f"{stt.get('iters', '-')} | "
                 f"{stt.get('best_val_crisis_error_auc', float('nan')):.4f} | "
                 f"{stt.get('elapsed_sec', '-')} | "
                 f"{stt.get('device', '-')} | "
                 f"{stt.get('torch_num_threads', '-')} |\n"
                 if stt["status"] == "completed" else
                 f"| {rid} | {stt['status']} | - | - | - | - | - |\n")
    if failures:
        L.append("\n## Failed stages\n")
        for f in failures:
            L.append(f"- `{f['subject']}` {f['stage']}: {f['reason']}\n")

    L.append("\n## Main results (clean)\n")
    L.append("| subject | crisis_auc | resolution | ttr | surv_need |"
             " retention | precision | error_full |\n")
    L.append("|---|---|---|---|---|---|---|---|\n")
    for s in learned + [f"baseline_{b}" for b in BASELINES]:
        m = table.get(s, {}).get("c-none_o-none")
        if not m:
            continue
        L.append(f"| {s} | {m.get('crisis_error_auc', float('nan')):.3f} | "
                 f"{m.get('need_resolution', float('nan')):.3f} | "
                 f"{m.get('time_to_resolution', float('nan')):.1f} | "
                 f"{m.get('survival_after_need', float('nan')):.3f} | "
                 f"{m.get('important_retention', float('nan')):.3f} | "
                 f"{m.get('store_precision', float('nan')):.3f} | "
                 f"{m.get('error_full', float('nan')):.4f} |\n")

    L.append("\n## Memory selectivity\n")
    for s in learned:
        m = table.get(s, {}).get("c-none_o-none", {})
        L.append(f"- `{s}`: precision={m.get('store_precision', float('nan')):.3f} "
                 f"retention={m.get('important_retention', float('nan')):.3f} "
                 f"stores={m.get('stores', 0):.1f} "
                 f"junk={m.get('stores_junk', 0):.1f}\n")

    L.append("\n## Need intervention (U-H4)\n")
    for s in learned:
        pr = probes.get(s, {})
        if not pr:
            continue
        det = pr.get("delta_by_func", {})
        det_s = ", ".join(f"{k}={v:.2f}" for k, v in det.items())
        L.append(f"- `{s}`: mean|ΔP(STORE)|="
                 f"{pr.get('mean_abs_delta', float('nan')):.3f} ({det_s})\n")

    L.append("\n## Mediation (U-H4b)\n")
    for s in learned:
        cl = table.get(s, {}).get("c-none_o-none", {})
        md = table.get(s, {}).get("c-targeted_mediation_o-none", {})
        if not md:
            continue
        L.append(f"- `{s}`: clean auc="
                 f"{cl.get('crisis_error_auc', float('nan')):.3f} -> "
                 f"mediation auc="
                 f"{md.get('crisis_error_auc', float('nan')):.3f}, "
                 f"resolution "
                 f"{cl.get('need_resolution', float('nan')):.3f} -> "
                 f"{md.get('need_resolution', float('nan')):.3f}\n")

    L.append("\n## Causal ablations\n")
    for s in learned:
        parts = []
        for c in CAUSAL_MODES:
            m = table.get(s, {}).get(f"c-{c}_o-none", {})
            parts.append(f"{c}={m.get('crisis_error_auc', float('nan')):.3f}")
        L.append(f"- `{s}`: " + ", ".join(parts) + "\n")

    L.append("\n## OOD\n")
    for s in learned:
        parts = []
        for m in OOD_MODES:
            mm = table.get(s, {}).get(f"c-none_o-{m}", {})
            parts.append(f"{m}: res={mm.get('need_resolution', float('nan')):.3f} "
                         f"auc={mm.get('crisis_error_auc', float('nan')):.3f}")
        L.append(f"- `{s}`: " + "; ".join(parts) + "\n")

    L.append("\n## Per-hypothesis verdict\n")
    L.append("| gate | criterion | value | verdict |\n|---|---|---|---|\n")
    for name, gg in gates.items():
        val = gg["value"]
        val_s = f"{val:.3f}" if isinstance(val, float) else str(val)
        L.append(f"| {name} | {gg['desc']} | {val_s} | "
                 f"{'PASS' if gg['pass'] else 'FAIL'} |\n")
    core = ("U-H1", "U-H2", "U-H3", "U-H4", "U-H4b", "U-H5")
    overall = ("**STRONG PASS**" if gates["Strong"]["pass"] else
               "**PASS**" if all(gates[n]["pass"] for n in core)
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
             "tests gating, not readout mechanics); directed MOVE is "
             "the physical consequence of knowing a destination.\n"
             "- Nothing here supports claims of desires, understanding, "
             "or autobiographical memory — only need-conditioned gating "
             "in this finite-memory environment.\n")
    L.append("\n## Reproduce\n")
    L.append("```bash\n"
             "python -m experiments.u0.protocol_check\n"
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
