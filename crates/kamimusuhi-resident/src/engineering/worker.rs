//! One native command or verification, owned by a separate resident process.
//!
//! The parent's stdin writer stays open after the request. EOF, any further
//! input, or a read error cancels the operation inside this worker; command
//! deadlines and process-group cleanup therefore survive the resident's death
//! while this worker remains alive. This is not an OS sandbox, a durable
//! execution fence, or a guarantee that escaped descendants have terminated.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::lease::{AcquireError, Mode};
use super::workspace::{self, Action, Candidate, Limits, Observation, Validation, WorkspaceConfig};
use super::{Job, PendingAction, Phase, execution};

pub(super) const ENTRY: &str = "engineering-worker";
const VERSION: u32 = 2;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BYTES: usize = 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OperationKind {
    Run,
    Verify,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Run { argv: Vec<String> },
    Verify,
}

impl Operation {
    fn from_action(action: &Action) -> Result<Self, String> {
        match action {
            Action::Run { argv } => Ok(Self::Run { argv: argv.clone() }),
            Action::Verify | Action::Finish { .. } => Ok(Self::Verify),
            _ => Err("engineering worker requires Run or verification".into()),
        }
    }

    fn kind(&self) -> OperationKind {
        match self {
            Self::Run { .. } => OperationKind::Run,
            Self::Verify => OperationKind::Verify,
        }
    }

