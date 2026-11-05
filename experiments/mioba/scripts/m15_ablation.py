"""M1.5 organ-ablation runner (instruction §2-§6).

Measures whether the structures evolution added to one M1 individual are
*functional*: each enabled artificial organ is disabled in turn, plus the
whole-organ and attachment ablations, and every variant is re-evaluated
against the identical replicate seed set the full genome sees (paired
comparison — ``replicate_seed``/``disturbance_seed`` are pure functions of
the shared evaluation seed).

This is a measurement run, not an experiment: no coordinator, no births,
no selection, and the source M1 database is opened read-only. Variants
are derived genomes with ``enabled = False`` — development simply does not
build disabled organs (and drops attachments that would touch them), so
the genome itself is never destroyed.

Outputs under ``--out`` (default ``<runs_dir>/M1.5-ablation-<ts>/``):

    variants/<label>.genome.json   the exact genome each condition ran
    results/<label>.json           worker summary + derived per-replicate rows
    analysis.json                  paired Δfitness stats + classification
    ABLATION_TABLE.md              human-readable §17 ablation table
    manifest.json                  provenance (source run, seeds, hashes)

Usage:
    python -m experiments.mioba.scripts.m15_ablation \
        --config experiments/mioba/configs/m15_ablation.yaml \
        --m1-db experiments/mioba/.runs/<M1 run>/lineage.sqlite \
        [--genome b2b:<id>] [--procs N]
"""
from __future__ import annotations

import argparse
import json
import math
import multiprocessing as mp
import random
import sqlite3
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path


def _load_config(path: str) -> dict:
    import yaml
    with open(path) as fh:
        return yaml.safe_load(fh)


def _clone(genome):
    from ..genome.schema import Genome
    return Genome.from_json(genome.to_json())


def build_variants(genome) -> list[tuple[str, object]]:
    """Non-destructive ablation variants of ``genome``.

    ``full`` keeps the original genome identity; every disabled variant is
    re-finalised so its content hash differs from the source genome it was
    derived from (``variant_of`` in the manifest preserves the link).
    """
    variants = [("full", _clone(genome))]

    def variant(label, mutate):
        v = _clone(genome)
        mutate(v)
        v.genome_id = ""
        v.finalize()
        variants.append((label, v))

    for organ in genome.artificial_organs:
        if not getattr(organ, "enabled", True):
            continue  # already a no-op in development — not a condition
        variant(f"disable_organ:{organ.organ_id}",
                lambda v, oid=organ.organ_id: [
                    setattr(o, "enabled", False) for o in v.artificial_organs
                    if o.organ_id == oid])
    variant("all_organs_disabled",
            lambda v: [setattr(o, "enabled", False)
                       for o in v.artificial_organs])
    variant("attachments_disabled",
            lambda v: [setattr(a, "enabled", False)
                       for a in v.attachments])
    variant("all_artificial_disabled",
            lambda v: ([setattr(o, "enabled", False)
                        for o in v.artificial_organs],
                       [setattr(a, "enabled", False)
                        for a in v.attachments]))
    return variants


def _run_condition(task: dict) -> dict:
    """One worker-like evaluation for one variant (child process entry)."""
    # heavy imports live inside the child: the spawn context re-imports the
    # module anyway, but keeping them here keeps --help/parse fast.
    from ..workers.worker import run_job
    label = task["label"]
    t0 = time.time()
    try:
        body = run_job(None, task["worker_id"], task["job"], task["device"],
                       task["execution_batch"], deliver=False,
                       profile=True)
    except Exception as exc:  # a variant that cannot develop is a result
        return {"label": label, "status": "FAILED",
                "error": f"{type(exc).__name__}: {exc}",
                "wall_s_total": time.time() - t0}
    body["label"] = label
    body["wall_s_total"] = time.time() - t0
    return body


def _per_replicate(summary: dict, target: float) -> list[dict]:
    """One row per replicate, aligned by replicate index (paired key)."""
    rates = summary.get("per_replicate_mean_rate_hz") or []
    # per_lane is built from sorted(lane_reports) — already replicate-ordered
    lanes = ((summary.get("episode") or {}).get("per_lane")) or []
    rows = []
    for i, rate in enumerate(rates):
        lane = lanes[i] if i < len(lanes) else {}
        rows.append({
            "replicate_index": i,
            "mean_rate_hz": rate,
            "fitness": -abs(rate - target) if rate is not None else None,
            "task_score_clean": lane.get("task_score_clean"),
            "task_score_all": lane.get("task_score_all"),
            "terminated_early": lane.get("terminated_early"),
            "n_disturbances": len(lane.get("disturbances") or []),
            "peak_debt": lane.get("peak_debt"),
        })
    return rows


