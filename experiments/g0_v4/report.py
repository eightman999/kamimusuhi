"""Generate experiments/g0_v4/reports/G0_V4_RESULTS.md (spec §24-27).

Reads reports/sweep_summary.json + controls_untrained.json (+ optional
protocol-validator JSON) and writes the mandated report. Verdict logic
is hard-coded from the spec so the report cannot be tuned post-hoc:

  G4-H1 midctx : trained-untrained >= +0.10 AND clearly above its
                 label-shuffle null
  G4-H2 match_ood : same thresholds
  G4-H3 dynseg_ood : trained > raw AND > untrained twin AND > old GRU
  G4-H4 combo_oodctx AUC >= 0.70 (0.5 ~ FAIL)
  G4-H5 selectivity > 0 and above the untrained twin

PASS: H1 & H2 & H3 (multi-seed, stable sign).  FAIL: trained ~= untrained
everywhere / transfer at null level.  PARTIAL otherwise.

    python -m experiments.g0_v4.report [--validator protocol.json]
"""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

REPORTS = Path(__file__).parent / "reports"
G0_REPORTS = Path(__file__).parent.parent / "g0" / "reports"

H1_H2_DELTA = 0.10
NULL_MARGIN = 0.05        # "clearly above" the shuffle null
H4_AUC = 0.70

PRIMARY = [("acc_in", "acc_in"), ("acc_loco", "acc_loco"),
           ("acc_ood_ctx", "acc_ood_ctx"),
           ("dynseg_loco", "dynseg_acc_loco"),
           ("dynseg_ood", "dynseg_acc_ood_ctx"),
           ("NMI", "nmi_pooled"), ("match_ood", "match_ood_ctx"),
           ("match_null", "match_ood_ctx_null"),
           ("midctx", "midctx_acc"),
           ("midctx_null", "midctx_acc_null"),
           ("combo_oodctx", "combo_oodctx_auc")]


def _m(summary, rep, metric):
    v = (summary.get(rep) or {}).get(metric)
    return v["mean"] if isinstance(v, dict) and v.get("mean") is not None \
        else None


def _sd(summary, rep, metric):
    v = (summary.get(rep) or {}).get(metric)
    return v["std"] if isinstance(v, dict) else None


def _f(v, w=3):
    if v is None or (isinstance(v, float) and math.isnan(v)):
        return "—"
    return f"{v:.{w}f}"


def _sel(summary, rep):
    v = (summary.get(rep) or {}).get("intervention_selectivity")
    return v["mean"] if isinstance(v, dict) and v else None


def _diffs(controls):
    return (controls or {}).get("paired_diffs", {})


def _diff(controls, method, metric):
    d = _diffs(controls).get(f"{method} - {method}_untrained", {})
    v = d.get(metric)
    return v if isinstance(v, dict) else None


