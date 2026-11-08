//! Input-free cognition through the real CLI and loopback reflection API.
//! Fixtures establish isolation, restart behavior and evidence boundaries;
//! they make no claim about actual model reflection quality.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, PersonaBackendId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::routing::LocalityClass;
use kamimusuhi_runtime::config::{
    PersonaBackendKind, PersonaProviderConfig, PersonaSetting, RuntimeConfig,
};
use kamimusuhi_runtime::{Runtime, RuntimeOptions};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

fn run(dir: &Path, command: &str, flags: &[&str]) -> Output {
    let mut process = Command::new(BINARY);
    for name in [
        "TYPESAFE_API_KEY",
        "TYPESAFE_BASE_URL",
        "TYPESAFE_DEFAULT_MODEL",
        "KAMIMUSUHI_LLM_PROVIDER",
        "KAMIMUSUHI_LLM_BASE_URL",
        "KAMIMUSUHI_LLM_MODEL",
        "KAMIMUSUHI_LLM_AUTH_ENV",
        "KAMIMUSUHI_IDENTITY_DECLARATIONS",
        "HAI_API_KEY",
        "GEMMA_API_KEY",
        "GBVM_API_KEY",
    ] {
        process.env_remove(name);
    }
    process
        .arg(command)
        .arg("--dir")
        .arg(dir)
        .args(flags)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn configured(
    server: &FixtureServer,
    locality: LocalityClass,
    accepted: bool,
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    success(run(dir.path(), "init", &[]));
    let path = dir.path().join("runtime.json");
    let mut config: RuntimeConfig = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config.persona = PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: PersonaBackendId::from_u128(0x1D1E),
            locality,
            base_url: server.base_url(),
            model: "idle-reflection-fixture".to_owned(),
            auth_env: None,
            timeout_ms: 2000,
            tls_root_ca_path: None,
            system_instruction: None,
            reasoning: Default::default(),
            extra_body: None,
        }),
        seed: config.persona.seed,
    };
    config.c0.idle.idle_after_secs = 0;
    config.c0.idle.min_interval_secs = 1;
    config.c0.idle.retry_after_secs = 60;
    std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    if accepted {
        let review = success(run(dir.path(), "identity-status", &[]));
        success(run(
            dir.path(),
            "identity-accept",
            &["--review-token", review["review_token"].as_str().unwrap()],
        ));
    }
    assert_eq!(server.request_count(), 0);
    dir
}

