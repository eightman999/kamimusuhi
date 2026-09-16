"""Export an M1.5 ablation analysis as Observatory static JSON.

Reads ``analysis.json`` from an ablation run directory and writes a
small, self-describing ``ablation.json`` that the organism page loads
when present (the file is a DERIVED artifact — deleting it removes the
panel, it is never required for the GUI to function).

Usage:
    python -m experiments.mioba.scripts.m15_export_ablation \
        --analysis <ablation_run>/analysis.json \
        --out experiments/mioba/gui/static/ablation.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def export(analysis_path: str, out_path: str) -> dict:
    a = json.loads(Path(analysis_path).read_text())
    gid = a["source_genome"]
    full = (a.get("conditions") or {}).get("full") or {}
    organ_rates = full.get("organ_rates") or {}
    conds = {}
    for label, d in (a.get("paired_vs_full") or {}).items():
        if not d or "mean" not in d:
            continue
        entry = {
            "delta": d["mean"],
            "median": d.get("median"),
            "ci95": (d.get("bootstrap") or {}).get("ci95"),
            "consistency": (d.get("frac_removal_hurt") if d["mean"] > 0
                            else d.get("frac_removal_helped")),
            "classification": d.get("classification"),
            "selection_score_delta": d.get("selection_score_delta"),
            "n_pairs": d.get("n_pairs"),
        }
        if label.startswith("disable_organ:"):
            oid = label.split(":", 1)[1]
            rates = organ_rates.get(oid) or []
            entry["activity_hz"] = (sum(rates) / len(rates)
                                    if rates else None)
        conds[label] = entry
    doc = {
        "kind": "M1.5 organ ablation (DERIVED)",
        "source_experiment": a.get("source_experiment"),
        "source_generation": a.get("source_generation"),
        "replicates": a.get("replicates"),
        "evaluation_seed": a.get("evaluation_seed"),
        "target_rate_hz": a.get("target_rate_hz"),
        "full_fitness_mean": full.get("fitness_mean"),
        "genomes": {gid: {"full_fitness_mean": full.get("fitness_mean"),
                          "conditions": conds}},
    }
    Path(out_path).write_text(json.dumps(doc, indent=1))
    return doc


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--analysis", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    doc = export(args.analysis, args.out)
    print(f"[m15-export] {len(doc['genomes'])} genome(s), "
          f"{sum(len(g['conditions']) for g in doc['genomes'].values())} "
          f"conditions -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
