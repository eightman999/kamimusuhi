"""Deterministic inverted index over the immutable event store.

The index is built ONLY from :class:`Event` fields (id, turn, kind, text,
claim, entities). It never sees dataset labels, QA pairs or answer keys —
its constructor simply accepts ``Iterable[Event]``, so there is nowhere
to put them. The prohibition test re-builds the index while permuting
all evaluation metadata and asserts byte-identical postings.
"""

from __future__ import annotations

import hashlib
import json
from collections import Counter
from typing import Dict, Iterable, List, Tuple

from ..store.events import Event


class InvertedIndex:
    """token → postings index with BM25 statistics, plus field lookups."""

    def __init__(self):
        self.postings: Dict[str, List[str]] = {}
        self.tf: Dict[str, Counter] = {}
        self.doc_len: Dict[str, int] = {}
        self.entities: Dict[str, List[str]] = {}
        self.by_id: Dict[str, Event] = {}
        self.order: List[str] = []
        self.n_docs = 0
        self.avgdl = 0.0

    # -- construction ------------------------------------------------------

    @classmethod
    def build(cls, events: Iterable[Event]) -> "InvertedIndex":
        idx = cls()
        total_len = 0
        for ev in events:
            eid = ev.event_id
            idx.order.append(eid)
            idx.by_id[eid] = ev
            counts = Counter(ev.tokens())
            idx.tf[eid] = counts
            idx.doc_len[eid] = sum(counts.values())
            total_len += idx.doc_len[eid]
            for tok in counts:
                idx.postings.setdefault(tok, []).append(eid)
            for ent in ev.entities:
                idx.entities.setdefault(ent, []).append(eid)
        # deterministic postings order: by turn then id
        turn_of = {eid: idx.by_id[eid].turn for eid in idx.order}
        for lst in idx.postings.values():
            lst.sort(key=lambda eid: (turn_of[eid], eid))
        for lst in idx.entities.values():
            lst.sort(key=lambda eid: (turn_of[eid], eid))
        idx.n_docs = len(idx.order)
        idx.avgdl = (total_len / idx.n_docs) if idx.n_docs else 0.0
        return idx

    # -- stats --------------------------------------------------------------

    def df(self, token: str) -> int:
        return len(self.postings.get(token, ()))

    def digest(self) -> str:
        """Content digest of postings + doc stats (for tamper/leak tests)."""
        payload = {
            "postings": {k: v for k, v in sorted(self.postings.items())},
            "doc_len": self.doc_len,
            "entities": self.entities,
        }
        return hashlib.sha256(
            json.dumps(payload, sort_keys=True).encode()
        ).hexdigest()
