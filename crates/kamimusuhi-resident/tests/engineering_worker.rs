//! The private engineering worker through the actual resident executable.
//! No resident Config, token, Shared or inline action runner is constructed.

#![cfg(unix)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_core::digest::json_digest;
use kamimusuhi_resident::engineering::{Candidate, WorkspaceConfig, workspace};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi");
const JOB: &str = "eng-worker-fixture";

/// Must match `engineering::execution`'s candidate identity digest.
fn candidate_digest(candidate: &Candidate) -> String {
    json_digest(&json!({"schema_version":1,"candidate":candidate}))
}

fn request_digest(request: &Value) -> String {
    json_digest(&json!({"schema_version":1,"request":request}))
}

fn wait(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll owned child") {
            return status;
        }
        assert!(Instant::now() < deadline, "owned child exceeded deadline");
        thread::sleep(Duration::from_millis(10));
    }
}

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Worker {
    fn reply(&mut self) -> Value {
        let status = wait(&mut self.child, Duration::from_secs(5));
        assert!(
            status.success(),
            "worker failed: {}",
            fs::read_to_string(&self.stderr).unwrap_or_default()
        );
        let bytes = fs::read(&self.stdout).unwrap();
        assert!(bytes.len() <= 128 * 1024, "worker receipt is bounded");
        serde_json::from_slice(&bytes).expect("worker emits exactly one JSON receipt")
    }

    fn owner_disappears(&mut self) {
        drop(self.input.take());
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the owned pipe lets the real worker cancel and reap its
        // direct command first. A broken worker still has bounded cleanup.
        self.owner_disappears();
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = Command::new("/bin/kill")
                .env_clear()
                .args(["-KILL", "--", &format!("-{}", self.child.id())])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    policy: WorkspaceConfig,
    candidate: Candidate,
    state_path: PathBuf,
    attempts: AtomicU64,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in ["checkout", "home", "tmp", "resident/current_state"] {
            fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        let checkout = dir.path().join("checkout");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", dir.path().join("home"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .current_dir(&checkout)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "fixture git failed");
        };
        git(&["init", "-q"]);
        fs::write(checkout.join("code.txt"), "initial\n").unwrap();
        git(&["add", "code.txt"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@local",
            "commit",
            "-qm",
            "initial fixture",
        ]);
        let policy = WorkspaceConfig {
            name: "fixture".into(),
            path: checkout,
            checks: vec![vec![
                "/bin/sh".into(),
                "-c".into(),
                "test \"$(cat code.txt)\" = fixed".into(),
            ]],
            commands: vec![
                "/bin/sh".into(),
                std::env::current_exe().unwrap().to_str().unwrap().into(),
            ],
        };
        let candidate = workspace::plan(
            &policy,
            &dir.path().join("resident/engineering-worktrees"),
            JOB,
        )
        .unwrap();
        workspace::prepare(&policy, &candidate).unwrap();
        let fixture = Self {
            state_path: dir.path().join("resident/current_state/engineering.json"),
            dir,
            policy,
            candidate,
            attempts: AtomicU64::new(0),
        };
        fixture.add_ledger(JOB, &fixture.candidate);
        fixture
    }

    /// Directory every execution-dedup file for this checkpoint lives in.
    fn executions(&self) -> PathBuf {
        self.state_path
            .parent()
            .unwrap()
            .join("engineering.json.executions")
    }

    /// Create the lockfile and genesis ledger a parent-side claim would have
    /// published for `job_id`. The ledger persists across `request` calls.
    fn add_ledger(&self, job_id: &str, candidate: &Candidate) {
        let executions = self.executions();
        fs::create_dir_all(&executions).unwrap();
        let lock = executions.join(format!("{job_id}.lock"));
        if !lock.exists() {
            fs::write(&lock, b"").unwrap();
        }
        fs::write(
            executions.join(format!("{job_id}.json")),
            serde_json::to_vec(&json!({
                "schema_version":1,"job_id":job_id,
                "candidate_digest":candidate_digest(candidate),
                "high_water":0,"request_digest":null}))
            .unwrap(),
        )
        .unwrap();
    }

    /// Prepare a second job's worktree, lockfile and genesis ledger.
    fn add_job(&self, job_id: &str) -> Candidate {
        let candidate = workspace::plan(
            &self.policy,
            &self.dir.path().join("resident/engineering-worktrees"),
            job_id,
        )
        .unwrap();
        workspace::prepare(&self.policy, &candidate).unwrap();
        self.add_ledger(job_id, &candidate);
        candidate
    }

    fn job_entry(
        &self,
        job_id: &str,
        candidate: &Candidate,
        sequence: u64,
        action: &Value,
        marker: bool,
    ) -> Value {
        json!({
            "id":job_id,"objective":"Exercise the actual private worker",
            "workspace":"fixture","candidate":candidate,"phase":"active",
            "checkpoint":{"summary":"","next_step":""},
            "metrics":{"decisions":sequence,"actions":sequence-1,
                "verification_runs":0,"verification_failures":0,"repairs":0,
                "interruptions":0,"model_failures":0},
            "next_action_sequence":sequence+1,"validation":null,"repair_pending":false,
            "observations":[],"pending":{"sequence":sequence,"action":action,"started_at":1},
            "execution_ledger_version":if marker { Value::from(1) } else { Value::Null },
            "created_at":1,"updated_at":1,"next_attempt_at":0,"last_error":null,
            "request_id":null,"model_recovery":null})
    }

    fn write_snapshot(&self, jobs: Vec<Value>) {
        let snapshot = json!({"schema_version":2,"jobs":jobs});
        fs::write(&self.state_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    }

    /// The wire frame a resident would dispatch — no checkpoint write.
    fn request_json(
        &self,
        job_id: &str,
        candidate: &Candidate,
        sequence: u64,
        action: &Value,
        operation_timeout_ms: u64,
    ) -> Value {
        let operation = match action["action"].as_str().unwrap() {
            "run" => json!({"kind":"run","argv":action["argv"]}),
            "verify" | "finish" => json!({"kind":"verify"}),
            _ => panic!("unsupported fixture action"),
        };
        json!({"version":2,"state_path":self.state_path,"job_id":job_id,"sequence":sequence,
            "policy":self.policy,"candidate":candidate,"claimed_action":action,
            "limits":{"operation_timeout_ms":operation_timeout_ms,
                "validation_timeout_ms":5000,"output_bytes":32768},"operation":operation})
    }

    /// Write a one-job snapshot carrying the durable claim, then return the
    /// matching request frame. The ledger is never rewritten here.
    fn request(&self, sequence: u64, action: Value, operation_timeout_ms: u64) -> Value {
        self.request_for(JOB, &self.candidate, sequence, action, operation_timeout_ms)
    }

    fn request_for(
        &self,
        job_id: &str,
        candidate: &Candidate,
        sequence: u64,
        action: Value,
        operation_timeout_ms: u64,
    ) -> Value {
        self.write_snapshot(vec![
            self.job_entry(job_id, candidate, sequence, &action, true),
        ]);
        self.write_claim(job_id, candidate, sequence, &action, true);
        self.request_json(job_id, candidate, sequence, &action, operation_timeout_ms)
    }

    /// The bounded per-claim projection the parent publishes with the
    /// pending commit — what the worker verifies instead of the checkpoint.
    fn write_claim(
        &self,
        job_id: &str,
        candidate: &Candidate,
        sequence: u64,
        action: &Value,
        marker: bool,
    ) {
        fs::write(
            self.executions().join(format!("{job_id}.claim.json")),
            serde_json::to_vec(&json!({
                "schema_version":1,"job_id":job_id,"phase":"active",
                "workspace":"fixture","candidate":candidate,
                "pending":{"sequence":sequence,"action":action,"started_at":1},
                "execution_ledger_version":if marker { Value::from(1) } else { Value::Null }}))
            .unwrap(),
        )
        .unwrap();
    }

    fn ledger(&self, job_id: &str) -> Value {
        serde_json::from_slice(&fs::read(self.executions().join(format!("{job_id}.json"))).unwrap())
            .unwrap()
    }

    fn spawn(&self, request: &Value) -> Worker {
        let sequence = request["sequence"].as_u64().unwrap();
        let job = request["job_id"].as_str().unwrap();
        let attempt = self.attempts.fetch_add(1, AtomicOrdering::Relaxed);
        let stdout = self
            .dir
            .path()
            .join(format!("worker-{job}-{sequence}-{attempt}.out"));
        let stderr = self
            .dir
            .path()
            .join(format!("worker-{job}-{sequence}-{attempt}.err"));
        let mut command = Command::new(BINARY);
        command
            .arg("engineering-worker")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir.path().join("home"))
            .env("TMPDIR", self.dir.path().join("tmp"))
            .env("LANG", "C")
            .env(
                "KAMIMUSUHI_CONFIG",
                self.dir.path().join("absent-config.json"),
            )
            .current_dir(self.dir.path())
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap());
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let mut worker = Worker {
            child,
            input,
            stdout,
            stderr,
        };
        let input = worker.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, request).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        worker
    }

    fn blocking_request(&self, barrier: &Barrier, timeout_ms: u64) -> Value {
        fs::write(
            self.dir.path().join("tmp/worker-run-fixture.json"),
            serde_json::to_vec(&json!({
                "candidate":self.candidate.path,"counter":self.dir.path().join("runs.txt"),
                "released":self.dir.path().join("released.txt"),
                "address":barrier.listener.local_addr().unwrap().to_string(),
            }))
            .unwrap(),
        )
        .unwrap();
        self.request(
            1,
            json!({"action":"run","argv":[std::env::current_exe().unwrap(),
                "--ignored","--exact","worker_blocking_run_helper",
                "--nocapture","--test-threads=1"]}),
            timeout_ms,
        )
    }
}

