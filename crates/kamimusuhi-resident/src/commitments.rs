//! Commitment engine: durable goals the individual pursues until done.
//!
//! A commitment is an object that outlives the conversation — and the
//! process. It holds the goal, the success condition, a checkpoint
//! (`doing`/`why`/`worked`/`failed`/`current_state`/`next`), the effort
//! budget and the retry ladder position, all persisted in
//! `current_state/commitments.json` and journaled under
//! `logs/commitments`.
//!
//! A dedicated thread ticks over the commitments:
//!
//! ```text
//! observe linked task ─ done → advance plan → verify → close
//!                     ├ failed ─ same signature → replan L0→L6
//!                     │          new signature → retry
//!                     └ interrupted (restart) → resume / retry
//! no task in flight ── act on checkpoint.next
//!                     ├ delegate → task plane
//!                     ├ verify   → run success checks
//!                     ├ sleep    → wait for an external condition
//!                     └ ask      → escalate to the operator
//! ```
//!
//! The escalation ladder separates "keep trying" from "repeat the same
//! thing forever":
//!
//! * L0 retry the same method
//! * L1 same executor, another model
//! * L2 another method (reformulated objective carrying failure context)
//! * L3 another executor (routing switch)
//! * L4 decompose: a planning task turns what remains into steps
//! * L5 question the premises (a review task judges continue/stop)
//! * L6 ask the operator
//!
//! Giving up is never implicit: a commitment leaves `active` only via a
//! verified close, an explicit `abandon`, or `escalated` when the budget
//! or the ladder runs out. The task plane is the actuator; delegated
//! results remain external evidence, and nothing here writes canonical
//! state.
//!
//! `persistence()` reports the PersistenceScore: survival time,
//! interruption recovery, resume success, replans, repeated-failure
//! avoidance, abandonment and long-horizon completion.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use kamimusuhi_runtime::agent_exec as ax;
use kamimusuhi_runtime::agent_exec::{TaskKind, TaskPermissions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::CommitmentsConfig;
use crate::spool::Spool;
use crate::state::Shared;
use crate::tasks::{NewTask, TaskStatus};
use crate::util::{atomic_write, iso8601, run_with_timeout, unix_now, unix_now_ms};

const KEEP_COMMITMENTS: usize = 200;
const KEEP_ATTEMPTS: usize = 60;
const KEEP_NOTES: usize = 40;
const MAX_GOAL_CHARS: usize = 8_000;
const MAX_FIELD_CHARS: usize = 2_000;
const MAX_PLAN_STEPS: usize = 16;
const MAX_LADDER: u8 = 6;
/// Sleep reason for "no untried executor left": on wake the ladder moves
/// to L4 instead of sleeping again.
const NO_EXECUTOR_WAIT: &str = "利用できる別 executor がない";
/// Peer task-plane calls are admission/status checks, not the work
/// itself; 60s covers a slow peer without hanging the tick.
const REMOTE_PLANE_TIMEOUT: Duration = Duration::from_secs(60);

/// POST a task-plane action to a peer's resident. `/v1/agents` serves the
/// same `task_orchestrator::handle` the local dispatch calls, so request
/// and reply shapes are identical — only the transport differs.
fn remote_plane_call(shared: &Shared, node: &str, request: &Value) -> Option<(u16, Value)> {
    let peer = shared.config.peers.iter().find(|p| p.id == node)?;
    crate::tools::post_peer(
        &shared.peer_url(peer),
        "/v1/agents",
        request,
        shared.token.as_deref(),
        REMOTE_PLANE_TIMEOUT,
    )
}

/// What a remote `status` call learned about the delegated task.
enum RemoteTask {
    /// The peer did not answer; keep waiting for it.
    Unreachable,
    /// The peer answered but does not know the task.
    Gone,
    /// Queued or running.
    Running,
    Done,
    /// Failed with no run result — the peer's restart/crash cut it; this
    /// is an interruption to resume, not a strategy failure.
    FailedNoEvidence,
    Failed,
    Cancelled,
}

fn remote_status(shared: &Shared, node: &str, task_id: &str) -> RemoteTask {
    match remote_plane_call(shared, node, &json!({"action": "status", "id": task_id})) {
        None => RemoteTask::Unreachable,
        Some((404, _)) => RemoteTask::Gone,
        Some((_, reply)) => match TaskStatus::parse(reply["status"].as_str().unwrap_or("")) {
            Some(TaskStatus::Done) => RemoteTask::Done,
            Some(TaskStatus::Failed) if reply["result"].is_null() => RemoteTask::FailedNoEvidence,
            Some(TaskStatus::Failed) => RemoteTask::Failed,
            Some(TaskStatus::Cancelled) => RemoteTask::Cancelled,
            _ => RemoteTask::Running,
        },
    }
}

/// A remote task's detail (`executor`/`model`/session) and run result in
/// the shapes `finish_task` consumes locally.
fn remote_snapshot(shared: &Shared, node: &str, task_id: &str) -> (Value, Option<Value>) {
    let Some((_, reply)) =
        remote_plane_call(shared, node, &json!({"action": "status", "id": task_id}))
    else {
        return (json!({}), None);
    };
    let result = reply["result"].clone();
    let detail = json!({
        "executor": reply["executor"],
        "model": reply["model"],
        "external_session_id": reply["external_session_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| reply["result"]["session_id"].as_str()),
    });
    (detail, result.is_object().then_some(result))
}

/// Tools the individual gets for its own commitments.
pub const TOOL_NAMES: [&str; 7] = [
    "commit_create",
    "commit_list",
    "commit_show",
    "commit_update",
    "commit_resume",
    "commit_close",
    "commit_abandon",
];

fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn push_bounded(list: &mut Vec<String>, text: String, keep: usize, chars: usize) {
    let text = clip(text.trim(), chars);
    if !text.is_empty() {
        list.push(text);
    }
    if list.len() > keep {
        let over = list.len() - keep;
        list.drain(..over);
    }
}

// ---------------------------------------------------------------------------
// Types

/// Where a commitment is in its life. `active` work ticks; `sleeping`
/// waits for time or an external condition; `escalated` waits for the
/// operator; `done`/`abandoned` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentState {
    Active,
    Sleeping,
    Escalated,
    Done,
    Abandoned,
}

impl CommitmentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Sleeping => "sleeping",
            Self::Escalated => "escalated",
            Self::Done => "done",
            Self::Abandoned => "abandoned",
        }
    }

    pub const fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Abandoned)
    }
}

/// A machine-checkable finish condition. `command` checks are only
/// honored on operator-created commitments — the individual's own checks
/// are observational (`files_exist`) or delegated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SuccessCheck {
    /// Shell command run inside the named workspace (operator only).
    Command {
        run: String,
        #[serde(default)]
        workspace: Option<String>,
        #[serde(default = "default_check_timeout")]
        timeout_secs: u64,
    },
    /// Every listed path must exist inside the named workspace.
    FilesExist {
        workspace: String,
        paths: Vec<String>,
    },
}

const fn default_check_timeout() -> u64 {
    600
}

/// What "done" means, and how it is verified before closing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SuccessCondition {
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<SuccessCheck>,
    /// Close only after the operator confirms.
    #[serde(default)]
    pub operator_confirm: bool,
}

/// A special delegated task whose result the engine consumes itself:
/// `decompose` returns new plan steps, `premise_review` a continue/stop
/// verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingEffect {
    Decompose,
    PremiseReview,
}

/// One unit of work handed to the task plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegateAction {
    pub objective: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub executor: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permissions: Option<String>,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub context: Vec<String>,
    /// Set for L4/L5 tasks so their result is consumed by the engine.
    #[serde(default)]
    pub effect: Option<PendingEffect>,
}

impl DelegateAction {
    fn for_step(step: &PlanStep) -> Self {
        Self {
            objective: step.objective.clone(),
            kind: Some(step.kind.clone()),
            workspace: step.workspace.clone(),
            executor: None,
            model: None,
            permissions: step.permissions.clone(),
            success_criteria: Vec::new(),
            context: Vec::new(),
            effect: None,
        }
    }

    fn task_kind(&self) -> TaskKind {
        self.kind
            .as_deref()
            .and_then(TaskKind::parse)
            .unwrap_or(TaskKind::General)
    }
}

/// What the engine should do next, checkpointed with the commitment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NextAction {
    /// Dispatch one delegated task on the task plane.
    Delegate(Box<DelegateAction>),
    /// Run the success-condition checks.
    Verify,
    /// Wait until `until` (unix secs) or the condition named in `reason`.
    Sleep { until: u64, reason: String },
    /// Ask the operator; the commitment goes `escalated`.
    AskHuman { question: String },
    /// Recompute the action for the current replan level on the next tick.
    Replan,
}

/// One step of a decomposed plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub objective: String,
    #[serde(default = "default_step_kind")]
    pub kind: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub permissions: Option<String>,
    #[serde(default)]
    pub status: StepStatus,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_step_kind() -> String {
    "general".to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// The restart-safe checkpoint: what the commitment was doing, why, what
/// worked and failed, where it stands, and what to do next.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    #[serde(default)]
    pub doing: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub worked: Vec<String>,
    #[serde(default)]
    pub failed: Vec<String>,
    #[serde(default)]
    pub current_state: String,
    #[serde(default)]
    pub next: Option<NextAction>,
    #[serde(default)]
    pub updated_at: u64,
}

/// How much effort this goal may consume before escalating to the
/// operator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EffortBudget {
    #[serde(default)]
    pub time_secs: Option<u64>,
    #[serde(default)]
    pub attempts: Option<u64>,
    #[serde(default)]
    pub usd: Option<f64>,
}

/// What the goal has consumed so far (elapsed time is
/// `now - created_at`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EffortSpent {
    #[serde(default)]
    pub attempts: u64,
    #[serde(default)]
    pub usd: f64,
}

/// How one delegated attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Done,
    Failed,
    Cancelled,
    /// Cut by a resident restart; retried without consuming the ladder.
    Interrupted,
}

/// The record of one attempt: what was tried, at which ladder level, on
/// which executor/model, and how it ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub at: u64,
    pub task_id: String,
    /// Peer that ran this attempt (None = this node's plane).
    #[serde(default)]
    pub node: Option<String>,
    pub level: u8,
    pub executor: String,
    #[serde(default)]
    pub model: Option<String>,
    pub outcome: AttemptOutcome,
    #[serde(default)]
    pub failure_signature: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentNote {
    pub at: u64,
    pub by: String,
    pub text: String,
}

/// A durable goal. Survives restarts: on load, active commitments resume
/// from `checkpoint.next`, not from chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commitment {
    pub id: String,
    pub goal: String,
    #[serde(default)]
    pub success_condition: SuccessCondition,
    pub state: CommitmentState,
    /// `mio` | `operator`.
    pub owner: String,
    /// Task-board entry tracking this commitment.
    pub board_task: String,
    #[serde(default)]
    pub plan: Vec<PlanStep>,
    #[serde(default)]
    pub checkpoint: Checkpoint,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// `executor` / `executor:model` combinations already tried.
    #[serde(default)]
    pub strategies_tried: Vec<String>,
    /// Replan ladder position 0..=6.
    #[serde(default)]
    pub replan_level: u8,
    #[serde(default)]
    pub last_failure_signature: Option<String>,
    #[serde(default)]
    pub same_failures: u64,
    #[serde(default)]
    pub budget: EffortBudget,
    #[serde(default)]
    pub spent: EffortSpent,
    #[serde(default)]
    pub sleep_until: Option<u64>,
    #[serde(default)]
    pub sleep_reason: Option<String>,
    /// Board task id of the delegated attempt in flight.
    #[serde(default)]
    pub current_task: Option<String>,
    /// Node owning `current_task`: this node's own board, or a peer's when
    /// the attempt was delegated through a peer's task plane.
    #[serde(default)]
    pub current_task_node: Option<String>,
    /// `current_task` was cut by a restart; resume instead of replanning.
    #[serde(default)]
    pub interrupted: bool,
    /// A decompose/premise-review task whose result arrives next.
    #[serde(default)]
    pub pending: Option<PendingEffect>,
    /// Question waiting on the operator while `escalated`.
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub notes: Vec<CommitmentNote>,
    /// Restarts survived while active.
    #[serde(default)]
    pub interruptions: u64,
    /// Times execution was resumed after an interruption.
    #[serde(default)]
    pub resumes: u64,
    /// Resumes that made progress afterwards.
    #[serde(default)]
    pub resumed_progressed: u64,
    /// Replan-level escalations (L→L+1).
    #[serde(default)]
    pub replans: u64,
    /// Failures that repeated the previous signature (avoided loops).
    #[serde(default)]
    pub repeated_failures: u64,
    /// A resume has since produced progress.
    #[serde(default)]
    progress_after_resume: bool,
    /// Last time a stall/escalation note was written.
    #[serde(default)]
    stall_noted_at: u64,
    /// When the commitment entered `escalated` (cleared when it leaves).
    #[serde(default)]
    pub escalated_at: Option<u64>,
    /// The escalation question already notified to the individual — a
    /// resume that changes nothing must not re-tell the same question
    /// forever.
    #[serde(default)]
    escalation_notified: Option<String>,
    /// The operator asked for the close; an `operator_confirm` success
    /// condition is satisfied once checks pass.
    #[serde(default)]
    pub operator_confirmed: bool,
    pub resume_after_restart: bool,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub last_progress_at: u64,
    #[serde(default)]
    pub closed_at: Option<u64>,
    #[serde(default)]
    pub outcome: Option<String>,
    /// Bumped on every mutation; optimistic currency for tick decisions.
    #[serde(default)]
    pub revision: u64,
}

impl Commitment {
    /// Caller-side JSON for `list` and `show`.
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["created"] = json!(iso8601(self.created_at));
        v["updated"] = json!(iso8601(self.updated_at));
        v["last_progress"] = json!(iso8601(self.last_progress_at));
        if let Some(t) = self.closed_at {
            v["closed"] = json!(iso8601(t));
            v["survival_secs"] = json!(t.saturating_sub(self.created_at));
        }
        if let Some(until) = self.sleep_until {
            v["sleep_until_iso"] = json!(iso8601(until));
        }
        v
    }
}

fn push_note(c: &mut Commitment, by: &str, text: &str) {
    c.notes.push(CommitmentNote {
        at: unix_now(),
        by: by.to_owned(),
        text: clip(text, 600),
    });
    if c.notes.len() > KEEP_NOTES {
        c.notes.remove(0);
    }
}

/// The plan step a commitment is working on: the first not finished.
fn current_step(c: &Commitment) -> Option<usize> {
    c.plan
        .iter()
        .position(|s| matches!(s.status, StepStatus::Pending | StepStatus::Running))
}

