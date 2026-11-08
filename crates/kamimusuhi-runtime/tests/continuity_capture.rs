//! Real loopback transport, synthetic turns, no individual DB or paid API.
use std::path::Path;
use std::process::{Command, Output};

use kamimusuhi_core::ids::{NodeId, PersonaBackendId};
use kamimusuhi_core::routing::LocalityClass;
use kamimusuhi_runtime::ResourceImplementation;
use kamimusuhi_runtime::config::{
    PersonaBackendKind, PersonaProviderConfig, ReasoningSetting, RuntimeConfig,
};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

fn prepare(dir: &Path, server: &FixtureServer) -> Value {
    let mut config = RuntimeConfig::new(NodeId::from_u128(1), ResourceImplementation::FakeA);
    config.persona.backend = PersonaBackendKind::OpenaiCompatible;
    config.persona.provider = Some(PersonaProviderConfig {
        backend_id: PersonaBackendId::from_u128(2),
        locality: LocalityClass::LocalHost,
        base_url: server.base_url(),
        model: "capture-fixture".into(),
        auth_env: Some("CONTINUITY_CAPTURE_FIXTURE_KEY".into()),
        timeout_ms: 1000,
        tls_root_ca_path: None,
        system_instruction: None,
        reasoning: ReasoningSetting::Off,
        extra_body: None,
    });
    // A configured tool server must still never be offered by replay Persona.
    config.tools = Some(kamimusuhi_runtime::config::ToolServerSetting {
        base_url: server.base_url(),
        auth_env: None,
        max_rounds: 1,
        timeout_ms: 1000,
        allowed: vec!["*".into()],
        core: None,
    });
    std::fs::write(
        dir.join("runtime.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let suite = json!({"format_version":1,"suite_id":"synthetic-voice", "cases":[{
        "case_id":"greeting", "turn_input":{"context":{"individual_id":"00000000000000000000000000000011","session_id":"00000000000000000000000000000012","turn_id":"00000000000000000000000000000013"},"input":{"evidence_id":"00000000000000000000000000000014","text":"こんばんは"}},
        "required_text":["こんばんは"],"human_axes":["voice_continuity","relational_continuity"]
    }]});
    std::fs::write(dir.join("cases.json"), serde_json::to_vec(&suite).unwrap()).unwrap();
    suite
}
fn command(dir: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_continuity-capture"))
        .args([
            "--config",
            dir.join("runtime.json").to_str().unwrap(),
            "--cases",
            dir.join("cases.json").to_str().unwrap(),
            "--out",
            dir.join("paired.json").to_str().unwrap(),
            "--meta",
            dir.join("meta.json").to_str().unwrap(),
        ])
        .args(extra)
        .env(
            "CONTINUITY_CAPTURE_FIXTURE_KEY",
            "capture-token-never-in-artifact",
        )
        .output()
        .unwrap()
}

#[test]
fn capture_uses_same_seeded_turn_twice_without_tools_or_database_and_keeps_actual_model_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let server =
        FixtureServer::always(FixtureResponse::ok("こんばんは。おつかれさまです。")).unwrap();
    prepare(dir.path(), &server);
    let output = command(dir.path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.request_count(), 2);
    let pair_text = std::fs::read_to_string(dir.path().join("paired.json")).unwrap();
    let paired: Value = serde_json::from_str(&pair_text).unwrap();
    assert_eq!(paired["baseline"]["capture_kind"], "model_output");
    assert_eq!(
        paired["baseline"]["condition_digest"],
        paired["candidate"]["condition_digest"]
    );
    assert_eq!(
        paired["baseline"]["outputs"][0]["context_digest"],
        paired["candidate"]["outputs"][0]["context_digest"]
    );
    assert!(
        paired["baseline"]["outputs"][0]["human_reviews"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let meta_text = std::fs::read_to_string(dir.path().join("meta.json")).unwrap();
    let meta: Value = serde_json::from_str(&meta_text).unwrap();
    assert_eq!(meta["comparison_status"], "unverified");
    assert!(meta["captures"][0]["actual_model"].is_null());
    assert!(meta["captures"][0]["usage"].is_null());
    assert!(!pair_text.contains("capture-token-never-in-artifact"));
    assert!(!meta_text.contains("capture-token-never-in-artifact"));
    assert!(!dir.path().join("kamimusuhi.sqlite").exists());
    assert!(!dir.path().join("identity-manifest.json").exists());
    for request in server.requests() {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert!(body.get("tools").is_none());
        assert!(
            body["messages"].to_string().contains("PERSONA_SEED"),
            "configured seed must be applied before freezing context"
        );
    }
}

#[test]
fn malformed_unknown_or_duplicate_cases_are_refused_before_inference() {
    for mode in ["unknown", "duplicate", "empty-contract", "nil-context"] {
        let dir = tempfile::tempdir().unwrap();
        let server = FixtureServer::always(FixtureResponse::ok("must not be called")).unwrap();
        let mut suite = prepare(dir.path(), &server);
        match mode {
            "unknown" => suite["cases"][0]["turn_input"]["new_authority"] = json!(true),
            "duplicate" => {
                let c = suite["cases"][0].clone();
                suite["cases"].as_array_mut().unwrap().push(c);
            }
            "empty-contract" => {
                suite["cases"][0]["required_text"] = json!([]);
                suite["cases"][0]["human_axes"] = json!([]);
            }
            _ => {
                suite["cases"][0]["turn_input"]["context"]["individual_id"] =
                    json!("00000000000000000000000000000000")
            }
        }
        std::fs::write(
            dir.path().join("cases.json"),
            serde_json::to_vec(&suite).unwrap(),
        )
        .unwrap();
        assert!(!command(dir.path(), &[]).status.success(), "{mode}");
        assert_eq!(server.request_count(), 0, "{mode}");
        assert!(!dir.path().join("paired.json").exists());
    }
}

#[test]
fn logical_request_bound_and_existing_outputs_prevent_all_provider_calls() {
    for overwrite in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let server = FixtureServer::always(FixtureResponse::ok("must not be called")).unwrap();
        prepare(dir.path(), &server);
        if overwrite {
            std::fs::write(dir.path().join("paired.json"), b"preserve me").unwrap();
        }
        let flags = if overwrite {
            vec![]
        } else {
            vec!["--max-requests", "1"]
        };
        assert!(!command(dir.path(), &flags).status.success());
        assert_eq!(server.request_count(), 0);
        if overwrite {
            assert_eq!(
                std::fs::read(dir.path().join("paired.json")).unwrap(),
                b"preserve me"
            );
        }
    }
}

#[test]
fn failed_transport_never_publishes_a_complete_capture() {
    let dir = tempfile::tempdir().unwrap();
    let server = FixtureServer::always(FixtureResponse::Status { code: 503 }).unwrap();
    prepare(dir.path(), &server);
    assert!(!command(dir.path(), &[]).status.success());
    assert!(server.request_count() > 0);
    assert!(!dir.path().join("paired.json").exists());
    assert!(!dir.path().join("meta.json").exists());
}
