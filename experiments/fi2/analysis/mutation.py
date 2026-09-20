"""Mutation detectors.

Compare each claim appearing in a context bundle against the source
claims it is derived from (via provenance) or, when the pointer is lost,
against every claim in the store. A claim that canonically matches a
source claim is *faithful*; anything else is classified:

* ``polarity_flip``        — same slot+object, sign inverted
* ``object_drift``         — same slot, object replaced
* ``uncertainty_normalized`` — hedged source rendered definite
* ``certainty_drift``      — other certainty change
* ``seq_drift``            — temporal sequence index changed
* ``fabricated``           — no source claim shares the slot
* ``unverifiable``         — pointer lost but content still consistent
                           with some source claim (counted separately,
                           not as a mutation)

Verbatim source spans and memory claims are faithful by construction;
they still pass through the same check.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

from ..context_builder.bundle import ContextBundle
from ..store.events import Claim
from ..store.immutable_store import ImmutableEventStore

MUTATION_KINDS = (
    "polarity_flip",
    "object_drift",
    "uncertainty_normalized",
    "certainty_drift",
    "seq_drift",
    "fabricated",
)


@dataclass(frozen=True)
class ClaimVerdict:
    span_text: str
    claim: Claim
    verdict: str           # faithful | unverifiable | <mutation kind>
    matched_source: Optional[str]


@dataclass
class MutationReport:
    total_claims: int = 0
    faithful: int = 0
    unverifiable: int = 0
    mutated: int = 0
    by_kind: Dict[str, int] = field(default_factory=dict)
    verdicts: List[ClaimVerdict] = field(default_factory=list)

    @property
    def mutation_rate(self) -> float:
        return self.mutated / self.total_claims if self.total_claims else 0.0


def _source_claims(store: ImmutableEventStore) -> List[Tuple[str, Claim]]:
    return [(e.event_id, e.claim) for e in store.claims()]


def classify_claim(
    claim: Claim, candidates: List[Tuple[str, Claim]]
) -> Tuple[str, Optional[str]]:
    """Classify ``claim`` against candidate (event_id, source-claim)s."""
    for eid, sc in candidates:
        if claim.canonical() == sc.canonical():
            return "faithful", eid
    same_slot = [(eid, sc) for eid, sc in candidates
                 if sc.slot() == claim.slot()]
    if not same_slot:
        return "fabricated", None
    # same slot: find closest difference
    for eid, sc in same_slot:
        if (sc.obj == claim.obj and sc.seq == claim.seq
                and sc.polarity != claim.polarity):
            return "polarity_flip", eid
    for eid, sc in same_slot:
        if (sc.obj == claim.obj and sc.polarity == claim.polarity
                and sc.seq == claim.seq and sc.certainty != claim.certainty):
            if claim.certainty == "certain" and sc.hedged():
                return "uncertainty_normalized", eid
            return "certainty_drift", eid
    for eid, sc in same_slot:
        if sc.obj == claim.obj and sc.seq != claim.seq:
            return "seq_drift", eid
    for eid, sc in same_slot:
        if sc.obj != claim.obj:
            return "object_drift", eid
    return "object_drift", same_slot[0][0]


def analyze_bundle(
    bundle: ContextBundle, store: ImmutableEventStore
) -> MutationReport:
    rep = MutationReport()
    all_sources = _source_claims(store)
    for span in bundle.claims():
        claim = span.claim
        rep.total_claims += 1
        pointed = [
            (eid, store.get(eid).claim)
            for eid in span.prov.derived_from
            if store.get(eid) is not None and store.get(eid).claim
        ]
        if pointed:
            verdict, eid = classify_claim(claim, pointed)
            if verdict == "fabricated":
                # pointer exists but content matches nothing it points
                # at — check all sources for a better explanation
                verdict, eid = classify_claim(claim, all_sources)
        else:
            verdict, eid = classify_claim(claim, all_sources)
            if verdict == "faithful":
                verdict = "unverifiable"
        rep.verdicts.append(
            ClaimVerdict(span.text, claim, verdict, eid)
        )
        if verdict == "faithful":
            rep.faithful += 1
        elif verdict == "unverifiable":
            rep.unverifiable += 1
        else:
            rep.mutated += 1
            rep.by_kind[verdict] = rep.by_kind.get(verdict, 0) + 1
    return rep