struct Barrier {
    listener: TcpListener,
    stream: Option<TcpStream>,
}

impl Barrier {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            listener,
            stream: None,
        }
    }

    fn started(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.listener.accept() {
                Ok((stream, address)) => {
                    assert!(address.ip().is_loopback());
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    self.stream = Some(stream);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "worker command never started");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        }
        let mut started = [0; 8];
        self.stream
            .as_mut()
            .unwrap()
            .read_exact(&mut started)
            .unwrap();
        assert_eq!(&started, b"started\n");
    }

    fn stopped(&mut self) {
        let mut extra = [0; 1];
        assert_eq!(
            self.stream.as_mut().unwrap().read(&mut extra).unwrap(),
            0,
            "command closes within 5s without fixture release or its 20s timeout"
        );
        self.stream = None;
    }
}

impl Drop for Barrier {
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.write_all(b"release\n");
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[test]
#[ignore = "fixture child, invoked explicitly by the real engineering worker"]
fn worker_blocking_run_helper() {
    let configuration: Value = serde_json::from_slice(
        &fs::read(std::env::temp_dir().join("worker-run-fixture.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::env::current_dir().unwrap(),
        Path::new(configuration["candidate"].as_str().unwrap())
    );
    let mut counter = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(configuration["counter"].as_str().unwrap())
        .unwrap();
    counter.write_all(b"run\n").unwrap();
    counter.sync_all().unwrap();
    let address: std::net::SocketAddr = configuration["address"].as_str().unwrap().parse().unwrap();
    assert!(address.ip().is_loopback());
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream.write_all(b"started\n").unwrap();
    let mut release = [0; 8];
    stream
        .read_exact(&mut release)
        .expect("bounded fixture release");
    assert_eq!(&release, b"release\n");
    fs::write(configuration["released"].as_str().unwrap(), "released\n").unwrap();
    stream.write_all(b"done\n").unwrap();
}

fn identity(reply: &Value, sequence: u64, operation: &str) {
    assert_eq!(reply["version"], 2);
    assert_eq!(reply["job_id"], JOB);
    assert_eq!(reply["sequence"], sequence);
    assert_eq!(reply["operation"], operation);
}

#[test]
fn worker_executes_run_and_verifies_the_actual_candidate_without_resident_configuration() {
    let fixture = Fixture::new();
    let request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c",
            "printf 'fixed\\n' > code.txt; printf 'run-receipt\\n'"]}),
        5000,
    );
    let claimed = fs::read(&fixture.state_path).unwrap();
    let reply = fixture.spawn(&request).reply();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "completed");
    let outcome = &reply["result"]["outcome"]["Ok"];
    assert_eq!(outcome["kind"], "run");
    assert_eq!(outcome["observation"]["ok"], true);
    assert_eq!(outcome["observation"]["detail"]["stdout"], "run-receipt\n");
    assert_eq!(fs::read(&fixture.state_path).unwrap(), claimed);

    let request = fixture.request(
        2,
        json!({"action":"finish","summary":"verified fixture"}),
        5000,
    );
    let claimed = fs::read(&fixture.state_path).unwrap();
    let reply = fixture.spawn(&request).reply();
    identity(&reply, 2, "verify");
    assert_eq!(reply["result"]["status"], "completed");
    let outcome = &reply["result"]["outcome"]["Ok"];
    assert_eq!(outcome["kind"], "verified");
    assert_eq!(outcome["validation"]["passed"], true);
    assert_eq!(outcome["validation"]["checks"][0]["exit_code"], 0);
    assert_ne!(outcome["validation"]["head"], fixture.candidate.base_commit);
    assert_eq!(
        outcome["validation"]["head"],
        workspace::revision(&fixture.candidate).unwrap()
    );
    assert!(workspace::clean(&fixture.candidate).unwrap());
    assert_eq!(fs::read(&fixture.state_path).unwrap(), claimed);
    assert_eq!(
        fs::read_to_string(fixture.policy.path.join("code.txt")).unwrap(),
        "initial\n"
    );
    assert!(!fixture.dir.path().join("resident/spool").exists());
    assert!(
        !fixture
            .dir
            .path()
            .join("resident/current_state/boot_epoch")
            .exists()
    );
}

