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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi");
const OBJECTIVE: &str = "Fix code.txt, verify the candidate, and preserve the operator checkout";
const MODEL: &str = "fixture-native-engineer";

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
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve test port");
        let address = listener.local_addr().unwrap();
        let mut config: Value = serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap();
        config["node"]["listen"] = json!(address.to_string());
        fs::write(&self.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let log = self
            .dir
            .path()
            .join(format!("resident-{}.stderr", address.port()));
        let mut command = self.command(BINARY);
        command
            .args(["serve", "--config"])
            .arg(&self.config)
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
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "fixture HTTP {}: {}",
            response.status, response.body
        ));
    }
    serde_json::from_str(&response.body).map_err(|error| error.to_string())
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
    errors: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ModelFixture {
    fn start(actions: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (recorded, failures, stopping) =
            (Arc::clone(&calls), Arc::clone(&errors), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let result = model_request(&mut stream).and_then(|(path, body)| {
                            if path == "GET /v1/models" {
                                return reply(&mut stream, 200, &json!({"data":[{"id":MODEL}]}));
                            }
                            if path != "POST /v1/chat/completions" {
                                return Err(format!("unexpected model path: {path}"));
                            }
                            let mut calls = recorded.lock().unwrap();
                            let index = calls.len();
                            calls.push(body);
                            let Some(action) = actions.get(index) else {
                                return Err("unexpected extra model decision".to_owned());
                            };
                            reply(&mut stream, 200, &json!({
                                "id":format!("fixture-{index}"),"object":"chat.completion",
                                "model":MODEL,"choices":[{"index":0,"finish_reason":"stop",
                                    "message":{"role":"assistant","content":action.to_string()}}],
                                "usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}
                            }))
                        });
                        if let Err(error) = result {
                            failures.lock().unwrap().push(error);
                            let _ = reply(
                                &mut stream,
                                500,
                                &json!({"error":"fixture rejected request"}),
                            );
                        }
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
        });
        Self {
            url,
            calls,
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
