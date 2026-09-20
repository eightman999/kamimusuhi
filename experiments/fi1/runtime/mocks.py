"""Deterministic mocks: a scripted "model" and an idempotent effect sink.

MockModel.plan() is a pure function of the turn envelope — same input always
yields the same reply, effect list and memory writes. That determinism is
what makes crash-recovery safe to re-derive a plan without an LLM.

The sinks stand in for "the external world". Every call is an *invocation*;
the sink dedupes on (turn_id, effect_id), so a re-invocation is visible as a
duplicate *invocation* but does not produce a duplicate *applied* effect.
FileSink additionally appends each invocation to an fsync'd JSONL so the
external world survives a SIGKILL of the worker process.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

from ..protocol.schema import Turn, canonical_json


def _payload_digest(turn: Turn) -> str:
    return hashlib.sha256(
        canonical_json(turn.payload).encode("utf-8")).hexdigest()[:12]


class MockModel:
    """Scripted responder. Deterministic by construction."""

    def __init__(self, seed: int = 0):
        self.seed = seed
        self.calls: List[str] = []  # turn_ids, for "was inference run" checks

    def plan(self, turn: Turn) -> Dict[str, Any]:
        """Return {reply, effects, memory_writes} for a turn."""
        self.calls.append(turn.turn_id)
        digest = _payload_digest(turn)
        reply = f"ack[{turn.conversation_id}/{turn.sequence}]:{digest}"
        effects: List[Dict[str, Any]] = [
            {"effect_id": "reply", "kind": "send_reply",
             "data": {"to": turn.metadata.get("sender", "unknown"),
                      "text": reply}},
        ]
        for i, e in enumerate(turn.payload.get("effects", [])):
            e = dict(e)
            e.setdefault("effect_id", f"fx{i}")
            e.setdefault("kind", "generic")
            e.setdefault("data", {})
            effects.append(e)
        memory_writes = {"last_reply": reply,
                         f"seq:{turn.sequence}": digest}
        memory_writes.update(turn.payload.get("memory_writes", {}))
        return {"reply": reply, "effects": effects,
                "memory_writes": memory_writes}


class SinkError(RuntimeError):
    pass


class MemorySink:
    """In-process idempotent side-effect sink.

    ``invocations`` records every call (raw log). ``applied`` dedupes on
    (turn_id, effect_id): a repeated call returns the first response with
    ``applied=False``. ``fail_on`` may raise SinkError to emulate an
    external-world failure.
    """

    def __init__(self,
                 fail_on: Optional[Callable[[str, str, str, Dict], bool]] = None):
        self.invocations: List[Dict[str, Any]] = []
        self.applied: Dict[Tuple[str, str], Dict[str, Any]] = {}
        self.fail_on = fail_on

    def invoke(self, turn_id: str, effect_id: str, kind: str,
               data: Dict[str, Any]) -> Dict[str, Any]:
        rec = {"turn_id": turn_id, "effect_id": effect_id, "kind": kind,
               "data": data, "seq": len(self.invocations)}
        self.invocations.append(rec)
        self._record(rec)
        if self.fail_on and self.fail_on(turn_id, effect_id, kind, data):
            raise SinkError(f"sink failure injected for {effect_id}")
        key = (turn_id, effect_id)
        if key in self.applied:
            r = dict(self.applied[key])
            r["applied"] = False
            return r
        resp = {"ok": True, "applied": True, "seq": rec["seq"]}
        self.applied[key] = resp
        return resp

    def _record(self, rec: Dict[str, Any]) -> None:
        """Persistence hook (FileSink overrides)."""

    # -- metrics ------------------------------------------------------
    @property
    def invocation_count(self) -> int:
        return len(self.invocations)

    @property
    def applied_count(self) -> int:
        return len(self.applied)

    @property
    def duplicate_invocations(self) -> int:
        return self.invocation_count - self.applied_count


class FileSink(MemorySink):
    """Durable external world: every invocation is appended to a JSONL file
    with fsync before returning. Survives SIGKILL of the worker."""

    def __init__(self, path: str | Path, **kw):
        self.path = Path(path)
        super().__init__(**kw)
        if self.path.exists():
            for line in self.path.read_text().splitlines():
                if not line.strip():
                    continue
                rec = json.loads(line)
                self.invocations.append(rec)
                key = (rec["turn_id"], rec["effect_id"])
                if key not in self.applied:
                    self.applied[key] = {"ok": True, "applied": True,
                                         "seq": rec["seq"]}
        self._fh = open(self.path, "a", encoding="utf-8")

    def _record(self, rec: Dict[str, Any]) -> None:
        self._fh.write(json.dumps(rec, sort_keys=True) + "\n")
        self._fh.flush()
        os.fsync(self._fh.fileno())

    def close(self) -> None:
        self._fh.close()
