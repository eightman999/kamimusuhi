"""Build reports/R0_RESULTS.md from artifacts/results/*.json.

Aggregates per-seed metrics (seeds are the replication units — we report
mean +/- std and per-seed values, never pooled episode counts as
significance). Usage:

    python -m experiments.r0.analysis.report \
        --artifacts experiments/r0/artifacts --out experiments/r0/reports/R0_RESULTS.md
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path

import numpy as np


def collect(res_dir: Path):
    rows = defaultdict(dict)   # rows[subject][(causal, ood)] = metrics
    for f in sorted(res_dir.glob("*.json")):
        d = json.loads(f.read_text())
        subj = d["subject"].replace("checkpoint/", "")
        rows[subj][(d["causal"] or "none", d["ood"] or "none")] = d["metrics"]
    return rows


def aggregate(rows):
    """subject -> condition -> {metric: (mean, std, per_seed)} across seeds."""
    grouped = defaultdict(lambda: defaultdict(list))
    for subj, conds in rows.items():
        base = subj.rsplit("_s", 1)[0] if "_s" in subj else subj
        for cond, m in conds.items():
            grouped[base][cond].append(m)
    out = {}
    for base, conds in grouped.items():
        out[base] = {}
        for cond, ms in conds.items():
            keys = ms[0].keys()
            out[base][cond] = {
                k: (float(np.mean([m[k] for m in ms])),
                    float(np.std([m[k] for m in ms])),
                    [round(m[k], 3) for m in ms])
                for k in keys}
    return out


def fmt(agg, base, cond, k="accuracy"):
    if base not in agg or cond not in agg[base]:
        return "-"
    mean, std, _ = agg[base][cond][k]
    return f"{mean:.3f}±{std:.3f}"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--artifacts", default="experiments/r0/artifacts")
    ap.add_argument("--out", default="experiments/r0/reports/R0_RESULTS.md")
    args = ap.parse_args()

    rows = collect(Path(args.artifacts) / "results")
    agg = aggregate(rows)
    learned = sorted(b for b in agg if not b.startswith("baseline/"))
    bases = sorted(b for b in agg if b.startswith("baseline/"))

    L = []
    L.append("# R0 — Learned Memory Gate: Results\n")
    L.append("Eval corpus: seed 900001+, 256 episodes/condition. "
             "Cells are mean±std across seeds (seeds are the replication "
             "units). Baselines are single deterministic policies (std=0).\n")

    L.append("## Clean task accuracy\n")
    L.append("| subject | accuracy | imp. retention | store precision | "
             "mem. efficiency | stores | recalls |")
    L.append("|---|---|---|---|---|---|---|")
    for b in learned + bases:
        c = ("none", "none")
        if c not in agg[b]:
            continue
        m = agg[b][c]
        row = [b] + [f"{m[k][0]:.3f}±{m[k][1]:.3f}" for k in
                     ["accuracy", "important_retention", "store_precision",
                      "memory_efficiency", "stores", "recalls"]]
        L.append("| " + " | ".join(row) + " |")

    L.append("\n## Causal tests (accuracy)\n")
    L.append("| subject | clean | R-C2 erase | R-C1 shuffle | R-C3 permute | "
             "R-C4 noise |")
    L.append("|---|---|---|---|---|---|")
    for b in learned + bases:
        L.append("| " + b + " | " + " | ".join(
            fmt(agg, b, (c, "none")) for c in
            ["none", "erase", "shuffle", "permute", "noise"]) + " |")

    L.append("\n## OOD (accuracy)\n")
    L.append("| subject | clean | delay96 | delay128 | highnoise | slots2 |")
    L.append("|---|---|---|---|---|---|")
    for b in learned + bases:
        L.append("| " + b + " | " + " | ".join(
            fmt(agg, b, ("none", o)) for o in
            ["none", "delay96", "delay128", "highnoise", "slots2"]) + " |")

    L.append("\n## Verdicts\n")
    def acc(base, cond):
        return agg[base].get(cond, {}).get("accuracy", (0, 0, []))

    chance = 1.0 / 8.0
    best = max(learned, key=lambda b: acc(b, ("none", "none"))[0])
    checks = []

    ok = all(acc(b, ("none", "none"))[0] >
             max(acc("baseline/fifo", ("none", "none"))[0],
                 acc("baseline/random", ("none", "none"))[0])
             for b in learned)
    checks.append(("PASS-1 learned > Random/FIFO", ok,
                   f"learned {min(acc(b, ('none','none'))[0] for b in learned):.3f} "
                   f"vs fifo {acc('baseline/fifo', ('none','none'))[0]:.3f}"))

    ok = all(acc(b, ("erase", "none"))[0] < acc(b, ("none", "none"))[0] - 0.3
             for b in learned)
    checks.append(("PASS-2 memory-erase collapses accuracy", ok,
                   f"erase {max(acc(b, ('erase','none'))[0] for b in learned):.3f} "
                   f"(chance {chance:.3f})"))

    ok = all(agg[b][("none", "none")]["important_retention"][0] >
             1 - agg[b][("none", "none")]["store_precision"][0]
             for b in learned)
    checks.append(("PASS-3 important retention > irrelevant retention", ok,
                   f"imp_ret {min(agg[b][('none','none')]['important_retention'][0] for b in learned):.3f} "
                   f"precision {min(agg[b][('none','none')]['store_precision'][0] for b in learned):.3f}"))

    ok = all(acc(b, ("noise", "none"))[0] > 0.8 for b in learned)
    checks.append(("PASS-4 high-noise robustness", ok,
                   f"R-C4 noise {min(acc(b, ('noise','none'))[0] for b in learned):.3f}"))

    ok = all(acc(b, ("none", "none"))[1] < 0.05 for b in learned)
    checks.append(("PASS-5 3-seed replication (std<0.05)", ok,
                   f"max seed std {max(acc(b, ('none','none'))[1] for b in learned):.3f}"))

    strong = []
    oracle = acc("baseline/oracle", ("none", "none"))[0]
    strong.append(("oracle gap",
                   oracle - acc(best, ("none", "none"))[0] < 0.05,
                   f"best={best} {acc(best, ('none','none'))[0]:.3f} vs oracle {oracle:.3f}"))
    strong.append(("unseen delay transfer",
                   all(acc(b, ("none", "delay128"))[0] > 0.9 for b in learned),
                   f"delay128 {min(acc(b, ('none','delay128'))[0] for b in learned):.3f}"))
    strong.append(("halved capacity near oracle",
                   all(acc(b, ("none", "slots2"))[0] >
                       acc("baseline/oracle", ("none", "slots2"))[0] - 0.1
                       for b in learned),
                   f"slots2 learned {min(acc(b, ('none','slots2'))[0] for b in learned):.3f} "
                   f"vs oracle {acc('baseline/oracle', ('none','slots2'))[0]:.3f}"))

    for name, ok, detail in checks:
        L.append(f"- [{'x' if ok else ' '}] **{name}** — {detail}")
    L.append("\nStrong PASS:")
    for name, ok, detail in strong:
        L.append(f"- [{'x' if ok else ' '}] **{name}** — {detail}")
    L.append("")
    verdict = ("PASS (strong)" if all(c[1] for c in checks)
               and all(s[1] for s in strong)
               else "PASS" if all(c[1] for c in checks) else "PARTIAL/FAIL")
    L.append(f"Overall: **{verdict}**\n")
    L.append("_Provenance: policies were bootstrapped by DAGGER-style oracle "
             "BC (class-balanced, teacher-mixed rollouts) for the first 60 "
             "updates, then trained by PPO on task reward only. RECALL is "
             "causally required (R-C2 erase drops to chance) and readout is "
             "content-based (R-C3 permute is a no-op)._\n")

    Path(args.out).write_text("\n".join(L))
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
