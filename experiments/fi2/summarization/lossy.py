"""LossySummarizer — a parameterized *model* of LLM summarization failure.

This is not an LLM and does not pretend to be one. It is a seeded,
explicitly-parameterized model of the failure modes reported for
abstractive summarization, so that cascade degradation is *observable*
under controlled conditions. Its output is evidence about how the
context ARCHITECTURE propagates mutations, not about any specific LLM.

Parameters (all probabilities, applied per claim span, seeded):

* ``fact_drop``            — fact deletion: span omitted entirely.
* ``obj_drift``            — paraphrase drift: object/value replaced by a
                             confusable token seen elsewhere in the input.
* ``polarity_flip``        — relationship/polarity inversion at depth 1.
* ``polarity_flip_growth`` — flip probability grows per cascade level:
                             ``p(d) = polarity_flip * (1 + growth * (d-1))``.
* ``uncertainty_norm``     — hedged claims ("may", "unknown") rewritten
                             as definite ("uncertainty loss").
* ``source_loss``          — source pointer dropped (provenance broken).
* ``gist_drop``            — non-claim (filler/transient-text) spans dropped.

Text of retained claim spans is re-rendered from the (possibly mutated)
claim so mutations are visible in the summary text.
"""

from __future__ import annotations

import hashlib
import random
from dataclasses import dataclass
from typing import List, Sequence

from ..store.events import Claim, Provenance, Span
from .base import Summarizer, SummaryArtifact, count_tokens


@dataclass(frozen=True)
class LossyParams:
    fact_drop: float = 0.30
    obj_drift: float = 0.05
    polarity_flip: float = 0.03
    polarity_flip_growth: float = 1.0
    uncertainty_norm: float = 0.50
    source_loss: float = 0.60
    gist_drop: float = 0.90
    confidence_decay: float = 0.9   # per cascade level


def _rng(seed: int, depth: int, spans: Sequence[Span]) -> random.Random:
    """Deterministic RNG keyed on seed, depth and input content."""
    h = hashlib.sha256()
    h.update(f"{seed}:{depth}:".encode())
    for s in spans:
        h.update((s.prov.source_event_id or "-").encode())
        h.update(b"|")
    return random.Random(int.from_bytes(h.digest()[:8], "big"))


class LossySummarizer(Summarizer):
    name = "lossy"

    def __init__(self, params: LossyParams | None = None, seed: int = 0):
        self.params = params or LossyParams()
        self.seed = seed

    def _mutate_claim(
        self, claim: Claim, rng: random.Random, depth: int, obj_pool: List[str],
        applied: List[str],
    ) -> Claim:
        p = self.params
        subj, pred, obj = claim.subject, claim.predicate, claim.obj
        pol, cert = claim.polarity, claim.certainty

        if obj_pool and rng.random() < p.obj_drift:
            obj = rng.choice(obj_pool)
            applied.append("obj_drift")
        flip_p = min(1.0, p.polarity_flip * (1.0 + p.polarity_flip_growth * (depth - 1)))
        if rng.random() < flip_p:
            pol = -pol
            applied.append("polarity_flip")
        if cert != "certain" and rng.random() < p.uncertainty_norm:
            cert = "certain"
            applied.append("uncertainty_norm")

        return Claim(
            subject=subj,
            predicate=pred,
            obj=obj,
            polarity=pol,
            certainty=cert,
            source=claim.source,
            seq=claim.seq,
            supersedes=claim.supersedes,
        )

    def summarize(
        self,
        spans: Sequence[Span],
        budget_tokens: int,
        depth: int = 1,
    ) -> SummaryArtifact:
        p = self.params
        rng = _rng(self.seed, depth, spans)
        applied: List[str] = []
        obj_pool = sorted(
            {s.claim.obj for s in spans if s.claim is not None}
        )

        out: List[Span] = []
        for span in spans:
            if span.claim is None:
                if rng.random() < p.gist_drop:
                    applied.append("gist_drop")
                    continue
                # non-claim spans are kept verbatim (gist lines are not
                # mutated — only claims are), but still counted as derived
                out.append(
                    Span(
                        text=span.text,
                        claim=None,
                        prov=Provenance(
                            derivation_type="summary_lossy",
                            source_event_id=span.prov.source_event_id,
                            derived_from=span.prov.derived_from,
                            turn=span.prov.turn,
                            depth=depth,
                            confidence=span.prov.confidence
                            * p.confidence_decay,
                        ),
                        role="gist",
                    )
                )
                continue

            if rng.random() < p.fact_drop:
                applied.append("fact_drop")
                continue

            claim = self._mutate_claim(span.claim, rng, depth, obj_pool, applied)

            if rng.random() < p.source_loss:
                derived_from: tuple = ()
                source_id = None
                applied.append("source_loss")
            else:
                derived_from = span.prov.derived_from
                source_id = (
                    span.prov.derived_from[0]
                    if span.prov.derived_from
                    else span.prov.source_event_id
                )

            out.append(
                Span(
                    text=claim.render(),
                    claim=claim,
                    prov=Provenance(
                        derivation_type="summary_lossy",
                        source_event_id=source_id,
                        derived_from=derived_from,
                        turn=span.prov.turn,
                        depth=depth,
                        confidence=span.prov.confidence * p.confidence_decay,
                    ),
                )
            )

        # budget enforcement: drop non-claim spans oldest-first, then
        # claim spans oldest-first (deterministic truncation).
        def drop_order(s: Span):
            return (s.claim is not None, s.prov.turn)

        while count_tokens(out) > budget_tokens and out:
            victim = min(range(len(out)), key=lambda i: drop_order(out[i]))
            applied.append("budget_drop")
            out.pop(victim)

        return SummaryArtifact(
            spans=out,
            depth=depth,
            token_count=count_tokens(out),
            applied=applied,
        )
