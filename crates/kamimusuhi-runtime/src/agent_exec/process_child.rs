//! Single ownership of a harness child, its process group, and its reap.
//!
//! On Linux/macOS, waitid(WNOWAIT) observes exit while the direct child's PID
//! remains reserved. Group signals precede Child::wait; no raw Child escapes
//! this module. This assumes no independent SIGCHLD handler/global reaper.
//! Other platforms retain std::Child polling, but never signal a group after
//! that poll has reaped its leader. They therefore lack the same post-exit
//! descendant cleanup guarantee. Escaped process groups are not a sandbox.

use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::time::Duration;

const TERM_GRACE: Duration = Duration::from_millis(300);

fn ownership_was_lost(error: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        error.raw_os_error() == Some(rustix::io::Errno::CHILD.raw_os_error())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // No portable errno classification is available on the std fallback.
        // Suppress signals when that platform cannot establish ownership.
        let _ = error;
        true
    }
}

#[derive(Debug, Clone, Copy)]
enum Observed {
    Running,
    ExitedUnreaped,
    // The std fallback has already consumed the status. Group signalling is
    // no longer permitted, even if descendants might remain in that group.
    Reaped(ExitStatus),
}

#[derive(Debug)]
enum Ownership {
    Unreaped,
    Reaped(ExitStatus),
    Lost(String),
}

#[derive(Debug, Clone, Copy)]
enum GroupSignal {
    Term,
    Kill,
}

// Keep the ownership state machine testable without signalling arbitrary
// PIDs or attempting to provoke PID reuse. Only OsChild implements real ops.
trait ChildOps {
    fn observe(&mut self) -> io::Result<Observed>;
    fn signal_group(&mut self, signal: GroupSignal);
    fn kill_direct(&mut self);
    fn reap(&mut self) -> io::Result<ExitStatus>;
    fn term_grace(&mut self);
}

struct Lifecycle {
    ownership: Ownership,
}

impl Lifecycle {
    fn new() -> Self {
        Self {
            ownership: Ownership::Unreaped,
        }
    }

    fn lost(&mut self, error: io::Error) -> io::Error {
        let reason = format!("harness child ownership could not be confirmed: {error}");
        self.ownership = Ownership::Lost(reason.clone());
        io::Error::other(reason)
    }

    fn observe(&mut self, ops: &mut impl ChildOps) -> io::Result<Observed> {
        match &self.ownership {
            Ownership::Reaped(status) => return Ok(Observed::Reaped(*status)),
            Ownership::Lost(reason) => return Err(io::Error::other(reason.clone())),
            Ownership::Unreaped => {}
        }
        match ops.observe() {
            Ok(Observed::Reaped(status)) => {
                self.ownership = Ownership::Reaped(status);
                Ok(Observed::Reaped(status))
            }
            Ok(observed) => Ok(observed),
            // Interruption supplies no new status and does not reap. Return
            // to the caller's deadline/cancellation loop without busy retry.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(Observed::Running),
            Err(error) if ownership_was_lost(&error) => Err(self.lost(error)),
            Err(error) => Err(error),
        }
    }

    fn observe_for_cleanup(&mut self, ops: &mut impl ChildOps) -> io::Result<Observed> {
        match self.observe(ops) {
            // EPERM/ENOSYS can deny waitid without reaping our child. The
            // private handle still owns it, so cleanup must not depend on
            // successfully retrying the unavailable observation API.
            Err(_) if matches!(self.ownership, Ownership::Unreaped) => Ok(Observed::Running),
            result => result,
        }
    }

    fn stop_group_and_reap(
        &mut self,
        ops: &mut impl ChildOps,
        direct_still_running: bool,
    ) -> io::Result<ExitStatus> {
        // Reached only while the direct child remains ours and unreaped.
        ops.signal_group(GroupSignal::Kill);
        if direct_still_running {
            // A direct child could have moved out of its original group.
            ops.kill_direct();
        }
        match ops.reap() {
            Ok(status) => {
                self.ownership = Ownership::Reaped(status);
                Ok(status)
            }
            Err(error) if ownership_was_lost(&error) => Err(self.lost(error)),
            Err(error) => Err(error),
        }
    }

    fn try_wait(&mut self, ops: &mut impl ChildOps) -> io::Result<Option<ExitStatus>> {
        match self.observe(ops)? {
            Observed::Running => Ok(None),
            Observed::ExitedUnreaped => self.stop_group_and_reap(ops, false).map(Some),
            Observed::Reaped(status) => Ok(Some(status)),
        }
    }

