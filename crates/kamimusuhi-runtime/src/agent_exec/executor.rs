//! The executor interface and its command-line implementation.
//!
//! A [`TaskExecutor`] is an agent harness: given an objective it explores,
//! calls tools and several models, and reports a result. It is a separate
//! interface from the chat `LanguageProvider` on purpose — text-in/text-out
//! providers and agent harnesses have different latency, cost and safety
//! properties and are never interchangeable.
//!
//! Executors are blocking; the caller (the resident's orchestrator) owns
//! threads, the task board, status and cancellation. Long-lived helpers
//! (an attached `opencode serve`, ACP servers) are owned by the executor
//! and stopped when it is dropped.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::catalog::{self, CatalogEntry};
use super::config::{ExecutorSpec, Protocol, ServerSpec};
use super::output::OutputParser;
use super::policy;
use super::process::{self, ProcessEnd, ProcessSpec};
use super::types::{ExecutorHealth, RunEnd, TaskEvent, TaskOutcome, TaskRequest};

/// Longest answer kept from one run.
pub const SUMMARY_CHARS: usize = 16_000;
/// Tool names kept per run.
pub(crate) const TOOL_ACTIVITY_CAP: usize = 200;

/// State of a long-lived helper process, for status displays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SupervisorStatus {
    /// `server` (attached CLI server) or `acp`.
    pub kind: String,
    /// `running` | `stopped` | `starting` | `failed`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// ACP servers currently up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connections: Option<usize>,
}

pub trait TaskExecutor: Send + Sync {
    fn spec(&self) -> &ExecutorSpec;

    /// Cheap: no process is started.
    fn health(&self) -> ExecutorHealth;

    /// List the models the harness offers now. May take tens of seconds.
    fn discover(&self, now: u64) -> CatalogEntry;

    /// Run one task to completion, deadline or cancellation. With
    /// `request.resume_session` the harness continues that session.
    fn run(
        &self,
        request: &TaskRequest,
        cancel: &AtomicBool,
        on_event: &mut dyn FnMut(TaskEvent),
    ) -> TaskOutcome;

    /// Whether finished sessions can be continued.
    fn can_resume(&self) -> bool;

    /// Periodic upkeep: restart or idle-stop helper processes.
    fn maintain(&self) {}

    fn supervisor(&self) -> Option<SupervisorStatus> {
        None
    }
}

pub(crate) fn substitute(template: &str, pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .fold(template.to_owned(), |acc, (k, v)| acc.replace(k, v))
}

pub(crate) fn clip_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - max).collect();
    format!("…{tail}")
}

pub(crate) fn failed_outcome(error: String, duration_ms: u64) -> TaskOutcome {
    TaskOutcome {
        end: RunEnd::Failed,
        summary: String::new(),
        error: Some(error),
        session_id: None,
        usage: Default::default(),
        reported_cost_usd: None,
        tool_activity: Vec::new(),
        exit_code: None,
        duration_ms,
    }
}

/// Health and discovery are the same for every protocol: both go through
/// the harness binary.
pub(crate) fn health_of(spec: &ExecutorSpec) -> ExecutorHealth {
    let binary = process::resolve_binary(&spec.command);
    let (ok, detail) = match (&binary, spec.enabled) {
        (_, false) => (false, Some("disabled in configuration".to_owned())),
        (None, true) => (false, Some(format!("{} not found", spec.command))),
        (Some(_), true) => (true, None),
    };
    ExecutorHealth {
        executor: spec.name.clone(),
        adapter: spec.adapter.as_str().to_owned(),
        enabled: spec.enabled,
        binary: binary.map(|p| p.display().to_string()),
        ok,
        detail,
    }
}

fn version_of(spec: &ExecutorSpec) -> Option<String> {
    let args = vec!["--version".to_owned()];
    let result = process::run(
        &ProcessSpec {
            program: &spec.command,
            args: &args,
            cwd: None,
            env_passthrough: &spec.env_passthrough,
            extra_env: &[],
            timeout: Duration::from_secs(10),
        },
        &AtomicBool::new(false),
        |_| {},
    )
    .ok()?;
    result
        .success()
        .then(|| result.stdout.lines().next().unwrap_or("").trim().to_owned())
        .filter(|v| !v.is_empty())
}