#[test]
fn owner_pipe_eof_cancels_the_running_command_and_returns_a_confirmed_receipt() {
    let fixture = Fixture::new();
    let mut barrier = Barrier::new();
    let request = fixture.blocking_request(&barrier, 30_000);
    let claimed = fs::read(&fixture.state_path).unwrap();
    let mut worker = fixture.spawn(&request);
    barrier.started();
    let cancellation_started = Instant::now();
    worker.owner_disappears();
    barrier.stopped();
    let owner_eof_to_command_eof_ms = cancellation_started.elapsed().as_millis();
    let reply = worker.reply();
    let owner_eof_to_receipt_ms = cancellation_started.elapsed().as_millis();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "completed");
    let observation = &reply["result"]["outcome"]["Ok"]["observation"];
    assert_eq!(observation["ok"], false);
    assert_eq!(observation["detail"]["end"], "cancelled");
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("runs.txt")).unwrap(),
        "run\n"
    );
    assert!(!fixture.dir.path().join("released.txt").exists());
    assert_eq!(fs::read(&fixture.state_path).unwrap(), claimed);
    eprintln!(
        "worker EOF cancellation: owner_eof_to_command_eof_ms={owner_eof_to_command_eof_ms} owner_eof_to_receipt_ms={owner_eof_to_receipt_ms}"
    );
}

