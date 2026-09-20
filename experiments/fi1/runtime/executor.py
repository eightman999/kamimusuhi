"""Turn executor: drives READY -> EXECUTING -> COMMITTED.

Flow per turn (each numbered step is one fsync'd transaction):

  1. claim: READY -> EXECUTING  + EXECUTION_STARTED
  2. model.plan()               + MODEL_PLANNED (plan stored durably)
  3. per effect, via the gated SideEffectBroker:
       requested record + EFFECT_REQUESTED
       sink.invoke()
       completed record + EFFECT_COMPLETED
  4. stage memory writes        + MEMORY_PREPARED   (prepared, not visible)
  5. commit tx: memory -> committed, turn -> COMMITTED, result stored
                                + TURN_COMMITTED

A crash between any two steps leaves a consistent durable state that
recovery can resume or explicitly abort. Injection points (injector.hit)
sit at every step boundary — see fault.injector.P.
"""

from __future__ import annotations

import json
import time
from typing import Any, Dict, Optional

from ..fault.injector import P
from ..persistence.store import Store
from ..persistence.wal import WalEvent, WalWriter
from ..protocol.schema import Turn, TurnState
from ..protocol.transaction import (InvalidTransition, SideEffectBroker,
                                    TransactionGate)


class Executor:
    def __init__(self, store: Store, wal: WalWriter, model, broker,
                 injector=None):
        self.store = store
        self.wal = wal
        self.model = model
        self.broker = broker
        self.injector = injector
        self.gate = TransactionGate(store)

    def _hit(self, point: str, **ctx) -> None:
        if self.injector is not None:
            self.injector.hit(point, **ctx)

    def execute(self, turn_id: str) -> Dict[str, Any]:
        """Execute one READY turn to COMMITTED. Idempotent: a COMMITTED turn
        returns its stored result; an ABORTED turn returns its marker."""
        row = self.store.get_turn(turn_id)
        if row is None:
            raise InvalidTransition(f"execute: unknown turn {turn_id}")
        state = TurnState(row["state"])
        if state == TurnState.COMMITTED:
            return {"status": "committed", "duplicate": True,
                    "result": json.loads(row["result_json"] or "{}")}
        if state == TurnState.ABORTED:
            return {"status": "aborted", "error": row["error"]}
        if state != TurnState.READY:
            raise InvalidTransition(
                f"execute: turn {turn_id} in {state.value}, need READY")

        # (1) claim + EXECUTION_STARTED in one transaction.
        with self.store.transaction():
            if not self.store.claim_for_execution(turn_id):
                raise InvalidTransition(f"execute: lost claim on {turn_id}")
            self.wal.append(WalEvent.EXECUTION_STARTED, turn_id, {})
        self._hit(P.EXECUTING_START, turn_id=turn_id)

        result = self._run_executing(turn_id)
        return {"status": "committed", "duplicate": False, "result": result}

    def _run_executing(self, turn_id: str,
                       allow_reinvoke: bool = False) -> Dict[str, Any]:
        """Body of an EXECUTING turn: plan -> effects -> memory -> commit.
        Also used by recovery when the crash preceded MODEL_PLANNED — the
        model is deterministic, so re-planning is side-effect-free."""
        row = self.store.get_turn(turn_id)
        turn = Turn.from_dict(json.loads(row["turn_json"]))

        # (2) model inference — gated: inference only in EXECUTING.
        self.gate.require_state(turn_id, TurnState.EXECUTING)
        plan = self.model.plan(turn)
        with self.store.transaction():
            self.store.set_turn_plan(turn_id, json.dumps(plan,
                                                         sort_keys=True))
            self.wal.append(WalEvent.MODEL_PLANNED, turn_id,
                            {"n_effects": len(plan["effects"]),
                             "n_memory": len(plan["memory_writes"])})
        self._hit(P.AFTER_MODEL_PLANNED, turn_id=turn_id)

        self._run_effects_and_commit(turn_id, plan,
                                     allow_reinvoke=allow_reinvoke)
        return self._committed_result(turn_id)

    def _run_effects_and_commit(self, turn_id: str, plan: Dict[str, Any],
                                allow_reinvoke: bool = False) -> None:
        """Steps 3-5. Used by normal execution and recovery resume (with the
        stored plan, so the model is not re-invoked)."""
        responses = []
        for effect in plan["effects"]:
            resp = self.broker.invoke_effect(turn_id, effect,
                                             allow_reinvoke=allow_reinvoke)
            responses.append({"effect_id": effect["effect_id"],
                              "response": resp})
        self._hit(P.BEFORE_COMMIT, turn_id=turn_id)

        # (4) stage memory writes (prepared, invisible until commit).
        self.broker.stage_memory(turn_id, plan["memory_writes"])
        self._hit(P.AFTER_MEMORY_PREPARED, turn_id=turn_id)

        # (5) single commit transaction: memory + state + result + WAL.
        result = {"reply": plan["reply"], "effects": responses,
                  "memory_writes": sorted(plan["memory_writes"])}
        with self.store.transaction():
            self.store.commit_memory(turn_id)
            self.store.set_turn_result(turn_id, json.dumps(result,
                                                           sort_keys=True))
            self.store.set_turn_state(turn_id, TurnState.COMMITTED.value)
            self.wal.append(WalEvent.TURN_COMMITTED, turn_id,
                            {"n_effects": len(responses),
                             "n_memory": len(result["memory_writes"])})
        self._hit(P.AFTER_COMMIT, turn_id=turn_id)

    def _committed_result(self, turn_id: str) -> Dict[str, Any]:
        row = self.store.get_turn(turn_id)
        return json.loads(row["result_json"])

    def execute_pending(self) -> int:
        """Execute all turns left in READY (auto_execute=False engines)."""
        n = 0
        for row in self.store.list_turns(state=TurnState.READY.value):
            self.execute(row["turn_id"])
            n += 1
        return n
