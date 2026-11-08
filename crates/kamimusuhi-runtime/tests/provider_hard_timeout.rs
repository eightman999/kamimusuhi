//! Exec-isolated non-cooperative provider regressions. The helper executable
//! ignores SIGTERM and has no own deadline. The host must SIGKILL/reap it and
//! return its admission slot before another invocation can proceed.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::testing::ScriptedDecisionProvider;
use kamimusuhi_runtime::llm_jev::{
    CancellationToken, ConversationError, Decision, DecisionProvider, LanguageProvider,
    RuleBasedDecisionProvider,
};
use kamimusuhi_runtime::provider_process::{
    MAX_PROVIDER_PROCESSES, ProcessCommand, active_provider_processes,
};
use kamimusuhi_runtime::provider_worker::{ProcessDecisionProvider, ProcessLanguageProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};

const PROCESS_TIMEOUT: Duration = Duration::from_millis(100);
// Healthy interpreter/worker startup needs scheduling headroom on busy CI.
const HOST_TIMEOUT: Duration = Duration::from_secs(1);
const ALL_STOP_TIMEOUT: Duration = Duration::from_millis(250);
const HEALTHY_TIMEOUT: Duration = Duration::from_secs(5);
const REPEATED_HANGS: usize = 96;

const FIXTURE: &str = r#"
import json, os, signal, subprocess, sys, time
mode, audit, worker = sys.argv[1:]
def record(kind):
    with open(audit, 'a', encoding='utf-8') as f:
        f.write(json.dumps({'pid': os.getpid(), 'kind': kind, 'mode': mode})+'\n')
        f.flush()
def trace(stage, size=0):
    with open(audit+'.trace', 'a', encoding='utf-8') as f:
        f.write(json.dumps({'pid':os.getpid(),'mode':mode,'stage':stage,'size':size,'at':time.monotonic()})+'\n')
def hang():
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    record('entered-hang')
    while True: time.sleep(3600)
record('root')
if mode == 'never-read': hang()
raw = sys.stdin.buffer.readline(1024*1024+1)
request = json.loads(raw)
trace('received', len(raw))
operation = request.get('operation')
if mode == 'probe':
    print(json.dumps({'probe': 'usable'}), flush=True)
    sys.exit(0)
if mode == 'flood':
    sys.stdout.write('X'*(2*1024*1024)); sys.stdout.flush(); hang()
if mode in ('descendant', 'held-stdout'):
    code = '''import json,os,signal,sys,time
signal.signal(signal.SIGTERM,signal.SIG_IGN)
with open(sys.argv[1], 'a') as f:
 f.write(json.dumps({'pid':os.getpid(),'kind':'descendant','mode':sys.argv[2]})+'\\n'); f.flush()
while True: time.sleep(3600)
'''
    subprocess.Popen([sys.executable, '-u', '-c', code, audit, mode])
    time.sleep(0.025)
    if mode == 'held-stdout':
        print(json.dumps({'probe': 'usable'}), flush=True)
        sys.exit(0)
    hang()
if mode == 'hang' or mode == 'hang-'+str(operation): hang()
if mode == 'hang-repair' and request.get('request',{}).get('persona_input',{}).get('envelope',{}).get('response_guidance'):
    hang()
