"""Builders for conditions B0–B5.

All builders read from the immutable store / index / summarizer and emit
a :class:`ContextBundle`. None of them writes to the store — the
prohibition test asserts the store head digest is identical before and
after building every condition.

* B0 Full History           — every event, unbudgeted (upper baseline).
* B1 Rolling Window         — last ``window_turns`` turns, budgeted.
* B2 Recursive Summary      — cascade-summarized history + recent window.
* B3 Retrieval Only         — BM25 over source events, per question.
* B4 Summary + Retrieval    — retrieved source events + cascade summary.
* B5 Source-preserving Mem. — structured claims + resolved source text.
"""

from __future__ import annotations

from typing import Dict, List, Optional, Sequence

from ..index.inverted import InvertedIndex
from ..retrieval.retriever import BM25Retriever, ScoredSpan
from ..store.events import Event, Provenance, Span
from ..store.immutable_store import ImmutableEventStore
from ..summarization.base import Summarizer
from ..summarization.cascade import run_cascade
from .bundle import ContextBundle

CONDITIONS = ("B0", "B1", "B2", "B3", "B4", "B5")

# Trust tiers used by the answer resolver: verbatim source / memory
# projections outrank derived (possibly lossy) summary claims when they
# disagree — a reader with source access trusts the source.
TRUST = {
    "source": 2,
    "source_excerpt": 2,
    "memory_claim": 2,
    "summary_extractive": 2,
    "summary_lossy": 1,
}


def _pack(spans: Sequence[Span], budget: int) -> tuple[List[Span], bool]:
    """Greedily keep spans in order until the token budget is exhausted."""
    out: List[Span] = []
    used = 0
    truncated = False
    for s in spans:
        c = s.token_count()
        if used + c > budget:
            truncated = True
            continue
        out.append(s)
        used += c
    return out, truncated


