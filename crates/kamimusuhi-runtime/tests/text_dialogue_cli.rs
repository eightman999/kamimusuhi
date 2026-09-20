//! Dialogue through separate binary processes and a real loopback HTTP fixture.
//! These checks establish CLI/process behavior, not actual model quality.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use kamimusuhi_core::persona::{ConversationMessage, ConversationRole};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

fn run(dir: &Path, command: &str, flags: &[&str], stdin: Option<&str>) -> Output {
    run_with_env(dir, command, flags, stdin, &[])
}

fn run_with_env(
    dir: &Path,
    command: &str,
    flags: &[&str],
    stdin: Option<&str>,
    environment: &[(&str, &str)],
) -> Output {
    let mut child = Command::new(BINARY)
        .arg(command)
        .arg("--dir")
        .arg(dir)
        .args(flags)
        .envs(environment.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "runtime failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn initialized() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    success(run(dir.path(), "init", &[], None));
    dir
}

fn config_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("runtime.json")).unwrap()
}

fn counts(dir: &Path) -> [i64; 4] {
    let connection = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    ["sessions", "turns", "evidence_records", "state_records"].map(|table| {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    })
}

fn identity(dir: &Path) -> (String, String, String, i64, i64) {
    let connection = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    connection
        .query_row(
            "SELECT i.individual_id, i.root_commit_id, h.commit_id, h.generation, i.current_writer_epoch
         FROM individuals i JOIN continuity_heads h USING (individual_id)",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap()
}

fn prompt(server: &FixtureServer, index: usize) -> String {
    let request = &server.requests()[index];
    assert_eq!(request.path, "/v1/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    body["messages"][1]["content"].as_str().unwrap().to_owned()
}

fn section<'a>(prompt: &'a str, label: &str) -> Option<&'a str> {
    prompt.split_once(&format!("[{label}]\n"))?.1.lines().next()
}

/// Compare without ever printing a credential or an Authorization header on
/// failure. The fixture credentials are synthetic and valid nowhere.
fn assert_no_credential(dir: &Path, output: &Output, credential: &str) {
    let contains = |bytes: &[u8]| {
        bytes
            .windows(credential.len())
            .any(|window| window == credential.as_bytes())
    };
    assert!(!contains(&output.stdout), "credential leaked to stdout");
    assert!(!contains(&output.stderr), "credential leaked to stderr");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            // Includes runtime.json, trace and SQLite files, including any
            // WAL/SHM files still present after the child process exited.
            assert!(
                !contains(&std::fs::read(path).unwrap()),
                "credential reached a runtime file"
            );
        }
    }
}

