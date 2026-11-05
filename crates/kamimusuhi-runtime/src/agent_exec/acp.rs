//! Agent Client Protocol executor: a long-lived harness server (`devin
//! acp`, `opencode acp`) spoken to with JSON-RPC over stdio, one session
//! per task.
//!
//! Compared with a process per task this avoids a cold start, reports the
//! session id (so a task can be continued), streams structured tool
//! activity, and lets the resident answer the agent's permission requests
//! itself: in a read-only task every request is rejected, whatever mode the
//! harness claims to be in.
//!
//! Servers are pooled per model when the model is fixed at start
//! (`acp.model_args`), otherwise one server serves every model through a
//! session config option. Idle servers are stopped by `maintain`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::catalog::CatalogEntry;
use super::config::{AcpSpec, ExecutorSpec};
use super::executor::{
    SUMMARY_CHARS, SupervisorStatus, TOOL_ACTIVITY_CAP, TaskExecutor, clip_tail, discover_with,
    failed_outcome, health_of, substitute,
};
use super::policy;
use super::process;
use super::types::{
    ExecutorHealth, RunEnd, TaskEvent, TaskOutcome, TaskPermissions, TaskRequest, TaskUsage,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a cancelled prompt may take to acknowledge.
const CANCEL_GRACE: Duration = Duration::from_secs(15);

/// What the reader thread hands to a waiting task.
#[derive(Debug)]
pub enum Inbound {
    /// Response to one of our requests.
    Response(Result<Value, Value>),
    /// `session/update` notification.
    Update(Value),
    /// A request from the agent (`session/request_permission`, `fs/*` …).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

type Routes = Mutex<HashMap<String, Sender<Inbound>>>;

/// One running ACP server.
pub struct Connection {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Sender<Inbound>>>>,
    sessions: Arc<Routes>,
    alive: Arc<AtomicBool>,
    active: AtomicUsize,
    last_used: Mutex<Instant>,
}

/// Route one line from the server.
fn dispatch(
    line: &str,
    pending: &Mutex<HashMap<u64, Sender<Inbound>>>,
    sessions: &Routes,
    unrouted: &mut Vec<Value>,
) {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let method = message["method"].as_str();
    let session = message["params"]["sessionId"].as_str().unwrap_or("");
    let route = |s: &str| {
        sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(s)
            .cloned()
    };
    match (method, message.get("id")) {
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let waiter = pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            if let Some(waiter) = waiter {
                let outcome = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message["result"].clone()),
                };
                let _ = waiter.send(Inbound::Response(outcome));
            }
        }
        (Some(method), Some(id)) => match route(session) {
            Some(tx) => {
                let _ = tx.send(Inbound::Request {
                    id: id.clone(),
                    method: method.to_owned(),
                    params: message["params"].clone(),
                });
            }
            // Nobody to ask: answered as unsupported by the caller.
            None => unrouted.push(message),
        },
        (Some("session/update"), None) => {
            if let Some(tx) = route(session) {
                let _ = tx.send(Inbound::Update(message["params"]["update"].clone()));
            }
        }
        _ => {}
    }
}