pub(crate) fn discover_with(spec: &ExecutorSpec, now: u64) -> CatalogEntry {
    let mut entry = CatalogEntry {
        checked_at: now,
        ..CatalogEntry::default()
    };
    let Some(discover) = &spec.discover else {
        entry.error = Some("no model discovery configured".to_owned());
        return entry;
    };
    entry.harness_version = version_of(spec);
    let result = process::run(
        &ProcessSpec {
            program: &spec.command,
            args: &discover.args,
            cwd: None,
            env_passthrough: &spec.env_passthrough,
            extra_env: &[],
            timeout: Duration::from_secs(spec.discover_timeout_secs),
        },
        &AtomicBool::new(false),
        |_| {},
    );
    match result {
        Err(e) => entry.error = Some(e),
        Ok(r) if !r.success() => {
            entry.error = Some(format!(
                "discovery {:?} exit {:?}: {}",
                r.end,
                r.exit_code,
                clip_tail(r.stderr_tail.trim(), 300)
            ));
        }
        Ok(r) => match catalog::parse(discover.format, &r.stdout) {
            Ok(raw) => {
                entry.discovered_at = now;
                entry.models = raw
                    .into_iter()
                    .map(|m| catalog::describe(spec, m, now))
                    .collect();
            }
            Err(e) => entry.error = Some(e),
        },
    }
    entry
}

/// Arguments and environment that attach a run to a live server.
pub type Attach = (Vec<String>, Vec<(String, String)>);

struct ServerState {
    child: Option<std::process::Child>,
    password: Option<String>,
    failures: u32,
    next_try: Instant,
    detail: Option<String>,
}

/// Keeps a harness server (`opencode serve`) running for CLI runs to
/// attach to. When it cannot be started, runs fall back to a cold start.
pub struct ServerSupervisor {
    spec: ServerSpec,
    command: String,
    env_passthrough: Vec<String>,
    state: Mutex<ServerState>,
}

