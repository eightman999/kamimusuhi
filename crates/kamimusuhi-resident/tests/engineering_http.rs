//! Native engineering through the real CLI, resident HTTP server and router.
//! Only the model is scripted; git worktrees, edits, checks and restart are real.

#![cfg(unix)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi");
const OBJECTIVE: &str = "Fix code.txt, verify the candidate, and preserve the operator checkout";
const MODEL: &str = "fixture-native-engineer";
const LEASE_ERROR: &str = "engineering state is already owned by another resident";

struct ChildGuard(Child);

impl ChildGuard {
    fn spawn(mut command: Command) -> Self {
        command.process_group(0);
        Self(command.spawn().expect("spawn isolated fixture process"))
    }

    fn wait(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().expect("poll fixture process") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "fixture process exceeded deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Every process we start has its own process group. Do not inspect
        // system processes or signal anything identified through a pidfile.
        let _ = Command::new("/bin/kill")
            .env_clear()
            .args(["-KILL", "--", &format!("-{}", self.0.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    counter: PathBuf,
    command_id: AtomicUsize,
}

impl Fixture {
    fn new(model_url: &str) -> Self {
        let dir = tempfile::tempdir().expect("isolated fixture directory");
        let workspace = dir.path().join("checkout");
        for name in ["checkout", "home", "tmp"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        let fixture = Self {
            config: dir.path().join("resident.json"),
            counter: dir.path().join("check-receipts.tsv"),
            command_id: AtomicUsize::new(0),
            workspace,
            dir,
        };
        fixture.git(&fixture.workspace, &["init", "-q", "-b", "main"]);
        fs::write(fixture.workspace.join("code.txt"), "initial\n").unwrap();
        fixture.git(&fixture.workspace, &["add", "code.txt"]);
        fixture.git(
            &fixture.workspace,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@local",
                "commit",
                "-qm",
                "initial fixture",
            ],
        );
        let validator = fixture.dir.path().join("validate.sh");
        fs::write(
            &validator,
            "#!/bin/sh\nset -eu\nIFS= read -r value < code.txt\n\
             printf '%s\\t%s\\n' \"$PWD\" \"$value\" >> \"$1\"\n\
             test \"$value\" = fixed\n",
        )
        .unwrap();
        let config = json!({
            "node":{"id":"engineering-http-test","role":"continuity",
                    "listen":"127.0.0.1:1"},
            "paths":{"root":fixture.dir.path().join("resident")},
            "tiers":[{"name":"fixture","base_url":model_url,"model":MODEL,
                      "billing":"local","node_local":true,"timeout_secs":3,
                      "probe_interval_secs":3600}],
            "peers":[],"jobs":[],"mcp_servers":[],
            "commitments":{"enabled":false},
            "intercom":{"enabled":false},
            "delegation":{"enabled":false},
            "engineering":{
                "model":"auto","tick_secs":1,
                "operation_timeout_secs":5,"validation_timeout_secs":5,
                "workspaces":[{"name":"fixture","path":fixture.workspace,
                    "checks":[["/bin/sh",validator,fixture.counter]],"commands":[]}]
            }
        });
        fs::write(&fixture.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir.path().join("home"))
            .env("TMPDIR", self.dir.path().join("tmp"))
            .env("LANG", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(self.dir.path())
            .stdin(Stdio::null());
        command
    }

    fn complete(&self, mut command: Command) -> String {
        let id = self.command_id.fetch_add(1, Ordering::Relaxed);
        let stdout = self.dir.path().join(format!("command-{id}.out"));
        let stderr = self.dir.path().join(format!("command-{id}.err"));
        command
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap());
        let mut child = ChildGuard::spawn(command);
        let status = child.wait(Duration::from_secs(10));
        assert!(
            status.success(),
            "fixture command failed: {}",
            fs::read_to_string(stderr).unwrap_or_default()
        );
        fs::read_to_string(stdout).expect("fixture output is UTF-8")
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let mut command = self.command("git");
        command
            .current_dir(cwd)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args);
        self.complete(command).trim().to_owned()
    }

    fn cli(&self, url: &str, args: &[&str]) -> Value {
        let mut command = self.command(BINARY);
        command
            .args(["engineer", "--url", url, "--config"])
            .arg(&self.config)
            .args(args);
        serde_json::from_str(&self.complete(command)).expect("CLI emits JSON")
    }

    fn start(&self) -> Resident {
        self.start_as(MODEL, 3)
    }

    fn start_as(&self, model: &str, model_timeout_secs: u64) -> Resident {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve test port");
        let address = listener.local_addr().unwrap();
        let mut config: Value = serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap();
        config["node"]["listen"] = json!(address.to_string());
        config["tiers"][0]["model"] = json!(model);
        config["tiers"][0]["timeout_secs"] = json!(model_timeout_secs);
        // Concurrent daemons share only their state root, not a mutable
        // configuration file or listening port.
        let config_path = self
            .dir
            .path()
            .join(format!("resident-{}.json", address.port()));
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let log = self
            .dir
            .path()
            .join(format!("resident-{}.stderr", address.port()));
        let mut command = self.command(BINARY);
        command
            .args(["serve", "--config"])
            .arg(config_path)
            .stdout(Stdio::null())
            .stderr(File::create(&log).unwrap());
        drop(listener);
        let mut resident = Resident {
            child: ChildGuard::spawn(command),
            url: format!("http://{address}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if request(&resident.url, "GET", "/v1/engineering", None).is_ok() {
                return resident;
            }
            assert!(
                resident.child.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "resident did not start: {}",
                fs::read_to_string(&log).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct Resident {
    child: ChildGuard,
    url: String,
}

fn request(url: &str, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let (status, value) = request_status(url, method, path, body)?;
    if !(200..300).contains(&status) {
        return Err(format!("fixture HTTP {status}: {value}"));
    }
    Ok(value)
}

fn request_status(
    url: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    let endpoint = Endpoint::parse(url, path)?;
    let response = http::request(
        method,
        &endpoint,
        &body.map_or_else(String::new, Value::to_string),
        &[],
        Duration::from_secs(2),
        &TrustAnchors::Webpki,
    )
    .map_err(|error| format!("fixture HTTP transport: {error:?}"))?;
    let value = serde_json::from_str(&response.body).map_err(|error| error.to_string())?;
    Ok((response.status, value))
}

fn show(url: &str, id: &str) -> Value {
    let response = request(
        url,
        "POST",
        "/v1/engineering",
        Some(&json!({"action":"show","id":id})),
    )
    .expect("HTTP engineering show");
    assert_eq!(response["ok"], true, "{response}");
    response["job"].clone()
}

struct ModelFixture {
    url: String,
    calls: Arc<Mutex<Vec<Value>>>,
    probes: Arc<AtomicUsize>,
    errors: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

/// The request is observable before the test permits its model response.
/// Abandoning it simulates an inference interrupted by the resident dying,
/// without returning an action or manufacturing a provider failure first.
struct ModelDecision {
    request: Value,
    received_at: Instant,
    response: mpsc::SyncSender<ModelReply>,
    completed: mpsc::Receiver<Instant>,
}

enum ModelReply {
    Action(Value),
    Text(String),
    HttpStatus(u16),
    Abandon,
}

impl ModelDecision {
    fn respond(self, action: Value) {
        self.response
            .send(ModelReply::Action(action))
            .expect("release model reply");
    }

    fn respond_text(self, text: String) {
        self.response
            .send(ModelReply::Text(text))
            .expect("release raw assistant text");
    }

    fn respond_status(self, status: u16) -> Instant {
        self.response
            .send(ModelReply::HttpStatus(status))
            .expect("release provider failure");
        self.completed
            .recv_timeout(Duration::from_secs(2))
            .expect("provider failure reply is written")
    }

    fn abandon(self) {
        self.response
            .send(ModelReply::Abandon)
            .expect("abandon old model request");
        self.completed
            .recv_timeout(Duration::from_secs(2))
            .expect("old model connection closes");
    }

    fn context(&self) -> Value {
        serde_json::from_str(self.request["messages"][1]["content"].as_str().unwrap())
            .expect("engineering context is JSON")
    }
}

#[derive(Clone)]
enum ModelResponses {
    Script(Arc<Vec<Value>>),
    Controlled(mpsc::SyncSender<ModelDecision>),
}

impl ModelResponses {
    fn answer(
        &self,
        stream: &mut TcpStream,
        index: usize,
        body: Value,
        stopping: &AtomicBool,
    ) -> Result<(), String> {
        match self {
            Self::Script(actions) => {
                let action = actions
                    .get(index)
                    .ok_or("unexpected extra model decision")?;
                completion(stream, index, &action.to_string())
            }
            Self::Controlled(incoming) => {
                let (send, receive) = mpsc::sync_channel(1);
                let (finished, completed) = mpsc::sync_channel(1);
                incoming
                    .try_send(ModelDecision {
                        request: body,
                        received_at: Instant::now(),
                        response: send,
                        completed,
                    })
                    .map_err(|_| "model request controller unavailable")?;
                let response = loop {
                    if stopping.load(Ordering::Relaxed) {
                        break ModelReply::Abandon;
                    }
                    match receive.recv_timeout(Duration::from_millis(50)) {
                        Ok(response) => break response,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break ModelReply::Abandon,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                };
                // Timestamp before writing: the router cannot observe a
                // failure before this point, even if the worker is preempted.
                let sent_at = Instant::now();
                let result = match response {
                    ModelReply::Action(action) => completion(stream, index, &action.to_string()),
                    ModelReply::Text(text) => completion(stream, index, &text),
                    ModelReply::HttpStatus(status) => reply(
                        stream,
                        status,
                        &json!({"error":"scripted provider failure"}),
                    ),
                    ModelReply::Abandon => {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        Ok(())
                    }
                };
                let _ = finished.send(sent_at);
                result
            }
        }
    }
}

fn completion(stream: &mut TcpStream, index: usize, text: &str) -> Result<(), String> {
    reply(
        stream,
        200,
        &json!({
            "id":format!("fixture-{index}"),"object":"chat.completion",
            "model":MODEL,"choices":[{"index":0,"finish_reason":"stop",
                "message":{"role":"assistant","content":text}}],
            "usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}
        }),
    )
}

impl ModelFixture {
    fn start(actions: Vec<Value>) -> Self {
        Self::serve(ModelResponses::Script(Arc::new(actions)))
    }

    fn controlled() -> (Self, mpsc::Receiver<ModelDecision>) {
        let (send, receive) = mpsc::sync_channel(8);
        (Self::serve(ModelResponses::Controlled(send)), receive)
    }

    fn serve(responses: ModelResponses) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let probes = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (recorded, probed, failures, stopping) = (
            Arc::clone(&calls),
            Arc::clone(&probes),
            Arc::clone(&errors),
            Arc::clone(&stop),
        );
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (recorded, probed, failures, stopping, responses) = (
                            Arc::clone(&recorded),
                            Arc::clone(&probed),
                            Arc::clone(&failures),
                            Arc::clone(&stopping),
                            responses.clone(),
                        );
                        // A held inference must not hide a contender's
                        // requests behind a blocked accept loop.
                        workers.push(thread::spawn(move || {
                            let result = model_request(&mut stream).and_then(|(path, body)| {
                                if path == "GET /v1/models" {
                                    probed.fetch_add(1, Ordering::Relaxed);
                                    return reply(
                                        &mut stream,
                                        200,
                                        &json!({"data":[{"id":MODEL}]}),
                                    );
                                }
                                if path != "POST /v1/chat/completions" {
                                    return Err(format!("unexpected model path: {path}"));
                                }
                                let index = {
                                    let mut calls = recorded.lock().unwrap();
                                    let index = calls.len();
                                    calls.push(body.clone());
                                    index
                                };
                                responses.answer(&mut stream, index, body, &stopping)
                            });
                            if let Err(error) = result {
                                failures.lock().unwrap().push(error);
                                let _ = reply(
                                    &mut stream,
                                    500,
                                    &json!({"error":"fixture rejected request"}),
                                );
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => {
                        failures.lock().unwrap().push(error.to_string());
                        break;
                    }
                }
            }
            for worker in workers {
                if worker.join().is_err() {
                    failures
                        .lock()
                        .unwrap()
                        .push("model fixture worker panicked".into());
                }
            }
        });
        Self {
            url,
            calls,
            probes,
            errors,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for ModelFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn model_request(stream: &mut TcpStream) -> Result<(String, Value), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() > 16_384 {
            return Err("fixture request headers too large".into());
        }
        let count = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("fixture request ended in headers".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(|e| e.to_string())?;
    let mut lines = headers.lines();
    let path = lines
        .next()
        .ok_or("missing request line")?
        .strip_suffix(" HTTP/1.1")
        .ok_or("expected HTTP/1.1")?
        .to_owned();
    let mut length = 0;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("authorization") {
                return Err("test model must never receive credentials".into());
            }
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().map_err(|e| e.to_string())?;
            }
        }
    }
    if length > 262_144 {
        return Err("fixture request body too large".into());
    }
    while bytes.len() - header_end < length {
        let count = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("fixture request ended in body".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + length])
            .map_err(|e| e.to_string())?
    };
    Ok((path, body))
}

fn reply(stream: &mut TcpStream, status: u16, value: &Value) -> Result<(), String> {
    let body = value.to_string();
    write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .map_err(|e| e.to_string())
}

#[test]
fn native_engineering_cli_repairs_real_checks_and_survives_resident_restart() {
    let model = ModelFixture::start(vec![
        json!({"action":"read","path":"code.txt"}),
        json!({"action":"write","path":"code.txt","expected":"initial\n","content":"still-broken\n"}),
        json!({"action":"finish","summary":"The model claims the first edit is finished"}),
        json!({"action":"write","path":"code.txt","expected":"still-broken\n","content":"fixed\n"}),
        json!({"action":"finish","summary":"Fixed the file and passed the configured check"}),
    ]);
    let fixture = Fixture::new(&model.url);
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let resident = fixture.start();
    let created = fixture.cli(&resident.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap();
    let planned = created["job"]["candidate"].clone();
    let deadline = Instant::now() + Duration::from_secs(25);
    let ready = loop {
        let job = show(&resident.url, id);
        if job["phase"] == "ready" {
            break job;
        }
        assert_ne!(job["phase"], "paused", "engineering paused: {job}");
        assert!(
            Instant::now() < deadline,
            "engineering never reached Ready: {job}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(ready["candidate"], planned);
    assert_eq!(ready["metrics"]["decisions"], 5);
    assert_eq!(ready["metrics"]["actions"], 5);
    assert_eq!(ready["metrics"]["verification_runs"], 2);
    assert_eq!(ready["metrics"]["verification_failures"], 1);
    assert_eq!(ready["metrics"]["repairs"], 1);
    assert_eq!(ready["metrics"]["model_failures"], 0);
    assert_eq!(ready["validation"]["passed"], true);
    assert!(ready["pending"].is_null());
    let candidate = Path::new(planned["path"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert_eq!(
        ready["validation"]["head"],
        fixture.git(candidate, &["rev-parse", "HEAD"])
    );
    assert!(
        fixture
            .git(candidate, &["status", "--porcelain"])
            .is_empty()
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "initial\n"
    );
    let checks = fs::read_to_string(&fixture.counter).unwrap();
    assert_eq!(
        checks,
        format!(
            "{}\tstill-broken\n{}\tfixed\n",
            candidate.display(),
            candidate.display()
        )
    );

    let calls = model.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        5,
        "count actual model HTTP calls, not store counters"
    );
    let contexts: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            assert_eq!(
                call["model"], MODEL,
                "router must resolve the backend model"
            );
            assert_eq!(call["stream"], false);
            assert!(
                call.get("kamimusuhi_route_lane").is_none(),
                "router-only hints stay local"
            );
            assert!(
                call["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("untrusted data")
            );
            let context: Value =
                serde_json::from_str(call["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(context["objective"], OBJECTIVE);
            assert_eq!(context["candidate"], planned);
            assert_eq!(context["metrics"]["actions"], index as u64);
            assert_eq!(context["configured_checks"].as_array().unwrap().len(), 1);
            context
        })
        .collect();
    let read_observation = contexts[1]["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(read_observation["action"], "read");
    let data: Value =
        serde_json::from_str(read_observation["external_data"].as_str().unwrap()).unwrap();
    assert_eq!(data["content"], "initial\n");
    assert_eq!(contexts[3]["validation"]["passed"], false);
    assert_eq!(contexts[3]["metrics"]["verification_failures"], 1);
    assert_eq!(contexts[4]["metrics"]["repairs"], 1);

    // A second real daemon loads the persisted checkpoint. Completed edits
    // and checks must not replay just because its scheduler starts again.
    drop(resident);
    let restarted = fixture.start();
    let visible = fixture.cli(&restarted.url, &["show", id]);
    assert_eq!(visible["job"]["candidate"], planned);
    assert_eq!(visible["job"]["phase"], "ready");
    assert_eq!(visible["job"]["metrics"], ready["metrics"]);
    let until = Instant::now() + Duration::from_millis(1250);
    while Instant::now() < until {
        assert_eq!(show(&restarted.url, id)["phase"], "ready");
        thread::sleep(Duration::from_millis(50));
    }
    let listing = request(&restarted.url, "GET", "/v1/engineering", None).unwrap();
    assert_eq!(listing["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(listing["jobs"][0]["id"], id);
    assert_eq!(model.calls.lock().unwrap().len(), 5);
    assert_eq!(fs::read_to_string(&fixture.counter).unwrap(), checks);
    assert!(
        model.errors.lock().unwrap().is_empty(),
        "{:?}",
        model.errors.lock().unwrap()
    );
}

fn next_decision(
    requests: &mpsc::Receiver<ModelDecision>,
    model: &str,
    completed_actions: u64,
) -> ModelDecision {
    next_decision_within(requests, model, completed_actions, Duration::from_secs(10))
}

fn next_decision_within(
    requests: &mpsc::Receiver<ModelDecision>,
    model: &str,
    completed_actions: u64,
    timeout: Duration,
) -> ModelDecision {
    let decision = requests
        .recv_timeout(timeout)
        .expect("resident reaches the next model barrier");
    assert_eq!(
        decision.request["model"], model,
        "which resident called the model"
    );
    assert_eq!(decision.context()["metrics"]["actions"], completed_actions);
    decision
}

/// Call while the next model response is held, so both the HTTP view and
/// the on-disk checkpoint describe the completed failure, without a race.
fn retry_checkpoint(fixture: &Fixture, url: &str, id: &str, planned: &Value) -> Value {
    let job = show(url, id);
    assert_eq!(job["id"], id);
    assert_eq!(job["objective"], OBJECTIVE);
    assert_eq!(job["candidate"], *planned);
    assert_eq!(job["phase"], "active");
    assert!(job["pending"].is_null());
    assert_eq!(job["metrics"]["decisions"], 0);
    assert_eq!(job["metrics"]["actions"], 0);
    assert_eq!(job["metrics"]["verification_runs"], 0);
    let candidate = Path::new(planned["path"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "initial\n",
        "a rejected model response must not edit the candidate"
    );
    assert!(
        !fixture.counter.exists(),
        "a rejected Finish must not run checks"
    );
    let checkpoint = fixture
        .dir
        .path()
        .join("resident/current_state/engineering.json");
    let durable: Value = serde_json::from_slice(&fs::read(checkpoint).unwrap()).unwrap();
    assert_eq!(durable["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(durable["jobs"][0], job, "model failures are durable");
    job
}

fn wait_ready(url: &str, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let job = show(url, id);
        if job["phase"] == "ready" {
            return job;
        }
        assert_ne!(job["phase"], "paused", "{job}");
        assert!(
            Instant::now() < deadline,
            "recovery never reached Ready: {job}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn assert_repaired_candidate(
    fixture: &Fixture,
    ready: &Value,
    planned: &Value,
    original_head: &str,
) {
    assert_eq!(ready["objective"], OBJECTIVE);
    assert_eq!(ready["candidate"], *planned);
    assert_eq!(ready["metrics"]["decisions"], 3);
    assert_eq!(ready["metrics"]["actions"], 3);
    assert_eq!(ready["metrics"]["verification_runs"], 1);
    assert_eq!(ready["metrics"]["verification_failures"], 0);
    assert_eq!(ready["metrics"]["interruptions"], 0);
    assert!(ready["pending"].is_null());
    assert!(ready["last_error"].is_null());
    assert_eq!(ready["validation"]["passed"], true);
    let candidate = Path::new(planned["path"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert_eq!(
        fs::read_to_string(&fixture.counter).unwrap(),
        format!("{}\tfixed\n", candidate.display()),
        "exactly one real validation ran in the preserved candidate"
    );
    assert_eq!(
        ready["validation"]["head"],
        fixture.git(candidate, &["rev-parse", "HEAD"])
    );
    assert!(
        fixture
            .git(candidate, &["status", "--porcelain"])
            .is_empty()
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "initial\n"
    );
}

fn recorded_contexts(model: &ModelFixture, planned: &Value) -> Vec<Value> {
    model
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| {
            assert_eq!(call["model"], MODEL);
            let context: Value =
                serde_json::from_str(call["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(context["objective"], OBJECTIVE);
            assert_eq!(context["candidate"], *planned);
            context
        })
        .collect()
}

#[test]
fn malformed_actions_preserve_the_goal_and_recover_through_the_real_router() {
    const ACTION_ERROR: &str = "model response is not a valid engineering action";
    let (model, requests) = ModelFixture::controlled();
    let fixture = Fixture::new(&model.url);
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let resident = fixture.start_as(MODEL, 30);
    let created = fixture.cli(&resident.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap();
    let planned = created["job"]["candidate"].clone();

    // The outer completion is valid. Only the native action parser must
    // reject the Markdown fence, without executing the enclosed Write.
    let rejected_write = json!({
        "action":"write","path":"code.txt","expected":"initial\n","content":"wrong\n"
    });
    next_decision(&requests, MODEL, 0).respond_text(format!("```json\n{rejected_write}\n```"));
    let malformed = next_decision(&requests, MODEL, 0);
    let first_failure = retry_checkpoint(&fixture, &resident.url, id, &planned);
    assert_eq!(first_failure["metrics"]["model_failures"], 1);
    assert_eq!(first_failure["last_error"], ACTION_ERROR);
    // A recognized action carrying an unknown field also must be rejected;
    // accepting it would run an observable failing check prematurely.
    malformed.respond_text(
        json!({"action":"finish","summary":"Must not execute","unexpected":true}).to_string(),
    );
    let recovered = next_decision(&requests, MODEL, 0);
    let second_failure = retry_checkpoint(&fixture, &resident.url, id, &planned);
    assert_eq!(second_failure["metrics"]["model_failures"], 2);
    assert_eq!(second_failure["last_error"], ACTION_ERROR);
    recovered.respond(json!({"action":"read","path":"code.txt"}));
    next_decision(&requests, MODEL, 1).respond(json!({
        "action":"write","path":"code.txt","expected":"initial\n","content":"fixed\n"
    }));
    next_decision(&requests, MODEL, 2)
        .respond(json!({"action":"finish","summary":"Recovered from rejected actions"}));
    let ready = wait_ready(&resident.url, id);
    assert_repaired_candidate(&fixture, &ready, &planned, &original_head);
    assert_eq!(ready["metrics"]["model_failures"], 2);

    let contexts = recorded_contexts(&model, &planned);
    assert_eq!(contexts.len(), 5, "actual completion POST count");
    let actions: Vec<_> = contexts
        .iter()
        .map(|context| context["metrics"]["actions"].as_u64().unwrap())
        .collect();
    assert_eq!(actions, [0, 0, 0, 1, 2]);
    for (index, context) in contexts[1..=2].iter().enumerate() {
        assert_eq!(context["metrics"]["model_failures"], index as u64 + 1);
        assert_eq!(context["last_error"], ACTION_ERROR);
        let latest = context["observations"].as_array().unwrap().last().unwrap();
        assert_eq!(latest["action"], "model");
        assert_eq!(latest["ok"], false);
        let detail: Value =
            serde_json::from_str(latest["external_data"].as_str().unwrap()).unwrap();
        assert_eq!(detail["error"], ACTION_ERROR);
    }
    assert!(requests.try_recv().is_err(), "no unaccounted model request");
    let errors = model.errors.lock().unwrap().clone();
    assert!(errors.is_empty(), "{errors:?}");
    eprintln!(
        "malformed-action recovery: completion_posts=5 durable_actions=3 model_failures=2 checks=1"
    );
}

#[test]
fn auto_provider_503_recovers_after_the_real_bench_without_losing_the_goal() {
    const FIRST_ERROR: &str = "engineering model unavailable (HTTP 503)";
    const RETRY_ERROR: &str = "engineering model unavailable (HTTP 400)";
    let (model, requests) = ModelFixture::controlled();
    let fixture = Fixture::new(&model.url);
    // Healthy model-list probes must not erase a failed completion's bench.
    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["tiers"][0]["probe_interval_secs"] = json!(1);
    fs::write(&fixture.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let resident = fixture.start_as(MODEL, 30);
    let created = fixture.cli(&resident.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap();
    let planned = created["job"]["candidate"].clone();
    let first = next_decision(&requests, MODEL, 0);
    let probes_before = model.probes.load(Ordering::Relaxed);
    let failed_at = first.respond_status(503);

    // Local no-eligible-tier failures are durable retries, but are not
    // additional provider HTTP calls. Observe that state before recovery.
    let deadline = Instant::now() + Duration::from_secs(10);
    let first_episode = loop {
        let waiting = show(&resident.url, id);
        assert_eq!(waiting["candidate"], planned);
        assert_eq!(waiting["phase"], "active");
        assert!(waiting["pending"].is_null());
        assert_eq!(waiting["metrics"]["actions"], 0);
        if waiting["metrics"]["model_failures"].as_u64().unwrap() >= 2 {
            assert_eq!(waiting["last_error"], RETRY_ERROR);
            let episode = waiting["model_recovery"].clone();
            assert_eq!(episode["first_error"], FIRST_ERROR);
            assert_eq!(episode["latest_error"], RETRY_ERROR);
            assert_eq!(episode["attempts"], waiting["metrics"]["model_failures"]);
            assert!(episode["first_failure_at"].as_u64().is_some());
            assert!(episode["last_failure_at"].as_u64().is_some());
            assert!(episode["recovered_at"].is_null());
            break episode;
        }
        assert!(
            Instant::now() < deadline,
            "provider failure was not recorded: {waiting}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(model.calls.lock().unwrap().len(), 1);
    // This is a real wall-clock integration test of the default 60 s bench.
    // A request channel, not an arbitrary sleep, signals that auto routing
    // has admitted the provider again. No forced tier, restart or clock hack.
    let recovered = next_decision_within(
        &requests,
        MODEL,
        0,
        Duration::from_secs(95).saturating_sub(failed_at.elapsed()),
    );
    let bench_duration = recovered.received_at.duration_since(failed_at);
    assert!(
        bench_duration >= Duration::from_secs(60),
        "early retry: {bench_duration:?}"
    );
    assert!(
        bench_duration < Duration::from_secs(95),
        "late retry: {bench_duration:?}"
    );
    assert!(model.probes.load(Ordering::Relaxed) >= probes_before + 2);
    assert_eq!(model.calls.lock().unwrap().len(), 2);
    let retried = retry_checkpoint(&fixture, &resident.url, id, &planned);
    let failures = retried["metrics"]["model_failures"].as_u64().unwrap();
    assert!(
        failures >= 2,
        "count local routing retries separately from HTTP calls"
    );
    assert_eq!(retried["last_error"], RETRY_ERROR);
    let waiting_episode = retried["model_recovery"].clone();
    assert_eq!(waiting_episode["first_error"], FIRST_ERROR);
    assert_eq!(waiting_episode["latest_error"], RETRY_ERROR);
    assert_eq!(waiting_episode["attempts"], failures);
    assert_eq!(
        waiting_episode["first_failure_at"],
        first_episode["first_failure_at"]
    );
    assert!(waiting_episode["recovered_at"].is_null());
    assert_eq!(recovered.context()["metrics"], retried["metrics"]);
    assert_eq!(recovered.context()["model_recovery"], waiting_episode);
    let first_cause_rolled_out = !retried["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|record| record["action"] == "model" && record["detail"]["error"] == FIRST_ERROR);
    // This integration test does not require a particular count of local
    // retries under load. The no-sleep unit fixture forces window rollover.
    recovered.respond(json!({"action":"read","path":"code.txt"}));
    let edit = next_decision(&requests, MODEL, 1);
    let after_read = show(&resident.url, id);
    let recovered_episode = after_read["model_recovery"].clone();
    assert!(recovered_episode["recovered_at"].as_u64().is_some());
    for field in [
        "first_error",
        "latest_error",
        "first_failure_at",
        "last_failure_at",
        "attempts",
    ] {
        assert_eq!(recovered_episode[field], waiting_episode[field]);
    }
    assert_eq!(edit.context()["model_recovery"], recovered_episode);
    let checkpoint = fixture
        .dir
        .path()
        .join("resident/current_state/engineering.json");
    let durable: Value = serde_json::from_slice(&fs::read(checkpoint).unwrap()).unwrap();
    assert_eq!(durable["jobs"][0]["model_recovery"], recovered_episode);
    edit.respond(json!({
        "action":"write","path":"code.txt","expected":"initial\n","content":"fixed\n"
    }));
    let finish = next_decision(&requests, MODEL, 2);
    assert_eq!(finish.context()["model_recovery"], recovered_episode);
    finish.respond(json!({"action":"finish","summary":"Recovered after one provider outage"}));
    let ready = wait_ready(&resident.url, id);
    assert_repaired_candidate(&fixture, &ready, &planned, &original_head);
    assert_eq!(ready["metrics"]["model_failures"], failures);
    assert_eq!(ready["model_recovery"], recovered_episode);
    let contexts = recorded_contexts(&model, &planned);
    assert_eq!(
        contexts.len(),
        4,
        "one failed POST plus three successful decisions"
    );
    let actions: Vec<_> = contexts
        .iter()
        .map(|context| context["metrics"]["actions"].as_u64().unwrap())
        .collect();
    assert_eq!(actions, [0, 0, 1, 2]);
    assert_eq!(contexts[0]["metrics"]["model_failures"], 0);
    assert_eq!(contexts[1]["metrics"]["model_failures"], failures);
    assert!(contexts[0]["model_recovery"].is_null());
    assert_eq!(contexts[1]["model_recovery"], waiting_episode);
    assert_eq!(contexts[2]["model_recovery"], recovered_episode);
    assert_eq!(contexts[3]["model_recovery"], recovered_episode);
    assert!(requests.try_recv().is_err(), "no unaccounted model request");
    let errors = model.errors.lock().unwrap().clone();
    assert!(errors.is_empty(), "{errors:?}");
    eprintln!(
        "provider-503 recovery: bench_ms={} completion_posts=4 durable_actions=3 model_failures={} probes={} checks=1 first_cause_rolled_out={} first_cause_retained=true recovered_once=true",
        bench_duration.as_millis(),
        failures,
        model.probes.load(Ordering::Relaxed),
        first_cause_rolled_out,
    );
}

#[test]
fn active_engineering_restarts_after_model_interruption_and_excludes_second_resident() {
    const OWNER: &str = "fixture-owner-a";
    const CONTENDER: &str = "fixture-contender-b";
    const SUCCESSOR: &str = "fixture-successor-c";
    let (model, requests) = ModelFixture::controlled();
    let fixture = Fixture::new(&model.url);
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let owner = fixture.start_as(OWNER, 30);
    let created = fixture.cli(&owner.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    let planned = created["job"]["candidate"].clone();
    let candidate = Path::new(planned["path"].as_str().unwrap());
    next_decision(&requests, OWNER, 0).respond(json!({"action":"read","path":"code.txt"}));
    let edit = next_decision(&requests, OWNER, 1);
    assert_eq!(edit.context()["candidate"], planned);
    edit.respond(json!({
        "action":"write","path":"code.txt","expected":"initial\n","content":"fixed\n"
    }));

    // Receiving this request proves the previous Write and its outcome
    // checkpoint finished. No action is claimed until we release its reply.
    // Keep it held while checking state and starting the competing daemon.
    let interrupted = next_decision(&requests, OWNER, 2);
    let before = show(&owner.url, &id);
    assert_eq!(before["phase"], "active");
    assert!(before["pending"].is_null());
    assert_eq!(before["metrics"]["decisions"], 2);
    assert_eq!(before["candidate"], planned);
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert_eq!(
        fixture.git(candidate, &["rev-parse", "HEAD"]),
        original_head
    );
    assert!(
        !fixture
            .git(candidate, &["status", "--porcelain"])
            .is_empty(),
        "the restart must preserve an uncommitted edit"
    );
    let checkpoint = fixture
        .dir
        .path()
        .join("resident/current_state/engineering.json");
    let durable_before = fs::read(&checkpoint).expect("durable engineering checkpoint");
    let snapshot: Value = serde_json::from_slice(&durable_before).unwrap();
    assert_eq!(snapshot["jobs"][0]["phase"], "active");
    assert!(snapshot["jobs"][0]["pending"].is_null());
    assert_eq!(snapshot["jobs"][0]["metrics"]["actions"], 2);

    let contender = fixture.start_as(CONTENDER, 30);
    let busy = request(&contender.url, "GET", "/v1/engineering", None).unwrap();
    assert_eq!(
        busy["status"]["error"], LEASE_ERROR,
        "the HTTP server lives, but this engineering store is quarantined"
    );
    for body in [
        json!({"action":"create","workspace":"fixture","objective":"must not create a second job"}),
        json!({"action":"pause","id":id}),
        json!({"action":"resume","id":id}),
    ] {
        let (status, rejected) =
            request_status(&contender.url, "POST", "/v1/engineering", Some(&body)).unwrap();
        assert_eq!(status, 400, "{rejected}");
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"], LEASE_ERROR);
    }
    assert_eq!(
        fs::read(&checkpoint).unwrap(),
        durable_before,
        "contender must not rewrite the owner's checkpoint"
    );
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert!(
        !fixture.counter.exists(),
        "neither daemon has been allowed to verify yet"
    );
    assert_eq!(show(&owner.url, &id)["metrics"], before["metrics"]);
    assert_eq!(model.calls.lock().unwrap().len(), 3);
    assert!(
        matches!(requests.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the contender must not call the model while the owner is held"
    );

    // Kill/wait is the handoff signal. Only afterwards close the old RPC,
    // so a provider error cannot be persisted before the simulated crash.
    // The quarantined B stays alive during C's successful takeover.
    drop(owner);
    interrupted.abandon();
    let successor = fixture.start_as(SUCCESSOR, 30);
    let resumed = next_decision(&requests, SUCCESSOR, 2);
    assert_eq!(resumed.context()["candidate"], planned);
    assert_eq!(resumed.context()["metrics"], before["metrics"]);
    let restored = show(&successor.url, &id);
    assert_eq!(restored["phase"], "active");
    assert!(restored["pending"].is_null());
    assert_eq!(restored["candidate"], planned);
    assert_eq!(
        fs::read(&checkpoint).unwrap(),
        durable_before,
        "a pending-free inference interruption needs no action replay or state rewrite"
    );
    resumed.respond(json!({"action":"read","path":"code.txt"}));
    let finish = next_decision(&requests, SUCCESSOR, 3);
    let context = finish.context();
    let latest = context["observations"].as_array().unwrap().last().unwrap();
    assert_eq!(latest["action"], "read");
    let data: Value = serde_json::from_str(latest["external_data"].as_str().unwrap()).unwrap();
    assert_eq!(
        data["content"], "fixed\n",
        "successor observes the preserved bytes"
    );
    finish.respond(json!({"action":"finish","summary":"Re-read preserved work and verified it"}));
    let deadline = Instant::now() + Duration::from_secs(10);
    let ready = loop {
        let job = show(&successor.url, &id);
        if job["phase"] == "ready" {
            break job;
        }
        assert_ne!(job["phase"], "paused", "{job}");
        assert!(
            Instant::now() < deadline,
            "successor never completed verification: {job}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(ready["candidate"], planned);
    assert_eq!(ready["metrics"]["actions"], 4);
    assert_eq!(ready["metrics"]["decisions"], 4);
    assert_eq!(ready["metrics"]["verification_runs"], 1);
    assert_eq!(ready["metrics"]["verification_failures"], 0);
    assert_eq!(ready["metrics"]["model_failures"], 0);
    assert_eq!(
        ready["metrics"]["interruptions"], 0,
        "the interrupted model request had not claimed an action"
    );
    assert_eq!(ready["validation"]["passed"], true);
    let actions: Vec<_> = ready["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["prepare", "read", "write", "read", "finish"]);
    assert_eq!(
        fs::read_to_string(&fixture.counter).unwrap(),
        format!("{}\tfixed\n", candidate.display())
    );
    assert_eq!(
        ready["validation"]["head"],
        fixture.git(candidate, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "initial\n"
    );

    let still_quarantined = request(&contender.url, "GET", "/v1/engineering", None).unwrap();
    assert_eq!(still_quarantined["status"]["error"], LEASE_ERROR);
    let calls = model.calls.lock().unwrap().clone();
    let callers: Vec<_> = calls
        .iter()
        .map(|call| call["model"].as_str().unwrap())
        .collect();
    assert_eq!(callers, [OWNER, OWNER, OWNER, SUCCESSOR, SUCCESSOR]);
    let counts: Vec<_> = calls
        .iter()
        .map(|call| {
            let context: Value =
                serde_json::from_str(call["messages"][1]["content"].as_str().unwrap()).unwrap();
            context["metrics"]["actions"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(counts, [0, 1, 2, 2, 3]);
    assert!(requests.try_recv().is_err(), "no unaccounted model request");
    let errors = model.errors.lock().unwrap().clone();
    assert!(errors.is_empty(), "{errors:?}");
}

/// The Run child has a separate process group from the daemon. The success
/// path waits for worker cancellation; Drop releases only failed fixtures.
/// This does not assume that parent death directly kills every descendant.
struct ClaimedRunBarrier {
    listener: TcpListener,
    stream: Option<TcpStream>,
}

impl ClaimedRunBarrier {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            listener,
            stream: None,
        }
    }

    fn wait_started(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.listener.accept() {
                Ok((stream, address)) => {
                    assert!(address.ip().is_loopback());
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    self.stream = Some(stream);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "claimed Run never started");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Run barrier accept failed: {error}"),
            }
        }
        let mut started = [0; 8];
        self.stream
            .as_mut()
            .unwrap()
            .read_exact(&mut started)
            .expect("Run confirms its effect before the daemon is killed");
        assert_eq!(&started, b"started\n");
    }

    fn wait_cancelled(&mut self) {
        let stream = self.stream.as_mut().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut end = [0; 1];
        assert_eq!(
            stream
                .read(&mut end)
                .expect("worker cancels Run within 5s after its owner disappears"),
            0,
            "Run stops without release or waiting for the fixture's 20s deadline"
        );
        self.stream = None;
    }
}

impl Drop for ClaimedRunBarrier {
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.write_all(b"release\n");
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Run only by the real resident in the test below. TMPDIR is unique to
/// that fixture and is part of the runner's existing minimal environment.
#[test]
#[ignore = "fixture child, invoked explicitly by the claimed-Run integration test"]
fn claimed_run_barrier_helper() {
    let configuration = std::env::temp_dir().join("claimed-run-fixture.json");
    let configuration: Value =
        serde_json::from_slice(&fs::read(configuration).expect("isolated Run configuration"))
            .unwrap();
    let candidate = Path::new(configuration["candidate"].as_str().unwrap());
    assert_eq!(std::env::current_dir().unwrap(), candidate);
    let address: std::net::SocketAddr = configuration["address"].as_str().unwrap().parse().unwrap();
    assert!(address.ip().is_loopback());
    let mut counter = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(configuration["counter"].as_str().unwrap())
        .unwrap();
    counter.write_all(b"run\n").unwrap();
    counter.sync_all().unwrap();
    fs::write(candidate.join("code.txt"), "fixed\n").unwrap();
    let mut barrier = TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap();
    // The fixture still exits if its controller is lost during an assertion
    // or the parent daemon dies before the controller can accept this socket.
    barrier
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    barrier
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    barrier.write_all(b"started\n").unwrap();
    let mut release = [0; 8];
    barrier
        .read_exact(&mut release)
        .expect("Run controller releases the bounded fixture");
    assert_eq!(&release, b"release\n");
    barrier.write_all(b"done\n").unwrap();
    // The driver never releases this helper on the successful crash path:
    // the worker observes owner EOF, cancels its command group and reaps the
    // direct child. This deadline/release remains a test-failure fallback.
}

#[test]
fn claimed_run_is_not_replayed_after_daemon_death_and_explicit_resume() {
    const OWNER: &str = "fixture-claimed-run-owner";
    const SUCCESSOR: &str = "fixture-claimed-run-successor";
    let (model, requests) = ModelFixture::controlled();
    let fixture = Fixture::new(&model.url);
    let mut barrier = ClaimedRunBarrier::new();
    let executable = std::env::current_exe().unwrap();
    let program = executable.to_str().unwrap();
    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["engineering"]["workspaces"][0]["commands"] = json!([program]);
    config["engineering"]["operation_timeout_secs"] = json!(30);
    fs::write(&fixture.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let owner = fixture.start_as(OWNER, 30);
    let created = fixture.cli(&owner.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    let planned = created["job"]["candidate"].clone();
    let candidate = Path::new(planned["path"].as_str().unwrap());
    let run_counter = fixture.dir.path().join("actual-runs.txt");
    fs::write(
        fixture.dir.path().join("tmp/claimed-run-fixture.json"),
        serde_json::to_vec(&json!({
            "candidate":candidate,"counter":run_counter,
            "address":barrier.listener.local_addr().unwrap().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    next_decision(&requests, OWNER, 0).respond(json!({
        "action":"run","argv":[program,"--ignored","--exact",
            "claimed_run_barrier_helper","--nocapture","--test-threads=1"]
    }));
    barrier.wait_started();
    let before = show(&owner.url, &id);
    assert_eq!(before["phase"], "active");
    assert_eq!(before["candidate"], planned);
    assert_eq!(before["pending"]["action"]["action"], "run");
    assert_eq!(before["pending"]["sequence"], 1);
    assert_eq!(before["next_action_sequence"], 2);
    assert_eq!(before["metrics"]["decisions"], 1);
    assert_eq!(before["metrics"]["actions"], 0);
    assert_eq!(fs::read_to_string(&run_counter).unwrap(), "run\n");
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert!(!fixture.counter.exists());
    let checkpoint = fixture
        .dir
        .path()
        .join("resident/current_state/engineering.json");
    let durable = fs::read(&checkpoint).unwrap();
    let snapshot: Value = serde_json::from_slice(&durable).unwrap();
    assert_eq!(snapshot["jobs"][0], before);

    // Only the resident's owned group is killed here. Its worker has a
    // separate group and detects pipe EOF, then cancels/reaps this Run.
    // The fixture's 20s timeout cannot satisfy the 5s EOF assertion.
    let parent_exit_started = Instant::now();
    drop(owner);
    barrier.wait_cancelled();
    let parent_exit_to_command_eof_ms = parent_exit_started.elapsed().as_millis();
    assert_eq!(fs::read(&checkpoint).unwrap(), durable);
    let successor = fixture.start_as(SUCCESSOR, 30);
    let restored = show(&successor.url, &id);
    assert_eq!(restored["phase"], "paused");
    assert_eq!(restored["candidate"], planned);
    assert_eq!(restored["objective"], OBJECTIVE);
    assert!(restored["pending"].is_null());
    assert_eq!(restored["next_action_sequence"], 2);
    assert_eq!(restored["metrics"]["decisions"], 1);
    assert_eq!(restored["metrics"]["actions"], 0);
    assert_eq!(restored["metrics"]["interruptions"], 1);
    assert_eq!(restored["last_error"], "interrupted_action_outcome_unknown");
    let unknown = restored["observations"].as_array().unwrap().last().unwrap();
    assert_eq!(unknown["action"], "run");
    assert_eq!(unknown["sequence"], 1);
    assert_eq!(unknown["detail"]["sequence"], 1);
    assert_eq!(
        unknown["detail"]["kind"],
        "interrupted_action_outcome_unknown"
    );
    assert!(matches!(
        requests.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(model.calls.lock().unwrap().len(), 1);
    assert_eq!(show(&successor.url, &id)["metrics"], restored["metrics"]);
    assert_eq!(fs::read_to_string(&run_counter).unwrap(), "run\n");
    assert!(!fixture.counter.exists());
    let resumed = fixture.cli(
        &successor.url,
        &[
            "resume",
            &id,
            "Inspect the preserved Run effect before choosing new work",
        ],
    );
    assert_eq!(resumed["job"]["phase"], "active");
    let read = next_decision(&requests, SUCCESSOR, 0);
    assert_eq!(read.context()["candidate"], planned);
    assert_eq!(read.context()["metrics"]["interruptions"], 1);
    read.respond(json!({"action":"read","path":"code.txt"}));
    let finish = next_decision(&requests, SUCCESSOR, 1);
    let context = finish.context();
    let observed = context["observations"].as_array().unwrap().last().unwrap();
    assert_eq!(observed["action"], "read");
    assert_eq!(observed["sequence"], 2);
    let detail: Value = serde_json::from_str(observed["external_data"].as_str().unwrap()).unwrap();
    assert_eq!(detail["content"], "fixed\n");
    finish.respond(json!({"action":"finish","summary":"Verified the preserved Run effect"}));
    let ready = wait_ready(&successor.url, &id);
    assert_eq!(ready["candidate"], planned);
    assert_eq!(ready["metrics"]["decisions"], 3);
    assert_eq!(ready["metrics"]["actions"], 2);
    assert_eq!(ready["metrics"]["interruptions"], 1);
    assert_eq!(ready["metrics"]["verification_runs"], 1);
    assert_eq!(ready["metrics"]["verification_failures"], 0);
    assert_eq!(ready["metrics"]["model_failures"], 0);
    assert_eq!(ready["next_action_sequence"], 4);
    assert_eq!(ready["validation"]["passed"], true);
    let sequences: Vec<_> = ready["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["sequence"] != 0)
        .map(|record| record["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, [1, 2, 3]);
    assert_eq!(fs::read_to_string(&run_counter).unwrap(), "run\n");
    assert_eq!(
        fs::read_to_string(&fixture.counter).unwrap(),
        format!("{}\tfixed\n", candidate.display())
    );
    assert_eq!(
        ready["validation"]["head"],
        fixture.git(candidate, &["rev-parse", "HEAD"])
    );
    assert!(
        fixture
            .git(candidate, &["status", "--porcelain"])
            .is_empty()
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "initial\n"
    );
    let callers: Vec<String> = model
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call["model"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(callers, [OWNER, SUCCESSOR, SUCCESSOR]);
    assert!(requests.try_recv().is_err());
    let errors = model.errors.lock().unwrap().clone();
    assert!(errors.is_empty(), "{errors:?}");
    eprintln!(
        "claimed-run recovery: real_runs=1 replayed_runs=0 paused_model_calls=0 action_sequences=[1,2,3] checks=1 parent_exit_to_command_eof_ms={parent_exit_to_command_eof_ms}"
    );
}

#[test]
fn losing_the_worker_pauses_its_claim_without_replaying_or_counting_an_outcome() {
    let (model, requests) = ModelFixture::controlled();
    let fixture = Fixture::new(&model.url);
    let run_counter = fixture.dir.path().join("actual-runs.txt");
    let script = fixture.dir.path().join("terminate-own-worker.sh");
    // This is the worker's direct command. It signals only its live parent,
    // never the test driver or a process found through a saved/global PID.
    // It exits immediately afterwards; worker death does not promise that
    // arbitrary descendants can be discovered or terminated by the resident.
    fs::write(
        &script,
        "#!/bin/sh\nset -eu\nprintf 'run\\n' >> \"$1\"\n\
         printf 'fixed\\n' > code.txt\nkill -KILL \"$PPID\"\n",
    )
    .unwrap();
    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["engineering"]["workspaces"][0]["commands"] = json!(["/bin/sh"]);
    fs::write(&fixture.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let original_head = fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let resident = fixture.start_as(MODEL, 30);
    let created = fixture.cli(&resident.url, &["start", "fixture", OBJECTIVE]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    let planned = created["job"]["candidate"].clone();
    let candidate = Path::new(planned["path"].as_str().unwrap());
    next_decision(&requests, MODEL, 0).respond(json!({
        "action":"run","argv":["/bin/sh",script,run_counter]
    }));
    let deadline = Instant::now() + Duration::from_secs(10);
    let paused = loop {
        let observed = show(&resident.url, &id);
        if observed["phase"] == "paused" && observed["pending"].is_null() {
            break observed;
        }
        assert!(
            Instant::now() < deadline,
            "worker loss did not pause its claim: {observed}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(paused["candidate"], planned);
    assert_eq!(paused["objective"], OBJECTIVE);
    assert_eq!(paused["metrics"]["decisions"], 1);
    assert_eq!(paused["metrics"]["actions"], 0);
    assert_eq!(paused["metrics"]["interruptions"], 1);
    assert_eq!(paused["metrics"]["verification_runs"], 0);
    assert_eq!(paused["metrics"]["model_failures"], 0);
    assert_eq!(paused["next_action_sequence"], 2);
    assert_eq!(paused["last_error"], "interrupted_action_outcome_unknown");
    assert!(paused["validation"].is_null());
    let unknown = paused["observations"].as_array().unwrap().last().unwrap();
    assert_eq!(unknown["action"], "run");
    assert_eq!(unknown["sequence"], 1);
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["detail"]["sequence"], 1);
    assert_eq!(
        unknown["detail"]["kind"],
        "interrupted_action_outcome_unknown"
    );
    assert!(!unknown["detail"]["error"].as_str().unwrap().is_empty());
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .dir
                .path()
                .join("resident/current_state/engineering.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["jobs"][0], paused);
    assert_eq!(fs::read_to_string(&run_counter).unwrap(), "run\n");
    assert_eq!(
        fs::read_to_string(candidate.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert!(!fixture.counter.exists());
    assert!(matches!(
        requests.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(model.calls.lock().unwrap().len(), 1);
    assert_eq!(show(&resident.url, &id)["metrics"], paused["metrics"]);

    let resumed = fixture.cli(
        &resident.url,
        &["resume", &id, "Inspect the unconfirmed worker effect"],
    );
    assert_eq!(resumed["job"]["phase"], "active");
    // The lost worker consumed a decision, but saved no completed action.
    let read = next_decision(&requests, MODEL, 0);
    assert_eq!(read.context()["candidate"], planned);
    assert_eq!(read.context()["metrics"]["interruptions"], 1);
    read.respond(json!({"action":"read","path":"code.txt"}));
    let finish = next_decision(&requests, MODEL, 1);
    let context = finish.context();
    let observed = context["observations"].as_array().unwrap().last().unwrap();
    assert_eq!(observed["sequence"], 2);
    let detail: Value = serde_json::from_str(observed["external_data"].as_str().unwrap()).unwrap();
    assert_eq!(detail["content"], "fixed\n");
    finish.respond(json!({"action":"finish","summary":"Reverified the preserved candidate"}));
    let ready = wait_ready(&resident.url, &id);
    assert_eq!(ready["metrics"]["decisions"], 3);
    assert_eq!(ready["metrics"]["actions"], 2);
    assert_eq!(ready["metrics"]["interruptions"], 1);
    assert_eq!(ready["metrics"]["verification_runs"], 1);
    assert_eq!(ready["next_action_sequence"], 4);
    assert_eq!(ready["validation"]["passed"], true);
    assert_eq!(
        ready["validation"]["head"],
        fixture.git(candidate, &["rev-parse", "HEAD"])
    );
    let sequences: Vec<_> = ready["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["sequence"] != 0)
        .map(|record| record["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, [1, 2, 3]);
    assert_eq!(fs::read_to_string(&run_counter).unwrap(), "run\n");
    assert_eq!(
        fs::read_to_string(&fixture.counter).unwrap(),
        format!("{}\tfixed\n", candidate.display())
    );
    assert_eq!(
        fixture.git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "initial\n"
    );
    assert_eq!(model.calls.lock().unwrap().len(), 3);
    assert!(requests.try_recv().is_err());
    let errors = model.errors.lock().unwrap().clone();
    assert!(errors.is_empty(), "{errors:?}");
    eprintln!(
        "worker-loss recovery: real_runs=1 replayed_runs=0 unknown_actions_counted=0 interruptions=1 paused_model_calls=0 action_sequences=[1,2,3] checks=1"
    );
}