impl Connection {
    /// Start the server and complete `initialize`.
    pub fn start(
        program: &str,
        args: &[String],
        env_passthrough: &[String],
    ) -> Result<Arc<Self>, String> {
        let mut child = process::harness_command(program, args, env_passthrough, &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin pipe")?;
        let stdout = child.stdout.take().ok_or("no stdout pipe")?;
        let pending: Arc<Mutex<HashMap<u64, Sender<Inbound>>>> = Arc::default();
        let sessions: Arc<Routes> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let connection = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending: Arc::clone(&pending),
            sessions: Arc::clone(&sessions),
            alive: Arc::clone(&alive),
            active: AtomicUsize::new(0),
            last_used: Mutex::new(Instant::now()),
        });
        let weak = Arc::downgrade(&connection);
        thread::Builder::new()
            .name("acp-reader".to_owned())
            .spawn(move || {
                let mut unrouted = Vec::new();
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    dispatch(&line, &pending, &sessions, &mut unrouted);
                    if let Some(conn) = weak.upgrade() {
                        for request in unrouted.drain(..) {
                            conn.reply_error(&request["id"], -32601, "no such session");
                        }
                    }
                }
                alive.store(false, Ordering::Relaxed);
                // Wake everyone waiting on this server.
                pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
                sessions.lock().unwrap_or_else(|p| p.into_inner()).clear();
            })
            .map_err(|e| e.to_string())?;
        connection.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                // No client filesystem or terminal: the agent uses its own
                // tools, under the session mode and our permission answers.
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false},
                                       "terminal": false},
                "clientInfo": {"name": "kamimusuhi", "version": env!("CARGO_PKG_VERSION")},
            }),
            Duration::from_secs(60),
        )?;
        Ok(connection)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn write(&self, message: &Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().unwrap_or_else(|p| p.into_inner());
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("acp write: {e}"))
    }

    /// Send a request whose response is delivered to `to`.
    pub fn send_request(
        &self,
        method: &str,
        params: Value,
        to: Sender<Inbound>,
    ) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, to);
        let sent =
            self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if sent.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
        }
        sent.map(|()| id)
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        let id = self.send_request(method, params, tx)?;
        match rx.recv_timeout(timeout) {
            Ok(Inbound::Response(Ok(v))) => Ok(v),
            Ok(Inbound::Response(Err(e))) => Err(format!(
                "{method}: {}",
                e["message"].as_str().unwrap_or(&e.to_string())
            )),
            Ok(_) => Err(format!("{method}: unexpected message")),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                Err(if self.is_alive() {
                    format!("{method}: no response")
                } else {
                    format!("{method}: server exited")
                })
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    pub fn reply(&self, id: &Value, result: Value) {
        let _ = self.write(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub fn reply_error(&self, id: &Value, code: i64, message: &str) {
        let _ = self.write(&json!({"jsonrpc": "2.0", "id": id,
                                   "error": {"code": code, "message": message}}));
    }

    fn route(&self, session: &str) -> Receiver<Inbound> {
        let (tx, rx) = mpsc::channel();
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session.to_owned(), tx);
        rx
    }

    fn unroute(&self, session: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session);
    }

    fn sender(&self, session: &str) -> Option<Sender<Inbound>> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session)
            .cloned()
    }

    pub fn shutdown(&self) {
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        process::kill_tree(&mut child);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(|p| p.into_inner());
        process::kill_tree(child);
    }
}

