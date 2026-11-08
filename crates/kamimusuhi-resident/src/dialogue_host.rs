//! Per-subject persistent dialogue hosts.
//!
//! Each subject maps to one long-lived `kamimusuhi-runtime chat --quiet`
//! child held with stdin/stdout pipes. The child speaks a JSON-lines
//! protocol: `{"message": "<text>", "turn": "<task_id>"}` in, one complete
//! reply JSON document out per turn. Keeping the process — and its
//! `DialogueSession` — alive across turns is what gives a subject
//! continuity: warm relationship memory, one session id, no per-turn
//! process and init cost.
//!
//! Lifecycle: a host is spawned on first use, reused while it stays
//! healthy, and reaped once it has been idle for `session_idle_secs` or
//! evicted when the pool is full (least recently used). A host that dies
//! or times out mid-turn is dropped, never repaired — the next turn
//! spawns a fresh process transparently. Canonical state lives in the
//! runtime directory's SQLite store, so continuity is not lost when a
//! host is replaced; only the in-memory session warms again.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// One stdout line delivered by the reader thread; `Err` carries the read
/// error that ends the stream.
type Line = Result<String, String>;

/// Lines of child stderr retained for failure diagnostics.
const STDERR_TAIL_LINES: usize = 10;

/// What a host turn produced, plus the pool bookkeeping a caller may want
/// to surface (e.g. whether this turn reused an existing process).
#[derive(Debug)]
pub struct TurnOutcome {
    /// The child's reply document — same shape `talk` emits.
    pub reply: Value,
    /// OS pid of the host that served the turn.
    pub pid: u32,
    /// Whether this turn ran on an already-warm host.
    pub reused: bool,
    /// Turns served by this host including this one.
    pub turns_on_host: u64,
}

/// Why a turn could not be served. Callers map these to HTTP statuses the
/// same way `talk` maps its process outcomes.
#[derive(Debug)]
pub enum HostError {
    /// Child could not be spawned.
    Spawn(String),
    /// The host produced no reply inside the turn deadline. The host is
    /// killed — a pending line arriving later would desync the protocol.
    Timeout,
    /// The child exited or its stdout closed mid-turn. Carries the reason
    /// and the child's last stderr lines for diagnostics.
    Dead(String, Vec<String>),
    /// Writing to or flushing the child's stdin failed.
    Io(String),
    /// The child answered, but with a protocol `{"error": ...}` document —
    /// the second field is the child's stderr tail.
    Child(String, Vec<String>),
}

struct Host {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Line>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    last_used: Instant,
    turns: u64,
}

impl Host {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Kill the child and reap it. Dropping a live `Child` does not kill
    /// it — a host must always go through this.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Whether the child is still running. `try_wait` also reaps it when
    /// it already exited, so a zombie never lingers.
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

/// Spawn one `chat --quiet` child. `env` carries spawn-time variables
/// (none today — the per-turn task id travels inside the protocol).
fn spawn(binary: &Path, args: &[String], env: &[(String, String)]) -> Result<Host, HostError> {
    let mut child = Command::new(binary)
        .args(args)
        .envs(env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| HostError::Spawn(format!("could not start dialogue host: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| HostError::Spawn("child stdin was not piped".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Spawn("child stdout was not piped".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostError::Spawn("child stderr was not piped".to_owned()))?;

    // A dedicated reader turns the child's stdout into a channel of
    // lines, which is what lets a turn wait with a deadline.
    let (tx, lines) = channel::<Line>();
    thread::Builder::new()
        .name("dialogue-host-stdout".to_owned())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.map_err(|e| e.to_string())).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| HostError::Spawn(format!("stdout reader thread: {e}")))?;

    // stderr is piped but only kept for diagnostics — draining it on a
    // thread stops a chatty child from deadlocking on a full pipe.
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES + 1)));
    let tail_writer = Arc::clone(&tail);
    thread::Builder::new()
        .name("dialogue-host-stderr".to_owned())
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut buf = tail_writer.lock().unwrap_or_else(|p| p.into_inner());
                buf.push_back(line);
                while buf.len() > STDERR_TAIL_LINES {
                    buf.pop_front();
                }
            }
        })
        .map_err(|e| HostError::Spawn(format!("stderr reader thread: {e}")))?;

    Ok(Host {
        child,
        stdin,
        lines,
        stderr_tail: tail,
        last_used: Instant::now(),
        turns: 0,
    })
}

