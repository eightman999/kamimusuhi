"""Append-only, hash-chained event store for Fi2.

The store is the single ground truth. It is a JSONL file in which every
record is ``{seq, prev, event, digest}`` where ``digest`` is the SHA-256
of the canonical JSON of ``{seq, prev, event}`` and ``prev`` is the
digest of the previous record (genesis uses ``"0"*64``).

Guarantees:

* append-only — the class exposes no update/delete API at all;
* tamper-evident — editing or removing any historical line breaks the
  chain and is detected by :meth:`verify` / :meth:`load`;
* deterministic — identical append sequences produce identical digests.

Summarization, indexing and retrieval never write to the store; the
Fi2 test-suite asserts the head digest is unchanged after a full run
(原文削除しない — source events are never deleted or replaced).
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Dict, Iterator, List, Optional, Tuple

from .events import Event, event_from_dict, event_to_dict

GENESIS = "0" * 64


class TamperDetected(Exception):
    """Raised when the hash chain does not validate."""


def _digest(payload: dict) -> str:
    blob = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(blob).hexdigest()


class ImmutableEventStore:
    """Append-only checksummed store of :class:`Event` records."""

    def __init__(self, path: str | Path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._events: List[Event] = []
        self._by_id: Dict[str, Event] = {}
        self._records: List[dict] = []
        self._head = GENESIS
        if self.path.exists() and self.path.stat().st_size > 0:
            self._load()

    # -- read API ---------------------------------------------------------

    def __len__(self) -> int:
        return len(self._events)

    def __iter__(self) -> Iterator[Event]:
        return iter(self.events())

    def events(self) -> Tuple[Event, ...]:
        """Read-only snapshot of all events in append order."""
        return tuple(self._events)

    def get(self, event_id: str) -> Optional[Event]:
        return self._by_id.get(event_id)

    def claims(self) -> List[Event]:
        """All events carrying a structured claim."""
        return [e for e in self._events if e.claim is not None]

    @property
    def head_digest(self) -> str:
        return self._head

    # -- write API (append only) ------------------------------------------

    def append(self, event: Event) -> str:
        """Append an event; returns the new record digest."""
        if event.event_id in self._by_id:
            raise ValueError(f"duplicate event_id {event.event_id}")
        record = {
            "seq": len(self._records),
            "prev": self._head,
            "event": event_to_dict(event),
        }
        digest = _digest(record)
        line = json.dumps({**record, "digest": digest}, sort_keys=True)
        with open(self.path, "a", encoding="utf-8") as f:
            f.write(line + "\n")
        self._records.append({**record, "digest": digest})
        self._events.append(event)
        self._by_id[event.event_id] = event
        self._head = digest
        return digest

    # -- verification ------------------------------------------------------

    def _load(self) -> None:
        self._events, self._by_id, self._records = [], {}, []
        self._head = GENESIS
        with open(self.path, encoding="utf-8") as f:
            for lineno, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                payload = {"seq": rec["seq"], "prev": rec["prev"], "event": rec["event"]}
                if rec.get("digest") != _digest(payload):
                    raise TamperDetected(f"line {lineno}: digest mismatch")
                if rec["seq"] != len(self._records):
                    raise TamperDetected(f"line {lineno}: bad seq {rec['seq']}")
                if rec["prev"] != self._head:
                    raise TamperDetected(f"line {lineno}: broken chain")
                ev = event_from_dict(rec["event"])
                self._records.append(rec)
                self._events.append(ev)
                self._by_id[ev.event_id] = ev
                self._head = rec["digest"]

    def verify(self) -> bool:
        """Re-read the file from disk and re-validate the whole chain."""
        events_before = len(self._events)
        head_before = self._head
        self._load()
        return len(self._events) == events_before and self._head == head_before

    @classmethod
    def load(cls, path: str | Path) -> "ImmutableEventStore":
        return cls(path)
