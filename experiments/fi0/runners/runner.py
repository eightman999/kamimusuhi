"""Experiment runner: execute a sweep, persist raw rows, compute metrics.

Outputs under ``results/<run_id>/``:

* ``raw.jsonl``      — one JSON object per (task, variant, rep); includes
                       the full prompt text and response (raw log
                       preservation — nothing is post-hoc reconstructed)
* ``manifest.json``  — git commit/branch/dirty, config + config sha256,
                       seed, python/platform versions, UTC timestamp,
                       row counts (reproducibility record)
* ``analysis.json``  — per-task + overall metrics and the verdict line
"""

from __future__ import annotations

import json
import platform
import subprocess
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, Iterable, List, Optional

from experiments.fi0.analysis import agreement, cache, drift
from experiments.fi0.sweep import SweepConfig
from experiments.fi0.tasks import TASKS


def _git(*args: str) -> str:
    try:
        return subprocess.run(
            ["git", *args],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    except Exception:
        return "unknown"


def collect_manifest(
    cfg: SweepConfig, model_name: str, n_rows: int, n_variants: int
) -> Dict[str, object]:
    return {
        "experiment": "fi0-prompt-stability",
        "run_at_utc": datetime.now(timezone.utc).isoformat(),
        "git_commit": _git("rev-parse", "HEAD"),
        "git_branch": _git("rev-parse", "--abbrev-ref", "HEAD"),
        "git_dirty": bool(_git("status", "--porcelain")),
        "python_version": sys.version.split()[0],
        "platform": platform.platform(),
        "model": model_name,
        "seed": cfg.seed,
        "reps": cfg.reps,
        "config": json.loads(json.dumps(_config_view(cfg))),
        "config_sha256": cfg.sha256(),
        "n_rows": n_rows,
        "n_variants": n_variants,
    }


def _config_view(cfg: SweepConfig) -> Dict[str, object]:
    return {
        "name": cfg.name,
        "seed": cfg.seed,
        "model": cfg.model,
        "reps": cfg.reps,
        "tasks": list(cfg.tasks),
        "variants": {
            "orders": list(cfg.variants.orders),
            "formats": list(cfg.variants.formats),
            "whitespace": list(cfg.variants.whitespace),
            "metadata_positions": list(cfg.variants.metadata_positions),
            "include_canonical": cfg.variants.include_canonical,
        },
    }


def analyze(rows: List[Dict[str, object]]) -> Dict[str, object]:
    """Compute all spec metrics from raw rows."""
    by_task: Dict[str, List[Dict[str, object]]] = defaultdict(list)
    for r in rows:
        by_task[str(r["task"])].append(r)

    per_task: Dict[str, object] = {}
    for name, trs in sorted(by_task.items()):
        task = TASKS[name]
        by_variant = agreement.choices_by_variant(trs)
        prompts = {str(r["variant"]): str(r["prompt"]) for r in trs}
        cons = drift.consistency(
            trs, expected=task.expected(task.build()), marker=task.marker
        )
        canon_shas = {str(r["canonical_sha256"]) for r in trs}
        lat = [float(r["latency_ms"]) for r in trs]
        toks = [int(r["prompt_tokens"]) for r in trs]
        per_task[name] = {
            "n_runs": len(trs),
            "n_variants": len(by_variant),
            "exact_agreement": agreement.exact_agreement(
                [str(r["choice"]) for r in trs]
            ),
            "pairwise_agreement": agreement.pairwise_agreement(by_variant),
            "drift": drift.per_task_drift(trs, task.options),
            "semantic_consistency": cons["semantic"],
            "persona_consistency": cons["persona"],
            "distinct_canonical_sha256": len(canon_shas),
            "canonical_integrity_failures": sum(
                1 for r in trs if not r["canonical_matches_build"]
            ),
            "latency_ms": {
                "mean": sum(lat) / len(lat),
                "min": min(lat),
                "max": max(lat),
            },
            "prompt_tokens": {
                "mean": sum(toks) / len(toks),
                "min": min(toks),
                "max": max(toks),
            },
            "cacheability": cache.cacheability_by_group(prompts),
            "choice_by_variant": dict(sorted(by_variant.items())),
        }

    pair = [v["pairwise_agreement"] for v in per_task.values()]
    jsm = [v["drift"]["js_mean"] for v in per_task.values()]
    sem = [v["semantic_consistency"] for v in per_task.values()]
    per = [v["persona_consistency"] for v in per_task.values()]
    canon_bad = sum(
        v["canonical_integrity_failures"] for v in per_task.values()
    )
    canon_sets = [v["distinct_canonical_sha256"] for v in per_task.values()]

    overall = {
        "mean_pairwise_agreement": sum(pair) / len(pair) if pair else 1.0,
        "min_pairwise_agreement": min(pair) if pair else 1.0,
        "mean_js_divergence": sum(jsm) / len(jsm) if jsm else 0.0,
        "max_js_divergence": max(
            v["drift"]["js_max"] for v in per_task.values()
        ) if per_task else 0.0,
        "mean_semantic_consistency": sum(sem) / len(sem) if sem else 0.0,
        "mean_persona_consistency": sum(per) / len(per) if per else 0.0,
        "canonical_integrity_failures": canon_bad,
        "max_distinct_canonical_sha256_per_task": max(canon_sets) if canon_sets else 0,
    }

    # Canonicalization benefit: raw-variant prompts map to how many
    # distinct canonical byte strings per task?  (Expect exactly 1.)
    overall["canonical_collapses_variants"] = all(
        v["distinct_canonical_sha256"] == 1 for v in per_task.values()
    )
    return {"per_task": per_task, "overall": overall}


def verdict(analysis: Dict[str, object], model_name: str) -> str:
    ov = analysis["overall"]
    if model_name.startswith("sensitive"):
        if ov["mean_pairwise_agreement"] < 0.9:
            return (
                "DRIFT DETECTED (positive control): byte-sensitive client "
                "shows serialization drift — harness detection capability "
                "confirmed"
            )
        return "WARNING: positive control failed to drift"
    if ov["mean_pairwise_agreement"] >= 0.999 and ov["mean_js_divergence"] == 0:
        return (
            "NO MATERIAL INSTABILITY DETECTED: semantics-only model is "
            "invariant across all serializations"
        )
    return (
        f"INSTABILITY DETECTED: mean pairwise agreement "
        f"{ov['mean_pairwise_agreement']:.3f}"
    )


def run_evaluation(
    cfg: SweepConfig,
    client,
    results_dir: Path,
    run_id: Optional[str] = None,
) -> Dict[str, object]:
    """Execute the sweep, write raw.jsonl + manifest.json + analysis.json."""
    from experiments.fi0.sweep import run_sweep

    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_id = run_id or f"{ts}-{cfg.model}-{cfg.name}"
    out = Path(results_dir) / run_id
    out.mkdir(parents=True, exist_ok=True)

    rows: List[Dict[str, object]] = []
    raw_path = out / "raw.jsonl"
    with raw_path.open("w") as f:
        for row in run_sweep(cfg, client):
            rows.append(row)
            f.write(json.dumps(row, sort_keys=True) + "\n")

    analysis = analyze(rows)
    analysis["verdict"] = verdict(analysis, client.name)
    analysis["model"] = client.name

    manifest = collect_manifest(
        cfg, client.name, len(rows), len(cfg.variant_specs())
    )
    (out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    )
    (out / "analysis.json").write_text(
        json.dumps(analysis, indent=2, sort_keys=True) + "\n"
    )
    return {
        "run_dir": str(out),
        "n_rows": len(rows),
        "manifest": manifest,
        "analysis": analysis,
    }
