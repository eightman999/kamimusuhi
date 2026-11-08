//! The production CLI and its isolated provider children, with no Jev key.
//! Scripted loopback API replies prove orchestration and delivery boundaries;
//! they do not establish actual language-model judgment quality.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_persona_http::{OpenAiCompatiblePersona, PersonaBackendConfig};
use kamimusuhi_runtime::config::{
    DecisionSetting, PersonaBackendKind, PersonaProviderConfig, PersonaSetting, RuntimeConfig,
};
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, OpenAiDecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeError, RuntimeOptions};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");
const DECISION_PROVIDER: &str = "openai-compatible-decision";

fn run(dir: &Path, command: &str, flags: &[&str]) -> Output {
    let mut process = Command::new(BINARY);
    // A developer's real API configuration must not alter this fixture or
    // enroll additional organs. The sole endpoint is the local fixture.
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

fn configured(server: &FixtureServer, locality: LocalityClass) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    success(run(dir.path(), "init", &[]));
    let path = dir.path().join("runtime.json");
    let mut config: RuntimeConfig = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // Exercise the default automatic selection, rather than injecting a
    // DecisionProvider into DialogueSession or using the legacy rule gate.
    assert_eq!(config.decision, DecisionSetting::Auto);
    config.persona = PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: PersonaBackendId::from_u128(0xA91D),
            locality,
            base_url: server.base_url(),
            model: "api-decision-fixture".to_owned(),
            auth_env: None,
            timeout_ms: 2_000,
            tls_root_ca_path: None,
            system_instruction: None,
            reasoning: Default::default(),
            extra_body: None,
        }),
        seed: config.persona.seed,
    };
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    // Fixture-only explicit operator review; this is never automatic in the
    // product, and accepting the declaration must itself make no API call.
    let review = success(run(dir.path(), "identity-status", &[]));
    success(run(
        dir.path(),
        "identity-accept",
        &["--review-token", review["review_token"].as_str().unwrap()],
    ));
    assert_eq!(server.request_count(), 0);
    dir
}

fn talk(dir: &Path) -> Output {
    run(
        dir,
        "talk",
        &[
            "--subject",
            "api-fixture-user",
            "--message",
            "こんにちは",
            "--debug",
        ],
    )
}

fn choice(selected: &str, labels: &[&str]) -> Value {
    let probabilities: BTreeMap<_, _> = labels
        .iter()
        .map(|label| {
            (
                (*label).to_owned(),
                if *label == selected { 1.0 } else { 0.0 },
            )
        })
        .collect();
    json!({"type": "choice", "choice": selected, "confidence": 1.0, "probabilities": probabilities})
}

fn preparation() -> FixtureResponse {
    FixtureResponse::ok(json!({"answers": {
        "invocation_gate": choice("SPEAK", &["SPEAK", "WAIT", "OBSERVE_MORE"]),
        "observation_need": choice("NONE", &["NONE", "RECALL", "RUNTIME", "RESEARCH", "CLARIFY"]),
    }}).to_string())
}

fn assessment(grounding: &str) -> Value {
    json!({"answers": {
        "response_candidate": choice("candidate-0", &["candidate-0"]),
        "grounding_candidate-0": choice(grounding, &["SUPPORTED", "CONTRADICTED", "INSUFFICIENT", "NOT_APPLICABLE"]),
        "attribution_candidate-0": choice("CONSISTENT", &["CONSISTENT", "CONFLICT", "UNCLEAR", "NOT_APPLICABLE"]),
        "task_fit_candidate-0": choice("MET", &["MET", "UNMET", "UNCLEAR"]),
        // Raw ACCEPT with an insufficient grounding must still trigger the
        // host's bounded repair; the model cannot override that contract.
        "response_gate_candidate-0": choice("ACCEPT", &["ACCEPT", "RETRY", "REJECT"]),
        "repair_reason_candidate-0": choice("NONE", &["NONE", "GROUNDING", "ATTRIBUTION", "TASK_FIT", "LANGUAGE"]),
    }})
}

fn table_count(dir: &Path, table: &str) -> i64 {
    let connection = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn utterances(dir: &Path) -> Vec<Value> {
    let connection = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    let mut statement = connection
        .prepare("SELECT payload_json FROM evidence_records WHERE kind = 'agent_utterance'")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}

fn request_body(server: &FixtureServer, index: usize) -> Value {
    let requests = server.requests();
    assert_eq!(requests[index].path, "/v1/chat/completions");
    serde_json::from_str(&requests[index].body).unwrap()
}

fn packet(body: &Value) -> Value {
    serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap()
}

#[test]
fn api_only_auto_runs_preparation_generation_and_assessment_in_production_children() {
    let server = FixtureServer::start(vec![
        preparation(),
        FixtureResponse::ok("こんにちは。"),
        FixtureResponse::ok(assessment("NOT_APPLICABLE").to_string()),
    ])
    .unwrap();
    let dir = configured(&server, LocalityClass::LocalHost);
    let reply = success(talk(dir.path()));
    assert_eq!(reply["response"], "こんにちは。");
    let trace = &reply["llm_jev"];
    assert_eq!(trace["invocation_gate"]["provider"], DECISION_PROVIDER);
    assert_eq!(trace["response_gate"]["provider"], DECISION_PROVIDER);
    assert_eq!(trace["response_gate"]["decision"], "ACCEPT");
    assert_eq!(trace["assessments"][0]["grounding"], "NOT_APPLICABLE");
    assert!(trace.get("decision_fallbacks").is_none());
    assert_eq!(server.request_count(), 3);
    let prepare = request_body(&server, 0);
    let language = request_body(&server, 1);
    let assess = request_body(&server, 2);
    assert_eq!(prepare["response_format"]["type"], "json_object");
    assert!(
        packet(&prepare)["questions"]
            .get("invocation_gate")
            .is_some()
    );
    assert!(language.get("response_format").is_none());
    assert_eq!(assess["response_format"]["type"], "json_object");
    assert!(
        packet(&assess)["questions"]
            .get("response_candidate")
            .is_some()
    );
    for body in [&prepare, &assess] {
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
        assert!(body.get("tools").is_none());
    }
    let delivered = utterances(dir.path());
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["text"], reply["response"]);
}

