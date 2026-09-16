"""M1.5 offline diagnostics over an existing M1 run's stored summaries.

Two analyses, both read-only over ``lineage.sqlite`` — no re-simulation:

``--landscape``   (§11) Re-scores every stored evaluation at candidate
                  target rates. The per-slice ``rate_hz`` trace persisted
                  in each summary is the only measurement needed, so this
                  answers "which target makes mutation-visible gradient"
                  without running anything.

``--hyperactive`` (§7) Quantifies the runaway phenotypes: which of their
                  measurements predict wall time, and whether the cost
                  explosion tracks *artificial* edge events specifically.

Usage:
    python -m experiments.mioba.scripts.m15_diagnostics \
        --m1-db experiments/mioba/.runs/<run>/lineage.sqlite --out <dir>
"""
from __future__ import annotations

import argparse
import json
import math
import sqlite3
import sys
from pathlib import Path


def _summaries(db_path: str):
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    rows = db.execute(
        """select e.evaluation_id, e.genome_id, g.generation, e.fitness,
                  e.summary_json
           from evaluations e join genomes g
             on g.genome_id = e.genome_id
            and g.experiment_id = e.experiment_id
           where e.summary_json is not null""").fetchall()
    for eid, gid, gen, fit, sj in rows:
        yield eid, gid, gen, fit, json.loads(sj)


def _task_score_at(trace: list[dict], target: float,
                   clean_only: bool) -> float | None:
    vals = [s.get("rate_hz") for s in trace
            if s.get("rate_hz") is not None
            and (not clean_only or not s.get("disturbed"))]
    if not vals:
        return None
    return sum(-abs(r - target) for r in vals) / len(vals)