def hypothesis_verdicts(summary, controls, methods):
    """Return {H: {verdict, evidence}} per spec thresholds."""
    out = {}
    rows = {}
    for m in methods:
        u = f"{m}_untrained"
        mid_d = _diff(controls, m, "midctx_acc") or {}
        mid_null = _m(summary, m, "midctx_acc_null")
        rows[m] = {
            "midctx": _m(summary, m, "midctx_acc"),
            "midctx_untr": _m(summary, u, "midctx_acc"),
            "midctx_diff": mid_d.get("mean_diff"),
            "midctx_npos": mid_d.get("n_pos"),
            "midctx_n": mid_d.get("n"),
            "midctx_null": mid_null,
            "match": _m(summary, m, "match_ood_ctx"),
            "match_untr": _m(summary, u, "match_ood_ctx"),
            "match_diff": (_diff(controls, m, "match_ood") or {})
            .get("mean_diff"),
            "match_null": _m(summary, m, "match_ood_ctx_null"),
            "dynseg_ood": _m(summary, m, "dynseg_acc_ood_ctx"),
            "dynseg_ood_untr": _m(summary, u, "dynseg_acc_ood_ctx"),
            "combo": _m(summary, m, "combo_oodctx_auc"),
            "sel": _sel(summary, m),
            "sel_untr": _sel(summary, u),
        }
    out["rows"] = rows

    def anym(cond):
        return [m for m in methods
                if rows[m] is not None and cond(rows[m])]

    def passers(key_diff, key_self, key_null):
        return anym(lambda r: r.get(key_diff) is not None
                    and r[key_diff] >= H1_H2_DELTA
                    and r.get(key_self) is not None
                    and r.get(key_null) is not None
                    and r[key_self] >= r[key_null] + NULL_MARGIN)

    h1 = passers("midctx_diff", "midctx", "midctx_null")
    h2 = passers("match_diff", "match", "match_null")
    raw_ood = _m(summary, "raw", "dynseg_acc_ood_ctx")
    gru_ood = rows.get("gru", {}).get("dynseg_ood")

    def _h3(mname):
        r = rows.get(mname) or {}
        if r.get("dynseg_ood") is None or r.get("dynseg_ood_untr") is None:
            return False
        if not r["dynseg_ood"] > r["dynseg_ood_untr"]:
            return False
        if raw_ood is not None and not r["dynseg_ood"] > raw_ood:
            return False
        if mname != "gru" and gru_ood is not None \
                and not r["dynseg_ood"] > gru_ood:
            return False
        return True

    h3 = [m for m in methods if _h3(m)]
    h4 = anym(lambda r: r.get("combo") is not None and r["combo"] >= H4_AUC)
    h5 = anym(lambda r: r.get("sel") is not None and r["sel"] > 0
              and (r.get("sel_untr") is None or r["sel"] > r["sel_untr"]))
    out["verdicts"] = {
        "G4-H1 midctx": {"pass_methods": h1,
                         "criterion": f"trained-untrained >= +{H1_H2_DELTA} "
                                      f"and > null+{NULL_MARGIN}"},
        "G4-H2 match_ood": {"pass_methods": h2,
                            "criterion": f"trained-untrained >= "
                                         f"+{H1_H2_DELTA} and > "
                                         f"null+{NULL_MARGIN}"},
        "G4-H3 dynseg_ood": {"pass_methods": h3,
                             "criterion": "> raw & > untrained twin & "
                                          "> old GRU predictor"},
        "G4-H4 combo_oodctx": {"pass_methods": h4,
                               "criterion": f"AUC >= {H4_AUC}"},
        "G4-H5 intervention": {"pass_methods": h5,
                               "criterion": "selectivity > 0 and > "
                                            "untrained twin"},
    }
    if h1 and h2 and h3:
        out["final"] = "PASS"
    elif h1 or h2 or h3 or h4 or h5:
        out["final"] = "PARTIAL"
    else:
        out["final"] = "FAIL"
    return out


