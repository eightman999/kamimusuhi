"""Event schema for Fi2 — Context Integrity.

An *event* is one turn of recorded history. Events are the only things that
enter the immutable store. They carry the raw authored text plus, for
fact-like events, a structured :class:`Claim` payload.

Design rules enforced here:

* Events never carry evaluation metadata. Future-relevance labels, answer
  keys and QA bookkeeping live in the dataset side-tables (see
  ``datasets/generator.py``) and are structurally unable to leak into the
  store, index or retriever — they are simply never fields of ``Event``.
* ``Claim`` is the canonical unit of information. Mutation detectors
  compare canonical claim tuples, not surface text, so paraphrase is not
  confused with mutation.
* ``Provenance`` / ``Span`` describe derived artifacts (summaries, memory
  claims, retrieved excerpts). Derived artifacts never enter the store;
  they only point back at it via ``derived_from`` source-event ids.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Optional, Tuple

EVENT_KINDS = (
    "fact",
    "correction",
    "contradiction",
    "uncertainty",
    "relation",
    "temporal",
    "id",
    "transient",
    "filler",
)

CERTAINTIES = ("certain", "maybe", "unknown")

_TOKEN_RE = re.compile(r"[a-z0-9]+")


def tokenize(text: str) -> list[str]:
    """Deterministic whitespace/punctuation tokenizer (lowercased)."""
    return _TOKEN_RE.findall(text.lower())


@dataclass(frozen=True)
class Claim:
    """Canonical structured claim carried by fact-like events.

    ``(subject, predicate)`` identifies the *slot*; ``obj``, ``polarity``,
    ``certainty`` and ``seq`` are the *state*. Two claims about the same
    slot with different state are different versions of one fact.

    ``source`` records who asserted the claim (narrator, sensor_a, ...) so
    that unresolved contradictions between sources are representable.
    ``supersedes`` holds the event_id of the claim this one corrects.
    """

    subject: str
    predicate: str
    obj: str
    polarity: int = 1                # +1 asserted, -1 negated
    certainty: str = "certain"       # certain | maybe | unknown
    source: str = "narrator"
    seq: Optional[int] = None        # sequence index for temporal steps
    supersedes: Optional[str] = None

    def __post_init__(self):
        if self.polarity not in (1, -1):
            raise ValueError(f"polarity must be ±1, got {self.polarity}")
        if self.certainty not in CERTAINTIES:
            raise ValueError(f"bad certainty {self.certainty!r}")

    def slot(self) -> Tuple[str, str]:
        return (self.subject, self.predicate)

    def canonical(self) -> Tuple:
        """Content identity used by all mutation/retention comparisons."""
        return (
            self.subject,
            self.predicate,
            self.obj,
            self.polarity,
            self.certainty,
            self.seq,
        )

    def hedged(self) -> bool:
        return self.certainty != "certain"

    def render(self) -> str:
        """Canonical one-line rendering (used by lossy summaries)."""
        neg = "not " if self.polarity < 0 else ""
        base = f"{self.subject} {neg}{self.predicate} {self.obj}"
        if self.certainty == "maybe":
            base += " (reported as uncertain)"
        elif self.certainty == "unknown":
            base += " (reported as unknown)"
        if self.source != "narrator":
            base += f" [source: {self.source}]"
        return base


@dataclass(frozen=True)
class Event:
    """One immutable turn of history. The sole ground-truth record."""

    event_id: str
    turn: int
    kind: str
    text: str
    claim: Optional[Claim] = None
    entities: Tuple[str, ...] = ()

    def __post_init__(self):
        if self.kind not in EVENT_KINDS:
            raise ValueError(f"bad event kind {self.kind!r}")

    def tokens(self) -> list[str]:
        return tokenize(self.text)


@dataclass(frozen=True)
class Provenance:
    """Where a derived artifact's content came from.

    ``derived_from`` is always the transitive closure of *source event
    ids* (never intermediate summary ids), so any artifact can be resolved
    directly against the store. ``derivation_type`` is one of
    ``source`` | ``memory_claim`` | ``source_excerpt`` |
    ``summary_lossy`` | ``summary_extractive``.
    """

    derivation_type: str
    source_event_id: Optional[str]
    derived_from: Tuple[str, ...]
    turn: int
    depth: int = 0                 # cascade depth (0 = direct from source)
    confidence: float = 1.0


@dataclass(frozen=True)
class Span:
    """A unit of context: raw source text or a derived claim/gist."""

    text: str
    claim: Optional[Claim]
    prov: Provenance
    role: str = "content"          # content | source_excerpt | gist

    @staticmethod
    def from_event(ev: Event, role: str = "content") -> "Span":
        return Span(
            text=ev.text,
            claim=ev.claim,
            prov=Provenance(
                derivation_type="source",
                source_event_id=ev.event_id,
                derived_from=(ev.event_id,),
                turn=ev.turn,
                depth=0,
                confidence=1.0,
            ),
            role=role,
        )

    def tokens(self) -> list[str]:
        return tokenize(self.text)

    def token_count(self) -> int:
        return len(self.tokens())


# ---------------------------------------------------------------------------
# Serialization (JSONL store)
# ---------------------------------------------------------------------------


def claim_to_dict(c: Optional[Claim]) -> Optional[dict]:
    if c is None:
        return None
    return {
        "subject": c.subject,
        "predicate": c.predicate,
        "obj": c.obj,
        "polarity": c.polarity,
        "certainty": c.certainty,
        "source": c.source,
        "seq": c.seq,
        "supersedes": c.supersedes,
    }


def claim_from_dict(d: Optional[dict]) -> Optional[Claim]:
    if d is None:
        return None
    return Claim(
        subject=d["subject"],
        predicate=d["predicate"],
        obj=d["obj"],
        polarity=int(d["polarity"]),
        certainty=d["certainty"],
        source=d.get("source", "narrator"),
        seq=d.get("seq"),
        supersedes=d.get("supersedes"),
    )


def event_to_dict(ev: Event) -> dict:
    return {
        "event_id": ev.event_id,
        "turn": ev.turn,
        "kind": ev.kind,
        "text": ev.text,
        "claim": claim_to_dict(ev.claim),
        "entities": list(ev.entities),
    }


def event_from_dict(d: dict) -> Event:
    return Event(
        event_id=d["event_id"],
        turn=int(d["turn"]),
        kind=d["kind"],
        text=d["text"],
        claim=claim_from_dict(d.get("claim")),
        entities=tuple(d.get("entities") or ()),
    )
