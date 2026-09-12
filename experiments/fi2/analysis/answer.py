"""Deterministic answer resolver — stands in for "the model".

There is no LLM available, so QA is scored by a *perfect deterministic
reader* of the context bundle: it collects claims for the queried slot,
keeps the highest-source-turn versions, prefers verbatim/provenanced
claims over lossy-derived ones when they conflict, and compares the
resolved state to the structured gold answer.

QA accuracy therefore measures whether the context bundle *delivers*
enough faithful information for a perfect reader — it is a property of
the context architecture, not of any model.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Tuple

from ..context_builder.bundle import ContextBundle
from ..context_builder.builders import TRUST
from ..datasets.generator import QA
from ..store.events import Claim


@dataclass(frozen=True)
class Resolution:
    found: bool
    values: Tuple[str, ...] = ()        # resolved object values
    polarity: int = 0
    certainty: str = "certain"
    conflict: bool = False
    correct: bool = False
    effective: Tuple[Claim, ...] = ()   # winning claims (for inspection)


def _slot_claims(bundle: ContextBundle, slot: Tuple[str, str]):
    return [
        s for s in bundle.claims()
        if s.claim is not None and s.claim.slot() == slot
    ]


def _effective_claims(spans) -> List:
    """Resolve the effective claims for one slot.

    Each *source* (narrator, sensor_a, kai, ...) gets one effective
    position: its highest-turn claim. At equal turn, verbatim /
    provenanced tiers outrank lossy-derived tiers — a reader with
    source access trusts the source over the summary. Disagreement
    across sources at their latest positions is a live contradiction,
    not a supersession.
    """
    if not spans:
        return []
    by_source = {}
    for s in spans:
        by_source.setdefault(s.claim.source, []).append(s)
    top: List = []
    for lst in by_source.values():
        max_turn = max(s.prov.turn for s in lst)
        latest = [s for s in lst if s.prov.turn == max_turn]
        best_trust = max(TRUST.get(s.prov.derivation_type, 0) for s in latest)
        top.extend(
            s for s in latest
            if TRUST.get(s.prov.derivation_type, 0) == best_trust
        )
    # dedup identical claims
    seen, out = set(), []
    for s in top:
        key = s.claim.canonical() + (s.claim.source,)
        if key not in seen:
            seen.add(key)
            out.append(s)
    return out


def resolve(bundle: ContextBundle, qa: QA) -> Resolution:
    gold = qa.gold

    if qa.kind == "temporal":
        want = gold["order"]  # [earlier_step, later_step]
        seqs = {}
        for s in bundle.claims():
            c = s.claim
            if c.subject == "winterize_boat" and c.obj in want and c.seq:
                seqs[c.obj] = c.seq
        if len(seqs) < 2:
            return Resolution(found=False)
        ok = seqs[want[0]] < seqs[want[1]]
        return Resolution(found=True, values=tuple(sorted(seqs)),
                          correct=ok)

    if "claim" in gold:
        slot = (gold["claim"][0], gold["claim"][1])
    else:  # contradiction: slot derived from entities
        slot = (qa.entities[0], qa.entities[1])
    spans = _slot_claims(bundle, slot)
    eff = _effective_claims(spans)
    if not eff:
        return Resolution(found=False)

    values = tuple(sorted({s.claim.obj for s in eff}))
    conflict = len({s.claim.obj for s in eff}) > 1 or len(
        {s.claim.polarity for s in eff}) > 1

    if qa.kind == "contradiction":
        want_set = tuple(gold["conflict"])
        return Resolution(
            found=True, values=values, conflict=conflict,
            correct=conflict and values == want_set,
            effective=tuple(s.claim for s in eff),
        )

    g = gold["claim"]
    want_obj, want_pol, want_cert = g[2], g[3], g[4]
    if conflict:
        return Resolution(found=True, values=values, conflict=True,
                          correct=False,
                          effective=tuple(s.claim for s in eff))
    c = eff[0].claim
    ok = (
        c.obj == want_obj
        and c.polarity == want_pol
        # hedged gold must stay hedged; certain gold tolerates a hedge
        # (a hedged truth is still the truth — tracked separately in
        # the uncertainty-preservation metric)
        and (want_cert == "certain" or c.certainty == want_cert)
    )
    return Resolution(
        found=True, values=(c.obj,), polarity=c.polarity,
        certainty=c.certainty, correct=ok,
        effective=tuple(s.claim for s in eff),
    )
