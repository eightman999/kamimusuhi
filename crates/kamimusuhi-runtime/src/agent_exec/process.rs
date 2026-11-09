//! Running a harness process: minimal environment, streamed stdout,
//! deadline and cancellation.
//!
//! The child gets a cleared environment plus a short allowlist and the
//! executor's own `env_passthrough` names, so credentials the resident
//! holds (node token, provider keys) never reach an external agent. The
//! harness authenticates with its own stored login.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Variables every harness needs to find its binary, home directory
/// (where its login lives) and locale.
pub const BASE_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// Most stdout delivered to callbacks and kept in memory per run, including
/// newlines. A complete protocol line is either delivered or omitted.
pub const STDOUT_CAP: usize = 4 * 1024 * 1024;
/// A single JSON event may be large, but a missing newline must not grow the
/// resident indefinitely. Oversized lines are discarded, never split.
pub const STDOUT_LINE_CAP: usize = 1024 * 1024;
/// Most stderr kept (the tail).
pub const STDERR_CAP: usize = 16 * 1024;
const PIPE_CHUNK: usize = 64 * 1024;
const PIPE_QUEUE: usize = 16;
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEnd {
    Exited,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub end: ProcessEnd,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr_tail: String,
    /// A line or the aggregate stdout exceeded its cap, a read failed, or
    /// termination left output unread. Callbacks have the same stdout cap.
    pub stdout_truncated: bool,
    /// The stderr tail omitted earlier bytes or could not be fully drained.
    pub stderr_truncated: bool,
    pub duration_ms: u64,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        self.end == ProcessEnd::Exited && self.exit_code == Some(0)
    }
}

pub struct ProcessSpec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env_passthrough: &'a [String],
    /// Values set by the resident itself (e.g. a server password).
    pub extra_env: &'a [(String, String)],
    pub timeout: Duration,
}

/// Find `program` the way a shell would: as given when it contains a
/// path separator, otherwise on `PATH`.
pub fn resolve_binary(program: &str) -> Option<PathBuf> {
    let is_file = |p: &Path| p.is_file();
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_file(&path).then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|p| is_file(p))
    })
}