fn mark_step(c: &mut Commitment, task_id: &str, status: StepStatus, note: Option<&str>) {
    if let Some(step) = c
        .plan
        .iter_mut()
        .find(|s| s.task_id.as_deref() == Some(task_id))
    {
        step.status = status;
        if let Some(note) = note {
            step.note = Some(clip(note, 200));
        }
    }
}

// ---------------------------------------------------------------------------
// Store

/// Fields for a new commitment (keeps `create` readable).
pub struct NewCommitment<'a> {
    pub owner: &'a str,
    pub goal: String,
    pub success_condition: SuccessCondition,
    pub plan: Vec<PlanStep>,
    pub budget: EffortBudget,
    pub resume_after_restart: bool,
}

pub struct Commitments {
    path: PathBuf,
    items: Mutex<Vec<Commitment>>,
    config: CommitmentsConfig,
    /// Transitions the individual should hear about (done / abandoned /
    /// each entry into escalated) when `notify_subject` is configured —
    /// drained by the notify thread and delivered as dialogue turns, so
    /// outcomes reach her memory through the normal intake path.
    outbox: Mutex<VecDeque<Value>>,
    outbox_changed: Condvar,
}

impl Commitments {
    /// Load the store. Active commitments survive the restart: each gets
    /// `interruptions += 1`, and a delegated task still in flight is
    /// flagged `interrupted` so the next tick resumes it instead of
    /// replanning. `resume_after_restart = false` escalates instead.
    pub fn load(path: PathBuf, config: CommitmentsConfig) -> Self {
        let mut items: Vec<Commitment> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for c in &mut items {
            if matches!(c.state, CommitmentState::Active | CommitmentState::Sleeping) {
                c.interruptions += 1;
                if c.current_task.is_some() {
                    c.interrupted = true;
                }
                if c.resume_after_restart {
                    c.state = CommitmentState::Active;
                    push_note(c, "system", "resident の再起動 — checkpoint から再開");
                } else {
                    c.state = CommitmentState::Escalated;
                    c.escalated_at = Some(unix_now());
                    c.stall_noted_at = unix_now();
                    c.question = Some(
                        "再起動をまたいだが resume_after_restart=false — resume で再開".to_owned(),
                    );
                }
                c.sleep_until = None;
            }
        }
        Self {
            path,
            items: Mutex::new(items),
            config,
            outbox: Mutex::new(VecDeque::new()),
            outbox_changed: Condvar::new(),
        }
    }

    fn persist(&self, items: &mut Vec<Commitment>) {
        let len = items.len();
        if len > KEEP_COMMITMENTS {
            items.drain(..len - KEEP_COMMITMENTS);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(items) {
            let _ = atomic_write(&self.path, &bytes);
        }
    }

    pub fn get(&self, id: &str) -> Option<Commitment> {
        self.items
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    pub fn list(&self, all: bool) -> Vec<Commitment> {
        let now = unix_now();
        self.items
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|c| {
                all || !c.state.terminal()
                    || now.saturating_sub(c.closed_at.unwrap_or(now)) < 3 * 24 * 3600
            })
            .cloned()
            .collect()
    }

    /// Aggregate PersistenceScore over the stored commitments.
    pub fn persistence(&self) -> Value {
        let items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        let now = unix_now();
        let mut by_state: std::collections::BTreeMap<&str, u64> = Default::default();
        let (mut attempts, mut replans, mut interruptions, mut resumes) = (0u64, 0u64, 0u64, 0u64);
        let (mut progressed, mut repeated) = (0u64, 0u64);
        let (mut done, mut abandoned, mut long_done, mut long_closed) = (0u64, 0u64, 0u64, 0u64);
        let mut escalated_now = 0u64;
        let mut survival: Vec<u64> = Vec::new();
        let mut oldest_active = 0u64;
        for c in items.iter() {
            *by_state.entry(c.state.as_str()).or_default() += 1;
            attempts += c.spent.attempts;
            replans += c.replans;
            interruptions += c.interruptions;
            resumes += c.resumes;
            progressed += c.resumed_progressed;
            repeated += c.repeated_failures;
            escalated_now += u64::from(c.state == CommitmentState::Escalated);
            if let Some(closed) = c.closed_at {
                let life = closed.saturating_sub(c.created_at);
                survival.push(life);
                match c.state {
                    CommitmentState::Done => done += 1,
                    CommitmentState::Abandoned => abandoned += 1,
                    _ => {}
                }
                if life >= 3600 {
                    long_closed += 1;
                    long_done += u64::from(c.state == CommitmentState::Done);
                }
            } else if matches!(c.state, CommitmentState::Active | CommitmentState::Sleeping) {
                oldest_active = oldest_active.max(now.saturating_sub(c.created_at));
            }
        }
        let created = items.len() as u64;
        let closed = done + abandoned;
        let rate = |n: u64, d: u64| (d > 0).then_some(n as f64 / d as f64);
        json!({
            "commitments": by_state,
            "totals": {"created": created, "attempts": attempts, "replans": replans,
                       "interruptions": interruptions, "resumes": resumes,
                       "resumed_progressed": progressed, "repeated_failures": repeated,
                       "escalated_now": escalated_now},
            "persistence_score": {
                "avg_survival_secs": (!survival.is_empty())
                    .then(|| survival.iter().sum::<u64>() / survival.len() as u64),
                "oldest_active_secs": oldest_active,
                "interruption_recovery_rate": rate(resumes, interruptions),
                "resume_progress_rate": rate(progressed, resumes),
                "repeated_failure_share": rate(repeated, attempts),
                "completion_rate": rate(done, closed),
                "abandonment_rate": rate(abandoned, created),
                "long_horizon_completion_rate": rate(long_done, long_closed),
            },
        })
    }

    /// Compact section for `/status`. Escalated commitments are listed
    /// individually — an unanswered escalation must stay visible.
    pub fn summary(&self) -> Value {
        let p = self.persistence();
        let now = unix_now();
        let escalated: Vec<Value> = self
            .items
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|c| c.state == CommitmentState::Escalated)
            .map(|c| {
                json!({"id": c.id, "goal": c.goal, "question": c.question,
                       "waiting_secs": now.saturating_sub(
                           c.escalated_at.unwrap_or(c.updated_at))})
            })
            .collect();
        json!({
            "enabled": self.config.enabled,
            "tick_secs": self.config.tick_secs,
            "commitments": p["commitments"],
            "totals": p["totals"],
            "escalated": escalated,
        })
    }

    /// One mutation under the lock, then persist + journal. A transition
    /// into a state the individual should hear about (done, abandoned,
    /// escalated) also queues a notification.
    fn mutate<R>(
        &self,
        spool: &Spool,
        id: &str,
        event: &str,
        f: impl FnOnce(&mut Commitment) -> R,
    ) -> Option<R> {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        let c = items.iter_mut().find(|c| c.id == id)?;
        let was = c.state;
        let r = f(c);
        // `escalated_at` follows the transition for every mutation path.
        if c.state == CommitmentState::Escalated && was != CommitmentState::Escalated {
            c.escalated_at = Some(unix_now());
        } else if was == CommitmentState::Escalated && c.state != CommitmentState::Escalated {
            c.escalated_at = None;
        }
        c.updated_at = unix_now();
        c.revision += 1;
        let view = c.view();
        // Notify only on entry: a revision-guarded no-op and updates that
        // keep the same state never re-tell the same outcome. An
        // escalation re-enters with the same question after an unchanged
        // resume — that is told once, not every cycle.
        let notify = if c.state == was {
            None
        } else {
            let kind = match c.state {
                CommitmentState::Done => Some("done"),
                CommitmentState::Abandoned => Some("abandoned"),
                CommitmentState::Escalated if c.question != c.escalation_notified => {
                    c.escalation_notified = c.question.clone();
                    Some("escalated")
                }
                _ => None,
            };
            kind.map(|kind| notify_event(kind, &view))
        };
        self.persist(&mut items);
        drop(items);
        let _ = spool.append_sync(
            "logs/commitments",
            json!({"event": event, "commitment": view}),
        );
        if let Some(event) = notify {
            self.push_notify(event);
        }
        Some(r)
    }