/// The answer to `session/request_permission`: reject everything in a
/// read-only task, allow once otherwise.
pub fn permission_answer(params: &Value, permissions: TaskPermissions) -> (Value, bool) {
    let wanted: &[&str] = if permissions == TaskPermissions::ReadOnly {
        &["reject_once", "reject_always"]
    } else {
        &["allow_once", "allow_always"]
    };
    let option = params["options"].as_array().and_then(|options| {
        wanted.iter().find_map(|kind| {
            options
                .iter()
                .find(|o| o["kind"].as_str() == Some(kind))
                .and_then(|o| o["optionId"].as_str())
        })
    });
    let allowed = permissions != TaskPermissions::ReadOnly && option.is_some();
    let outcome = match option {
        Some(id) => json!({"outcome": {"outcome": "selected", "optionId": id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    };
    (outcome, allowed)
}

/// Collects a prompt turn's streamed updates.
#[derive(Default)]
struct Turn {
    segments: Vec<String>,
    current: String,
    tools: Vec<String>,
    titles: HashMap<String, String>,
    usage: TaskUsage,
    cost_usd: Option<f64>,
}

impl Turn {
    fn close_segment(&mut self) {
        let text = std::mem::take(&mut self.current);
        if !text.trim().is_empty() {
            self.segments.push(text.trim().to_owned());
        }
    }

    fn update(&mut self, update: &Value) -> Vec<TaskEvent> {
        let mut events = Vec::new();
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                if let Some(text) = update["content"]["text"].as_str() {
                    self.current.push_str(text);
                }
            }
            "tool_call" => {
                self.close_segment();
                let id = update["toolCallId"].as_str().unwrap_or("").to_owned();
                let name = update["title"]
                    .as_str()
                    .or_else(|| update["kind"].as_str())
                    .unwrap_or("tool")
                    .chars()
                    .take(120)
                    .collect::<String>();
                self.titles.insert(id, name.clone());
                if matches!(update["status"].as_str(), Some("completed" | "failed")) {
                    self.tools.push(name.clone());
                    events.push(TaskEvent::Tool {
                        name,
                        finished: true,
                    });
                } else {
                    events.push(TaskEvent::Tool {
                        name,
                        finished: false,
                    });
                }
            }
            "tool_call_update" => {
                if matches!(update["status"].as_str(), Some("completed" | "failed")) {
                    let id = update["toolCallId"].as_str().unwrap_or("");
                    let name = update["title"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| self.titles.get(id).cloned())
                        .unwrap_or_else(|| "tool".to_owned());
                    self.tools.push(name.clone());
                    events.push(TaskEvent::Tool {
                        name,
                        finished: true,
                    });
                }
            }
            "usage_update" => {
                // Context-window usage, not billing; recorded when a cost is
                // attached.
                if let Some(cost) = update["cost"]["amount"].as_f64() {
                    self.cost_usd = Some(cost);
                }
            }
            "plan" => events.push(TaskEvent::Note {
                text: format!(
                    "plan: {} steps",
                    update["entries"].as_array().map_or(0, Vec::len)
                ),
            }),
            _ => {}
        }
        events
    }

    fn usage_from_result(&mut self, result: &Value) {
        let usage = &result["usage"];
        if usage.is_object() {
            self.usage = TaskUsage {
                input_tokens: usage["inputTokens"].as_u64(),
                output_tokens: usage["outputTokens"].as_u64(),
                cache_read_tokens: usage["cachedReadTokens"].as_u64(),
                cache_write_tokens: usage["cachedWriteTokens"].as_u64(),
            };
        }
    }
}

pub struct AcpExecutor {
    spec: ExecutorSpec,
    acp: AcpSpec,
    pool: Mutex<HashMap<String, Arc<Connection>>>,
    last_error: Mutex<Option<String>>,
}

impl AcpExecutor {
    pub fn new(spec: ExecutorSpec) -> Self {
        let acp = spec.acp.clone().unwrap_or_else(|| AcpSpec {
            args: vec!["acp".to_owned()],
            model_args: Vec::new(),
            model_option: None,
            mode_option: "mode".to_owned(),
            modes: super::config::PermissionModes::default(),
            idle_secs: 900,
        });
        Self {
            spec,
            acp,
            pool: Mutex::new(HashMap::new()),
            last_error: Mutex::new(None),
        }
    }

    /// Servers fixed to a model are keyed by it; a shared server by "".
    fn key(&self, model: Option<&str>) -> String {
        if self.acp.model_args.is_empty() {
            String::new()
        } else {
            model.unwrap_or("").to_owned()
        }
    }

    fn connection(&self, model: Option<&str>) -> Result<Arc<Connection>, String> {
        let key = self.key(model);
        let mut pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(conn) = pool.get(&key).filter(|c| c.is_alive()) {
            return Ok(Arc::clone(conn));
        }
        let mut args = self.acp.args.clone();
        if let Some(model) = model.filter(|_| !self.acp.model_args.is_empty()) {
            args.extend(
                self.acp
                    .model_args
                    .iter()
                    .map(|a| substitute(a, &[("{model}", model)])),
            );
        }
        args.extend(self.spec.extra_args.iter().cloned());
        let started = Connection::start(&self.spec.command, &args, &self.spec.env_passthrough);
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) =
            started.as_ref().err().cloned();
        let conn = started?;
        pool.insert(key, Arc::clone(&conn));
        Ok(conn)
    }

    fn set_option(
        conn: &Connection,
        session: &str,
        option: &str,
        value: &str,
    ) -> Result<(), String> {
        conn.request(
            "session/set_config_option",
            json!({"sessionId": session, "configId": option, "value": value}),
            REQUEST_TIMEOUT,
        )
        .map(|_| ())
    }

    /// Open (or reload) the session and put it in the task's mode.
    fn open_session(
        &self,
        conn: &Connection,
        request: &TaskRequest,
        mode: &str,
    ) -> Result<(String, Receiver<Inbound>), String> {
        let cwd = request.workspace.to_string_lossy().into_owned();
        let (session, rx) = match &request.resume_session {
            Some(id) => {
                // Route first: the reload replays history as updates,
                // which are drained and ignored below.
                let rx = conn.route(id);
                if let Err(e) = conn.request(
                    "session/load",
                    json!({"sessionId": id, "cwd": cwd, "mcpServers": []}),
                    REQUEST_TIMEOUT,
                ) {
                    conn.unroute(id);
                    return Err(e);
                }
                while rx.try_recv().is_ok() {}
                (id.clone(), rx)
            }
            None => {
                let created = conn.request(
                    "session/new",
                    json!({"cwd": cwd, "mcpServers": []}),
                    REQUEST_TIMEOUT,
                )?;
                let id = created["sessionId"]
                    .as_str()
                    .ok_or("session/new returned no sessionId")?
                    .to_owned();
                let rx = conn.route(&id);
                (id, rx)
            }
        };
        let configured = (|| {
            if let (Some(option), Some(model)) = (&self.acp.model_option, &request.model) {
                Self::set_option(conn, &session, option, model)?;
            }
            // The mode is what keeps a read-only task read-only on the
            // harness side; failing to set it fails the task.
            Self::set_option(conn, &session, &self.acp.mode_option, mode)
        })();
        match configured {
            Ok(()) => Ok((session, rx)),
            Err(e) => {
                conn.unroute(&session);
                Err(e)
            }
        }
    }
}

