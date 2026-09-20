"""Provenance-preserving BM25-ish retriever over the inverted index.

Fully deterministic: scoring is a closed-form function of the index, and
ties break by ``(turn, event_id)``. The retriever's only inputs are the
index and the query — it has no access to relevance labels or answer
keys (prohibition: 評価answerをindex metadataに埋めない / future
relevance labelをretrieval時に使用しない; both are enforced structurally
and exercised by negative tests).
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Iterable, List, Sequence

from ..index.inverted import InvertedIndex
from ..store.events import Span, tokenize


@dataclass(frozen=True)
class ScoredSpan:
    span: Span
    score: float


class BM25Retriever:
    def __init__(self, index: InvertedIndex, k1: float = 1.5, b: float = 0.75):
        self.index = index
        self.k1 = k1
        self.b = b

    def _idf(self, df: int) -> float:
        # BM25 idf, clamped non-negative for determinism-friendly scores
        n = self.index.n_docs
        return math.log(1.0 + (n - df + 0.5) / (df + 0.5))

    def score(self, query_tokens: Sequence[str], event_id: str) -> float:
        idx = self.index
        dl = idx.doc_len[event_id]
        avgdl = idx.avgdl or 1.0
        s = 0.0
        for tok in set(query_tokens):
            tf = idx.tf[event_id].get(tok, 0)
            if tf == 0:
                continue
            idf = self._idf(idx.df(tok))
            s += idf * (tf * (self.k1 + 1.0)) / (
                tf + self.k1 * (1.0 - self.b + self.b * dl / avgdl)
            )
        return s

    def search(
        self, query: str | Iterable[str], k: int = 5
    ) -> List[ScoredSpan]:
        """Rank events by BM25-ish overlap with the query."""
        tokens = tokenize(query) if isinstance(query, str) else list(query)
        candidates = set()
        for tok in set(tokens):
            candidates.update(self.index.postings.get(tok, ()))
        scored = [
            (self.score(tokens, eid), eid)
            for eid in candidates
        ]
        # deterministic order: score desc, then turn asc, then id asc
        scored.sort(key=lambda x: (-x[0], self.index.by_id[x[1]].turn, x[1]))
        out: List[ScoredSpan] = []
        for s, eid in scored:
            if s <= 0.0:
                break
            ev = self.index.by_id[eid]
            out.append(ScoredSpan(span=Span.from_event(ev), score=s))
            if len(out) >= k:
                break
        return out