/// A harness command with the minimal environment, in its own process
/// group so stopping it reaches its children. Pipes are the caller's.
pub fn harness_command(
    program: &str,
    args: &[String],
    env_passthrough: &[String],
    extra_env: &[(String, String)],
) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear();
    for name in BASE_ENV
        .iter()
        .copied()
        .chain(env_passthrough.iter().map(String::as_str))
    {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    cmd.env("NO_COLOR", "1").env("TERM", "dumb");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

fn command(spec: &ProcessSpec<'_>) -> Command {
    let mut cmd = harness_command(
        spec.program,
        spec.args,
        spec.env_passthrough,
        spec.extra_env,
    );
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = spec.cwd {
        cmd.current_dir(cwd);
    }
    cmd
}

/// 128 random bits as hex, for per-start secrets.
pub fn random_hex() -> Option<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Signal the child's whole process group (no `libc`: `unsafe` is
/// forbidden in this workspace, so this goes through `kill(1)`).
pub fn kill_group(child: &std::process::Child, signal: &str) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .args([signal, "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = (child, signal);
}

pub fn kill_tree(child: &mut std::process::Child) {
    kill_group(child, "-TERM");
    thread::sleep(Duration::from_millis(300));
    kill_group(child, "-KILL");
    let _ = child.kill();
    let _ = child.wait();
}

enum PipeEvent {
    Data { stdout: bool, bytes: Vec<u8> },
    Closed { stdout: bool, failed: bool },
}

fn read_pipe(mut pipe: impl Read, stdout: bool, tx: mpsc::SyncSender<PipeEvent>) {
    let mut buffer = vec![0u8; PIPE_CHUNK];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => {
                let _ = tx.send(PipeEvent::Closed {
                    stdout,
                    failed: false,
                });
                return;
            }
            Ok(n) => {
                if tx
                    .send(PipeEvent::Data {
                        stdout,
                        bytes: buffer[..n].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                let _ = tx.send(PipeEvent::Closed {
                    stdout,
                    failed: true,
                });
                return;
            }
        }
    }
}

fn stop_reason(cancel: &AtomicBool, deadline: Instant) -> Option<ProcessEnd> {
    if cancel.load(Ordering::Relaxed) {
        Some(ProcessEnd::Cancelled)
    } else if Instant::now() >= deadline {
        Some(ProcessEnd::TimedOut)
    } else {
        None
    }
}

#[derive(Default)]
struct StdoutCapture {
    text: String,
    pending: Vec<u8>,
    discarding_line: bool,
    saturated: bool,
    truncated: bool,
}

impl StdoutCapture {
    fn line(&mut self, on_line: &mut impl FnMut(&str)) {
        if self.discarding_line || self.saturated {
            self.pending.clear();
            self.discarding_line = false;
            return;
        }
        let line = String::from_utf8_lossy(&self.pending);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        // Lossy UTF-8 conversion can expand bytes, so apply the limits to
        // the delivered text as well as the raw pending buffer.
        if line.len() > STDOUT_LINE_CAP {
            self.truncated = true;
        } else if line.len() + 1 > STDOUT_CAP - self.text.len() {
            self.truncated = true;
            self.saturated = true;
        } else {
            on_line(line);
            self.text.push_str(line);
            self.text.push('\n');
        }
        self.pending.clear();
    }

    fn push(
        &mut self,
        bytes: &[u8],
        cancel: &AtomicBool,
        deadline: Instant,
        on_line: &mut impl FnMut(&str),
    ) -> Option<ProcessEnd> {
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            // A chunk may hold thousands of events; checking only between
            // reads lets busy stdout bypass cancellation and deadlines.
            if let Some(end) = stop_reason(cancel, deadline) {
                self.truncated = true;
                return Some(end);
            }
            let newline = part.last() == Some(&b'\n');
            let content = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            if self.saturated {
                self.truncated = true;
                continue;
            }
            if !self.discarding_line {
                if content.len() > STDOUT_LINE_CAP - self.pending.len() {
                    self.pending.clear();
                    self.discarding_line = true;
                    self.truncated = true;
                } else {
                    self.pending.extend_from_slice(content);
                }
            }
            if newline {
                self.line(on_line);
            }
        }
        None
    }
}

#[derive(Default)]
struct StderrTail {
    bytes: Vec<u8>,
    truncated: bool,
}

impl StderrTail {
    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= STDERR_CAP {
            self.truncated |= !self.bytes.is_empty() || bytes.len() > STDERR_CAP;
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - STDERR_CAP..]);
        } else {
            let discard = (self.bytes.len() + bytes.len()).saturating_sub(STDERR_CAP);
            self.truncated |= discard > 0;
            self.bytes.drain(..discard);
            self.bytes.extend_from_slice(bytes);
        }
    }

    fn text(&mut self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if text.len() > STDERR_CAP {
            let mut start = text.len() - STDERR_CAP;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            text.drain(..start);
            self.truncated = true;
        }
        text
    }
}

