//! Typed node-task delegation: the light/heavy split in code.
//!
//! `POST /task` accepts a [`kamimusuhi_node::TaskSubmission`]. A rule-based
//! classifier decides `LOCAL` (run here), `DELEGATE` (a capable peer runs
//! it), `QUEUE` (no capable peer is healthy — wait for one) or `REJECT`
//! (no configured node can ever serve it). The decision is capability-
//! driven: `task.requires` is matched against advertised capabilities
//! (live `/health` data first, static `peers[].capabilities` fallback),
//! never against machine names.
//!
//! Every accepted task is persisted in `current_state/delegations.sqlite`
//! before it runs, so a restart loses nothing: rows in `delegating`,
//! `running_remote` or `running` reopen as `retry_pending` and resume —
//! remote execution is idempotent on `task_id`, so re-submitting after a
//! disconnect returns the peer's own record instead of duplicating work.
//!
//! Task states follow the node protocol (`queued → delegating →
//! running_remote → completed | failed | timeout`, `retry_pending` on
//! recoverable failure). A task is never an infinite wait: its deadline
//! (`timeout_ms`) bounds queueing, submission and remote execution.
//!
//! What is stored where is deliberate: `payload`/`result` may carry
//! private content and live only in this node-local database. The task
//! board and the `logs/delegations` journal — which sync to the NAS —
//! carry ids, types, states and latencies, never bodies.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_node::{NodeMode, TaskRecord, TaskState, TaskSubmission};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::capabilities;
use crate::state::Shared;
use crate::util::{unix_now, unix_now_ms};

/// Stored result bodies are capped: full LLM output can be large, and this
/// database is a coordination store, not an archive.
const MAX_RESULT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Submitted here, executed remotely (or queued for a peer).
    Outbound,
    /// Submitted for this node — either by a peer (target_node = us) or a
    /// caller that landed here and classified local.
    Inbound,
}

impl Direction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "outbound" => Self::Outbound,
            _ => Self::Inbound,
        }
    }
}

/// A persisted node task (SQLite row).
#[derive(Debug, Clone)]
pub struct Row {
    pub task_id: String,
    pub direction: Direction,
    pub task_type: String,
    pub requires: Vec<String>,
    pub payload: Value,
    pub state: TaskState,
    pub node: Option<String>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub timeout_ms: u64,
    /// Unix seconds: queueing + submission + remote execution all bound
    /// by this.
    pub deadline: u64,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Task-board id for outbound delegations (existing tasks.json UI).
    pub board_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub next_attempt_at: u64,
}

impl Row {
    pub fn record(&self) -> TaskRecord {
        TaskRecord {
            task_id: self.task_id.clone(),
            task_type: self.task_type.clone(),
            state: self.state,
            node: self.node.clone(),
            result: self.result.clone(),
            error: self.error.clone(),
            attempts: self.attempts,
            created_at: self.created_at,
            updated_at: self.updated_at,
            next_attempt_at: (self.next_attempt_at > 0).then_some(self.next_attempt_at),
        }
    }

    /// View for lists/status: metadata only — `payload` and `result` are
    /// node-local detail, not listing content.
    pub fn view(&self) -> Value {
        json!({
            "task_id": self.task_id,
            "type": self.task_type,
            "direction": self.direction.as_str(),
            "state": self.state.as_str(),
            "node": self.node,
            "attempts": self.attempts,
            "error": self.error,
            "created_at": crate::util::iso8601(self.created_at),
            "updated_at": crate::util::iso8601(self.updated_at),
        })
    }
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let state = r.get::<_, String>(5)?;
    let parse_state =
        || serde_json::from_value::<TaskState>(json!(state)).unwrap_or(TaskState::Failed);
    Ok(Row {
        task_id: r.get(0)?,
        direction: Direction::parse(&r.get::<_, String>(1)?),
        task_type: r.get(2)?,
        requires: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
        state: parse_state(),
        node: r.get(6)?,
        attempts: r.get::<_, i64>(7)? as u32,
        max_attempts: r.get::<_, i64>(8)? as u32,
        timeout_ms: r.get::<_, i64>(9)? as u64,
        deadline: r.get::<_, i64>(10)? as u64,
        result: r
            .get::<_, Option<String>>(11)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        error: r.get(12)?,
        board_id: r.get(13)?,
        created_at: r.get::<_, i64>(14)? as u64,
        updated_at: r.get::<_, i64>(15)? as u64,
        next_attempt_at: r.get::<_, i64>(16)? as u64,
    })
}

const COLUMNS: &str = "task_id, direction, type, requires, payload, state, node, attempts,
    max_attempts, timeout_ms, deadline, result, error, board_id, created_at, updated_at,
    next_attempt_at";

#[derive(Default)]
struct Stats {
    submitted: u64,
    completed: u64,
    failed: u64,
    timeouts: u64,
    retries: u64,
    /// EMA-ish running latency of completed outbound tasks.
    last_latency_ms: u64,
    last_error: Option<String>,
    last_error_at: u64,
}

/// Durable task queue plus the wake channel workers block on.
pub struct DelegationStore {
    db: Mutex<Option<Connection>>,
    wake: Condvar,
    stats: Mutex<Stats>,
    /// Health of persistence itself; false degrades submits to 503 instead
    /// of pretending the queue exists.
    healthy: AtomicBool,
}