#[test]
fn talk_reopens_in_a_new_process_with_raw_evidence_history_and_the_same_head() {
    let dir = initialized();
    let before = identity(dir.path());
    let server = FixtureServer::start(vec![
        FixtureResponse::ok("プロセス1の返事。"),
        FixtureResponse::ok("プロセス2の返事。"),
    ])
    .unwrap();
    let raw = "  改行と空白を保つ原文。\n次の行  ";
    let first: serde_json::Value = serde_json::from_str(&success(run(
        dir.path(),
        "talk",
        &[
            "--message",
            raw,
            "--subject",
            "alice",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &server.base_url(),
            "--persona-model",
            "cli-fixture",
            "--persona-locality",
            "local-host",
        ],
        None,
    )))
    .unwrap();
    assert_eq!(first["response"], "プロセス1の返事。");
    let configured = config_bytes(dir.path());
    // The next process receives only the saved directory and a new input.
    let second: serde_json::Value = serde_json::from_str(&success(run(
        dir.path(),
        "talk",
        &["--message", "前の原文は？", "--subject", "alice"],
        None,
    )))
    .unwrap();
    assert_eq!(second["response"], "プロセス2の返事。");
    assert_ne!(first["session_id"], second["session_id"]);
    assert_eq!(second["history_messages"], 2);
    let sent = prompt(&server, 1);
    let history: Vec<ConversationMessage> =
        serde_json::from_str(section(&sent, "CONVERSATION_HISTORY").unwrap()).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text, raw);
    assert_eq!(history[0].role, ConversationRole::User);
    assert_eq!(
        serde_json::json!(history[0].evidence_id),
        first["input_evidence_id"]
    );
    assert_eq!(history[1].text, "プロセス1の返事。");
    assert_eq!(history[1].role, ConversationRole::Assistant);
    let connection = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    let (kind, payload): (String, String) = connection
        .query_row(
            "SELECT kind, payload_json FROM evidence_records WHERE evidence_id = ?1",
            [history[1].evidence_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(kind, "agent_utterance");
    assert_eq!(
        payload["generated_evidence_id"],
        first["generated_evidence_id"]
    );
    assert_eq!(payload["delivery"], "output_surface_accepted");
    assert_eq!(identity(dir.path()), before);
    assert_eq!(counts(dir.path()), [2, 2, 6, 0]);
    assert_eq!(config_bytes(dir.path()), configured);
}

#[test]
fn piped_chat_emits_two_responses_and_quit_prevents_a_third_turn() {
    let dir = initialized();
    let server = FixtureServer::start(vec![
        FixtureResponse::ok("最初の返事。"),
        FixtureResponse::ok("次の返事。"),
    ])
    .unwrap();
    let output = success(run(
        dir.path(),
        "chat",
        &[
            "--subject",
            "alice",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &server.base_url(),
            "--persona-model",
            "cli-fixture",
            "--persona-locality",
            "local-host",
        ],
        Some("  一行目  \r\n\n二行目\n/quit\n送信されてはいけない行\n"),
    ));
    assert_eq!(output, "最初の返事。\n次の返事。\n");
    assert_eq!(server.request_count(), 2);
    let first = prompt(&server, 0);
    let first_input: String =
        serde_json::from_str(section(&first, "CURRENT_INPUT").unwrap()).unwrap();
    assert_eq!(first_input, "  一行目  ");
    let second = prompt(&server, 1);
    let second_input: String =
        serde_json::from_str(section(&second, "CURRENT_INPUT").unwrap()).unwrap();
    assert_eq!(second_input, "二行目");
    let history: Vec<ConversationMessage> =
        serde_json::from_str(section(&second, "CONVERSATION_HISTORY").unwrap()).unwrap();
    assert_eq!(history[0].text, "  一行目  ");
    assert_eq!(history[1].text, "最初の返事。");
    assert_eq!(counts(dir.path()), [1, 2, 6, 0]);
}

#[test]
fn debug_trace_exposes_decisions_and_latency_without_provider_secrets() {
    let dir = initialized();
    let output: serde_json::Value = serde_json::from_str(&success(run(
        dir.path(),
        "talk",
        &["--persona", "fake", "--message", "こんにちは", "--debug"],
        None,
    )))
    .unwrap();
    assert_eq!(output["llm_jev"]["invocation_gate"]["decision"], "SPEAK");
    assert_eq!(output["llm_jev"]["response_gate"]["decision"], "ACCEPT");
    assert_eq!(output["llm_jev"]["language_provider"], "openai-compatible");
    assert!(output["llm_jev"]["candidate_digest"].as_str().is_some());
    assert!(output["llm_jev"].get("fallback_reason").is_none());
}

#[test]
fn the_default_fake_requires_explicit_selection_on_each_invocation() {
    let dir = initialized();
    let config = config_bytes(dir.path());
    let head = identity(dir.path());
    let rejected = run(dir.path(), "talk", &["--message", "こんにちは"], None);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--persona fake"));
    assert_eq!(counts(dir.path()), [0, 0, 0, 0]);
    assert_eq!(config_bytes(dir.path()), config);
    let accepted: serde_json::Value = serde_json::from_str(&success(run(
        dir.path(),
        "talk",
        &["--message", "こんにちは", "--persona", "fake"],
        None,
    )))
    .unwrap();
    assert_eq!(accepted["persona_backend"]["kind"], "fixture");
    assert!(
        accepted["response"]
            .as_str()
            .unwrap()
            .starts_with("fixture-ack:")
    );
    let before_retry = counts(dir.path());
    let rejected_again = run(dir.path(), "talk", &["--message", "もう一度"], None);
    assert_eq!(rejected_again.status.code(), Some(2));
    assert_eq!(counts(dir.path()), before_retry);
    assert_eq!(identity(dir.path()), head);
}

#[test]
fn local_only_is_default_and_privacy_rejections_preserve_config_and_interactions() {
    let dir = initialized();
    let server =
        FixtureServer::always(FixtureResponse::ok("explicitly permitted fixture reply")).unwrap();
    let config_before = config_bytes(dir.path());
    let head = identity(dir.path());
    // This endpoint is physically a test loopback socket; declare it external
    // to exercise the operator's data-boundary policy without external I/O.
    let rejected = run(
        dir.path(),
        "talk",
        &[
            "--message",
            "PRIVATE_INPUT",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &server.base_url(),
            "--persona-model",
            "cli-fixture",
            "--persona-locality",
            "external",
        ],
        None,
    );
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("[PRIVACY]"));
    assert_eq!(server.request_count(), 0);
    assert_eq!(counts(dir.path()), [0, 0, 0, 0]);
    assert_eq!(config_bytes(dir.path()), config_before);
    success(run(
        dir.path(),
        "talk",
        &[
            "--message",
            "PERMITTED_FIXTURE_INPUT",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &server.base_url(),
            "--persona-model",
            "cli-fixture",
            "--persona-locality",
            "external",
            "--privacy",
            "unconstrained",
        ],
        None,
    ));
    assert_eq!(server.request_count(), 1);
    let configured = config_bytes(dir.path());
    let prior_counts = counts(dir.path());
    // The previous invocation's wider privacy choice is not a saved default.
    let rejected_again = run(
        dir.path(),
        "talk",
        &["--message", "PRIVATE_NEXT_INPUT"],
        None,
    );
    assert_eq!(rejected_again.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected_again.stderr).contains("[PRIVACY]"));
    assert_eq!(server.request_count(), 1);
    assert_eq!(counts(dir.path()), prior_counts);
    assert_eq!(config_bytes(dir.path()), configured);
    assert_eq!(identity(dir.path()), head);
}

