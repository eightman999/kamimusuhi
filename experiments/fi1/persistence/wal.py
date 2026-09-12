"""Write-ahead log: the durable record of every transactional step.

The WAL is an append-only table inside the same SQLite database as the turn
store, so each WAL record is committed in the same SQLite transaction as the
state change it describes (SQLite commit = fsync'd WAL-frame flush). That is
stronger than a separate log file: a step and its record can never diverge.

Event order for a happy-path turn:

    TURN_RECEIVED -> TURN_VALIDATED -> EXECUTION_STARTED -> MODEL_PLANNED
    -> EFFECT_REQUESTED -> EFFECT_COMPLETED (per effect)
    -> MEMORY_PREPARED -> TURN_COMMITTED

Recovery replays these records; it never needs the in-memory state of the
crashed process.
"""

from __future__ import annotations

import time
from enum import Enum
from typing import Any, Dict, Optional


class WalEvent(str, Enum):
    TURN_RECEIVED = "TURN_RECEIVED"
    TURN_VALIDATED = "TURN_VALIDATED"
    TURN_REJECTED = "TURN_REJECTED"
    TURN_CONFLICT = "TURN_CONFLICT"
    EXECUTION_STARTED = "EXECUTION_STARTED"
    EXECUTION_RESUMED = "EXECUTION_RESUMED"   # recovery roll-forward marker
    MODEL_PLANNED = "MODEL_PLANNED"
    EFFECT_REQUESTED = "EFFECT_REQUESTED"
    EFFECT_COMPLETED = "EFFECT_COMPLETED"
    EFFECT_ORPHANED = "EFFECT_ORPHANED"
    MEMORY_PREPARED = "MEMORY_PREPARED"
    MEMORY_DISCARDED = "MEMORY_DISCARDED"
    TURN_COMMITTED = "TURN_COMMITTED"
    TURN_ABORTED = "TURN_ABORTED"
    RECOVERY_STARTED = "RECOVERY_STARTED"
    RECOVERY_COMPLETED = "RECOVERY_COMPLETED"


class WalWriter:
    """Append-only writer bound to a Store, with overhead stats.

    ``enabled=False`` turns appends into no-ops (used only by the benchmark
    to quantify WAL overhead; never in correctness tests).
    """

    def __init__(self, store, enabled: bool = True):
        self.store = store
        self.enabled = enabled
        self.appends = 0
        self.bytes_written = 0
        self.seconds = 0.0

    def append(self, event: WalEvent, turn_id: Optional[str] = None,
               data: Optional[Dict[str, Any]] = None) -> None:
        if not self.enabled:
            return
        t0 = time.perf_counter()
        n = self.store._append_wal(event.value, turn_id, data or {})
        self.appends += 1
        self.bytes_written += n
        self.seconds += time.perf_counter() - t0

    def stats(self) -> Dict[str, Any]:
        return {"appends": self.appends,
                "bytes_written": self.bytes_written,
                "seconds": self.seconds}