#[test]
fn worker_timeout_stops_the_command_while_its_owner_pipe_is_still_open() {
    let fixture = Fixture::new();
    let mut barrier = Barrier::new();
    let request = fixture.blocking_request(&barrier, 1000);
    let claimed = fs::read(&fixture.state_path).unwrap();
    let worker_started = Instant::now();
    let mut worker = fixture.spawn(&request);
    barrier.started();
    barrier.stopped();
    let spawn_to_command_eof_ms = worker_started.elapsed().as_millis();
    assert!(worker.input.is_some(), "owner did not cancel the worker");
    let reply = worker.reply();
    let spawn_to_receipt_ms = worker_started.elapsed().as_millis();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "completed");
    let observation = &reply["result"]["outcome"]["Ok"]["observation"];
    assert_eq!(observation["ok"], false);
    assert_eq!(observation["detail"]["end"], "timed_out");
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("runs.txt")).unwrap(),
        "run\n"
    );
    assert!(!fixture.dir.path().join("released.txt").exists());
    assert_eq!(fs::read(&fixture.state_path).unwrap(), claimed);
    eprintln!(
        "worker timeout: timeout_ms=1000 spawn_to_command_eof_ms={spawn_to_command_eof_ms} spawn_to_receipt_ms={spawn_to_receipt_ms} command_duration_ms={}",
        observation["detail"]["duration_ms"]
    );
}