impl TaskExecutor for AcpExecutor {
    fn spec(&self) -> &ExecutorSpec {
        &self.spec
    }

    fn health(&self) -> ExecutorHealth {
        health_of(&self.spec)
    }

    fn discover(&self, now: u64) -> CatalogEntry {
        discover_with(&self.spec, now)
    }

    fn can_resume(&self) -> bool {
        true
    }

    fn maintain(&self) {
        let idle = Duration::from_secs(self.acp.idle_secs);
        self.pool
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, conn| {
                let unused = conn.active.load(Ordering::Relaxed) == 0
                    && conn
                        .last_used
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .elapsed()
                        >= idle;
                if unused {
                    conn.shutdown();
                }
                conn.is_alive() && !unused
            });
    }

    fn supervisor(&self) -> Option<SupervisorStatus> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        let up = pool.values().filter(|c| c.is_alive()).count();
        let error = self
            .last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        Some(SupervisorStatus {
            kind: "acp".to_owned(),
            state: if up > 0 {
                "running"
            } else if error.is_some() {
                "failed"
            } else {
                "stopped"
            }
            .to_owned(),
            detail: error,
            connections: Some(up),
        })
    }

    fn run(
        &self,
        request: &TaskRequest,
        cancel: &AtomicBool,
        on_event: &mut dyn FnMut(TaskEvent),
    ) -> TaskOutcome {
        let started = Instant::now();
        let elapsed = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let Some(mode) = self.acp.modes.for_mode(request.permissions) else {
            return failed_outcome(
                format!(
                    "executor {} has no {} mode configured",
                    self.spec.name,
                    request.permissions.as_str()
                ),
                0,
            );
        };
        let conn = match self.connection(request.model.as_deref()) {
            Ok(c) => c,
            Err(e) => return failed_outcome(e, elapsed()),
        };
        conn.active.fetch_add(1, Ordering::Relaxed);
        let outcome = self.run_session(&conn, request, mode, cancel, on_event, started);
        conn.active.fetch_sub(1, Ordering::Relaxed);
        *conn.last_used.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        outcome
    }
}

