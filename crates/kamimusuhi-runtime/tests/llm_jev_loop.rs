//! Closed-loop wire coverage for the conversation-side K-CORE interface.
//!
//! The server is local and scripted: it proves request paths, typed Jev
//! payloads, state propagation and the ordering
//! Jev -> language organ -> Jev without making a live API claim.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_runtime::config::{PersonaBackendKind, PersonaProviderConfig, PersonaSetting};
use kamimusuhi_runtime::dialogue::{DialogueSession, MAX_INPUT_BYTES};
use kamimusuhi_runtime::llm_jev::{
    Decision, DecisionKind, DecisionProvider, DecisionRequest, DecisionResult, JevDecisionProvider,
    LanguageProvider, MockLanguageProvider, PersonaLanguageProvider, RuleBasedDecisionProvider,
    TypesafeConfig,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

fn raw_json(value: serde_json::Value) -> FixtureResponse {
    let body = value.to_string();
    FixtureResponse::RawHttp {
        response: format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    }
}

fn jev_choice(question: &str, choice: &str) -> FixtureResponse {
    let probabilities = match question {
        "invocation_gate" => serde_json::json!({
            "SPEAK": 0.93,
            "WAIT": 0.05,
            "OBSERVE_MORE": 0.02
        }),
        "response_gate" => serde_json::json!({
            "ACCEPT": 0.93,
            "RETRY": 0.05,
            "REJECT": 0.02
        }),
        other => panic!("unknown Jev fixture question {other}"),
    };
    raw_json(serde_json::json!({
        "answers": {
            question: {
                "type": "choice",
                "choice": choice,
                "probabilities": probabilities,
                "confidence": 0.91
            }
        }
    }))
}

fn prepared(server: &FixtureServer) -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(10),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut config = runtime.config().clone();
    config.persona = PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: PersonaBackendId::from_u128(0xC0DE),
            locality: LocalityClass::LocalHost,
            base_url: server.base_url(),
            model: "fixture-language".to_owned(),
            auth_env: None,
            timeout_ms: 2_000,
            tls_root_ca_path: None,
            system_instruction: None,
        }),
        seed: config.persona.seed.clone(),
    };
    runtime.save_config(config).unwrap();
    (dir, runtime)
}

fn systemone_base_url(server: &FixtureServer) -> String {
    format!("http://127.0.0.1:{}", server.port())
}

fn request_body(server: &FixtureServer, index: usize) -> serde_json::Value {
    serde_json::from_str(&server.requests()[index].body).unwrap()
}

#[test]
fn typed_jev_and_language_provider_form_a_two_turn_closed_loop() {
    let server = FixtureServer::start(vec![
        jev_choice("invocation_gate", "SPEAK"),
        FixtureResponse::ok("こんにちは。状態を確認しています。"),
        jev_choice("response_gate", "ACCEPT"),
        jev_choice("invocation_gate", "SPEAK"),
        FixtureResponse::ok("問題を確認しました。状況を整理します。"),
        jev_choice("response_gate", "ACCEPT"),
    ])
    .unwrap();
    let (_dir, mut runtime) = prepared(&server);
    let persona = runtime.config().build_persona().unwrap();
    let language_provider: Box<dyn LanguageProvider> = Box::new(PersonaLanguageProvider::new(
        persona,
        "fixture-language",
        "fixture-language",
    ));
    let decision_provider = Box::new(JevDecisionProvider::new(TypesafeConfig {
        base_url: systemone_base_url(&server),
        model: "jev-1.13.0".to_owned(),
        auth_env: String::new(),
        timeout_ms: 2_000,
    }));
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::Unconstrained,
        decision_provider,
        Some(language_provider),
    )
    .unwrap();
    session.set_debug_trace(true);

    let first = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let second = session
        .turn(&mut runtime, "エラーが起きた", |_| Ok(()))
        .unwrap();

    assert_eq!(first.response, "こんにちは。状態を確認しています。");
    assert_eq!(second.response, "問題を確認しました。状況を整理します。");
    assert_eq!(
        first.llm_jev.as_ref().unwrap().core_state_before.speech_act,
        "greeting"
    );
    assert_eq!(
        second
            .llm_jev
            .as_ref()
            .unwrap()
            .core_state_before
            .speech_act,
        "report_problem"
    );
    assert_eq!(
        second
            .llm_jev
            .as_ref()
            .unwrap()
            .core_state_after
            .last_action
            .as_deref(),
        Some("ACCEPT")
    );
    assert_eq!(server.request_count(), 6);

    let requests = server.requests();
    assert_eq!(requests[0].path, "/v1/systemone");
    assert_eq!(requests[1].path, "/v1/chat/completions");
    assert_eq!(requests[2].path, "/v1/systemone");
    assert_eq!(requests[3].path, "/v1/systemone");
    assert_eq!(requests[4].path, "/v1/chat/completions");
    assert_eq!(requests[5].path, "/v1/systemone");

    for (index, expected) in [(0, "greeting"), (3, "report_problem")] {
        let body = request_body(&server, index);
        assert_eq!(body["questions"].as_object().unwrap().len(), 1);
        let state: serde_json::Value =
            serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
        assert_eq!(state["speech_act"], expected);
        assert_eq!(state["state"]["speech_act"], expected);
        assert_eq!(body["questions"]["invocation_gate"]["type"], "choice");
    }
    for index in [1, 4] {
        let body = request_body(&server, index);
        let prompt = body["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains("[CONVERSATION_CORE_STATE]"));
        assert!(prompt.contains("speech_act"));
        assert!(prompt.len() <= MAX_INPUT_BYTES * 8);
    }
}

