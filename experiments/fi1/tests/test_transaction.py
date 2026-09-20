"""State machine + side-effect gate: illegal transitions and pre-READY
side effects are refused at runtime, not just by convention."""
import json

import pytest

from experiments.fi1.protocol.schema import TurnState
from experiments.fi1.protocol.transaction import (InvalidTransition,
                                                  OrphanedEffectError,
                                                  SideEffectGateError)
from experiments.fi1.runtime.engine import TurnEngine


def _receive_only(engine, d):
    """Accept a turn but stop before execution."""
    return engine._receive_inner(d)


def test_happy_path_transitions(engine, make, sink):
    d = make()
    tid = d["turn_id"]
    r = engine.receive(d)
    assert r["status"] == "committed"
    assert engine.gate.state_of(tid) == TurnState.COMMITTED
    events = [e["event"] for e in engine.store.wal_events(tid)]
    assert events == ["TURN_RECEIVED", "TURN_VALIDATED",
                      "EXECUTION_STARTED", "MODEL_PLANNED",
                      "EFFECT_REQUESTED", "EFFECT_COMPLETED",
                      "MEMORY_PREPARED", "TURN_COMMITTED"]
    assert sink.invocation_count == 1
    assert engine.store.count_effects("completed") == 1
    mem = engine.store.memory_for(tid, status="committed")
    assert {m["mkey"] for m in mem} == {"last_reply", "seq:1"}


def test_illegal_transitions_refused(engine, make):
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    gate = engine.gate
    with pytest.raises(InvalidTransition):
        gate.transition(tid, TurnState.RECEIVING)
    with pytest.raises(InvalidTransition):
        gate.transition(tid, TurnState.EXECUTING)
    with pytest.raises(InvalidTransition):
        gate.transition(tid, TurnState.READY)
    assert gate.state_of(tid) == TurnState.COMMITTED


def test_gate_blocks_side_effect_before_executing(db_path, make):
    """A READY (never-executed) turn cannot produce a side effect."""
    engine = TurnEngine(db_path, auto_execute=False)
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    assert engine.gate.state_of(tid) == TurnState.READY
    with pytest.raises(SideEffectGateError):
        engine.broker.invoke_effect(tid, {"effect_id": "x", "kind": "k"})
    with pytest.raises(SideEffectGateError):
        engine.broker.stage_memory(tid, {"k": "v"})
    assert engine.sink.invocation_count == 0   # nothing escaped
    engine.close()


def test_gate_blocks_receiving_turn_effects(db_path, make):
    engine = TurnEngine(db_path, auto_execute=False)
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    # rewind to RECEIVING (still pre-READY): effects must stay gated
    engine.store.set_turn_state(tid, TurnState.RECEIVING.value)
    with pytest.raises(SideEffectGateError):
        engine.broker.invoke_effect(tid, {"effect_id": "x", "kind": "k"})
    engine.close()


def test_no_model_inference_before_ready(db_path, make):
    """The model is only invoked after the EXECUTING claim."""
    engine = TurnEngine(db_path, auto_execute=False)
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    assert engine.model.calls == []
    engine.execute_pending()
    assert engine.model.calls == [tid]
    engine.close()


def test_memory_only_visible_after_commit(engine, make):
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    prepared = engine.store.memory_for(tid, status="prepared")
    committed = engine.store.memory_for(tid, status="committed")
    assert prepared == [] and len(committed) == 2


def test_orphaned_effect_refuses_reinvoke(db_path, make):
    engine = TurnEngine(db_path)
    d = make()
    tid = d["turn_id"]
    engine.receive(d)
    # forge the state a crash leaves: EXECUTING + dangling 'requested' row
    engine.store.set_turn_state(tid, TurnState.EXECUTING.value)
    engine.store.effect_requested(tid, "ghost", "k", {})
    with pytest.raises(OrphanedEffectError):
        engine.broker.invoke_effect(tid, {"effect_id": "ghost",
                                          "kind": "k"})
    engine.close()