/// Run one turn on `host`. Any error leaves the host's protocol state
/// unknown — the caller must drop it, never reuse it.
fn run_turn(
    host: &mut Host,
    message: &str,
    turn: &str,
    timeout: Duration,
    identity_context: Option<&Value>,
) -> Result<Value, HostError> {
    if !host.alive() {
        return Err(HostError::Dead(
            "host already exited".to_owned(),
            host.stderr_tail(),
        ));
    }
    let mut request = serde_json::json!({"message": message, "turn": turn});
    if let Some(context) = identity_context {
        request["identity_context"] = context.clone();
    }
    let request = request.to_string();
    host.stdin
        .write_all(request.as_bytes())
        .and_then(|()| host.stdin.write_all(b"\n"))
        .and_then(|()| host.stdin.flush())
        .map_err(|e| HostError::Io(format!("write to host stdin failed: {e}")))?;

    let deadline = Instant::now() + timeout;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(HostError::Timeout);
        };
        match host.lines.recv_timeout(remaining) {
            Ok(Ok(line)) => {
                // Protocol lines are complete JSON documents; anything else
                // (a stray log line on stdout) is skipped, never consumed
                // as a reply.
                let Ok(doc) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(error) = doc.get("error").and_then(Value::as_str) {
                    return Err(HostError::Child(error.to_owned(), host.stderr_tail()));
                }
                return Ok(doc);
            }
            Ok(Err(error)) => {
                return Err(HostError::Dead(
                    format!("host stdout failed: {error}"),
                    host.stderr_tail(),
                ));
            }
            Err(RecvTimeoutError::Timeout) => return Err(HostError::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(HostError::Dead(
                    "host stdout closed".to_owned(),
                    host.stderr_tail(),
                ));
            }
        }
    }
}

/// What the pool needs to spawn a host: everything that goes on the
/// child's command line.
#[derive(Debug, Clone)]
pub struct HostSpec {
    pub runtime_binary: std::path::PathBuf,
    pub dir: std::path::PathBuf,
    pub subject: String,
    pub privacy: String,
    /// Extra environment for the child (empty today).
    pub env: Vec<(String, String)>,
}

impl HostSpec {
    fn args(&self) -> Vec<String> {
        vec![
            "chat".to_owned(),
            "--dir".to_owned(),
            self.dir.to_string_lossy().into_owned(),
            "--subject".to_owned(),
            self.subject.clone(),
            "--privacy".to_owned(),
            self.privacy.clone(),
            "--quiet".to_owned(),
        ]
    }
}

struct PoolInner {
    hosts: HashMap<String, Host>,
    /// Spawn counter for observability and tests.
    total_spawns: u64,
}

/// Map of subject → warm dialogue host. Cheap to clone-share via `Arc`;
/// the map itself lives behind one mutex, but a turn runs *outside* the
/// lock: the host is checked out, served, and checked back in, so a slow
/// turn for one subject never blocks another's.
pub struct HostPool {
    inner: Mutex<PoolInner>,
    /// A host idle this long is reaped on the next pool access.
    idle: Duration,
    /// Maximum hosts held at once; a new subject evicts the LRU host.
    max: usize,
}

impl HostPool {
    pub fn new(idle: Duration, max: usize) -> Self {
        Self {
            inner: Mutex::new(PoolInner {
                hosts: HashMap::new(),
                total_spawns: 0,
            }),
            idle,
            max: max.max(1),
        }
    }

