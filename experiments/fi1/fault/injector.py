"""Fault injection: deterministic crash points + message-level corruptions.

Two mechanisms:

1. ``FaultInjector`` — a schedule mapping named injection points to actions.
   The engine/executor/broker call ``injector.hit(point, **ctx)`` at each
   durable-step boundary. Actions: ``crash`` (raise CrashError, simulating
   mid-flight process death between two committed WAL steps) and
   ``marker_and_block`` (write a marker file then wait — the parent test
   SIGKILLs the worker at exactly that point).

2. Message fault helpers — pure functions that corrupt/reorder envelopes or
   raw bytes before delivery: partial message, block reorder, duplicated
   JSON block, whole-turn redelivery, dropped metadata, bad checksum,
   same-id-different-payload, different-id-same-payload.
"""

from __future__ import annotations

import json
import os
import random
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from ..protocol.schema import compute_checksum


class CrashError(RuntimeError):
    """Simulates process death at an injection point (in-process crash)."""


class P:
    """Injection point names (called at committed-step boundaries)."""

    BEFORE_READY = "before_ready"                  # crash before READY
    EXECUTING_START = "executing_start"            # crash during EXECUTING
    AFTER_MODEL_PLANNED = "after_model_planned"    # crash during EXECUTING
    AFTER_EFFECT_REQUESTED = "after_effect_requested"   # intent durable, sink untouched
    AFTER_EFFECT_COMPLETED = "after_effect_completed"   # effect done, uncommitted
    AFTER_MEMORY_PREPARED = "after_memory_prepared"     # staged, uncommitted
    BEFORE_COMMIT = "before_commit"                # crash right before commit tx
    AFTER_COMMIT = "after_commit"                  # crash right after commit tx

    ALL = (BEFORE_READY, EXECUTING_START, AFTER_MODEL_PLANNED,
           AFTER_EFFECT_REQUESTED, AFTER_EFFECT_COMPLETED,
           AFTER_MEMORY_PREPARED, BEFORE_COMMIT, AFTER_COMMIT)


class FaultInjector:
    """Deterministic schedule: {point: action}. Actions may be the string
    'crash' or a callable(ctx)->None. ``hit`` is a no-op when unscheduled."""

    def __init__(self, schedule: Optional[Dict[str, Any]] = None):
        self.schedule = dict(schedule or {})
        self.hits: List[str] = []

    def hit(self, point: str, **ctx) -> None:
        self.hits.append(point)
        action = self.schedule.get(point)
        if action is None:
            return
        if action == "crash":
            raise CrashError(f"crash injected at {point}")
        if callable(action):
            action(**ctx)
            return
        raise ValueError(f"unknown injection action {action!r}")


def marker_and_block(marker_path: str | Path,
                     proceed_path: Optional[str | Path] = None,
                     timeout: float = 120.0) -> Callable:
    """Injection action for the kill-harness worker: durably write
    ``marker_path`` then block until ``proceed_path`` appears or timeout.
    The parent polls for the marker, then SIGKILLs — a real process death
    at a known WAL boundary."""
    marker_path = Path(marker_path)
    proceed = Path(proceed_path) if proceed_path else marker_path.with_suffix(
        marker_path.suffix + ".proceed")

    def _action(**ctx) -> None:
        marker_path.write_text(json.dumps(ctx) + "\n")
        fd = os.open(marker_path, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        deadline = time.time() + timeout
        while time.time() < deadline:
            if proceed.exists():
                return
            time.sleep(0.02)
        raise CrashError("marker action timed out waiting for proceed")

    return _action


# ------------------------------------------------------------------ faults

def partial_message(raw: bytes, keep_fraction: float = 0.6) -> bytes:
    """Truncated delivery: only a prefix of the envelope arrives."""
    n = max(1, int(len(raw) * keep_fraction))
    return raw[:n]


def drop_fields(d: Dict[str, Any], fields: List[str]) -> Dict[str, Any]:
    """Partial message at the block level: blocks never arrived."""
    out = dict(d)
    for f in fields:
        out.pop(f, None)
    return out


def duplicate_block(d: Dict[str, Any], field: str = "payload") -> bytes:
    """Serialize an envelope with one top-level block repeated — the classic
    'duplicate block' wire fault. Standard parsers keep the last copy."""
    items = list(d.items())
    out: List[str] = []
    for k, v in items:
        out.append(json.dumps(k) + ":" + json.dumps(v, sort_keys=True))
        if k == field:
            out.append(json.dumps(k) + ":" +
                       json.dumps(v, sort_keys=True))
    return ("{" + ",".join(out) + "}").encode("utf-8")


def reorder(turns: List[Dict[str, Any]], rng: random.Random
            ) -> List[Dict[str, Any]]:
    """Packet reorder: deliver envelopes out of sequence order."""
    out = list(turns)
    rng.shuffle(out)
    return out


def drop_metadata(d: Dict[str, Any]) -> Dict[str, Any]:
    out = dict(d)
    out.pop("metadata", None)
    return out


def corrupt_checksum(d: Dict[str, Any]) -> Dict[str, Any]:
    """Flip one hex digit: content and checksum now disagree."""
    out = dict(d)
    cs = out["checksum"]
    i = len(cs) - 1
    out["checksum"] = cs[:i] + ("0" if cs[i] != "0" else "1")
    return out


def mutate_payload_same_id(d: Dict[str, Any],
                           extra: str = "mutated") -> Dict[str, Any]:
    """Same turn_id, different payload, honestly re-checksummed — the
    'same UUID different payload' conflict case."""
    out = json.loads(json.dumps(d))
    out["payload"]["text"] = str(out["payload"].get("text", "")) + extra
    out["checksum"] = compute_checksum(out)
    return out


def same_payload_new_id(d: Dict[str, Any]) -> Dict[str, Any]:
    """Identical payload under a fresh turn_id — a distinct transaction."""
    out = json.loads(json.dumps(d))
    out["turn_id"] = f"turn_{uuid.uuid4().hex[:16]}"
    out["checksum"] = compute_checksum(out)
    return out


def random_mutation(d: Dict[str, Any], rng: random.Random) -> Dict[str, Any]:
    """Adversarial envelope mutation used by the fuzz test. Always produces
    an envelope that SHOULD be rejected or conflicted, never executed."""
    out = json.loads(json.dumps(d))
    choice = rng.randrange(6)
    if choice == 0:
        out["payload"]["text"] = rng.random()
    elif choice == 1:
        out["metadata"] = rng.choice([{"sender": "eve"}, "not-a-dict",
                                      None, [1, 2]])
    elif choice == 2:
        out["sequence"] = rng.choice([-1, "x", 1.5, None])
    elif choice == 3:
        out["timestamp"] = rng.choice(["now", None, -1e9])
    elif choice == 4:
        out.pop(rng.choice(["payload", "metadata", "references",
                            "sequence", "checksum"]))
    else:
        out["references"] = rng.choice([None, "x", [1, 2]])
    # leave checksum stale with 50% probability, else honestly recompute —
    # both paths must be refused.
    if rng.random() < 0.5 and "checksum" in out:
        try:
            out["checksum"] = compute_checksum(out)
        except Exception:
            pass
    return out
