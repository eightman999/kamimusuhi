"""Cheap vs gold evaluator agreement (M1 §2.3).

M1 updates generations with a cheaper evaluator (fewer replicates and/or
a shorter episode) and re-runs only the interesting individuals — the
generation champion, anomalies, the final champion, structural
innovations — on the gold evaluator (M0's 500 ms x 8 replicates).

That is only legitimate if the cheap evaluator *orders* individuals the
same way the gold one does. Selection consumes ranks, not absolute
fitness, so the acceptance criteria are rank statistics:

    Spearman rho    >= 0.85
    top-8 overlap   >= 6 of 8

Both are computed here without scipy (ties get average ranks, which is
what makes rho well-defined when several individuals score identically —
exactly the M0 situation where fitness barely moved).

A cheap evaluator that fails these is not used. The decision, its
numbers, and the two evaluator definitions belong in the M1 report.
"""
from __future__ import annotations

import math


def average_ranks(values: list[float]) -> list[float]:
    """Ranks 1..n, ties sharing their average rank."""
    order = sorted(range(len(values)), key=lambda i: values[i])
    ranks = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        shared = (i + j) / 2.0 + 1.0
        for k in range(i, j + 1):
            ranks[order[k]] = shared
        i = j + 1
    return ranks


def pearson(a: list[float], b: list[float]) -> float | None:
    n = len(a)
    if n < 2 or len(b) != n:
        return None
    ma, mb = sum(a) / n, sum(b) / n
    va = sum((x - ma) ** 2 for x in a)
    vb = sum((x - mb) ** 2 for x in b)
    if va <= 0 or vb <= 0:
        return None          # one side is constant: rank order undefined
    cov = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    return cov / math.sqrt(va * vb)


def spearman(a: list[float], b: list[float]) -> float | None:
    """Rank correlation. ``None`` when either side has no spread — which
    is itself a finding (a cheap evaluator that scores everything the
    same cannot select)."""
    if len(a) != len(b) or len(a) < 2:
        return None
    return pearson(average_ranks(a), average_ranks(b))


def top_k_overlap(ids: list[str], gold: list[float], cheap: list[float],
                  k: int) -> dict:
    """How many of the gold top-k the cheap evaluator also puts in its
    top-k. Ties at the boundary are resolved by score then id, so the
    result is deterministic."""
    k = max(1, min(int(k), len(ids)))

    def top(scores):
        order = sorted(range(len(ids)),
                       key=lambda i: (-scores[i], ids[i]))
        return {ids[i] for i in order[:k]}

    g, c = top(gold), top(cheap)
    return {"k": k, "overlap": len(g & c), "gold_top": sorted(g),
            "cheap_top": sorted(c), "fraction": len(g & c) / k}


def agreement_report(ids: list[str], gold: list[float], cheap: list[float],
                     k: int = 8, min_rho: float = 0.85,
                     min_overlap: int = 6) -> dict:
    """Full §2.3 acceptance decision for one cheap-evaluator candidate."""
    rho = spearman(gold, cheap)
    ov = top_k_overlap(ids, gold, cheap, k)
    # an overlap threshold stated as "6 of 8" scales with a smaller sample
    required_overlap = min(int(min_overlap), ov["k"])
    accepted = (rho is not None and rho >= min_rho
                and ov["overlap"] >= required_overlap)
    reasons = []
    if rho is None:
        reasons.append("no rank spread on at least one evaluator")
    elif rho < min_rho:
        reasons.append(f"spearman {rho:.3f} < {min_rho}")
    if ov["overlap"] < required_overlap:
        reasons.append(f"top-{ov['k']} overlap {ov['overlap']} "
                       f"< {required_overlap}")
    return {
        "n": len(ids),
        "spearman_rho": (None if rho is None else round(rho, 4)),
        "top_k": ov,
        "criteria": {"min_spearman_rho": min_rho,
                     "min_top_k_overlap": required_overlap, "k": ov["k"]},
        "accepted": bool(accepted),
        "rejected_because": reasons,
    }


def compare_evaluators(genomes, config: dict, device: str,
                       gold: dict, cheap: dict, score,
                       execution_batch: int = 1, data_dir: str | None = None,
                       k: int = 8, min_rho: float = 0.85,
                       min_overlap: int = 6) -> dict:
    """Evaluate the same genomes under two evaluator settings and report
    whether the cheap one may replace the gold one for generation updates.

    ``gold`` / ``cheap`` are ``{"duration_ms": float, "replicates": int}``.
    ``score(summary) -> float`` is the selection value; pass the
    experiment's own so the check measures the ranking that selection
    actually uses.
    """
    from .evalbench import run_one

    def evaluate(setting: dict) -> tuple[list[str], list[float], list[dict]]:
        ids, scores, rows = [], [], []
        for i, g in enumerate(genomes):
            r = run_one(g, config, device, execution_batch=execution_batch,
                        seed=g.random_seed or i,
                        duration_ms=setting.get("duration_ms"),
                        replicates=setting.get("replicates"),
                        data_dir=data_dir, profile=False)
            ids.append(g.genome_id)
            scores.append(float("-inf") if not r["ok"]
                          else float(score(r["summary"])))
            rows.append({"genome_id": g.genome_id, "ok": r["ok"],
                         "wall_s": round(r["wall_s"], 4),
                         "score": scores[-1],
                         "mean_rate_hz": (r["summary"] or {}).get(
                             "mean_rate_hz")})
        return ids, scores, rows

    ids, gold_scores, gold_rows = evaluate(gold)
    _, cheap_scores, cheap_rows = evaluate(cheap)
    gold_wall = sum(r["wall_s"] for r in gold_rows)
    cheap_wall = sum(r["wall_s"] for r in cheap_rows)
    report = agreement_report(ids, gold_scores, cheap_scores, k=k,
                              min_rho=min_rho, min_overlap=min_overlap)
    report.update({
        "device": device,
        "gold": dict(gold), "cheap": dict(cheap),
        "gold_wall_s": round(gold_wall, 3),
        "cheap_wall_s": round(cheap_wall, 3),
        "speedup": (round(gold_wall / cheap_wall, 3) if cheap_wall > 0
                    else None),
        "gold_rows": gold_rows, "cheap_rows": cheap_rows,
    })
    return report