impl AcpExecutor {
    fn run_session(
        &self,
        conn: &Connection,
        request: &TaskRequest,
        mode: &str,
        cancel: &AtomicBool,
        on_event: &mut dyn FnMut(TaskEvent),
        started: Instant,
    ) -> TaskOutcome {
        let elapsed = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (session, rx) = match self.open_session(conn, request, mode) {
            Ok(s) => s,
            Err(e) => return failed_outcome(e, elapsed()),
        };
        on_event(TaskEvent::Session {
            id: session.clone(),
        });
        let Some(to_self) = conn.sender(&session) else {
            return failed_outcome("session route vanished".to_owned(), elapsed());
        };
        let prompt = policy::envelope(request);
        if let Err(e) = conn.send_request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": prompt}]}),
            to_self,
        ) {
            conn.unroute(&session);
            return failed_outcome(e, elapsed());
        }
        let deadline = started + Duration::from_secs(self.spec.timeout_secs);
        let mut turn = Turn::default();
        let mut stop_sent: Option<Instant> = None;
        let mut timed_out = false;
        let mut denied = 0usize;
        let result: Result<Value, String> = loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Inbound::Update(update)) => {
                    for event in turn.update(&update) {
                        on_event(event);
                    }
                }
                Ok(Inbound::Request { id, method, params }) => {
                    if method == "session/request_permission" {
                        let (answer, allowed) = permission_answer(&params, request.permissions);
                        if !allowed {
                            denied += 1;
                            on_event(TaskEvent::Note {
                                text: format!(
                                    "permission denied: {}",
                                    params["toolCall"]["title"].as_str().unwrap_or("tool")
                                ),
                            });
                        }
                        conn.reply(&id, answer);
                    } else {
                        conn.reply_error(&id, -32601, "not supported by this client");
                    }
                }
                Ok(Inbound::Response(Ok(result))) => break Ok(result),
                Ok(Inbound::Response(Err(error))) => {
                    break Err(error["message"]
                        .as_str()
                        .map_or_else(|| error.to_string(), str::to_owned));
                }
                Err(RecvTimeoutError::Disconnected) => break Err("ACP server exited".to_owned()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if stop_sent.is_none() && (cancel.load(Ordering::Relaxed) || now >= deadline) {
                timed_out = now >= deadline;
                let _ = conn.notify("session/cancel", json!({"sessionId": session}));
                stop_sent = Some(now);
            }
            if stop_sent.is_some_and(|t| t.elapsed() >= CANCEL_GRACE) {
                break Err("did not stop after session/cancel".to_owned());
            }
            if !conn.is_alive() {
                break Err("ACP server exited".to_owned());
            }
        };
        conn.unroute(&session);
        turn.close_segment();
        if let Ok(result) = &result {
            turn.usage_from_result(result);
        }
        let summary = clip_tail(turn.segments.join("\n\n").trim(), SUMMARY_CHARS);
        let stop_reason = result
            .as_ref()
            .ok()
            .and_then(|r| r["stopReason"].as_str().map(str::to_owned));
        let (end, error) = match (&result, stop_reason.as_deref()) {
            _ if timed_out => (
                RunEnd::TimedOut,
                Some(format!("timed out after {}s", self.spec.timeout_secs)),
            ),
            _ if stop_sent.is_some() => (RunEnd::Cancelled, Some("cancelled".to_owned())),
            (Err(e), _) => (RunEnd::Failed, Some(e.clone())),
            (Ok(_), Some("end_turn")) if !summary.is_empty() => (RunEnd::Succeeded, None),
            (Ok(_), Some("end_turn")) => (
                RunEnd::Failed,
                Some("harness produced no answer".to_owned()),
            ),
            (Ok(_), reason) => (
                RunEnd::Failed,
                Some(format!("stopped: {}", reason.unwrap_or("unknown"))),
            ),
        };
        let error = match (error, denied) {
            (Some(e), n) if n > 0 => Some(format!("{e} ({n} permission requests denied)")),
            (e, _) => e,
        };
        let mut tools = turn.tools;
        tools.truncate(TOOL_ACTIVITY_CAP);
        TaskOutcome {
            end,
            summary,
            error,
            session_id: Some(session),
            usage: turn.usage,
            reported_cost_usd: turn.cost_usd,
            tool_activity: tools,
            exit_code: None,
            duration_ms: elapsed(),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent_exec::config::ExecutorConfig;
    use crate::agent_exec::types::TaskKind;

    /// A scripted ACP server in Python: answers initialize, session/new,
    /// set_config_option and prompt; streams a tool call and text; asks for
    /// one permission and reports what it was told.
    const FAKE: &str = r#"
import json, sys
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
mode = None
for line in sys.stdin:
    m = json.loads(line)
    meth, mid = m.get("method"), m.get("id")
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}})
    elif meth in ("session/new", "session/load"):
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": m["params"].get("sessionId", "s-1")}})
    elif meth == "session/set_config_option":
        if m["params"]["configId"] == "mode": mode = m["params"]["value"]
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif meth == "session/prompt":
        sid = m["params"]["sessionId"]
        text = m["params"]["prompt"][0]["text"]
        if "HANG" in text:
            prompt_id = mid
            continue
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "read a.txt", "status": "pending"}}})
        send({"jsonrpc": "2.0", "id": 900, "method": "session/request_permission", "params": {"sessionId": sid, "toolCall": {"title": "edit a.txt"}, "options": [{"optionId": "yes", "kind": "allow_once"}, {"optionId": "no", "kind": "reject_once"}]}})
        answer = json.loads(sys.stdin.readline())
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}}})
        chosen = answer["result"]["outcome"].get("optionId")
        for chunk in ["mode=", str(mode), " permission=", str(chosen), " resumed=", str(sid)]:
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": chunk}}}})
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
    elif meth == "session/cancel":
        send({"jsonrpc": "2.0", "id": prompt_id, "result": {"stopReason": "cancelled"}})
