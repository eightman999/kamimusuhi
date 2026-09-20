"""Summarizer protocol for Fi2.

There is no LLM in this experiment. A summarizer is a pluggable,
deterministic function ``spans -> SummaryArtifact`` that models the
information-preservation behaviour of summarization. The output is
always a *derived artifact*: a list of :class:`Span` objects whose
provenance points back into the immutable store. Summaries are never
written to the store and never replace it (prohibition: summary is
never the only ground truth).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Protocol, Sequence

from ..store.events import Span


@dataclass
class SummaryArtifact:
    """Result of one summarize() call."""

    spans: List[Span]
    depth: int
    token_count: int
    # bookkeeping: which mutation operators fired, for tests/debugging
    applied: List[str] = field(default_factory=list)

    def render_text(self) -> str:
        return "\n".join(s.text for s in self.spans)


class Summarizer(Protocol):
    """Pluggable summarization contract."""

    name: str

    def summarize(
        self,
        spans: Sequence[Span],
        budget_tokens: int,
        depth: int = 1,
    ) -> SummaryArtifact:
        ...


def count_tokens(spans: Sequence[Span]) -> int:
    return sum(s.token_count() for s in spans)