fn experience(dir: &Path, subject: &str, id: u128, text: &str) {
    let runtime = Runtime::open(dir, RuntimeOptions::deterministic(0x1D1E)).unwrap();
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: EvidenceId::from_u128(id),
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Observed,
            payload: json!({"text":text}),
            source: EvidenceSource {
                source_id: Some(format!("text-chat:{subject}")),
                ..Default::default()
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
}

fn cogitate(dir: &Path, subject: &str) -> Output {
    run(
        dir,
        "cogitate",
        &["--subject", subject, "--privacy", "local-only"],
    )
}

fn response() -> FixtureResponse {
    FixtureResponse::ok(json!({"notes":["これは未配信の内省fixture。"], "evidence_corrections":[], "canonical_drafts":[], "improvement_drafts":[]}).to_string())
}

fn counts(dir: &Path) -> [i64; 5] {
    let connection = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    let count = |sql: &str| connection.query_row(sql, [], |row| row.get(0)).unwrap();
    [
        count("SELECT count(*) FROM sessions"),
        count("SELECT count(*) FROM turns"),
        count("SELECT count(*) FROM evidence_records WHERE kind='user_utterance'"),
        count("SELECT count(*) FROM evidence_records WHERE kind='agent_utterance'"),
        count("SELECT count(*) FROM evidence_records WHERE kind='reflection'"),
    ]
}

#[test]
fn cogitate_reflects_once_without_synthetic_turns_and_consumes_source_across_processes() {
    let server = FixtureServer::always(response()).unwrap();
    let dir = configured(&server, LocalityClass::LocalHost, true);
    experience(dir.path(), "alice", 0xA11CE, "ALICE_PRIVATE_EXPERIENCE");
    let before = counts(dir.path());
    let first = success(cogitate(dir.path(), "alice"));
    assert_eq!(first["status"], "completed");
    assert_eq!(first["reason"], "REFLECTED");
    assert_eq!(first["reflection"]["episodes_presented"], 1);
    assert_eq!(server.request_count(), 1);
    let after = counts(dir.path());
    assert_eq!(
        after[..4],
        before[..4],
        "idle cognition must not create or deliver a user/assistant turn"
    );
    assert_eq!(after[4], before[4] + 1);
    assert!(dir.path().join("idle-cognition.json").is_file());
    for _ in 0..2 {
        let reopened = success(cogitate(dir.path(), "alice"));
        assert_eq!(reopened["status"], "idle");
        assert_eq!(reopened["reason"], "NO_NEW_EXPERIENCE");
        assert_eq!(reopened["source_digest"], first["source_digest"]);
    }
    assert_eq!(
        server.request_count(),
        1,
        "the emitted Reflection must not wake itself"
    );
    assert_eq!(counts(dir.path()), after);
    let requests = server.requests();
    assert_eq!(requests[0].path, "/v1/chat/completions");
    let body: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
    assert!(body.get("tools").is_none());
}

#[test]
fn another_subjects_experience_is_not_sent_or_used_to_wake_this_subject() {
    let server = FixtureServer::always(response()).unwrap();
    let dir = configured(&server, LocalityClass::LocalHost, true);
    experience(dir.path(), "alice", 0xA11CE, "ALICE_ONLY_PRIVATE_TEXT");
    experience(
        dir.path(),
        "bob",
        0xB0B,
        "BOB_MUST_NOT_REACH_ALICE_REFLECTION",
    );
    assert_eq!(
        success(cogitate(dir.path(), "alice"))["status"],
        "completed"
    );
    let body = &server.requests()[0].body;
    assert!(body.contains("ALICE_ONLY_PRIVATE_TEXT"));
    assert!(!body.contains("BOB_MUST_NOT_REACH_ALICE_REFLECTION"));
    experience(dir.path(), "bob", 0xB0B2, "BOB_NEWER_PRIVATE_TEXT");
    assert_eq!(
        success(cogitate(dir.path(), "alice"))["reason"],
        "NO_NEW_EXPERIENCE"
    );
    assert_eq!(
        success(cogitate(dir.path(), "nobody"))["reason"],
        "NO_EXPERIENCE"
    );
    assert_eq!(server.request_count(), 1);
    let checkpoint = std::fs::read_to_string(dir.path().join("idle-cognition.json")).unwrap();
    for text in [
        "ALICE_ONLY_PRIVATE_TEXT",
        "BOB_MUST_NOT_REACH_ALICE_REFLECTION",
        "BOB_NEWER_PRIVATE_TEXT",
    ] {
        assert!(!checkpoint.contains(text));
    }
}

#[test]
fn unreviewed_api_identity_is_refused_before_reflection_or_checkpoint_admission() {
    let server = FixtureServer::always(response()).unwrap();
    let dir = configured(&server, LocalityClass::LocalHost, false);
    experience(dir.path(), "alice", 0xA11CE, "NEVER_SENT_UNREVIEWED_SOURCE");
    let before = counts(dir.path());
    let result = cogitate(dir.path(), "alice");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("identity change requires explicit acceptance")
    );
    assert_eq!(server.request_count(), 0);
    assert_eq!(counts(dir.path()), before);
    assert!(!dir.path().join("idle-cognition.json").exists());
}

#[test]
fn local_only_refuses_external_reflector_before_sending_or_admitting_a_run() {
    let server = FixtureServer::always(response()).unwrap();
    let dir = configured(&server, LocalityClass::External, true);
    experience(dir.path(), "alice", 0xA11CE, "NEVER_SENT_PRIVATE_SOURCE");
    let before = counts(dir.path());
    let result = cogitate(dir.path(), "alice");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(server.request_count(), 0);
    assert_eq!(counts(dir.path()), before);
    assert!(!dir.path().join("idle-cognition.json").exists());
}

#[test]
fn malformed_reflection_repairs_once_then_records_failure_and_backs_off() {
    let server = FixtureServer::always(FixtureResponse::ok("NOT_A_REFLECTION_OBJECT")).unwrap();
    let dir = configured(&server, LocalityClass::LocalHost, true);
    experience(
        dir.path(),
        "alice",
        0xA11CE,
        "ALICE_SOURCE_FOR_FAILED_REFLECTION",
    );
    let before = counts(dir.path());
    let result = cogitate(dir.path(), "alice");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(server.request_count(), 2);
    assert_eq!(counts(dir.path()), before);
    let again = success(cogitate(dir.path(), "alice"));
    assert_eq!(again["status"], "cooldown");
    assert!(["MIN_INTERVAL", "RETRY_BACKOFF"].contains(&again["reason"].as_str().unwrap()));
    assert_eq!(server.request_count(), 2);
    let checkpoint: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("idle-cognition.json")).unwrap())
            .unwrap();
    assert_eq!(checkpoint["subjects"]["alice"]["attempts"], 1);
    assert_eq!(checkpoint["subjects"]["alice"]["in_flight"], false);
}