    /// Queued notifications (tests inspect delivery without the runtime).
    #[cfg(test)]
    fn pending_notifications(&self) -> Vec<Value> {
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Queue one event for the notify thread. Bounded: notifications are
    /// opportunistic — the journal carries the durable record.
    fn push_notify(&self, event: Value) {
        if self.config.notify_subject.is_none() {
            return;
        }
        let mut q = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        if q.len() >= 64 {
            q.pop_front();
        }
        q.push_back(event);
        drop(q);
        self.outbox_changed.notify_one();
    }

    /// Deliver queued commitment events to the individual as `talk` turns
    /// on `notify_subject`. One event = one turn, so a done/abandoned/
    /// escalated transition becomes something she experienced — and her
    /// memory gates decide what to keep. A failed turn requeues the
    /// event once; the commitment journal always retains the record.
    fn notify_loop(&self, shared: &Shared) {
        let (Some(subject), Some(dialogue)) = (
            self.config.notify_subject.clone(),
            shared.config.dialogue.clone(),
        ) else {
            return;
        };
        loop {
            let event = {
                let mut q = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
                loop {
                    if let Some(e) = q.pop_front() {
                        break e;
                    }
                    q = self
                        .outbox_changed
                        .wait(q)
                        .unwrap_or_else(|p| p.into_inner());
                }
            };
            let message = notify_message(&event);
            let (status, _) = crate::dialogue::talk(
                shared,
                &dialogue,
                &json!({"subject": subject, "message": message}),
            );
            if status >= 500 && event["retried"] != json!(true) {
                let mut event = event;
                event["retried"] = json!(true);
                self.outbox
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push_back(event);
            } else {
                let _ = shared.spool.append(
                    "logs/commitments",
                    json!({"event": "notified", "subject": subject,
                           "commitment_id": event["commitment"]["id"],
                           "status": status}),
                );
            }
        }
    }

    fn create(&self, shared: &Shared, new: NewCommitment<'_>) -> Commitment {
        let NewCommitment {
            owner,
            goal,
            success_condition,
            plan,
            budget,
            resume_after_restart,
        } = new;
        let now = unix_now();
        let board_task = shared.tasks.create(
            &shared.spool,
            NewTask {
                title: &format!("目標: {goal}"),
                kind: "commitment",
                node: &shared.config.node.id,
                owner,
                status: TaskStatus::InProgress,
                depends_on: Vec::new(),
                detail: json!({}),
            },
        );
        let mut id = format!("c{:x}", unix_now_ms());
        {
            let items = self.items.lock().unwrap_or_else(|p| p.into_inner());
            while items.iter().any(|c| c.id == id) {
                id.push('x');
            }
        }
        let first = plan.first().map(DelegateAction::for_step);
        let mut c = Commitment {
            id: id.clone(),
            goal: goal.clone(),
            success_condition,
            state: CommitmentState::Active,
            owner: owner.to_owned(),
            board_task: board_task.clone(),
            plan,
            checkpoint: Checkpoint::default(),
            attempts: Vec::new(),
            strategies_tried: Vec::new(),
            replan_level: 0,
            last_failure_signature: None,
            same_failures: 0,
            budget,
            spent: EffortSpent::default(),
            sleep_until: None,
            sleep_reason: None,
            current_task: None,
            current_task_node: None,
            interrupted: false,
            pending: None,
            question: None,
            notes: Vec::new(),
            interruptions: 0,
            resumes: 0,
            resumed_progressed: 0,
            replans: 0,
            repeated_failures: 0,
            progress_after_resume: false,
            stall_noted_at: 0,
            escalated_at: None,
            escalation_notified: None,
            operator_confirmed: false,
            resume_after_restart,
            created_at: now,
            updated_at: now,
            last_progress_at: now,
            closed_at: None,
            outcome: None,
            revision: 0,
        };
        c.checkpoint.why = goal;
        c.checkpoint.updated_at = now;
        c.checkpoint.current_state = "created".to_owned();
        c.checkpoint.next = Some(NextAction::Delegate(Box::new(first.unwrap_or(
            DelegateAction {
                objective: c.goal.clone(),
                kind: None,
                workspace: None,
                executor: None,
                model: None,
                permissions: None,
                success_criteria: if c.success_condition.description.is_empty() {
                    Vec::new()
                } else {
                    vec![c.success_condition.description.clone()]
                },
                context: Vec::new(),
                effect: None,
            },
        ))));
        push_note(&mut c, "system", "commitment 作成");
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        items.push(c.clone());
        self.persist(&mut items);
        drop(items);
        shared.tasks.update(
            &shared.spool,
            &board_task,
            None,
            None,
            Some(json!({"commitment": id})),
        );
        let _ = shared.spool.append_sync(
            "logs/commitments",
            json!({"event": "created", "commitment": c.view()}),
        );
        c
    }
}

/// The queued notification payload: the event kind plus the commitment
/// view as it looked at the transition, so the message carries outcome,
/// question and provenance even if the record changes before delivery.
fn notify_event(kind: &str, view: &Value) -> Value {
    json!({"event": kind, "commitment": view})
}

/// The message a commitment event is delivered as. Framed as a system
/// event — something that happened, not a request — so the individual
/// records the outcome as experience.
fn notify_message(event: &Value) -> String {
    let c = &event["commitment"];
    let id = c["id"].as_str().unwrap_or("?");
    let goal = c["goal"].as_str().unwrap_or("");
    let spent = format!(
        "attempts: {} / replans: {} / interruptions: {}",
        c["spent"]["attempts"], c["replans"], c["interruptions"]
    );
    match event["event"].as_str().unwrap_or("") {
        "done" => format!(
            "[commitment-engine] commitment {id} が完了（done）しました。\n\
             goal: {goal}\noutcome: {}\n{spent}\n\
             commit_show で詳細を確認できます。",
            c["outcome"].as_str().unwrap_or("（結果なし）")
        ),
        "abandoned" => format!(
            "[commitment-engine] commitment {id} は放棄（abandoned）されました。\n\
             goal: {goal}\noutcome: {}\n{spent}",
            c["outcome"].as_str().unwrap_or("（結果なし）")
        ),
        _ => format!(
            "[commitment-engine] commitment {id} がエスカレートしました — 回答を待っています。\n\
             goal: {goal}\nquestion: {}\n{spent}\n\
             operator が commit_update/resume で応答します。経緯は commit_show で。",
            c["question"].as_str().unwrap_or("（理由不明）")
        ),
    }
}

// ---------------------------------------------------------------------------
// Tick decisions

/// What the tick decided for one commitment. Executed outside the store
/// lock; applied back under it. `revision` guards against API edits made
/// in between: mutations that would clobber an edit check it.
enum Decision {
    None,
    Wake {
        revision: u64,
    },
    Stall {
        revision: u64,
        task_id: String,
    },
    Escalate {
        revision: u64,
        reason: String,
    },
    Sleep {
        revision: u64,
        until: u64,
        reason: String,
    },
    Delegate {
        revision: u64,
        action: Box<DelegateAction>,
    },
    /// The in-flight task was cut by a restart: continue its external
    /// session when there is one, else retry the same action.
    Resume {
        revision: u64,
        task_id: String,
        instruction: String,
    },
    TaskDone {
        revision: u64,
        task_id: String,
    },
    TaskFailed {
        revision: u64,
        task_id: String,
        cancelled: bool,
    },
    TaskLost {
        revision: u64,
        task_id: String,
    },
    RunChecks {
        revision: u64,
        checks: Vec<SuccessCheck>,
    },
    Replan {
        revision: u64,
    },
    /// An escalated commitment is still waiting on its answer: journal a
    /// reminder so it cannot be forgotten silently.
    StillEscalated {
        revision: u64,
    },
}

/// Normalize an outcome into a failure signature: identical causes must
/// produce identical signatures so "the same failure" is measurable.
fn failure_signature(result: Option<&Value>, detail: &Value, status: &str) -> String {
    let executor = detail["executor"].as_str().unwrap_or("?");
    let model = detail["model"].as_str().unwrap_or("-");
    let class = if result
        .and_then(|r| r["read_only_violation"].as_bool())
        .unwrap_or(false)
    {
        "violation"
    } else if result
        .and_then(|r| r["end"].as_str())
        .is_some_and(|e| e == "timed_out")
    {
        "timeout"
    } else {
        status
    };
    let error = result
        .and_then(|r| r["error"].as_str())
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .to_lowercase();
    // Mask digits (numbers, hex, timestamps) so the *same* error hashes to
    // the same signature.
    let mut norm = String::with_capacity(80);
    let mut last_digit = false;
    for ch in error.chars().take(200) {
        if ch.is_ascii_alphanumeric() || ch.is_whitespace() {
            if ch.is_ascii_digit() {
                if !last_digit {
                    norm.push('#');
                }
                last_digit = true;
            } else {
                norm.push(ch);
                last_digit = false;
            }
        }
    }
    format!("{class}:{executor}:{model}:{}", norm.trim())
}

/// The state-machine step for one commitment. `c` is a snapshot; the
/// returned decision is applied against the store under a revision check.
fn decide(shared: &Shared, c: &Commitment) -> Decision {
    let now = unix_now();
    let config = &shared.config.commitments;
    match c.state {
        CommitmentState::Sleeping => {
            return if c.sleep_until.is_none_or(|t| now >= t) {
                Decision::Wake {
                    revision: c.revision,
                }
            } else {
                Decision::None
            };
        }
        // Escalated is a terminal-shaped wait for an answer, not rest —
        // keep it on the tick so it is re-surfaced periodically.
        CommitmentState::Escalated => {
            return if now.saturating_sub(c.stall_noted_at) >= config.stall_note_secs {
                Decision::StillEscalated {
                    revision: c.revision,
                }
            } else {
                Decision::None
            };
        }
        CommitmentState::Active => {}
        _ => return Decision::None,
    }
    // Effort budget: time, attempts, cost.
    let elapsed = now.saturating_sub(c.created_at);
    if c.budget.time_secs.is_some_and(|t| elapsed >= t)
        || c.budget.attempts.is_some_and(|a| c.spent.attempts >= a)
        || c.budget.usd.is_some_and(|u| c.spent.usd >= u)
    {
        return Decision::Escalate {
            revision: c.revision,
            reason: format!(
                "effort budget exhausted: {} attempts, ${:.2}, {}h",
                c.spent.attempts,
                c.spent.usd,
                elapsed / 3600
            ),
        };
    }
    // Observe the delegated task in flight.
    if let Some(task_id) = &c.current_task {
        // A task on a peer's board is observed over `/v1/agents` instead.
        let node = c
            .current_task_node
            .clone()
            .unwrap_or_else(|| shared.config.node.id.clone());
        if node != shared.config.node.id {
            let instruction = || {
                format!(
                    "中断の続き。commitment {}: {}\n\n現在地: {}",
                    c.id,
                    clip(&c.goal, 300),
                    clip(&c.checkpoint.current_state, 300)
                )
            };
            return match remote_status(shared, &node, task_id) {
                // The peer is unreachable; a restart elsewhere did not
                // touch its task, so keep observing.
                RemoteTask::Unreachable | RemoteTask::Running => Decision::None,
                RemoteTask::Gone => Decision::TaskLost {
                    revision: c.revision,
                    task_id: task_id.clone(),
                },
                RemoteTask::Done => Decision::TaskDone {
                    revision: c.revision,
                    task_id: task_id.clone(),
                },
                RemoteTask::FailedNoEvidence => Decision::Resume {
                    revision: c.revision,
                    task_id: task_id.clone(),
                    instruction: instruction(),
                },
                RemoteTask::Failed => Decision::TaskFailed {
                    revision: c.revision,
                    task_id: task_id.clone(),
                    cancelled: false,
                },
                RemoteTask::Cancelled => Decision::TaskFailed {
                    revision: c.revision,
                    task_id: task_id.clone(),
                    cancelled: true,
                },
            };
        }
        let Some(task) = shared.tasks.get(task_id) else {
            return Decision::TaskLost {
                revision: c.revision,
                task_id: task_id.clone(),
            };
        };
        return match task.status {
            TaskStatus::Done => Decision::TaskDone {
                revision: c.revision,
                task_id: task_id.clone(),
            },
            // A restart-cut task has no `result_evidence_id`: the board
            // marked it Failed without a run result. A failed task *with*
            // evidence is failure accounting, not an interruption.
            TaskStatus::Failed if c.interrupted && task.detail["result_evidence_id"].is_null() => {
                Decision::Resume {
                    revision: c.revision,
                    task_id: task_id.clone(),
                    instruction: format!(
                        "中断の続き。commitment {}: {}\n\n現在地: {}",
                        c.id,
                        clip(&c.goal, 300),
                        clip(&c.checkpoint.current_state, 300)
                    ),
                }
            }
            TaskStatus::Failed => Decision::TaskFailed {
                revision: c.revision,
                task_id: task_id.clone(),
                cancelled: false,
            },
            TaskStatus::Cancelled => Decision::TaskFailed {
                revision: c.revision,
                task_id: task_id.clone(),
                cancelled: true,
            },
            _ => {
                if now.saturating_sub(task.updated_at) >= config.stall_note_secs
                    && now.saturating_sub(c.stall_noted_at) >= config.stall_note_secs
                {
                    Decision::Stall {
                        revision: c.revision,
                        task_id: task_id.clone(),
                    }
                } else {
                    Decision::None
                }
            }
        };
    }
    // No task in flight: act on the checkpointed next action.
    match c.checkpoint.next.clone().unwrap_or(NextAction::Replan) {
        NextAction::Delegate(action) => {
            let in_flight = shared
                .commitments
                .items
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .filter(|c| c.current_task.is_some())
                .count();
            if in_flight >= config.max_active {
                return Decision::None;
            }
            Decision::Delegate {
                revision: c.revision,
                action,
            }
        }
        NextAction::Verify => {
            // `command` checks run only on the operator's commitments; the
            // individual's own shell is never executed by the engine.
            let checks = c
                .success_condition
                .checks
                .iter()
                .filter(|check| {
                    c.owner == "operator" || !matches!(check, SuccessCheck::Command { .. })
                })
                .cloned()
                .collect();
            Decision::RunChecks {
                revision: c.revision,
                checks,
            }
        }
        NextAction::Sleep { until, reason } => Decision::Sleep {
            revision: c.revision,
            until,
            reason,
        },
        NextAction::AskHuman { question } => Decision::Escalate {
            revision: c.revision,
            reason: question,
        },
        NextAction::Replan => Decision::Replan {
            revision: c.revision,
        },
    }
}

// ---------------------------------------------------------------------------
// Actions performed outside the lock

/// Ask the task plane to run `action`; returns the owning node and the
/// board task id. With no local plane the attempt is delegated through
/// the first healthy peer that has one — the same `delegate` request over
/// `/v1/agents`.
fn dispatch(
    shared: &Shared,
    c_id: &str,
    action: &DelegateAction,
) -> Result<(String, String), (u16, Value)> {
    let Some(c) = shared.commitments.get(c_id) else {
        return Err((404, json!({"error": "commitment gone"})));
    };
    let request = json!({
        "action": "delegate",
        "objective": action.objective,
        "kind": action.kind,
        "workspace": action.workspace,
        "executor": action.executor,
        "model": action.model,
        "permissions": action.permissions,
        "success_criteria": action.success_criteria,
        "context": attempt_context(&c, &action.context),
    });
    if shared.task_plane.is_some() {
        let (status, reply) = crate::task_orchestrator::handle(shared, &request, &c.owner);
        if status != 200 {
            return Err((status, reply));
        }
        let task_id = reply["task_id"].as_str().unwrap_or("").to_owned();
        if task_id.is_empty() {
            return Err((500, json!({"error": "no task id returned"})));
        }
        shared.tasks.update(
            &shared.spool,
            &task_id,
            None,
            None,
            Some(json!({"commitment": c_id, "commitment_level": c.replan_level})),
        );
        return Ok((shared.config.node.id.clone(), task_id));
    }
    // No local plane: the task lives on a peer's board instead.
    let mut first_err = None;
    for peer in &shared.config.peers {
        if !shared.peer_healthy(&peer.id) {
            continue;
        }
        let Some((status, reply)) = remote_plane_call(shared, &peer.id, &request) else {
            continue;
        };
        if status == 200 {
            let task_id = reply["task_id"].as_str().unwrap_or("").to_owned();
            if task_id.is_empty() {
                return Err((500, json!({"error": "no task id returned"})));
            }
            return Ok((peer.id.clone(), task_id));
        }
        // A plane-less peer is skipped; quota and other admission errors
        // surface to the caller (sleep / failure accounting).
        if reply["error"]
            .as_str()
            .is_some_and(|e| e.contains("no task plane"))
        {
            continue;
        }
        first_err.get_or_insert((status, reply));
    }
    Err(first_err.unwrap_or((
        503,
        json!({"error": "no task plane on this node or any healthy peer"}),
    )))
}

/// The context handed to every attempt: goal, success condition and what
/// the commitment has learned so far — the persisted checkpoint, not chat.
fn attempt_context(c: &Commitment, extra: &[String]) -> Vec<String> {
    let mut context = Vec::new();
    let mut block = format!(
        "goal: {}\nsuccess: {}",
        c.goal, c.success_condition.description
    );
    if !c.checkpoint.worked.is_empty() {
        block.push_str(&format!("\nworked: {}", c.checkpoint.worked.join(" / ")));
    }
    if !c.checkpoint.failed.is_empty() {
        block.push_str(&format!("\nfailed: {}", c.checkpoint.failed.join(" / ")));
    }
    if !c.checkpoint.current_state.is_empty() {
        block.push_str(&format!("\nstate: {}", c.checkpoint.current_state));
    }
    context.push(clip(&block, ax::policy::CONTEXT_CHARS));
    context.extend(extra.iter().map(|s| clip(s, ax::policy::CONTEXT_CHARS)));
    context
}

/// Run success-condition checks on the tick thread. Each is bounded by
/// its own timeout; a failure names the check and the stderr tail.
fn run_checks(shared: &Shared, checks: &[SuccessCheck]) -> Result<String, String> {
    for check in checks {
        match check {
            SuccessCheck::Command {
                run,
                workspace,
                timeout_secs,
            } => {
                let dir = resolve_workspace(shared, workspace.as_deref())?;
                let script = format!("cd {} && {}", shell_quote(&dir), run);
                let out =
                    run_with_timeout("sh", &["-c", &script], Duration::from_secs(*timeout_secs));
                match out {
                    Some(o) if o.success => {}
                    Some(o) => {
                        return Err(format!(
                            "check {run:?} failed: {}",
                            clip(o.stderr.lines().last().unwrap_or(""), 200)
                        ));
                    }
                    None => return Err(format!("check {run:?} timed out")),
                }
            }
            SuccessCheck::FilesExist { workspace, paths } => {
                let dir = resolve_workspace(shared, Some(workspace))?;
                for p in paths {
                    let joined = dir.join(p);
                    let canon = joined.canonicalize().unwrap_or(joined.clone());
                    if !canon.starts_with(&dir) || !canon.exists() {
                        return Err(format!("path {p} not present under {}", dir.display()));
                    }
                }
            }
        }
    }
    Ok(if checks.is_empty() {
        "no machine checks".to_owned()
    } else {
        format!("{} check(s) passed", checks.len())
    })
}

fn resolve_workspace(shared: &Shared, name: Option<&str>) -> Result<PathBuf, String> {
    let Some(plane) = &shared.task_plane else {
        return Err("no task plane configured".to_owned());
    };
    let (_, path) = plane.workspace(name)?;
    Ok(path)
}

fn shell_quote(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// The replan ladder

/// The most recent delegated action (checkpoint or plan), used as the
/// base for L0–L3 retries.
fn last_delegate(c: &Commitment) -> DelegateAction {
    if let Some(NextAction::Delegate(a)) = &c.checkpoint.next {
        return (**a).clone();
    }
    if let Some(step) = current_step(c).and_then(|i| c.plan.get(i)) {
        return DelegateAction::for_step(step);
    }
    DelegateAction {
        objective: c.goal.clone(),
        kind: None,
        workspace: None,
        executor: None,
        model: None,
        permissions: None,
        success_criteria: if c.success_condition.description.is_empty() {
            Vec::new()
        } else {
            vec![c.success_condition.description.clone()]
        },
        context: Vec::new(),
        effect: None,
    }
}

fn last_workspace(c: &Commitment) -> Option<String> {
    c.plan.iter().rev().find_map(|s| s.workspace.clone())
}

/// L1: keep the executor, switch to a catalog model not yet tried.
fn alternative_model(
    shared: &Shared,
    c: &Commitment,
    last: &DelegateAction,
) -> Option<DelegateAction> {
    let plane = shared.task_plane.as_ref()?;
    let executor = last
        .executor
        .clone()
        .or_else(|| c.attempts.last().map(|a| a.executor.clone()))?;
    let catalog = plane.catalog_snapshot();
    let entry = catalog.executors.get(&executor)?;
    let tried = |model: &str| {
        c.strategies_tried
            .iter()
            .any(|s| s == &format!("{executor}:{model}"))
    };
    let next = entry
        .models
        .iter()
        .filter(|m| m.available && m.billing.auto_eligible() && !tried(&m.id.model))
        .min_by_key(|m| m.billing.rank())?;
    let mut action = last.clone();
    action.executor = Some(executor.clone());
    action.model = Some(next.id.model.clone());
    Some(action)
}

/// L3: keep the task, switch to an executor not yet tried.
fn alternative_executor(
    shared: &Shared,
    c: &Commitment,
    last: &DelegateAction,
) -> Option<DelegateAction> {
    let plane = shared.task_plane.as_ref()?;
    let registry = plane.registry();
    let pool: Vec<(Arc<dyn ax::TaskExecutor>, bool)> = registry
        .all()
        .iter()
        .map(|e| (Arc::clone(e), e.health().ok))
        .collect();
    let cands: Vec<ax::Candidate<'_>> = pool
        .iter()
        .filter(|(e, _)| {
            !c.strategies_tried
                .iter()
                .any(|s| s.split(':').next() == Some(e.spec().name.as_str()))
        })
        .map(|(e, ok)| ax::Candidate {
            spec: e.spec(),
            healthy: *ok,
        })
        .collect();
    let catalog = plane.catalog_snapshot();
    let selection = ax::select(last.task_kind(), None, None, &cands, &catalog).ok()?;
    let mut action = last.clone();
    action.executor = Some(selection.executor);
    action.model = selection.model;
    Some(action)
}

/// L2: same executor/model, a reframed objective carrying the failure
/// context so the harness does not walk into the same wall.
fn reformulated(c: &Commitment, last: &DelegateAction) -> DelegateAction {
    let mut action = last.clone();
    let mut objective = format!(
        "{}\n\n## これまでの失敗（同じ方法を繰り返さないこと）\n",
        last.objective
    );
    for f in c.checkpoint.failed.iter().rev().take(5).rev() {
        objective.push_str(&format!("- {f}\n"));
    }
    objective.push_str("上記とは別の方法・アプローチを選び、選んだ理由を報告に含めること。\n");
    action.objective = objective;
    action
}

/// L4: ask a planning agent to split what remains into steps.
fn decompose_action(c: &Commitment) -> DelegateAction {
    let remaining: Vec<String> = c
        .plan
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Pending | StepStatus::Failed))
        .map(|s| s.objective.clone())
        .collect();
    let objective = format!(
        "# 計画の分解\n\n## ゴール\n{}\n\n## 成功条件\n{}\n\n## これまでに試した方法\n{}\n\n## 失敗\n- {}\n\n## 残りの作業\n- {}\n\n\
         このゴールを達成するための残りの手順に分解し、次の JSON だけで答えてください:\n\
         {{\"steps\":[{{\"objective\":\"…\",\"kind\":\"research|coding|review|debug|planning|general\"}}]}}\n\
         steps は 1〜{MAX_PLAN_STEPS} 個。各 objective はそれ単独で外部作業者に渡せる自己完結の指示にすること。",
        c.goal,
        c.success_condition.description,
        c.strategies_tried.join(", "),
        c.checkpoint.failed.join("\n- "),
        if remaining.is_empty() {
            c.goal.clone()
        } else {
            remaining.join("\n- ")
        },
    );
    DelegateAction {
        objective,
        kind: Some("planning".to_owned()),
        workspace: last_workspace(c),
        executor: None,
        model: None,
        permissions: Some("read_only".to_owned()),
        success_criteria: Vec::new(),
        context: Vec::new(),
        effect: Some(PendingEffect::Decompose),
    }
}

