"""TurnEngine: the composition root wiring store + WAL + gate + executor.

Delivery semantics
------------------
``receive()`` returns a receipt dict. Status values:

- ``committed``  — turn executed exactly once, result committed
- ``ready``      — accepted, not yet executed (auto_execute=False)
- ``duplicate``  — turn_id already known with identical checksum; returns
                   the previous result/marker. Never re-executes.
- ``conflict``   — turn_id already known but a DIFFERENT envelope arrived
                   (same UUID different payload, or mutated-in-transit
                   redelivery). Never re-executes.
- ``rejected``   — failed validation (partial message, missing metadata,
                   bad checksum, duplicate block, unknown version).

Idempotency key is ``turn_id`` alone: two valid envelopes with different
turn_ids but identical payloads are two distinct transactions (documented
in README — dedupe-by-content would conflate client retries that mint new
ids, and is out of scope for observed exactly-once per turn_id).
"""

from __future__ import annotations

import json
import time
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

from .. import metrics as metrics_mod
from .. import repro
from ..fault.injector import P
from ..persistence.store import Store
from ..persistence.wal import WalEvent, WalWriter
from ..protocol.schema import (DuplicateKeyError, SchemaError, TurnState,
                               loads_strict)
from ..protocol.transaction import SideEffectBroker, TransactionGate
from ..protocol.validator import ValidationError, validate_envelope
from . import recovery as recovery_mod
from .executor import Executor
from .mocks import MemorySink, MockModel