impl DelegationStore {
    pub fn open(path: &Path) -> Self {
        let opened = (|| -> Result<Connection, String> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let db = Connection::open(path).map_err(|e| e.to_string())?;
            db.busy_timeout(Duration::from_millis(250))
                .map_err(|e| e.to_string())?;
            db.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
                CREATE TABLE IF NOT EXISTS node_tasks(
                    task_id TEXT PRIMARY KEY,
                    direction TEXT NOT NULL,
                    type TEXT NOT NULL,
                    requires TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    state TEXT NOT NULL,
                    node TEXT,
                    attempts INTEGER NOT NULL,
                    max_attempts INTEGER NOT NULL,
                    timeout_ms INTEGER NOT NULL,
                    deadline INTEGER NOT NULL,
                    result TEXT,
                    error TEXT,
                    board_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    next_attempt_at INTEGER NOT NULL DEFAULT 0);
                CREATE TABLE IF NOT EXISTS node_task_events(
                    id INTEGER PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    at INTEGER NOT NULL,
                    event TEXT NOT NULL,
                    detail TEXT);
                CREATE INDEX IF NOT EXISTS node_task_events_id ON node_task_events(task_id);",
            )
            .map_err(|e| e.to_string())?;
            Ok(db)
        })();
        let healthy = opened.is_ok();
        let store = Self {
            db: Mutex::new(opened.ok()),
            wake: Condvar::new(),
            stats: Mutex::new(Stats::default()),
            healthy: AtomicBool::new(healthy),
        };
        if healthy {
            store.recover();
        }
        store
    }

    /// Tasks interrupted mid-flight by a restart are not trusted — the
    /// executor may have died with us — so they become retryable. Remote
    /// side reconciles via the idempotent `task_id`; local side simply
    /// re-executes.
    fn recover(&self) {
        let now = unix_now();
        let mut guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let Some(db) = guard.as_mut() else { return };
        let stuck: Vec<(String, String)> = db
            .prepare(
                "SELECT task_id, state FROM node_tasks
                    WHERE state IN ('delegating','running_remote','running')",
            )
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();
        for (id, was) in &stuck {
            let _ = db.execute(
                "UPDATE node_tasks SET state='retry_pending', next_attempt_at=?2,
                    updated_at=?2, error=NULL WHERE task_id=?1",
                params![id, now as i64],
            );
            let _ = db.execute(
                "INSERT INTO node_task_events(task_id, at, event, detail)
                    VALUES(?1, ?2, 'recovered', ?3)",
                params![id, now as i64, was],
            );
        }
        if !stuck.is_empty() {
            eprintln!(
                "delegation: recovered {} task(s) to retry_pending",
                stuck.len()
            );
        }
    }

    fn usable(&self) -> bool {
        self.healthy.load(AtomicOrdering::Relaxed)
    }

    /// Insert a new task. `false` when `task_id` already exists — the
    /// existing record stands (idempotency).
    fn insert(&self, row: &Row) -> Result<bool, String> {
        let mut guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let db = guard.as_mut().ok_or("delegation store unavailable")?;
        let n = db
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO node_tasks({COLUMNS})
                        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)"
                ),
                params![
                    row.task_id,
                    row.direction.as_str(),
                    row.task_type,
                    serde_json::to_string(&row.requires).unwrap_or_default(),
                    serde_json::to_string(&row.payload).unwrap_or_default(),
                    row.state.as_str(),
                    row.node,
                    row.attempts as i64,
                    row.max_attempts as i64,
                    row.timeout_ms as i64,
                    row.deadline as i64,
                    row.result
                        .as_ref()
                        .map(|v| bounded_json(v, MAX_RESULT_BYTES)),
                    row.error,
                    row.board_id,
                    row.created_at as i64,
                    row.updated_at as i64,
                    row.next_attempt_at as i64,
                ],
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            let _ = db.execute(
                "INSERT INTO node_task_events(task_id, at, event, detail)
                    VALUES(?1, ?2, 'created', ?3)",
                params![row.task_id, row.created_at as i64, row.task_type],
            );
        }
        drop(guard);
        if n > 0 {
            self.wake.notify_all();
        }
        Ok(n > 0)
    }

    pub fn get(&self, task_id: &str) -> Option<Row> {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let db = guard.as_ref()?;
        db.query_row(
            &format!("SELECT {COLUMNS} FROM node_tasks WHERE task_id=?1"),
            params![task_id],
            row_from,
        )
        .optional()
        .unwrap_or(None)
    }

    /// Move `task_id` to `to`, recording the event. Returns false when the
    /// row is already terminal or missing.
    fn transition(&self, task_id: &str, t: &Transition<'_>) -> bool {
        let (to, node, error, result, next_attempt_at, attempts, note) = (
            t.to,
            t.node,
            t.error,
            t.result,
            t.next_attempt_at,
            t.attempts,
            t.note,
        );
        let now = unix_now();
        let mut guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let Some(db) = guard.as_mut() else {
            return false;
        };
        let current: Option<String> = db
            .query_row(
                "SELECT state FROM node_tasks WHERE task_id=?1",
                params![task_id],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        let Some(from) = current else { return false };
        let from_state =
            serde_json::from_value::<TaskState>(json!(from)).unwrap_or(TaskState::Failed);
        if from_state.terminal() {
            return false;
        }
        let changed = db
            .execute(
                "UPDATE node_tasks SET state=?2, node=COALESCE(?3, node),
                    error=?4, result=COALESCE(?5, result), updated_at=?6,
                    next_attempt_at=?7, attempts=COALESCE(?8, attempts)
                    WHERE task_id=?1",
                params![
                    task_id,
                    to.as_str(),
                    node,
                    error,
                    result.map(|v| bounded_json(v, MAX_RESULT_BYTES)),
                    now as i64,
                    next_attempt_at as i64,
                    attempts.map(|a| a as i64),
                ],
            )
            .map_err(|e| e.to_string());
        let Ok(1) = changed else { return false };
        let _ = db.execute(
            "INSERT INTO node_task_events(task_id, at, event, detail) VALUES(?1,?2,?3,?4)",
            params![task_id, now as i64, to.as_str(), note],
        );
        drop(guard);
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        match to {
            TaskState::Completed => stats.completed += 1,
            TaskState::Failed => {
                stats.failed += 1;
                stats.last_error = error.map(str::to_owned);
                stats.last_error_at = now;
            }
            TaskState::Timeout => stats.timeouts += 1,
            TaskState::RetryPending => stats.retries += 1,
            _ => {}
        }
        if to == TaskState::RetryPending
            && let Some(e) = error
        {
            stats.last_error = Some(e.to_owned());
            stats.last_error_at = now;
        }
        drop(stats);
        self.wake.notify_all();
        true
    }

    /// Claim the next runnable task for a worker: expired tasks are swept
    /// to `timeout` first, then the oldest due `queued`/`retry_pending`
    /// row moves to `delegating` (outbound) or `running` (inbound).
    fn claim(&self) -> Option<Row> {
        let now = unix_now();
        let mut guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let db = guard.as_mut()?;
        let tx = db.transaction().ok()?;
        // Sweep tasks whose deadline elapsed while queued or waiting.
        let _ = tx.execute(
            "UPDATE node_tasks SET state='timeout', error='deadline', updated_at=?1
                WHERE deadline < ?1 AND state IN ('queued','retry_pending')",
            params![now as i64],
        );
        let row: Row = tx
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM node_tasks WHERE
                        (state='queued' OR (state='retry_pending' AND next_attempt_at<=?1))
                        ORDER BY created_at LIMIT 1"
                ),
                params![now as i64],
                row_from,
            )
            .optional()
            .ok()
            .flatten()?;
        let next = match row.direction {
            Direction::Outbound => TaskState::Delegating,
            Direction::Inbound => TaskState::Running,
        };
        let claimed = tx
            .execute(
                "UPDATE node_tasks SET state=?2, updated_at=?3 WHERE task_id=?1
                    AND state IN ('queued','retry_pending')",
                params![row.task_id, next.as_str(), now as i64],
            )
            .unwrap_or(0);
        if claimed != 1 {
            return None;
        }
        let _ = tx.execute(
            "INSERT INTO node_task_events(task_id, at, event, detail) VALUES(?1,?2,?3,'')",
            params![row.task_id, now as i64, next.as_str()],
        );
        let _ = tx.commit();
        drop(guard);
        self.get(&row.task_id)
    }

    /// Block until any transition happens or `dur` elapses.
    fn wait_work(&self, dur: Duration) {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let _ = self
            .wake
            .wait_timeout(guard, dur)
            .unwrap_or_else(|p| p.into_inner());
    }

    /// Block until `task_id` reaches a terminal state or `wait` elapses.
    /// Public for tests and operator waits; returns the last known row.
    pub fn wait_terminal(&self, task_id: &str, wait: Duration) -> Option<Row> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(row) = self.get(task_id)
                && row.state.terminal()
            {
                return Some(row);
            }
            let now = Instant::now();
            if now >= deadline {
                return self.get(task_id);
            }
            let step = (deadline - now).min(Duration::from_millis(50));
            let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
            let _ = self
                .wake
                .wait_timeout(guard, step)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Tasks actively moving right now (claimed by a worker or remote).
    pub fn in_flight_count(&self) -> usize {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .as_ref()
            .and_then(|db| {
                db.query_row(
                    "SELECT count(*) FROM node_tasks WHERE state IN
                        ('delegating','running_remote','running')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0) as usize
    }

    /// Non-terminal tasks (queue length + in flight, both directions).
    pub fn pending_count(&self) -> usize {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .as_ref()
            .and_then(|db| {
                db.query_row(
                    "SELECT count(*) FROM node_tasks WHERE state NOT IN
                        ('completed','failed','timeout','cancelled')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0) as usize
    }

    fn open_outbound(&self) -> usize {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .as_ref()
            .and_then(|db| {
                db.query_row(
                    "SELECT count(*) FROM node_tasks WHERE direction='outbound' AND
                        state NOT IN ('completed','failed','timeout','cancelled')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0) as usize
    }

    fn recent(&self, limit: usize) -> Vec<Row> {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let Some(db) = guard.as_ref() else {
            return Vec::new();
        };
        let mut stmt = match db.prepare(&format!(
            "SELECT {COLUMNS} FROM node_tasks ORDER BY created_at DESC LIMIT ?1"
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![limit as i64], row_from)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Snapshot for `/status` and `kamimusuhi status`.
    pub fn status(&self) -> Value {
        let counts: Vec<(String, i64)> = {
            let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
            guard
                .as_ref()
                .and_then(|db| {
                    let mut stmt = db
                        .prepare("SELECT state, count(*) FROM node_tasks GROUP BY state")
                        .ok()?;
                    let rows = stmt
                        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                        .ok()?;
                    Some(rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default()
        };
        let stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        json!({
            "enabled": self.usable(),
            "pending": self.pending_count(),
            "by_state": counts
                .into_iter()
                .map(|(k, v)| (k, json!(v)))
                .collect::<serde_json::Map<_, _>>(),
            "submitted": stats.submitted,
            "completed": stats.completed,
            "failed": stats.failed,
            "timeouts": stats.timeouts,
            "retries": stats.retries,
            "last_latency_ms": stats.last_latency_ms,
            "last_error": stats.last_error,
            "last_error_at": (stats.last_error_at > 0)
                .then(|| crate::util::iso8601(stats.last_error_at)),
            "recent": self.recent(10).iter().map(Row::view).collect::<Vec<_>>(),
        })
    }
}

fn bounded_json(v: &Value, cap: usize) -> String {
    let s = v.to_string();
    if s.len() <= cap {
        return s;
    }
    let mut cut = cap;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    json!({"truncated": true, "preview": &s[..cut]}).to_string()
}

/// Bundle of state-transition fields (keeps `transition` calls readable).
pub struct Transition<'a> {
    pub to: TaskState,
    pub node: Option<&'a str>,
    pub error: Option<&'a str>,
    pub result: Option<&'a Value>,
    pub next_attempt_at: u64,
    pub attempts: Option<u32>,
    pub note: &'a str,
}

impl<'a> Transition<'a> {
    fn to(state: TaskState) -> Self {
        Self {
            to: state,
            node: None,
            error: None,
            result: None,
            next_attempt_at: 0,
            attempts: None,
            note: "",
        }
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn new_task_id() -> String {
    let seq = SEQ.fetch_add(1, AtomicOrdering::Relaxed);
    format!("nt-{:x}-{:x}", unix_now_ms(), seq)
}

/// A peer's standing offer to the classifier.
pub struct PeerOffer {
    pub id: String,
    pub priority: u32,
    pub healthy: bool,
    /// `Some` once capabilities are known (live probe or static config).
    pub caps: Option<Vec<String>>,
}

/// The classifier's verdict (the spec's LOCAL/DELEGATE/QUEUE/REJECT).
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Local,
    Remote(String),
    Queue,
    Reject(String),
}

/// Rule-based routing for a submission.
///
/// 1. `target_node` pins execution: `== self` must run locally (a peer
///    forwarding work marks us as the target); naming another node routes
///    straight there when that peer is configured and can serve the
///    requirements — and nowhere else, so delegations never chain.
/// 2. Node-local types (`node.*`, `state.*`, `queue.*`, `heartbeat`) never
///    leave the node.
/// 3. Light nodes prefer a healthy capable peer over local execution —
///    Pi's job is continuity, not heavy work. Heavy nodes prefer local
///    execution and only delegate types they cannot serve.
/// 4. Capable-but-unhealthy peers queue the task for retry.
/// 5. No capable node at all rejects it — better refused than parked.
pub fn decide(
    self_id: &str,
    mode: NodeMode,
    local_caps: &BTreeSet<String>,
    submission: &TaskSubmission,
    peers: &[PeerOffer],
) -> Decision {
    let requires = &submission.requires;
    let local_sat =
        capabilities::satisfy(&local_caps.iter().cloned().collect::<Vec<_>>(), requires);
    if let Some(target) = &submission.target_node {
        if target == self_id {
            return if local_sat || capabilities::is_local_type(&submission.task_type) {
                Decision::Local
            } else {
                Decision::Reject("unsupported_capability".into())
            };
        }
        let Some(peer) = peers.iter().find(|p| &p.id == target) else {
            return Decision::Reject(format!("unknown_node:{target}"));
        };
        if let Some(caps) = &peer.caps
            && !capabilities::satisfy(caps, requires)
        {
            return Decision::Reject(format!("unsupported_capability:{target}"));
        }
        return Decision::Remote(target.clone());
    }
    if capabilities::is_local_type(&submission.task_type) {
        return if local_sat {
            Decision::Local
        } else {
            Decision::Reject("unsupported_capability".into())
        };
    }
    let mut healthy: Vec<&PeerOffer> = peers
        .iter()
        .filter(|p| {
            p.healthy
                && p.caps
                    .as_ref()
                    .is_some_and(|c| capabilities::satisfy(c, requires))
        })
        .collect();
    healthy.sort_by(|a, b| b.priority.cmp(&a.priority));
    let capable = peers.iter().any(|p| {
        p.caps
            .as_ref()
            .is_some_and(|c| capabilities::satisfy(c, requires))
    });
    match mode {
        NodeMode::Light => {
            if let Some(peer) = healthy.first() {
                Decision::Remote(peer.id.clone())
            } else if local_sat {
                Decision::Local
            } else if capable {
                Decision::Queue
            } else {
                Decision::Reject("no_capable_node".into())
            }
        }
        NodeMode::Heavy => {
            if local_sat {
                Decision::Local
            } else if let Some(peer) = healthy.first() {
                Decision::Remote(peer.id.clone())
            } else if capable {
                Decision::Queue
            } else {
                Decision::Reject("no_capable_node".into())
            }
        }
    }
}

fn peer_offers(shared: &Shared) -> Vec<PeerOffer> {
    shared
        .config
        .peers
        .iter()
        .map(|p| PeerOffer {
            id: p.id.clone(),
            priority: p.priority,
            healthy: shared.peer_healthy(&p.id),
            caps: capabilities::of_peer(shared, p),
        })
        .collect()
}

/// `POST /task` and operator submissions land here.
pub fn submit(shared: &Shared, body: &Value) -> (u16, Value) {
    let Ok(sub) = serde_json::from_value::<TaskSubmission>(body.clone()) else {
        return (400, json!({"error": "invalid task submission"}));
    };
    if !shared.config.delegation.enabled || !shared.delegations.usable() {
        return (503, json!({"error": "delegation is unavailable"}));
    }
    if let Err(reason) = kamimusuhi_node::validate_submission(&sub) {
        return (400, json!({"error": reason}));
    }
    let task_id = sub.task_id.clone().unwrap_or_else(new_task_id);
    // Idempotent resume: the same task_id returns the standing record.
    if let Some(existing) = shared.delegations.get(&task_id) {
        let mut v = serde_json::to_value(existing.record()).unwrap_or_default();
        v["deduplicated"] = json!(true);
        return (200, v);
    }
    let config = &shared.config.delegation;
    let timeout_ms = sub
        .timeout_ms
        .unwrap_or(config.default_timeout_ms)
        .min(config.max_timeout_ms);
    let local_caps = capabilities::local(&shared.config);
    let mode = capabilities::mode(&shared.config);
    let decision = decide(
        &shared.config.node.id,
        mode,
        &local_caps,
        &sub,
        &peer_offers(shared),
    );
    let (direction, target) = match &decision {
        Decision::Reject(reason) => {
            return (422, json!({"error": reason, "type": sub.task_type}));
        }
        Decision::Local => (Direction::Inbound, None),
        Decision::Remote(peer) => (Direction::Outbound, Some(peer.clone())),
        Decision::Queue => (Direction::Outbound, None),
    };
    if direction == Direction::Outbound && shared.delegations.open_outbound() >= config.max_queue {
        return (429, json!({"error": "queue_full"}));
    }
    let now = unix_now();
    let row = Row {
        task_id: task_id.clone(),
        direction,
        task_type: sub.task_type.clone(),
        requires: sub.requires.clone(),
        payload: sub.payload.clone(),
        state: TaskState::Queued,
        node: target,
        attempts: 0,
        max_attempts: config.max_attempts,
        timeout_ms,
        deadline: now + timeout_ms.div_ceil(1000),
        result: None,
        error: None,
        board_id: None,
        created_at: now,
        updated_at: now,
        next_attempt_at: 0,
    };
    if let Err(e) = shared.delegations.insert(&row) {
        return (500, json!({"error": e}));
    }
    {
        let mut stats = shared
            .delegations
            .stats
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        stats.submitted += 1;
    }
    if direction == Direction::Outbound {
        let board_id = shared.tasks.create(
            &shared.spool,
            crate::tasks::NewTask {
                title: &format!(
                    "委譲 {} → {}",
                    sub.task_type,
                    row.node.as_deref().unwrap_or("待機")
                ),
                kind: "delegated",
                node: &shared.config.node.id,
                owner: "node",
                status: crate::tasks::TaskStatus::InProgress,
                depends_on: Vec::new(),
                detail: json!({"task_id": task_id, "type": sub.task_type}),
            },
        );
        let _ = shared.delegations.set_board_id(&task_id, &board_id);
        let _ = shared.spool.append(
            "logs/delegations",
            json!({"task_id": task_id, "type": sub.task_type,
                   "event": "accepted", "node": row.node}),
        );
    }
    let wait_ms = sub.wait_ms.unwrap_or(0).min(kamimusuhi_node::MAX_WAIT_MS);
    let row = if wait_ms > 0 {
        shared
            .delegations
            .wait_terminal(&task_id, Duration::from_millis(wait_ms))
            .unwrap_or(row)
    } else {
        shared.delegations.get(&task_id).unwrap_or(row)
    };
    let status = if row.state.terminal() { 200 } else { 202 };
    (
        status,
        serde_json::to_value(row.record()).unwrap_or_default(),
    )
}

/// `GET /task/<id>`.
pub fn show(shared: &Shared, task_id: &str) -> (u16, Value) {
    match shared.delegations.get(task_id) {
        Some(row) => (200, serde_json::to_value(row.record()).unwrap_or_default()),
        None => (404, json!({"error": "not_found"})),
    }
}

/// `POST /v1/delegations` operator actions: list / show / retry / cancel.
pub fn handle(shared: &Shared, request: &Value) -> (u16, Value) {
    match request["action"].as_str().unwrap_or("list") {
        "list" => {
            let limit = usize::try_from(request["limit"].as_u64().unwrap_or(50))
                .unwrap_or(50)
                .clamp(1, 200);
            let rows: Vec<Value> = shared
                .delegations
                .recent(limit)
                .iter()
                .map(Row::view)
                .collect();
            (200, json!({"tasks": rows}))
        }
        "show" => {
            let id = request["task_id"].as_str().unwrap_or("");
            match shared.delegations.get(id) {
                Some(row) => (200, serde_json::to_value(row.record()).unwrap_or_default()),
                None => (404, json!({"error": "not_found"})),
            }
        }
        "retry" => {
            let id = request["task_id"].as_str().unwrap_or("");
            let now = unix_now();
            // Operator retry reopens a terminal failure with a fresh budget.
            let ok = shared.delegations.reopen(id, now);
            if ok {
                let _ = shared.spool.append(
                    "logs/delegations",
                    json!({"task_id": id, "event": "operator_retry"}),
                );
                (200, json!({"task_id": id, "state": "queued"}))
            } else {
                (409, json!({"error": "task is not in a retryable state"}))
            }
        }
        "cancel" => {
            let id = request["task_id"].as_str().unwrap_or("");
            let ok = shared.delegations.transition(
                id,
                &Transition {
                    error: Some("cancelled"),
                    note: "operator",
                    ..Transition::to(TaskState::Cancelled)
                },
            );
            if ok {
                (200, json!({"task_id": id, "state": "cancelled"}))
            } else {
                (409, json!({"error": "task is already terminal or missing"}))
            }
        }
        other => (400, json!({"error": format!("unknown action {other:?}")})),
    }
}

impl DelegationStore {
    fn set_board_id(&self, task_id: &str, board_id: &str) -> Result<(), String> {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let db = guard.as_ref().ok_or("store unavailable")?;
        db.execute(
            "UPDATE node_tasks SET board_id=?2 WHERE task_id=?1",
            params![task_id, board_id],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    /// Reopen a failed/timed-out/cancelled task with attempts reset.
    fn reopen(&self, task_id: &str, now: u64) -> bool {
        let mut guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        let Some(db) = guard.as_mut() else {
            return false;
        };
        let ok = db
            .execute(
                "UPDATE node_tasks SET state='queued', attempts=0, error=NULL,
                    next_attempt_at=0, updated_at=?2,
                    deadline=created_at + CAST((timeout_ms+999)/1000 AS INTEGER)
                    WHERE task_id=?1 AND state IN ('failed','timeout','cancelled')",
                params![task_id, now as i64],
            )
            .unwrap_or(0);
        if ok == 1 {
            let _ = db.execute(
                "INSERT INTO node_task_events(task_id, at, event, detail)
                    VALUES(?1,?2,'reopened','operator')",
                params![task_id, now as i64],
            );
        }
        drop(guard);
        if ok == 1 {
            self.wake.notify_all();
        }
        ok == 1
    }

    /// Update the remote node once delegation has a target.
    fn set_node(&self, task_id: &str, node: &str) {
        let guard = self.db.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(db) = guard.as_ref() {
            let _ = db.execute(
                "UPDATE node_tasks SET node=?2 WHERE task_id=?1",
                params![task_id, node],
            );
        }
    }
}

// ── Worker lifecycle ────────────────────────────────────────────────────

/// Spawn the delegation workers. Called from `serve` startup; a disabled
/// or unhealthy store leaves no threads behind.
pub fn spawn(shared: &Arc<Shared>) {
    if !shared.config.delegation.enabled {
        return;
    }
    let workers = shared.config.delegation.workers.max(1);
    for i in 0..workers {
        let shared = Arc::clone(shared);
        let _ = thread::Builder::new()
            .name(format!("delegate-{i}"))
            .spawn(move || {
                loop {
                    match shared.delegations.claim() {
                        Some(row) => run(&shared, row),
                        None => shared.delegations.wait_work(Duration::from_millis(500)),
                    }
                }
            });
    }
}

fn run(shared: &Shared, row: Row) {
    let started = Instant::now();
    match row.direction {
        Direction::Inbound => run_inbound(shared, &row),
        Direction::Outbound => run_outbound(shared, &row),
    }
    if row.direction == Direction::Outbound {
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut stats = shared
            .delegations
            .stats
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        stats.last_latency_ms = ms;
    }
}

fn run_inbound(shared: &Shared, row: &Row) {
    if unix_now() > row.deadline {
        finish_local(shared, row, TaskState::Timeout, None, Some("deadline"));
        return;
    }
    match execute_local(shared, &row.task_type, &row.payload) {
        Ok(result) => finish_local(shared, row, TaskState::Completed, Some(&result), None),
        Err(e) if e == "timeout" => {
            finish_local(shared, row, TaskState::Timeout, None, Some("timeout"))
        }
        Err(e) => finish_local(shared, row, TaskState::Failed, None, Some(&e)),
    }
}

fn finish_local(
    shared: &Shared,
    row: &Row,
    state: TaskState,
    result: Option<&Value>,
    error: Option<&str>,
) {
    shared.delegations.transition(
        &row.task_id,
        &Transition {
            node: Some(&shared.config.node.id),
            error,
            result,
            note: "local",
            ..Transition::to(state)
        },
    );
}

/// Local executors — what this node can actually run. Keep it small: the
/// light/heavy split exists so the light node *doesn't* grow executors.
fn execute_local(shared: &Shared, task_type: &str, payload: &Value) -> Result<Value, String> {
    match task_type {
        kamimusuhi_node::types::NODE_PING => Ok(json!({
            "pong": true,
            "node": shared.config.node.id,
            "role": shared.config.node.role.as_str(),
            "at": unix_now(),
        })),
        kamimusuhi_node::types::STATE_READ => {
            let section = payload
                .get("section")
                .and_then(Value::as_str)
                .unwrap_or("health");
            match section {
                "health" => Ok(shared.health()),
                "delegations" => Ok(shared.delegations.status()),
                _ => Err("unknown_section".to_owned()),
            }
        }
        kamimusuhi_node::types::LLM_GENERATE => {
            // A peer-delegated generation is served by *local* tiers only:
            // the delegating node keeps its own fallback policy, and this
            // node never pushes another node's private payload upstream.
            let mut body = payload.clone();
            if !body.get("messages").is_some_and(Value::is_array) {
                return Err("payload.messages must be an array".to_owned());
            }
            if body.get("model").is_none() {
                body["model"] = json!("kamimusuhi");
            }
            let routed = crate::router::route(
                shared,
                &crate::router::RouteRequest {
                    body,
                    local_only: true,
                },
            );
            if routed.status == 200 {
                Ok(routed.body)
            } else {
                Err(format!("route_failed:{}", routed.status))
            }
        }
        _ => Err("unsupported_capability".to_owned()),
    }
}

/// Outbound lifecycle: pick a peer, submit (idempotent on task_id), poll
/// until terminal or deadline, leave the record retryable on transport
/// failure.
fn run_outbound(shared: &Shared, row: &Row) {
    let config = &shared.config.delegation;
    let http = Duration::from_secs(config.http_timeout_secs);
    let offers = peer_offers(shared);
    let local_caps = capabilities::local(&shared.config);
    // Re-decide at run time: health may have changed since submission.
    let sub = TaskSubmission {
        task_id: Some(row.task_id.clone()),
        task_type: row.task_type.clone(),
        requires: row.requires.clone(),
        payload: row.payload.clone(),
        timeout_ms: Some(row.timeout_ms),
        target_node: row.node.clone(),
        wait_ms: None,
        reply_to: None,
    };
    let decision = decide(
        &shared.config.node.id,
        capabilities::mode(&shared.config),
        &local_caps,
        &sub,
        &offers,
    );
    let peer_id = match decision {
        Decision::Remote(id) => id,
        // A peer exists but is down: park without burning an attempt. The
        // deadline still bounds the wait.
        Decision::Queue => {
            park(shared, row, "no healthy peer", config);
            return;
        }
        Decision::Local => {
            // Requirements changed (e.g. this node gained the capability):
            // run it here rather than bounce it.
            match execute_local(shared, &row.task_type, &row.payload) {
                Ok(result) => finish_outbound(
                    shared,
                    row,
                    TaskState::Completed,
                    Some(&result),
                    None,
                    config,
                ),
                Err(e) => finish_outbound(shared, row, TaskState::Failed, None, Some(&e), config),
            }
            return;
        }
        Decision::Reject(reason) => {
            finish_outbound(shared, row, TaskState::Failed, None, Some(&reason), config);
            return;
        }
    };
    let Some(peer) = shared.config.peers.iter().find(|p| p.id == peer_id) else {
        finish_outbound(
            shared,
            row,
            TaskState::Failed,
            None,
            Some("unknown_node"),
            config,
        );
        return;
    };
    if !shared.peer_healthy(&peer.id) {
        park(shared, row, "peer unhealthy", config);
        return;
    }
    shared.delegations.set_node(&row.task_id, &peer.id);
    let remaining_ms = row
        .deadline
        .saturating_sub(unix_now())
        .saturating_mul(1000)
        .max(1);
    let submit_body = json!({
        "task_id": row.task_id,
        "type": row.task_type,
        "requires": row.requires,
        "payload": row.payload,
        "timeout_ms": remaining_ms,
        "target_node": peer.id,
    });
    let url = shared.peer_url(peer);
    match post_task(&url, &submit_body, shared.token.as_deref(), http) {
        Ok((status, value)) if (200..300).contains(&status) => {
            let record: Option<TaskRecord> = serde_json::from_value(value).ok();
            match record.as_ref().map(|r| r.state) {
                Some(state) if state.terminal() => {
                    let (result, error) = record.map(|r| (r.result, r.error)).unwrap_or_default();
                    finish_outbound(
                        shared,
                        row,
                        terminal_from(state),
                        result.as_ref(),
                        error.as_deref(),
                        config,
                    );
                }
                Some(_) | None => {
                    shared.delegations.transition(
                        &row.task_id,
                        &Transition {
                            node: Some(&peer.id),
                            note: "submitted",
                            ..Transition::to(TaskState::RunningRemote)
                        },
                    );
                    poll_remote(shared, row, peer, config);
                }
            }
        }
        Ok((429, _)) => park_with_attempt(shared, row, "remote queue_full", config),
        Ok((status, value)) => {
            let err = value["error"].as_str().unwrap_or("rejected").to_owned();
            finish_outbound(
                shared,
                row,
                TaskState::Failed,
                None,
                Some(&format!("rejected:{status}:{err}")),
                config,
            );
        }
        Err(e) => {
            let attempt = row.attempts + 1;
            if attempt >= row.max_attempts {
                finish_outbound(
                    shared,
                    row,
                    TaskState::Failed,
                    None,
                    Some(&format!("transport:{e}")),
                    config,
                );
            } else {
                park_with_attempt(shared, row, &format!("transport:{e}"), config);
            }
        }
    }
}

fn terminal_from(state: TaskState) -> TaskState {
    match state {
        TaskState::Completed => TaskState::Completed,
        TaskState::Timeout => TaskState::Timeout,
        TaskState::Cancelled => TaskState::Cancelled,
        _ => TaskState::Failed,
    }
}

/// Park until a peer can take the task: `retry_pending` without spending
/// an attempt — waiting for a down node is not a failure of the task.
fn park(shared: &Shared, row: &Row, note: &str, config: &crate::config::DelegationConfig) {
    shared.delegations.transition(
        &row.task_id,
        &Transition {
            error: Some(note),
            next_attempt_at: unix_now() + config.retry_secs,
            note: "parked",
            ..Transition::to(TaskState::RetryPending)
        },
    );
}

/// A real attempt failed; bounded retries with backoff.
fn park_with_attempt(
    shared: &Shared,
    row: &Row,
    note: &str,
    config: &crate::config::DelegationConfig,
) {
    let attempt = row.attempts + 1;
    let backoff = config.retry_secs.saturating_mul(u64::from(attempt));
    shared.delegations.transition(
        &row.task_id,
        &Transition {
            error: Some(note),
            next_attempt_at: unix_now() + backoff,
            attempts: Some(attempt),
            note: "retry",
            ..Transition::to(TaskState::RetryPending)
        },
    );
}

/// Poll the remote record until it settles, the task's deadline hits, or
/// consecutive transport failures exceed the poll error budget (the peer
/// may have restarted — a retry_pending re-submit reconciles).
fn poll_remote(
    shared: &Shared,
    row: &Row,
    peer: &crate::config::PeerConfig,
    config: &crate::config::DelegationConfig,
) {
    let http = Duration::from_secs(config.http_timeout_secs);
    let mut errors = 0u32;
    loop {
        if unix_now() > row.deadline {
            finish_outbound(
                shared,
                row,
                TaskState::Timeout,
                None,
                Some("deadline"),
                config,
            );
            return;
        }
        let latest = shared
            .delegations
            .get(&row.task_id)
            .unwrap_or_else(|| row.clone());
        if latest.state.terminal() {
            return; // cancelled meanwhile
        }
        std::thread::sleep(Duration::from_millis(config.poll_ms));
        match get_task(
            &shared.peer_url(peer),
            &row.task_id,
            shared.token.as_deref(),
            http,
        ) {
            Ok((200, value)) => {
                errors = 0;
                let Ok(record) = serde_json::from_value::<TaskRecord>(value) else {
                    continue;
                };
                if record.state.terminal() {
                    finish_outbound(
                        shared,
                        row,
                        terminal_from(record.state),
                        record.result.as_ref(),
                        record.error.as_deref(),
                        config,
                    );
                    return;
                }
            }
            Ok(_) | Err(_) => {
                errors += 1;
                if errors >= config.poll_error_limit {
                    shared.delegations.transition(
                        &row.task_id,
                        &Transition {
                            error: Some("peer unreachable mid-flight"),
                            next_attempt_at: unix_now() + config.retry_secs,
                            note: "poll_failed",
                            ..Transition::to(TaskState::RetryPending)
                        },
                    );
                    return;
                }
            }
        }
    }
}

fn finish_outbound(
    shared: &Shared,
    row: &Row,
    state: TaskState,
    result: Option<&Value>,
    error: Option<&str>,
    _config: &crate::config::DelegationConfig,
) {
    shared.delegations.transition(
        &row.task_id,
        &Transition {
            error,
            result,
            note: "remote",
            ..Transition::to(state)
        },
    );
    let _ = shared.spool.append(
        "logs/delegations",
        json!({"task_id": row.task_id, "type": row.task_type,
               "event": state.as_str(), "node": row.node,
               "attempts": row.attempts, "error": error}),
    );
    if let Some(board_id) = &row.board_id {
        let (status, note) = match state {
            TaskState::Completed => (crate::tasks::TaskStatus::Done, "委譲完了"),
            TaskState::Timeout => (crate::tasks::TaskStatus::Failed, "委譲タイムアウト"),
            _ => (crate::tasks::TaskStatus::Failed, "委譲失敗"),
        };
        shared.tasks.update(
            &shared.spool,
            board_id,
            Some(status),
            Some(("system", note)),
            None,
        );
    }
}

// ── Peer transport ──────────────────────────────────────────────────────

fn post_task(
    url: &str,
    body: &Value,
    token: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
    let endpoint = Endpoint::parse(url, "/task")?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &crate::client::auth(token),
        timeout,
        &TrustAnchors::Webpki,
    )
    .map_err(|e| crate::probes::describe(&e))?;
    let value = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
    Ok((response.status, value))
}

fn get_task(
    url: &str,
    task_id: &str,
    token: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
    let endpoint = Endpoint::parse(url, &format!("/task/{task_id}"))?;
    let response = http::get_json(
        &endpoint,
        &crate::client::auth(token),
        timeout,
        &TrustAnchors::Webpki,
    )
    .map_err(|e| crate::probes::describe(&e))?;
    let value = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
    Ok((response.status, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(task_type: &str, requires: &[&str]) -> TaskSubmission {
        TaskSubmission {
            task_id: None,
            task_type: task_type.into(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            payload: json!({}),
            timeout_ms: None,
            target_node: None,
            wait_ms: None,
            reply_to: None,
        }
    }

    fn peer(id: &str, caps: &[&str], healthy: bool, priority: u32) -> PeerOffer {
        PeerOffer {
            id: id.into(),
            priority,
            healthy,
            caps: Some(caps.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn local(caps: &[&str]) -> BTreeSet<String> {
        caps.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn local_types_stay_local() {
        let peers = [peer("gpu", &["llm.generate"], true, 100)];
        let d = decide(
            "pi",
            NodeMode::Light,
            &local(&["node.ping"]),
            &sub("node.ping", &[]),
            &peers,
        );
        assert_eq!(d, Decision::Local);
    }

    #[test]
    fn light_delegates_when_a_healthy_peer_can_serve() {
        let peers = [peer("gpu", &["llm.generate"], true, 100)];
        let d = decide(
            "pi",
            NodeMode::Light,
            &local(&["node.ping"]),
            &sub("llm.generate", &["llm.generate"]),
            &peers,
        );
        assert_eq!(d, Decision::Remote("gpu".into()));
    }

    #[test]
    fn light_queues_when_capable_peers_are_down() {
        let peers = [peer("gpu", &["llm.generate"], false, 100)];
        let d = decide(
            "pi",
            NodeMode::Light,
            &local(&["node.ping"]),
            &sub("llm.generate", &["llm.generate"]),
            &peers,
        );
        assert_eq!(d, Decision::Queue);
    }

    #[test]
    fn light_rejects_when_nothing_can_ever_serve() {
        let peers = [peer("gpu", &["llm.generate"], false, 100)];
        let d = decide(
            "pi",
            NodeMode::Light,
            &local(&["node.ping"]),
            &sub("vision.scan", &["vision"]),
            &peers,
        );
        assert!(matches!(d, Decision::Reject(_)));
    }

    #[test]
    fn heavy_runs_locally_first() {
        let peers = [peer("pi", &["node.ping"], true, 50)];
        let d = decide(
            "gpu",
            NodeMode::Heavy,
            &local(&["node.ping", "llm.generate"]),
            &sub("llm.generate", &["llm.generate"]),
            &peers,
        );
        assert_eq!(d, Decision::Local);
    }

    #[test]
    fn priority_orders_candidates() {
        let peers = [
            peer("slow", &["llm.generate"], true, 10),
            peer("fast", &["llm.generate"], true, 100),
        ];
        let d = decide(
            "pi",
            NodeMode::Light,
            &local(&[]),
            &sub("llm.generate", &["llm.generate"]),
            &peers,
        );
        assert_eq!(d, Decision::Remote("fast".into()));
    }

    #[test]
    fn target_pins_the_node() {
        let peers = [peer("gpu", &["llm.generate"], true, 100)];
        let mut s = sub("llm.generate", &["llm.generate"]);
        s.target_node = Some("gpu".into());
        let d = decide("pi", NodeMode::Light, &local(&[]), &s, &peers);
        assert_eq!(d, Decision::Remote("gpu".into()));
        // And a task targeted *at* this node runs locally or not at all.
        s.target_node = Some("pi".into());
        let d = decide("pi", NodeMode::Light, &local(&["llm.generate"]), &s, &peers);
        assert_eq!(d, Decision::Local);
        s.target_node = Some("nobody".into());
        let d = decide("pi", NodeMode::Light, &local(&[]), &s, &peers);
        assert!(matches!(d, Decision::Reject(r) if r.starts_with("unknown_node")));
    }

    #[test]
    fn store_round_trip_and_recovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("delegations.sqlite");
        let store = DelegationStore::open(&path);
        let row = Row {
            task_id: "nt-1".into(),
            direction: Direction::Outbound,
            task_type: "llm.generate".into(),
            requires: vec!["llm.generate".into()],
            payload: json!({"messages": []}),
            state: TaskState::Queued,
            node: Some("gpu".into()),
            attempts: 0,
            max_attempts: 6,
            timeout_ms: 60_000,
            deadline: unix_now() + 60,
            result: None,
            error: None,
            board_id: None,
            created_at: unix_now(),
            updated_at: unix_now(),
            next_attempt_at: 0,
        };
        assert!(store.insert(&row).unwrap());
        // Idempotent: a second insert of the same id does not duplicate.
        assert!(!store.insert(&row).unwrap());
        assert_eq!(store.get("nt-1").unwrap().state, TaskState::Queued);
        // Claim marks it delegating.
        let claimed = store.claim().expect("claimable");
        assert_eq!(claimed.state, TaskState::Delegating);
        // Simulate crash: a fresh open flips in-flight to retry_pending.
        drop(store);
        let store = DelegationStore::open(&path);
        assert_eq!(store.get("nt-1").unwrap().state, TaskState::RetryPending);
        // Claimed again, executed, completed.
        let claimed = store.claim().expect("claimable again");
        assert_eq!(claimed.state, TaskState::Delegating);
        store.transition(
            "nt-1",
            &Transition {
                node: Some("gpu"),
                result: Some(&json!({"ok": true})),
                note: "done",
                ..Transition::to(TaskState::Completed)
            },
        );
        let done = store.get("nt-1").unwrap();
        assert_eq!(done.state, TaskState::Completed);
        assert_eq!(done.result.unwrap()["ok"], json!(true));
        // Terminal rows are immutable.
        assert!(!store.transition(
            "nt-1",
            &Transition {
                error: Some("x"),
                note: "nope",
                ..Transition::to(TaskState::Failed)
            },
        ));
    }
}
