//! Shared task protocol between Kamimusuhi nodes.
//!
//! A "light" node (continuity role — e.g. a Raspberry Pi that holds runtime
//! and session continuity) delegates heavy work to "heavy" nodes (cognition
//! role — GPU machines or external providers) through typed tasks. Both
//! directions speak the same wire format defined here:
//!
//! - `POST /task` accepts a [`TaskSubmission`], idempotent on `task_id`,
//!   and answers with a [`TaskRecord`]. `wait_ms` asks the receiver to
//!   hold the request until the task settles or the wait elapses.
//! - `GET /task/<task_id>` returns the current [`TaskRecord`].
//! - `GET /capabilities` returns a [`CapabilityDoc`]; light nodes route by
//!   matching `submission.requires` against advertised capabilities, not by
//!   machine names, so future executors (GPU nodes, V100, cloud APIs,
//!   GrokBot, HAI) join the same protocol without code changes.
//!
//! This crate is wire contracts only: no HTTP, no SQLite, no threads. The
//! resident daemon implements the endpoints and the delegation lifecycle.
//!
//! Privacy note: `payload`/`result` may carry private content. Records live
//! in node-local storage; journals and metrics must carry identifiers and
//! outcomes only, never bodies.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Whether a node keeps continuity (light) or executes heavy work (heavy).
///
/// Derived from the resident node's configured `role`: `continuity` maps to
/// `Light`, everything else (`cognition`, future executor roles) to `Heavy`.
/// A light node keeps session/goal/task state and never blocks its event
/// loop on remote work; a heavy node executes delegated tasks and holds no
/// canonical state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeMode {
    Light,
    Heavy,
}

impl NodeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Heavy => "heavy",
        }
    }
}

/// Lifecycle of a node task. Both ends share one enum so each side can
/// parse the other's record; each side only emits the subset it owns.
///
/// A light-side (outbound) task moves
/// `queued → delegating → running_remote → completed | failed | timeout`,
/// with `retry_pending → delegating` on recoverable failures.
/// A heavy-side (inbound) task moves
/// `queued → running → completed | failed | timeout`.
/// `cancelled` is reachable from any non-terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Accepted and persisted, waiting for a worker.
    Queued,
    /// Light side: selecting/contacting the target node.
    Delegating,
    /// Light side: accepted by the remote node, execution in progress.
    RunningRemote,
    /// Heavy side: executing locally.
    Running,
    /// Finished; `result` is set.
    Completed,
    /// Permanently failed; `error` is set.
    Failed,
    /// The task's own deadline elapsed; `error` is set.
    Timeout,
    /// Recoverable failure; will be re-attempted after `next_attempt_at`.
    RetryPending,
    /// Cancelled by the operator before completion.
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delegating => "delegating",
            Self::RunningRemote => "running_remote",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::RetryPending => "retry_pending",
            Self::Cancelled => "cancelled",
        }
    }

    /// Terminal states end the lifecycle; nothing transitions out of them.
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Timeout | Self::Cancelled
        )
    }

    /// States that represent work possibly in flight right now. On restart
    /// these are not trusted — the executor may have died — so the daemon
    /// moves them to `RetryPending` and lets the idempotent submit reconcile.
    pub fn in_flight(self) -> bool {
        matches!(self, Self::Delegating | Self::RunningRemote | Self::Running)
    }
}

/// `POST /task` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmission {
    /// Caller-chosen idempotency key. Re-submitting the same `task_id`
    /// returns the existing record instead of executing again, which is
    /// what makes retries safe. Generated when absent.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Typed work, e.g. `llm.generate`, `node.ping`. Same charset rules as
    /// capability ids.
    #[serde(rename = "type")]
    pub task_type: String,
    /// Capabilities the executor must advertise, e.g. `["llm.generate"]`.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Type-specific request body; validated by the executor, not here.
    #[serde(default)]
    pub payload: Value,
    /// Per-task deadline in milliseconds. Bounds how long a light node
    /// waits for a remote result before recording `timeout`.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Pin execution to a specific node id. Set by a delegating node when
    /// forwarding so the receiver executes locally; a receiver refuses
    /// (`409`) when `target_node` names a different node, which prevents
    /// delegation loops.
    #[serde(default)]
    pub target_node: Option<String>,
    /// Long-poll: hold the response until the task settles or this many
    /// milliseconds elapse. Absent/0 returns the current state at once.
    #[serde(default)]
    pub wait_ms: Option<u64>,
    /// Reserved for push delivery: where the result should be sent. Today
    /// the submitter polls `GET /task/<id>`; this field is recorded so a
    /// future webhook/relay delivery can honour it. Identifier or URL,
    /// at most 200 characters.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// A task's wire-visible record, returned by `POST /task` and