    /// Number of warm hosts — the `dialogue_hosts` field on `/health`.
    pub fn host_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .hosts
            .len()
    }

    /// Total hosts ever spawned — a reuse ratio for tests and debugging.
    pub fn total_spawns(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .total_spawns
    }

    /// Drop every host idle longer than the configured bound, plus any
    /// that already exited. Called lazily inside `turn` — no reaper
    /// thread, so a quiet pool holds no threads at all.
    fn reap(inner: &mut PoolInner, idle: Duration) {
        inner.hosts.retain(|_, host| {
            let keep = host.last_used.elapsed() <= idle && host.alive();
            if !keep {
                host.kill();
            }
            keep
        });
    }

    /// Evict the least-recently-used host when the pool is at capacity.
    fn evict_lru(inner: &mut PoolInner) {
        let Some(oldest) = inner
            .hosts
            .iter()
            .min_by_key(|(_, h)| h.last_used)
            .map(|(s, _)| s.clone())
        else {
            return;
        };
        if let Some(mut host) = inner.hosts.remove(&oldest) {
            host.kill();
        }
    }

    /// Serve one turn for `spec.subject`. Spawns on first use, reuses the
    /// warm host while it stays healthy, replaces it transparently when
    /// it dies or goes stale.
    ///
    /// A `Dead` or `Io` failure is retried once on a fresh host: a child
    /// that produced no reply line provably never answered, so the retry
    /// cannot double-record the message. `Timeout` and protocol `error`
    /// documents are *not* retried — the child may still have processed
    /// (or refused) the message.
    pub fn turn(
        &self,
        spec: &HostSpec,
        message: &str,
        turn_id: &str,
        timeout: Duration,
    ) -> Result<TurnOutcome, HostError> {
        self.turn_with_identity(spec, message, turn_id, timeout, None)
    }

    /// Same protocol with an authenticated parent-owned identity context.
    pub fn turn_with_identity(
        &self,
        spec: &HostSpec,
        message: &str,
        turn_id: &str,
        timeout: Duration,
        identity_context: Option<&Value>,
    ) -> Result<TurnOutcome, HostError> {
        // Up to two attempts: the second only after a provably-unanswered
        // first, and always on a fresh process.
        for attempt in 0..2 {
            let mut host = self.checkout(spec)?;
            let reused = host.turns > 0;
            match run_turn(&mut host, message, turn_id, timeout, identity_context) {
                Ok(reply) => {
                    host.turns += 1;
                    host.last_used = Instant::now();
                    let outcome = TurnOutcome {
                        reply,
                        pid: host.pid(),
                        reused,
                        turns_on_host: host.turns,
                    };
                    self.inner
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .hosts
                        .insert(spec.subject.clone(), host);
                    return Ok(outcome);
                }
                Err(error @ (HostError::Dead(..) | HostError::Io(_))) if attempt == 0 => {
                    host.kill();
                    drop(error);
                    continue;
                }
                Err(error) => {
                    host.kill();
                    return Err(error);
                }
            }
        }
        unreachable!("the loop returns on its last iteration")
    }

    /// Take the subject's host out of the map, spawning (or respawning a
    /// dead one) as needed. The caller holds it exclusively until it is
    /// checked back in or dropped.
    fn checkout(&self, spec: &HostSpec) -> Result<Host, HostError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        Self::reap(&mut inner, self.idle);
        match inner.hosts.remove(&spec.subject) {
            Some(mut host) => {
                if host.alive() {
                    Ok(host)
                } else {
                    host.kill();
                    inner.total_spawns += 1;
                    spawn(&spec.runtime_binary, &spec.args(), &spec.env)
                }
            }
            None => {
                if inner.hosts.len() >= self.max {
                    Self::evict_lru(&mut inner);
                }
                inner.total_spawns += 1;
                spawn(&spec.runtime_binary, &spec.args(), &spec.env)
            }
        }
    }
}

impl Drop for HostPool {
    fn drop(&mut self) {
        for (_, mut host) in self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .hosts
            .drain()
        {
            host.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake `chat --quiet` child: a shell that echoes one JSON reply per
    /// input line, so pool lifecycle is exercised without a real runtime.
    /// $script is the per-line body appended to the base loop.
    #[cfg(unix)]
    fn fake_host_script(dir: &Path, body: &str) -> HostSpec {
        let bin = dir.join("fake-chat");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nwhile IFS= read -r line; do\n{body}\ndone\n"),
        )
        .expect("script");
        let mut perms = std::fs::metadata(&bin).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&bin, perms).expect("chmod");
        HostSpec {
            runtime_binary: bin,
            dir: dir.to_path_buf(),
            subject: "s1".to_owned(),
            privacy: "unconstrained".to_owned(),
            env: Vec::new(),
        }
    }

    #[cfg(unix)]
    const ECHO: &str = "printf '%s\\n' '{\"response\":\"pong\"}'";

    #[cfg(unix)]
    fn pool() -> HostPool {
        HostPool::new(Duration::from_secs(1800), 8)
    }