def landscape(db_path: str, targets: list[float]) -> dict:
    """§11: population score variance + gradient visibility per target."""
    per_gen = {}
    for eid, gid, gen, fit, s in _summaries(db_path):
        lanes = ((s.get("episode") or {}).get("per_lane")) or []
        rates = [s.get("mean_rate_hz")]
        rec = per_gen.setdefault(gen, {"rates": [], "scores": {}})
        rec["rates"].extend(r for r in rates if r is not None)
        for t in targets:
            lane_scores = [_task_score_at((l.get("trace") or []), t,
                                          clean_only=False)
                           for l in lanes]
            lane_scores = [x for x in lane_scores if x is not None]
            if lane_scores:
                # replicate mean like episode_metrics' task_score_all
                sc = sum(lane_scores) / len(lane_scores)
                rec["scores"].setdefault(t, []).append(
                    max(0.0, 1.0 + sc / t))  # task_quality at this target
    out = {"targets": targets, "per_generation": {}, "overall": {}}
    all_rates = []
    for gen in sorted(per_gen):
        rec = per_gen[gen]
        all_rates.extend(rec["rates"])
        row = {"n": len(rec["rates"]),
               "rate_mean": _mean(rec["rates"]),
               "rate_max": max(rec["rates"], default=None),
               "rate_min": min(rec["rates"], default=None)}
        for t in targets:
            q = rec["scores"].get(t) or []
            row[f"quality@{t}"] = {
                "mean": _mean(q),
                "std": _std(q),
                "min": min(q, default=None), "max": max(q, default=None),
            }
        out["per_generation"][str(gen)] = row
    all_rates.sort()
    n = len(all_rates)
    out["overall"]["reachable_rate_percentiles"] = {
        "p50": all_rates[n // 2] if n else None,
        "p90": all_rates[int(0.9 * n)] if n else None,
        "p99": all_rates[int(0.99 * n)] if n else None,
        "max": all_rates[-1] if n else None,
    }
    # per-target population spread pooled across generations: the spread
    # is what selection can see
    pooled = {}
    for t in targets:
        qs = [v for gen in per_gen.values()
              for v in (gen["scores"].get(t) or [])]
        pooled[str(t)] = {"mean": _mean(qs), "std": _std(qs),
                          "min": min(qs, default=None),
                          "max": max(qs, default=None),
                          "spread": (max(qs) - min(qs)) if qs else None}
    out["overall"]["task_quality_pooled"] = pooled
    return out


def _mean(v):
    v = [x for x in v if x is not None]
    return sum(v) / len(v) if v else None


def _std(v):
    v = [x for x in v if x is not None]
    if len(v) < 2:
        return 0.0
    m = sum(v) / len(v)
    return math.sqrt(sum((x - m) ** 2 for x in v) / (len(v) - 1))


def _corr(xs, ys):
    pairs = [(x, y) for x, y in zip(xs, ys)
             if x is not None and y is not None]
    if len(pairs) < 3:
        return None
    xs, ys = zip(*pairs)
    mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
    num = sum((x - mx) * (y - my) for x, y in pairs)
    dx = math.sqrt(sum((x - mx) ** 2 for x in xs))
    dy = math.sqrt(sum((y - my) ** 2 for y in ys))
    return num / (dx * dy) if dx and dy else None


def hyperactive(db_path: str, worst_fitness: float = -100.0,
                min_cases: int = 3) -> dict:
    """§7: quantify runaway phenotypes + what predicts wall time."""
    cases, xs = [], {"wall": [], "edges": [], "edges_art": [],
                     "spikes": [], "organs": [], "rate": []}
    for eid, gid, gen, fit, s in _summaries(db_path):
        res = s.get("resource") or {}
        act = s.get("activity") or {}
        circuit = s.get("circuit") or {}
        wall = s.get("wall_s")
        edges = res.get("active_propagated_edges")
        edges_art = res.get("active_propagated_edges_artificial")
        spikes = s.get("spikes_total")
        rate = s.get("mean_rate_hz")
        n_org = circuit.get("n_artificial_organs")
        xs["wall"].append(wall)
        xs["edges"].append(edges)
        xs["edges_art"].append(edges_art)
        xs["spikes"].append(spikes)
        xs["organs"].append(n_org)
        xs["rate"].append(rate)
        if fit is not None and fit <= worst_fitness:
            org_rates = {k: max(v) for k, v in act.items()
                         if k.startswith("organ:")}
            cases.append({
                "evaluation_id": eid, "genome_id": gid, "generation": gen,
                "fitness": fit,
                "task_score_all": _mean([
                    l.get("task_score_all")
                    for l in ((s.get("episode") or {})
                              .get("per_lane") or [])]),
                "substrate_rate_hz": _mean(act.get("fba0") or []),
                "organ_max_rate_hz": (max(org_rates.values())
                                      if org_rates else None),
                "organ_rates": org_rates,
                "spikes_total": spikes,
                "active_propagated_edges": edges,
                "active_propagated_edges_artificial": edges_art,
                "marginal_compute_cost":
                    res.get("marginal_compute_cost"),
                "wall_s": wall,
                "n_artificial_organs": n_org,
                "n_artificial_neurons":
                    circuit.get("n_artificial_neurons"),
                "simulated_ms": ((s.get("episode") or {})
                                 .get("simulated_ms")),
            })
    cases.sort(key=lambda c: -(c["wall_s"] or 0))
    corr = {k: _corr(xs[k], xs["wall"]) for k in xs if k != "wall"}
    return {
        "n_runaway_cases": len(cases),
        "cases": cases[:max(min_cases, 10)],
        "wall_time_correlations": corr,
        "hypothesis_check": {
            "edges_vs_wall": corr.get("edges"),
            "artificial_edges_vs_wall": corr.get("edges_art"),
            "rate_vs_wall": corr.get("rate"),
        },
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--m1-db", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--targets", default="0.5,1,2,5")
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    targets = [float(t) for t in args.targets.split(",")]
    land = landscape(args.m1_db, targets)
    (out / "landscape.json").write_text(json.dumps(land, indent=1))
    print("=== task-quality spread per target (pooled over all evals) ===")
    for t in targets:
        p = land["overall"]["task_quality_pooled"][str(t)]
        print(f"  target {t:>4}Hz  mean={p['mean']:.3f} std={p['std']:.3f} "
              f"spread=[{p['min']:.3f} .. {p['max']:.3f}]")
    pct = land["overall"]["reachable_rate_percentiles"]
    print(f"  reachable rates: p50={pct['p50']:.3f} p90={pct['p90']:.3f} "
          f"p99={pct['p99']:.3f} max={pct['max']:.3f} Hz")

    hyper = hyperactive(args.m1_db)
    (out / "hyperactive.json").write_text(json.dumps(hyper, indent=1))
    print("\n=== runaway cases (top by wall) ===")
    for c in hyper["cases"][:8]:
        print(f"  gen{c['generation']:>2} fit={c['fitness']:>9.1f} "
              f"wall={c['wall_s']:>7.0f}s edges={c['active_propagated_edges']} "
              f"edges_art={c['active_propagated_edges_artificial']} "
              f"organ_max={c['organ_max_rate_hz']}")
    print("wall-time correlations:", {
        k: (round(v, 3) if v is not None else None)
        for k, v in hyper["wall_time_correlations"].items()})
    return 0


if __name__ == "__main__":
    sys.exit(main())