    fn terminate(&mut self, ops: &mut impl ChildOps) -> io::Result<ExitStatus> {
        match self.observe_for_cleanup(ops)? {
            Observed::Reaped(status) => return Ok(status),
            Observed::ExitedUnreaped => return self.stop_group_and_reap(ops, false),
            Observed::Running => {}
        }
        ops.signal_group(GroupSignal::Term);
        ops.term_grace();
        // Recheck ownership before escalation. In particular, an ECHILD
        // result must not lead to signals based on an old numeric PID.
        match self.observe_for_cleanup(ops)? {
            Observed::Running => self.stop_group_and_reap(ops, true),
            Observed::ExitedUnreaped => self.stop_group_and_reap(ops, false),
            Observed::Reaped(status) => Ok(status),
        }
    }
}

struct OsChild(Child);

impl ChildOps for OsChild {
    fn observe(&mut self) -> io::Result<Observed> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            waitid(
                WaitId::Pid(Pid::from_child(&self.0)),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map(|status| {
                if status.is_some() {
                    Observed::ExitedUnreaped
                } else {
                    Observed::Running
                }
            })
            .map_err(io::Error::from)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            self.0
                .try_wait()
                .map(|status| status.map_or(Observed::Running, Observed::Reaped))
        }
    }

    fn signal_group(&mut self, signal: GroupSignal) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use rustix::process::{Pid, Signal, kill_process_group};
            let signal = match signal {
                GroupSignal::Term => Signal::TERM,
                GroupSignal::Kill => Signal::KILL,
            };
            // An already-empty group may return ESRCH. The unreaped direct
            // child still reserves the numeric PID until the subsequent wait.
            let _ = kill_process_group(Pid::from_child(&self.0), signal);
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
        {
            use std::process::Stdio;
            let signal = match signal {
                GroupSignal::Term => "-TERM",
                GroupSignal::Kill => "-KILL",
            };
            let _ = Command::new("/bin/kill")
                .env_clear()
                .args([signal, "--", &format!("-{}", self.0.id())])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(not(unix))]
        let _ = signal;
    }

    fn kill_direct(&mut self) {
        let _ = self.0.kill();
    }

    fn reap(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }

    fn term_grace(&mut self) {
        std::thread::sleep(TERM_GRACE);
    }
}

pub(crate) struct OwnedChild {
    os: OsChild,
    lifecycle: Lifecycle,
}