/// L5: ask a reviewer to challenge the goal's premises.
fn premise_action(c: &Commitment) -> DelegateAction {
    let objective = format!(
        "# 前提の検証\n\n## ゴール\n{}\n\n## 成功条件\n{}\n\n## 試行済み\n{}\n\n## 失敗履歴\n- {}\n\n\
         このゴールは L4 まで再計画しても進めなかった。前提・制約・成功条件そのものを批判的に検証し、次の JSON だけで答えてください:\n\
         {{\"continue\": true|false, \"reason\":\"…\", \"revised_goal\":\"…\", \"hints\":[\"…\"]}}\n\
         continue=false は「人間の判断が必要」という意味。",
        c.goal,
        c.success_condition.description,
        c.strategies_tried.join(", "),
        c.checkpoint.failed.join("\n- "),
    );
    DelegateAction {
        objective,
        kind: Some("review".to_owned()),
        workspace: last_workspace(c),
        executor: None,
        model: None,
        permissions: Some("read_only".to_owned()),
        success_criteria: Vec::new(),
        context: Vec::new(),
        effect: Some(PendingEffect::PremiseReview),
    }
}

/// The action for the current ladder level. Levels that cannot act climb
/// further (set on `c.replan_level` and resolve via [`NextAction::Replan`]).
fn action_for_level(shared: &Shared, c: &mut Commitment) -> NextAction {
    loop {
        let last = last_delegate(c);
        let next = match c.replan_level {
            // L0: same method again.
            0 => NextAction::Delegate(Box::new(last)),
            // L1: same executor, another model.
            1 => match alternative_model(shared, c, &last) {
                Some(a) => NextAction::Delegate(Box::new(a)),
                None => {
                    c.replan_level = 3;
                    continue;
                }
            },
            // L2: another method on the same executor/model.
            2 => NextAction::Delegate(Box::new(reformulated(c, &last))),
            // L3: another executor (routing switch).
            3 => match alternative_executor(shared, c, &last) {
                Some(a) => NextAction::Delegate(Box::new(a)),
                None => NextAction::Sleep {
                    until: unix_now() + shared.config.commitments.blocked_sleep_secs,
                    reason: NO_EXECUTOR_WAIT.to_owned(),
                },
            },
            4 => NextAction::Delegate(Box::new(decompose_action(c))),
            5 => NextAction::Delegate(Box::new(premise_action(c))),
            _ => {
                return NextAction::AskHuman {
                    question: format!(
                        "commitment {} は L{MAX_LADDER} まで再計画しても進めない: {}",
                        c.id,
                        c.checkpoint.failed.last().cloned().unwrap_or_default()
                    ),
                };
            }
        };
        return next;
    }
}

/// Record one failure and drive the replan ladder. Called under the store
/// lock; may lift `replan_level` and always sets `checkpoint.next`.
fn record_failure(shared: &Shared, c: &mut Commitment, signature: &str, description: &str) {
    push_bounded(&mut c.checkpoint.failed, description.to_owned(), 10, 300);
    if c.last_failure_signature.as_deref() == Some(signature) {
        c.same_failures += 1;
        c.repeated_failures += 1;
    } else {
        c.same_failures = 1;
        c.last_failure_signature = Some(signature.to_owned());
    }
    if c.same_failures >= shared.config.commitments.same_failure_limit
        && c.replan_level < MAX_LADDER
    {
        c.replan_level += 1;
        c.replans += 1;
        c.same_failures = 0;
        push_note(
            c,
            "system",
            &format!("同じ失敗の繰り返し — 再計画レベル L{}", c.replan_level),
        );
    }
    // A failed engine task (decompose / premise review) delivers no
    // effect; leaving it would mislabel the next task's result.
    c.pending = None;
    c.checkpoint.next = Some(action_for_level(shared, c));
    c.checkpoint.updated_at = unix_now();
    if let Some(task_id) = c.current_task.take() {
        c.current_task_node = None;
        mark_step(c, &task_id, StepStatus::Failed, Some(description));
    }
}

