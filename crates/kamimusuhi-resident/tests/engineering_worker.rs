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

use kamimusuhi_resident::engineering::{Candidate, WorkspaceConfig, workspace};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi");
const JOB: &str = "eng-worker-fixture";

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
        Self {
            state_path: dir.path().join("resident/current_state/engineering.json"),
            dir,
            policy,
            candidate,
        }
    }

    fn request(&self, sequence: u64, action: Value, operation_timeout_ms: u64) -> Value {
        let operation = match action["action"].as_str().unwrap() {
            "run" => json!({"kind":"run","argv":action["argv"]}),
            "verify" | "finish" => json!({"kind":"verify"}),
            _ => panic!("unsupported fixture action"),
        };
        let snapshot = json!({"schema_version":2,"jobs":[{
            "id":JOB,"objective":"Exercise the actual private worker",
            "workspace":"fixture","candidate":self.candidate,"phase":"active",
            "checkpoint":{"summary":"","next_step":""},
            "metrics":{"decisions":sequence,"actions":sequence-1,
                "verification_runs":0,"verification_failures":0,"repairs":0,
                "interruptions":0,"model_failures":0},
            "next_action_sequence":sequence+1,"validation":null,"repair_pending":false,
            "observations":[],"pending":{"sequence":sequence,"action":action,"started_at":1},
            "created_at":1,"updated_at":1,"next_attempt_at":0,"last_error":null,
            "request_id":null,"model_recovery":null
        }]});
        fs::write(&self.state_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        json!({"version":1,"state_path":self.state_path,"job_id":JOB,"sequence":sequence,
            "policy":self.policy,"candidate":self.candidate,"claimed_action":action,
            "limits":{"operation_timeout_ms":operation_timeout_ms,
                "validation_timeout_ms":5000,"output_bytes":32768},"operation":operation})
    }

    fn spawn(&self, request: &Value) -> Worker {
        let sequence = request["sequence"].as_u64().unwrap();
        let stdout = self.dir.path().join(format!("worker-{sequence}.out"));
        let stderr = self.dir.path().join(format!("worker-{sequence}.err"));
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
    assert_eq!(reply["version"], 1);
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
