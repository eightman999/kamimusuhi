//! Operator approval for side-effecting tool calls.
//!
//! A tool listed in an MCP server's `approval_required` is shown to the
//! model but never executed on the model's request. The request is queued
//! (persisted in `current_state/approvals.json`) and the model is told it is
//! waiting. Only the operator — through `POST /v1/approvals/decide`, which
//! always requires the node bearer token, even from loopback — can approve
//! it; the resident then executes the call exactly as requested and runs the
//! server's `after_approved` hook (e.g. commit + push).
//!
//! Approval is not a tool, so no model output can grant it.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::Shared;
use crate::util::{atomic_write, iso8601, run_with_timeout, unix_now};

const MAX_PENDING: usize = 50;
const EXPIRE_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Rejected,
    Expired,
    /// Approved and executed successfully.
    Done,
    /// Approved but the call or its hook failed.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub created_at: u64,
    pub server: String,
    pub tool: String,
    pub exposed: String,
    pub arguments: Value,
    pub state: ApprovalState,
    #[serde(default)]
    pub decided_at: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    /// Task-board entry tracking this request.
    #[serde(default)]
    pub task_id: Option<String>,
}

impl Approval {
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["created"] = json!(iso8601(self.created_at));
        if let Some(t) = self.decided_at {
            v["decided"] = json!(iso8601(t));
        }
        v
    }
}

pub struct ApprovalQueue {
    path: PathBuf,
    items: Mutex<Vec<Approval>>,
}

impl ApprovalQueue {
    pub fn load(path: PathBuf) -> Self {
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    fn persist(&self, items: &[Approval]) {
        if let Ok(bytes) = serde_json::to_vec_pretty(items) {
            let _ = atomic_write(&self.path, &bytes);
        }
    }

    fn expire(items: &mut Vec<Approval>) {
        let now = unix_now();
        for item in items.iter_mut() {
            if item.state == ApprovalState::Pending
                && now.saturating_sub(item.created_at) > EXPIRE_SECS
            {
                item.state = ApprovalState::Expired;
                item.decided_at = Some(now);
            }
        }
        // Keep the recent history bounded.
        let len = items.len();
        if len > 200 {
            items.drain(..len - 200);
        }
    }

    pub fn enqueue(
        &self,
        server: &str,
        tool: &str,
        exposed: &str,
        arguments: Value,
        task_id: Option<String>,
    ) -> Result<Approval, String> {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut items);
        let pending = items
            .iter()
            .filter(|a| a.state == ApprovalState::Pending)
            .count();
        if pending >= MAX_PENDING {
            return Err("too many pending approvals".to_owned());
        }
        let now = unix_now();
        let approval = Approval {
            id: format!("a{:x}", crate::util::unix_now_ms()),
            created_at: now,
            server: server.to_owned(),
            tool: tool.to_owned(),
            exposed: exposed.to_owned(),
            arguments,
            state: ApprovalState::Pending,
            decided_at: None,
            result: None,
            task_id,
        };
        items.push(approval.clone());
        self.persist(&items);
        Ok(approval)
    }

    /// Attach the task-board entry that tracks this request.
    pub fn set_task(&self, id: &str, task_id: &str) {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(item) = items.iter_mut().find(|a| a.id == id) {
            item.task_id = Some(task_id.to_owned());
        }
        self.persist(&items);
    }

    pub fn list(&self, pending_only: bool) -> Vec<Approval> {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut items);
        items
            .iter()
            .filter(|a| !pending_only || a.state == ApprovalState::Pending)
            .cloned()
            .collect()
    }

    /// Take a pending item for decision (marks it decided under the lock so
    /// it cannot be executed twice).
    fn claim(&self, id: &str, state: ApprovalState) -> Result<Approval, String> {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut items);
        let item = items
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no approval {id}"))?;
        if item.state != ApprovalState::Pending {
            return Err(format!("approval {id} is already {:?}", item.state));
        }
        item.state = state;
        item.decided_at = Some(unix_now());
        let claimed = item.clone();
        self.persist(&items);
        Ok(claimed)
    }

    fn finish(&self, id: &str, state: ApprovalState, result: Value) {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(item) = items.iter_mut().find(|a| a.id == id) {
            item.state = state;
            item.result = Some(result);
        }
        self.persist(&items);
    }
}