class TurnEngine:
    def __init__(self, db_path: str | Path, seed: int = 0, sink=None,
                 auto_execute: bool = True, injector=None,
                 synchronous: str = "FULL", wal_enabled: bool = True,
                 orphan_policy: str = "abort", run_role: str = "engine",
                 record_run: bool = True):
        self.db_path = Path(db_path)
        self.seed = seed
        self.auto_execute = auto_execute
        self.orphan_policy = orphan_policy
        self.injector = injector
        self.store = Store(self.db_path, synchronous=synchronous)
        self.wal = WalWriter(self.store, enabled=wal_enabled)
        self.model = MockModel(seed=seed)
        self.sink = sink if sink is not None else MemorySink()
        self.gate = TransactionGate(self.store)
        self.broker = SideEffectBroker(self.store, self.wal, self.sink,
                                       injector=injector)
        self.executor = Executor(self.store, self.wal, self.model,
                                 self.broker, injector=injector)
        self.receipts: List[Dict[str, Any]] = []
        self.recovery_reports: List[Dict[str, Any]] = []
        self.run_id = f"run_{uuid.uuid4().hex[:12]}"
        if record_run:
            self.store.record_run(self.run_id, run_role,
                                  repro.run_metadata(seed, self._config()))

    def _config(self) -> Dict[str, Any]:
        return {"seed": self.seed, "auto_execute": self.auto_execute,
                "wal_enabled": self.wal.enabled,
                "orphan_policy": self.orphan_policy,
                "sink": type(self.sink).__name__}

    def close(self) -> None:
        self.store.close()

    # ------------------------------------------------------------- receive
    def receive(self, msg: Union[Dict, bytes, str]) -> Dict[str, Any]:
        """Deliver one envelope (parsed dict or raw wire bytes)."""
        t0 = time.perf_counter()
        receipt = self._receive_inner(msg)
        receipt["latency_ms"] = (time.perf_counter() - t0) * 1000.0
        self.receipts.append(receipt)
        return receipt

    def _receive_inner(self, msg: Union[Dict, bytes, str]) -> Dict[str, Any]:
        # -- parse ------------------------------------------------------
        if isinstance(msg, (bytes, str)):
            try:
                d = loads_strict(msg)
            except (DuplicateKeyError, SchemaError,
                    json.JSONDecodeError, UnicodeDecodeError) as e:
                self.wal.append(WalEvent.TURN_REJECTED, None,
                                {"reason": "parse_error",
                                 "detail": str(e)[:200],
                                 "raw": (msg if isinstance(msg, str)
                                         else msg[:200]).__repr__()[:200]})
                return {"status": "rejected", "reason": "parse_error",
                        "detail": str(e), "duplicate": False}
        else:
            d = msg

        turn_id = d.get("turn_id") if isinstance(d, dict) else None

        # -- validate ----------------------------------------------------
        try:
            validate_envelope(d)
        except ValidationError as e:
            return self._reject(d, e)

        # -- dedupe ------------------------------------------------------
        stored = self.store.get_turn(turn_id)
        if stored is not None:
            if stored["checksum"] == d["checksum"]:
                return {"status": "duplicate", "turn_id": turn_id,
                        "state": stored["state"], "duplicate": True,
                        "result": json.loads(stored["result_json"] or "null")}
            with self.store.transaction():
                self.wal.append(WalEvent.TURN_CONFLICT, turn_id,
                                {"stored_checksum": stored["checksum"],
                                 "incoming_checksum": d["checksum"]})
            return {"status": "conflict", "turn_id": turn_id,
                    "duplicate": False}

        # -- accept: RECEIVING -------------------------------------------
        with self.store.transaction():
            self.store.insert_turn(turn_id, d["conversation_id"],
                                   d["sequence"], TurnState.RECEIVING.value,
                                   d["checksum"], json.dumps(d,
                                                             sort_keys=True))
            self.wal.append(WalEvent.TURN_RECEIVED, turn_id,
                            {"conversation_id": d["conversation_id"],
                             "sequence": d["sequence"]})
        self._hit(P.BEFORE_READY, turn_id=turn_id)

        # -- READY ---------------------------------------------------------
        with self.store.transaction():
            self.gate.transition(turn_id, TurnState.READY)
            self.wal.append(WalEvent.TURN_VALIDATED, turn_id, {})

        if not self.auto_execute:
            return {"status": "ready", "turn_id": turn_id,
                    "state": TurnState.READY.value, "duplicate": False}
        r = self.executor.execute(turn_id)
        r.update({"turn_id": turn_id})
        return r

    def _reject(self, d: Dict[str, Any], e: ValidationError
                ) -> Dict[str, Any]:
        turn_id = d.get("turn_id") if isinstance(d, dict) else None
        stored = (self.store.get_turn(turn_id)
                  if isinstance(turn_id, str) and turn_id else None)
        if stored is not None:
            same = stored["checksum"] == d.get("checksum")
            with self.store.transaction():
                self.wal.append(
                    WalEvent.TURN_REJECTED if same else WalEvent.TURN_CONFLICT,
                    turn_id, {"reason": e.reason})
            return {"status": "rejected" if same else "conflict",
                    "turn_id": turn_id, "reason": e.reason,
                    "duplicate": same}
        with self.store.transaction():
            if isinstance(turn_id, str) and turn_id:
                self.store.insert_turn(
                    turn_id,
                    str(d.get("conversation_id") or ""),
                    d.get("sequence") if isinstance(d.get("sequence"), int)
                    else -1,
                    TurnState.ABORTED.value, d.get("checksum"),
                    json.dumps(d, sort_keys=True, default=str),
                    error=f"rejected:{e.reason}")
                self.wal.append(WalEvent.TURN_RECEIVED, turn_id,
                                {"rejected": True})
            self.wal.append(WalEvent.TURN_REJECTED, turn_id,
                            {"reason": e.reason})
        return {"status": "rejected", "turn_id": turn_id,
                "reason": e.reason, "duplicate": False}

    def _hit(self, point: str, **ctx) -> None:
        if self.injector is not None:
            self.injector.hit(point, **ctx)

    # ------------------------------------------------------------- drive
    def execute_pending(self) -> int:
        return self.executor.execute_pending()

    def recover(self, orphan_policy: Optional[str] = None) -> Dict[str, Any]:
        report = recovery_mod.recover(
            self.store, self.wal, self.executor, self.broker,
            orphan_policy=orphan_policy or self.orphan_policy)
        self.recovery_reports.append(report)
        return report

    def metrics(self) -> Dict[str, Any]:
        return metrics_mod.collect(self.store, self.sink, self.receipts,
                                   self.wal, self.recovery_reports,
                                   self.broker)