/// `GET /task/<id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub state: TaskState,
    /// Node currently executing the task, when routed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Machine-readable-ish failure (`unsupported_capability`, `timeout`,
    /// `transport`, `no_capable_node`, `queue_full`, `internal`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Unix seconds.
    pub created_at: u64,
    pub updated_at: u64,
    /// Unix seconds; `retry_pending` tasks become due at this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<u64>,
}

/// `GET /capabilities` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDoc {
    pub node_id: String,
    /// Resident role (`continuity`, `cognition`, ...).
    pub role: String,
    pub mode: NodeMode,
    /// Capability ids this node can execute locally.
    pub capabilities: Vec<String>,
}

/// Stable task-type ids. Unknown types are rejected at submission; new
/// types join by advertising a capability and handling the type.
pub mod types {
    /// Echo/debug probe — always local.
    pub const NODE_PING: &str = "node.ping";
    /// Bounded read of the node's own status — always local.
    pub const STATE_READ: &str = "state.read";
    /// Chat-completion generation through the node's *local* tiers.
    /// Payload is an OpenAI-shaped chat body; `model` defaults to the
    /// auto-router. Result is the routed OpenAI-shaped response.
    pub const LLM_GENERATE: &str = "llm.generate";
    /// Vector embeddings (future executor).
    pub const EMBEDDING: &str = "embedding";
    /// Image understanding (future executor).
    pub const VISION: &str = "vision";
    /// Heavy web/library research (future executor).
    pub const WEB_RESEARCH: &str = "web.research";
    /// Multi-step planning (future executor).
    pub const PLAN_DEEP: &str = "plan.deep";
}

// The same ids double as capability names a node advertises.
pub mod caps {
    pub const NODE_PING: &str = "node.ping";
    pub const STATE_READ: &str = "state.read";
    pub const LLM_GENERATE: &str = "llm.generate";
    pub const EMBEDDING: &str = "embedding";
    pub const VISION: &str = "vision";
    pub const WEB_RESEARCH: &str = "web.research";
    pub const PLAN_DEEP: &str = "plan.deep";
    pub const AGENT_DELEGATE: &str = "agent.delegate";
    pub const LIBRARY_READ: &str = "library.read";
    pub const DIALOGUE_TURN: &str = "dialogue.turn";
}

pub const MAX_ID_LEN: usize = 80;
pub const MAX_TYPE_LEN: usize = 64;
pub const MAX_REQUIRES: usize = 16;
pub const MAX_CAPABILITIES: usize = 64;
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_TIMEOUT_MS: u64 = 30 * 60 * 1000;
pub const MAX_WAIT_MS: u64 = 60 * 1000;

