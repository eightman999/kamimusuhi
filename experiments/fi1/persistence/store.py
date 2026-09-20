"""Durable turn/memory/side-effect store on SQLite.

Tables
------
turns         one row per turn_id; state machine column + plan/result JSON
wal_events    append-only logical write-ahead log (see wal.py)
side_effects  one row per (turn_id, effect_id): requested -> completed
memory        staged memory writes: (turn_id, mkey) prepared -> committed
runs          reproducibility record per process that opened the store

Durability: ``PRAGMA journal_mode=WAL`` and ``synchronous=FULL`` (default),
so every ``transaction()`` block ends in an fsync'd commit. A process killed
with SIGKILL at any point leaves the last committed transaction readable;
SQLite's own journal makes each multi-statement block atomic.
"""

from __future__ import annotations

import json
import sqlite3
import threading
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

SCHEMA_VERSION = 1

_SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS runs(
    run_id TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    started_at TEXT NOT NULL,
    git_commit TEXT, branch TEXT,
    python_version TEXT,
    seed INTEGER,
    config_json TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS turns(
    turn_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    state TEXT NOT NULL,
    checksum TEXT,
    turn_json TEXT NOT NULL,
    plan_json TEXT,
    result_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turns_state ON turns(state);
CREATE TABLE IF NOT EXISTS wal_events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    turn_id TEXT,
    event TEXT NOT NULL,
    data_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_wal_turn ON wal_events(turn_id);
CREATE TABLE IF NOT EXISTS side_effects(
    turn_id TEXT NOT NULL,
    effect_id TEXT NOT NULL,
    kind TEXT,
    status TEXT NOT NULL,            -- requested | completed | orphaned
    request_json TEXT,
    response_json TEXT,
    requested_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY(turn_id, effect_id)
);
CREATE TABLE IF NOT EXISTS memory(
    turn_id TEXT NOT NULL,
    mkey TEXT NOT NULL,
    value_json TEXT NOT NULL,
    status TEXT NOT NULL,            -- prepared | committed
    PRIMARY KEY(turn_id, mkey)
);
"""


def utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


class Store:
    """Single-writer SQLite store. All mutating methods join the ambient
    ``transaction()`` if one is open, else commit immediately (fsync)."""

    def __init__(self, path: str | Path, synchronous: str = "FULL"):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.RLock()
        self._tx_depth = 0
        self.conn = sqlite3.connect(str(self.path), check_same_thread=False,
                                    timeout=30)
        self.conn.row_factory = sqlite3.Row
        with self._lock:
            self.conn.execute("PRAGMA journal_mode=WAL")
            self.conn.execute(f"PRAGMA synchronous={synchronous}")
            self.conn.execute("PRAGMA busy_timeout=30000")
            self.conn.executescript(_SCHEMA_SQL)
            row = self.conn.execute(
                "SELECT version FROM schema_version").fetchone()
            if row is None:
                self.conn.execute(
                    "INSERT INTO schema_version(version) VALUES (?)",
                    (SCHEMA_VERSION,))
            self.conn.commit()

    def close(self) -> None:
        with self._lock:
            self.conn.close()

    # ------------------------------------------------------------ util
    def _q(self, sql: str, params: Iterable = ()) -> sqlite3.Cursor:
        return self.conn.execute(sql, tuple(params))

    def _commit(self) -> None:
        if self._tx_depth == 0:
            self.conn.commit()

    @contextmanager
    def transaction(self):
        """Group writes into one SQLite transaction: atomic + one fsync."""
        with self._lock:
            self._tx_depth += 1
            try:
                yield self
            except BaseException:
                self._tx_depth -= 1
                if self._tx_depth == 0:
                    self.conn.rollback()
                raise
            else:
                self._tx_depth -= 1
                if self._tx_depth == 0:
                    self.conn.commit()

    # ------------------------------------------------------------ runs
    def record_run(self, run_id: str, role: str, meta: Dict[str, Any]) -> None:
        with self._lock:
            self._q("INSERT OR REPLACE INTO runs(run_id,role,started_at,"
                    "git_commit,branch,python_version,seed,config_json)"
                    " VALUES(?,?,?,?,?,?,?,?)",
                    (run_id, role, utcnow(), meta.get("git_commit"),
                     meta.get("branch"), meta.get("python_version"),
                     meta.get("seed"), json.dumps(meta.get("config") or {})))
            self._commit()

    # ------------------------------------------------------------ wal
    def _append_wal(self, event: str, turn_id: Optional[str],
                    data: Dict[str, Any]) -> int:
        """Append one WAL record. Joins the ambient transaction if any.
        Returns the serialized payload size (overhead accounting)."""
        payload = json.dumps(data, sort_keys=True)
        self._q("INSERT INTO wal_events(ts,turn_id,event,data_json)"
                " VALUES(?,?,?,?)", (utcnow(), turn_id, event, payload))
        self._commit()
        return len(payload)

    def wal_events(self, turn_id: Optional[str] = None) -> List[Dict]:
        with self._lock:
            if turn_id is None:
                rows = self._q("SELECT * FROM wal_events ORDER BY seq").fetchall()
            else:
                rows = self._q("SELECT * FROM wal_events WHERE turn_id=?"
                               " ORDER BY seq", (turn_id,)).fetchall()
        return [dict(r) for r in rows]

    def wal_event_counts(self) -> Dict[str, int]:
        with self._lock:
            rows = self._q("SELECT event, COUNT(*) c FROM wal_events"
                           " GROUP BY event").fetchall()
        return {r["event"]: r["c"] for r in rows}

    def wal_count_for(self, turn_id: str, event: str) -> int:
        with self._lock:
            return self._q("SELECT COUNT(*) c FROM wal_events WHERE"
                           " turn_id=? AND event=?",
                           (turn_id, event)).fetchone()["c"]

    # ------------------------------------------------------------ turns
    def insert_turn(self, turn_id: str, conversation_id: str, sequence: int,
                    state: str, checksum: Optional[str], turn_json: str,
                    error: Optional[str] = None) -> None:
        now = utcnow()
        with self._lock:
            self._q("INSERT INTO turns(turn_id,conversation_id,sequence,state,"
                    "checksum,turn_json,error,created_at,updated_at)"
                    " VALUES(?,?,?,?,?,?,?,?,?)",
                    (turn_id, conversation_id, sequence, state, checksum,
                     turn_json, error, now, now))
            self._commit()

    def get_turn(self, turn_id: str) -> Optional[Dict]:
        with self._lock:
            row = self._q("SELECT * FROM turns WHERE turn_id=?",
                          (turn_id,)).fetchone()
        return dict(row) if row else None

    def list_turns(self, state: Optional[str] = None) -> List[Dict]:
        with self._lock:
            if state is None:
                rows = self._q("SELECT * FROM turns ORDER BY created_at,"
                               " turn_id").fetchall()
            else:
                rows = self._q("SELECT * FROM turns WHERE state=?"
                               " ORDER BY created_at, turn_id",
                               (state,)).fetchall()
        return [dict(r) for r in rows]

    def set_turn_state(self, turn_id: str, state: str,
                       error: Optional[str] = None) -> None:
        with self._lock:
            self._q("UPDATE turns SET state=?, error=?, updated_at=?"
                    " WHERE turn_id=?", (state, error, utcnow(), turn_id))
            self._commit()

    def claim_for_execution(self, turn_id: str) -> bool:
        """READY -> EXECUTING, atomically. False if not READY."""
        with self._lock:
            cur = self._q("UPDATE turns SET state='EXECUTING', updated_at=?"
                          " WHERE turn_id=? AND state='READY'",
                          (utcnow(), turn_id))
            if cur.rowcount != 1:
                return False
            self._commit()
            return True

    def set_turn_plan(self, turn_id: str, plan_json: str) -> None:
        with self._lock:
            self._q("UPDATE turns SET plan_json=?, updated_at=?"
                    " WHERE turn_id=?", (plan_json, utcnow(), turn_id))
            self._commit()

    def set_turn_result(self, turn_id: str, result_json: str) -> None:
        with self._lock:
            self._q("UPDATE turns SET result_json=?, updated_at=?"
                    " WHERE turn_id=?", (result_json, utcnow(), turn_id))
            self._commit()

    # ------------------------------------------------------------ side effects
    def effect_row(self, turn_id: str, effect_id: str) -> Optional[Dict]:
        with self._lock:
            row = self._q("SELECT * FROM side_effects WHERE turn_id=?"
                          " AND effect_id=?", (turn_id, effect_id)).fetchone()
        return dict(row) if row else None

    def effect_requested(self, turn_id: str, effect_id: str, kind: str,
                         request: Dict[str, Any]) -> None:
        with self._lock:
            self._q("INSERT OR IGNORE INTO side_effects(turn_id,effect_id,"
                    "kind,status,request_json,requested_at)"
                    " VALUES(?,?,?, 'requested', ?, ?)",
                    (turn_id, effect_id, kind, json.dumps(request,
                                                          sort_keys=True),
                     utcnow()))
            self._commit()

    def effect_completed(self, turn_id: str, effect_id: str,
                         response: Dict[str, Any]) -> None:
        with self._lock:
            self._q("UPDATE side_effects SET status='completed',"
                    " response_json=?, completed_at=?"
                    " WHERE turn_id=? AND effect_id=?",
                    (json.dumps(response, sort_keys=True), utcnow(),
                     turn_id, effect_id))
            self._commit()

    def effect_orphaned(self, turn_id: str, effect_id: str) -> None:
        with self._lock:
            self._q("UPDATE side_effects SET status='orphaned'"
                    " WHERE turn_id=? AND effect_id=?",
                    (turn_id, effect_id))
            self._commit()

    def effects_for(self, turn_id: str) -> List[Dict]:
        with self._lock:
            rows = self._q("SELECT * FROM side_effects WHERE turn_id=?"
                           " ORDER BY effect_id", (turn_id,)).fetchall()
        return [dict(r) for r in rows]

    def count_effects(self, status: Optional[str] = None) -> int:
        with self._lock:
            if status is None:
                return self._q("SELECT COUNT(*) c FROM side_effects"
                               ).fetchone()["c"]
            return self._q("SELECT COUNT(*) c FROM side_effects WHERE"
                           " status=?", (status,)).fetchone()["c"]

    # ------------------------------------------------------------ memory
    def stage_memory(self, turn_id: str, writes: Dict[str, Any]) -> int:
        """Idempotent prepare: (turn_id, mkey) upserted as 'prepared'."""
        with self._lock:
            for k in sorted(writes):
                self._q("INSERT OR REPLACE INTO memory(turn_id,mkey,"
                        "value_json,status) VALUES(?,?,?,'prepared')",
                        (turn_id, k, json.dumps(writes[k], sort_keys=True)))
            self._commit()
        return len(writes)

    def commit_memory(self, turn_id: str) -> int:
        with self._lock:
            cur = self._q("UPDATE memory SET status='committed'"
                          " WHERE turn_id=? AND status='prepared'",
                          (turn_id,))
            self._commit()
            return cur.rowcount

    def discard_prepared_memory(self, turn_id: str) -> int:
        with self._lock:
            cur = self._q("DELETE FROM memory WHERE turn_id=?"
                          " AND status='prepared'", (turn_id,))
            self._commit()
            return cur.rowcount

    def memory_for(self, turn_id: str,
                   status: Optional[str] = None) -> List[Dict]:
        with self._lock:
            if status is None:
                rows = self._q("SELECT * FROM memory WHERE turn_id=?"
                               " ORDER BY mkey", (turn_id,)).fetchall()
            else:
                rows = self._q("SELECT * FROM memory WHERE turn_id=?"
                               " AND status=? ORDER BY mkey",
                               (turn_id, status)).fetchall()
        return [dict(r) for r in rows]

    # ------------------------------------------------------------ metrics
    def count_turns(self, state: Optional[str] = None) -> int:
        with self._lock:
            if state is None:
                return self._q("SELECT COUNT(*) c FROM turns").fetchone()["c"]
            return self._q("SELECT COUNT(*) c FROM turns WHERE state=?",
                           (state,)).fetchone()["c"]

    def nonterminal_turns(self) -> List[Dict]:
        with self._lock:
            rows = self._q("SELECT * FROM turns WHERE state NOT IN"
                           " ('COMMITTED','ABORTED') ORDER BY turn_id"
                           ).fetchall()
        return [dict(r) for r in rows]

    def wal_bytes(self) -> int:
        with self._lock:
            row = self._q("SELECT COALESCE(SUM(LENGTH(data_json)),0) b"
                          " FROM wal_events").fetchone()
        return int(row["b"])
