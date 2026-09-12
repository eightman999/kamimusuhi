"""ExtractiveSummarizer — deterministic near-lossless control.

Selects whole spans verbatim; never mutates claim content, certainty or
provenance. Under a token budget it keeps every claim-bearing span first
(claims are compact and information-dense), then fills remaining budget
with the most recent non-claim spans. This is the control arm that lets
us attribute observed degradation to the lossy summarization model
rather than to the compression architecture itself.
"""

from __future__ import annotations

from typing import List, Sequence

from ..store.events import Provenance, Span
from .base import Summarizer, SummaryArtifact, count_tokens


class ExtractiveSummarizer(Summarizer):
    name = "extractive"

    def __init__(self, seed: int = 0):
        self.seed = seed  # unused; kept for interface symmetry

    def _derive(self, span: Span, depth: int) -> Span:
        return Span(
            text=span.text,
            claim=span.claim,
            prov=Provenance(
                derivation_type="summary_extractive",
                source_event_id=span.prov.source_event_id,
                derived_from=span.prov.derived_from,
                turn=span.prov.turn,
                depth=depth,
                confidence=span.prov.confidence,
            ),
            role=span.role,
        )

    def summarize(
        self,
        spans: Sequence[Span],
        budget_tokens: int,
        depth: int = 1,
    ) -> SummaryArtifact:
        claims = [s for s in spans if s.claim is not None]
        others = sorted(
            (s for s in spans if s.claim is None),
            key=lambda s: (-s.prov.turn, s.prov.source_event_id or ""),
        )
        out: List[Span] = []
        used = 0
        for s in claims:
            c = s.token_count()
            if used + c > budget_tokens:
                break
            out.append(self._derive(s, depth))
            used += c
        for s in others:
            c = s.token_count()
            if used + c > budget_tokens:
                break
            out.append(self._derive(s, depth))
            used += c
        # restore input order (by original turn, then source id) for output
        out.sort(key=lambda s: (s.prov.turn, s.prov.source_event_id or ""))
        return SummaryArtifact(
            spans=out, depth=depth, token_count=count_tokens(out)
        )