/// Identifier charset shared by task ids, task types, capability ids and
/// node ids: printable, shell-safe, URL-path-safe.
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn valid_short_id(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Validate a `POST /task` body. Returns a short machine-usable reason.
pub fn validate_submission(s: &TaskSubmission) -> Result<(), &'static str> {
    if let Some(id) = &s.task_id
        && !valid_id(id)
    {
        return Err("invalid_task_id");
    }
    if !valid_short_id(&s.task_type, MAX_TYPE_LEN) {
        return Err("invalid_type");
    }
    if s.requires.len() > MAX_REQUIRES {
        return Err("too_many_requires");
    }
    if s.requires.iter().any(|r| !valid_short_id(r, MAX_TYPE_LEN)) {
        return Err("invalid_requirement");
    }
    if let Some(t) = &s.target_node
        && !valid_id(t)
    {
        return Err("invalid_target_node");
    }
    if s.timeout_ms.is_some_and(|t| t == 0 || t > MAX_TIMEOUT_MS) {
        return Err("invalid_timeout_ms");
    }
    if s.wait_ms.is_some_and(|w| w > MAX_WAIT_MS) {
        return Err("invalid_wait_ms");
    }
    if s.reply_to.as_deref().is_some_and(|r| r.len() > 200) {
        return Err("invalid_reply_to");
    }
    let payload_bytes = serde_json::to_vec(&s.payload).map(|v| v.len()).unwrap_or(0);
    if payload_bytes > MAX_PAYLOAD_BYTES {
        return Err("payload_too_large");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sub() -> TaskSubmission {
        TaskSubmission {
            task_id: Some("nt-1".into()),
            task_type: types::LLM_GENERATE.into(),
            requires: vec![caps::LLM_GENERATE.into()],
            payload: json!({"messages": []}),
            timeout_ms: Some(120_000),
            target_node: Some("llm_master".into()),
            wait_ms: None,
            reply_to: None,
        }
    }

    #[test]
    fn a_valid_submission_passes() {
        validate_submission(&sub()).expect("valid");
    }

    #[test]
    fn bad_fields_are_rejected() {
        let mut s = sub();
        s.task_id = Some("../escape".into());
        assert_eq!(validate_submission(&s), Err("invalid_task_id"));

        let mut s = sub();
        s.task_type = String::new();
        assert_eq!(validate_submission(&s), Err("invalid_type"));

        let mut s = sub();
        s.requires = (0..=MAX_REQUIRES).map(|i| format!("c{i}")).collect();
        assert_eq!(validate_submission(&s), Err("too_many_requires"));

        let mut s = sub();
        s.requires = vec!["bad cap".into()];
        assert_eq!(validate_submission(&s), Err("invalid_requirement"));

        let mut s = sub();
        s.target_node = Some("a b".into());
        assert_eq!(validate_submission(&s), Err("invalid_target_node"));

        let mut s = sub();
        s.timeout_ms = Some(MAX_TIMEOUT_MS + 1);
        assert_eq!(validate_submission(&s), Err("invalid_timeout_ms"));

        let mut s = sub();
        s.payload = json!("x".repeat(MAX_PAYLOAD_BYTES));
        assert_eq!(validate_submission(&s), Err("payload_too_large"));
    }

    #[test]
    fn wire_shapes_are_stable() {
        let s = serde_json::to_value(sub()).expect("ser");
        assert_eq!(s["type"], json!("llm.generate"));
        assert_eq!(s["target_node"], json!("llm_master"));
        let back: TaskSubmission = serde_json::from_value(s).expect("round-trip");
        assert_eq!(back.task_type, "llm.generate");

        let record = TaskRecord {
            task_id: "nt-1".into(),
            task_type: "llm.generate".into(),
            state: TaskState::RunningRemote,
            node: Some("llm_master".into()),
            result: None,
            error: None,
            attempts: 1,
            created_at: 1,
            updated_at: 2,
            next_attempt_at: None,
        };
        let v = serde_json::to_value(&record).expect("ser");
        assert_eq!(v["state"], json!("running_remote"));
        assert_eq!(v["type"], json!("llm.generate"));
        assert!(v.get("result").is_none(), "absent when unset");
        assert!(v.get("error").is_none());
        let back: TaskRecord = serde_json::from_value(v).expect("rt");
        assert_eq!(back.state, TaskState::RunningRemote);
    }

    #[test]
    fn state_classification() {
        assert!(TaskState::Completed.terminal());
        assert!(TaskState::Failed.terminal());
        assert!(!TaskState::RetryPending.terminal());
        assert!(TaskState::RunningRemote.in_flight());
        assert!(TaskState::Delegating.in_flight());
        assert!(!TaskState::Queued.in_flight());
    }
}
