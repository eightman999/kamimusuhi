"""Agreement metrics over sweep rows.

Each sweep row carries (task, variant, rep, choice).  Within one task,
serializations of the *same* semantic input should yield the same choice
if behaviour is stable.

* ``exact_agreement``   — 1.0 iff every run of the task chose identically
* ``pairwise_agreement``— mean over unordered variant pairs of
                          P(same choice), computed over per-variant
                          majority choices (reps are deterministic for the
                          bundled clients; majority is the honest summary)
* ``variant_agreement_matrix`` — per-pair agreement for the report
"""

from __future__ import annotations

from collections import Counter, defaultdict
from itertools import combinations
from typing import Dict, Iterable, List, Mapping, Sequence, Tuple

Row = Mapping[str, object]


def majority_choice(choices: Sequence[str]) -> str:
    """Most common choice; ties broken by option order (lexicographic)."""
    c = Counter(choices)
    best_n = max(c.values())
    return sorted(k for k, v in c.items() if v == best_n)[0]


def choices_by_variant(rows: Iterable[Row]) -> Dict[str, str]:
    """variant name -> majority choice across reps."""
    grouped: Dict[str, List[str]] = defaultdict(list)
    for r in rows:
        grouped[str(r["variant"])].append(str(r["choice"]))
    return {v: majority_choice(cs) for v, cs in grouped.items()}


def pairwise_agreement(by_variant: Mapping[str, str]) -> float:
    """Mean fraction of variant pairs whose majority choices agree."""
    items = list(by_variant.items())
    if len(items) < 2:
        return 1.0
    agree = sum(
        1 for (_, a), (_, b) in combinations(items, 2) if a == b
    )
    total = len(items) * (len(items) - 1) // 2
    return agree / total


def exact_agreement(choices: Sequence[str]) -> float:
    """1.0 iff all choices identical, else 0."""
    return 1.0 if len(set(choices)) <= 1 else 0.0


def agreement_matrix(by_variant: Mapping[str, str]) -> List[Tuple[str, str, int]]:
    """Sorted [(variant_a, variant_b, agree?)] for the report."""
    items = sorted(by_variant.items())
    return [
        (a, b, int(ca == cb))
        for (a, ca), (b, cb) in combinations(items, 2)
    ]