#[test]
fn mock_closed_loop_returns_core_state_to_the_next_turn_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(20),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();
    session.set_debug_trace(true);

    let greeting = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let problem = session
        .turn(&mut runtime, "エラーが起きた", |_| Ok(()))
        .unwrap();

    assert!(greeting.response.contains("こんにちは"));
    assert!(problem.response.contains("問題を確認しました"));
    assert_ne!(greeting.response, problem.response);
    assert_eq!(
        greeting.llm_jev.unwrap().invocation_gate.provider,
        "rule-based"
    );
    assert_eq!(
        problem.llm_jev.unwrap().core_state_before.speech_act,
        "report_problem"
    );
    assert_eq!(session.turn_count(), 2);
}

struct RetryOnceDecisionProvider {
    response_calls: std::sync::Mutex<u8>,
}

impl DecisionProvider for RetryOnceDecisionProvider {
    fn decide(
        &self,
        request: &DecisionRequest,
    ) -> Result<DecisionResult, kamimusuhi_runtime::llm_jev::ConversationError> {
        let decision = match request.kind {
            DecisionKind::InvocationGate => Decision::Speak,
            DecisionKind::ResponseGate => {
                let mut calls = self.response_calls.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    Decision::Retry
                } else {
                    Decision::Accept
                }
            }
        };
        let choices = request.kind.choices();
        let mut probabilities = BTreeMap::new();
        for choice in choices {
            probabilities.insert(
                (*choice).to_owned(),
                if *choice == decision.as_str() {
                    1.0
                } else {
                    0.0
                },
            );
        }
        Ok(DecisionResult {
            decision,
            confidence: 1.0,
            probabilities,
            provider: "retry-fixture".to_owned(),
            model: "retry-fixture-v0".to_owned(),
            latency_ms: 0,
            fallback: false,
            fallback_reason: None,
        })
    }
}

struct CountingMockLanguageProvider {
    calls: Arc<AtomicUsize>,
}

impl LanguageProvider for CountingMockLanguageProvider {
    fn generate(
        &self,
        request: &kamimusuhi_runtime::llm_jev::LanguageRequest,
    ) -> Result<
        kamimusuhi_runtime::llm_jev::LanguageResult,
        kamimusuhi_runtime::llm_jev::ConversationError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        MockLanguageProvider.generate(request)
    }

    fn descriptor(&self) -> kamimusuhi_core::persona::PersonaBackendDescriptor {
        MockLanguageProvider.descriptor()
    }
}

#[test]
fn response_retry_regenerates_once_and_binds_the_accepted_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(30),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(RetryOnceDecisionProvider {
            response_calls: std::sync::Mutex::new(0),
        }),
        Some(Box::new(CountingMockLanguageProvider {
            calls: Arc::clone(&calls),
        })),
    )
    .unwrap();
    session.set_debug_trace(true);

    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.retry_count, 1);
    assert_eq!(trace.response_gate.decision, Decision::Accept);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(trace.candidate_digest.len(), "sha256:".len() + 64);
}
