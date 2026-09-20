"""Prefix-cacheability metrics.

KV-cache reuse depends on sharing the longest possible prompt prefix.
For a set of variant prompts over the same semantics we measure the
mean pairwise longest-common-prefix (LCP) ratio, and contrast the
cache-friendly order (``cache_max`` + metadata suffix) against the
scattered one.
"""

from __future__ import annotations

import os
from itertools import combinations
from typing import Dict, Iterable, List, Mapping, Sequence, Tuple

Row = Mapping[str, object]


def lcp_len(a: str, b: str) -> int:
    """Length of the longest common byte-prefix of two strings."""
    n = 0
    for ca, cb in zip(a.encode("utf-8"), b.encode("utf-8")):
        if ca != cb:
            break
        n += 1
    return n


def lcp_ratio(a: str, b: str) -> float:
    """LCP length / max(len(a), len(b)) in bytes; 1.0 iff identical."""
    la, lb = len(a.encode("utf-8")), len(b.encode("utf-8"))
    if max(la, lb) == 0:
        return 1.0
    return lcp_len(a, b) / max(la, lb)


def pairwise_lcp(prompts: Sequence[str]) -> float:
    """Mean LCP ratio over all unordered pairs (deduped)."""
    uniq = sorted(set(prompts))
    if len(uniq) < 2:
        return 1.0
    vals = [lcp_ratio(a, b) for a, b in combinations(uniq, 2)]
    return sum(vals) / len(vals)


def cacheability_by_group(
    prompts: Mapping[str, str],
) -> Dict[str, object]:
    """Split the LCP picture by format group.

    ``prompts``: variant_name -> prompt text for one task.
    Returns mean pairwise LCP overall, and within each format (different
    formats diverge at byte ~0 by construction — that contrast is itself
    the finding), plus cache_max-vs-scattered comparison.
    """
    groups: Dict[str, List[str]] = {}
    for name, text in prompts.items():
        fmt = name.split("|", 1)[0]
        groups.setdefault(fmt, []).append(text)

    within = {g: pairwise_lcp(ts) for g, ts in groups.items()}

    cache_max = [t for n, t in prompts.items() if "|cache_max|" in n]
    scattered = [t for n, t in prompts.items() if "|cache_scattered|" in n]
    return {
        "overall_mean_lcp_ratio": pairwise_lcp(list(prompts.values())),
        "within_format_mean_lcp_ratio": within,
        "cache_max_mean_lcp_ratio": pairwise_lcp(cache_max) if cache_max else None,
        "scattered_mean_lcp_ratio": pairwise_lcp(scattered) if scattered else None,
    }
