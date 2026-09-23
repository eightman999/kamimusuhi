//! Running a harness process: minimal environment, streamed stdout,
//! deadline and cancellation.
//!
//! The child gets a cleared environment plus a short allowlist and the
//! executor's own `env_passthrough` names, so credentials the resident
//! holds (node token, provider keys) never reach an external agent. The
//! harness authenticates with its own stored login.

use std::io::Read;
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

/// Most stdout kept in memory per run.
const STDOUT_CAP: usize = 4 * 1024 * 1024;
/// Most stderr kept (the tail).
const STDERR_CAP: usize = 16 * 1024;

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

/// Run to completion, deadline or cancellation. `on_line` sees each stdout
/// line as it arrives, on the calling thread.
pub fn run(
    spec: &ProcessSpec<'_>,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<ProcessResult, String> {
    let started = Instant::now();
    let mut child = command(spec)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", spec.program))?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut stderr = child.stderr.take().ok_or("no stderr pipe")?;
    // The reader only moves raw chunks: some harnesses (Bun-based
    // `opencode`) drop output they could not write before exiting when the
    // pipe fills, so the pipe is drained as fast as possible and lines are
    // split on this side.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut stdout = stdout;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (err_tx, err_rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        let start = bytes.len().saturating_sub(STDERR_CAP);
        let _ = err_tx.send(String::from_utf8_lossy(&bytes[start..]).into_owned());
    });
    let deadline = started + spec.timeout;
    let mut captured = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut take_line = |bytes: &[u8], captured: &mut String| {
        let line = String::from_utf8_lossy(bytes);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        on_line(line);
        if captured.len() + line.len() < STDOUT_CAP {
            captured.push_str(line);
            captured.push('\n');
        }
    };
    let mut take = |chunk: Vec<u8>, pending: &mut Vec<u8>, captured: &mut String| {
        pending.extend_from_slice(&chunk);
        let mut start = 0;
        while let Some(at) = pending[start..].iter().position(|b| *b == b'\n') {
            take_line(&pending[start..start + at], captured);
            start += at + 1;
        }
        pending.drain(..start);
    };
    let mut stdout_open = true;
    let (end, status) = loop {
        if stdout_open {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    take(chunk, &mut pending, &mut captured);
                    continue;
                }
                // Closed stdout usually means exit, but a harness may keep
                // running; the deadline still applies.
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_open = false,
                // Exit is only checked once stdout is closed: a harness may
                // exit while a helper process is still writing its output
                // (observed with `opencode models`, which lost half of it).
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        } else {
            thread::sleep(Duration::from_millis(50));
        }
        if cancel.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            break (ProcessEnd::Cancelled, None);
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            break (ProcessEnd::TimedOut, None);
        }
        if !stdout_open && let Ok(Some(status)) = child.try_wait() {
            break (ProcessEnd::Exited, Some(status));
        }
    };
    if !pending.is_empty() {
        take(b"\n".to_vec(), &mut pending, &mut captured);
    }
    if end == ProcessEnd::Exited {
        // Whatever the harness left running in its group goes with it.
        kill_group(&child, "-KILL");
    }
    let stderr_tail = err_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_default();
    Ok(ProcessResult {
        end,
        exit_code: status.and_then(|s| s.code()),
        stdout: captured,
        stderr_tail,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64, cancel: &AtomicBool) -> (ProcessResult, Vec<String>) {
        let args = vec!["-c".to_owned(), script.to_owned()];
        let mut lines = Vec::new();
        let result = run(
            &ProcessSpec {
                program: "/bin/sh",
                args: &args,
                cwd: None,
                env_passthrough: &[],
                extra_env: &[],
                timeout: Duration::from_millis(timeout_ms),
            },
            cancel,
            |l| lines.push(l.to_owned()),
        )
        .expect("runs");
        (result, lines)
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