fn port_open(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

impl ServerSupervisor {
    fn new(spec: ServerSpec, command: String, env_passthrough: Vec<String>) -> Self {
        Self {
            spec,
            command,
            env_passthrough,
            state: Mutex::new(ServerState {
                child: None,
                password: None,
                failures: 0,
                next_try: Instant::now(),
                detail: None,
            }),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.spec.port)
    }

    /// Attach arguments and environment, starting the server if needed.
    /// `None` = not available right now.
    pub fn ensure(&self) -> Option<Attach> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let alive = state
            .child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !alive {
            if let Some(mut dead) = state.child.take() {
                process::kill_tree(&mut dead);
            }
            if Instant::now() < state.next_try {
                return None;
            }
            if port_open(self.spec.port) && self.reap_stale() {
                let until = Instant::now() + Duration::from_secs(5);
                while port_open(self.spec.port) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            if port_open(self.spec.port) {
                // Someone else owns the port; attaching to it would hand
                // tasks to a server this resident does not control.
                state.detail = Some(format!("port {} is already in use", self.spec.port));
                state.next_try = Instant::now() + Duration::from_secs(60);
                return None;
            }
            let password = self
                .spec
                .password_env
                .as_ref()
                .and_then(|_| process::random_hex());
            let extra: Vec<(String, String)> = self
                .spec
                .password_env
                .iter()
                .zip(&password)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let port = self.spec.port.to_string();
            let args: Vec<String> = self
                .spec
                .args
                .iter()
                .map(|a| substitute(a, &[("{port}", &port)]))
                .collect();
            let spawned =
                process::harness_command(&self.command, &args, &self.env_passthrough, &extra)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    self.backoff(&mut state, format!("could not start: {e}"));
                    return None;
                }
            };
            let deadline = Instant::now() + Duration::from_secs(20);
            while !port_open(self.spec.port) {
                if Instant::now() >= deadline || !matches!(child.try_wait(), Ok(None)) {
                    process::kill_tree(&mut child);
                    self.backoff(&mut state, "did not start listening".to_owned());
                    return None;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = std::fs::write(
                self.pidfile(),
                format!("{} {}", child.id(), std::process::id()),
            );
            state.child = Some(child);
            state.password = password;
            state.failures = 0;
            state.detail = None;
        }
        let url = self.url();
        let args = self
            .spec
            .attach_args
            .iter()
            .map(|a| substitute(a, &[("{url}", &url)]))
            .collect();
        let env = self
            .spec
            .password_env
            .iter()
            .zip(&state.password)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some((args, env))
    }

    /// Records the server this resident started, so a server orphaned by
    /// an unclean exit can be recognised and stopped on the next start.
    fn pidfile(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kamimusuhi-serve-{}.pid", self.spec.port))
    }

    /// Stop a server left behind by an earlier resident: only a process
    /// whose pid we recorded and whose command line is still ours.
    fn reap_stale(&self) -> bool {
        let text = std::fs::read_to_string(self.pidfile()).unwrap_or_default();
        let mut parts = text.split_whitespace().map(str::parse::<u32>);
        let Some(Ok(pid)) = parts.next() else {
            return false;
        };
        // A server whose owning resident is still alive is not stale, even
        // if that resident is another one on this machine. (Pidfiles without
        // an owner predate this check.)
        if let Some(Ok(owner)) = parts.next() {
            let owner_alive = std::process::Command::new("ps")
                .args(["-p", &owner.to_string(), "-o", "pid="])
                .output()
                .is_ok_and(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty());
            if owner_alive && owner != std::process::id() {
                return false;
            }
        }
        let command = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        // The exact argument line this supervisor launches.
        let port = self.spec.port.to_string();
        let launched = self
            .spec
            .args
            .iter()
            .map(|a| substitute(a, &[("{port}", &port)]))
            .collect::<Vec<_>>()
            .join(" ");
        if !command.ends_with(&launched) {
            return false;
        }
        let group = format!("-{pid}");
        let stopped = std::process::Command::new("kill")
            .args(["-TERM", "--", &group])
            .status()
            .is_ok_and(|s| s.success());
        let _ = std::fs::remove_file(self.pidfile());
        stopped
    }

    fn backoff(&self, state: &mut ServerState, detail: String) {
        state.failures = state.failures.saturating_add(1);
        let secs = (5u64 << state.failures.min(6)).min(300);
        state.next_try = Instant::now() + Duration::from_secs(secs);
        state.detail = Some(detail);
    }

    pub fn status(&self) -> SupervisorStatus {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let running = state
            .child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        SupervisorStatus {
            kind: "server".to_owned(),
            state: if running {
                "running"
            } else if state.failures > 0 {
                "failed"
            } else {
                "stopped"
            }
            .to_owned(),
            detail: state.detail.clone().or_else(|| running.then(|| self.url())),
            connections: None,
        }
    }
}

impl Drop for ServerSupervisor {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|p| p.into_inner());
        if let Some(mut child) = state.child.take() {
            process::kill_tree(&mut child);
            let _ = std::fs::remove_file(self.pidfile());
        }
    }
}

/// Any harness driven through its command line, per [`ExecutorSpec`].
pub struct CliExecutor {
    spec: ExecutorSpec,
    server: Option<ServerSupervisor>,
}

impl CliExecutor {
    pub fn new(spec: ExecutorSpec) -> Self {
        let server = spec
            .server
            .clone()
            .map(|s| ServerSupervisor::new(s, spec.command.clone(), spec.env_passthrough.clone()));
        Self { spec, server }
    }

