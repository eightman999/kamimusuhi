//! `POST /v1/kamimusuhi/talk`: one turn with the individual.
//!
//! Each turn runs `kamimusuhi-runtime talk` against the canonical runtime
//! directory, so the reply comes from the Persona Core with the individual's
//! memory and continuity, and the turn is recorded by the runtime itself.
//!
//! Up to `max_concurrent` turns run at once, each in its own runtime
//! process; the runtime serializes the part that writes to the individual
//! (writer claim and draft submission) behind its own directory lock. Turns
//! for the same subject run one at a time, in arrival order, so each sees
//! the previous one's reply in its context.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use kamimusuhi_resource_http::http::TURN_ENV;
use serde_json::{Value, json};

use crate::config::DialogueConfig;
use crate::state::Shared;
use crate::util::run_with_timeout_env;

/// Admission for dialogue turns: a global limit plus one turn per subject.
struct TurnGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

struct GateState {
    next_ticket: u64,
    /// Waiting (ticket, subject), oldest first.
    waiting: VecDeque<(u64, String)>,
    running: Vec<String>,
}

static GATE: TurnGate = TurnGate::new();

/// Holds one admitted turn; dropping it admits the next.
struct TurnSlot {
    gate: &'static TurnGate,
    subject: String,
}

impl TurnGate {
    const fn new() -> Self {
        Self {
            state: Mutex::new(GateState {
                next_ticket: 0,
                waiting: VecDeque::new(),
                running: Vec::new(),
            }),
            changed: Condvar::new(),
        }
    }

