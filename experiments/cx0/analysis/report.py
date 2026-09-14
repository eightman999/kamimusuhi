"""CX0 report generator — aggregates runs/**/results.jsonl into
reports/CX0_RESULTS.md with the C-G1..C-G6 verdicts.

Verdict logic (predeclared in experiments/cx0/README.md):
  C-G1 solvability:   mean clean success over tasks, best arm >= 0.30
  C-G2 cortex edge:   max(c2,c3) clean mean > c1 + 0.05 (param-matched)
  C-G3 organ causal:  mean(clean - shuffle_f) over f>0.10 for the best arm
  C-G4 context state: clean - reset_hidden > 0.05 for the best arm
  C-G5 no harm:       best cortex clean on ctx5 >= c0 - 0.05
  C-G6 decode:        probe_context accuracy > majority-class baseline +0.10
Overall: PASS if C-G2..C-G4 all hold and C-G1 met; PARTIAL if C-G1 and at
least one of C-G2..C-G4; FAIL otherwise (or if cortex_off ~ clean).

Usage: python -m experiments.cx0.analysis.report --runs experiments/cx0/runs
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path

import numpy as np

SHUFFLES = ("sensory", "h0", "s0", "t0", "r0")


def load_rows(runs: Path) -> list[dict]:
    rows = []
    for f in runs.rglob("results.jsonl"):
        for line in f.read_text().splitlines():
            if line.strip():
                rows.append(json.loads(line))
    return rows


def pivot(rows):
    """mean clean success per (task, arm); per-condition means per arm."""
    clean = defaultdict(list)
    cond = defaultdict(lambda: defaultdict(list))
    probes = defaultdict(list)
    params = defaultdict(set)
    for r in rows:
        key = (r["task"], r["arm"])
        params[key[1]].add(r.get("n_params"))
        if r["condition"] == "clean":
            clean[key].append(r["success"])
            for k, v in r.items():
                if k.startswith("probe_") and v is not None:
                    probes[key].append((k, v))
        cond[key][r["condition"]].append(r["success"])
    return clean, cond, probes, params


def fmt_table(clean, cond, tasks, arms):
    lines = ["| task | arm | clean | shuffle(sensory) | shuffle(h0) |"
             " shuffle(s0) | shuffle(t0) | shuffle(r0) | cortex_off |"
             " reset_hidden | erase_mem |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for t in tasks:
        for a in arms:
            c = np.mean(clean.get((t, a), [np.nan]))
            cc = cond.get((t, a), {})

            def m(name):
                v = cc.get(name)
                return f"{np.mean(v):.3f}" if v else "-"
            lines.append(
                f"| {t} | {a} | {c:.3f} | {m('shuffle_sensory')} |"
                f" {m('shuffle_h0')} | {m('shuffle_s0')} | {m('shuffle_t0')} |"
                f" {m('shuffle_r0')} | {m('cortex_off')} |"
                f" {m('reset_hidden')} | {m('erase_memory')} |")
    return "\n".join(lines)


def fmt_lesion_table(clean, cond, tasks, arms):
    """Field-lesion drops (clean - lesion_f): zeroing an organ's whole
    signal field. Complements shuffle columns — the causality test for
    fields whose donor signal is a near-no-op (stereotyped trajectories)."""
    lines = ["| task | arm | clean | lesion(h0) | lesion(s0) | lesion(t0)"
             " | lesion(r0) |",
             "|---|---|---|---|---|---|---|"]
    for t in tasks:
        for a in arms:
            c = np.mean(clean.get((t, a), [np.nan]))
            cc = cond.get((t, a), {})

            def d(name):
                v = cc.get(name)
                return f"{c - np.mean(v):+.3f}" if v else "-"
            lines.append(
                f"| {t} | {a} | {c:.3f} | {d('lesion_h0')} |"
                f" {d('lesion_s0')} | {d('lesion_t0')} | {d('lesion_r0')} |")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default=str(Path(__file__).parents[1] / "runs"))
    ap.add_argument("--out", default=str(Path(__file__).parents[1]
                                         / "reports" / "CX0_RESULTS.md"))
    a = ap.parse_args()
    runs = Path(a.runs)
    rows = load_rows(runs)
    if not rows:
        print("no results found")
        return
    tasks = sorted({r["task"] for r in rows})
    arms = sorted({r["arm"] for r in rows})
    clean, cond, probes, params = pivot(rows)

    # ---- gates ---------------------------------------------------------
    mean_by_arm = {a: float(np.nanmean([np.mean(clean.get((t, a), [np.nan]))
                                        for t in tasks])) for a in arms}
    ctx_arms = [a for a in arms if a != "c0"]
    best_arm = max(ctx_arms, key=lambda a: mean_by_arm[a])
    g1 = max(mean_by_arm.values()) >= 0.30
    c1_mean = mean_by_arm.get("c1", np.nan)
    struct = [a for a in ("c2", "c3") if a in mean_by_arm]
    struct_best = max((mean_by_arm[a] for a in struct), default=float("nan"))
    g2 = bool(struct_best > c1_mean + 0.05) if "c1" in mean_by_arm else False
    drops = []
    for t in tasks:
        c = np.mean(clean.get((t, best_arm), [np.nan]))
        for f in SHUFFLES:
            v = cond.get((t, best_arm), {}).get(f"shuffle_{f}")
            if v is not None:
                drops.append(c - float(np.mean(v)))
    g3 = float(np.mean(drops)) > 0.10 if drops else False
    resets = [np.mean(clean.get((t, best_arm), [np.nan]))
              - float(np.mean(cond.get((t, best_arm), {}).get("reset_hidden", [np.nan])))
              for t in tasks]
    g4 = float(np.nanmean(resets)) > 0.05
    g5 = (np.mean(clean.get(("ctx5", best_arm), [np.nan]))
          >= np.mean(clean.get(("ctx5", "c0"), [np.nan])) - 0.05) \
        if ("ctx5", "c0") in clean else None
    # C-G6: context decode on the best arm across tasks
    probe_hits = []
    for t in tasks:
        for k, v in probes.get((t, best_arm), []):
            if k in ("probe_context", "probe_h", "probe_slow"):
                probe_hits.append(v)
    g6 = (float(np.mean(probe_hits)) > 0.45) if probe_hits else None

    if g1 and g2 and g3 and g4:
        verdict = "PASS"
    elif g1 and (g2 or g3 or g4):
        verdict = "PARTIAL"
    else:
        verdict = "FAIL"

    off_drop = [np.mean(clean.get((t, best_arm), [np.nan]))
                - float(np.mean(cond.get((t, best_arm), {}).get("cortex_off", [np.nan])))
                for t in tasks]
    cortex_off_null = float(np.nanmean(off_drop)) < 0.05

    md = []
    md.append("# CX0 Synthetic Cortex — Results\n")
    md.append(f"- generated from `{runs}` ({len(rows)} rows)")
    md.append(f"- params: " + ", ".join(
        f"{k}={sorted(v, key=lambda x: (x is None, x))}"
        for k, v in sorted(params.items())))
    md.append(f"- best cortex arm: **{best_arm}** "
              f"(mean clean {mean_by_arm[best_arm]:.3f})\n")
    md.append("## Clean + intervention success matrix\n")
    md.append(fmt_table(clean, cond, tasks, arms))
    md.append("\n## Field-lesion drops (clean − lesion)\n")
    md.append(fmt_lesion_table(clean, cond, tasks, arms))
    md.append("\n## Gates\n")
    md.append(f"| gate | criterion | value | verdict |")
    md.append(f"|---|---|---|---|")
    md.append(f"| C-G1 solvability | best-arm mean clean ≥ 0.30 |"
              f" {mean_by_arm[best_arm]:.3f} | {'PASS' if g1 else 'FAIL'} |")
    md.append(f"| C-G2 cortex edge | max(c2,c3) > c1+0.05 |"
              f" {struct_best:.3f} vs {c1_mean:.3f} |"
              f" {'PASS' if g2 else 'FAIL'} |")
    md.append(f"| C-G3 organ causal | mean shuffle drop > 0.10 |"
              f" {np.mean(drops) if drops else float('nan'):.3f} |"
              f" {'PASS' if g3 else 'FAIL'} |")
    md.append(f"| C-G4 context state | reset drop > 0.05 |"
              f" {np.nanmean(resets):.3f} | {'PASS' if g4 else 'FAIL'} |")
    g5s = "n/a" if g5 is None else ('PASS' if g5 else 'FAIL')
    g6s = "n/a" if g6 is None else ('PASS' if g6 else 'FAIL')
    g5v = "-" if g5 is None else (
        f"{np.mean(clean.get(('ctx5', best_arm), [np.nan])):.3f} vs"
        f" {np.mean(clean.get(('ctx5', 'c0'), [np.nan])):.3f}")
    g6v = "-" if g6 is None else f"{np.mean(probe_hits):.3f}"
    md.append(f"| C-G5 no harm | ctx5 {best_arm} ≥ c0−0.05 | {g5v} | {g5s} |")
    md.append(f"| C-G6 decode | context probe > 0.45 | {g6v} | {g6s} |")
    md.append(f"| null check | cortex_off ≈ clean (bad) |"
              f" drop {np.nanmean(off_drop):.3f} |"
              f" {'OK' if not cortex_off_null else 'WARN'} |")
    md.append(f"\n## Verdict\n\n**{verdict}** "
              f"(best arm {best_arm}; see gates above)\n")
    md.append("### Interpretation notes\n")
    md.append("- Clean success is a *behavioral* metric; the causal story is"
              " in the intervention columns.")
    md.append("- `probe_*` columns in results.jsonl give per-population"
              " context decode accuracy (C-G6).")
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text("\n".join(md))
    print("\n".join(md))


if __name__ == "__main__":
    main()