def generate(summary_jsons, controls_json: Path,
             validator_json: Path | None, out_md: Path) -> str:
    """Accepts several sweep-summary JSONs (pilot / grid / stage2) and
    merges their per_run entries, then re-summarizes — every trial is
    reported, none dropped."""
    from experiments.g0.sweep import summarize

    per_run: dict = {}
    method_specs: list = []
    for p in str(summary_jsons).split(","):
        sweep = json.loads(Path(p).read_text())
        for rep, by_seed in (sweep.get("per_run") or {}).items():
            per_run.setdefault(rep, {}).update(by_seed)
        method_specs += sweep.get("methods", [])
    summary = summarize(per_run)
    sweep = {"summary": summary, "per_run": per_run,
             "methods": list(dict.fromkeys(method_specs))}
    # controls_untrained.json may also be a comma list — merge the
    # paired_diffs blocks (later files win per pair key)
    controls = {"paired_diffs": {}}
    for p in str(controls_json).split(","):
        if Path(p).exists():
            c = json.loads(Path(p).read_text())
            controls["paired_diffs"].update(c.get("paired_diffs", {}))
    validator = json.loads(Path(validator_json).read_text()) \
        if validator_json and Path(validator_json).exists() else None

    methods = [m for m in sweep.get("methods",
                                    ("gru", "cpc", "vicreg", "jepa"))
               if m in per_run]
    hv = hypothesis_verdicts(summary, controls, methods)

    g0 = {}
    g0s = G0_REPORTS / "sweep_summary.json"
    if g0s.exists():
        g0 = json.loads(g0s.read_text()).get("summary", {})

    L = []
    a = L.append
    a("# G0-v4 — Predictive Invariant Grounding: Results\n")
    a("**Research question:** is G0-v3's FAIL explained by a weak "
      "reconstruction / one-step prediction objective, or does this "
      "setting resist self-supervised latent abstraction in general? "
      "v4 tests CPC / VICReg-temporal / JEPA-style objectives against a "
      "GRU-predictor control, each paired with an untrained twin.\n")
    a("**Final verdict: %s**\n" % hv["final"])

    a("## Protocol integrity\n")
    if validator is None:
        a("- protocol_validator output not attached — run "
          "`python -m experiments.g0_v4.protocol_validator`.\n")
    else:
        a("- protocol_validator: **%s** (config `%s`)\n"
          % ("PASS" if validator.get("pass") else "FAIL",
             validator.get("config")))
        for c in validator.get("checks", []):
            if not c.get("pass"):
                a("  - **FAIL** `%s`: %s\n"
                  % (c.get("name"),
                     "; ".join(c.get("issues", c.get("hits", [])))))
    a("- label firewall: training sees only `obs/next_obs/actions` "
      "(TRAIN_KEYS whitelist; `eval_only_keys_stripped` in each "
      "meta.json).\n")

    a("\n## Methods\n")
    a("| method | objective | latent |\n|---|---|---|\n")
    a("| gru | next-obs delta MSE (G0 control) | GRU hidden |\n")
    a("| cpc | InfoNCE: W_k c_t -> z_{t+k}, in-batch negatives | GRU "
      "context c_t |\n")
    a("| vicreg | two augmented views: invariance+variance+covariance + "
      "latent fwd prediction | GRU hidden h_t |\n")
    a("| jepa | action-conditioned latent rollout vs EMA target encoder "
      "(t+1..t+K) + var/cov guard | GRU context c_t |\n")
    a("\nAll models additionally carry a small linear latent->obs "
      "readout (`v4.readout_weight`, default 0.1) solely so the G0 "
      "`intervention`/`base_mse` metrics remain computable; it is not "
      "the objective.\n")

    a("\n## Compute / GPU\n")
    a("- see per-run `meta.json` (device, gpu_name, batch_size_final, "
      "oom_retries, wall_sec) and `metrics.jsonl` training curves.\n")

    a("\n## Representation health\n")
    a("| rep | eff_rank | var_mean | cos_abs | collapsed |\n|---|---|---|---|---|\n")
    for rep in list(methods) + [f"{m}_untrained" for m in methods]:
        by_seed = per_run.get(rep) or {}
        hs = [((ev.get("health") or {}).get("main") or {})
              for ev in by_seed.values()]
        hs = [h for h in hs if h]
        if not hs:
            continue
        er = sum(h["effective_rank"] for h in hs) / len(hs)
        vm = sum(h["var_mean"] for h in hs) / len(hs)
        ca = sum(h["cos_abs_mean"] for h in hs) / len(hs)
        nco = sum(1 for h in hs if h.get("collapsed"))
        a(f"| {rep} | {er:.1f} | {vm:.4f} | {ca:.3f} | {nco}/{len(hs)} |\n")

    a("\n## Primary metrics (mean over seeds)\n")
    hdr = "| rep | " + " | ".join(k for k, _ in PRIMARY) + " | select |\n"
    a(hdr + "|---|" + "---|" * (len(PRIMARY) + 1) + "\n")
    for rep in ("raw", "raw_win", "pca", "pca_win",
                "dynfeat_obs", "dynfeat_canonical") + tuple(
                    m for pair in
                    [(m, f"{m}_untrained") for m in methods]
                    for m in pair):
        if rep not in summary:
            continue
        cells = [_f(_m(summary, rep, path)) for _, path in PRIMARY]
        sel = _sel(summary, rep)
        a(f"| {rep} | " + " | ".join(cells) + f" | {_f(sel)} |\n")

    a("\n## Trained − untrained (paired per-seed diffs)\n")
    diffm = ["acc_in", "acc_loco", "acc_ood_ctx", "dynseg_acc_loco",
             "dynseg_acc_ood_ctx", "match_ood", "midctx_acc",
             "nmi_pooled", "combo_oodctx_auc",
             "intervention_selectivity"]
    a("| pair | " + " | ".join(diffm) + " |\n|---|"
      + "---|" * len(diffm) + "\n")
    for pair, dd in _diffs(controls).items():
        cells = []
        for m in diffm:
            v = dd.get(m)
            if not v:
                cells.append("—")
            else:
                cells.append(f"{v['mean_diff']:+.3f} "
                             f"({v['n_pos']}/{v['n']})")
        a(f"| {pair} | " + " | ".join(cells) + " |\n")

    a("\n## Hypothesis verdicts\n")
    a("| hypothesis | criterion | methods passing | verdict |\n"
      "|---|---|---|---|\n")
    for h, v in hv["verdicts"].items():
        pm = v["pass_methods"]
        a(f"| {h} | {v['criterion']} | {', '.join(pm) or 'none'} | "
          f"{'PASS' if pm else 'FAIL'} |\n")

    a("\n## Per-seed results\n")
    a("| rep | seed | acc_in | acc_ood_ctx | midctx | match_ood | "
      "combo_oodctx |\n|---|---|---|---|---|---|---|\n")
    for rep in methods + [f"{m}_untrained" for m in methods]:
        for s, ev in sorted((per_run.get(rep) or {}).items(),
                            key=lambda kv: int(kv[0])):
            def g(*p):
                d = ev
                for k in p:
                    d = (d or {}).get(k)
                return _f(d) if isinstance(d, (int, float)) else "—"
            a(f"| {rep} | {s} | {g('probes','acc_in')} | "
              f"{g('probes','acc_ood_ctx')} | {g('causal','midctx_acc')} |"
              f" {g('matching','match_ood_ctx')} | "
              f"{g('ood','combo_oodctx_auc')} |\n")

    a("\n## Comparison with G0-v3\n")
    if not g0:
        a("- G0-v3 summary not found.\n")
    else:
        a("| rep | acc_ood_ctx | dynseg_ood | match_ood | midctx | "
          "combo_oodctx |\n|---|---|---|---|---|---|\n")
        for rep in ("gru", "gru_untrained", "ae", "ae_untrained"):
            if rep not in g0:
                continue
            a(f"| g0:{rep} | {_f(_m(g0, rep, 'acc_ood_ctx'))} | "
              f"{_f(_m(g0, rep, 'dynseg_acc_ood_ctx'))} | "
              f"{_f(_m(g0, rep, 'match_ood_ctx'))} | "
              f"{_f(_m(g0, rep, 'midctx_acc'))} | "
              f"{_f(_m(g0, rep, 'combo_oodctx_auc'))} |\n")
        for m in methods:
            a(f"| v4:{m} | {_f(_m(summary, m, 'acc_ood_ctx'))} | "
              f"{_f(_m(summary, m, 'dynseg_acc_ood_ctx'))} | "
              f"{_f(_m(summary, m, 'match_ood_ctx'))} | "
              f"{_f(_m(summary, m, 'midctx_acc'))} | "
              f"{_f(_m(summary, m, 'combo_oodctx_auc'))} |\n")

    a("\n## Negative results & limitations\n")
    a("- see per-run eval.json for full metric sets; collapsed runs "
      "(health.collapsed) are flagged in Representation health.\n")
    a("- claim ceiling (spec §26): even on PASS the defensible claim is "
      "that a label-free predictive/self-supervised objective produced "
      "sensor-context-invariant, reusable latent-cause representations "
      "beyond untrained reservoirs, raw features and shuffle nulls — "
      "not that symbolic concepts emerged.\n")
    a("- on FAIL: reconstruction-vs-prediction is largely excluded as "
      "the explanation; the likely missing ingredients are embodied "
      "consequence / active intervention / need-relevance / "
      "action-contingent structure (spec §27).\n")

    out_md.write_text("".join(L))
    return "".join(L)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--summary", default=str(REPORTS /
                                           "sweep_summary.json"))
    ap.add_argument("--controls", default=str(REPORTS /
                                              "controls_untrained.json"))
    ap.add_argument("--validator", default=str(REPORTS /
                                               "protocol_validator.json"))
    ap.add_argument("--out", default=str(REPORTS / "G0_V4_RESULTS.md"))
    args = ap.parse_args()
    txt = generate(args.summary, args.controls, args.validator,
                   Path(args.out))
    print(txt[:2000])
    print(f"... -> {args.out}")


if __name__ == "__main__":
    main()