    /// Full argument vector for a request (the prompt included).
    /// `attach` are the arguments that point the run at a live server.
    pub fn command_line(
        &self,
        request: &TaskRequest,
        attach: &[String],
    ) -> Result<Vec<String>, String> {
        let spec = &self.spec;
        let permission = spec
            .permission_args
            .for_mode(request.permissions)
            .ok_or_else(|| {
                format!(
                    "executor {} has no {} mode configured",
                    spec.name,
                    request.permissions.as_str()
                )
            })?;
        let prompt = policy::envelope(request);
        let workspace = request.workspace.to_string_lossy().into_owned();
        let mut args = spec.run.prefix.clone();
        args.extend(permission.iter().cloned());
        if let Some(model) = &request.model {
            args.extend(
                spec.run
                    .model_args
                    .iter()
                    .map(|a| substitute(a, &[("{model}", model)])),
            );
        }
        if let Some(session) = &request.resume_session {
            if spec.run.resume_args.is_empty() {
                return Err(format!(
                    "executor {} cannot continue a session from the command line",
                    spec.name
                ));
            }
            args.extend(
                spec.run
                    .resume_args
                    .iter()
                    .map(|a| substitute(a, &[("{session}", session)])),
            );
        }
        args.extend(spec.extra_args.iter().cloned());
        args.extend(attach.iter().cloned());
        args.extend(spec.run.args.iter().map(|a| {
            // The prompt is substituted last so text inside it that looks
            // like a placeholder is left alone.
            substitute(a, &[("{workspace}", &workspace)]).replace("{prompt}", &prompt)
        }));
        Ok(args)
    }
}

impl TaskExecutor for CliExecutor {
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
        !self.spec.run.resume_args.is_empty()
    }

    fn maintain(&self) {
        if let Some(server) = &self.server
            && self.spec.enabled
        {
            let _ = server.ensure();
        }
    }

    fn supervisor(&self) -> Option<SupervisorStatus> {
        self.server.as_ref().map(ServerSupervisor::status)
    }

    fn run(
        &self,
        request: &TaskRequest,
        cancel: &AtomicBool,
        on_event: &mut dyn FnMut(TaskEvent),
    ) -> TaskOutcome {
        let (attach, extra_env) = match &self.server {
            Some(server) => server.ensure().unwrap_or_else(|| {
                on_event(TaskEvent::Note {
                    text: "server unavailable; cold start".to_owned(),
                });
                (Vec::new(), Vec::new())
            }),
            None => (Vec::new(), Vec::new()),
        };
        let args = match self.command_line(request, &attach) {
            Ok(args) => args,
            Err(e) => return failed_outcome(e, 0),
        };
        let mut parser = OutputParser::new(self.spec.run.output);
        let result = process::run(
            &ProcessSpec {
                program: &self.spec.command,
                args: &args,
                cwd: Some(&request.workspace),
                env_passthrough: &self.spec.env_passthrough,
                extra_env: &extra_env,
                timeout: Duration::from_secs(self.spec.timeout_secs),
            },
            cancel,
            |line| {
                for event in parser.feed(line) {
                    on_event(event);
                }
            },
        );
        let result = match result {
            Ok(r) => r,
            Err(e) => return failed_outcome(e, 0),
        };
        let parsed = parser.finish(&result.stdout);
        let summary = clip_tail(parsed.texts.join("\n\n").trim(), SUMMARY_CHARS);
        let stderr_hint = || {
            let tail = clip_tail(result.stderr_tail.trim(), 500);
            (!tail.is_empty()).then_some(tail)
        };
        let (end, error) = match result.end {
            ProcessEnd::Cancelled => (RunEnd::Cancelled, Some("cancelled".to_owned())),
            ProcessEnd::TimedOut => (
                RunEnd::TimedOut,
                Some(format!("timed out after {}s", self.spec.timeout_secs)),
            ),
            ProcessEnd::Exited => {
                if parsed.error.is_some() || parsed.reported_success == Some(false) {
                    (RunEnd::Failed, parsed.error.clone().or_else(stderr_hint))
                } else if result.exit_code != Some(0) {
                    (
                        RunEnd::Failed,
                        Some(
                            stderr_hint()
                                .unwrap_or_else(|| format!("exit code {:?}", result.exit_code)),
                        ),
                    )
                } else if summary.is_empty() {
                    (
                        RunEnd::Failed,
                        Some("harness produced no answer".to_owned()),
                    )
                } else {
                    (RunEnd::Succeeded, None)
                }
            }
        };
        let mut tools = parsed.tools;
        tools.truncate(TOOL_ACTIVITY_CAP);
        TaskOutcome {
            end,
            summary,
            error,
            // A continued run may not repeat the id; it is the same session.
            session_id: parsed.session_id.or_else(|| request.resume_session.clone()),
            usage: parsed.usage,
            reported_cost_usd: parsed.cost_usd,
            tool_activity: tools,
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
        }
    }
}

