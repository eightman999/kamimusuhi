//! The ordinary CLI factory must use exec workers, not just explicitly
//! constructed ProcessLanguageProvider instances in library tests.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const WRAPPER: &str = r#"#!/usr/bin/env python3
import json, os, sys
assert sys.argv[1:] == ['provider-worker']
raw = sys.stdin.buffer.readline(1024*1024+1)
frame = json.loads(raw)
entry = {'operation': frame['operation']}
if frame['operation'] == 'language':
    entry['persona_input_present'] = isinstance(frame.get('request', {}).get('persona_input'), dict)
    entry['has_current_input'] = frame.get('request', {}).get('persona_input', {}).get('input', {}).get('text') == 'こんにちは'
with open(os.environ['KAMIMUSUHI_CLI_AUDIT'], 'a', encoding='utf-8') as audit:
    audit.write(json.dumps(entry)+'\n')
    audit.flush()
# A one-shot worker entry point does not instantiate DialogueSession, so the
# inherited KAMIMUSUHI_PROVIDER_WORKER setting cannot recursively invoke us.
# Use a fresh pipe only to supply the already consumed request to the worker.
import subprocess
result = subprocess.run([os.environ['KAMIMUSUHI_CLI_REAL_WORKER'], 'provider-worker'],
    input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
sys.stdout.buffer.write(result.stdout)
sys.stdout.buffer.flush()
sys.exit(result.returncode)
"#;

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start fixture CLI");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().expect("poll fixture CLI").is_some() {
            return child.wait_with_output().expect("collect fixture CLI");
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled fixture CLI");
            let output = child.wait_with_output().expect("reap stalled fixture CLI");
            panic!(
                "fixture CLI exceeded independent parent deadline: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn default_cli_dispatches_language_and_judgment_to_exec_workers() {
    let fixture = std::env::var_os("KAMIMUSUHI_PROCESS_TEST_ROOT")
        .map_or_else(tempfile::tempdir, tempfile::tempdir_in)
        .unwrap();
    let runtime_dir = fixture.path().join("runtime");
    let wrapper = fixture.path().join("worker-audit.py");
    let audit = fixture.path().join("worker-audit.jsonl");
    std::fs::write(&wrapper, WRAPPER).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let binary = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

    let initialized = bounded_output(
        Command::new(binary)
            .arg("init")
            .arg("--dir")
            .arg(&runtime_dir)
            .args(["--seed", "58001"])
            .env_remove("TYPESAFE_API_KEY")
            .env("KAMIMUSUHI_LLM_PROVIDER", "mock"),
    );
    assert!(
        initialized.status.success(),
        "fixture init: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    let reply = bounded_output(
        Command::new(binary)
            .arg("talk")
            .arg("--dir")
            .arg(&runtime_dir)
            .args([
                "--seed",
                "58002",
                "--persona",
                "fake",
                "--privacy",
                "local-only",
                "--message",
                "こんにちは",
            ])
            .env("KAMIMUSUHI_PROVIDER_WORKER", &wrapper)
            .env("KAMIMUSUHI_CLI_AUDIT", &audit)
            .env("KAMIMUSUHI_CLI_REAL_WORKER", binary)
            .env("KAMIMUSUHI_LLM_PROVIDER", "mock")
            .env_remove("TYPESAFE_API_KEY"),
    );
    assert!(
        reply.status.success(),
        "fixture talk: {}",
        String::from_utf8_lossy(&reply.stderr)
    );
    let stdout = String::from_utf8(reply.stdout).unwrap();
    let delivered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        delivered["response"]
            .as_str()
            .is_some_and(|response| response.starts_with("mock:"))
    );

    let records: Vec<Value> = std::fs::read_to_string(audit)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let operations: Vec<&str> = records
        .iter()
        .map(|record| record["operation"].as_str().unwrap())
        .collect();
    assert_eq!(operations, ["prepare", "language", "assess"]);
    let language = records
        .iter()
        .find(|record| record["operation"] == "language")
        .unwrap();
    assert_eq!(language["persona_input_present"], true);
    assert_eq!(language["has_current_input"], true);
}