#[test]
fn worker_rejects_a_request_that_does_not_match_the_durable_action_claim() {
    let fixture = Fixture::new();
    let mut request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf changed > code.txt"]}),
        5000,
    );
    // The request is internally consistent, but differs from the command
    // the resident durably claimed. Neither variant may execute.
    request["claimed_action"]["argv"][2] = json!("printf unclaimed > code.txt");
    request["operation"]["argv"][2] = json!("printf unclaimed > code.txt");
    let claimed = fs::read(&fixture.state_path).unwrap();
    let reply = fixture.spawn(&request).reply();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "rejected");
    assert!(!reply["result"]["reason"].as_str().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("code.txt")).unwrap(),
        "initial\n"
    );
    assert_eq!(
        workspace::revision(&fixture.candidate).unwrap(),
        fixture.candidate.base_commit
    );
    assert_eq!(fs::read(&fixture.state_path).unwrap(), claimed);
}

/// The same claim given to two live workers at once: the contender is
/// refused (busy or already reserved), the side effect is measured once by
/// an external counter, and resending after completion is refused too.
#[test]
fn a_claim_dispatched_twice_executes_at_most_once_concurrently_and_after_completion() {
    let fixture = Fixture::new();
    let mut barrier = Barrier::new();
    let request = fixture.blocking_request(&barrier, 30_000);
    let mut first = fixture.spawn(&request);
    barrier.started();
    let ledger = fixture.ledger(JOB);
    assert_eq!(ledger["high_water"], 1, "reservation precedes execution");
    assert!(
        ledger["request_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{ledger}"
    );

    // A second worker holding the identical request arrives mid-execution.
    let mut contender = fixture.spawn(&request);
    let reply = contender.reply();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(!reply["result"]["reason"].as_str().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("runs.txt")).unwrap(),
        "run\n",
        "the held claim executed exactly once"
    );

    // Release the first worker; it confirms one completed run.
    barrier
        .stream
        .as_mut()
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    let reply = first.reply();
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    assert_eq!(fixture.ledger(JOB)["high_water"], 1);

    // Resend the finished claim: the ledger refuses it regardless of digest.
    let mut resent = fixture.spawn(&request);
    let reply = resent.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("runs.txt")).unwrap(),
        "run\n"
    );
}

/// A different argument list under an already-reserved sequence is refused
/// by the ledger before the claim comparison is even reached.
#[test]
fn the_same_sequence_with_different_arguments_never_executes_again() {
    let fixture = Fixture::new();
    let request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'first\\n' >> side-effects.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("side-effects.txt")).unwrap(),
        "first\n"
    );

    // The resident's checkpoint still claims the same sequence with new
    // arguments; digest differs, but high-water already refuses it.
    let replayed = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'second\\n' >> side-effects.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&replayed);
    let reply = worker.reply();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("side-effects.txt")).unwrap(),
        "first\n"
    );
}

/// Gaps in the durable sequence are normal — non-worker actions also
/// consume cursor values — so a higher sequence executes normally.
#[test]
fn a_sequence_above_the_high_water_executes_normally() {
    let fixture = Fixture::new();
    let request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'a\\n' >> gap.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&request);
    assert_eq!(worker.reply()["result"]["status"], "completed");

    let request = fixture.request(
        4,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'b\\n' >> gap.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    identity(&reply, 4, "run");
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    assert_eq!(fixture.ledger(JOB)["high_water"], 4);
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("gap.txt")).unwrap(),
        "a\nb\n"
    );
}