/// Build the executor for a spec: command line or ACP.
pub fn build(spec: ExecutorSpec) -> Arc<dyn TaskExecutor> {
    match spec.protocol {
        Protocol::Cli => Arc::new(CliExecutor::new(spec)),
        Protocol::Acp => Arc::new(super::acp::AcpExecutor::new(spec)),
    }
}

/// The configured executors, in configuration order.
#[derive(Clone, Default)]
pub struct ExecutorRegistry {
    executors: Vec<Arc<dyn TaskExecutor>>,
}

impl ExecutorRegistry {
    pub fn from_specs(specs: Vec<ExecutorSpec>) -> Self {
        Self {
            executors: specs.into_iter().map(build).collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn TaskExecutor>> {
        self.executors
            .iter()
            .find(|e| e.spec().name == name)
            .cloned()
    }

    pub fn all(&self) -> &[Arc<dyn TaskExecutor>] {
        &self.executors
    }

    pub fn names(&self) -> Vec<&str> {
        self.executors
            .iter()
            .map(|e| e.spec().name.as_str())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_exec::config::{ExecutorConfig, ServerSpec};
    use crate::agent_exec::types::{TaskKind, TaskPermissions};

    fn executor(json: serde_json::Value) -> CliExecutor {
        CliExecutor::new(
            serde_json::from_value::<ExecutorConfig>(json)
                .expect("config")
                .resolve()
                .expect("resolves"),
        )
    }

    fn request(model: Option<&str>, permissions: TaskPermissions) -> TaskRequest {
        TaskRequest {
            task_id: "t1".to_owned(),
            kind: TaskKind::Review,
            objective: "look at {model}".to_owned(),
            success_criteria: Vec::new(),
            workspace: "/work".into(),
            context: Vec::new(),
            constraints: Vec::new(),
            permissions,
            model: model.map(str::to_owned),
            resume_session: None,
        }
    }

    #[test]
    fn command_lines_follow_the_presets() {
        let oc = executor(
            serde_json::json!({"name": "opencode", "adapter": "opencode",
            "extra_args": ["--attach", "http://127.0.0.1:4096"]}),
        );
        let args = oc
            .command_line(
                &request(Some("opencode/big-pickle"), TaskPermissions::ReadOnly),
                &[],
            )
            .expect("args");
        assert_eq!(
            args[..10],
            [
                "run",
                "--agent",
                "plan",
                "--model",
                "opencode/big-pickle",
                "--attach",
                "http://127.0.0.1:4096",
                "--format",
                "json",
                "--dir"
            ]
        );
        assert_eq!(args[10], "/work");
        // Placeholders inside the prompt are not substituted.
        assert!(args[11].contains("look at {model}"));

        let devin = executor(serde_json::json!({"name": "devin", "adapter": "devin_cli"}));
        let args = devin
            .command_line(&request(None, TaskPermissions::ReadOnly), &[])
            .expect("args");
        assert_eq!(args[..3], ["--permission-mode", "auto", "-p"]);
        assert!(
            !args.iter().any(|a| a == "--model"),
            "no model → harness default"
        );

        let cmd = executor(serde_json::json!({"name": "cc", "adapter": "command_code"}));
        let args = cmd
            .command_line(&request(Some("kimi-k2.5"), TaskPermissions::ReadOnly), &[])
            .expect("args");
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "plan"]));
        assert!(!args.iter().any(|a| a == "--yolo"));
    }

