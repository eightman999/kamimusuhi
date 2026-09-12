"""Recursive summarization cascade driver (Fi2-C6).

Level 0 input is a sequence of source spans. At each level the spans are
grouped into consecutive chunks of ``fan_in`` and each chunk is
summarized at ``depth = level + 1``. The process repeats for ``depth``
levels, so depth-3 output is summary-of-summary-of-summary. Provenance
``derived_from`` always keeps the transitive closure of *source event
ids*, so every surviving claim can in principle be traced to the store.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Sequence

from ..store.events import Span
from .base import Summarizer, SummaryArtifact, count_tokens


@dataclass
class CascadeResult:
    artifact: SummaryArtifact
    levels: List[SummaryArtifact] = field(default_factory=list)


def run_cascade(
    spans: Sequence[Span],
    summarizer: Summarizer,
    depth: int = 3,
    fan_in: int = 8,
    level_budget_tokens: int = 400,
) -> CascadeResult:
    """Summarize ``spans`` through ``depth`` recursive levels."""
    if depth < 1:
        raise ValueError("depth must be >= 1")
    if fan_in < 2:
        raise ValueError("fan_in must be >= 2")

    current = list(spans)
    levels: List[SummaryArtifact] = []
    for level in range(1, depth + 1):
        chunks = [
            current[i : i + fan_in] for i in range(0, len(current), fan_in)
        ]
        merged: List[Span] = []
        applied: List[str] = []
        for chunk in chunks:
            art = summarizer.summarize(
                chunk, budget_tokens=level_budget_tokens, depth=level
            )
            merged.extend(art.spans)
            applied.extend(art.applied)
        merged.sort(key=lambda s: (s.prov.turn, s.prov.source_event_id or ""))
        current = merged
        levels.append(
            SummaryArtifact(
                spans=list(current),
                depth=level,
                token_count=count_tokens(current),
                applied=applied,
            )
        )
    return CascadeResult(artifact=levels[-1], levels=levels)
