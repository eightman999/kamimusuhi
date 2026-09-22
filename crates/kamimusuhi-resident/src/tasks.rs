//! Task board: what the individual and its nodes are working on, in
//! parallel, and how the pieces connect.
//!
//! Tasks are recorded automatically from resident activity — a dialogue
//! turn, the tool calls made during it (including ones executed on a peer),
//! approval requests, the commit that follows an approval, scheduled jobs —
//! and can also be created by the operator or by the individual itself
//! (`task_*` tools). `depends_on` links a task to the tasks it follows, so
//! the board can show predecessor → successor chains.
//!
//! The board is observational: nothing here grants authority or changes
//! canonical state. It is persisted in `current_state/tasks.json` and every
//! change is journaled under `logs/tasks/`.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::spool::Spool;
use crate::util::{atomic_write, iso8601, unix_now, unix_now_ms};

const KEEP_TASKS: usize = 400;
const DONE_VISIBLE_SECS: u64 = 3 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Being worked on now.
    InProgress,
    /// Blocked on an operator decision.
    AwaitingOperator,
    /// Queued / waiting for its turn or a dependency.
    Waiting,
    OnHold,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "in_progress" => Self::InProgress,
            "awaiting_operator" => Self::AwaitingOperator,
            "waiting" => Self::Waiting,
            "on_hold" => Self::OnHold,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    const fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNote {
    pub at: u64,
    pub by: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    /// dialogue | tool | approval | commit | job | manual | agent
    pub kind: String,
    /// Node doing the work (`pi`, `llm_master`, …).
    pub node: String,
    /// operator | mio (the individual) | system
    pub owner: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub notes: Vec<TaskNote>,
    #[serde(default)]
    pub detail: Value,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub finished_at: Option<u64>,
}

impl Task {
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["created"] = json!(iso8601(self.created_at));
        v["updated"] = json!(iso8601(self.updated_at));
        if let Some(t) = self.finished_at {
            v["finished"] = json!(iso8601(t));
            v["duration_secs"] = json!(t.saturating_sub(self.created_at));
        }
        v
    }
}

/// Fields for a new task.
pub struct NewTask<'a> {
    pub title: &'a str,
    pub kind: &'a str,
    pub node: &'a str,
    pub owner: &'a str,
    pub status: TaskStatus,
    pub depends_on: Vec<String>,
    pub detail: Value,
}

pub struct TaskBoard {
    path: PathBuf,
    tasks: Mutex<Vec<Task>>,
}

fn title_limit(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(80).collect();
    if one_line.chars().count() > 80 {
        out.push('…');
    }
    out
}

impl TaskBoard {
    pub fn load(path: PathBuf) -> Self {
        let mut tasks: Vec<Task> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Anything still "running" after a restart was interrupted.
        let now = unix_now();
        for task in &mut tasks {
            if task.status == TaskStatus::InProgress
                && task.kind != "manual"
                && task.kind != "agent"
            {
                task.status = TaskStatus::Failed;
                task.finished_at = Some(now);
                task.notes.push(TaskNote {
                    at: now,
                    by: "system".to_owned(),
                    text: "resident の再起動で中断".to_owned(),
                });
            }
        }
        Self {
            path,
            tasks: Mutex::new(tasks),
        }
    }

    fn persist(&self, tasks: &mut Vec<Task>) {
        let len = tasks.len();
        if len > KEEP_TASKS {
            tasks.drain(..len - KEEP_TASKS);
        }
        if let Ok(bytes) = serde_json::to_vec(tasks) {
            let _ = atomic_write(&self.path, &bytes);
        }
    }

    pub fn create(&self, spool: &Spool, new: NewTask<'_>) -> String {
        let now = unix_now();
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        // Ids are millisecond stamps; bump on collision.
        let mut id = format!("t{:x}", unix_now_ms());
        while tasks.iter().any(|t| t.id == id) {
            id.push('x');
        }
        let task = Task {
            id: id.clone(),
            title: title_limit(new.title),
            status: new.status,
            kind: new.kind.to_owned(),
            node: new.node.to_owned(),
            owner: new.owner.to_owned(),
            depends_on: new.depends_on,
            notes: Vec::new(),
            detail: new.detail,
            created_at: now,
            updated_at: now,
            finished_at: new.status.finished().then_some(now),
        };
        let view = task.view();
        tasks.push(task);
        self.persist(&mut tasks);
        drop(tasks);
        let _ = spool.append("logs/tasks", json!({"event": "created", "task": view}));
        id
    }

    /// Update status and/or append a note. Unknown ids are ignored.
    pub fn update(
        &self,
        spool: &Spool,
        id: &str,
        status: Option<TaskStatus>,
        note: Option<(&str, &str)>,
        detail: Option<Value>,
    ) -> Option<Task> {
        let now = unix_now();
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        let task = tasks.iter_mut().find(|t| t.id == id)?;
        if let Some(status) = status {
            task.status = status;
            task.finished_at = status.finished().then_some(now);
        }
        if let Some((by, text)) = note.filter(|(_, t)| !t.trim().is_empty()) {
            task.notes.push(TaskNote {
                at: now,
                by: by.to_owned(),
                text: text.chars().take(600).collect(),
            });
            if task.notes.len() > 30 {
                task.notes.remove(0);
            }
        }
        if let Some(Value::Object(extra)) = detail {
            if !task.detail.is_object() {
                task.detail = json!({});
            }
            for (k, v) in extra {
                task.detail[k] = v;
            }
        }
        task.updated_at = now;
        let updated = task.clone();
        self.persist(&mut tasks);
        drop(tasks);
        let _ = spool.append(
            "logs/tasks",
            json!({"event": "updated", "task": updated.view()}),
        );
        Some(updated)
    }