def _condition_metrics(label: str, body: dict, config: dict,
                       target: float) -> dict:
    from ..evolution import fitness as F
    from ..genome.schema import Genome
    from ..development.phenotype import develop
    summary = (body.get("evaluation") or {}).get("summary") or {}
    genome = Genome.from_json(body["_genome_json"])
    structure = develop(genome).get("structure")
    metrics = F.compute_metrics(summary, structure, config, target)
    rows = _per_replicate(summary, target)
    fits = [r["fitness"] for r in rows if r["fitness"] is not None]
    activity = summary.get("activity") or {}
    organ_rates = {k[6:]: v for k, v in activity.items()
                   if k.startswith("organ:")}
    flat_organ = [x for vals in organ_rates.values() for x in vals]
    resource = summary.get("resource") or {}
    return {
        "label": label,
        "status": body.get("status"),
        "error": body.get("error"),
        "fitness_placeholder": F.fitness_placeholder(summary, target),
        "selection_score": F.selection_score(metrics, config),
        "fitness_replicates": fits,
        "fitness_mean": (sum(fits) / len(fits)) if fits else None,
        "fitness_std": (math.sqrt(sum((f - sum(fits) / len(fits)) ** 2
                                      for f in fits) / max(1, len(fits) - 1))
                        if len(fits) > 1 else None),
        "fitness_median": _median(fits),
        "task_score": (metrics.get("task_score")),
        "task_quality": metrics.get("task_quality"),
        "homeostasis_score": metrics.get("homeostasis_score"),
        "disturbance_recovery_score":
            metrics.get("disturbance_recovery_score"),
        "resource_efficiency_score":
            metrics.get("resource_efficiency_score"),
        "resource_efficiency_gated":
            metrics.get("resource_efficiency_gated"),
        "resource_cost_normalised":
            metrics.get("resource_cost_normalised"),
        "substrate_mean_rate_hz": _mean_or_none(
            activity.get("fba0") or []),
        "organ_mean_rate_hz": _mean_or_none(flat_organ),
        "organ_max_rate_hz": max(flat_organ) if flat_organ else None,
        "organ_rates": organ_rates,
        "spikes_total": summary.get("spikes_total"),
        "active_propagated_edges": resource.get("active_propagated_edges"),
        "active_propagated_edges_artificial":
            resource.get("active_propagated_edges_artificial"),
        "estimated_operations": resource.get("estimated_operations"),
        "wall_s": summary.get("wall_s"),
        "wall_s_total": body.get("wall_s_total"),
        "simulated_ms": ((summary.get("episode") or {})
                         .get("simulated_ms")),
        "terminated_fraction": metrics.get("terminated_fraction"),
        "circuit": summary.get("circuit"),
        "replicate_rows": rows,
        "metrics": {k: v for k, v in metrics.items()
                    if k not in ("descriptor", "resource", "departure")},
    }


def _mean_or_none(vals):
    vals = [v for v in vals if v is not None]
    return (sum(vals) / len(vals)) if vals else None