/// An attempt's delegated task finished; consume the result.
fn finish_task(shared: &Shared, id: &str, revision: u64, task_id: &str, outcome: AttemptOutcome) {
    let node = shared
        .commitments
        .get(id)
        .and_then(|c| c.current_task_node.clone());
    let remote = node.as_deref().is_some_and(|n| n != shared.config.node.id);
    let (detail, result): (Value, Option<Value>) = if remote {
        remote_snapshot(shared, node.as_deref().unwrap_or_default(), task_id)
    } else {
        let task = shared.tasks.get(task_id);
        let result = shared
            .task_plane
            .as_ref()
            .and_then(|p| p.load_result(task_id));
        (
            task.map(|t| t.detail.clone()).unwrap_or_else(|| json!({})),
            result,
        )
    };
    let attempt_node = remote.then(|| node.clone().unwrap_or_default());
    let executor = detail["executor"].as_str().unwrap_or("?").to_owned();
    let model = detail["model"].as_str().map(str::to_owned);
    let usd = result.as_ref().and_then(|r| {
        r["cost"]["reported_usd"]
            .as_f64()
            .or(r["cost"]["estimated_usd"].as_f64())
    });
    let summary = result
        .as_ref()
        .and_then(|r| r["summary"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let error = result
        .as_ref()
        .and_then(|r| r["error"].as_str().map(str::to_owned));
    let status_str = match outcome {
        AttemptOutcome::Done => "done",
        AttemptOutcome::Cancelled => "cancelled",
        _ => "failed",
    };
    let signature = failure_signature(result.as_ref(), &detail, status_str);

    shared
        .commitments
        .mutate(&shared.spool, id, "attempt-finished", |c| {
            if c.revision != revision {
                return;
            }
            c.current_task = None;
            c.current_task_node = None;
            c.interrupted = false;
            c.attempts.push(Attempt {
                at: unix_now(),
                task_id: task_id.to_owned(),
                node: attempt_node.clone(),
                level: c.replan_level,
                executor: executor.clone(),
                model: model.clone(),
                outcome,
                failure_signature: (outcome != AttemptOutcome::Done).then(|| signature.clone()),
                note: clip(
                    if outcome == AttemptOutcome::Done {
                        &summary
                    } else {
                        error.as_deref().unwrap_or(status_str)
                    },
                    300,
                ),
                usd,
            });
            if c.attempts.len() > KEEP_ATTEMPTS {
                c.attempts.remove(0);
            }
            if let Some(usd) = usd {
                c.spent.usd += usd;
            }
            // Track tried strategies for L1/L3 exclusion.
            let reference = model
                .as_ref()
                .map(|m| format!("{executor}:{m}"))
                .unwrap_or_else(|| executor.clone());
            push_bounded(&mut c.strategies_tried, reference, 12, 120);

            if outcome == AttemptOutcome::Done {
                c.last_progress_at = unix_now();
                c.last_failure_signature = None;
                c.same_failures = 0;
                if c.resumes > 0 && !c.progress_after_resume {
                    c.progress_after_resume = true;
                    c.resumed_progressed += 1;
                }
                match c.pending.take() {
                    Some(PendingEffect::Decompose) => match parse_steps(&summary) {
                        Ok(steps) => {
                            c.plan = steps;
                            c.replan_level = 0;
                            c.same_failures = 0;
                            c.last_failure_signature = None;
                            c.checkpoint.next = c.plan.first().map(|s| {
                                NextAction::Delegate(Box::new(DelegateAction::for_step(s)))
                            });
                            push_note(c, "system", &format!("分解完了: {} ステップ", c.plan.len()));
                        }
                        Err(e) => {
                            record_failure(
                                shared,
                                c,
                                "decompose:parse",
                                &format!("decompose の出力が不正: {e}"),
                            );
                        }
                    },
                    Some(PendingEffect::PremiseReview) => match parse_premise(&summary) {
                        Ok(verdict) if verdict.go_on => {
                            c.replan_level = 2;
                            c.checkpoint.current_state = format!(
                                "premise review: {}{}",
                                clip(&verdict.reason, 200),
                                verdict
                                    .hints
                                    .iter()
                                    .map(|h| format!(" hint: {}", clip(h, 120)))
                                    .collect::<String>()
                            );
                            if let Some(goal) =
                                verdict.revised_goal.filter(|g| !g.trim().is_empty())
                            {
                                push_note(
                                    c,
                                    "system",
                                    &format!("goal 修正提案: {}", clip(&goal, 200)),
                                );
                                c.checkpoint.why = clip(&goal, MAX_FIELD_CHARS);
                            }
                            c.checkpoint.next = Some(NextAction::Replan);
                            c.checkpoint.updated_at = unix_now();
                        }
                        Ok(verdict) => {
                            c.state = CommitmentState::Escalated;
                            c.question = Some(format!(
                                "前提レビューで続行不可: {}",
                                clip(&verdict.reason, 400)
                            ));
                            c.checkpoint.next = Some(NextAction::AskHuman {
                                question: verdict.reason.clone(),
                            });
                        }
                        Err(e) => {
                            record_failure(
                                shared,
                                c,
                                "premise:parse",
                                &format!("premise review の出力が不正: {e}"),
                            );
                        }
                    },
                    None => {
                        mark_step(c, task_id, StepStatus::Done, Some(&summary));
                        push_bounded(
                            &mut c.checkpoint.worked,
                            format!(
                                "{}:{}{}",
                                executor,
                                model.as_deref().unwrap_or("default"),
                                if summary.is_empty() {
                                    String::new()
                                } else {
                                    format!(" → {}", clip(&summary, 120))
                                }
                            ),
                            10,
                            300,
                        );
                        c.checkpoint.doing = String::new();
                        c.checkpoint.current_state = format!("task {task_id} done");
                        advance(c);
                        c.checkpoint.updated_at = unix_now();
                    }
                }
            } else {
                let desc = format!(
                    "{}:{} — {}",
                    executor,
                    model.as_deref().unwrap_or("default"),
                    clip(error.as_deref().unwrap_or(status_str), 120)
                );
                record_failure(shared, c, &signature, &desc);
            }
        });
}

/// Advance after a successful step: next pending step, else verify.
fn advance(c: &mut Commitment) {
    match c.plan.iter().position(|s| s.status == StepStatus::Pending) {
        Some(i) => {
            c.checkpoint.doing = format!(
                "step {}/{}: {}",
                i + 1,
                c.plan.len(),
                clip(&c.plan[i].objective, 200)
            );
            c.checkpoint.current_state = format!("plan step {} of {}", i + 1, c.plan.len());
            c.checkpoint.next = Some(NextAction::Delegate(Box::new(DelegateAction::for_step(
                &c.plan[i],
            ))));
        }
        None => {
            c.checkpoint.doing = "verify".to_owned();
            c.checkpoint.next = Some(NextAction::Verify);
        }
    }
}

/// Verified close.
fn close(c: &mut Commitment, note: &str, verified: bool) {
    c.state = CommitmentState::Done;
    c.closed_at = Some(unix_now());
    c.outcome = Some(format!(
        "{}{}",
        if verified { "verified" } else { "unverified" },
        if note.is_empty() {
            String::new()
        } else {
            format!(": {note}")
        }
    ));
    c.checkpoint.doing = String::new();
    c.checkpoint.current_state = "done".to_owned();
    push_note(c, "system", &format!("完了: {note}"));
}

// ---------------------------------------------------------------------------
// Tick driver

/// One pass over all commitments. Public for tests; the resident's tick
/// thread is [`spawn`]. External work (dispatch, checks) happens outside
/// the store lock; results are applied back under a revision check so a
/// concurrent API edit is never clobbered.
pub fn tick(shared: &Shared) {
    if !shared.config.commitments.enabled {
        return;
    }
    let ids: Vec<String> = shared
        .commitments
        .items
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|c| {
            matches!(
                c.state,
                CommitmentState::Active | CommitmentState::Sleeping | CommitmentState::Escalated
            )
        })
        .map(|c| c.id.clone())
        .collect();
    for id in ids {
        let Some(c) = shared.commitments.get(&id) else {
            continue;
        };
        match decide(shared, &c) {
            Decision::None => {}
            Decision::Wake { revision } => {
                shared.commitments.mutate(&shared.spool, &id, "wake", |c| {
                    if c.revision == revision {
                        c.state = CommitmentState::Active;
                        c.sleep_until = None;
                        let reason = c.sleep_reason.take().unwrap_or_default();
                        // The wait changed nothing: waiting longer cannot
                        // conjure an executor — decompose instead.
                        if reason == NO_EXECUTOR_WAIT && c.replan_level < 4 {
                            c.replan_level = 4;
                            c.replans += 1;
                            push_note(c, "system", "待機しても executor は増えない — L4 分解へ");
                        }
                        push_note(c, "system", &format!("再開（待機解除: {reason}）"));
                        // A Delegate still queued is retried as-is (quota
                        // waits keep the action); anything else replans.
                        if !matches!(c.checkpoint.next, Some(NextAction::Delegate(_))) {
                            c.checkpoint.next = Some(NextAction::Replan);
                        }
                        c.checkpoint.updated_at = unix_now();
                    }
                });
            }
            Decision::Stall { revision, task_id } => {
                shared.commitments.mutate(&shared.spool, &id, "stall", |c| {
                    if c.revision == revision {
                        c.stall_noted_at = unix_now();
                        push_note(
                            c,
                            "system",
                            &format!("タスク {task_id} に進捗がない — 監視継続"),
                        );
                        c.checkpoint.current_state =
                            format!("waiting on task {task_id} (no progress)");
                        c.checkpoint.updated_at = unix_now();
                    }
                });
            }
            Decision::Escalate { revision, reason } => {
                shared
                    .commitments
                    .mutate(&shared.spool, &id, "escalated", |c| {
                        if c.revision != revision {
                            return;
                        }
                        c.state = CommitmentState::Escalated;
                        c.question = Some(reason.clone());
                        // The reminder cadence starts here; escalated_at
                        // is set by `mutate` on the transition.
                        c.stall_noted_at = unix_now();
                        c.checkpoint.next = Some(NextAction::AskHuman {
                            question: reason.clone(),
                        });
                        push_note(c, "system", &format!("エスカレーション: {reason}"));
                    });
                shared.tasks.update(
                    &shared.spool,
                    &c.board_task,
                    Some(TaskStatus::AwaitingOperator),
                    Some(("system", &reason)),
                    Some(json!({"escalated": true})),
                );
            }
            Decision::StillEscalated { revision } => {
                shared
                    .commitments
                    .mutate(&shared.spool, &id, "escalation_reminder", |c| {
                        if c.revision == revision && c.state == CommitmentState::Escalated {
                            c.stall_noted_at = unix_now();
                            let waiting = unix_now()
                                .saturating_sub(c.escalated_at.unwrap_or(c.updated_at))
                                / 60;
                            push_note(
                                c,
                                "system",
                                &format!(
                                    "エスカレート継続中（{waiting}分待ち）: {}",
                                    c.question.as_deref().unwrap_or("?")
                                ),
                            );
                        }
                    });
            }
            Decision::Sleep {
                revision,
                until,
                reason,
            } => {
                shared.commitments.mutate(&shared.spool, &id, "sleep", |c| {
                    if c.revision == revision {
                        c.state = CommitmentState::Sleeping;
                        c.sleep_until = Some(until);
                        c.sleep_reason = Some(reason.clone());
                        push_note(c, "system", &format!("待機: {reason}"));
                    }
                });
            }
            Decision::Delegate { revision, action } => {
                // An engine task (decompose/premise review) is not a plan
                // step; only a plain delegate advances the running step.
                let step_idx = action.effect.is_none().then(|| current_step(&c)).flatten();
                let level = c.replan_level;
                match dispatch(shared, &id, &action) {
                    Ok((node, task_id)) => {
                        let effect = action.effect;
                        let remote = (node != shared.config.node.id).then(|| node.clone());
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "attempt", |c| {
                                // current_task is recorded even when the
                                // revision moved: the task is real work.
                                if c.current_task.is_none() {
                                    c.current_task = Some(task_id.clone());
                                    c.current_task_node = remote.clone();
                                }
                                if c.revision == revision {
                                    c.spent.attempts += 1;
                                    if let Some(i) = step_idx
                                        && let Some(step) = c.plan.get_mut(i)
                                    {
                                        step.status = StepStatus::Running;
                                        step.task_id = Some(task_id.clone());
                                    }
                                    c.pending = effect;
                                    c.checkpoint.doing = format!(
                                        "attempt {}: {}",
                                        clip(&action.objective, 120),
                                        task_id
                                    );
                                    c.checkpoint.updated_at = unix_now();
                                }
                            });
                        shared.tasks.update(
                            &shared.spool,
                            &c.board_task,
                            None,
                            Some(("system", &format!("試行 L{level}: {}", clip(&action.objective, 120)))),
                            Some(json!({"attempt_task": task_id, "attempt_node": node, "level": level})),
                        );
                    }
                    Err((429, _)) => {
                        // Quota exhausted: an external block. Sleep until
                        // tomorrow, keeping checkpoint.next = the action
                        // so the wake retries it unchanged.
                        let tomorrow = (unix_now() / 86_400 + 1) * 86_400;
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "quota-sleep", |c| {
                                if c.revision == revision {
                                    c.state = CommitmentState::Sleeping;
                                    c.sleep_until = Some(tomorrow);
                                    c.sleep_reason = Some("task plane quota".to_owned());
                                    push_note(c, "system", "クォータ超過 — 翌日まで待機");
                                    c.checkpoint.updated_at = unix_now();
                                }
                            });
                    }
                    Err((status, reply)) => {
                        let error =
                            clip(reply["error"].as_str().unwrap_or("admission failed"), 160);
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "admission-failure", |c| {
                                if c.revision != revision {
                                    return;
                                }
                                let external = error.contains("task plane")
                                    || error.contains("利用できない")
                                    || error.contains("設定されていない");
                                if external {
                                    // No executor can take it right now: sleep,
                                    // keep the action for the wake's retry.
                                    c.state = CommitmentState::Sleeping;
                                    c.sleep_until = Some(
                                        unix_now() + shared.config.commitments.blocked_sleep_secs,
                                    );
                                    c.sleep_reason = Some(error.clone());
                                    push_note(c, "system", &format!("外部条件待ち: {error}"));
                                } else {
                                    record_failure(
                                        shared,
                                        c,
                                        &format!("admission:{status}"),
                                        &format!("admission: {error}"),
                                    );
                                }
                                c.checkpoint.updated_at = unix_now();
                            });
                    }
                }
            }
            Decision::Resume {
                revision,
                task_id,
                instruction,
            } => {
                // The restart cut this task: record it as interrupted
                // (not a strategy failure), then continue the external
                // session when there is one, else retry the same action.
                let node = c
                    .current_task_node
                    .clone()
                    .unwrap_or_else(|| shared.config.node.id.clone());
                let remote = node != shared.config.node.id;
                let (executor, model, continued) = if remote {
                    let (detail, _result) = remote_snapshot(shared, &node, &task_id);
                    // The peer decides whether the session can be
                    // continued (it knows its own result/session rows).
                    let continued = remote_plane_call(
                        shared,
                        &node,
                        &json!({"action": "continue", "id": task_id, "instruction": instruction}),
                    );
                    (
                        detail["executor"].as_str().unwrap_or("?").to_owned(),
                        detail["model"].as_str().map(str::to_owned),
                        continued,
                    )
                } else {
                    let prev = shared.tasks.get(&task_id);
                    let (executor, model, session) = prev
                        .as_ref()
                        .map(|t| {
                            (
                                t.detail["executor"].as_str().unwrap_or("?").to_owned(),
                                t.detail["model"].as_str().map(str::to_owned),
                                t.detail["external_session_id"].as_str().map(str::to_owned),
                            )
                        })
                        .unwrap_or_default();
                    let continued = session.is_some().then(|| {
                        crate::task_orchestrator::handle(
                            shared,
                            &json!({"action": "continue", "id": task_id, "instruction": instruction}),
                            &c.owner,
                        )
                    });
                    (executor, model, continued)
                };
                let attempt_node = remote.then(|| node.clone());
                let record = |c: &mut Commitment| {
                    c.interrupted = false;
                    c.resumes += 1;
                    // The cut task is finished: detach it so the next tick
                    // does not observe it as a fresh failure.
                    c.current_task = None;
                    c.current_task_node = None;
                    c.attempts.push(Attempt {
                        at: unix_now(),
                        task_id: task_id.clone(),
                        node: attempt_node.clone(),
                        level: c.replan_level,
                        executor: executor.clone(),
                        model: model.clone(),
                        outcome: AttemptOutcome::Interrupted,
                        failure_signature: None,
                        note: "resident の再起動で中断".to_owned(),
                        usd: None,
                    });
                    if c.attempts.len() > KEEP_ATTEMPTS {
                        c.attempts.remove(0);
                    }
                    mark_step(c, &task_id, StepStatus::Pending, Some("interrupted"));
                };
                match continued {
                    Some((200, reply)) => {
                        let new_task = reply["task_id"].as_str().unwrap_or("").to_owned();
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "resumed", |c| {
                                if c.revision == revision {
                                    record(c);
                                    c.current_task = Some(new_task.clone());
                                    c.current_task_node = remote.then(|| node.clone());
                                    push_note(c, "system", "中断した外部セッションを継続");
                                }
                            });
                    }
                    Some((_, reply)) => {
                        let error = clip(reply["error"].as_str().unwrap_or("?"), 120);
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "resume-failed", |c| {
                                if c.revision == revision {
                                    record(c);
                                    c.checkpoint.next = Some(last_delegate_for_retry(c));
                                    push_note(
                                        c,
                                        "system",
                                        &format!(
                                            "セッション継続不可（{error}）— 同じ方法でやり直し"
                                        ),
                                    );
                                }
                            });
                    }
                    None => {
                        shared
                            .commitments
                            .mutate(&shared.spool, &id, "resumed", |c| {
                                if c.revision == revision {
                                    record(c);
                                    c.checkpoint.next = Some(last_delegate_for_retry(c));
                                    push_note(
                                        c,
                                        "system",
                                        "外部セッションなし — 同じ方法でやり直し",
                                    );
                                }
                            });
                    }
                }
            }
            Decision::TaskDone { revision, task_id } => {
                finish_task(shared, &id, revision, &task_id, AttemptOutcome::Done);
            }
            Decision::TaskFailed {
                revision,
                task_id,
                cancelled,
            } => {
                finish_task(
                    shared,
                    &id,
                    revision,
                    &task_id,
                    if cancelled {
                        AttemptOutcome::Cancelled
                    } else {
                        AttemptOutcome::Failed
                    },
                );
            }
            Decision::TaskLost { revision, task_id } => {
                shared
                    .commitments
                    .mutate(&shared.spool, &id, "task-lost", |c| {
                        if c.revision == revision {
                            c.current_task = None;
                            c.current_task_node = None;
                            record_failure(
                                shared,
                                c,
                                "lost",
                                &format!("task {task_id} disappeared from the board"),
                            );
                        }
                    });
            }
            Decision::RunChecks { revision, checks } => {
                let verified = !checks.is_empty();
                let outcome = run_checks(shared, &checks);
                shared.commitments.mutate(&shared.spool, &id, "verify", |c| {
                    if c.revision != revision {
                        return;
                    }
                    match outcome {
                        Ok(note) => {
                            if c.success_condition.operator_confirm && !c.operator_confirmed {
                                // Checks pass; the operator still owes the
                                // confirmation this success condition asked for.
                                c.state = CommitmentState::Escalated;
                                c.question = Some(format!(
                                    "成功条件の確認: {}（{note}）。`close` で確定、`resume` で差し戻し",
                                    clip(&c.success_condition.description, 200)
                                ));
                                c.checkpoint.next = Some(NextAction::AskHuman {
                                    question: c.question.clone().unwrap_or_default(),
                                });
                            } else {
                                close(c, &note, verified);
                            }
                        }
                        Err(error) => {
                            if error.contains("no task plane") {
                                // The check itself cannot run here (the
                                // work lives on a peer's filesystem): a
                                // human must verify or amend the success
                                // condition, not retry the ladder.
                                c.state = CommitmentState::Escalated;
                                c.question = Some(format!(
                                    "検証不能: {error} — 成功条件を operator が確認するか `update` で修正"
                                ));
                                c.checkpoint.next = Some(NextAction::AskHuman {
                                    question: c.question.clone().unwrap_or_default(),
                                });
                            } else {
                                record_failure(
                                    shared,
                                    c,
                                    &format!("verify:{}", clip(&error, 60)),
                                    &format!("verify: {error}"),
                                );
                            }
                            c.checkpoint.updated_at = unix_now();
                        }
                    }
                });
                if shared
                    .commitments
                    .get(&id)
                    .is_some_and(|c| c.state == CommitmentState::Done)
                {
                    shared.tasks.update(
                        &shared.spool,
                        &c.board_task,
                        Some(TaskStatus::Done),
                        Some(("system", "commitment closed")),
                        None,
                    );
                }
            }
            Decision::Replan { revision } => {
                shared
                    .commitments
                    .mutate(&shared.spool, &id, "replan", |c| {
                        if c.revision == revision {
                            c.checkpoint.next = Some(action_for_level(shared, c));
                            c.checkpoint.updated_at = unix_now();
                        }
                    });
            }
        }
    }
}

/// The retry action after an interruption: the action that was in flight
/// (checkpoint.next was left at it) or the current plan step.
fn last_delegate_for_retry(c: &Commitment) -> NextAction {
    match &c.checkpoint.next {
        Some(action @ NextAction::Delegate(_)) => action.clone(),
        _ => NextAction::Delegate(Box::new(last_delegate(c))),
    }
}

// ---------------------------------------------------------------------------
// Decompose / premise-review result parsing

