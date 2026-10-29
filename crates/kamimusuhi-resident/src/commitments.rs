//! Work the individual took on in a turn, and whether anything started it.
//!
//! A reply like 「調べてみますね」 starts nothing by itself: work continues
//! after the turn only as a delegated task. After each turn the resident
//! reports what is running in the background — the tasks delegated during
//! the turn, or a promise that no task backs. An unbacked promise is put
//! on the board as a `commitment` awaiting the operator, who can start it
//! as a delegated task (`POST /v1/tasks {"action":"start","id":…}`) or
//! cancel it. Nothing here delegates on its own.

use serde_json::{Value, json};

use crate::state::Shared;
use crate::tasks::{NewTask, TaskStatus};

/// Board kind of an unbacked promise.
pub const KIND: &str = "commitment";

/// What `turn_task` left running: `{"state":"started","tasks":[…]}` for
/// delegated tasks, `{"state":"unstarted","task_id":…}` for a promise now
/// on the board, `null` when the reply took nothing on.
pub fn background(
    shared: &Shared,
    turn_task: &str,
    subject: &str,
    message: &str,
    response: &str,
    tool_calls: &Value,
) -> Value {
    let started: Vec<Value> = tool_calls
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|call| call.get("task").filter(|t| t["id"].is_string()).cloned())
        .collect();
    if !started.is_empty() {
        return json!({"state": "started", "tasks": started});
    }
    if !kamimusuhi_core::commitment::promises_background_work(response) {
        return Value::Null;
    }
    let id = shared.tasks.create(
        &shared.spool,
        NewTask {
            title: &format!("未着手の引受: {message}"),
            kind: KIND,
            node: &shared.config.node.id,
            owner: "operator",
            status: TaskStatus::AwaitingOperator,
            depends_on: vec![turn_task.to_owned()],
            detail: json!({"subject": subject, "message": message, "response": response}),
        },
    );
    json!({"state": "unstarted", "task_id": id})
}

/// Start a commitment as a delegated task on this node's task plane, or on
/// the first peer that has one, and close it on the board. Operator only.
/// `objective`, `executor`, `model` and `kind` override the defaults
/// (the operator's original message, automatic selection).
pub fn start(shared: &Shared, request: &Value, by: &str) -> (u16, Value) {
    if by != "operator" {
        return (
            403,
            json!({"error": "commitments are started by the operator"}),
        );
    }
    let id = request["id"].as_str().unwrap_or("").trim();
    let Some(task) = shared.tasks.get(id) else {
        return (404, json!({"error": format!("no task {id}")}));
    };
    if task.kind != KIND {
        return (
            400,
            json!({"error": "only commitment tasks can be started"}),
        );
    }
    if task.status != TaskStatus::AwaitingOperator {
        return (409, json!({"error": "this commitment was already handled"}));
    }
    let objective = request["objective"]
        .as_str()
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .or_else(|| task.detail["message"].as_str())
        .unwrap_or("")
        .to_owned();
    let mut body = json!({
        "action": "delegate",
        "objective": objective,
        "context": [format!(
            "対話中の依頼から作られたタスク。個体はこう返答していた: {}",
            task.detail["response"].as_str().unwrap_or("")
        )],
    });
    for key in ["executor", "model", "kind"] {
        if let Some(value) = request
            .get(key)
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            body[key] = value.clone();
        }
    }
    // 「Devinで調べて」: an agent the operator named in the request is the
    // one to use, not whatever automatic selection prefers.
    if body.get("executor").is_none() {
        let (status, health, _) = crate::task_orchestrator::handle_or_forward(
            shared,
            &json!({"action": "health"}),
            "operator",
        );
        if status == 200
            && let Some(executor) =
                named_executor(task.detail["message"].as_str().unwrap_or(""), &health)
        {
            body["executor"] = json!(executor);
        }
    }
    let (status, reply, node) =
        crate::task_orchestrator::handle_or_forward(shared, &body, "operator");
    if status != 200 {
        let error = reply["error"]
            .to_string()
            .chars()
            .take(200)
            .collect::<String>();
        shared.tasks.update(
            &shared.spool,
            id,
            None,
            Some(("system", &format!("開始できなかった: {error}"))),
            None,
        );
        return (status, reply);
    }
    let task_id = reply["task_id"].as_str().unwrap_or("");
    let executor = reply["executor"].as_str().unwrap_or("?");
    shared.tasks.update(
        &shared.spool,
        id,
        Some(TaskStatus::Done),
        Some((by, &format!("委譲 → {task_id}（{executor} @ {node}）"))),
        Some(json!({"delegated": {"id": task_id, "node": node,
                                  "executor": reply["executor"], "model": reply["model"]}})),
    );
    (
        200,
        json!({"ok": true, "commitment": id, "node": node, "delegated": reply}),
    )
}