/// Run to completion, deadline or cancellation. `on_line` sees each stdout
/// line as it arrives, on the calling thread, within the documented caps.
/// The callback must return promptly: an in-progress callback cannot be
/// interrupted. Shutdown allows the process group's 300 ms TERM grace and
/// at most 100 ms of pipe draining; readers are never joined indefinitely.
pub fn run(
    spec: &ProcessSpec<'_>,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<ProcessResult, String> {
    let started = Instant::now();
    let deadline = started
        .checked_add(spec.timeout)
        .ok_or("process timeout is out of range")?;
    let mut child = command(spec)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", spec.program))?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let stderr = child.stderr.take().ok_or("no stderr pipe")?;
    // Read ahead enough for ordinary CLI bursts without an unbounded queue
    // when callbacks are slow. Both pipes together queue at most 1 MiB.
    let (tx, rx) = mpsc::sync_channel(PIPE_QUEUE);
    let err_tx = tx.clone();
    thread::spawn(move || read_pipe(stdout, true, tx));
    thread::spawn(move || read_pipe(stderr, false, err_tx));
    let mut captured = StdoutCapture::default();
    let mut stderr_tail = StderrTail::default();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let (end, status) = loop {
        if let Some(end) = stop_reason(cancel, deadline) {
            kill_tree(&mut child);
            break (end, None);
        }
        // Let stdout-writing helpers finish (some CLIs use them for their
        // final events), but a child holding only stderr must not delay a
        // completed command until its full task deadline.
        if !stdout_open && let Ok(Some(status)) = child.try_wait() {
            break (ProcessEnd::Exited, Some(status));
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        if !stdout_open && !stderr_open {
            thread::sleep(wait);
            continue;
        }
        match rx.recv_timeout(wait) {
            Ok(PipeEvent::Data {
                stdout: true,
                bytes,
            }) => {
                if let Some(end) = captured.push(&bytes, cancel, deadline, &mut on_line) {
                    kill_tree(&mut child);
                    break (end, None);
                }
            }
            Ok(PipeEvent::Data {
                stdout: false,
                bytes,
            }) => stderr_tail.push(&bytes),
            Ok(PipeEvent::Closed { stdout, failed }) => {
                if stdout {
                    stdout_open = false;
                    captured.truncated |= failed;
                    if !failed && stop_reason(cancel, deadline).is_none() {
                        if !captured.pending.is_empty() {
                            captured.line(&mut on_line);
                        }
                    } else if !captured.pending.is_empty() {
                        captured.truncated = true;
                    }
                } else {
                    stderr_open = false;
                    stderr_tail.truncated |= failed;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                captured.truncated |= stdout_open;
                stderr_tail.truncated |= stderr_open;
                stdout_open = false;
                stderr_open = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    if end == ProcessEnd::Exited {
        // Whatever the harness left running in its group goes with it.
        kill_group(&child, "-KILL");
    } else {
        captured.truncated |= stdout_open;
    }
    // Preserve already-written diagnostics without calling back after
    // cancellation/deadline. A descendant retaining a pipe cannot hold
    // the caller here forever, even if it escaped the process group.
    let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
    while (stdout_open || stderr_open) && Instant::now() < drain_deadline {
        match rx.recv_timeout(drain_deadline.saturating_duration_since(Instant::now())) {
            Ok(PipeEvent::Data {
                stdout: false,
                bytes,
            }) => stderr_tail.push(&bytes),
            Ok(PipeEvent::Closed { stdout, failed }) => {
                if stdout {
                    stdout_open = false;
                    captured.truncated |= failed;
                } else {
                    stderr_open = false;
                    stderr_tail.truncated |= failed;
                }
            }
            Ok(PipeEvent::Data { .. }) => captured.truncated = true,
            Err(_) => break,
        }
    }
    stderr_tail.truncated |= stderr_open;
    let stderr_text = stderr_tail.text();
    Ok(ProcessResult {
        end,
        exit_code: status.and_then(|s| s.code()),
        stdout: captured.text,
        stderr_tail: stderr_text,
        stdout_truncated: captured.truncated,
        stderr_truncated: stderr_tail.truncated,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64, cancel: &AtomicBool) -> (ProcessResult, Vec<String>) {
        let mut lines = Vec::new();
        let result = sh_with(script, timeout_ms, cancel, |l| lines.push(l.to_owned()));
        (result, lines)
    }

    fn sh_with(
        script: &str,
        timeout_ms: u64,
        cancel: &AtomicBool,
        on_line: impl FnMut(&str),
    ) -> ProcessResult {
        let args = vec!["-c".to_owned(), script.to_owned()];
        run(
            &ProcessSpec {
                program: "/bin/sh",
                args: &args,
                cwd: None,
                env_passthrough: &[],
                extra_env: &[],
                timeout: Duration::from_millis(timeout_ms),
            },
            cancel,
            on_line,
        )
        .expect("runs")
    }

    #[test]
    fn streams_lines_and_reports_exit() {
        let (result, lines) = sh(
            "echo one; echo two; echo err >&2; exit 3",
            5_000,
            &AtomicBool::new(false),
        );
        assert_eq!(lines, vec!["one", "two"]);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.end, ProcessEnd::Exited);
        assert!(result.stderr_tail.contains("err"));
        assert!(!result.stdout_truncated);
        assert!(!result.stderr_truncated);
    }

    #[test]
    fn large_output_and_a_last_line_without_newline_survive() {
        let (result, lines) = sh(
            "i=0; while [ $i -lt 20000 ]; do echo \"line $i\"; i=$((i+1)); done; printf 'tail'",
            20_000,
            &AtomicBool::new(false),
        );
        assert_eq!(lines.len(), 20_001);
        assert_eq!(lines[19_999], "line 19999");
        assert_eq!(lines[20_000], "tail");
        assert!(result.success());
    }

    #[test]
    fn deadline_kills_the_process_group() {
        let started = Instant::now();
        let (result, _) = sh("sleep 30 & sleep 30; wait", 300, &AtomicBool::new(false));
        assert_eq!(result.end, ProcessEnd::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cancellation_stops_the_run() {
        let cancel = AtomicBool::new(true);
        let (result, _) = sh("sleep 30", 60_000, &cancel);
        assert_eq!(result.end, ProcessEnd::Cancelled);
    }

    #[test]
    fn busy_stdout_checks_deadlines_between_callback_lines() {
        let started = Instant::now();
        let mut lines = 0;
        let result = sh_with(
            "i=0; while [ $i -lt 1000 ]; do echo event; i=$((i+1)); done; sleep 30",
            100,
            &AtomicBool::new(false),
            |_| {
                lines += 1;
                thread::sleep(Duration::from_millis(2));
            },
        );
        assert_eq!(result.end, ProcessEnd::TimedOut);
        assert!(lines < 1000, "queued output must not postpone the deadline");
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn busy_stdout_observes_cancellation_before_the_next_event() {
        let cancel = AtomicBool::new(false);
        let mut lines = 0;
        let result = sh_with(
            "i=0; while [ $i -lt 5000 ]; do echo event; i=$((i+1)); done; sleep 30",
            30_000,
            &cancel,
            |_| {
                lines += 1;
                if lines == 10 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result.end, ProcessEnd::Cancelled);
        assert_eq!(lines, 10);
    }

    #[test]
    fn an_oversized_line_is_discarded_and_the_next_event_survives() {
        let block = "x".repeat(4096);
        let script = format!(
            "i=0; while [ $i -lt 512 ]; do printf '%s' '{block}'; i=$((i+1)); done; \
             printf '\\n{{\"type\":\"result\",\"text\":\"done\"}}\\n'"
        );
        let (result, lines) = sh(&script, 5000, &AtomicBool::new(false));
        assert!(result.success());
        assert!(result.stdout_truncated);
        assert_eq!(lines, [r#"{"type":"result","text":"done"}"#]);
        assert_eq!(result.stdout, format!("{}\n", lines[0]));
    }

    #[test]
    fn large_complete_json_events_are_delivered_without_splitting() {
        let block = "x".repeat(4096);
        let script = format!(
            "printf '{{\"type\":\"result\",\"text\":\"'; \
             i=0; while [ $i -lt 192 ]; do printf '%s' '{block}'; i=$((i+1)); done; \
             printf '\"}}\\n'"
        );
        let (result, lines) = sh(&script, 5000, &AtomicBool::new(false));
        assert!(result.success());
        assert!(!result.stdout_truncated);
        assert_eq!(lines.len(), 1);
        let event: serde_json::Value = serde_json::from_str(&lines[0]).expect("complete JSON");
        assert_eq!(event["type"], "result");
        assert_eq!(event["text"].as_str().unwrap().len(), 192 * 4096);
    }

    #[test]
    fn stdout_capture_and_callbacks_share_the_total_budget() {
        let block = "x".repeat(4095);
        let script =
            format!("i=0; while [ $i -lt 1536 ]; do printf '%s\\n' '{block}'; i=$((i+1)); done");
        let mut delivered = 0;
        let result = sh_with(&script, 5000, &AtomicBool::new(false), |line| {
            assert_eq!(line.len(), 4095, "a protocol line is never split");
            delivered += line.len() + 1;
        });
        assert!(result.success());
        assert!(result.stdout_truncated);
        assert_eq!(delivered, STDOUT_CAP);
        assert_eq!(result.stdout.len(), delivered);
    }

    #[test]
    fn stderr_flood_keeps_a_bounded_tail_and_reports_truncation() {
        let block = "e".repeat(4096);
        let script = format!(
            "i=0; while [ $i -lt 1024 ]; do printf '%s' '{block}' >&2; i=$((i+1)); done; \
             printf 'tail-marker\\n' >&2; echo done"
        );
        let (result, lines) = sh(&script, 5000, &AtomicBool::new(false));
        assert!(result.success());
        assert_eq!(lines, ["done"]);
        assert!(!result.stdout_truncated);
        assert!(result.stderr_truncated);
        assert_eq!(result.stderr_tail.len(), STDERR_CAP);
        assert!(result.stderr_tail.ends_with("tail-marker\n"));
    }

    #[test]
    fn lossy_utf8_cannot_expand_output_past_the_caps() {
        let mut capture = StdoutCapture::default();
        let mut lines = Vec::new();
        let mut input = vec![0xff; STDOUT_LINE_CAP / 2];
        input.extend_from_slice(b"\nvalid\n");
        assert_eq!(
            capture.push(
                &input,
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(5),
                &mut |line| lines.push(line.to_owned()),
            ),
            None
        );
        assert!(capture.truncated);
        assert_eq!(lines, ["valid"]);
        let mut tail = StderrTail::default();
        tail.push(&vec![0xff; STDERR_CAP]);
        assert!(tail.text().len() <= STDERR_CAP);
        assert!(tail.truncated);
    }

    #[test]
    fn a_descendant_holding_stdout_does_not_bypass_the_deadline() {
        let started = Instant::now();
        let (result, _) = sh("sleep 30 & exit 0", 100, &AtomicBool::new(false));
        assert_eq!(result.end, ProcessEnd::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn a_stderr_only_descendant_is_stopped_after_the_parent_exits() {
        let started = Instant::now();
        let (result, _) = sh("sleep 30 >&2 & exit 0", 30_000, &AtomicBool::new(false));
        assert!(result.success());
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn resident_secrets_do_not_reach_the_harness() {
        // `cargo test` sets CARGO_MANIFEST_DIR in this process; like any
        // resident secret, it must not be inherited unless passed by name.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        let (_, lines) = sh(
            "echo \"token=${CARGO_MANIFEST_DIR:-unset}\"; echo \"home=${HOME:+set}\"",
            5_000,
            &AtomicBool::new(false),
        );
        assert_eq!(lines[0], "token=unset");
        assert_eq!(lines[1], "home=set");
    }

    #[test]
    fn binaries_resolve_on_path() {
        assert!(resolve_binary("sh").is_some());
        assert!(resolve_binary("/bin/sh").is_some());
        assert!(resolve_binary("definitely-not-a-kamimusuhi-binary").is_none());
    }
}