"#;

    fn executor(dir: &std::path::Path) -> AcpExecutor {
        let script = dir.join("fake_acp.py");
        std::fs::write(&script, FAKE).expect("script");
        let config: ExecutorConfig = serde_json::from_value(json!({
            "name": "fake", "adapter": "generic", "command": "python3", "protocol": "acp",
            "run": {"args": ["{prompt}"], "output": "text"},
            "permission_args": {"read_only": []},
            "acp": {"args": [script], "model_option": "model",
                    "modes": {"read_only": "ask", "workspace_write": "code"}, "idle_secs": 1}
        }))
        .expect("config");
        AcpExecutor::new(config.resolve().expect("resolves"))
    }

    fn request(dir: &std::path::Path, objective: &str, resume: Option<&str>) -> TaskRequest {
        TaskRequest {
            task_id: "t1".to_owned(),
            kind: TaskKind::Review,
            objective: objective.to_owned(),
            success_criteria: Vec::new(),
            workspace: dir.to_path_buf(),
            context: Vec::new(),
            constraints: Vec::new(),
            permissions: TaskPermissions::ReadOnly,
            model: Some("m".to_owned()),
            resume_session: resume.map(str::to_owned),
        }
    }

    #[test]
    fn a_read_only_session_runs_in_ask_mode_and_refuses_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ex = executor(dir.path());
        let mut events = Vec::new();
        let out = ex.run(
            &request(dir.path(), "look", None),
            &AtomicBool::new(false),
            &mut |e| events.push(e),
        );
        assert_eq!(out.end, RunEnd::Succeeded, "{out:?}");
        assert_eq!(out.summary, "mode=ask permission=no resumed=s-1");
        assert_eq!(out.session_id.as_deref(), Some("s-1"));
        assert_eq!(out.tool_activity, vec!["read a.txt"]);
        assert!(
            events.iter().any(
                |e| matches!(e, TaskEvent::Note { text } if text.contains("permission denied"))
            )
        );
        // The server stays up for the next task, and continuing reuses the
        // session id.
        assert_eq!(ex.supervisor().expect("status").connections, Some(1));
        let out = ex.run(
            &request(dir.path(), "more", Some("s-1")),
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(out.summary.ends_with("resumed=s-1"), "{out:?}");
        // Idle servers are stopped.
        std::thread::sleep(Duration::from_millis(1100));
        ex.maintain();
        assert_eq!(ex.supervisor().expect("status").connections, Some(0));
    }

    #[test]
    fn cancelling_sends_session_cancel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ex = executor(dir.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::Relaxed);
        });
        let out = ex.run(&request(dir.path(), "HANG", None), &cancel, &mut |_| {});
        stopper.join().expect("join");
        assert_eq!(out.end, RunEnd::Cancelled, "{out:?}");
    }

    #[test]
    fn permission_answers() {
        let params = json!({"options": [{"optionId": "a", "kind": "allow_always"},
                                        {"optionId": "r", "kind": "reject_always"}]});
        let (answer, allowed) = permission_answer(&params, TaskPermissions::ReadOnly);
        assert_eq!(answer["outcome"]["optionId"], "r");
        assert!(!allowed);
        let (answer, allowed) = permission_answer(&params, TaskPermissions::WorkspaceWrite);
        assert_eq!(answer["outcome"]["optionId"], "a");
        assert!(allowed);
        let (answer, _) = permission_answer(&json!({}), TaskPermissions::ReadOnly);
        assert_eq!(answer["outcome"]["outcome"], "cancelled");
    }
}