/// A durable reservation whose execution never started — a worker killed
/// between publish and the first side effect — blocks any reissue without
/// ever double-counting.
#[test]
fn a_reservation_without_an_effect_blocks_reissue_and_counts_zero() {
    let fixture = Fixture::new();
    let request = fixture.request_json(
        JOB,
        &fixture.candidate,
        1,
        &json!({"action":"run","argv":["/bin/sh","-c","printf 'x\\n' >> counter.txt"]}),
        5000,
    );
    // Simulate the post-crash state: reservation persisted, no side effect.
    fs::write(
        fixture.executions().join(format!("{JOB}.json")),
        serde_json::to_vec(&json!({
            "schema_version":1,"job_id":JOB,
            "candidate_digest":candidate_digest(&fixture.candidate),
            "high_water":1,"request_digest":request_digest(&request)}))
        .unwrap(),
    )
    .unwrap();
    fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'x\\n' >> counter.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(
        !fixture.candidate.path.join("counter.txt").exists(),
        "the reserved-but-unexecuted attempt is never replayed"
    );
}

/// A missing or corrupt ledger stops the worker instead of being repaired
/// into a fresh empty record.
#[test]
fn a_missing_or_corrupt_ledger_stops_the_worker_without_executing() {
    let fixture = Fixture::new();
    let ledger_path = fixture.executions().join(format!("{JOB}.json"));

    fs::remove_file(&ledger_path).unwrap();
    let request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'x\\n' >> never.txt"]}),
        5000,
    );
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(
        !ledger_path.exists(),
        "a vanished ledger is not recreated by the worker"
    );
    assert!(!fixture.candidate.path.join("never.txt").exists());

    fs::write(&ledger_path, b"not a ledger").unwrap();
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(!fixture.candidate.path.join("never.txt").exists());
    assert_eq!(fs::read(&ledger_path).unwrap(), b"not a ledger");
}

/// A claim without the contract marker is a durability defect, not an
/// executable request — the worker refuses it even with a genesis ledger.
#[test]
fn a_claim_without_the_ledger_marker_is_refused() {
    let fixture = Fixture::new();
    let action = json!({"action":"run","argv":["/bin/sh","-c","printf 'x\\n' >> never.txt"]});
    fixture.write_snapshot(vec![fixture.job_entry(
        JOB,
        &fixture.candidate,
        1,
        &action,
        false,
    )]);
    fixture.write_claim(JOB, &fixture.candidate, 1, &action, false);
    let request = fixture.request_json(JOB, &fixture.candidate, 1, &action, 5000);
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(!fixture.candidate.path.join("never.txt").exists());
    assert_eq!(fixture.ledger(JOB)["high_water"], 0);
}

/// Two jobs hold disjoint locks and ledgers under one checkpoint; both
/// execute exactly once even when dispatched concurrently.
#[test]
fn independent_jobs_execute_independently_under_one_checkpoint() {
    let fixture = Fixture::new();
    let job_b = "eng-worker-fixture-b";
    let candidate_b = fixture.add_job(job_b);
    let action_a = json!({"action":"run","argv":["/bin/sh","-c","printf 'a\\n' >> out.txt"]});
    let action_b = json!({"action":"run","argv":["/bin/sh","-c","printf 'b\\n' >> out.txt"]});
    fixture.write_snapshot(vec![
        fixture.job_entry(JOB, &fixture.candidate, 1, &action_a, true),
        fixture.job_entry(job_b, &candidate_b, 1, &action_b, true),
    ]);
    fixture.write_claim(JOB, &fixture.candidate, 1, &action_a, true);
    fixture.write_claim(job_b, &candidate_b, 1, &action_b, true);
    let request_a = fixture.request_json(JOB, &fixture.candidate, 1, &action_a, 5000);
    let request_b = fixture.request_json(job_b, &candidate_b, 1, &action_b, 5000);
    let mut worker_a = fixture.spawn(&request_a);
    let mut worker_b = fixture.spawn(&request_b);
    let reply_a = worker_a.reply();
    let reply_b = worker_b.reply();
    assert_eq!(reply_a["result"]["status"], "completed", "{reply_a}");
    assert_eq!(reply_b["result"]["status"], "completed", "{reply_b}");
    assert_eq!(fixture.ledger(JOB)["high_water"], 1);
    assert_eq!(fixture.ledger(job_b)["high_water"], 1);
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("out.txt")).unwrap(),
        "a\n"
    );
    assert_eq!(
        fs::read_to_string(candidate_b.path.join("out.txt")).unwrap(),
        "b\n"
    );
}