#[test]
fn api_auth_reads_the_named_environment_on_each_process_without_persisting_its_value() {
    const ENV_NAME: &str = "KAMIMUSUHI_CLI_TEST_API_KEY";
    const FIRST_KEY: &str = "dummy-kamimusuhi-fixture-api-key-first-not-real";
    const NEXT_KEY: &str = "dummy-kamimusuhi-fixture-api-key-next-not-real";
    let dir = initialized();
    let server = FixtureServer::always(FixtureResponse::ok("認証付きfixture応答。")).unwrap();
    let first = run_with_env(
        dir.path(),
        "talk",
        &[
            "--message",
            "API fixture first turn",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &server.base_url(),
            "--persona-model",
            "api-fixture",
            "--persona-auth-env",
            ENV_NAME,
            "--persona-locality",
            "external",
            "--privacy",
            "unconstrained",
        ],
        None,
        &[(ENV_NAME, FIRST_KEY)],
    );
    assert_no_credential(dir.path(), &first, FIRST_KEY);
    success(first);
    let requests = server.requests();
    assert!(
        requests[0].authorization.as_deref() == Some(format!("Bearer {FIRST_KEY}").as_str()),
        "Authorization did not use the configured environment value"
    );
    assert!(
        !requests[0].body.contains(FIRST_KEY),
        "credential leaked into the prompt"
    );
    let configured = config_bytes(dir.path());
    let config: serde_json::Value = serde_json::from_slice(&configured).unwrap();
    assert_eq!(config["persona"]["provider"]["auth_env"], ENV_NAME);

    // The saved configuration retains the environment variable's name. A
    // different child environment supplies its current value at call time.
    let second = run_with_env(
        dir.path(),
        "talk",
        &[
            "--message",
            "API fixture next turn",
            "--privacy",
            "unconstrained",
        ],
        None,
        &[(ENV_NAME, NEXT_KEY)],
    );
    assert_no_credential(dir.path(), &second, FIRST_KEY);
    assert_no_credential(dir.path(), &second, NEXT_KEY);
    success(second);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].authorization.as_deref() == Some(format!("Bearer {NEXT_KEY}").as_str()),
        "the resumed process did not read the current environment value"
    );
    assert!(
        !requests[1].body.contains(FIRST_KEY) && !requests[1].body.contains(NEXT_KEY),
        "credential leaked into the resumed prompt"
    );
    assert_eq!(config_bytes(dir.path()), configured);
}