/// Tolerant JSON extraction from an agent's report: the first `{`…`}`
/// block containing `key` wins.
fn extract_json(text: &str, key: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(s) = start.take()
                    && text[s..=i].contains(key)
                    && let Ok(v) = serde_json::from_str::<Value>(&text[s..=i])
                    && v.get(key).is_some()
                {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_steps(summary: &str) -> Result<Vec<PlanStep>, String> {
    let v = extract_json(summary, "steps").ok_or("no {\"steps\": …} in report")?;
    let steps = v["steps"].as_array().ok_or("\"steps\" is not an array")?;
    if steps.is_empty() {
        return Err("steps is empty".to_owned());
    }
    steps
        .iter()
        .take(MAX_PLAN_STEPS)
        .map(|s| {
            let objective = s["objective"]
                .as_str()
                .map(str::trim)
                .filter(|o| !o.is_empty() && o.chars().count() <= MAX_GOAL_CHARS)
                .ok_or("step.objective missing or too long")?;
            let kind = s["kind"]
                .as_str()
                .filter(|k| TaskKind::parse(k).is_some())
                .unwrap_or("general")
                .to_owned();
            Ok(PlanStep {
                objective: objective.to_owned(),
                kind,
                workspace: s["workspace"].as_str().map(str::to_owned),
                permissions: s["permissions"].as_str().map(str::to_owned),
                status: StepStatus::Pending,
                task_id: None,
                note: None,
            })
        })
        .collect()
}

struct PremiseVerdict {
    go_on: bool,
    reason: String,
    revised_goal: Option<String>,
    hints: Vec<String>,
}

fn parse_premise(summary: &str) -> Result<PremiseVerdict, String> {
    let v = extract_json(summary, "continue").ok_or("no {\"continue\": …} in report")?;
    Ok(PremiseVerdict {
        go_on: v["continue"].as_bool().unwrap_or(false),
        reason: v["reason"].as_str().unwrap_or("").to_owned(),
        revised_goal: v["revised_goal"].as_str().map(str::to_owned),
        hints: v["hints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .take(6)
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// API surface

/// `POST /v1/commitments` — operator and individual actions.
///
/// * `{"action":"list","all":bool}` / `{"action":"show","id"}`
/// * `{"action":"create","goal":…,"success_condition":{…},"steps":[…],"budget":{…}}`
/// * `{"action":"update","id","note"|"doing"|"current_state"|"worked"|"steps"|"next_action"|"budget"}`
///   — `budget` fields merge onto the current budget; operator-only
/// * `{"action":"resume","id","note","budget"?}` — answer an escalated
///   commitment (raising `budget` lifts a budget-exhaustion escalation)
/// * `{"action":"abandon","id","reason"}` / `{"action":"close","id"}`
/// * `{"action":"metrics"}`
///
/// `command` success checks are honored only for operator-created
/// commitments; the individual's are filtered out at verify time.
pub fn handle(shared: &Shared, request: &Value, by: &str) -> (u16, Value) {
    let store = &shared.commitments;
    let text = |k: &str| request[k].as_str().unwrap_or("").trim().to_owned();
    match request["action"].as_str().unwrap_or("list") {
        "list" => (
            200,
            json!({"commitments": store.list(request["all"].as_bool().unwrap_or(false))
                .iter().map(Commitment::view).collect::<Vec<_>>()}),
        ),
        "show" => match store.get(&text("id")) {
            Some(c) => (200, c.view()),
            None => (404, json!({"error": "no commitment with that id"})),
        },
        "metrics" => (200, store.persistence()),
        "create" => {
            let goal = text("goal");
            if goal.is_empty() || goal.chars().count() > MAX_GOAL_CHARS {
                return (400, json!({"error": "goal must be 1..8000 chars"}));
            }
            let success_condition: SuccessCondition = if request["success_condition"].is_null() {
                SuccessCondition::default()
            } else {
                match serde_json::from_value(request["success_condition"].clone()) {
                    Ok(s) => s,
                    Err(e) => return (400, json!({"error": format!("success_condition: {e}")})),
                }
            };
            if success_condition.checks.len() > 8 {
                return (400, json!({"error": "at most 8 success checks"}));
            }
            let plan = match parse_plan(&request["steps"], by == "operator") {
                Ok(p) => p,
                Err(e) => return (400, json!({"error": e})),
            };
            let mut budget: EffortBudget =
                serde_json::from_value(request["budget"].clone()).unwrap_or_default();
            // Unset limits inherit the engine's defaults.
            if budget.attempts.is_none() {
                budget.attempts = Some(shared.config.commitments.default_attempts);
            }
            if budget.time_secs.is_none() {
                budget.time_secs = shared.config.commitments.default_time_secs;
            }
            if budget.usd.is_none() {
                budget.usd = shared.config.commitments.default_usd;
            }
            let resume = request["resume_after_restart"].as_bool().unwrap_or(true);
            let c = store.create(
                shared,
                NewCommitment {
                    owner: by,
                    goal,
                    success_condition,
                    plan,
                    budget,
                    resume_after_restart: resume,
                },
            );
            (200, json!({"ok": true, "commitment": c.view()}))
        }
        "update" => {
            let id = text("id");
            if store.get(&id).is_none() {
                return (404, json!({"error": format!("no commitment {id}")}));
            }
            let note = text("note");
            let doing = request["doing"].as_str().map(str::to_owned);
            let current_state = request["current_state"].as_str().map(str::to_owned);
            let worked = request["worked"].as_str().map(str::to_owned);
            let next_action = match request.get("next_action") {
                Some(v) => match serde_json::from_value::<NextAction>(v.clone()) {
                    Ok(n) => Some(n),
                    Err(e) => return (400, json!({"error": format!("next_action: {e}")})),
                },
                None => None,
            };
            let steps = if request.get("steps").is_some() {
                match parse_plan(&request["steps"], by == "operator") {
                    Ok(p) => Some(p),
                    Err(e) => return (400, json!({"error": e})),
                }
            } else {
                None
            };
            // Budget fields that are present replace the current values;
            // absent fields keep them. Only the operator may change the
            // effort budget — otherwise the bound means nothing.
            let budget = match request.get("budget") {
                Some(v) if !v.is_null() => {
                    match serde_json::from_value::<EffortBudget>(v.clone()) {
                        Ok(b) => Some(b),
                        Err(e) => return (400, json!({"error": format!("budget: {e}")})),
                    }
                }
                _ => None,
            };
            if budget.is_some() && by != "operator" {
                return (403, json!({"error": "budget changes are operator-only"}));
            }
            store.mutate(&shared.spool, &id, "updated", |c| {
                if !note.is_empty() {
                    push_note(c, by, &note);
                }
                if let Some(doing) = doing {
                    c.checkpoint.doing = clip(&doing, MAX_FIELD_CHARS);
                }
                if let Some(state) = current_state {
                    c.checkpoint.current_state = clip(&state, MAX_FIELD_CHARS);
                }
                if let Some(worked) = worked {
                    push_bounded(&mut c.checkpoint.worked, worked, 10, 300);
                }
                if let Some(next) = next_action {
                    c.checkpoint.next = Some(next);
                }
                if let Some(steps) = steps {
                    c.plan = steps;
                }
                if let Some(b) = budget {
                    if b.time_secs.is_some() {
                        c.budget.time_secs = b.time_secs;
                    }
                    if b.attempts.is_some() {
                        c.budget.attempts = b.attempts;
                    }
                    if b.usd.is_some() {
                        c.budget.usd = b.usd;
                    }
                }
                if request["operator_confirmed"].as_bool().unwrap_or(false) && by == "operator" {
                    c.operator_confirmed = true;
                }
                c.checkpoint.updated_at = unix_now();
                // An operator edit re-arms an escalated commitment.
                if by == "operator" && c.state == CommitmentState::Escalated {
                    c.state = CommitmentState::Active;
                    c.question = None;
                }
            });
            (
                200,
                json!({"ok": true, "commitment": store.get(&id).map(|c| c.view())}),
            )
        }
        "resume" => {
            let id = text("id");
            let Some(c) = store.get(&id) else {
                return (404, json!({"error": format!("no commitment {id}")}));
            };
            if !matches!(
                c.state,
                CommitmentState::Escalated | CommitmentState::Sleeping
            ) {
                return (
                    409,
                    json!({"error": format!("{id} is {}", c.state.as_str())}),
                );
            }
            let answer = text("note");
            // Resuming a budget-exhausted commitment without a new budget
            // would just re-escalate on the next tick, so `resume` accepts
            // the same budget override as `update` (operator-only).
            let budget = match request.get("budget") {
                Some(v) if !v.is_null() => {
                    match serde_json::from_value::<EffortBudget>(v.clone()) {
                        Ok(b) => Some(b),
                        Err(e) => return (400, json!({"error": format!("budget: {e}")})),
                    }
                }
                _ => None,
            };
            if budget.is_some() && by != "operator" {
                return (403, json!({"error": "budget changes are operator-only"}));
            }
            store.mutate(&shared.spool, &id, "resumed", |c| {
                c.state = CommitmentState::Active;
                c.sleep_until = None;
                let question = c.question.take().unwrap_or_default();
                if !answer.is_empty() {
                    c.checkpoint.current_state =
                        format!("{by}: {answer}（問: {}）", clip(&question, 200));
                    push_bounded(
                        &mut c.checkpoint.worked,
                        format!("{by} answer: {answer}"),
                        10,
                        300,
                    );
                }
                if let Some(b) = budget {
                    if b.time_secs.is_some() {
                        c.budget.time_secs = b.time_secs;
                    }
                    if b.attempts.is_some() {
                        c.budget.attempts = b.attempts;
                    }
                    if b.usd.is_some() {
                        c.budget.usd = b.usd;
                    }
                }
                // A human answer is new information: restart the ladder.
                c.replan_level = 0;
                c.same_failures = 0;
                c.checkpoint.next = Some(NextAction::Replan);
                c.checkpoint.updated_at = unix_now();
                push_note(c, by, "再開");
            });
            shared.tasks.update(
                &shared.spool,
                &c.board_task,
                Some(TaskStatus::InProgress),
                Some((by, "commitment resumed")),
                Some(json!({"escalated": false})),
            );
            (
                200,
                json!({"ok": true, "commitment": store.get(&id).map(|c| c.view())}),
            )
        }
        "abandon" => {
            let id = text("id");
            let reason = text("reason");
            let Some(c) = store.get(&id) else {
                return (404, json!({"error": format!("no commitment {id}")}));
            };
            if c.state.terminal() {
                return (409, json!({"error": format!("{id} already closed")}));
            }
            store.mutate(&shared.spool, &id, "abandoned", |c| {
                c.state = CommitmentState::Abandoned;
                c.closed_at = Some(unix_now());
                c.outcome = Some(if reason.is_empty() {
                    "abandoned".to_owned()
                } else {
                    format!("abandoned: {}", clip(&reason, 200))
                });
                push_note(c, by, "放棄");
            });
            shared.tasks.update(
                &shared.spool,
                &c.board_task,
                Some(TaskStatus::Cancelled),
                Some((by, "commitment abandoned")),
                None,
            );
            // Stop a delegated attempt still running: abandoning the
            // commitment must not orphan a harness process consuming quota.
            if let Some(task_id) = &c.current_task {
                let node = c
                    .current_task_node
                    .clone()
                    .unwrap_or_else(|| shared.config.node.id.clone());
                if node == shared.config.node.id {
                    let _ = crate::task_orchestrator::cancel(shared, &json!({"id": task_id}), by);
                } else {
                    let _ = remote_plane_call(
                        shared,
                        &node,
                        &json!({"action": "cancel", "id": task_id}),
                    );
                }
            }
            (
                200,
                json!({"ok": true, "commitment": store.get(&id).map(|c| c.view())}),
            )
        }
        "close" => {
            let id = text("id");
            let Some(c) = store.get(&id) else {
                return (404, json!({"error": format!("no commitment {id}")}));
            };
            if c.state.terminal() {
                return (409, json!({"error": format!("{id} already closed")}));
            }
            if c.current_task.is_some() {
                return (409, json!({"error": format!("{id} has a task in flight")}));
            }
            store.mutate(&shared.spool, &id, "verify-requested", |c| {
                c.state = CommitmentState::Active;
                c.sleep_until = None;
                if by == "operator" {
                    c.operator_confirmed = true;
                }
                c.checkpoint.next = Some(NextAction::Verify);
                c.checkpoint.updated_at = unix_now();
                push_note(c, by, "close 要求 — 検証へ");
            });
            (
                200,
                json!({"ok": true, "verifying": true,
                       "commitment": store.get(&id).map(|c| c.view())}),
            )
        }
        other => (
            400,
            json!({"error": format!("action must be list, show, create, update, resume, abandon, close or metrics (got {other})")}),
        ),
    }
}

fn parse_plan(value: &Value, operator: bool) -> Result<Vec<PlanStep>, String> {
    let Some(list) = value.as_array() else {
        return Ok(Vec::new());
    };
    if list.len() > MAX_PLAN_STEPS {
        return Err(format!("at most {MAX_PLAN_STEPS} steps"));
    }
    list.iter()
        .map(|s| {
            let objective = s["objective"]
                .as_str()
                .map(str::trim)
                .filter(|o| !o.is_empty() && o.chars().count() <= MAX_GOAL_CHARS)
                .ok_or("step.objective missing or too long")?;
            let kind = match s["kind"].as_str() {
                None => "general",
                Some(k) if TaskKind::parse(k).is_some() => k,
                Some(k) => return Err(format!("unknown step kind {k}")),
            };
            let permissions = match s["permissions"].as_str() {
                None => None,
                Some("autonomous_workspace") if !operator => {
                    return Err("autonomous_workspace は操作者だけが指定できる".to_owned());
                }
                Some(p) if TaskPermissions::parse(p).is_some() => Some(p.to_owned()),
                Some(p) => return Err(format!("unknown step permissions {p}")),
            };
            Ok(PlanStep {
                objective: objective.to_owned(),
                kind: kind.to_owned(),
                workspace: s["workspace"].as_str().map(str::to_owned),
                permissions,
                status: StepStatus::Pending,
                task_id: None,
                note: None,
            })
        })
        .collect()
}

/// Tool-call entry point, same `ok`/`error` envelope as the other tools.
/// `request` is the raw `/v1/tools/call` body; its `arguments` may be an
/// object or a JSON string.
pub fn tool(shared: &Shared, name: &str, request: &Value) -> (u16, Value) {
    let mut args = match &request["arguments"] {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other.clone(),
    };
    args["action"] = json!(match name {
        "commit_create" => "create",
        "commit_show" => "show",
        "commit_update" => "update",
        "commit_resume" => "resume",
        "commit_close" => "close",
        "commit_abandon" => "abandon",
        _ => "list",
    });
    let (status, reply) = handle(shared, &args, "mio");
    if status == 200 {
        (200, json!({"ok": true, "result": reply}))
    } else {
        (
            200,
            json!({"ok": false, "error": reply.get("error").cloned().unwrap_or(reply)}),
        )
    }
}

/// Tool definitions for the individual.
pub fn definitions() -> Vec<Value> {
    let f = |name: &str, description: &str, parameters: Value| {
        json!({"type": "function", "function": {
            "name": name, "description": description, "parameters": parameters}})
    };
    vec![
        f(
            "commit_create",
            "Start a durable commitment — a goal you keep pursuing across turns and restarts until verified done. The engine retries failures, replans (other models, other executors, decomposition) and escalates to the operator; it never quietly gives up. Use steps to give your own decomposition up front.",
            json!({"type": "object", "properties": {
                "goal": {"type": "string", "description": "the goal, self-contained"},
                "success_condition": {"type": "object", "properties": {
                    "description": {"type": "string"},
                    "operator_confirm": {"type": "boolean"},
                    "checks": {"type": "array", "items": {"type": "object", "properties": {
                        "type": {"type": "string", "enum": ["files_exist", "command"]},
                        "workspace": {"type": "string"},
                        "paths": {"type": "array", "items": {"type": "string"}},
                        "run": {"type": "string"},
                        "timeout_secs": {"type": "integer"}},
                        "required": ["type"]},
                        "description": "machine checks; command checks run only on operator commitments"}},
                    "required": ["description"]},
                "steps": {"type": "array", "items": {"type": "object", "properties": {
                    "objective": {"type": "string"},
                    "kind": {"type": "string", "enum": ["research","coding","review","debug","planning","background","general"]},
                    "workspace": {"type": "string"},
                    "permissions": {"type": "string", "enum": ["read_only","workspace_write"]}},
                    "required": ["objective"]},
                    "description": "your decomposition; each step becomes one delegated task"},
                "budget": {"type": "object", "properties": {
                    "time_secs": {"type": "integer"}, "attempts": {"type": "integer"},
                    "usd": {"type": "number"}},
                    "description": "effort cap; exhaustion escalates to the operator"},
                "resume_after_restart": {"type": "boolean", "default": true}},
                "required": ["goal"]}),
        ),
        f(
            "commit_list",
            "List commitments: durable goals in flight or recently closed, with checkpoint and progress.",
            json!({"type": "object", "properties": {"all": {"type": "boolean"}}}),
        ),
        f(
            "commit_show",
            "Show one commitment: goal, plan, attempts, replan level, checkpoint (doing/why/worked/failed/next).",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        f(
            "commit_update",
            "Update your own commitment's checkpoint (doing/current_state/worked), replace the plan (steps) or the next_action, or leave a note.",
            json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "note": {"type": "string"},
                "doing": {"type": "string"},
                "current_state": {"type": "string"},
                "worked": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "object"}},
                "next_action": {"type": "object"}},
                "required": ["id"]}),
        ),
        f(
            "commit_resume",
            "Re-arm an escalated or sleeping commitment: attach your answer or finding as note and the engine replans from it. Raising the effort budget itself is operator-only.",
            json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "note": {"type": "string", "description": "the answer to the escalation question"}},
                "required": ["id"]}),
        ),
        f(
            "commit_close",
            "Ask for verified close: runs the success-condition checks, then closes or resumes work.",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        f(
            "commit_abandon",
            "Explicitly abandon a commitment (giving up is a visible act, never silent).",
            json!({"type": "object", "properties": {
                "id": {"type": "string"}, "reason": {"type": "string"}},
                "required": ["id"]}),
        ),
    ]
}