def _median(vals):
    vals = sorted(v for v in vals if v is not None)
    if not vals:
        return None
    n = len(vals)
    return (vals[n // 2] if n % 2 else (vals[n // 2 - 1] + vals[n // 2]) / 2)


def _paired_delta(full: dict, other: dict) -> dict | None:
    """Paired full-minus-ablated stats over shared replicate indices."""
    a = {r["replicate_index"]: r["fitness"] for r in full["replicate_rows"]}
    b = {r["replicate_index"]: r["fitness"] for r in other["replicate_rows"]}
    common = sorted(i for i in a if i in b
                    and a[i] is not None and b[i] is not None)
    if not common:
        return None
    deltas = [a[i] - b[i] for i in common]  # >0 = removal hurt = functional
    n = len(deltas)
    mean = sum(deltas) / n
    std = math.sqrt(sum((d - mean) ** 2 for d in deltas)
                    / max(1, n - 1)) if n > 1 else 0.0
    return {"n_pairs": n, "deltas": deltas, "mean": mean,
            "median": _median(deltas), "std": std,
            "min": min(deltas), "max": max(deltas),
            "frac_removal_hurt": sum(1 for d in deltas if d > 0) / n,
            "frac_removal_helped": sum(1 for d in deltas if d < 0) / n}


def _bootstrap_ci(deltas: list[float], iters: int, seed: int) -> dict:
    rng = random.Random(seed)
    n = len(deltas)
    means = sorted(sum(deltas[rng.randrange(n)] for _ in range(n)) / n
                   for _ in range(int(iters)))
    return {"ci95": [means[int(0.025 * iters)], means[int(0.975 * iters)]],
            "iters": iters}


def _classify(delta: dict, th: dict) -> str:
    """§5 labels from config thresholds. Sign convention: positive mean
    delta = ablation lowered fitness = the removed organ was functional."""
    mean = delta["mean"]
    consistent_dir = (delta["frac_removal_hurt"] if mean > 0
                      else delta["frac_removal_helped"])
    consistent = consistent_dir >= float(th["consistency_fraction"])
    if mean >= float(th["functional_delta"]) and consistent:
        return "functional"
    if mean <= -float(th["functional_delta"]) and consistent:
        return "harmful"
    if abs(mean) < float(th["weak_delta"]):
        return "neutral"
    return "weak_or_ambiguous"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--config", required=True)
    ap.add_argument("--m1-db", required=True, help="source run lineage.sqlite")
    ap.add_argument("--genome", default=None,
                    help="genome_id (default: best fitness of latest gen)")
    ap.add_argument("--out", default=None)
    ap.add_argument("--procs", type=int, default=None)
    ap.add_argument("--replicates", type=int, default=None)
    args = ap.parse_args()

    config = _load_config(args.config)
    abl = config.get("ablation") or {}
    seed = int(abl.get("seed", 0))
    n_rep = int(args.replicates or abl.get("replicates", 32))
    exec_batch = int(abl.get("execution_batch", 4))
    device = str(abl.get("device", "cpu"))
    procs = int(args.procs or abl.get("procs", 4))
    th = abl.get("thresholds") or {}
    target = float((config.get("evaluation") or {})
                   .get("target_rate_hz", 5.0))

    db = sqlite3.connect(f"file:{args.m1_db}?mode=ro", uri=True)
    if args.genome:
        gid = args.genome
    else:
        row = db.execute(
            """select e.genome_id, e.fitness from evaluations e
               join genomes g on g.genome_id = e.genome_id
               and g.experiment_id = e.experiment_id
               where g.generation = (select max(generation) from genomes)
               order by e.fitness desc limit 1""").fetchone()
        gid = row[0]
        print(f"[m15] best of latest generation: {gid} fitness={row[1]:.4f}")
    row = db.execute("select genome_json, experiment_id, generation "
                     "from genomes where genome_id=?", (gid,)).fetchone()
    if not row:
        raise SystemExit(f"genome {gid} not found in {args.m1_db}")
    genome_json, source_exp, source_gen = row

    from ..genome.schema import Genome
    genome = Genome.from_json(genome_json)
    variants = build_variants(genome)
    enabled = [o.organ_id for o in genome.artificial_organs
               if getattr(o, "enabled", True)]
    print(f"[m15] {len(variants)} conditions over {len(enabled)} enabled "
          f"organs, N={n_rep} paired replicates, seed={seed}")

    out = Path(args.out or (Path(config["experiment"]["runs_dir"])
                            / f"M1.5-ablation-{int(time.time())}"))
    (out / "variants").mkdir(parents=True, exist_ok=True)
    (out / "results").mkdir(parents=True, exist_ok=True)

    tasks = []
    for i, (label, vg) in enumerate(variants):
        (out / "variants" / f"{label.replace(':', '_')}.genome.json")\
            .write_text(vg.to_json())
        tasks.append({
            "label": label,
            "worker_id": f"m15-ablation-{i}",
            "device": device,
            "execution_batch": exec_batch,
            "job": {
                "job_id": f"abl_{label.replace(':', '_')}",
                "genome_id": vg.genome_id,
                "genome_json": vg.to_json(),
                "seed": seed,
                "backend": (config.get("evaluation") or {})
                    .get("backend", "torch"),
                "environment_id": (config.get("evaluation") or {})
                    .get("environment_id"),
                "duration_ms": (config.get("evaluation") or {})
                    .get("duration_ms", 500),
                "replicates": n_rep,
                "config": config,
                "run_dir": str(out),
                "requested_traces": [],
                "attempt": 0,
            },
        })

    bodies: dict[str, dict] = {}
    t0 = time.time()
    ctx = mp.get_context("spawn")
    with ProcessPoolExecutor(max_workers=procs, mp_context=ctx) as pool:
        for body in pool.map(_run_condition, tasks):
            bodies[body["label"]] = body
            print(f"[m15] {body['label']:<40} {body.get('status')} "
                  f"({body.get('wall_s_total', 0):.0f}s)", flush=True)

    task_by_label = {t["label"]: t for t in tasks}
    results: dict[str, dict] = {}
    for label, body in bodies.items():
        body["_genome_json"] = task_by_label[label]["job"]["genome_json"]
        (out / "results" / f"{label.replace(':', '_')}.json")\
            .write_text(json.dumps(body, default=str))
        if body.get("status") == "SUCCEEDED" and body.get("evaluation"):
            results[label] = _condition_metrics(label, body, config,
                                                target)
        else:
            results[label] = {"label": label, "status": body.get("status"),
                              "error": body.get("error")}

    full = results.get("full") or {}
    comparisons = {}
    for label, res in results.items():
        if label == "full" or not res.get("replicate_rows"):
            continue
        d = _paired_delta(full, res)
        if d is None:
            comparisons[label] = {"paired": None}
            continue
        d["bootstrap"] = _bootstrap_ci(
            d["deltas"], int(th.get("bootstrap_iters", 2000)),
            int(th.get("bootstrap_seed", seed)))
        d["effect_size_dz"] = (d["mean"] / d["std"]) if d["std"] else None
        # selection_score is the quantity elites are ranked on; its delta
        # is the most direct answer to "did removal lower what selection
        # sees", complementing the replicate-paired task-score deltas.
        if (full.get("selection_score") is not None
                and res.get("selection_score") is not None):
            d["selection_score_delta"] = (full["selection_score"]
                                          - res["selection_score"])
        d["classification"] = _classify(d, th)
        comparisons[label] = d

    analysis = {
        "experiment_id": f"M1.5-ablation-{int(t0)}",
        "source_experiment": source_exp,
        "source_genome": gid,
        "source_generation": source_gen,
        "evaluation_seed": seed,
        "replicates": n_rep,
        "target_rate_hz": target,
        "thresholds": th,
        "conditions": results,
        "paired_vs_full": comparisons,
        "wall_s_total": time.time() - t0,
    }
    (out / "analysis.json").write_text(json.dumps(analysis, default=str,
                                                indent=1))
    _write_table(out / "ABLATION_TABLE.md", analysis)
    (out / "manifest.json").write_text(json.dumps({
        "kind": "M1.5 organ ablation (functional validation)",
        "source_experiment": source_exp, "source_genome": gid,
        "config": args.config, "m1_db": args.m1_db,
        "seed": seed, "replicates": n_rep, "conditions": len(variants),
        "note": "paired seeds; source run opened read-only; "
                "variants carry enabled=False, genomes never mutated",
    }, indent=1))
    print(f"[m15] done in {time.time() - t0:.0f}s -> {out}")
    _print_table(analysis)
    return 0


def _write_table(path, analysis):
    lines = ["# M1.5 ablation — %s" % analysis["source_genome"][:24],
             "",
             "| condition | fitness mean | Δfitness (paired) | 95% CI | "
             "consistency | class |",
             "|---|---|---|---|---|---|"]
    full = analysis["conditions"]["full"]
    lines.append("| full | %.4f | — | — | — | — |"
                 % (full.get("fitness_mean") or float("nan")))
    for label, d in analysis["paired_vs_full"].items():
        if not d or "mean" not in d:
            lines.append("| %s | n/a | n/a | n/a | n/a | n/a |" % label)
            continue
        ci = d["bootstrap"]["ci95"]
        lines.append("| %s | %.4f | %+.4f | [%+.4f, %+.4f] | %.2f | %s |"
                     % (label,
                        analysis["conditions"][label].get("fitness_mean")
                        or float("nan"),
                        d["mean"], ci[0], ci[1],
                        d["frac_removal_hurt"] if d["mean"] > 0
                        else d["frac_removal_helped"],
                        d["classification"]))
    path.write_text("\n".join(lines) + "\n")


def _print_table(analysis):
    for label, d in analysis["paired_vs_full"].items():
        if d and "mean" in d:
            print(f"  {label:<40} Δ={d['mean']:+.4f} "
                  f"ci={d['bootstrap']['ci95']} -> {d['classification']}")


if __name__ == "__main__":
    sys.exit(main())