    /// Board contents: open tasks plus recently finished ones.
    pub fn list(&self, include_old_done: bool) -> Vec<Value> {
        let now = unix_now();
        let tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks
            .iter()
            .filter(|t| {
                include_old_done
                    || !t.status.finished()
                    || now.saturating_sub(t.finished_at.unwrap_or(now)) < DONE_VISIBLE_SECS
            })
            .map(Task::view)
            .collect()
    }

    /// The task recorded for a long-lived source (e.g. one per job).
    pub fn find_by_detail(&self, kind: &str, key: &str, value: &str) -> Option<String> {
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .rev()
            .find(|t| t.kind == kind && t.detail[key].as_str() == Some(value))
            .map(|t| t.id.clone())
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }
}

/// `POST /v1/tasks` — operator or individual actions on the board.
///
/// * `{"action":"list"}`
/// * `{"action":"create","title":"…","depends_on":["t…"],"status":"waiting","note":"…"}`
/// * `{"action":"update","id":"t…","status":"done","note":"…"}`
///
/// `by` is `operator` for authenticated clients and `mio` for the
/// individual's tool calls; system tasks cannot be edited by the individual.
pub fn handle(shared: &crate::state::Shared, request: &Value, by: &str) -> (u16, Value) {
    let board = &shared.tasks;
    let text = |k: &str| request[k].as_str().unwrap_or("").trim().to_owned();
    match request["action"].as_str().unwrap_or("list") {
        "list" => (
            200,
            json!({"tasks": board.list(request["all"].as_bool().unwrap_or(false))}),
        ),
        "create" => {
            let title = text("title");
            if title.is_empty() {
                return (400, json!({"error": "title is required"}));
            }
            let status = TaskStatus::parse(&text("status")).unwrap_or(TaskStatus::Waiting);
            let depends_on: Vec<String> = request["depends_on"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .filter(|id| board.get(id).is_some())
                .take(10)
                .collect();
            let node = request["node"]
                .as_str()
                .filter(|n| !n.is_empty() && n.len() <= 32)
                .unwrap_or(&shared.config.node.id)
                .to_owned();
            let id = board.create(
                &shared.spool,
                NewTask {
                    title: &title,
                    kind: if by == "operator" { "manual" } else { "agent" },
                    node: &node,
                    owner: by,
                    status,
                    depends_on,
                    detail: json!({}),
                },
            );
            let note = text("note");
            if !note.is_empty() {
                board.update(&shared.spool, &id, None, Some((by, &note)), None);
            }
            (
                200,
                json!({"ok": true, "task": board.get(&id).map(|t| t.view())}),
            )
        }
        "update" => {
            let id = text("id");
            let Some(existing) = board.get(&id) else {
                return (404, json!({"error": format!("no task {id}")}));
            };
            // The individual may only edit tasks it (or the operator) planned.
            if by != "operator" && !matches!(existing.kind.as_str(), "agent" | "manual") {
                return (403, json!({"error": "system-recorded tasks are read-only"}));
            }
            let status = request["status"].as_str().map(TaskStatus::parse);
            if let Some(None) = status {
                return (400, json!({"error": "unknown status"}));
            }
            let note = text("note");
            let updated = board.update(
                &shared.spool,
                &id,
                status.flatten(),
                (!note.is_empty()).then_some((by, note.as_str())),
                None,
            );
            (200, json!({"ok": true, "task": updated.map(|t| t.view())}))
        }
        _ => (
            400,
            json!({"error": "action must be list, create or update"}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_update_and_restart_interrupts_running_system_tasks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spool = Spool::new(dir.path().join("spool"), "pi").expect("spool");
        let path = dir.path().join("tasks.json");
        let board = TaskBoard::load(path.clone());
        let turn = board.create(
            &spool,
            NewTask {
                title: "対話: こんにちは",
                kind: "dialogue",
                node: "pi",
                owner: "system",
                status: TaskStatus::InProgress,
                depends_on: Vec::new(),
                detail: json!({}),
            },
        );
        let tool = board.create(
            &spool,
            NewTask {
                title: "json_get",
                kind: "tool",
                node: "llm_master",
                owner: "mio",
                status: TaskStatus::Done,
                depends_on: vec![turn.clone()],
                detail: json!({}),
            },
        );
        assert_ne!(turn, tool);
        assert_eq!(
            board.get(&tool).expect("tool").depends_on,
            vec![turn.clone()]
        );
        board.update(&spool, &turn, None, Some(("system", "note")), None);

        let reloaded = TaskBoard::load(path);
        let t = reloaded.get(&turn).expect("persisted");
        assert_eq!(
            t.status,
            TaskStatus::Failed,
            "running system task was interrupted"
        );
        assert_eq!(reloaded.list(false).len(), 2);
    }
}
