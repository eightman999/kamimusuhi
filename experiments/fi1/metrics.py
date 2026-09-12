"""Primary metrics for Fi1, computed from durable state + the sink's raw log.

- duplicate_executions: EXECUTION_STARTED events beyond the first per turn
  (recovery resume emits EXECUTION_RESUMED, which does not count as a new
  execution — it continues the same transaction).
- duplicate_side_effects: sink invocations beyond applied effects.
- lost_turns: accepted turns stuck in a non-terminal state.
- partial_commits: COMMITTED turns missing committed memory their plan
  promised, or ABORTED turns that somehow hold committed memory. Both are
  impossible by construction (single commit tx); the metric proves it.
- recovery stats, per-receipt latency, WAL overhead counters.
"""

from __future__ import annotations

import json
import statistics
from typing import Any, Dict, List, Optional

from .persistence.store import Store
from .persistence.wal import WalEvent, WalWriter


def _percentiles(xs: List[float]) -> Dict[str, float]:
    if not xs:
        return {"n": 0, "min": 0.0, "mean": 0.0, "p50": 0.0,
                "p95": 0.0, "p99": 0.0, "max": 0.0}
    xs = sorted(xs)
    n = len(xs)

    def pct(p: float) -> float:
        i = min(n - 1, max(0, int(round((p / 100.0) * (n - 1)))))
        return xs[i]

    return {"n": n, "min": xs[0], "mean": statistics.fmean(xs),
            "p50": pct(50), "p95": pct(95), "p99": pct(99),
            "max": xs[-1]}


def collect(store: Store, sink=None,
            receipts: Optional[List[Dict]] = None,
            wal: Optional[WalWriter] = None,
            recovery_reports: Optional[List[Dict]] = None,
            broker=None) -> Dict[str, Any]:
    events = store.wal_events()
    starts: Dict[str, int] = {}
    for e in events:
        if e["event"] == WalEvent.EXECUTION_STARTED.value and e["turn_id"]:
            starts[e["turn_id"]] = starts.get(e["turn_id"], 0) + 1
    executions = sum(starts.values())
    duplicate_executions = sum(max(0, c - 1) for c in starts.values())

    receipts = receipts or []
    by_status: Dict[str, int] = {}
    for r in receipts:
        by_status[r["status"]] = by_status.get(r["status"], 0) + 1

    committed_rows = store.list_turns(state="COMMITTED")
    aborted_rows = store.list_turns(state="ABORTED")
    partial = 0
    for row in committed_rows:
        plan = json.loads(row["plan_json"] or "{}")
        planned = set(plan.get("memory_writes") or [])
        have = {m["mkey"] for m in store.memory_for(row["turn_id"],
                                                  status="committed")}
        if planned and planned - have:
            partial += 1
    for row in aborted_rows:
        if store.memory_for(row["turn_id"], status="committed"):
            partial += 1

    recovery_reports = recovery_reports or []
    m: Dict[str, Any] = {
        "deliveries": len(receipts),
        "receipts_by_status": by_status,
        "turns_total": store.count_turns(),
        "turns_committed": len(committed_rows),
        "turns_aborted": len(aborted_rows),
        "executions": executions,
        "duplicate_executions": duplicate_executions,
        "lost_turns": len(store.nonterminal_turns()),
        "partial_commits": partial,
        "orphaned_effects": store.count_effects("orphaned"),
        "effects_completed": store.count_effects("completed"),
        "effects_requested_open": store.count_effects("requested"),
        "recoveries": len(recovery_reports),
        "recovery_errors": sum(len(r["errors"]) for r in recovery_reports),
        "recovery_aborts": sum(len(r["aborted"]) for r in recovery_reports),
        "latency_ms": _percentiles(
            [r["latency_ms"] for r in receipts if "latency_ms" in r]),
        "wal": {
            "events": len(events),
            "payload_bytes": store.wal_bytes(),
            "by_event": store.wal_event_counts(),
        },
    }
    if sink is not None:
        m["side_effect_invocations"] = sink.invocation_count
        m["side_effect_applied"] = sink.applied_count
        m["duplicate_side_effects"] = sink.duplicate_invocations
    if wal is not None:
        m["wal"]["writer"] = wal.stats()
    if broker is not None:
        m["effects_deduped_on_replay"] = broker.skipped_completed
    return m