    #[test]
    fn a_mode_the_harness_lacks_is_refused() {
        let generic = executor(serde_json::json!({
            "name": "g", "adapter": "generic", "command": "g",
            "run": {"args": ["{prompt}"], "output": "text"},
            "permission_args": {"read_only": []}
        }));
        assert!(
            generic
                .command_line(&request(None, TaskPermissions::WorkspaceWrite), &[])
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_attached_server_is_started_reused_and_reaped() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("free port")
            .port();
        let spec = ServerSpec {
            args: ["-m", "http.server", "{port}", "--bind", "127.0.0.1"]
                .map(str::to_owned)
                .to_vec(),
            port,
            attach_args: vec!["--attach".to_owned(), "{url}".to_owned()],
            password_env: Some("SERVER_PASSWORD".to_owned()),
        };
        let first = ServerSupervisor::new(spec.clone(), "python3".to_owned(), Vec::new());
        let (args, env) = first.ensure().expect("started");
        assert_eq!(
            args,
            vec!["--attach".to_owned(), format!("http://127.0.0.1:{port}")]
        );
        assert_eq!(env[0].0, "SERVER_PASSWORD");
        assert_eq!(env[0].1.len(), 32, "per-start random password");
        assert_eq!(first.status().state, "running");
        assert_eq!(
            first.ensure().expect("reused").1,
            env,
            "same server, same password"
        );
        // Simulate an unclean resident exit: the server is left running.
        std::mem::forget(first);
        let second = ServerSupervisor::new(spec, "python3".to_owned(), Vec::new());
        assert!(
            second.ensure().is_some(),
            "the orphan is recognised and replaced"
        );
        drop(second);
        std::thread::sleep(Duration::from_millis(500));
        assert!(!port_open(port), "dropping the supervisor stops the server");
    }

    #[cfg(unix)]
    #[test]
    fn a_server_owned_by_another_live_resident_is_left_alone() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("free port")
            .port();
        let spec = ServerSpec {
            args: ["-m", "http.server", "{port}", "--bind", "127.0.0.1"]
                .map(str::to_owned)
                .to_vec(),
            port,
            attach_args: vec!["--attach".to_owned(), "{url}".to_owned()],
            password_env: None,
        };
        // Another resident (here: a sleeping process) owns a live server.
        let mut owner = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("owner");
        let mut server = std::process::Command::new("python3")
            .args([
                "-m",
                "http.server",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("server");
        let until = Instant::now() + Duration::from_secs(10);
        while !port_open(port) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        let supervisor = ServerSupervisor::new(spec, "python3".to_owned(), Vec::new());
        std::fs::write(
            supervisor.pidfile(),
            format!("{} {}", server.id(), owner.id()),
        )
        .expect("pidfile");
        assert!(
            supervisor.ensure().is_none(),
            "never attach to or kill a foreign server"
        );
        assert!(
            matches!(server.try_wait(), Ok(None)),
            "the other resident's server survives"
        );
        let _ = server.kill();
        let _ = server.wait();
        let _ = owner.kill();
        let _ = owner.wait();
        let _ = std::fs::remove_file(supervisor.pidfile());
    }

    #[cfg(unix)]
    #[test]
    fn a_generic_script_executor_runs_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("fake-agent");
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = \"--list\" ]; then printf 'm-1  first\\nm-2\\n'; exit 0; fi\n\
             echo \"cwd=$(pwd)\"; echo \"model=$2\"; echo \"secret=${CARGO_MANIFEST_DIR:-none}\"\n",
        )
        .expect("script");
        let mut perms = std::fs::metadata(&script).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script, perms).expect("chmod");
        let ex = executor(serde_json::json!({
            "name": "fake", "adapter": "generic", "command": script,
            "discover": {"args": ["--list"], "format": "lines"},
            "run": {"args": ["{prompt}"], "model_args": ["--model", "{model}"], "output": "text"},
            "permission_args": {"read_only": []},
            "default_billing": "local"
        }));
        assert!(ex.health().ok);
        let entry = ex.discover(42);
        assert_eq!(entry.error, None);
        assert_eq!(entry.models.len(), 2);
        assert_eq!(
            entry.models[0].billing,
            crate::agent_exec::AgentBilling::Local
        );

        let mut req = request(Some("m-1"), TaskPermissions::ReadOnly);
        req.workspace = dir.path().to_path_buf();
        let mut events = Vec::new();
        let outcome = ex.run(&req, &AtomicBool::new(false), &mut |e| events.push(e));
        assert_eq!(outcome.end, RunEnd::Succeeded, "{outcome:?}");
        assert!(outcome.summary.contains("model=m-1"));
        assert!(outcome.summary.contains("secret=none"));
        let canonical = dir.path().canonicalize().expect("canonical");
        assert!(outcome.summary.contains(&canonical.display().to_string()));
    }
}
