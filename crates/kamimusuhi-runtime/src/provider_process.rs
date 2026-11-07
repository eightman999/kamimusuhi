//! One-shot, exec-based provider transport with no pipe helper threads.
//!
//! Parent-side Unix sockets are nonblocking. The child receives ordinary
//! blocking stdin/stdout descriptors, so a child that never reads, never
//! writes, or leaves a descendant holding a descriptor cannot stall the host.
//! Every return path kills the process group and reaps the direct child before
//! releasing admission. This is lifetime isolation, not an OS security sandbox.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::llm_jev::{CancellationToken, ConversationError};

pub const MAX_PROVIDER_PROCESSES: usize = 64;
pub const MAX_PROVIDER_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_PROVIDER_REPLY_BYTES: usize = 1024 * 1024;
static ACTIVE_PROCESSES: AtomicUsize = AtomicUsize::new(0);

/// Includes every admitted child until kill/reap has completed.
pub fn active_provider_processes() -> usize {
    ACTIVE_PROCESSES.load(Ordering::Acquire)
}

struct Admission;

impl Admission {
    fn acquire() -> Result<Self, ConversationError> {
        ACTIVE_PROCESSES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_PROVIDER_PROCESSES).then_some(active + 1)
            })
            .map_err(|_| ConversationError::Transport)?;
        Ok(Self)
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        ACTIVE_PROCESSES.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Executable and fixed arguments. Requests, credentials and response content
/// are never placed in arguments or surfaced in transport errors.
#[derive(Clone, Debug)]
pub struct ProcessCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl ProcessCommand {
    pub fn invoke(
        &self,
        request: &Value,
        timeout: Duration,
        cancellation: Option<&CancellationToken>,
    ) -> Result<Value, ConversationError> {
        if timeout.is_zero() {
            return Err(ConversationError::Timeout);
        }
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(ConversationError::Cancelled);
        }
        let started = Instant::now();
        let mut request = serde_json::to_vec(request)
            .map_err(|_| ConversationError::Serialization("provider request".to_owned()))?;
        if request.len() >= MAX_PROVIDER_REQUEST_BYTES {
            return Err(ConversationError::InvalidConfig(
                "provider request exceeds byte limit".to_owned(),
            ));
        }
        request.push(b'\n');
        #[cfg(unix)]
        {
            self.invoke_unix(&request, started, timeout, cancellation)
        }
        #[cfg(not(unix))]
        {
            let _ = (request, started, timeout, cancellation);
            Err(ConversationError::InvalidConfig(
                "provider process transport requires Unix".to_owned(),
            ))
        }
    }

    #[cfg(unix)]
    fn invoke_unix(
        &self,
        request: &[u8],
        started: Instant,
        timeout: Duration,
        cancellation: Option<&CancellationToken>,
    ) -> Result<Value, ConversationError> {
        use std::io::{ErrorKind, Read, Write};
        use std::net::Shutdown;
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let _admission = Admission::acquire()?;
        let (mut input, child_input) =
            UnixStream::pair().map_err(|_| ConversationError::Transport)?;
        let (mut output, child_output) =
            UnixStream::pair().map_err(|_| ConversationError::Transport)?;
        input
            .set_nonblocking(true)
            .map_err(|_| ConversationError::Transport)?;
        output
            .set_nonblocking(true)
            .map_err(|_| ConversationError::Transport)?;
        if started.elapsed() >= timeout {
            return Err(ConversationError::Timeout);
        }
        let child = Command::new(&self.program)
            .args(&self.args)
            .process_group(0)
            .stdin(Stdio::from(OwnedFd::from(child_input)))
            .stdout(Stdio::from(OwnedFd::from(child_output)))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ConversationError::Transport)?;
        // Declared after admission: it is dropped and reaped before admission
        // is released, including early errors and unwinding.
        let mut child = ChildGuard {
            child,
            reaped: false,
            group_killed: false,
        };
        let mut written = 0;
        let mut input_closed = false;
        let mut output_closed = false;
        let mut response = Vec::new();
        let mut status = None;
        let mut buffer = [0_u8; 8192];
        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err(ConversationError::Cancelled);
            }
            if started.elapsed() >= timeout {
                return Err(ConversationError::Timeout);
            }
            if !input_closed {
                match input.write(&request[written..]) {
                    Ok(0) => return Err(ConversationError::Transport),
                    Ok(count) => written += count,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    // A child is permitted to reject a request without reading
                    // it all. Its exit/response determines the eventual result.
                    Err(error) if error.kind() == ErrorKind::BrokenPipe => {
                        input_closed = true;
                    }
                    Err(_) => return Err(ConversationError::Transport),
                }
                if written == request.len() {
                    input
                        .shutdown(Shutdown::Write)
                        .map_err(|_| ConversationError::Transport)?;
                    input_closed = true;
                }
            }
            if !output_closed {
                // One bounded read per iteration. Deadline/cancellation are
                // rechecked even if a broken provider floods stdout forever.
                match output.read(&mut buffer) {
                    Ok(0) => output_closed = true,
                    Ok(count) => {
                        if response.len().saturating_add(count) > MAX_PROVIDER_REPLY_BYTES {
                            return Err(ConversationError::Malformed(
                                "provider reply exceeds byte limit".to_owned(),
                            ));
                        }
                        response.extend_from_slice(&buffer[..count]);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return Err(ConversationError::Transport),
                }
            }
            if status.is_none()
                && let Some(exit) = child
                    .child
                    .try_wait()
                    .map_err(|_| ConversationError::Transport)?
            {
                child.reaped = true;
                status = Some(exit);
                // Descendants may still own stdin/stdout after the direct
                // child exits. Kill the entire group before awaiting EOF.
                child.kill_group();
            }
            if output_closed && let Some(exit) = status {
                if !exit.success() {
                    return Err(ConversationError::Transport);
                }
                return serde_json::from_slice(&response).map_err(|_| {
                    ConversationError::Malformed("invalid provider reply".to_owned())
                });
            }
            // Keep latency/cancellation bounded without busy-spinning. There
            // are no detached writer/reader threads or asynchronous queues.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(unix)]
struct ChildGuard {
    child: std::process::Child,
    reaped: bool,
    group_killed: bool,
}

#[cfg(unix)]
impl ChildGuard {
    fn kill_group(&mut self) {
        use std::process::{Command, Stdio};
        if self.group_killed {
            return;
        }
        self.group_killed = true;
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", self.child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_group();
        if !self.reaped {
            let _ = self.child.kill();
            // SIGKILL ends a non-cooperative userspace provider; wait reaps it
            // synchronously rather than releasing the slot and leaking it.
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> ProcessCommand {
        ProcessCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_owned(), script.to_owned()],
        }
    }

    #[test]
    fn successful_json_process_is_reaped() {
        let result = shell("cat >/dev/null; printf '{\"ok\":true}'")
            .invoke(
                &serde_json::json!({"request": 1}),
                Duration::from_secs(2),
                None,
            )
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
    }

    #[test]
    fn repeated_permanent_hangs_release_admission_and_allow_healthy_calls() {
        for _ in 0..80 {
            let started = Instant::now();
            assert!(matches!(
                shell("while :; do sleep 60; done").invoke(
                    &serde_json::json!({}),
                    Duration::from_millis(10),
                    None
                ),
                Err(ConversationError::Timeout)
            ));
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        successful_json_process_is_reaped();
    }

    #[test]
    fn a_child_that_never_reads_stdin_cannot_leak_a_writer_thread() {
        let request = serde_json::json!({"large": "x".repeat(800_000)});
        let started = Instant::now();
        assert!(matches!(
            shell("sleep 60").invoke(&request, Duration::from_millis(25), None),
            Err(ConversationError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn descendant_inherited_stdout_does_not_delay_successful_exit() {
        let started = Instant::now();
        assert_eq!(
            shell("(sleep 60) & printf '{\"ok\":true}'")
                .invoke(&serde_json::json!({}), Duration::from_secs(2), None)
                .unwrap(),
            serde_json::json!({"ok": true})
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn oversized_output_is_killed_and_returns_a_fixed_error() {
        let result = shell("yes raw-secret-provider-error").invoke(
            &serde_json::json!({}),
            Duration::from_secs(3),
            None,
        );
        assert!(matches!(result, Err(ConversationError::Malformed(_))));
        assert!(!result.unwrap_err().to_string().contains("raw-secret"));
    }

    #[test]
    fn cancellation_reaps_a_noncooperative_child() {
        let cancellation = CancellationToken::new();
        let other = cancellation.clone();
        let controller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            other.cancel();
        });
        let started = Instant::now();
        let result = shell("sleep 60").invoke(
            &serde_json::json!({}),
            Duration::from_secs(2),
            Some(&cancellation),
        );
        controller.join().unwrap();
        assert!(matches!(result, Err(ConversationError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