#[test]
fn insufficient_api_judgment_repairs_once_then_delivers_only_the_accepted_response() {
    let server = FixtureServer::start(vec![
        preparation(),
        FixtureResponse::ok("未確認の断定。"),
        FixtureResponse::ok(assessment("INSUFFICIENT").to_string()),
        FixtureResponse::ok("こんにちは。"),
        FixtureResponse::ok(assessment("NOT_APPLICABLE").to_string()),
    ])
    .unwrap();
    let dir = configured(&server, LocalityClass::LocalHost);
    let reply = success(talk(dir.path()));
    assert_eq!(reply["response"], "こんにちは。");
    assert_eq!(reply["llm_jev"]["retry_count"], 1);
    let assessments = reply["llm_jev"]["assessments"].as_array().unwrap();
    assert_eq!(assessments.len(), 2);
    assert_eq!(assessments[0]["raw_gate"], "ACCEPT");
    assert_eq!(assessments[0]["gate"]["decision"], "RETRY");
    assert_eq!(assessments[1]["gate"]["decision"], "ACCEPT");
    assert_eq!(assessments[1]["attempt"], 1);
    assert_eq!(server.request_count(), 5);
    assert!(
        request_body(&server, 3)
            .to_string()
            .contains("RESPONSE_GUIDANCE")
    );
    let delivered = utterances(dir.path());
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["text"], "こんにちは。");
}

#[test]
fn invented_candidate_is_never_delivered_or_silently_accepted_by_rules() {
    let mut invalid = assessment("NOT_APPLICABLE");
    invalid["answers"]["response_candidate"]["choice"] = json!("invented-candidate");
    let server = FixtureServer::start(vec![
        preparation(),
        FixtureResponse::ok("承認されていない返事。"),
        FixtureResponse::ok(invalid.to_string()),
    ])
    .unwrap();
    let dir = configured(&server, LocalityClass::LocalHost);
    let output = talk(dir.path());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        server.request_count(),
        4,
        "one bounded validation repair only"
    );
    assert!(utterances(dir.path()).is_empty());
    assert_eq!(table_count(dir.path(), "state_records"), 0);
}

#[test]
fn malformed_preparation_stops_after_one_repair_before_language_generation() {
    let server = FixtureServer::always(FixtureResponse::ok("UNTRUSTED_NON_JSON")).unwrap();
    let dir = configured(&server, LocalityClass::LocalHost);
    let output = talk(dir.path());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(server.request_count(), 2);
    assert!(utterances(dir.path()).is_empty());
    let repair = request_body(&server, 1).to_string();
    assert!(repair.contains("MALFORMED"));
    assert!(!repair.contains("UNTRUSTED_NON_JSON"));
}

#[test]
fn local_only_refuses_external_api_before_session_or_request() {
    let server = FixtureServer::always(preparation()).unwrap();
    let dir = configured(&server, LocalityClass::External);
    let output = talk(dir.path());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(server.request_count(), 0);
    assert_eq!(table_count(dir.path(), "sessions"), 0);
    assert_eq!(table_count(dir.path(), "turns"), 0);
    assert_eq!(table_count(dir.path(), "evidence_records"), 0);
}

#[test]
fn local_only_rejects_a_network_decision_provider_even_with_a_local_language_organ() {
    let server = FixtureServer::always(preparation()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(0xA91D),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let decision = OpenAiDecisionProvider::new(
        OpenAiCompatiblePersona::new(PersonaBackendConfig::new(
            PersonaBackendId::from_u128(0xA91D),
            server.base_url(),
            "network-decision-fixture",
        )),
        LocalityClass::LocalNetwork,
    );
    let result = DialogueSession::start_with_providers(
        &mut runtime,
        "api-fixture-user",
        PrivacyConstraint::LocalOnly,
        Box::new(decision),
        Some(Box::new(MockLanguageProvider)),
    );
    assert!(matches!(
        result,
        Err(RuntimeError::PersonaPrivacy {
            privacy: PrivacyConstraint::LocalOnly,
            locality: LocalityClass::LocalNetwork,
        })
    ));
    assert_eq!(server.request_count(), 0);
    assert_eq!(table_count(dir.path(), "sessions"), 0);
    assert_eq!(table_count(dir.path(), "evidence_records"), 0);
}
