//! `POST /v1/kamimusuhi/talk`: one turn with the individual.
//!
//! Each turn runs `kamimusuhi-runtime talk` against the canonical runtime
//! directory, so the reply comes from the Persona Core with the individual's
//! memory and continuity, and the turn is recorded by the runtime itself.
//! Turns are serialized: the runtime directory has a single writer.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::DialogueConfig;
use crate::state::Shared;
use crate::util::run_with_timeout;

static TURN_LOCK: Mutex<()> = Mutex::new(());

const MAX_MESSAGE_BYTES: usize = 32 * 1024;

fn valid_subject(subject: &str) -> bool {
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
    let _turn = TURN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    *shared
        .current_turn
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(task_id.clone());
    let finish = |status: crate::tasks::TaskStatus, note: &str| {
        *shared
            .current_turn
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
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
    let output = run_with_timeout(
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
        Duration::from_secs(config.timeout_secs),
    );
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let Some(output) = output else {
        finish(crate::tasks::TaskStatus::Failed, "時間切れ");
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
            let route = shared
                .last_route
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(|r| {
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
            let _ = shared.spool.append(
                "conversations/dialogue",
                json!({
                    "subject": subject,
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
            (
                502,
                json!({"error": "talk failed", "stderr_tail": tail.into_iter().rev().collect::<Vec<_>>()}),
            )
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
    let mut turns: Vec<Value> = collected
        .into_iter()
        .filter_map(|(_, line)| serde_json::from_str::<Value>(&line).ok())
        .filter(|v| v["subject"].as_str() == Some(subject.as_str()))
        .filter(|v| seen.insert(v["id"].as_str().unwrap_or("").to_owned()))
        .collect();
    turns.sort_by(|a, b| a["ts"].as_str().cmp(&b["ts"].as_str()));
    let start = turns.len().saturating_sub(limit);
    let turns: Vec<Value> = turns[start..]
        .iter()
        .map(|t| {
            json!({"ts": t["ts"], "message": t["message"], "response": t["response"],
                   "latency_ms": t["latency_ms"], "tool_calls": t["tool_calls"],
                   "route": t["route"]})
        })
        .collect();
    (200, json!({"subject": subject, "turns": turns}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_are_restricted() {
        assert!(valid_subject("eightman"));
        assert!(valid_subject("a.b@c-d_e"));
        assert!(!valid_subject(""));
        assert!(!valid_subject("--dir"));
        assert!(!valid_subject("a b"));
    }
}