    #[cfg(unix)]
    #[test]
    fn identity_context_is_refreshed_on_each_warm_host_turn() {
        let dir = tempfile::tempdir().unwrap();
        let spec = fake_host_script(dir.path(), "printf '%s\\n' \"$line\"");
        let pool = pool();
        let first = serde_json::json!({"version":1,"declarations":{"self":"first"}});
        let second = serde_json::json!({"version":1,"declarations":{"self":"changed"}});
        let a = pool
            .turn_with_identity(&spec, "hi", "t1", Duration::from_secs(5), Some(&first))
            .unwrap();
        let b = pool
            .turn_with_identity(&spec, "hi", "t2", Duration::from_secs(5), Some(&second))
            .unwrap();
        assert_eq!(a.pid, b.pid);
        assert_eq!(a.reply["identity_context"], first);
        assert_eq!(b.reply["identity_context"], second);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_turn_reuses_the_same_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = fake_host_script(dir.path(), ECHO);
        let pool = pool();
        let first = pool
            .turn(&spec, "hi", "t1", Duration::from_secs(5))
            .expect("t1");
        let second = pool
            .turn(&spec, "hi again", "t2", Duration::from_secs(5))
            .expect("t2");
        assert_eq!(first.reply["response"], "pong");
        assert_eq!(second.reply["response"], "pong");
        assert_eq!(first.pid, second.pid, "same process served both turns");
        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(pool.total_spawns(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_dead_host_is_replaced_transparently() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The child answers once then exits — the next turn must spawn fresh.
        let spec = fake_host_script(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"one\"}'\nexit 0",
        );
        let pool = pool();
        let first = pool
            .turn(&spec, "hi", "t1", Duration::from_secs(5))
            .expect("t1");
        assert_eq!(first.reply["response"], "one");
        let second = pool
            .turn(&spec, "hi", "t2", Duration::from_secs(5))
            .expect("t2");
        assert_eq!(second.reply["response"], "one");
        assert_ne!(first.pid, second.pid, "dead host was respawned");
        assert_eq!(pool.total_spawns(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_kills_the_host_and_the_next_turn_respawns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = fake_host_script(dir.path(), "sleep 2");
        let pool = pool();
        let error = pool
            .turn(&spec, "hi", "t1", Duration::from_millis(300))
            .expect_err("turn times out");
        assert!(matches!(error, HostError::Timeout));
        assert_eq!(pool.host_count(), 0, "dead host was not kept");
        // Next turn works on a fresh process.
        let dir2 = tempfile::tempdir().expect("tempdir2");
        let spec2 = fake_host_script(dir2.path(), ECHO);
        let ok = pool
            .turn(&spec2, "hi", "t2", Duration::from_secs(5))
            .expect("t2");
        assert_eq!(ok.reply["response"], "pong");
    }

    #[cfg(unix)]
    #[test]
    fn idle_hosts_are_reaped_and_full_pools_evict_the_lru() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Reap: an idle bound of zero means every checked-in host is stale
        // the moment the next access runs.
        let pool = HostPool::new(Duration::ZERO, 8);
        let spec = fake_host_script(dir.path(), ECHO);
        pool.turn(&spec, "hi", "t1", Duration::from_secs(5))
            .expect("t1");
        std::thread::sleep(Duration::from_millis(20));
        let second = pool
            .turn(&spec, "hi", "t2", Duration::from_secs(5))
            .expect("t2");
        assert!(!second.reused, "idle host was reaped, not reused");
        assert_eq!(pool.total_spawns(), 2);

        // LRU: two subjects under max=1 — the second evicts the first.
        let pool = HostPool::new(Duration::from_secs(1800), 1);
        let mut spec_b = fake_host_script(dir.path(), ECHO);
        spec_b.subject = "s2".to_owned();
        pool.turn(&spec, "hi", "t1", Duration::from_secs(5))
            .expect("t1");
        pool.turn(&spec_b, "hi", "t2", Duration::from_secs(5))
            .expect("t2");
        assert_eq!(pool.host_count(), 1);
        // s1's host was evicted — its next turn spawns again.
        let again = pool
            .turn(&spec, "hi", "t3", Duration::from_secs(5))
            .expect("t3");
        assert!(!again.reused);
        assert_eq!(pool.total_spawns(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_error_document_fails_the_turn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = fake_host_script(dir.path(), "printf '%s\\n' '{\"error\":\"bad input\"}'");
        let pool = pool();
        let error = pool
            .turn(&spec, "hi", "t1", Duration::from_secs(5))
            .expect_err("error doc");
        assert!(matches!(error, HostError::Child(..)));
    }
}
