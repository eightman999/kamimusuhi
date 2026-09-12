"""State machine enforcement and the side-effect gate.

Hard rule of Fi1: before a turn is READY there is NO model inference, NO
memory write, NO external side effect. External effects and staged memory
writes may only happen while the turn is EXECUTING, and only through
``SideEffectBroker`` — calling the sink directly is the bug this class makes
impossible to write accidentally.

Durable-write discipline (prepare -> commit): side-effect intent is recorded
(``requested``) *before* the sink is invoked, and its outcome is recorded
(``completed``) before execution proceeds. Memory writes are staged as
``prepared`` and flipped to ``committed`` inside the single commit
transaction, so a crash can never leave half-committed memory.
"""

from __future__ import annotations

import json
from typing import Any, Dict, Optional

from ..persistence.store import Store
from ..persistence.wal import WalEvent, WalWriter
from .schema import LEGAL_TRANSITIONS, TurnState


class InvalidTransition(RuntimeError):
    pass


class SideEffectGateError(RuntimeError):
    """Side effect attempted while the turn was not EXECUTING."""


class OrphanedEffectError(RuntimeError):
    """An effect is 'requested' but not 'completed': the previous process
    may or may not have invoked the sink before dying. Ambiguous — the
    effect must never be blindly re-invoked."""


class TransactionGate:
    """Checks state and enforces legal transitions against the store."""

    def __init__(self, store: Store):
        self.store = store

    def state_of(self, turn_id: str) -> Optional[TurnState]:
        row = self.store.get_turn(turn_id)
        return TurnState(row["state"]) if row else None

    def require_state(self, turn_id: str, expected: TurnState) -> None:
        state = self.state_of(turn_id)
        if state != expected:
            raise SideEffectGateError(
                f"turn {turn_id}: need {expected.value}, have "
                f"{state.value if state else 'MISSING'}")

    def transition(self, turn_id: str, to: TurnState,
                   error: Optional[str] = None) -> None:
        state = self.state_of(turn_id)
        if state is None:
            raise InvalidTransition(f"turn {turn_id} does not exist")
        if to not in LEGAL_TRANSITIONS[state]:
            raise InvalidTransition(
                f"turn {turn_id}: {state.value} -> {to.value} not allowed")
        self.store.set_turn_state(turn_id, to.value, error)


class SideEffectBroker:
    """The only legal path to external side effects and memory staging.

    Every invocation is: gate check -> durable ``requested`` record -> sink
    call -> durable ``completed`` record. A redelivery or recovery that finds
    a ``completed`` record returns the recorded response without touching
    the sink (idempotent side effects keyed by (turn_id, effect_id)).
    """

    def __init__(self, store: Store, wal: WalWriter, sink,
                 injector=None):
        self.store = store
        self.wal = wal
        self.sink = sink
        self.injector = injector
        self.gate = TransactionGate(store)
        self.skipped_completed = 0  # dedup counter for metrics

    def _hit(self, point: str, **ctx) -> None:
        if self.injector is not None:
            self.injector.hit(point, **ctx)

    def invoke_effect(self, turn_id: str, effect: Dict[str, Any],
                      allow_reinvoke: bool = False) -> Dict:
        """Run one effect under the gate + durable dedupe. Returns response.

        ``allow_reinvoke`` (recovery 'resume' policy only): a
        requested-but-not-completed effect is re-invoked through the
        idempotent sink instead of raising OrphanedEffectError."""
        effect_id = effect["effect_id"]
        self.gate.require_state(turn_id, TurnState.EXECUTING)

        row = self.store.effect_row(turn_id, effect_id)
        if row is not None and row["status"] == "completed":
            self.skipped_completed += 1
            return json.loads(row["response_json"])
        reinvoke = row is not None and row["status"] == "requested"
        if reinvoke and not allow_reinvoke:
            raise OrphanedEffectError(
                f"turn {turn_id} effect {effect_id}: requested but never "
                "recorded complete; refusing to re-invoke")

        if not reinvoke:
            with self.store.transaction():
                self.store.effect_requested(turn_id, effect_id,
                                            effect.get("kind", "unknown"),
                                            effect.get("data") or {})
                self.wal.append(WalEvent.EFFECT_REQUESTED, turn_id,
                                {"effect_id": effect_id,
                                 "kind": effect.get("kind", "unknown")})
        self._hit("after_effect_requested", turn_id=turn_id,
                  effect_id=effect_id, reinvoke=reinvoke)

        response = self.sink.invoke(turn_id, effect_id,
                                    effect.get("kind", "unknown"),
                                    effect.get("data") or {})

        with self.store.transaction():
            self.store.effect_completed(turn_id, effect_id, response)
            self.wal.append(WalEvent.EFFECT_COMPLETED, turn_id,
                            {"effect_id": effect_id})
        self._hit("after_effect_completed", turn_id=turn_id,
                  effect_id=effect_id)
        return response

    def stage_memory(self, turn_id: str, writes: Dict[str, Any]) -> int:
        """Prepare memory writes. Only legal while EXECUTING."""
        self.gate.require_state(turn_id, TurnState.EXECUTING)
        with self.store.transaction():
            n = self.store.stage_memory(turn_id, writes)
            self.wal.append(WalEvent.MEMORY_PREPARED, turn_id,
                            {"keys": sorted(writes)})
        return n