impl OwnedChild {
    /// Establish a private process group as part of spawn. No caller can
    /// subsequently reap or signal the raw Child behind this owner.
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        Ok(Self {
            os: OsChild(command.spawn()?),
            lifecycle: Lifecycle::new(),
        })
    }

    /// Used by the existing server pidfile. This does not transfer ownership.
    pub(crate) fn id(&self) -> u32 {
        self.os.0.id()
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.os.0.stdin.take()
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.os.0.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.os.0.stderr.take()
    }

    /// On Linux/macOS this observes without reaping, stops the group, then
    /// reaps. Subsequent polls return the cached status without OS calls.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.lifecycle.try_wait(&mut self.os)
    }

    pub(crate) fn terminate(&mut self) -> io::Result<ExitStatus> {
        self.lifecycle.terminate(&mut self.os)
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;

    fn lost_error() -> io::Error {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            rustix::io::Errno::CHILD.into()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        io::Error::other("no owned child")
    }

    struct Recorder {
        observations: VecDeque<io::Result<Observed>>,
        events: Vec<&'static str>,
        status: io::Result<ExitStatus>,
    }

    impl Recorder {
        fn new(observations: impl IntoIterator<Item = io::Result<Observed>>) -> Self {
            Self {
                observations: observations.into_iter().collect(),
                events: Vec::new(),
                status: Ok(ExitStatus::from_raw(37 << 8)),
            }
        }
    }

    impl ChildOps for Recorder {
        fn observe(&mut self) -> io::Result<Observed> {
            self.events.push("observe");
            self.observations.pop_front().expect("scripted observation")
        }

        fn signal_group(&mut self, signal: GroupSignal) {
            self.events.push(match signal {
                GroupSignal::Term => "group TERM",
                GroupSignal::Kill => "group KILL",
            });
        }

        fn kill_direct(&mut self) {
            self.events.push("direct KILL");
        }

        fn reap(&mut self) -> io::Result<ExitStatus> {
            self.events.push("reap");
            std::mem::replace(&mut self.status, Err(io::Error::other("already reaped")))
        }

        fn term_grace(&mut self) {
            self.events.push("TERM grace");
        }
    }

    #[test]
    fn observed_exit_stops_the_group_before_reap_and_cleanup_is_idempotent() {
        let mut lifecycle = Lifecycle::new();
        let mut ops = Recorder::new([Ok(Observed::ExitedUnreaped)]);
        assert_eq!(
            lifecycle.try_wait(&mut ops).unwrap().unwrap().code(),
            Some(37)
        );
        assert_eq!(ops.events, ["observe", "group KILL", "reap"]);
        assert_eq!(lifecycle.terminate(&mut ops).unwrap().code(), Some(37));
        assert_eq!(
            lifecycle.try_wait(&mut ops).unwrap().unwrap().code(),
            Some(37)
        );
        assert_eq!(ops.events, ["observe", "group KILL", "reap"]);
    }

    #[test]
    fn termination_retains_ownership_through_grace_and_escalation() {
        let mut lifecycle = Lifecycle::new();
        let mut ops = Recorder::new([Ok(Observed::Running), Ok(Observed::Running)]);
        assert_eq!(lifecycle.terminate(&mut ops).unwrap().code(), Some(37));
        assert_eq!(
            ops.events,
            [
                "observe",
                "group TERM",
                "TERM grace",
                "observe",
                "group KILL",
                "direct KILL",
                "reap"
            ]
        );
        let recorded = ops.events.clone();
        lifecycle.terminate(&mut ops).unwrap();
        assert_eq!(ops.events, recorded);
    }

    #[test]
    fn lost_ownership_never_signals_or_reaps_again() {
        for after_term in [false, true] {
            let mut observations = Vec::new();
            if after_term {
                observations.push(Ok(Observed::Running));
            }
            observations.push(Err(lost_error()));
            let mut lifecycle = Lifecycle::new();
            let mut ops = Recorder::new(observations);
            assert!(lifecycle.terminate(&mut ops).is_err());
            let expected = if after_term {
                vec!["observe", "group TERM", "TERM grace", "observe"]
            } else {
                vec!["observe"]
            };
            assert_eq!(ops.events, expected);
            assert!(lifecycle.terminate(&mut ops).is_err());
            assert!(lifecycle.try_wait(&mut ops).is_err());
            assert_eq!(ops.events, expected);
        }
    }

    #[test]
    fn fallback_reaped_status_never_receives_a_group_signal() {
        let status = ExitStatus::from_raw(0);
        let mut lifecycle = Lifecycle::new();
        let mut ops = Recorder::new([Ok(Observed::Reaped(status))]);
        assert_eq!(lifecycle.try_wait(&mut ops).unwrap(), Some(status));
        assert_eq!(lifecycle.terminate(&mut ops).unwrap(), status);
        assert_eq!(ops.events, ["observe"]);
    }

    #[test]
    fn interrupted_observation_keeps_the_child_waitable() {
        let mut lifecycle = Lifecycle::new();
        let mut ops = Recorder::new([
            Err(io::Error::from(io::ErrorKind::Interrupted)),
            Ok(Observed::ExitedUnreaped),
        ]);
        assert!(lifecycle.try_wait(&mut ops).unwrap().is_none());
        assert_eq!(
            lifecycle.try_wait(&mut ops).unwrap().unwrap().code(),
            Some(37)
        );
        assert_eq!(ops.events, ["observe", "observe", "group KILL", "reap"]);
    }

    #[test]
    fn failed_reap_does_not_allow_cleanup_to_signal_the_pid_again() {
        let mut lifecycle = Lifecycle::new();
        let mut ops = Recorder::new([Ok(Observed::ExitedUnreaped)]);
        ops.status = Err(lost_error());
        assert!(lifecycle.try_wait(&mut ops).is_err());
        assert!(lifecycle.terminate(&mut ops).is_err());
        assert_eq!(ops.events, ["observe", "group KILL", "reap"]);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unavailable_observation_does_not_abandon_the_owned_child() {
        for error in [rustix::io::Errno::PERM, rustix::io::Errno::NOSYS] {
            let mut lifecycle = Lifecycle::new();
            let mut ops = Recorder::new([Err(error.into()), Err(error.into()), Err(error.into())]);
            assert!(lifecycle.try_wait(&mut ops).is_err());
            assert_eq!(lifecycle.terminate(&mut ops).unwrap().code(), Some(37));
            assert_eq!(
                ops.events,
                [
                    "observe",
                    "observe",
                    "group TERM",
                    "TERM grace",
                    "observe",
                    "group KILL",
                    "direct KILL",
                    "reap"
                ]
            );
            let recorded = ops.events.clone();
            lifecycle.terminate(&mut ops).unwrap();
            assert_eq!(ops.events, recorded);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn real_nowait_observation_leaves_the_owned_exit_for_wait() {
        use std::process::Stdio;
        use std::time::Instant;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exit 37"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = OwnedChild::spawn(&mut command).expect("owned fixture child");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.os.observe().expect("observe owned child") {
                Observed::ExitedUnreaped => break,
                Observed::Running => {
                    assert!(Instant::now() < deadline, "fixture did not exit");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Observed::Reaped(_) => panic!("NOWAIT consumed the exit"),
            }
        }
        assert!(matches!(
            child.os.observe().unwrap(),
            Observed::ExitedUnreaped
        ));
        assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(37));
        assert_eq!(child.terminate().unwrap().code(), Some(37));
    }
}