    fn matches(&self, action: &Action) -> bool {
        match (self, action) {
            (Self::Run { argv }, Action::Run { argv: claimed }) => argv == claimed,
            (Self::Verify, Action::Verify | Action::Finish { .. }) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireLimits {
    operation_timeout_ms: u64,
    validation_timeout_ms: u64,
    output_bytes: usize,
}

impl WireLimits {
    fn from_limits(limits: &Limits) -> Result<Self, String> {
        Ok(Self {
            operation_timeout_ms: u64::try_from(limits.operation_timeout.as_millis())
                .map_err(|_| "engineering worker operation timeout is too large")?,
            validation_timeout_ms: u64::try_from(limits.validation_timeout.as_millis())
                .map_err(|_| "engineering worker validation timeout is too large")?,
            output_bytes: limits.output_bytes,
        })
    }

    fn limits(self) -> Limits {
        Limits {
            operation_timeout: Duration::from_millis(self.operation_timeout_ms),
            validation_timeout: Duration::from_millis(self.validation_timeout_ms),
            output_bytes: self.output_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    version: u32,
    state_path: PathBuf,
    job_id: String,
    sequence: u64,
    policy: WorkspaceConfig,
    candidate: Candidate,
    limits: WireLimits,
    // Finish's summary is compared with the persisted claim only. The worker
    // never changes working memory, job phase, or the parent's checkpoint.
    claimed_action: Action,
    operation: Operation,
}

impl Request {
    pub(super) fn new(
        state_path: &Path,
        job: &Job,
        policy: &WorkspaceConfig,
        limits: &Limits,
    ) -> Result<Self, String> {
        let pending = job
            .pending
            .as_ref()
            .ok_or("engineering worker requires a durable pending action")?;
        let operation = Operation::from_action(&pending.action)?;
        let request = Self {
            version: VERSION,
            state_path: std::path::absolute(state_path)
                .map_err(|_| "engineering worker checkpoint path is invalid")?,
            job_id: job.id.clone(),
            sequence: pending.sequence,
            policy: policy.clone(),
            candidate: job.candidate.clone(),
            limits: WireLimits::from_limits(limits)?,
            claimed_action: pending.action.clone(),
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || self.sequence == 0
            || self.job_id.is_empty()
            || self.job_id.len() > 80
            || !self
                .job_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !self.state_path.is_absolute()
            || !self.candidate.path.is_absolute()
            || !self.operation.matches(&self.claimed_action)
        {
            return Err("engineering worker request identity is invalid".into());
        }
        self.policy.validate()?;
        if !(1..=3_600_000).contains(&self.limits.operation_timeout_ms)
            || !(1..=86_400_000).contains(&self.limits.validation_timeout_ms)
            || !(1024..=262_144).contains(&self.limits.output_bytes)
            || self.limits.output_bytes.saturating_sub(256) / self.policy.checks.len() < 512
        {
            return Err("engineering worker limits are invalid".into());
        }
        if let Operation::Run { argv } = &self.operation
            && (argv.is_empty()
                || argv.len() > 64
                || argv.iter().any(|arg| arg.contains('\0'))
                || argv.iter().map(String::len).sum::<usize>() > 16 * 1024
                || !self.policy.commands.contains(&argv[0]))
        {
            return Err("engineering worker command is outside workspace policy".into());
        }
        Ok(())
    }

    fn timeout(&self) -> Duration {
        match self.operation {
            Operation::Run { .. } => self.limits.limits().operation_timeout,
            Operation::Verify => self.limits.limits().validation_timeout,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Outcome {
    Run { observation: Observation },
    Verified { validation: Validation },
}

#[derive(Debug)]
pub(super) enum Dispatch {
    /// The operation returned, including confirmed failures after side effects.
    Completed(Result<Outcome, String>),
    /// No workspace operation was started by this dispatch.
    Rejected(String),
    /// Execution may have begun. The caller must pause rather than replay it.
    Unknown(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum ReplyResult {
    Completed {
        outcome: Result<Outcome, String>,
    },
    Rejected {
        reason: String,
    },
    /// The reservation or its durability could not be confirmed; the
    /// operation may or may not have run. The parent pauses rather than
    /// replaying it.
    Unknown {
        reason: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u32,
    job_id: String,
    sequence: u64,
    operation: OperationKind,
    result: ReplyResult,
}

impl Reply {
    fn new(request: &Request, result: ReplyResult) -> Self {
        Self {
            version: VERSION,
            job_id: request.job_id.clone(),
            sequence: request.sequence,
            operation: request.operation.kind(),
            result,
        }
    }
}

fn encode_frame(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let mut frame =
        serde_json::to_vec(value).map_err(|_| "engineering worker frame cannot be encoded")?;
    if frame.len() >= limit {
        return Err("engineering worker frame exceeds its byte limit".into());
    }
    frame.push(b'\n');
    Ok(frame)
}

fn read_frame(reader: &mut impl BufRead, limit: usize) -> Result<Vec<u8>, String> {
    let mut frame = Vec::new();
    reader
        .take(limit.saturating_add(1) as u64)
        .read_until(b'\n', &mut frame)
        .map_err(|_| "engineering worker request read failed")?;
    if frame.len() > limit || frame.last() != Some(&b'\n') {
        return Err("engineering worker request is incomplete or exceeds its byte limit".into());
    }
    Ok(frame)
}

fn decode_reply(bytes: &[u8], request: &Request) -> Result<Dispatch, String> {
    if bytes.len() > MAX_REPLY_BYTES
        || bytes.last() != Some(&b'\n')
        || bytes[..bytes.len() - 1].contains(&b'\n')
    {
        return Err("engineering worker reply is not one bounded frame".into());
    }
    let reply: Reply =
        serde_json::from_slice(bytes).map_err(|_| "engineering worker reply is malformed")?;
    if reply.version != VERSION
        || reply.job_id != request.job_id
        || reply.sequence != request.sequence
        || reply.operation != request.operation.kind()
    {
        return Err("engineering worker reply does not match the pending action".into());
    }
    match reply.result {
        ReplyResult::Completed { outcome } => {
            match &outcome {
                Ok(Outcome::Run { .. }) if request.operation.kind() == OperationKind::Run => {}
                Ok(Outcome::Verified { validation })
                    if request.operation.kind() == OperationKind::Verify =>
                {
                    // Old checkpoints may lack this identity, but a live
                    // reply must bind its evidence to this exact request.
                    let matches = validation.contract.as_ref().is_some_and(|contract| {
                        super::verification::valid(contract)
                            && contract.policy_digest
                                == super::verification::policy_digest(
                                    &request.policy,
                                    &request.limits.limits(),
                                )
                    });
                    if !matches {
                        return Err(
                            "engineering worker verification contract does not match the request"
                                .into(),
                        );
                    }
                }
                Err(error) if error.len() <= MAX_ERROR_BYTES => {}
                _ => return Err("engineering worker reply has an invalid outcome".into()),
            }
            Ok(Dispatch::Completed(outcome))
        }
        ReplyResult::Rejected { reason } if reason.len() <= MAX_ERROR_BYTES => {
            Ok(Dispatch::Rejected(reason))
        }
        ReplyResult::Rejected { .. } => {
            Err("engineering worker rejection exceeds its byte limit".into())
        }
        ReplyResult::Unknown { reason } if reason.len() <= MAX_ERROR_BYTES => {
            Ok(Dispatch::Unknown(reason))
        }
        ReplyResult::Unknown { .. } => {
            Err("engineering worker unknown result exceeds its byte limit".into())
        }
    }
}

/// Why the durable claim check refused execution.
enum ClaimCheck {
    /// The checkpoint could not be read or validated; durability of the
    /// claim itself is uncertain.
    Invalid(String),
    /// The checkpoint is sound but this exact claim is no longer current.
    Mismatch(String),
}

/// Read-only stale-claim barrier evaluated while holding the job lock. It
/// deliberately neither acquires the resident's lease nor recovers/writes
/// its checkpoint; the per-job lock plus the execution ledger close the
/// boundary between this check and execution start.
/// The bounded projection of exactly one committed claim. The worker
/// verifies currency from this file instead of the whole checkpoint, whose
/// unbounded job accumulation cannot reach the worker's read bound.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProjection {
    schema_version: u32,
    job_id: String,
    phase: Phase,
    workspace: String,
    candidate: Candidate,
    pending: PendingAction,
    execution_ledger_version: Option<u32>,
}

/// Read-only stale-claim barrier evaluated while holding the job lock. It
/// deliberately neither acquires the resident's lease nor recovers/writes
/// its checkpoint; the per-job lock plus the execution ledger close the
/// boundary between this check and execution start. The durable checkpoint
/// file itself must exist as a regular file — an absent store cannot own a
/// live claim — but it is never fully read here.
fn verify_claim(request: &Request, namespace: &execution::Namespace) -> Result<(), ClaimCheck> {
    let invalid = |error: &str| ClaimCheck::Invalid(error.to_owned());
    let mismatch = |error: &str| ClaimCheck::Mismatch(error.to_owned());
    let metadata = std::fs::symlink_metadata(&request.state_path)
        .map_err(|_| invalid("engineering worker checkpoint is unavailable"))?;
    if !metadata.file_type().is_file() {
        return Err(invalid(
            "engineering worker checkpoint is not a regular file",
        ));
    }
    let bytes = match execution::read_claim(namespace, &request.job_id) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(mismatch("engineering worker claim is not published")),
        Err(error) => return Err(invalid(&error)),
    };
    let claim: ClaimProjection = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("engineering worker claim projection is malformed"))?;
    if claim.schema_version != 1 || claim.job_id != request.job_id {
        return Err(invalid(
            "engineering worker claim projection identity is invalid",
        ));
    }
    // The parent publishes the marker with the pending claim in one
    // checkpoint transaction; a claim without the contract marker is a
    // durability defect, not an executable request.
    if claim.execution_ledger_version != Some(execution::LEDGER_MARKER_VERSION) {
        return Err(invalid(
            "engineering worker job lacks its execution ledger marker",
        ));
    }
    if claim.phase != Phase::Active
        || claim.workspace != request.policy.name
        || claim.candidate != request.candidate
        || claim.pending.sequence != request.sequence
        || claim.pending.action != request.claimed_action
    {
        return Err(mismatch(
            "engineering worker pending action is no longer current",
        ));
    }
    Ok(())
}

/// Worker admission, in order: request validation, job lock, ledger
/// identity/high-water, checkpoint claim match, cancel check, durable
/// reservation, then the workspace operation under the held lock.
fn execute(request: &Request, cancel: &AtomicBool) -> ReplyResult {
    let unknown = |reason: &str| ReplyResult::Unknown {
        reason: reason.to_owned(),
    };
    let rejected = |reason: &str| ReplyResult::Rejected {
        reason: reason.to_owned(),
    };
    if let Err(error) = request.validate() {
        return rejected(&error);
    }
    let namespace = match execution::namespace(&request.state_path) {
        Ok(namespace) => namespace,
        Err(error) => return unknown(&error),
    };
    // Workers never create the namespace or the lockfile; the parent's claim
    // initialization owns that.
    let _job_lock = match execution::try_lock(&namespace, &request.job_id, Mode::Existing) {
        Ok(lock) => lock,
        Err(AcquireError::Busy) => {
            return unknown("engineering job is already executing this claim");
        }
        Err(AcquireError::Unavailable(error)) => return unknown(&error),
    };
    let expected = execution::candidate_digest(&request.candidate);
    let ledger = match execution::read(&namespace, &request.job_id, &expected) {
        Ok(Some(ledger)) => ledger,
        Ok(None) => return unknown("engineering execution ledger is missing"),
        Err(error) => return unknown(&error),
    };
    // A sequence at or below the high-water mark is already reserved —
    // rejected unconditionally, whether or not this request's digest matches
    // the earlier attempt's.
    if request.sequence <= ledger.high_water {
        return unknown("engineering execution sequence is already reserved");
    }
    match verify_claim(request, &namespace) {
        Ok(()) => {}
        Err(ClaimCheck::Invalid(error)) => return unknown(&error),
        Err(ClaimCheck::Mismatch(error)) => return rejected(&error),
    }
    if cancel.load(Ordering::Acquire) {
        return rejected("engineering worker parent disconnected before execution");
    }
    let reservation = execution::Ledger {
        schema_version: execution::LEDGER_SCHEMA_VERSION,
        job_id: request.job_id.clone(),
        candidate_digest: expected,
        high_water: request.sequence,
        request_digest: Some(execution::request_digest(request)),
    };
    if let Err(error) = execution::publish(&namespace, &request.job_id, &reservation) {
        return unknown(&error);
    }
    let limits = request.limits.limits();
    // These functions own their deadlines inside this process. Returning an
    // error here does not imply that no command or git mutation took place.
    let outcome = match &request.operation {
        Operation::Run { argv } => workspace::execute(
            &request.policy,
            &request.candidate,
            &Action::Run { argv: argv.clone() },
            &limits,
            cancel,
        )
        .map(|observation| Outcome::Run { observation }),
        Operation::Verify => {
            workspace::verify(&request.policy, &request.candidate, &limits, cancel)
                .map(|validation| Outcome::Verified { validation })
        }
    }
    .map_err(|error| super::clipped(&error, MAX_ERROR_BYTES));
    ReplyResult::Completed { outcome }
}

/// In-process execution used by tests' `InlineRunner`. It performs the same
/// admission sequence as the spawned binary — request build, job lock,
/// ledger check, claim match, reservation, workspace operation — and only
/// skips the IPC transport.
#[cfg(test)]
pub(crate) fn execute_inline(request: &Request, cancel: &AtomicBool) -> Dispatch {
    match execute(request, cancel) {
        ReplyResult::Completed { outcome } => Dispatch::Completed(outcome),
        ReplyResult::Rejected { reason } => Dispatch::Rejected(reason),
        ReplyResult::Unknown { reason } => Dispatch::Unknown(reason),
    }
}

/// Hidden binary entry. No configuration, credentials, Shared, HTTP server,
/// model provider, or service discovery is initialized here.
pub(super) fn run_stdio() -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let input_cancel = Arc::clone(&cancel);
    let (sender, receiver) = mpsc::sync_channel(1);
    let _control = std::thread::Builder::new()
        .name("engineering-worker-control".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let request = read_frame(&mut input, MAX_REQUEST_BYTES).and_then(|bytes| {
                serde_json::from_slice::<Request>(&bytes)
                    .map_err(|_| "engineering worker request is malformed".to_owned())
            });
            let valid_frame = request.is_ok();
            if sender.send(request).is_err() || !valid_frame {
                return;
            }
            let mut extra = [0_u8; 1];
            loop {
                match input.read(&mut extra) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    // EOF and extra input both revoke this one-shot request.
                    _ => {
                        input_cancel.store(true, Ordering::Release);
                        return;
                    }
                }
            }
        })
        .map_err(|_| "engineering worker control monitor could not start")?;
    let request = receiver
        .recv_timeout(REQUEST_TIMEOUT)
        .map_err(|_| "engineering worker request receive deadline exceeded")??;
    let reply = Reply::new(&request, execute(&request, &cancel));
    let frame = encode_frame(&reply, MAX_REPLY_BYTES)?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output
        .write_all(&frame)
        .and_then(|()| output.flush())
        .map_err(|_| "engineering worker reply could not be written".into())
}

/// No fallback executes an operation in the resident. Any post-spawn protocol
/// uncertainty is reported separately from a known operation failure.
pub(super) fn dispatch(executable: &Path, request: &Request, cancel: &AtomicBool) -> Dispatch {
    if let Err(error) = request.validate() {
        return Dispatch::Rejected(error);
    }
    if cancel.load(Ordering::Acquire) {
        return Dispatch::Rejected("engineering worker cancelled before dispatch".into());
    }
    let frame = match encode_frame(request, MAX_REQUEST_BYTES) {
        Ok(frame) => frame,
        Err(error) => return Dispatch::Rejected(error),
    };
    #[cfg(unix)]
    {
        dispatch_unix(executable, request, &frame, cancel)
    }
    #[cfg(not(unix))]
    {
        let _ = (executable, frame);
        Dispatch::Rejected("engineering worker transport requires Unix".into())
    }
}

#[cfg(unix)]
fn dispatch_unix(
    executable: &Path,
    request: &Request,
    frame: &[u8],
    cancel: &AtomicBool,
) -> Dispatch {
    use std::io::ErrorKind;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    use std::time::Instant;

    use kamimusuhi_runtime::agent_exec::process::harness_command;

    let Some(program) = executable.to_str() else {
        return Dispatch::Rejected("engineering worker executable is not UTF-8".into());
    };
    if !executable.is_absolute() {
        return Dispatch::Rejected("engineering worker executable must be an absolute path".into());
    }
    let sockets = || -> std::io::Result<_> {
        let (input, child_input) = UnixStream::pair()?;
        let (output, child_output) = UnixStream::pair()?;
        input.set_nonblocking(true)?;
        output.set_nonblocking(true)?;
        Ok((input, child_input, output, child_output))
    };
    let (input, child_input, mut output, child_output) = match sockets() {
        Ok(sockets) => sockets,
        Err(_) => return Dispatch::Rejected("engineering worker channels could not open".into()),
    };
    let child = harness_command(program, &[ENTRY.into()], &[], &[])
        .stdin(Stdio::from(OwnedFd::from(child_input)))
        .stdout(Stdio::from(OwnedFd::from(child_output)))
        // Stderr never carries a model reply or an unbounded diagnostic log.
        .stderr(Stdio::null())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(_) => return Dispatch::Rejected("engineering worker could not start".into()),
    };
    let mut child = ChildGuard {
        child,
        input: Some(input),
        stopped_at: None,
        reaped: false,
        group_killed: false,
    };
    let started = Instant::now();
    let deadline = request.timeout().saturating_add(REQUEST_TIMEOUT);
    let mut written = 0;
    let mut response = Vec::new();
    let mut output_closed = false;
    let mut status = None;
    let mut failure: Option<&str> = None;
    let mut buffer = [0_u8; 8192];
    loop {
        if cancel.load(Ordering::Acquire) || started.elapsed() >= deadline {
            child.request_stop();
        }
        if let Some(input) = child.input.as_mut()
            && written < frame.len()
        {
            match input.write(&frame[written..]) {
                Ok(0) => {
                    failure = Some("engineering worker request channel closed");
                    child.request_stop();
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    failure = Some("engineering worker request could not be delivered");
                    child.request_stop();
                }
            }
        }
        // In particular, do NOT close stdin after the final request byte.
        if !output_closed {
            match output.read(&mut buffer) {
                Ok(0) => {
                    output_closed = true;
                    child.request_stop();
                }
                Ok(count) => {
                    if response.len().saturating_add(count) > MAX_REPLY_BYTES {
                        failure = Some("engineering worker reply exceeds its byte limit");
                        child.request_stop();
                        output_closed = true;
                    } else {
                        response.extend_from_slice(&buffer[..count]);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    failure = Some("engineering worker reply channel failed");
                    child.request_stop();
                    output_closed = true;
                }
            }
        }
        if status.is_none() {
            match child.child.try_wait() {
                Ok(Some(exit)) => {
                    child.reaped = true;
                    status = Some(exit);
                    child.request_stop();
                }
                Ok(None) => {}
                Err(_) => {
                    return Dispatch::Unknown(
                        "engineering worker exit could not be observed".into(),
                    );
                }
            }
        }
        if output_closed && let Some(exit) = status {
            if let Some(error) = failure {
                return Dispatch::Unknown(error.into());
            }
            if !exit.success() {
                return Dispatch::Unknown("engineering worker exited without confirmation".into());
            }
            return decode_reply(&response, request).unwrap_or_else(Dispatch::Unknown);
        }
        if child
            .stopped_at
            .is_some_and(|stopped| stopped.elapsed() >= CLEANUP_GRACE)
        {
            return Dispatch::Unknown(
                failure
                    .unwrap_or("engineering worker cleanup did not confirm an outcome")
                    .into(),
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
struct ChildGuard {
    child: std::process::Child,
    input: Option<std::os::unix::net::UnixStream>,
    stopped_at: Option<std::time::Instant>,
    reaped: bool,
    group_killed: bool,
}

#[cfg(unix)]
impl ChildGuard {
    fn request_stop(&mut self) {
        if self.stopped_at.is_none() {
            self.stopped_at = Some(std::time::Instant::now());
        }
        if let Some(input) = self.input.take() {
            let _ = input.shutdown(std::net::Shutdown::Write);
        }
    }

    fn kill_group(&mut self) {
        use std::process::{Command, Stdio};
        // Once reaped, this PID/PGID can belong to a later process. The
        // worker's own command group is separate and cannot be inferred here.
        if !self.reaped && !self.group_killed {
            self.group_killed = true;
            let _ = Command::new("/bin/kill")
                .env_clear()
                .args(["-KILL", "--", &format!("-{}", self.child.id())])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // This ordering also covers an early return or unwind. Give a live
        // worker time to cancel its separately grouped workspace command.
        self.request_stop();
        while !self.reaped
            && self
                .stopped_at
                .is_some_and(|stopped| stopped.elapsed() < CLEANUP_GRACE)
        {
            match self.child.try_wait() {
                Ok(Some(_)) => self.reaped = true,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                // Without a known owned child, do not signal a possibly
                // reused PID. EOF has already requested cooperative cleanup.
                Err(_) => return,
            }
        }
        self.kill_group();
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> Request {
        let argv = vec!["true".into()];
        Request {
            version: VERSION,
            state_path: PathBuf::from("/state/engineering.json"),
            job_id: "eng-1".into(),
            sequence: 7,
            policy: WorkspaceConfig {
                name: "project".into(),
                path: PathBuf::from("/workspace/project"),
                checks: vec![argv.clone()],
                commands: argv.clone(),
            },
            candidate: Candidate {
                path: PathBuf::from("/candidates/project/eng-1"),
                branch: "kamimusuhi/engineering/eng-1".into(),
                base_commit: "a".repeat(40),
            },
            limits: WireLimits {
                operation_timeout_ms: 1000,
                validation_timeout_ms: 1000,
                output_bytes: 4096,
            },
            claimed_action: Action::Run { argv: argv.clone() },
            operation: Operation::Run { argv },
        }
    }

    #[test]
    fn framed_input_requires_newline_and_a_bounded_complete_message() {
        assert_eq!(read_frame(&mut &b"{}\nextra"[..], 3).unwrap(), b"{}\n");
        assert!(read_frame(&mut &b"{}"[..], 3).is_err());
        assert!(read_frame(&mut &b"{}\n"[..], 2).is_err());
        assert!(encode_frame(&json!({}), 2).is_err());
        assert_eq!(encode_frame(&json!({}), 3).unwrap(), b"{}\n");
    }

    #[test]
    fn receipts_bind_identity_operation_and_confirmed_failure() {
        let request = request();
        let mut reply = Reply::new(
            &request,
            ReplyResult::Completed {
                outcome: Err("git validation failed after capture".into()),
            },
        );
        let decode =
            |reply: &Reply| decode_reply(&encode_frame(reply, MAX_REPLY_BYTES).unwrap(), &request);
        assert!(matches!(decode(&reply), Ok(Dispatch::Completed(Err(_)))));
        reply.sequence += 1;
        assert!(decode(&reply).is_err());
        reply.sequence -= 1;
        reply.operation = OperationKind::Verify;
        assert!(decode(&reply).is_err());
        reply.operation = OperationKind::Run;
        reply.result = ReplyResult::Completed {
            outcome: Ok(Outcome::Verified {
                validation: Validation {
                    passed: true,
                    head: "a".repeat(40),
                    checks: vec![],
                    contract: None,
                },
            }),
        };
        assert!(decode(&reply).is_err());
        assert!(decode_reply(&[], &request).is_err());
        assert!(decode_reply(b"{}\n{}\n", &request).is_err());
    }

    fn assert_live_contract(
        edit: impl Fn(&mut Option<workspace::ValidationContract>),
        accepted: bool,
    ) {
        for action in [
            Action::Verify,
            Action::Finish {
                summary: "candidate verification receipt".into(),
            },
        ] {
            for passed in [false, true] {
                let mut request = request();
                request.claimed_action = action.clone();
                request.operation = Operation::Verify;
                request.validate().unwrap();
                let mut contract = Some(workspace::ValidationContract {
                    schema_version: 1,
                    policy_digest: super::super::verification::policy_digest(
                        &request.policy,
                        &request.limits.limits(),
                    ),
                    completed_at: 1,
                });
                edit(&mut contract);
                let reply = Reply::new(
                    &request,
                    ReplyResult::Completed {
                        outcome: Ok(Outcome::Verified {
                            validation: Validation {
                                passed,
                                head: "a".repeat(40),
                                checks: vec![json!({
                                    "index":0, "program":"true", "passed":passed,
                                    "exit_code":if passed { 0 } else { 1 },
                                })],
                                contract,
                            },
                        }),
                    },
                );
                let decoded =
                    decode_reply(&encode_frame(&reply, MAX_REPLY_BYTES).unwrap(), &request);
                if accepted {
                    assert!(matches!(
                        decoded,
                        Ok(Dispatch::Completed(Ok(Outcome::Verified { validation })))
                            if validation.passed == passed
                    ));
                } else {
                    assert!(
                        decoded.is_err(),
                        "{action:?} passed={passed} accepted a mismatched contract: {decoded:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn live_verification_contract_matches_the_request() {
        assert_live_contract(|_| {}, true);
    }

    #[test]
    fn live_verification_contract_cannot_be_missing() {
        assert_live_contract(|contract| *contract = None, false);
    }

    #[test]
    fn live_verification_contract_cannot_use_another_policy_digest() {
        let mut other = request();
        other.policy.checks[0] = vec!["false".into()];
        let other_digest =
            super::super::verification::policy_digest(&other.policy, &other.limits.limits());
        assert_live_contract(
            |contract| {
                contract
                    .as_mut()
                    .unwrap()
                    .policy_digest
                    .clone_from(&other_digest)
            },
            false,
        );
    }

    #[test]
    fn live_verification_contract_rejects_a_malformed_digest() {
        assert_live_contract(
            |contract| {
                contract.as_mut().unwrap().policy_digest = format!("sha256:{}", "g".repeat(64));
            },
            false,
        );
    }

    #[test]
    fn live_verification_contract_rejects_an_unsupported_version() {
        assert_live_contract(
            |contract| contract.as_mut().unwrap().schema_version = 2,
            false,
        );
    }

    #[test]
    fn request_operation_must_match_the_full_claimed_action() {
        let mut request = request();
        assert!(request.validate().is_ok());
        request.claimed_action = Action::Run {
            argv: vec!["false".into()],
        };
        assert!(request.validate().is_err());
        request.claimed_action = Action::Finish {
            summary: "parent-owned summary".into(),
        };
        request.operation = Operation::Verify;
        assert!(request.validate().is_ok());
        request.limits.operation_timeout_ms = 0;
        assert!(request.validate().is_err());
    }
}
