"""Startup recovery: replay the WAL and finish or abort in-flight turns.

Policy (explicit):

- RECEIVING turn at crash -> re-validate the stored envelope; it continues
  as READY or is ABORTED as rejected. Nothing was executed yet, so this is
  always safe.
- READY turn at crash -> executed normally (no side effect ran yet).
- EXECUTING turn at crash -> resumed in place (EXECUTION_RESUMED, not a new
  EXECUTION_STARTED — the same transaction continues):
    * recorded plan is reused; if the crash preceded MODEL_PLANNED the plan
      is re-derived (model is deterministic + side-effect-free, gate still
      requires EXECUTING).
    * effects recorded 'completed' are NOT re-invoked — the recorded
      response is returned (durable dedupe on (turn_id, effect_id)).
    * effects 'requested' but never 'completed' are AMBIGUOUS: the old
      process may or may not have invoked the sink. Policy ``abort``
      (default): mark the effect 'orphaned', discard any prepared memory,
      abort the turn. It is never re-invoked -> zero duplicate effects.
      Policy ``resume``: re-invoke through the idempotent sink, which
      dedupes on the effect key (counts a duplicate *invocation* but no
      duplicate *applied* effect).
- Memory 'prepared' but never committed -> rolled forward when the turn
  commits, discarded when it aborts. Memory can only become committed
  inside the single commit transaction, so partial commits are impossible
  by construction (SQLite atomicity).
- COMMITTED/ABORTED turns are terminal: never re-executed.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Optional

from ..persistence.store import Store
from ..persistence.wal import WalEvent, WalWriter
from ..protocol.schema import TurnState
from ..protocol.transaction import OrphanedEffectError, SideEffectBroker
from ..protocol.validator import ValidationError, validate_envelope


def recover(store: Store, wal: WalWriter, executor, broker: SideEffectBroker,
            orphan_policy: str = "abort",
            execute_ready: bool = True) -> Dict[str, Any]:
    """Replay WAL and settle every non-terminal turn. Returns a report."""
    assert orphan_policy in ("abort", "resume")
    report: Dict[str, Any] = {
        "orphan_policy": orphan_policy,
        "committed": [], "aborted": [], "rejected": [],
        "orphaned_effects": [], "resumed": [],
        "discarded_prepares": 0, "errors": [],
    }
    wal.append(WalEvent.RECOVERY_STARTED, None,
               {"orphan_policy": orphan_policy})

    for row in store.nonterminal_turns():
        tid = row["turn_id"]
        state = row["state"]
        try:
            if state == TurnState.RECEIVING.value:
                _recover_receiving(store, wal, row, report)
                state = TurnState.READY.value \
                    if store.get_turn(tid)["state"] == "READY" else None
            if state == TurnState.READY.value and execute_ready:
                executor.execute(tid)
                report["committed"].append(tid)
                continue
            if state == TurnState.EXECUTING.value:
                _recover_executing(store, wal, executor, broker, row,
                                   orphan_policy, report)
        except Exception as e:  # keep recovering other turns
            report["errors"].append({"turn_id": tid, "error": repr(e)})

    report["remaining_nonterminal"] = [r["turn_id"]
                                       for r in store.nonterminal_turns()]
    wal.append(WalEvent.RECOVERY_COMPLETED, None,
               {"committed": len(report["committed"]),
                "aborted": len(report["aborted"]),
                "errors": len(report["errors"])})
    return report


def _recover_receiving(store: Store, wal: WalWriter, row: Dict,
                       report: Dict) -> None:
    tid = row["turn_id"]
    try:
        validate_envelope(json.loads(row["turn_json"]))
    except (ValidationError, json.JSONDecodeError) as e:
        reason = getattr(e, "reason", "unparseable")
        with store.transaction():
            store.set_turn_state(tid, TurnState.ABORTED.value,
                                 f"rejected:{reason}")
            wal.append(WalEvent.TURN_ABORTED, tid,
                       {"reason": f"rejected:{reason}"})
        report["rejected"].append(tid)
        return
    with store.transaction():
        store.set_turn_state(tid, TurnState.READY.value)
        wal.append(WalEvent.TURN_VALIDATED, tid, {"via": "recovery"})


def _recover_executing(store: Store, wal: WalWriter, executor,
                       broker: SideEffectBroker, row: Dict,
                       orphan_policy: str, report: Dict) -> None:
    tid = row["turn_id"]
    wal.append(WalEvent.EXECUTION_RESUMED, tid, {})
    report["resumed"].append(tid)
    allow_reinvoke = orphan_policy == "resume"
    try:
        if row["plan_json"] is None:
            executor._run_executing(tid, allow_reinvoke=allow_reinvoke)
        else:
            executor._run_effects_and_commit(
                tid, json.loads(row["plan_json"]),
                allow_reinvoke=allow_reinvoke)
        report["committed"].append(tid)
    except OrphanedEffectError as e:
        # Ambiguous effect: never re-invoked. Conservative abort.
        for er in store.effects_for(tid):
            if er["status"] == "requested":
                store.effect_orphaned(tid, er["effect_id"])
                wal.append(WalEvent.EFFECT_ORPHANED, tid,
                           {"effect_id": er["effect_id"]})
                report["orphaned_effects"].append(
                    {"turn_id": tid, "effect_id": er["effect_id"]})
        with store.transaction():
            n = store.discard_prepared_memory(tid)
            if n:
                wal.append(WalEvent.MEMORY_DISCARDED, tid, {"n": n})
            store.set_turn_state(tid, TurnState.ABORTED.value,
                                 f"orphaned_effect:{e}")
            wal.append(WalEvent.TURN_ABORTED, tid,
                       {"reason": "orphaned_effect"})
        report["discarded_prepares"] += n
        report["aborted"].append(tid)