/// State growth must not strand workers: a checkpoint grown past the worker's
/// former 64MiB read bound is still admitted because the worker verifies the
/// bounded per-claim projection, never the whole snapshot.
#[test]
fn a_checkpoint_grown_past_the_former_limit_still_executes() {
    let fixture = Fixture::new();
    let request = fixture.request(
        1,
        json!({"action":"run","argv":["/bin/sh","-c","printf 'x\\n' >> grown.txt"]}),
        5000,
    );
    // Grow the checkpoint beyond 64MiB with inert whitespace; the JSON is
    // still valid, but no bounded reader may ever parse it here.
    let mut grown = fs::OpenOptions::new()
        .append(true)
        .open(&fixture.state_path)
        .unwrap();
    grown.write_all(&vec![b' '; 65 * 1024 * 1024 + 1]).unwrap();
    grown.sync_all().unwrap();
    drop(grown);
    assert!(
        fs::metadata(&fixture.state_path).unwrap().len() > 64 * 1024 * 1024,
        "fixture checkpoint exceeds the former worker bound"
    );
    let mut worker = fixture.spawn(&request);
    let reply = worker.reply();
    identity(&reply, 1, "run");
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("grown.txt")).unwrap(),
        "x\n",
        "the run's side effect landed exactly once"
    );
    assert_eq!(fixture.ledger(JOB)["high_water"], 1);
}

/// A resident resume mid-execution publishes a newer claim; a worker for it
/// defers (the job lock is busy) rather than racing, and the re-dispatched
/// claim runs exactly once — while a stale resend of the finished claim is
/// refused by the ledger's high-water.
#[test]
fn a_running_worker_serializes_the_resumed_claim() {
    let fixture = Fixture::new();
    let mut barrier = Barrier::new();
    let request_one = fixture.blocking_request(&barrier, 30_000);
    let mut first = fixture.spawn(&request_one);
    barrier.started();

    // The resident resumes while the first claim executes: the durable claim
    // advances to sequence 2 and its projection is republished.
    let action_two =
        json!({"action":"run","argv":["/bin/sh","-c","printf 'r2\\n' >> resumed.txt"]});
    fixture.write_snapshot(vec![fixture.job_entry(
        JOB,
        &fixture.candidate,
        2,
        &action_two,
        true,
    )]);
    fixture.write_claim(JOB, &fixture.candidate, 2, &action_two, true);
    let request_two = fixture.request_json(JOB, &fixture.candidate, 2, &action_two, 5000);

    // The deferred worker refuses to race: busy lock, zero new side effects.
    let mut contender = fixture.spawn(&request_two);
    let reply = contender.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert!(!fixture.candidate.path.join("resumed.txt").exists());

    // Finish claim 1; the re-dispatched claim 2 executes exactly once.
    barrier
        .stream
        .as_mut()
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    let reply = first.reply();
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    let mut second = fixture.spawn(&request_two);
    let reply = second.reply();
    identity(&reply, 2, "run");
    assert_eq!(reply["result"]["status"], "completed", "{reply}");
    assert_eq!(fixture.ledger(JOB)["high_water"], 2);
    assert_eq!(
        fs::read_to_string(fixture.candidate.path.join("resumed.txt")).unwrap(),
        "r2\n"
    );

    // The finished claim 1 can never execute again: high_water refuses it.
    let mut stale = fixture.spawn(&request_one);
    let reply = stale.reply();
    assert_eq!(reply["result"]["status"], "unknown", "{reply}");
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("runs.txt")).unwrap(),
        "run\n",
        "claim 1's side effect happened exactly once across resume"
    );
}
