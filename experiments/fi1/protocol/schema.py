"""Turn envelope schema: fields, canonical serialization, checksum, states.

A Turn is the unit of transactional processing in Fi1. The envelope is
content-addressed by a SHA-256 checksum computed over the canonical JSON of
every field except the checksum itself, so any mutation of payload, metadata,
sequence, timestamp or references is detected.

Canonical JSON = ``json.dumps(..., sort_keys=True, separators=(",", ":"))``.
Floats serialize via ``repr`` (exact round-trip in CPython), so a checksum is
stable across processes and platforms.
"""

from __future__ import annotations

import hashlib
import json
import time
import uuid
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List

TURN_SCHEMA_VERSION = 1

#: Envelope blocks that must be present and non-null in every turn.
REQUIRED_FIELDS = (
    "turn_id",
    "conversation_id",
    "sequence",
    "timestamp",
    "payload",
    "metadata",
    "references",
    "schema_version",
    "checksum",
)


class TurnState(str, Enum):
    """Lifecycle states of a transactional turn."""

    CREATED = "CREATED"
    RECEIVING = "RECEIVING"
    READY = "READY"
    EXECUTING = "EXECUTING"
    COMMITTED = "COMMITTED"
    ABORTED = "ABORTED"


TERMINAL_STATES = frozenset({TurnState.COMMITTED, TurnState.ABORTED})

#: Legal forward transitions. COMMITTED/ABORTED are terminal.
LEGAL_TRANSITIONS: Dict[TurnState, frozenset] = {
    TurnState.CREATED: frozenset({TurnState.RECEIVING, TurnState.ABORTED}),
    TurnState.RECEIVING: frozenset({TurnState.READY, TurnState.ABORTED}),
    TurnState.READY: frozenset({TurnState.EXECUTING, TurnState.ABORTED}),
    TurnState.EXECUTING: frozenset({TurnState.COMMITTED, TurnState.ABORTED}),
    TurnState.COMMITTED: frozenset(),
    TurnState.ABORTED: frozenset(),
}


class SchemaError(ValueError):
    """Envelope structure problem (distinct from checksum/validation)."""


def canonical_json(obj: Any) -> str:
    """Deterministic JSON used for checksums and content comparison."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False)


def compute_checksum(fields: Dict[str, Any]) -> str:
    """SHA-256 over the canonical JSON of every field except ``checksum``."""
    body = {k: v for k, v in fields.items() if k != "checksum"}
    return hashlib.sha256(canonical_json(body).encode("utf-8")).hexdigest()


def verify_checksum(fields: Dict[str, Any]) -> bool:
    """True iff the envelope's checksum matches its content."""
    provided = fields.get("checksum")
    if not isinstance(provided, str) or len(provided) != 64:
        return False
    try:
        int(provided, 16)
    except ValueError:
        return False
    return provided == compute_checksum(fields)


class DuplicateKeyError(SchemaError):
    """Raised when a raw JSON object contains the same key twice."""

    def __init__(self, key: str):
        super().__init__(f"duplicate object key: {key!r}")
        self.key = key


def _reject_dupes(pairs: List) -> Dict:
    out: Dict[str, Any] = {}
    for k, v in pairs:
        if k in out:
            raise DuplicateKeyError(k)
        out[k] = v
    return out


def loads_strict(raw: bytes | str) -> Dict[str, Any]:
    """Parse raw bytes strictly.

    ``json.loads`` silently keeps the last occurrence of a duplicated key;
    a "duplicate block" on the wire would therefore pass unnoticed. This
    parser rejects any object with a repeated key.
    """
    if isinstance(raw, bytes):
        raw = raw.decode("utf-8")
    obj = json.loads(raw, object_pairs_hook=_reject_dupes)
    if not isinstance(obj, dict):
        raise SchemaError("envelope must be a JSON object")
    return obj


@dataclass
class Turn:
    """Immutable in-memory view of a turn envelope."""

    turn_id: str
    conversation_id: str
    sequence: int
    timestamp: float
    payload: Dict[str, Any]
    metadata: Dict[str, Any]
    references: List[str] = field(default_factory=list)
    schema_version: int = TURN_SCHEMA_VERSION
    checksum: str = ""

    def fields_dict(self) -> Dict[str, Any]:
        return {
            "turn_id": self.turn_id,
            "conversation_id": self.conversation_id,
            "sequence": self.sequence,
            "timestamp": self.timestamp,
            "payload": self.payload,
            "metadata": self.metadata,
            "references": list(self.references),
            "schema_version": self.schema_version,
            "checksum": self.checksum,
        }

    def to_dict(self) -> Dict[str, Any]:
        return self.fields_dict()

    def recompute_checksum(self) -> "Turn":
        """Return a copy with a freshly computed checksum."""
        d = self.fields_dict()
        d["checksum"] = compute_checksum(d)
        return Turn.from_dict(d)

    def checksum_valid(self) -> bool:
        return verify_checksum(self.fields_dict())

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Turn":
        try:
            return cls(
                turn_id=d["turn_id"],
                conversation_id=d["conversation_id"],
                sequence=int(d["sequence"]),
                timestamp=float(d["timestamp"]),
                payload=dict(d["payload"]),
                metadata=dict(d["metadata"]),
                references=list(d["references"]),
                schema_version=int(d["schema_version"]),
                checksum=str(d["checksum"]),
            )
        except (KeyError, TypeError, ValueError) as e:
            raise SchemaError(f"cannot build Turn: {e}") from e

    def canonical_bytes(self) -> bytes:
        return canonical_json(self.fields_dict()).encode("utf-8")


def make_turn(
    conversation_id: str = "conv",
    sequence: int = 0,
    payload: Dict[str, Any] | None = None,
    metadata: Dict[str, Any] | None = None,
    references: List[str] | None = None,
    timestamp: float | None = None,
    turn_id: str | None = None,
    schema_version: int = TURN_SCHEMA_VERSION,
) -> Dict[str, Any]:
    """Build a complete, checksum-valid envelope dict.

    This is the canonical way to mint turns in tests/benchmarks; the
    checksum always reflects the content.
    """
    d = {
        "turn_id": turn_id or f"turn_{uuid.uuid4().hex[:16]}",
        "conversation_id": conversation_id,
        "sequence": sequence,
        "timestamp": time.time() if timestamp is None else float(timestamp),
        "payload": dict(payload or {"kind": "user_message", "text": "hello"}),
        "metadata": dict(metadata or {"sender": "user"}),
        "references": list(references or []),
        "schema_version": schema_version,
    }
    d["checksum"] = compute_checksum(d)
    return d