/// The enabled executor `message` names (case-insensitive, as a whole
/// word), when exactly one is named.
fn named_executor(message: &str, health: &Value) -> Option<String> {
    let lower = message.to_lowercase();
    let mut named = health["executors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["enabled"].as_bool().unwrap_or(true))
        .filter_map(|e| e["name"].as_str())
        .filter(|name| {
            let name = name.to_lowercase();
            lower.match_indices(&name).any(|(at, _)| {
                let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
                !lower[..at].chars().next_back().is_some_and(word)
                    && !lower[at + name.len()..].chars().next().is_some_and(word)
            })
        });
    let first = named.next()?;
    named.next().is_none().then(|| first.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_named_in_the_request_is_picked() {
        let health = json!({"executors": [
            {"name": "devin", "enabled": true},
            {"name": "opencode", "enabled": true},
            {"name": "agy", "enabled": false}]});
        let pick = |m: &str| named_executor(m, &health);
        assert_eq!(
            pick("実験モジュールで実用に足るものがないかDevinのswe2使って調べて．").as_deref(),
            Some("devin")
        );
        assert_eq!(pick("opencodeで見て").as_deref(), Some("opencode"));
        assert_eq!(pick("調べて"), None);
        assert_eq!(pick("devinとopencodeで比べて"), None, "ambiguous");
        assert_eq!(pick("agyで調べて"), None, "disabled");
        assert_eq!(pick("devinx を使って"), None, "not a whole word");
    }

    fn shared() -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = serde_json::from_value(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()}
        }))
        .expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "pi").expect("spool");
        (Shared::new(config, spool, 1, None), dir)
    }

    #[test]
    fn delegated_tasks_are_reported_as_started() {
        let (shared, _dir) = shared();
        let calls = json!([
            {"name": "library_search", "ok": true},
            {"name": "task_delegate", "ok": true, "task": {"id": "t1", "executor": "devin"}}
        ]);
        let bg = background(&shared, "turn", "me", "調べて", "調べてみますね。", &calls);
        assert_eq!(bg["state"], "started");
        assert_eq!(bg["tasks"][0]["id"], "t1");
        assert!(
            shared.tasks.list(true).is_empty(),
            "nothing extra on the board"
        );
    }

    #[test]
    fn an_unbacked_promise_waits_for_the_operator_and_answers_do_not() {
        let (shared, _dir) = shared();
        let bg = background(
            &shared,
            "turn",
            "me",
            "忘却を調べて",
            "調べてみますね。",
            &json!([]),
        );
        assert_eq!(bg["state"], "unstarted");
        let task = shared
            .tasks
            .get(bg["task_id"].as_str().expect("id"))
            .expect("task");
        assert_eq!(task.kind, KIND);
        assert_eq!(task.status, TaskStatus::AwaitingOperator);
        assert_eq!(task.depends_on, vec!["turn".to_owned()]);
        assert_eq!(task.detail["message"], "忘却を調べて");

        let bg = background(
            &shared,
            "turn",
            "me",
            "こんにちは",
            "こんにちは。",
            &json!([]),
        );
        assert!(bg.is_null());
        // A failed delegation is not a started task.
        let failed = json!([{"name": "task_delegate", "ok": false}]);
        let bg = background(&shared, "turn", "me", "調べて", "調べます。", &failed);
        assert_eq!(bg["state"], "unstarted");
    }

    #[test]
    fn starting_needs_the_operator_a_commitment_and_a_task_plane() {
        let (shared, _dir) = shared();
        let bg = background(
            &shared,
            "turn",
            "me",
            "調べて",
            "調べてみますね。",
            &json!([]),
        );
        let id = bg["task_id"].as_str().expect("id");
        let (status, _) = start(&shared, &json!({"id": id}), "mio");
        assert_eq!(status, 403);
        let (status, _) = start(&shared, &json!({"id": "turn"}), "operator");
        assert_eq!(status, 404);
        // No plane here and no peers: stays open with a note.
        let (status, reply) = start(&shared, &json!({"id": id}), "operator");
        assert_eq!(status, 404, "{reply}");
        let task = shared.tasks.get(id).expect("task");
        assert_eq!(task.status, TaskStatus::AwaitingOperator);
        assert!(
            task.notes
                .last()
                .expect("note")
                .text
                .contains("開始できなかった")
        );
    }
}