class ConditionRunner:
    """Holds shared state (store, index, retriever, cached summaries)."""

    def __init__(
        self,
        store: ImmutableEventStore,
        index: InvertedIndex,
        summarizer: Summarizer,
        window_turns: int = 100,
        recent_keep: int = 30,
        cascade_depth: int = 3,
        cascade_fan_in: int = 8,
        cascade_level_budget: int = 400,
        retrieve_k: int = 8,
    ):
        self.store = store
        self.index = index
        self.retriever = BM25Retriever(index)
        self.summarizer = summarizer
        self.window_turns = window_turns
        self.recent_keep = recent_keep
        self.cascade_depth = cascade_depth
        self.cascade_fan_in = cascade_fan_in
        self.cascade_level_budget = cascade_level_budget
        self.retrieve_k = retrieve_k
        self._summary_cache: Optional[List[Span]] = None
        self.retrieval_calls = 0  # instrumented by evaluate for latency

    # -- shared pieces ------------------------------------------------------

    def all_spans(self) -> List[Span]:
        return [Span.from_event(e) for e in self.store.events()]

    def summary_spans(self) -> List[Span]:
        """Cascade summary of all but the most recent turns (cached)."""
        if self._summary_cache is None:
            events = self.store.events()
            max_turn = events[-1].turn if events else 0
            old = [
                Span.from_event(e)
                for e in events
                if e.turn <= max_turn - self.recent_keep
            ]
            res = run_cascade(
                old,
                self.summarizer,
                depth=self.cascade_depth,
                fan_in=self.cascade_fan_in,
                level_budget_tokens=self.cascade_level_budget,
            )
            self._summary_cache = res.artifact.spans
        return list(self._summary_cache)

    def recent_spans(self) -> List[Span]:
        events = self.store.events()
        if not events:
            return []
        max_turn = events[-1].turn
        return [
            Span.from_event(e)
            for e in events
            if e.turn > max_turn - self.recent_keep
        ]

    def retrieve(self, query_tokens: Sequence[str], k: Optional[int] = None):
        return self.retriever.search(
            query_tokens, k=k or self.retrieve_k
        )

    # -- conditions ----------------------------------------------------------

    def build_b0(self, query_tokens=None) -> ContextBundle:
        spans = self.all_spans()
        return ContextBundle(
            condition="B0",
            spans=spans,
            budget=-1,
            token_count=sum(s.token_count() for s in spans),
            truncated=False,
        )

    def build_b1(self, query_tokens=None, budget: int = 10**9) -> ContextBundle:
        events = self.store.events()
        max_turn = events[-1].turn if events else 0
        spans = [
            Span.from_event(e)
            for e in events
            if e.turn > max_turn - self.window_turns
        ]
        # keep the MOST RECENT spans under budget (drop from the front)
        kept: List[Span] = []
        used = 0
        truncated = False
        for s in reversed(spans):
            c = s.token_count()
            if used + c > budget:
                truncated = True
                break
            kept.append(s)
            used += c
        kept.reverse()
        return ContextBundle(
            condition="B1", spans=kept, budget=budget,
            token_count=used, truncated=truncated,
        )

    def build_b2(self, query_tokens=None, budget: int = 10**9) -> ContextBundle:
        spans = self.summary_spans() + self.recent_spans()
        packed, truncated = _pack(spans, budget)
        return ContextBundle(
            condition="B2", spans=packed, budget=budget,
            token_count=sum(s.token_count() for s in packed),
            truncated=truncated,
        )

    def build_b3(self, query_tokens, budget: int = 10**9) -> ContextBundle:
        hits = self.retrieve(query_tokens)
        spans = [h.span for h in hits]
        packed, truncated = _pack(spans, budget)
        return ContextBundle(
            condition="B3", spans=packed, budget=budget,
            token_count=sum(s.token_count() for s in packed),
            truncated=truncated,
            meta={"n_retrieved": len(hits)},
        )

    def build_b4(self, query_tokens, budget: int = 10**9) -> ContextBundle:
        hits = self.retrieve(query_tokens)
        retrieved = [h.span for h in hits]
        seen = {s.prov.source_event_id for s in retrieved}
        summary = [
            s for s in self.summary_spans()
            if s.prov.source_event_id not in seen
        ]
        # retrieved sources first (query-relevant, verbatim), then summary
        packed, truncated = _pack(retrieved + summary, budget)
        return ContextBundle(
            condition="B4", spans=packed, budget=budget,
            token_count=sum(s.token_count() for s in packed),
            truncated=truncated,
            meta={"n_retrieved": len(hits)},
        )

    def build_b5(self, query_tokens, budget: int = 10**9) -> ContextBundle:
        """Structured claim memory + resolved source excerpts.

        Memory claims are verbatim projections of the store (lossless,
        fully provenanced). For each matched claim the original source
        event text is attached as a ``source_excerpt`` span while budget
        remains — the original text is always reachable.
        """
        idx = self.index
        matched_ids: List[str] = []
        seen_ids = set()
        # entity matches first (high precision), then token matches
        qset = set(query_tokens)
        entity_hits = [
            eid for ent, eids in idx.entities.items()
            if ent in qset for eid in eids
        ]
        scored = self.retrieve(query_tokens, k=self.retrieve_k)
        for eid in entity_hits + [h.span.prov.source_event_id for h in scored]:
            if eid and eid not in seen_ids:
                seen_ids.add(eid)
                matched_ids.append(eid)

        claims: List[Span] = []
        excerpts: List[Span] = []
        for eid in matched_ids:
            ev = idx.by_id[eid]
            if ev.claim is not None:
                claims.append(
                    Span(
                        text=ev.claim.render(),
                        claim=ev.claim,
                        prov=Provenance(
                            derivation_type="memory_claim",
                            source_event_id=ev.event_id,
                            derived_from=(ev.event_id,),
                            turn=ev.turn,
                            confidence=1.0,
                        ),
                    )
                )
            excerpts.append(
                Span(
                    text=f"[source {ev.event_id} t{ev.turn}] {ev.text}",
                    claim=ev.claim,
                    prov=Provenance(
                        derivation_type="source_excerpt",
                        source_event_id=ev.event_id,
                        derived_from=(ev.event_id,),
                        turn=ev.turn,
                        confidence=1.0,
                    ),
                    role="source_excerpt",
                )
            )
        packed_claims, _ = _pack(claims, budget)
        packed, truncated = _pack(packed_claims + excerpts, budget)
        return ContextBundle(
            condition="B5", spans=packed, budget=budget,
            token_count=sum(s.token_count() for s in packed),
            truncated=truncated,
            meta={"n_matched": len(matched_ids)},
        )

    def build(
        self,
        condition: str,
        query_tokens: Optional[Sequence[str]] = None,
        budget: int = 10**9,
    ) -> ContextBundle:
        fn = {
            "B0": self.build_b0,
            "B1": self.build_b1,
            "B2": self.build_b2,
            "B3": self.build_b3,
            "B4": self.build_b4,
            "B5": self.build_b5,
        }[condition]
        if condition in ("B0",):
            return fn()
        return fn(query_tokens, budget=budget)