/// The engine's tick thread, plus — when `commitments.notify_subject` is
/// configured — a delivery thread that turns done/abandoned/escalated
/// transitions into dialogue turns for the individual.
pub fn spawn(shared: &Arc<Shared>) {
    if !shared.config.commitments.enabled {
        return;
    }
    let s = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("commitments".to_owned())
        .spawn(move || {
            let interval = Duration::from_secs(s.config.commitments.tick_secs.max(5));
            loop {
                tick(&s);
                std::thread::sleep(interval);
            }
        });
    if shared.config.commitments.notify_subject.is_some() && shared.config.dialogue.is_some() {
        let s = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("commitments-notify".to_owned())
            .spawn(move || s.commitments.notify_loop(&s));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::Path;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A resident with two fake executors. Markers in the delegated
    /// prompt (which embeds the objective, so also the goal) steer the
    /// fake harness deterministically — headers checked before markers:
    ///
    /// * `# 前提の検証` — the L5 premise review: answer a JSON verdict
    /// * `# 計画の分解` — the L4 decompose task: a JSON plan, unless the
    ///   goal also carries `NODECOMP` (then it fails)
    /// * `SLEEP` — the run sleeps (an in-flight task to cancel)
    /// * `OK` — succeed even when the prompt also carries FAIL (the
    ///   decompose task returns OK-marked step objectives)
    /// * `FAIL` — the run fails
    /// * otherwise — succeed, reporting a `sessionID`
    fn fixture(commitments: Value) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).expect("ws");
        git(&ws, &["init", "-q"]);
        git(
            &ws,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let agent = script(
            dir.path(),
            "fake-agent",
            r#"if [ "$1" = "--list" ]; then printf 'fake-1\nfake-2\n'; exit 0; fi
for prompt; do :; done
case "$prompt" in *"前提の検証"*)
    printf '%s\n' '{"type":"text","sessionID":"s-1","part":{"type":"text","text":"{\"continue\":false,\"reason\":\"premise wrong\"}"}}'
    exit 0;; esac
case "$prompt" in *"計画の分解"*)
    case "$prompt" in *NODECOMP*) exit 1;; esac
    printf '%s\n' '{"type":"text","sessionID":"s-1","part":{"type":"text","text":"{\"steps\":[{\"objective\":\"OK first step\",\"kind\":\"research\"},{\"objective\":\"OK second step\"}]}"}}'
    exit 0;; esac