#[test]
fn api_endpoint_switch_does_not_forward_the_previous_endpoints_credential() {
    const ENV_NAME: &str = "KAMIMUSUHI_CLI_TEST_ORIGINAL_API_KEY";
    const KEY: &str = "dummy-kamimusuhi-original-endpoint-key-not-real";
    let dir = initialized();
    let original = FixtureServer::always(FixtureResponse::ok("元のendpoint。")).unwrap();
    let replacement = FixtureServer::always(FixtureResponse::ok("別のendpoint。")).unwrap();
    let first = run_with_env(
        dir.path(),
        "talk",
        &[
            "--message",
            "original endpoint",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &original.base_url(),
            "--persona-model",
            "api-fixture",
            "--persona-auth-env",
            ENV_NAME,
            "--persona-locality",
            "external",
            "--privacy",
            "unconstrained",
        ],
        None,
        &[(ENV_NAME, KEY)],
    );
    assert_no_credential(dir.path(), &first, KEY);
    success(first);
    assert!(
        original.requests()[0].authorization.as_deref() == Some(format!("Bearer {KEY}").as_str()),
        "original endpoint did not receive its configured credential"
    );

    let switched = run_with_env(
        dir.path(),
        "talk",
        &[
            "--message",
            "replacement endpoint",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &replacement.base_url(),
            "--persona-model",
            "api-fixture",
            "--persona-locality",
            "external",
            "--privacy",
            "unconstrained",
        ],
        None,
        &[(ENV_NAME, KEY)],
    );
    assert_no_credential(dir.path(), &switched, KEY);
    success(switched);
    assert_eq!(original.request_count(), 1);
    assert_eq!(replacement.request_count(), 1);
    assert!(
        replacement.requests()[0].authorization.is_none(),
        "a new endpoint inherited the previous endpoint's Authorization"
    );
    assert!(
        !replacement.requests()[0].body.contains(KEY),
        "credential leaked into the replacement prompt"
    );
    let config: serde_json::Value = serde_json::from_slice(&config_bytes(dir.path())).unwrap();
    assert_eq!(
        config["persona"]["provider"]["base_url"],
        replacement.base_url()
    );
    assert!(
        config["persona"]["provider"]
            .get("auth_env")
            .is_none_or(serde_json::Value::is_null)
    );
}

#[test]
fn api_key_like_environment_names_are_rejected_without_echoing_the_supplied_value() {
    let dir = initialized();
    let server = FixtureServer::always(FixtureResponse::ok("must not be reached")).unwrap();
    let config = config_bytes(dir.path());
    let trace = std::fs::read(dir.path().join("trace.jsonl")).unwrap();
    for invalid_name in [
        "sk-fixture-invalid-name-not-a-real-key",
        "API_KEY=dummy_fixture_value",
        "1INVALID_ENV",
    ] {
        let rejected = run(
            dir.path(),
            "talk",
            &[
                "--message",
                "must not dispatch",
                "--persona",
                "openai-compatible",
                "--persona-url",
                &server.base_url(),
                "--persona-model",
                "api-fixture",
                "--persona-auth-env",
                invalid_name,
                "--persona-locality",
                "external",
                "--privacy",
                "unconstrained",
            ],
            None,
        );
        assert_no_credential(dir.path(), &rejected, invalid_name);
        assert_eq!(rejected.status.code(), Some(2));
        assert!(rejected.stdout.is_empty());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("environment variable name"));
        assert_eq!(counts(dir.path()), [0, 0, 0, 0]);
        assert_eq!(config_bytes(dir.path()), config);
        assert_eq!(
            std::fs::read(dir.path().join("trace.jsonl")).unwrap(),
            trace
        );
    }
    assert_eq!(server.request_count(), 0);
}