    fn admit(&'static self, subject: &str, max_concurrent: usize) -> TurnSlot {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push_back((ticket, subject.to_owned()));
        loop {
            // A turn may start when there is room and no earlier waiter for
            // its subject is ahead of it; a busy subject never blocks others.
            let ready = state.running.len() < max_concurrent.max(1)
                && !state.running.iter().any(|s| s == subject)
                && state
                    .waiting
                    .iter()
                    .find(|(_, s)| s == subject)
                    .is_some_and(|(t, _)| *t == ticket);
            if ready {
                state.waiting.retain(|(t, _)| *t != ticket);
                state.running.push(subject.to_owned());
                return TurnSlot {
                    gate: self,
                    subject: subject.to_owned(),
                };
            }
            state = self.changed.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
}

impl Drop for TurnSlot {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(index) = state.running.iter().position(|s| *s == self.subject) {
            state.running.remove(index);
        }
        drop(state);
        self.gate.changed.notify_all();
    }
}

const MAX_MESSAGE_BYTES: usize = 32 * 1024;

pub(crate) fn valid_subject(subject: &str) -> bool {
    // Must start alphanumeric so it can never be parsed as a CLI flag.
    subject
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric())
        && subject.len() <= 64
        && subject
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

/// Returns (HTTP status, body).
pub fn talk(shared: &Shared, config: &DialogueConfig, request: &Value) -> (u16, Value) {
    let message = request.get("message").and_then(Value::as_str).unwrap_or("");
    let subject = request
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or("local-user");
    if message.trim().is_empty() || message.len() > MAX_MESSAGE_BYTES {
        return (400, json!({"error": "message must be 1..32KiB of text"}));
    }
    if !valid_subject(subject) {
        return (
            400,
            json!({"error": "subject must be 1-64 chars of [A-Za-z0-9-_.@]"}),
        );
    }
    if !config.dir.join("kamimusuhi.sqlite").is_file() {
        return (
            503,
            json!({"error": "individual is not initialized on this node"}),
        );
    }
    let _slot = GATE.admit(subject, config.max_concurrent);
    let task_id = shared.tasks.create(
        &shared.spool,
        crate::tasks::NewTask {
            title: &format!("対話: {message}"),
            kind: "dialogue",
            node: &shared.config.node.id,
            owner: "operator",
            status: crate::tasks::TaskStatus::InProgress,
            depends_on: Vec::new(),
            detail: json!({"subject": subject}),
        },
    );
    shared.begin_turn(&task_id);
    // Journal the intake before the turn runs: a message we received is
    // part of the dialogue history even if the runtime dies mid-turn —
    // timeout, crash, or a restart all leave this record behind. The
    // outcome lands as a second record under the same `task_id`.
    let _ = shared.spool.append_sync(
        "conversations/dialogue",
        json!({
            "kind": "received",
            "subject": subject,
            "task_id": task_id,
            "message": message,
        }),
    );
    let finish = |status: crate::tasks::TaskStatus, note: &str| {
        shared.tasks.update(
            &shared.spool,
            &task_id,
            Some(status),
            Some(("system", note)),
            None,
        );
    };
    let started = Instant::now();
    let dir = config.dir.to_string_lossy().into_owned();
    let binary = config.runtime_binary.to_string_lossy().into_owned();
    let output = run_with_timeout_env(
        &binary,
        &[
            "talk",
            "--dir",
            &dir,
            "--subject",
            subject,
            "--privacy",
            &config.privacy,
            "--message",
            message,
        ],
        &[(TURN_ENV, &task_id)],
        Duration::from_secs(config.timeout_secs),
    );
    // This turn's own routing event, not whichever turn routed last.
    let turn_route = shared.end_turn(&task_id);
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let Some(output) = output else {
        finish(crate::tasks::TaskStatus::Failed, "時間切れ");
        let _ = shared.spool.append_sync(
            "conversations/dialogue",
            json!({"kind": "failed", "subject": subject, "task_id": task_id,
                   "message": message, "error": "timeout", "latency_ms": latency_ms}),
        );
        return (504, json!({"error": "talk timed out or could not start"}));
    };
    let reply = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok());
    match reply {
        Some(reply) if output.success => {
            let preview: String = reply["response"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            finish(
                crate::tasks::TaskStatus::Done,
                &format!("応答 {latency_ms}ms: {preview}"),
            );
            // The routing event the router recorded while serving this
            // turn's upstream call(s): tier, model, tokens, and the cost
            // basis. `cost_usd` is absent when the plan's price is unknown —
            // never rendered as zero.
            let route = turn_route.as_ref().map(|r| {
                json!({
                    "tier": r.tier,
                    "model": r.model,
                    "billing": r.billing,
                    "latency_ms": r.latency_ms,
                    "prompt_tokens": r.prompt_tokens,
                    "completion_tokens": r.completion_tokens,
                    "cached_tokens": r.cached_tokens,
                    "cost_usd": r.cost_usd,
                    "cost_kind": r.cost_kind,
                })
            });
            let _ = shared.spool.append_sync(
                "conversations/dialogue",
                json!({
                    "kind": "answered",
                    "subject": subject,
                    "task_id": task_id,
                    "latency_ms": latency_ms,
                    "message": message,
                    "response": reply.get("response"),
                    "turn_id": reply.get("turn_id"),
                    "session_id": reply.get("session_id"),
                    "individual_id": reply.get("individual_id"),
                    "tool_calls": reply.get("tool_calls"),
                    "route": route,
                }),
            );
            (
                200,
                json!({
                    "response": reply.get("response"),
                    "individual_id": reply.get("individual_id"),
                    "turn_id": reply.get("turn_id"),
                    "subject": subject,
                    "latency_ms": latency_ms,
                    "tier": route.as_ref().and_then(|r| r["tier"].as_str().map(str::to_owned)),
                    "route": route,
                    "tool_calls": reply.get("tool_calls").cloned().unwrap_or_else(|| json!([])),
                    "reference_lookups": reply.get("reference_lookups"),
                }),
            )
        }
        _ => {
            finish(crate::tasks::TaskStatus::Failed, "応答生成に失敗");
            let tail: Vec<&str> = output.stderr.lines().rev().take(5).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            let _ = shared.spool.append_sync(
                "conversations/dialogue",
                json!({"kind": "failed", "subject": subject, "task_id": task_id,
                       "message": message, "error": "generation_failed",
                       "stderr_tail": tail, "latency_ms": latency_ms}),
            );
            (502, json!({"error": "talk failed", "stderr_tail": tail}))
        }
    }
}

/// Recent dialogue turns for `subject`, oldest first, from the journal
/// (NAS copy when reachable, plus what is still in the local spool).
pub fn history(shared: &Shared, request: &Value) -> (u16, Value) {
    let subject = request["subject"]
        .as_str()
        .unwrap_or("local-user")
        .to_owned();
    let limit = usize::try_from(request["limit"].as_u64().unwrap_or(40))
        .unwrap_or(40)
        .clamp(1, 200);
    let node = shared.config.node.id.clone();
    let spool_dir = shared
        .spool
        .root()
        .join("conversations/dialogue")
        .join(&node);
    let nas_dir = shared
        .config
        .nas
        .as_ref()
        .filter(|_| shared.nas_healthy())
        .map(|n| n.root.join("conversations/dialogue").join(&node));
    // Reading the NAS must not hang this request on a sick mount.
    let collected = crate::util::call_with_timeout(Duration::from_secs(8), move || {
        let mut lines: Vec<(String, String)> = Vec::new();
        for dir in nas_dir.into_iter().chain(std::iter::once(spool_dir)) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
            files.sort();
            // The last few days are enough for a conversation view.
            for file in files.iter().rev().take(4) {
                if let Ok(text) = std::fs::read_to_string(file) {
                    for line in text.lines() {
                        lines.push((file.display().to_string(), line.to_owned()));
                    }
                }
            }
        }
        lines
    })
    .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let all: Vec<Value> = collected
        .into_iter()
        .filter_map(|(_, line)| serde_json::from_str::<Value>(&line).ok())
        .filter(|v| v["subject"].as_str() == Some(subject.as_str()))
        .filter(|v| seen.insert(v["id"].as_str().unwrap_or("").to_owned()))
        .collect();
    // A `received` intake record with a matching outcome record is folded
    // into it; a `received` with no outcome stands on its own — the turn
    // died and this is the only trace that it was ever heard.
    let answered: std::collections::HashSet<String> = all
        .iter()
        .filter(|v| v["kind"].as_str() != Some("received"))
        .filter_map(|v| v["task_id"].as_str().map(str::to_owned))
        .collect();
    let mut turns: Vec<Value> = all
        .into_iter()
        .filter(|v| {
            v["kind"].as_str() != Some("received")
                || !answered.contains(v["task_id"].as_str().unwrap_or(""))
        })
        .collect();
    turns.sort_by(|a, b| a["ts"].as_str().cmp(&b["ts"].as_str()));
    let start = turns.len().saturating_sub(limit);
    let turns: Vec<Value> = turns[start..]
        .iter()
        .map(|t| {
            json!({"ts": t["ts"], "kind": t["kind"].as_str().unwrap_or("answered"),
                   "task_id": t["task_id"], "message": t["message"],
                   "response": t["response"], "error": t["error"],
                   "latency_ms": t["latency_ms"], "tool_calls": t["tool_calls"],
                   "route": t["route"]})
        })
        .collect();
    (200, json!({"subject": subject, "turns": turns}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> &'static TurnGate {
        Box::leak(Box::new(TurnGate::new()))
    }

    /// Admit `subject` on a helper thread; the receiver fires once admitted
    /// and the slot is held until the returned sender is dropped.
    fn admit_async(
        gate: &'static TurnGate,
        subject: &str,
        max: usize,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (admitted_tx, admitted_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let subject = subject.to_owned();
        std::thread::spawn(move || {
            let _slot = gate.admit(&subject, max);
            let _ = admitted_tx.send(());
            let _ = release_rx.recv();
        });
        (admitted_rx, release_tx)
    }

    const SOON: Duration = Duration::from_millis(200);

    #[test]
    fn different_subjects_run_together_up_to_the_limit() {
        let gate = gate();
        let (a, release_a) = admit_async(gate, "desktop", 2);
        let (b, _release_b) = admit_async(gate, "discord", 2);
        a.recv_timeout(SOON).expect("first admitted");
        b.recv_timeout(SOON)
            .expect("second runs alongside the first");
        let (c, _release_c) = admit_async(gate, "third", 2);
        assert!(c.recv_timeout(SOON).is_err(), "limit of 2 holds");
        drop(release_a);
        c.recv_timeout(SOON).expect("admitted once a slot frees");
    }

    #[test]
    fn the_same_subject_waits_without_blocking_others() {
        let gate = gate();
        let (first, release_first) = admit_async(gate, "eightman", 2);
        first.recv_timeout(SOON).expect("first admitted");
        let (second, _release_second) = admit_async(gate, "eightman", 2);
        assert!(second.recv_timeout(SOON).is_err(), "same subject waits");
        let (other, _release_other) = admit_async(gate, "someone-else", 2);
        other
            .recv_timeout(SOON)
            .expect("a waiting subject does not block another");
        drop(release_first);
        second.recv_timeout(SOON).expect("runs after the first");
    }

    #[test]
    fn subjects_are_restricted() {
        assert!(valid_subject("eightman"));
        assert!(valid_subject("a.b@c-d_e"));
        assert!(!valid_subject(""));
        assert!(!valid_subject("--dir"));
        assert!(!valid_subject("a b"));
    }

    /// A resident whose `runtime_binary` is a shell script, so `talk`
    /// runs end to end without a real runtime.
    #[cfg(unix)]
    fn shared_with_runtime(dir: &std::path::Path, script_body: &str) -> (Shared, DialogueConfig) {
        let rt = dir.join("rt");
        std::fs::create_dir_all(&rt).expect("rt dir");
        std::fs::write(rt.join("kamimusuhi.sqlite"), b"").expect("db marker");
        let bin = dir.join("fake-runtime");
        std::fs::write(&bin, format!("#!/bin/sh\n{script_body}\n")).expect("script");
        let mut perms = std::fs::metadata(&bin).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&bin, perms).expect("chmod");
        let config: crate::config::Config = serde_json::from_value(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir},
            "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 5},
        }))
        .expect("config");
        config.validate().expect("valid");
        let spool = crate::spool::Spool::new(dir.join("spool"), "pi").expect("spool");
        let shared = Shared::new(config, spool, 1, None);
        let dialogue = shared.config.dialogue.clone().expect("dialogue");
        (shared, dialogue)
    }

    /// The dialogue journal records for one subject, in file order.
    #[cfg(unix)]
    fn journal_records(shared: &Shared) -> Vec<Value> {
        let path = shared
            .spool
            .root()
            .join("conversations/dialogue/pi")
            .join(format!(
                "{}.jsonl",
                crate::util::utc_date(crate::util::unix_now())
            ));
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_successful_turn_journals_intake_then_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, cfg) = shared_with_runtime(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"pong\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"individual_id\":\"mio\"}'",
        );
        let (status, body) = talk(&shared, &cfg, &json!({"subject": "t", "message": "ping"}));
        assert_eq!(status, 200, "{body}");
        let records = journal_records(&shared);
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records[0]["kind"], json!("received"));
        assert_eq!(records[1]["kind"], json!("answered"));
        assert_eq!(records[0]["task_id"], records[1]["task_id"]);
        // History folds intake into its outcome.
        let (_, body) = history(&shared, &json!({"subject": "t"}));
        let turns = body["turns"].as_array().expect("turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["kind"], json!("answered"));
        assert_eq!(turns[0]["response"], json!("pong"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_turn_still_leaves_the_received_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, cfg) = shared_with_runtime(dir.path(), "echo broken >&2; exit 1");
        let (status, _) = talk(&shared, &cfg, &json!({"subject": "t", "message": "ping"}));
        assert_eq!(status, 502);
        let records = journal_records(&shared);
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records[0]["kind"], json!("received"));
        assert_eq!(records[0]["message"], json!("ping"));
        assert_eq!(records[1]["kind"], json!("failed"));
        assert_eq!(records[1]["error"], json!("generation_failed"));
        // History shows the heard-but-unanswered message.
        let (_, body) = history(&shared, &json!({"subject": "t"}));
        let turns = body["turns"].as_array().expect("turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["kind"], json!("failed"));
        assert_eq!(turns[0]["message"], json!("ping"));
        assert!(turns[0]["response"].is_null());
    }
}