case "$prompt" in *SLEEP*) sleep 30; printf '{"type":"text","sessionID":"s-1","part":{"type":"text","text":"done"}}\n'; exit 0;; esac
case "$prompt" in *OK*) printf '{"type":"text","sessionID":"s-1","part":{"type":"text","text":"done"}}\n'; exit 0;; esac
case "$prompt" in *FAIL*) exit 1;; esac
printf '{"type":"text","sessionID":"s-1","part":{"type":"text","text":"done"}}\n'"#,
        );
        let other = script(
            dir.path(),
            "other-agent",
            r#"if [ "$1" = "--list" ]; then printf 'other-1\n'; exit 0; fi
for prompt; do :; done
case "$prompt" in *FAIL*) exit 1;; esac
printf 'other done\n'"#,
        );
        let config: Config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "cognition", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "task_plane": {
                "workspaces": [{"name": "ws", "path": ws}],
                "check_interval_secs": 3600,
                "executors": [
                    {"name": "fake", "adapter": "generic", "command": agent,
                     "discover": {"args": ["--list"], "format": "lines"},
                     "run": {"args": ["{prompt}"], "model_args": ["--model", "{model}"],
                             "resume_args": ["--session", "{session}"],
                             "output": "opencode_json"},
                     "permission_args": {"read_only": []},
                     "default_billing": "local", "default_model": "fake-1",
                     "timeout_secs": 60},
                    {"name": "other", "adapter": "generic", "command": other,
                     "discover": {"args": ["--list"], "format": "lines"},
                     "run": {"args": ["{prompt}"], "output": "text"},
                     "permission_args": {"read_only": []},
                     "default_billing": "local", "default_model": "other-1",
                     "timeout_secs": 60}
                ]
            },
            "commitments": commitments,
        }))
        .expect("config");
        config.validate().expect("valid");
        let spool = Spool::new(dir.path().join("spool"), "test").expect("spool");
        let shared = Shared::new(config, spool, 1, None);
        shared.task_plane.as_ref().expect("plane").refresh_catalog();
        (shared, dir)
    }

    fn create(shared: &Shared, goal: &str, extra: Value) -> String {
        let mut req = json!({"action": "create", "goal": goal});
        for (k, v) in extra.as_object().into_iter().flatten() {
            req[k] = v.clone();
        }
        let (status, reply) = handle(shared, &req, "operator");
        assert_eq!(status, 200, "{reply}");
        reply["commitment"]["id"].as_str().expect("id").to_owned()
    }

    /// Tick until `cond` holds (bounded); the sleeps give delegated tasks
    /// — real child processes — time to run.
    fn tick_until(
        shared: &Shared,
        id: &str,
        tries: usize,
        cond: impl Fn(&Commitment) -> bool,
    ) -> Commitment {
        for _ in 0..tries {
            tick(shared);
            let c = shared.commitments.get(id).expect("commitment");
            if cond(&c) {
                return c;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        shared.commitments.get(id).expect("commitment")
    }

    #[test]
    fn a_goal_runs_to_verified_close() {
        let (shared, dir) = fixture(json!({}));
        std::fs::write(dir.path().join("ws/ok.txt"), "x").expect("marker");
        let id = create(
            &shared,
            "write the report",
            json!({"success_condition": {"description": "marker exists",
                "checks": [{"type": "files_exist", "workspace": "ws", "paths": ["ok.txt"]}]}}),
        );
        let c = tick_until(&shared, &id, 400, |c| c.state == CommitmentState::Done);
        assert_eq!(c.state, CommitmentState::Done, "{c:?}");
        assert!(
            c.outcome.as_deref().unwrap_or("").contains("verified"),
            "{:?}",
            c.outcome
        );
        assert_eq!(c.spent.attempts, 1);
        let task = shared.tasks.get(&c.board_task).expect("board task");
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.kind, "commitment");
        assert_eq!(task.detail["commitment"], json!(id));
    }

    #[test]
    fn steps_run_in_order_and_each_success_advances() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(
            &shared,
            "two-step goal",
            json!({"steps": [{"objective": "first", "kind": "research"},
                             {"objective": "second"}]}),
        );
        let c = tick_until(&shared, &id, 600, |c| {
            c.plan.iter().all(|s| s.status == StepStatus::Done)
        });
        assert!(c.plan.iter().all(|s| s.status == StepStatus::Done), "{c:?}");
        // With no machine checks the close is unverified but still closes.
        let c = tick_until(&shared, &id, 200, |c| c.state == CommitmentState::Done);
        assert_eq!(c.state, CommitmentState::Done);
        assert!(c.checkpoint.worked.len() >= 2, "{:?}", c.checkpoint.worked);
        assert!(c.checkpoint.worked[0].contains("fake:fake-1"));
    }

    #[test]
    fn failures_climb_the_ladder_and_switch_executors() {
        let (shared, _dir) = fixture(json!({"same_failure_limit": 2, "blocked_sleep_secs": 1}));
        let id = create(
            &shared,
            "FAIL the thing",
            json!({"budget": {"attempts": 30}}),
        );
        // Every attempt fails: 2×L0, then L1 switches model to fake-2 —
        // a new signature — so the L1 retry exhausts fake's catalog and
        // climbs straight to L3 (other), a no-executor wait, then L4
        // decompose → steps finish.
        let c = tick_until(&shared, &id, 6000, |c| c.state == CommitmentState::Done);
        assert_eq!(c.state, CommitmentState::Done, "{c:?}");
        assert!(c.replans >= 2, "ladder climbed: {}", c.replans);
        // Only the L0 retry repeats a signature; new signatures (new
        // model, new executor) are different failures, not repeats.
        assert_eq!(c.repeated_failures, 1);
        assert!(
            c.attempts.iter().any(|a| a.executor == "other"),
            "L3 switched executors: {:?}",
            c.attempts.iter().map(|a| &a.executor).collect::<Vec<_>>()
        );
        assert!(
            c.plan.iter().all(|s| s.status == StepStatus::Done),
            "decomposed plan ran: {:?}",
            c.plan
        );
        assert!(
            c.plan.iter().any(|s| s.objective.starts_with("OK")),
            "the decomposed plan replaced the work: {:?}",
            c.plan
        );
    }

    #[test]
    fn premise_review_can_escalate_to_the_operator() {
        let (shared, _dir) = fixture(json!({"same_failure_limit": 1, "blocked_sleep_secs": 1}));
        // FAIL: every attempt fails; NODECOMP: the decompose task fails
        // too, so the ladder reaches L5's premise review.
        let id = create(
            &shared,
            "FAIL NODECOMP work",
            json!({"budget": {"attempts": 40}}),
        );
        let c = tick_until(&shared, &id, 6000, |c| {
            c.state == CommitmentState::Escalated
                && c.question.as_deref().unwrap_or("").contains("premise")
        });
        assert_eq!(c.state, CommitmentState::Escalated, "{c:?}");
        // The operator's answer re-arms it at L0.
        let (status, _) = handle(
            &shared,
            &json!({"action": "resume", "id": id, "note": "try X instead"}),
            "operator",
        );
        assert_eq!(status, 200);
        let c = shared.commitments.get(&id).expect("c");
        assert_eq!(c.state, CommitmentState::Active);
        assert_eq!(c.replan_level, 0);
    }

    #[test]
    fn budget_exhaustion_escalates_to_the_operator() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(&shared, "do something", json!({"budget": {"attempts": 0}}));
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Escalated);
        assert_eq!(c.state, CommitmentState::Escalated);
        assert!(
            c.question.as_deref().unwrap_or("").contains("budget"),
            "{:?}",
            c.question
        );
        let task = shared.tasks.get(&c.board_task).expect("board");
        assert_eq!(task.status, TaskStatus::AwaitingOperator);
    }

    #[test]
    fn a_restart_interrupts_and_resumes_to_completion() {
        let (shared, dir) = fixture(json!({}));
        let id = create(&shared, "a long goal", json!({}));
        tick(&shared);
        let c = shared.commitments.get(&id).expect("c");
        let running = c.current_task.clone().expect("attempt dispatched");
        // Wait until the external session id is on the board so the
        // resume path can continue it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while shared
            .tasks
            .get(&running)
            .and_then(|t| t.detail["external_session_id"].as_str().map(str::to_owned))
            .is_none()
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            shared.tasks.get(&running).expect("task").detail["external_session_id"]
                .as_str()
                .is_some(),
            "session id reported"
        );
        // Simulate the restart: the board fails the in-flight task, then
        // the whole Shared state reloads from disk (fresh engine view).
        shared.tasks.update(
            &shared.spool,
            &running,
            Some(TaskStatus::Failed),
            Some(("system", "resident の再起動で中断")),
            None,
        );
        let restarted = Shared::new(
            shared.config.clone(),
            Spool::new(dir.path().join("spool"), "test").expect("spool"),
            2,
            None,
        );
        let c2 = restarted.commitments.get(&id).expect("persisted");
        assert_eq!(c2.interruptions, 1);
        assert!(c2.interrupted, "in-flight task flagged for resume");
        assert_eq!(c2.state, CommitmentState::Active);
        // The fake executor supports session resume: the interrupted
        // task's session is continued (a `continues` link), and the
        // commitment still closes.
        let c2 = tick_until(&restarted, &id, 600, |c| c.state == CommitmentState::Done);
        assert_eq!(c2.state, CommitmentState::Done, "{c2:?}");
        assert_eq!(c2.resumes, 1);
        assert_eq!(c2.resumed_progressed, 1);
        assert!(
            c2.attempts
                .iter()
                .any(|a| a.outcome == AttemptOutcome::Interrupted),
            "cut attempt recorded: {:?}",
            c2.attempts
        );
        let continued = c2.current_task.is_none() && c2.attempts.len() >= 2;
        assert!(continued);
        let new_task_id = &c2.attempts.last().expect("attempt").task_id;
        let new_task = restarted.tasks.get(new_task_id).expect("task");
        assert_eq!(new_task.detail["continues"], json!(running));
    }

    #[test]
    fn failure_signatures_ignore_volatile_parts() {
        let detail = json!({"executor": "opencode", "model": "m1"});
        let r1 = json!({"end": "failed", "error": "exit 42 at /tmp/abc failed"});
        let r2 = json!({"end": "failed", "error": "exit 99 at /tmp/abc failed"});
        assert_eq!(
            failure_signature(Some(&r1), &detail, "failed"),
            failure_signature(Some(&r2), &detail, "failed")
        );
        let r3 = json!({"end": "timed_out"});
        assert_ne!(
            failure_signature(Some(&r1), &detail, "failed"),
            failure_signature(Some(&r3), &detail, "failed")
        );
    }

    #[test]
    fn persistence_metrics_cover_the_score() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(&shared, "ok goal", json!({}));
        tick_until(&shared, &id, 400, |c| c.state == CommitmentState::Done);
        let p = shared.commitments.persistence();
        assert_eq!(p["commitments"]["done"], 1);
        assert_eq!(p["totals"]["attempts"], 1);
        assert_eq!(p["persistence_score"]["completion_rate"], json!(1.0));
    }

    #[test]
    fn abandon_is_explicit_and_terminal() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(&shared, "give up", json!({}));
        let (status, _) = handle(
            &shared,
            &json!({"action": "abandon", "id": id, "reason": "not worth it"}),
            "mio",
        );
        assert_eq!(status, 200);
        let c = shared.commitments.get(&id).expect("c");
        assert_eq!(c.state, CommitmentState::Abandoned);
        tick(&shared);
        assert_eq!(
            shared.commitments.get(&id).expect("c").state,
            CommitmentState::Abandoned
        );
    }

    #[test]
    fn abandon_cancels_the_delegated_task_in_flight() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(&shared, "SLEEP forever", json!({}));
        let c = tick_until(&shared, &id, 400, |c| c.current_task.is_some());
        let task_id = c.current_task.clone().expect("in flight");
        let (status, _) = handle(
            &shared,
            &json!({"action": "abandon", "id": id, "reason": "test over"}),
            "operator",
        );
        assert_eq!(status, 200);
        // The running harness process is cancelled, not left orphaned.
        let mut status_seen = None;
        for _ in 0..400 {
            status_seen = shared.tasks.get(&task_id).map(|t| t.status);
            if status_seen == Some(TaskStatus::Cancelled) {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(status_seen, Some(TaskStatus::Cancelled));
        // A terminal commitment is out of the tick's scope.
        tick(&shared);
        assert_eq!(
            shared.commitments.get(&id).expect("c").state,
            CommitmentState::Abandoned
        );
    }

    // -----------------------------------------------------------------
    // Peer delegation: with no local plane the attempt runs on a peer's
    // task plane through `/v1/agents` — same actions, remote transport.

    fn raw_json(body: &Value) -> kamimusuhi_testkit::FixtureResponse {
        let body = body.to_string();
        kamimusuhi_testkit::FixtureResponse::RawHttp {
            response: format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        }
    }

    /// A continuity node with no task plane of its own; `healthy` decides
    /// whether the `mac` peer carries a passing probe record.
    fn fixture_planeless(
        url: &str,
        healthy: bool,
        commitments: Value,
    ) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config: Config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "peers": [{"id": "mac", "role": "cognition", "url": url}],
            "commitments": commitments,
        }))
        .expect("config");
        config.validate().expect("valid");
        let spool = Spool::new(dir.path().join("spool"), "test").expect("spool");
        let shared = Shared::new(config, spool, 1, None);
        if healthy {
            shared
                .peers
                .write()
                .expect("peers")
                .entry("mac".to_owned())
                .or_default()
                .record(Ok(json!({"ok": true})), 1);
        }
        (shared, dir)
    }

    /// The task actions the peer fixture received, in wire order.
    fn peer_actions(server: &kamimusuhi_testkit::FixtureServer) -> Vec<String> {
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/v1/agents")
            .filter_map(|r| {
                serde_json::from_str::<Value>(&r.body).ok()?["action"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn a_planeless_node_delegates_through_a_peer() {
        let running = raw_json(&json!({
            "task_id": "remote-1", "status": "in_progress",
            "executor": "peerbox", "model": "m1"}));
        let done = raw_json(&json!({
            "task_id": "remote-1", "status": "done",
            "executor": "peerbox", "model": "m1",
            "result": {"summary": "remote done", "session_id": "s-x",
                       "cost": {"reported_usd": 0.01}}}));
        // delegate → observe running → observe done → finish's snapshot.
        let server = kamimusuhi_testkit::FixtureServer::start(vec![
            raw_json(&json!({"task_id": "remote-1", "status": "waiting"})),
            running,
            done.clone(),
            done,
        ])
        .expect("fixture");
        let (shared, _dir) = fixture_planeless(
            &format!("http://127.0.0.1:{}", server.port()),
            true,
            json!({}),
        );
        let id = create(&shared, "goal on the peer", json!({}));
        let c = tick_until(&shared, &id, 200, |c| c.state == CommitmentState::Done);
        assert_eq!(c.state, CommitmentState::Done);
        // The attempt ran on the peer's board, with its executor recorded.
        assert_eq!(c.attempts[0].task_id, "remote-1");
        assert_eq!(c.attempts[0].node.as_deref(), Some("mac"));
        assert_eq!(c.attempts[0].executor, "peerbox");
        assert!(c.current_task_node.is_none());
        // The remote task is linked back on the local board task.
        let board = shared.tasks.get(&c.board_task).expect("board");
        assert_eq!(board.detail["attempt_node"], json!("mac"));
        let actions = peer_actions(&server);
        assert_eq!(actions[0], "delegate");
        assert!(actions.iter().skip(1).all(|a| a == "status"));
    }

    #[test]
    fn a_remote_interruption_resumes_the_session_through_the_peer() {
        let failed_no_result = raw_json(&json!({
            "task_id": "remote-1", "status": "failed",
            "executor": "peerbox", "model": "m1"})); // no result → cut mid-run
        let done = raw_json(&json!({
            "task_id": "remote-2", "status": "done",
            "executor": "peerbox", "model": "m1",
            "result": {"summary": "resumed", "session_id": "s-2", "cost": {}}}));
        let server = kamimusuhi_testkit::FixtureServer::start(vec![
            raw_json(&json!({"task_id": "remote-1", "status": "waiting"})),
            failed_no_result.clone(), // observed → Resume
            failed_no_result,         // resume's snapshot
            raw_json(&json!({"task_id": "remote-2", "status": "waiting"})), // continue
            raw_json(&json!({"task_id": "remote-2", "status": "in_progress",
                            "executor": "peerbox", "model": "m1"})),
            done.clone(), // observed → TaskDone
            done,         // finish's snapshot
        ])
        .expect("fixture");
        let (shared, _dir) = fixture_planeless(
            &format!("http://127.0.0.1:{}", server.port()),
            true,
            json!({}),
        );
        let id = create(&shared, "resume me on the peer", json!({}));
        let c = tick_until(&shared, &id, 200, |c| c.state == CommitmentState::Done);
        assert_eq!(c.state, CommitmentState::Done);
        // The cut task counted as an interruption, not a strategy failure.
        assert_eq!(c.resumes, 1);
        assert_eq!(c.attempts[0].outcome, AttemptOutcome::Interrupted);
        assert_eq!(c.attempts[0].node.as_deref(), Some("mac"));
        assert_eq!(c.repeated_failures, 0);
        assert!(peer_actions(&server).contains(&"continue".to_owned()));
    }

    #[test]
    fn abandon_cancels_the_remote_task() {
        let running = raw_json(&json!({
            "task_id": "remote-1", "status": "in_progress",
            "executor": "peerbox", "model": "m1"}));
        let server = kamimusuhi_testkit::FixtureServer::start(vec![
            raw_json(&json!({"task_id": "remote-1", "status": "waiting"})),
            running,
        ])
        .expect("fixture");
        let (shared, _dir) = fixture_planeless(
            &format!("http://127.0.0.1:{}", server.port()),
            true,
            json!({}),
        );
        let id = create(&shared, "stop the remote work", json!({}));
        let c = tick_until(&shared, &id, 200, |c| c.current_task.is_some());
        assert_eq!(c.current_task.as_deref(), Some("remote-1"));
        let (status, _) = handle(
            &shared,
            &json!({"action": "abandon", "id": id, "reason": "test over"}),
            "operator",
        );
        assert_eq!(status, 200);
        let cancel = server
            .requests()
            .iter()
            .filter_map(|r| serde_json::from_str::<Value>(&r.body).ok())
            .find(|b| b["action"].as_str() == Some("cancel"));
        assert_eq!(cancel.expect("cancel call")["id"], json!("remote-1"));
    }

    #[test]
    fn without_a_healthy_peer_the_attempt_sleeps() {
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::ok("unused"),
        )
        .expect("fixture");
        let (shared, _dir) = fixture_planeless(
            &format!("http://127.0.0.1:{}", server.port()),
            false,
            json!({}),
        );
        let id = create(&shared, "nobody can run this", json!({}));
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Sleeping);
        assert_eq!(c.state, CommitmentState::Sleeping);
        assert!(
            c.sleep_reason
                .as_deref()
                .unwrap_or("")
                .contains("task plane")
        );
        assert_eq!(server.request_count(), 0, "unhealthy peer was contacted");
    }

    // -----------------------------------------------------------------
    // The individual's own commitments (tool surface, by "mio") and the
    // notification/escalation visibility built on top of the store.

    #[test]
    fn the_individual_creates_and_closes_her_own_commitment() {
        let (shared, _dir) = fixture(json!({}));
        let (_, reply) = tool(
            &shared,
            "commit_create",
            &json!({"arguments": {"goal": "OK mio's own goal",
                                 "success_condition": {"description": "it runs"}}}),
        );
        assert_eq!(reply["ok"], json!(true), "{reply}");
        let id = reply["result"]["commitment"]["id"]
            .as_str()
            .expect("id")
            .to_owned();
        let c = shared.commitments.get(&id).expect("commitment");
        assert_eq!(c.owner, "mio");
        let c = tick_until(&shared, &id, 200, |c| c.state.terminal());
        assert_eq!(c.state, CommitmentState::Done);
    }

    #[test]
    fn the_individual_cannot_raise_her_own_budget() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(&shared, "bounded", json!({}));
        let (_, reply) = tool(
            &shared,
            "commit_update",
            &json!({"arguments": {"id": id, "budget": {"attempts": 999}}}),
        );
        assert_eq!(reply["ok"], json!(false));
        assert!(
            reply["error"].as_str().unwrap_or("").contains("operator"),
            "{reply}"
        );
    }

    #[test]
    fn the_individual_resumes_an_escalated_commitment() {
        let (shared, _dir) = fixture(json!({}));
        let id = create(
            &shared,
            "ladder exhausted",
            json!({"budget": {"attempts": 0}}),
        );
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Escalated);
        assert_eq!(c.state, CommitmentState::Escalated);
        let (_, reply) = tool(
            &shared,
            "commit_resume",
            &json!({"arguments": {"id": id, "note": "新しい前提でやり直す"}}),
        );
        assert_eq!(reply["ok"], json!(true), "{reply}");
        let c = shared.commitments.get(&id).expect("commitment");
        assert_eq!(c.state, CommitmentState::Active);
        assert_eq!(c.replan_level, 0, "an answer restarts the ladder");
        assert!(c.escalated_at.is_none(), "leaving escalated clears it");
    }

    #[test]
    fn done_and_escalated_queue_notifications_for_the_individual() {
        let (shared, _dir) = fixture(json!({"notify_subject": "commitment-engine"}));
        let id = create(&shared, "OK tell me it worked", json!({}));
        let c = tick_until(&shared, &id, 200, |c| c.state.terminal());
        assert_eq!(c.state, CommitmentState::Done);
        let events = shared.commitments.pending_notifications();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["event"], json!("done"));
        assert_eq!(
            events[0]["commitment"]["goal"],
            json!("OK tell me it worked")
        );

        let id = create(
            &shared,
            "no budget left",
            json!({"budget": {"attempts": 0}}),
        );
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Escalated);
        assert_eq!(c.state, CommitmentState::Escalated);
        let events = shared.commitments.pending_notifications();
        let esc = events
            .iter()
            .find(|e| e["commitment"]["id"] == json!(id))
            .expect("escalation notification");
        assert_eq!(esc["event"], json!("escalated"));
        assert!(
            esc["commitment"]["question"]
                .as_str()
                .unwrap_or("")
                .contains("budget"),
            "{esc}"
        );
        // A resume that changes nothing re-escalates with the same
        // question — told once, not every cycle.
        let (_, reply) = tool(
            &shared,
            "commit_resume",
            &json!({"arguments": {"id": id, "note": "もう一度だけ"}}),
        );
        assert_eq!(reply["ok"], json!(true), "{reply}");
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Escalated);
        assert_eq!(c.state, CommitmentState::Escalated);
        let for_id = |e: &&Value| e["commitment"]["id"] == json!(id);
        assert_eq!(
            shared
                .commitments
                .pending_notifications()
                .iter()
                .filter(for_id)
                .count(),
            1,
            "same question re-notified"
        );
        // Notifications are off by default: a store without
        // notify_subject queues nothing.
        let (plain, _d) = fixture(json!({}));
        let id = create(&plain, "OK quiet", json!({}));
        tick_until(&plain, &id, 200, |c| c.state.terminal());
        assert!(plain.commitments.pending_notifications().is_empty());
    }

    #[test]
    fn an_escalated_commitment_is_reminded_not_forgotten() {
        let (shared, _dir) = fixture(json!({"stall_note_secs": 1}));
        let id = create(
            &shared,
            "needs an answer",
            json!({"budget": {"attempts": 0}}),
        );
        let c = tick_until(&shared, &id, 50, |c| c.state == CommitmentState::Escalated);
        assert!(c.escalated_at.is_some());
        std::thread::sleep(Duration::from_millis(1100));
        let c = tick_until(&shared, &id, 100, |c| {
            c.notes
                .iter()
                .any(|n| n.text.contains("エスカレート継続中"))
        });
        assert!(
            c.notes
                .iter()
                .any(|n| n.text.contains("エスカレート継続中")),
            "no reminder note: {:?}",
            c.notes
        );
        // /status surfaces it with the question and the wait so far.
        let summary = shared.commitments.summary();
        assert_eq!(summary["escalated"][0]["id"], json!(id));
        assert!(
            summary["escalated"][0]["question"]
                .as_str()
                .unwrap_or("")
                .contains("budget")
        );
        assert!(summary["escalated"][0]["waiting_secs"].as_u64() >= Some(1));
    }
}
