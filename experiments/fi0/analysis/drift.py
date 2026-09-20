"""Decision-distribution drift metrics.

For each task we collect, per variant, the distribution over answer
options (across reps), plus the pooled distribution across all variants.

* ``choice_distribution`` — counts per option
* ``js_divergence``       — Jensen-Shannon divergence (base-2, in [0,1])
* ``kl_divergence``       — KL(p || q) with epsilon smoothing
* ``drift_vs_pooled``     — mean / max JS of each variant vs pooled
* ``semantic_consistency``— fraction of runs whose choice == expected
* ``persona_consistency`` — fraction of runs containing the task marker
"""

from __future__ import annotations

import math
from collections import Counter, defaultdict
from typing import Dict, Iterable, List, Mapping, Sequence

Row = Mapping[str, object]
_EPS = 1e-12


def distribution(choices: Sequence[str], options: Sequence[str]) -> List[float]:
    c = Counter(choices)
    n = len(choices) or 1
    return [c.get(o, 0) / n for o in options]


def kl_divergence(p: Sequence[float], q: Sequence[float]) -> float:
    """KL(p || q), natural log, epsilon-smoothed."""
    return sum(
        pi * math.log(pi / max(qi, _EPS))
        for pi, qi in zip(p, q)
        if pi > 0
    )


def js_divergence(p: Sequence[float], q: Sequence[float]) -> float:
    """Jensen-Shannon divergence, base 2, bounded in [0, 1]."""
    m = [(pi + qi) / 2 for pi, qi in zip(p, q)]
    return 0.5 * _kl_base2(p, m) + 0.5 * _kl_base2(q, m)


def _kl_base2(p: Sequence[float], q: Sequence[float]) -> float:
    return sum(
        pi * math.log2(pi / max(qi, _EPS))
        for pi, qi in zip(p, q)
        if pi > 0
    )


def entropy(p: Sequence[float]) -> float:
    return -sum(pi * math.log2(pi) for pi in p if pi > 0)


def per_task_drift(
    rows: Iterable[Row],
    options: Sequence[str],
) -> Dict[str, object]:
    """Drift stats for the rows of one task."""
    rows = list(rows)
    by_variant: Dict[str, List[str]] = defaultdict(list)
    for r in rows:
        by_variant[str(r["variant"])].append(str(r["choice"]))
    pooled = [str(r["choice"]) for r in rows]

    dists = {
        v: distribution(cs, options) for v, cs in by_variant.items()
    }
    pooled_dist = distribution(pooled, options)
    js_vals = [js_divergence(d, pooled_dist) for d in dists.values()]

    return {
        "n_runs": len(rows),
        "n_variants": len(by_variant),
        "pooled_distribution": dict(zip(options, pooled_dist)),
        "pooled_entropy_bits": entropy(pooled_dist),
        "js_mean": sum(js_vals) / len(js_vals) if js_vals else 0.0,
        "js_max": max(js_vals) if js_vals else 0.0,
        "n_distinct_choices": len(set(pooled)),
        "choices": dict(Counter(pooled)),
    }


def consistency(rows: Iterable[Row], expected: str, marker: str) -> Dict[str, float]:
    """Semantic + persona consistency over runs."""
    rows = list(rows)
    n = len(rows) or 1
    sem = sum(1 for r in rows if r["choice"] == expected) / n
    per = sum(1 for r in rows if r.get("marker_present")) / n
    return {"semantic": sem, "persona": per}