if mode == 'healthy': time.sleep(0.025)
if operation != 'language': request['config'] = {'backend': 'rule_based'}
raw = (json.dumps(request)+'\n').encode('utf-8')
env = os.environ.copy()
env.pop('TYPESAFE_API_KEY', None)
env['KAMIMUSUHI_LLM_PROVIDER'] = 'mock'
trace('delegate')
result = subprocess.run([worker, 'provider-worker'], input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
trace('complete', len(result.stdout))
sys.stdout.buffer.write(result.stdout); sys.stdout.buffer.flush()
sys.exit(result.returncode)
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    script: PathBuf,
    audit: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::var_os("KAMIMUSUHI_PROCESS_TEST_ROOT")
            .map_or_else(tempfile::tempdir, tempfile::tempdir_in)
            .unwrap();
        let script = dir.path().join("provider.py");
        let audit = dir.path().join("children.jsonl");
        std::fs::write(&script, FIXTURE).unwrap();
        Self {
            _dir: dir,
            script,
            audit,
        }
    }

    fn command(&self, mode: &str) -> ProcessCommand {
        ProcessCommand {
            program: PathBuf::from("python3"),
            args: vec![
                "-u".to_owned(),
                self.script.display().to_string(),
                mode.to_owned(),
                self.audit.display().to_string(),
                env!("CARGO_BIN_EXE_kamimusuhi-runtime").to_owned(),
            ],
        }
    }

    fn language(&self, runtime: &Runtime, mode: &str) -> ProcessLanguageProvider {
        ProcessLanguageProvider::new(
            self.command(mode),
            runtime.config().clone(),
            None,
            PROCESS_TIMEOUT,
        )
    }

    fn assert_entered_more_than_capacity(&self) {
        let entered = std::fs::read_to_string(&self.audit)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|entry| entry["kind"] == "entered-hang")
            .count();
        eprintln!("witnessed permanent hang entries={entered}, capacity={MAX_PROVIDER_PROCESSES}");
        assert!(
            entered > MAX_PROVIDER_PROCESSES,
            "fixture must enter the non-cooperative body more often than capacity"
        );
    }

    fn assert_reaped(&self) {
        wait_for_no_active_processes();
        let entries = std::fs::read_to_string(&self.audit).unwrap_or_default();
        for line in entries.lines() {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            if entry["kind"] != "root" && entry["kind"] != "descendant" {
                continue;
            }
            let pid = entry["pid"].as_u64().unwrap() as u32;
            let descendant = entry["kind"] == "descendant";
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let result = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .expect("child liveness measurement");
                if !result.status.success() || result.stdout.iter().all(u8::is_ascii_whitespace) {
                    break;
                }
                // Only orphaned descendants may be waiting for init's reap.
                // Direct workers must disappear: a zombie would prove wait()
                // was omitted even though the process no longer runs.
                let stat = String::from_utf8_lossy(&result.stdout);
                if descendant && stat.trim_start().starts_with('Z') {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "provider PID {pid} still alive: {stat}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn wait_for_no_active_processes() {
    let deadline = Instant::now() + Duration::from_secs(2);
    while active_provider_processes() != 0 {
        assert!(
            Instant::now() < deadline,
            "provider process admission leaked"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn runtime() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(0x5800),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    (dir, runtime)
}

fn session(
    runtime: &mut Runtime,
    language: Box<dyn LanguageProvider>,
    decision: Box<dyn DecisionProvider>,
) -> DialogueSession {
    let mut session = DialogueSession::start_with_providers(
        runtime,
        "process-timeout-regression",
        PrivacyConstraint::Unconstrained,
        decision,
        Some(language),
    )
    .unwrap();
    session.set_provider_timeout(HOST_TIMEOUT).unwrap();
    session.set_debug_trace(true);
    session
}

fn rss_kib() -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("RSS measurement");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn thread_count() -> usize {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/task").unwrap().count()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = Command::new("ps")
            .args(["-M", "-p", &std::process::id().to_string()])
            .output()
            .expect("thread measurement");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .count()
            .saturating_sub(1)
    }
}

fn assert_plateau(threads: usize, rss: u64) {
    wait_for_no_active_processes();
    let after_threads = thread_count();
    let after_rss = rss_kib();
    eprintln!(
        "process plateau: threads={threads}->{after_threads}, RSS KiB={rss}->{after_rss}, active children={}",
        active_provider_processes()
    );
    assert!(after_threads <= threads + 1);
    assert!(after_rss <= rss + 16 * 1024);
}

fn repeated_transport_hangs_are_reaped() {
    let fixture = Fixture::new();
    let request = serde_json::json!({"probe": "PRIVATE_PROVIDER_MATERIAL_DO_NOT_PERSIST"});
    const { assert!(REPEATED_HANGS > MAX_PROVIDER_PROCESSES) };
    for _ in 0..4 {
        assert!(matches!(
            fixture
                .command("hang")
                .invoke(&request, PROCESS_TIMEOUT, None),
            Err(ConversationError::Timeout)
        ));
    }
    let threads = thread_count();
    let rss = rss_kib();
    for _ in 0..REPEATED_HANGS {
        let started = Instant::now();
        assert!(matches!(
            fixture
                .command("hang")
                .invoke(&request, PROCESS_TIMEOUT, None),
            Err(ConversationError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            active_provider_processes(),
            0,
            "timeout must release admission only after reap"
        );
        assert_eq!(
            fixture
                .command("probe")
                .invoke(&request, HEALTHY_TIMEOUT, None)
                .unwrap()["probe"],
            "usable"
        );
        assert_eq!(active_provider_processes(), 0);
    }
    fixture.assert_entered_more_than_capacity();
    fixture.assert_reaped();
    assert_plateau(threads, rss);
}

fn all_stopped_then_healthy_provider_continues() {
    let fixture = Fixture::new();
    let (_dir, mut runtime) = runtime();
    let language = fixture.language(&runtime, "hang");
    let mut session = session(
        &mut runtime,
        Box::new(language),
        Box::new(RuleBasedDecisionProvider),
    );
    session
        .add_language_provider(
            "second-stopped",
            Arc::new(fixture.language(&runtime, "hang")),
        )
        .unwrap();
    session.set_provider_timeout(ALL_STOP_TIMEOUT).unwrap();
    for _ in 0..REPEATED_HANGS {
        let started = Instant::now();
        let error = session
            .turn(&mut runtime, "こんにちは", |_| {
                panic!("no candidate may be delivered")
            })
            .unwrap_err();
        assert!(matches!(
            error,
            kamimusuhi_runtime::RuntimeError::Conversation(ConversationError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        wait_for_no_active_processes();
        assert_eq!(session.last_generated_candidates().len(), 2);
        assert!(
            session
                .last_generated_candidates()
                .iter()
                .all(|c| c.error_code.as_deref() == Some("TIMEOUT"))
        );
    }
    session.set_provider_timeout(HEALTHY_TIMEOUT).unwrap();
    session
        .add_language_provider("backup", Arc::new(fixture.language(&runtime, "healthy")))
        .unwrap();
    for _ in 0..16 {
        let reply = session
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap_or_else(|error| panic!("healthy recovery failed: {error:?}; attempts={:?}; active children={}; workers={}; fixture trace={}",
                session.last_generated_candidates(), active_provider_processes(),
                kamimusuhi_runtime::provider_boundary::active_provider_workers(),
                std::fs::read_to_string(fixture.audit.with_extension("jsonl.trace")).unwrap_or_default().lines().rev().take(12).collect::<Vec<_>>().join(";")));
        assert!(!reply.response.is_empty());
        assert_eq!(reply.llm_jev.unwrap().language_provider_id, "backup");
        wait_for_no_active_processes();
    }
    fixture.assert_entered_more_than_capacity();
    fixture.assert_reaped();
}

fn usable_provider_wins_each_time() {
    let fixture = Fixture::new();
    let (_dir, mut runtime) = runtime();
    let language = fixture.language(&runtime, "hang");
    let mut session = session(
        &mut runtime,
        Box::new(language),
        Box::new(RuleBasedDecisionProvider),
    );
    session.set_provider_timeout(HEALTHY_TIMEOUT).unwrap();
    session
        .add_language_provider("backup", Arc::new(fixture.language(&runtime, "healthy")))
        .unwrap();
    for _ in 0..4 {
        session
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap();
        wait_for_no_active_processes();
    }
    let threads = thread_count();
    let rss = rss_kib();
    for _ in 0..REPEATED_HANGS {
        let reply = session
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap();
        assert!(!reply.response.is_empty());
        assert_eq!(reply.llm_jev.unwrap().language_provider_id, "backup");
        wait_for_no_active_processes();
    }
    fixture.assert_entered_more_than_capacity();
    fixture.assert_reaped();
    assert_plateau(threads, rss);
}

fn decision_and_repair_stops_are_reaped() {
    for operation in ["prepare", "assess"] {
        let fixture = Fixture::new();
        let (_dir, mut runtime) = runtime();
        let language = fixture.language(&runtime, "healthy");
        let decision = ProcessDecisionProvider::new(
            fixture.command(&format!("hang-{operation}")),
            PROCESS_TIMEOUT,
        );
        let mut session = session(&mut runtime, Box::new(language), Box::new(decision));
        for _ in 0..8 {
            let reply = session
                .turn(&mut runtime, "こんにちは", |_| Ok(()))
                .unwrap();
            assert!(!reply.response.is_empty());
            assert!(!reply.llm_jev.unwrap().decision_fallbacks.is_empty());
            wait_for_no_active_processes();
        }
        fixture.assert_reaped();
    }
    let fixture = Fixture::new();
    let (_dir, mut runtime) = runtime();
    let language = fixture.language(&runtime, "hang-repair");
    let mut decision = ScriptedDecisionProvider::accept_all();
    decision
        .gate_rules
        .push(("mock:".to_owned(), Decision::Retry));
    let mut session = session(&mut runtime, Box::new(language), Box::new(decision));
    let error = session
        .turn(&mut runtime, "こんにちは", |_| {
            panic!("repair may not deliver old candidate")
        })
        .unwrap_err();
    assert!(matches!(
        error,
        kamimusuhi_runtime::RuntimeError::Conversation(ConversationError::Timeout)
    ));
    fixture.assert_reaped();
}

fn blocked_io_descendants_and_cancellation_are_reaped() {
    let fixture = Fixture::new();
    let small = serde_json::json!({"probe": "request"});
    let large = serde_json::json!({"payload": "X".repeat(512*1024)});
    for mode in ["never-read", "descendant"] {
        let request = if mode == "never-read" { &large } else { &small };
        assert!(matches!(
            fixture.command(mode).invoke(request, PROCESS_TIMEOUT, None),
            Err(ConversationError::Timeout)
        ));
        assert_eq!(active_provider_processes(), 0);
    }
    assert_eq!(
        fixture
            .command("held-stdout")
            .invoke(&small, HOST_TIMEOUT, None)
            .unwrap()["probe"],
        "usable"
    );
    assert!(
        fixture
            .command("flood")
            .invoke(&small, HOST_TIMEOUT, None)
            .is_err()
    );
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel.cancel();
    });
    assert!(matches!(
        fixture
            .command("hang")
            .invoke(&small, HOST_TIMEOUT, Some(&cancellation)),
        Err(ConversationError::Cancelled)
    ));
    canceller.join().unwrap();
    fixture.assert_reaped();
}

fn cleanup_failed_scenario(root: &std::path::Path) {
    // A broken transport must not leave its process groups behind when the
    // independent parent deadline fires. The audit files are parent-owned.
    let mut seen = std::collections::BTreeSet::new();
    for dir in std::fs::read_dir(root).unwrap().flatten() {
        let audit = dir.path().join("children.jsonl");
        for line in std::fs::read_to_string(audit).unwrap_or_default().lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(pid) = record["pid"].as_u64() else {
                continue;
            };
            if !seen.insert(pid) {
                continue;
            }
            // Historical PIDs can be reused. Only signal a live fixture whose
            // command still contains this parent's unique private directory.
            let Ok(identity) = Command::new("ps")
                .args(["-o", "command=", "-p", &pid.to_string()])
                .output()
            else {
                continue;
            };
            if !identity.status.success()
                || !String::from_utf8_lossy(&identity.stdout)
                    .contains(root.to_string_lossy().as_ref())
            {
                continue;
            }
            for target in [format!("-{pid}"), pid.to_string()] {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &target])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

fn run_isolated(scenario: &str) {
    let fixtures = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "isolated_process_scenario",
            "--ignored",
            "--nocapture",
        ])
        .env("KAMIMUSUHI_PROCESS_TIMEOUT_SCENARIO", scenario)
        .env("KAMIMUSUHI_PROCESS_TEST_ROOT", fixtures.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            if !status.success() {
                cleanup_failed_scenario(fixtures.path());
            }
            assert!(
                status.success(),
                "scenario {scenario}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            cleanup_failed_scenario(fixtures.path());
            let output = child.wait_with_output().unwrap();
            panic!(
                "scenario {scenario} exceeded deadline:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn repeated_noncooperative_processes_are_killed_reaped_and_slots_reused() {
    run_isolated("repeat");
}
#[test]
fn all_provider_stops_are_finite_and_later_healthy_provider_still_works() {
    run_isolated("all");
}
#[test]
fn healthy_provider_keeps_winning_across_more_than_the_process_limit_of_hangs() {
    run_isolated("one");
}
#[test]
fn decision_and_repair_processes_are_killed_and_reaped() {
    run_isolated("decision");
}
#[test]
fn blocked_pipes_and_descendant_processes_cannot_leak_admission() {
    run_isolated("io");
}
#[test]
#[ignore = "executed only by independent-deadline parent tests"]
fn isolated_process_scenario() {
    match std::env::var("KAMIMUSUHI_PROCESS_TIMEOUT_SCENARIO")
        .unwrap()
        .as_str()
    {
        "repeat" => repeated_transport_hangs_are_reaped(),
        "all" => all_stopped_then_healthy_provider_continues(),
        "one" => usable_provider_wins_each_time(),
        "decision" => decision_and_repair_stops_are_reaped(),
        "io" => blocked_io_descendants_and_cancellation_are_reaped(),
        other => panic!("unknown scenario {other}"),
    }
}