/// `POST /v1/approvals/decide` — `{"id": "...", "decision": "approve"|"reject"}`.
pub fn decide(shared: &Shared, request: &Value) -> (u16, Value) {
    let id = request["id"].as_str().unwrap_or("");
    let approve = match request["decision"].as_str() {
        Some("approve") => true,
        Some("reject") => false,
        _ => return (400, json!({"error": "decision must be approve or reject"})),
    };
    if !approve {
        return match shared.approvals.claim(id, ApprovalState::Rejected) {
            Ok(a) => {
                if let Some(task) = &a.task_id {
                    shared.tasks.update(
                        &shared.spool,
                        task,
                        Some(crate::tasks::TaskStatus::Failed),
                        Some(("operator", "却下")),
                        None,
                    );
                }
                let _ = shared.spool.append(
                    "logs/approvals",
                    json!({"event": "rejected", "approval": a.view()}),
                );
                (200, json!({"ok": true, "approval": a.view()}))
            }
            Err(e) => (409, json!({"error": e})),
        };
    }
    let claimed = match shared.approvals.claim(id, ApprovalState::Failed) {
        Ok(a) => a,
        Err(e) => return (409, json!({"error": e})),
    };
    let Some(server) = shared.mcp.iter().find(|m| m.config.name == claimed.server) else {
        let result = json!({"error": format!("server {} is not configured", claimed.server)});
        shared
            .approvals
            .finish(id, ApprovalState::Failed, result.clone());
        return (200, json!({"ok": false, "result": result}));
    };
    if let Some(task) = &claimed.task_id {
        shared.tasks.update(
            &shared.spool,
            task,
            Some(crate::tasks::TaskStatus::InProgress),
            Some(("operator", "承認 → 実行")),
            None,
        );
    }
    let outcome = server.call_approved(&claimed.tool, &claimed.arguments);
    let (ok, mut result) = match outcome {
        Ok(raw) => crate::mcp::normalize(&raw),
        Err(e) => (false, json!({"error": e})),
    };
    if ok && let Some(hook) = &server.config.after_approved {
        let args: Vec<String> = hook
            .iter()
            .skip(1)
            .map(|a| {
                a.replace("{id}", &claimed.id)
                    .replace("{tool}", &claimed.tool)
            })
            .collect();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let hook_out = hook
            .first()
            .and_then(|cmd| run_with_timeout(cmd, &refs, Duration::from_secs(180)));
        result["after_approved"] = match hook_out {
            Some(out) => json!({"ok": out.success,
                "stdout": out.stdout.lines().rev().take(8).collect::<Vec<_>>(),
                "stderr": out.stderr.lines().rev().take(8).collect::<Vec<_>>()}),
            None => json!({"ok": false, "error": "hook timed out or could not start"}),
        };
    }
    let hook_ok = result["after_approved"]["ok"].as_bool().unwrap_or(true);
    if let Some(task) = &claimed.task_id {
        let status = if ok {
            crate::tasks::TaskStatus::Done
        } else {
            crate::tasks::TaskStatus::Failed
        };
        shared
            .tasks
            .update(&shared.spool, task, Some(status), None, None);
        if ok && result.get("after_approved").is_some() {
            let line = result["after_approved"]["stdout"][0]
                .as_str()
                .or_else(|| result["after_approved"]["stderr"][0].as_str())
                .unwrap_or("")
                .to_owned();
            let commit = shared.tasks.create(
                &shared.spool,
                crate::tasks::NewTask {
                    title: &format!("反映: {} ({})", claimed.server, claimed.tool),
                    kind: "commit",
                    node: &shared.config.node.id,
                    owner: "system",
                    status: if hook_ok {
                        crate::tasks::TaskStatus::Done
                    } else {
                        crate::tasks::TaskStatus::Failed
                    },
                    depends_on: vec![task.clone()],
                    detail: json!({"approval": claimed.id}),
                },
            );
            shared
                .tasks
                .update(&shared.spool, &commit, None, Some(("system", &line)), None);
        }
    }
    let state = if ok && hook_ok {
        ApprovalState::Done
    } else {
        ApprovalState::Failed
    };
    shared.approvals.finish(id, state.clone(), result.clone());
    let _ = shared.spool.append(
        "logs/approvals",
        json!({"event": "approved", "id": id, "tool": claimed.exposed, "state": state,
               "arguments": claimed.arguments, "result": result}),
    );
    (
        200,
        json!({"ok": ok && hook_ok, "state": state, "result": result}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_persists_and_decides_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("approvals.json");
        let queue = ApprovalQueue::load(path.clone());
        let a = queue
            .enqueue(
                "obsidian",
                "write_note",
                "mcp__obsidian__write_note",
                json!({"path": "x.md"}),
                None,
            )
            .expect("enqueue");
        assert_eq!(queue.list(true).len(), 1);

        let reloaded = ApprovalQueue::load(path);
        assert_eq!(reloaded.list(true)[0].id, a.id, "survives restart");

        reloaded
            .claim(&a.id, ApprovalState::Rejected)
            .expect("first decision");
        assert!(
            reloaded.claim(&a.id, ApprovalState::Done).is_err(),
            "no second decision"
        );
        assert!(reloaded.list(true).is_empty());
    }
}
