//! Closed-loop wire coverage for the conversation-side K-CORE interface.
//!
//! The server is local and scripted: it proves request paths, typed Jev
//! payloads, state propagation and the ordering
//! Jev -> language organ -> Jev without making a live API claim.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kamimusuhi_core::evidence::EvidenceStore;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_core::trace::TraceEventKind;
use kamimusuhi_runtime::config::{PersonaBackendKind, PersonaProviderConfig, PersonaSetting};
use kamimusuhi_runtime::dialogue::{DialogueSession, MAX_INPUT_BYTES};
use kamimusuhi_runtime::llm_jev::{
    ConversationCoreState, Decision, DecisionKind, DecisionProvider, DecisionRequest,
    DecisionResult, FallbackDecisionProvider, JevDecisionProvider, LanguageProvider,
    LanguageProviderCandidate, LanguageProviderTelemetry, LanguageResponseCandidate,
    LanguageResponseSelectionRequest, MAX_JEV_CANDIDATE_RESPONSE_BYTES, MockLanguageProvider,
    PersonaLanguageProvider, ProviderSelectionRequest, RuleBasedDecisionProvider, TypesafeConfig,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, read_trace};
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

fn typed_choice(choice: &str, choices: &[&str]) -> serde_json::Value {
    let probabilities = choices
        .iter()
        .map(|candidate| {
            (
                (*candidate).to_owned(),
                if *candidate == choice { 1.0 } else { 0.0 },
            )
        })
        .collect::<BTreeMap<_, _>>();
    serde_json::json!({
        "type": "choice",
        "choice": choice,
        "probabilities": probabilities,
        "confidence": 1.0,
    })
}

fn jev_preparation(choice: &str) -> FixtureResponse {
    jev_preparation_with_observation(choice, "NONE")
}

fn jev_preparation_with_observation(choice: &str, observation: &str) -> FixtureResponse {
    raw_json(serde_json::json!({"answers": {
        "invocation_gate": typed_choice(choice, &["SPEAK", "WAIT", "OBSERVE_MORE"]),
        "observation_need": typed_choice(observation, &["NONE", "RECALL", "RUNTIME", "RESEARCH", "CLARIFY"]),
    }}))
}

fn anonymous_candidate_id(id: &str) -> String {
    match id {
        "primary" => "candidate-0".to_owned(),
        "backup" => "candidate-1".to_owned(),
        other => other.to_owned(),
    }
}

fn typed_choice_strings(choice: &str, choices: &[String]) -> serde_json::Value {
    let probabilities = choices
        .iter()
        .map(|candidate| {
            (
                candidate.clone(),
                if candidate == choice { 1.0 } else { 0.0 },
            )
        })
        .collect::<BTreeMap<_, _>>();
    serde_json::json!({
        "type": "choice",
        "choice": choice,
        "probabilities": probabilities,
        "confidence": 1.0,
    })
}

fn assessment_answers(selected: &str, candidates: &[(&str, &str)]) -> serde_json::Value {
    let candidates = candidates
        .iter()
        .map(|(id, gate)| (anonymous_candidate_id(id), *gate))
        .collect::<Vec<_>>();
    let ids = candidates
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let selected = anonymous_candidate_id(selected);
    let mut selected_answer = typed_choice_strings(
        if ids.contains(&selected) {
            &selected
        } else {
            &ids[0]
        },
        &ids,
    );
    selected_answer["choice"] = serde_json::json!(selected);
    let mut answers = serde_json::Map::new();
    answers.insert("response_candidate".to_owned(), selected_answer);
    for (id, gate) in &candidates {
        answers.insert(
            format!("grounding_{id}"),
            typed_choice(
                if *gate == "ACCEPT" {
                    "SUPPORTED"
                } else {
                    "INSUFFICIENT"
                },
                &[
                    "SUPPORTED",
                    "CONTRADICTED",
                    "INSUFFICIENT",
                    "NOT_APPLICABLE",
                ],
            ),
        );
        answers.insert(
            format!("attribution_{id}"),
            typed_choice(
                "CONSISTENT",
                &["CONSISTENT", "CONFLICT", "UNCLEAR", "NOT_APPLICABLE"],
            ),
        );
        answers.insert(
            format!("task_fit_{id}"),
            typed_choice("MET", &["MET", "UNMET", "UNCLEAR"]),
        );
        answers.insert(
            format!("response_gate_{id}"),
            typed_choice(gate, &["ACCEPT", "RETRY", "REJECT"]),
        );
        answers.insert(
            format!("repair_reason_{id}"),
            typed_choice(
                if *gate == "ACCEPT" {
                    "NONE"
                } else {
                    "GROUNDING"
                },
                &["NONE", "GROUNDING", "ATTRIBUTION", "TASK_FIT", "LANGUAGE"],
            ),
        );
    }
    serde_json::json!({"answers": answers})
}

fn jev_assessment(selected: &str, candidates: &[(&str, &str)]) -> FixtureResponse {
    raw_json(assessment_answers(selected, candidates))
}

fn jev_response_candidate_choice(choice: &str, candidates: &[&str]) -> FixtureResponse {
    let selected_probability = if candidates.len() == 1 { 1.0 } else { 0.93 };
    let remainder =
        (1.0 - selected_probability) / (candidates.len().saturating_sub(1).max(1) as f64);
    let probabilities = candidates
        .iter()
        .map(|candidate| {
            (
                (*candidate).to_owned(),
                if *candidate == choice {
                    selected_probability
                } else {
                    remainder
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    raw_json(serde_json::json!({
        "answers": {
            "response_candidate": {
                "type": "choice",
                "choice": choice,
                "probabilities": probabilities,
                "confidence": 0.93
            }
        }
    }))
}

fn jev_provider_choice(choice: &str) -> FixtureResponse {
    raw_json(serde_json::json!({
        "answers": {
            "language_provider": {
                "type": "choice",
                "choice": choice,
                "probabilities": {
                    "primary": 0.05,
                    "backup": 0.93
                },
                "confidence": 0.93
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

fn prompt_section(prompt: &str, name: &str) -> serde_json::Value {
    let marker = format!("[{name}]\n");
    let section = prompt.split_once(&marker).unwrap().1;
    let payload = section
        .split_once("\n[")
        .map_or(section, |(value, _)| value);
    serde_json::from_str(payload.trim()).unwrap()
}

fn fanout_session(
    primary: &FixtureServer,
    backup: &FixtureServer,
    judge: &FixtureServer,
) -> (tempfile::TempDir, Runtime, DialogueSession) {
    let (dir, mut runtime) = prepared(primary);
    let mut config = runtime.config().clone();
    config
        .set_language_provider(
            "backup",
            PersonaProviderConfig {
                backend_id: PersonaBackendId::from_u128(0xBEEF),
                locality: LocalityClass::LocalHost,
                base_url: backup.base_url(),
                model: "backup-language".to_owned(),
                auth_env: None,
                timeout_ms: 2_000,
                tls_root_ca_path: None,
                system_instruction: None,
            },
        )
        .unwrap();
    runtime.save_config(config).unwrap();
    let mut session = DialogueSession::start_with_decision_provider(
        &mut runtime,
        "alice",
        PrivacyConstraint::Unconstrained,
        Box::new(FallbackDecisionProvider::new(Box::new(
            JevDecisionProvider::new(TypesafeConfig {
                base_url: systemone_base_url(judge),
                model: "jev-1.13.0".to_owned(),
                auth_env: String::new(),
                timeout_ms: 2_000,
            }),
        ))),
    )
    .unwrap();
    session.set_debug_trace(true);
    (dir, runtime, session)
}

#[test]
fn failed_or_oversized_organs_are_excluded_without_losing_healthy_candidates() {
    for (failure, code) in [
        (FixtureResponse::Status { code: 503 }, "HTTP_STATUS"),
        (
            FixtureResponse::ok("x".repeat(MAX_JEV_CANDIDATE_RESPONSE_BYTES + 1)),
            "RESPONSE_TOO_LARGE",
        ),
        (FixtureResponse::WrongShape, "MALFORMED_RESPONSE"),
    ] {
        let primary = FixtureServer::always(failure).unwrap();
        let backup = FixtureServer::always(FixtureResponse::ok("healthy response")).unwrap();
        let judge = FixtureServer::start(vec![
            jev_preparation("SPEAK"),
            jev_assessment("backup", &[("backup", "ACCEPT")]),
        ])
        .unwrap();
        let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        let reply = session
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap();
        assert_eq!(reply.response, "healthy response");
        let trace = reply.llm_jev.unwrap();
        assert_eq!(trace.provider_telemetry["primary"].failures, 1);
        assert_eq!(trace.provider_telemetry["backup"].successes, 1);
        let failed = trace
            .generated_candidates
            .iter()
            .find(|candidate| candidate.id == "primary")
            .unwrap();
        assert_eq!(failed.error_code.as_deref(), Some(code));
        let state: serde_json::Value =
            serde_json::from_str(request_body(&judge, 1)["state"].as_str().unwrap()).unwrap();
        assert_eq!(judge.request_count(), 2);
        assert_eq!(
            state["selection"]["candidates"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            state["selection"]["candidates"][0]["response"],
            "healthy response"
        );
    }
}

#[test]
fn all_failures_stop_before_selection_and_remain_inspectable() {
    let primary = FixtureServer::always(FixtureResponse::Status { code: 503 }).unwrap();
    let backup = FixtureServer::always(FixtureResponse::WrongShape).unwrap();
    let judge = FixtureServer::always(jev_preparation("SPEAK")).unwrap();
    let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
    assert!(
        session
            .turn(&mut runtime, "こんにちは", |_| panic!(
                "must not deliver"
            ))
            .is_err()
    );
    assert_eq!(judge.request_count(), 1);
    assert_eq!(session.last_generated_candidates().len(), 2);
    assert!(
        session
            .last_generated_candidates()
            .iter()
            .all(|candidate| candidate.error_code.is_some())
    );
    assert!(
        session
            .history_for_display(&runtime)
            .unwrap()
            .iter()
            .all(|message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant)
    );
}

#[test]
fn gate_rejection_and_jev_failures_never_deliver_another_candidate() {
    for script in [
        vec![
            jev_preparation("SPEAK"),
            FixtureResponse::Status { code: 503 },
        ],
        vec![
            jev_preparation("SPEAK"),
            jev_assessment("backup", &[("primary", "ACCEPT"), ("backup", "REJECT")]),
        ],
        vec![
            jev_preparation("SPEAK"),
            jev_assessment("backup", &[("primary", "REJECT"), ("backup", "REJECT")]),
        ],
        vec![
            jev_preparation("SPEAK"),
            jev_assessment("unknown", &[("primary", "ACCEPT"), ("backup", "ACCEPT")]),
        ],
    ] {
        let primary = FixtureServer::always(FixtureResponse::ok("primary response")).unwrap();
        let backup = FixtureServer::always(FixtureResponse::ok("backup response")).unwrap();
        let judge = FixtureServer::start(script).unwrap();
        let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        assert!(
            session
                .turn(&mut runtime, "こんにちは", |_| panic!(
                    "must not deliver"
                ))
                .is_err()
        );
        assert_eq!(primary.request_count(), 1);
        assert_eq!(backup.request_count(), 1);
        assert_eq!(session.last_generated_candidates().len(), 2);
        assert!(
            session.history_for_display(&runtime).unwrap().iter().all(
                |message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant
            )
        );
    }
}

#[test]
fn invalid_batch_answers_are_repaired_once_before_any_response_is_delivered() {
    let valid = assessment_answers("primary", &[("primary", "ACCEPT"), ("backup", "ACCEPT")]);
    let mut cases = Vec::new();

    let mut missing = valid.clone();
    missing["answers"]
        .as_object_mut()
        .unwrap()
        .remove("grounding_candidate-1");
    cases.push(("missing unselected candidate assessment", missing));

    let mut out_of_range = valid.clone();
    out_of_range["answers"]["response_gate_2"] =
        typed_choice("ACCEPT", &["ACCEPT", "RETRY", "REJECT"]);
    cases.push(("out-of-range question", out_of_range));

    let mut unknown = valid.clone();
    unknown["answers"]["unexpected"] = typed_choice("MET", &["MET", "UNMET", "UNCLEAR"]);
    cases.push(("unknown question", unknown));

    let mut confidence = valid.clone();
    confidence["answers"]["grounding_candidate-1"]["confidence"] = serde_json::json!(1.1);
    cases.push(("out-of-range confidence", confidence));

    let mut missing_probability = valid.clone();
    missing_probability["answers"]["attribution_candidate-1"]["probabilities"]
        .as_object_mut()
        .unwrap()
        .remove("UNCLEAR");
    cases.push(("incomplete probabilities", missing_probability));

    let mut extra_probability = valid.clone();
    extra_probability["answers"]["task_fit_candidate-0"]["probabilities"]["UNKNOWN"] = serde_json::json!(0.0);
    cases.push(("unknown probability key", extra_probability));

    let mut probability_sum = valid.clone();
    probability_sum["answers"]["task_fit_candidate-0"]["probabilities"]["MET"] = serde_json::json!(0.5);
    cases.push(("invalid probability sum", probability_sum));

    for (case, invalid) in cases {
        for recover in [true, false] {
            let primary = FixtureServer::always(FixtureResponse::ok("primary response")).unwrap();
            let backup = FixtureServer::always(FixtureResponse::ok("backup response")).unwrap();
            let judge = FixtureServer::start(vec![
                jev_preparation("SPEAK"),
                raw_json(invalid.clone()),
                raw_json(if recover {
                    valid.clone()
                } else {
                    invalid.clone()
                }),
            ])
            .unwrap();
            let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
            let delivered = AtomicUsize::new(0);
            let result = session.turn(&mut runtime, "こんにちは", |_| {
                delivered.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            assert_eq!(result.is_ok(), recover, "{case}");
            assert_eq!(
                delivered.load(Ordering::SeqCst),
                usize::from(recover),
                "{case}"
            );
            assert_eq!(primary.request_count(), 1, "{case}");
            assert_eq!(backup.request_count(), 1, "{case}");
            assert_eq!(judge.request_count(), 3, "{case}");
            let original = request_body(&judge, 1);
            let repaired = request_body(&judge, 2);
            assert_eq!(original["state"], repaired["state"], "{case}");
            assert!(
                repaired["questions"]
                    .to_string()
                    .contains("INVALID_DECISION"),
                "{case}"
            );
            if !recover {
                assert!(
                    session
                        .history_for_display(&runtime)
                        .unwrap()
                        .iter()
                        .all(|message| message.role
                            != kamimusuhi_core::persona::ConversationRole::Assistant),
                    "{case}"
                );
            }
        }
    }
}

#[test]
fn accept_never_bypasses_an_unsuitable_selected_candidate() {
    for (question, choice, choices) in [
        (
            "grounding_candidate-1",
            "CONTRADICTED",
            &[
                "SUPPORTED",
                "CONTRADICTED",
                "INSUFFICIENT",
                "NOT_APPLICABLE",
            ][..],
        ),
        (
            "attribution_candidate-1",
            "CONFLICT",
            &["CONSISTENT", "CONFLICT", "UNCLEAR", "NOT_APPLICABLE"][..],
        ),
        ("task_fit_candidate-1", "UNMET", &["MET", "UNMET", "UNCLEAR"][..]),
    ] {
        let mut answer =
            assessment_answers("backup", &[("primary", "ACCEPT"), ("backup", "ACCEPT")]);
        answer["answers"][question] = typed_choice(choice, choices);
        let primary =
            FixtureServer::always(FixtureResponse::ok("suitable primary response")).unwrap();
        let backup =
            FixtureServer::always(FixtureResponse::ok("unsuitable selected response")).unwrap();
        let judge = FixtureServer::start(vec![jev_preparation("SPEAK"), raw_json(answer)]).unwrap();
        let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        assert!(
            session
                .turn(&mut runtime, "こんにちは", |_| panic!(
                    "must not deliver"
                ))
                .is_err(),
            "{question}"
        );
        assert_eq!(primary.request_count(), 1, "{question}");
        assert!(backup.request_count() <= 2, "{question}");
        assert!(judge.request_count() <= 3, "{question}");
        assert!(
            session.history_for_display(&runtime).unwrap().iter().all(
                |message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant
            ),
            "{question}"
        );
    }
}

#[test]
fn invocation_wait_or_failure_does_not_launch_any_language_organ() {
    for answer in [
        jev_preparation("WAIT"),
        jev_preparation("OBSERVE_MORE"),
        FixtureResponse::Status { code: 503 },
    ] {
        let primary = FixtureServer::always(FixtureResponse::ok("unused")).unwrap();
        let backup = FixtureServer::always(FixtureResponse::ok("unused")).unwrap();
        let judge = FixtureServer::always(answer).unwrap();
        let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        assert!(
            session
                .turn(&mut runtime, "こんにちは", |_| panic!(
                    "must not deliver"
                ))
                .is_err()
        );
        assert_eq!(primary.request_count(), 0);
        assert_eq!(backup.request_count(), 0);
        assert!(session.last_generated_candidates().is_empty());
    }
}

#[test]
fn missing_observation_requests_one_clarification_without_repeating_the_judge_loop() {
    let primary =
        FixtureServer::always(FixtureResponse::ok("どの対象について確認しますか？")).unwrap();
    let backup = FixtureServer::always(FixtureResponse::ok("対象を教えてください。")).unwrap();
    let judge = FixtureServer::start(vec![
        jev_preparation_with_observation("OBSERVE_MORE", "CLARIFY"),
        jev_assessment("primary", &[("primary", "ACCEPT"), ("backup", "ACCEPT")]),
    ])
    .unwrap();
    let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
    let reply = session
        .turn(&mut runtime, "それについて教えて", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "どの対象について確認しますか？");
    assert_eq!(judge.request_count(), 2);
    assert_eq!(primary.request_count(), 1);
    assert_eq!(backup.request_count(), 1);
    let generation = request_body(&primary, 0);
    let prompt = generation["messages"][1]["content"].as_str().unwrap();
    let guidance = prompt_section(prompt, "RESPONSE_GUIDANCE");
    assert_eq!(guidance["reason_code"], "OBSERVE_CLARIFY");
    assert!(!guidance["instruction"].as_str().unwrap().is_empty());
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.retry_count, 0);
    assert_eq!(trace.invocation_gate.decision, Decision::ObserveMore);
}

#[test]
fn retry_replaces_one_candidate_and_reselects_without_a_third_generation() {
    for (final_provider, final_choice) in [
        ("backup", "ACCEPT"),
        ("primary", "ACCEPT"),
        ("backup", "RETRY"),
    ] {
        let primary = FixtureServer::start(vec![
            FixtureResponse::ok("old response"),
            FixtureResponse::ok("replacement response"),
        ])
        .unwrap();
        let backup = FixtureServer::always(FixtureResponse::ok("backup response")).unwrap();
        let judge = FixtureServer::start(vec![
            jev_preparation("SPEAK"),
            jev_assessment("primary", &[("primary", "RETRY"), ("backup", "ACCEPT")]),
            jev_assessment(
                final_provider,
                &[
                    (
                        "primary",
                        if final_provider == "primary" {
                            final_choice
                        } else {
                            "ACCEPT"
                        },
                    ),
                    (
                        "backup",
                        if final_provider == "backup" {
                            final_choice
                        } else {
                            "ACCEPT"
                        },
                    ),
                ],
            ),
        ])
        .unwrap();
        let (dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        session.set_debug_context(true);
        let delivered = AtomicUsize::new(0);
        let reply = session.turn(&mut runtime, "こんにちは", |_| {
            delivered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        if final_choice == "ACCEPT" {
            let reply = reply.unwrap();
            assert_eq!(
                reply.response,
                if final_provider == "primary" {
                    "replacement response"
                } else {
                    "backup response"
                }
            );
            let trace = reply.llm_jev.as_ref().unwrap();
            assert_eq!(trace.retry_count, 1);
            assert_eq!(trace.language_provider_id, final_provider);
            assert_eq!(delivered.load(Ordering::SeqCst), 1);
            let debug_context = reply.debug_context.as_ref().unwrap();
            assert_eq!(session.last_context(), Some(debug_context));
            let events = read_trace(dir.path().join("trace.jsonl")).unwrap();
            let invocation = events
                .iter()
                .find(|event| event.event_kind == TraceEventKind::PersonaInvoked)
                .unwrap();
            let generated = runtime
                .store()
                .get(reply.generated_evidence_id)
                .unwrap()
                .unwrap();
            assert_eq!(generated.payload["event"], "response_generated");
            assert_eq!(
                debug_context["generation_input_digest"],
                generated.payload["input_digest"]
            );
            if final_provider == "primary" {
                let retry_generation = request_body(&primary, 1);
                let retry_prompt = retry_generation["messages"][1]["content"].as_str().unwrap();
                let guidance = prompt_section(retry_prompt, "RESPONSE_GUIDANCE");
                assert_eq!(debug_context["response_guidance"], guidance);
                assert_eq!(
                    debug_context["response_guidance"]["previous_response_digest"],
                    kamimusuhi_core::digest::content_digest(b"old response")
                );
                assert_ne!(
                    debug_context["generation_input_digest"],
                    invocation.detail["input_digest"]
                );
            } else {
                assert!(debug_context["response_guidance"].is_null());
                assert_eq!(
                    debug_context["generation_input_digest"],
                    invocation.detail["input_digest"]
                );
            }
            let assessment: serde_json::Value =
                serde_json::from_str(request_body(&judge, 2)["state"].as_str().unwrap()).unwrap();
            assert_eq!(
                debug_context["decision_evidence_digest"],
                assessment["evidence"]["snapshot_digest"]
            );
        } else {
            assert!(reply.is_err());
            assert_eq!(delivered.load(Ordering::SeqCst), 0);
        }
        assert_eq!(primary.request_count(), 2);
        assert_eq!(backup.request_count(), 1);
        assert_eq!(judge.request_count(), 3);
        assert_eq!(session.last_generated_candidates().len(), 3);
        assert_eq!(
            session.last_generated_candidates().last().unwrap().attempt,
            1
        );
        let selection = request_body(&judge, 2);
        let state: serde_json::Value =
            serde_json::from_str(selection["state"].as_str().unwrap()).unwrap();
        assert_eq!(
            state["selection"]["candidates"].as_array().unwrap().len(),
            2
        );
        let replacement = state["selection"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == "candidate-0")
            .unwrap();
        assert_eq!(replacement["response"], "replacement response");
        assert_eq!(replacement["telemetry"]["calls"], 2);
        assert_eq!(state["attempts"]["candidate-0"], 1);
        assert_eq!(state["attempts"]["candidate-1"], 0);
        let first_assessment: serde_json::Value =
            serde_json::from_str(request_body(&judge, 1)["state"].as_str().unwrap()).unwrap();
        assert_eq!(state["evidence"], first_assessment["evidence"]);
        let initial_generation = request_body(&primary, 0);
        let initial_prompt = initial_generation["messages"][1]["content"]
            .as_str()
            .unwrap();
        assert!(!initial_prompt.contains("[RESPONSE_GUIDANCE]"));
        let retry_generation = request_body(&primary, 1);
        let retry_prompt = retry_generation["messages"][1]["content"].as_str().unwrap();
        let guidance = prompt_section(retry_prompt, "RESPONSE_GUIDANCE");
        assert_eq!(guidance["reason_code"], "GROUNDING");
        assert_eq!(
            guidance["previous_response_digest"],
            kamimusuhi_core::digest::content_digest(b"old response")
        );
        assert!(!guidance["instruction"].as_str().unwrap().is_empty());
    }
}

#[test]
fn failed_retry_never_reuses_old_or_other_candidates() {
    for (replacement, code) in [
        (FixtureResponse::Silence { ms: 2_200 }, "TIMEOUT"),
        (FixtureResponse::ok(""), "MALFORMED_RESPONSE"),
        (
            FixtureResponse::ok("あ".repeat(5_462)),
            "RESPONSE_TOO_LARGE",
        ),
    ] {
        let primary =
            FixtureServer::start(vec![FixtureResponse::ok("old response"), replacement]).unwrap();
        let backup = FixtureServer::always(FixtureResponse::ok("unused backup")).unwrap();
        let judge = FixtureServer::start(vec![
            jev_preparation("SPEAK"),
            jev_assessment("primary", &[("primary", "RETRY"), ("backup", "ACCEPT")]),
        ])
        .unwrap();
        let (_dir, mut runtime, mut session) = fanout_session(&primary, &backup, &judge);
        assert!(
            session
                .turn(&mut runtime, "こんにちは", |_| panic!(
                    "must not deliver"
                ))
                .is_err()
        );
        assert_eq!(primary.request_count(), 2);
        assert_eq!(backup.request_count(), 1);
        assert_eq!(judge.request_count(), 2);
        let failure = session.last_generated_candidates().last().unwrap();
        assert_eq!(failure.id, "primary");
        assert_eq!(failure.attempt, 1);
        assert_eq!(failure.error_code.as_deref(), Some(code));
        assert!(
            session.history_for_display(&runtime).unwrap().iter().all(
                |message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant
            )
        );
    }
}

#[test]
fn response_selection_repairs_invalid_probability_answers_at_most_once() {
    let request = LanguageResponseSelectionRequest {
        user_text: "こんにちは".to_owned(),
        speech_act: "greeting".to_owned(),
        state: ConversationCoreState::default(),
        candidates: vec![LanguageResponseCandidate {
            id: "primary".to_owned(),
            provider: "mock".to_owned(),
            model: "fixture".to_owned(),
            response: "候補".to_owned(),
            response_bytes: "候補".len(),
            response_digest: kamimusuhi_core::digest::content_digest("候補".as_bytes()),
            latency_ms: 0,
            telemetry: LanguageProviderTelemetry::default().snapshot(),
        }],
    };
    for probabilities in [
        serde_json::json!({}),
        serde_json::json!({"primary": 1.0, "unknown": 0.0}),
        serde_json::json!({"primary": 1.1}),
        serde_json::json!({"primary": 0.5}),
    ] {
        let invalid = raw_json(serde_json::json!({"answers":{"response_candidate":{
            "type":"choice", "choice":"primary", "confidence":0.93, "probabilities":probabilities,
        }}}));
        for recover in [true, false] {
            let second = if recover {
                jev_response_candidate_choice("primary", &["primary"])
            } else {
                invalid.clone()
            };
            let server = FixtureServer::start(vec![invalid.clone(), second]).unwrap();
            let provider = JevDecisionProvider::new(TypesafeConfig {
                base_url: systemone_base_url(&server),
                model: "jev-1.13.0".to_owned(),
                auth_env: String::new(),
                timeout_ms: 2_000,
            });
            assert_eq!(provider.select_language_response(&request).is_ok(), recover);
            assert_eq!(server.request_count(), 2);
            let first = request_body(&server, 0);
            let retry = request_body(&server, 1);
            assert_eq!(first["state"], retry["state"]);
            assert!(
                retry["questions"]["response_candidate"]["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("INVALID_DECISION")
            );
        }
    }
}

#[test]
fn privacy_excludes_external_organs_and_refuses_external_judging_before_network() {
    let primary = FixtureServer::always(FixtureResponse::ok("local response")).unwrap();
    let external = FixtureServer::always(FixtureResponse::ok("must not be contacted")).unwrap();
    let judge = FixtureServer::always(jev_preparation("SPEAK")).unwrap();
    let (_dir, mut runtime) = prepared(&primary);
    let mut config = runtime.config().clone();
    let mut blocked = config.persona.provider.as_ref().unwrap().clone();
    blocked.locality = LocalityClass::External;
    blocked.base_url = external.base_url();
    config.set_language_provider("external", blocked).unwrap();
    runtime.save_config(config).unwrap();
    let mut session = DialogueSession::start_with_decision_provider(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
    )
    .unwrap();
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "local response");
    assert_eq!(primary.request_count(), 1);
    assert_eq!(external.request_count(), 0);
    assert_eq!(session.last_generated_candidates().len(), 1);
    assert!(
        DialogueSession::start_with_decision_provider(
            &mut runtime,
            "bob",
            PrivacyConstraint::LocalOnly,
            Box::new(JevDecisionProvider::new(TypesafeConfig {
                base_url: systemone_base_url(&judge),
                model: "jev-1.13.0".to_owned(),
                auth_env: String::new(),
                timeout_ms: 2_000,
            })),
        )
        .is_err()
    );
    assert_eq!(judge.request_count(), 0);
    assert_eq!(primary.request_count(), 1);
    assert_eq!(external.request_count(), 0);
}

#[test]
fn typed_jev_and_language_provider_form_a_two_turn_closed_loop() {
    let server = FixtureServer::start(vec![
        jev_preparation("SPEAK"),
        FixtureResponse::ok("こんにちは。状態を確認しています。"),
        jev_assessment("primary", &[("primary", "ACCEPT")]),
        jev_preparation("SPEAK"),
        FixtureResponse::ok("問題を確認しました。状況を整理します。"),
        jev_assessment("primary", &[("primary", "ACCEPT")]),
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
        .turn(&mut runtime, "G0-v6でエラーが起きた", |_| Ok(()))
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
        assert_eq!(body["questions"].as_object().unwrap().len(), 2);
        let state: serde_json::Value =
            serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
        assert_eq!(state["invocation"]["speech_act"], expected);
        assert_eq!(state["invocation"]["state"]["speech_act"], expected);
        assert_eq!(body["questions"]["invocation_gate"]["type"], "choice");
        assert_eq!(body["questions"]["observation_need"]["type"], "choice");
    }
    for index in [2, 5] {
        let body = request_body(&server, index);
        assert_eq!(body["questions"].as_object().unwrap().len(), 6);
        assert_eq!(body["questions"]["response_candidate"]["type"], "choice");
        let state: serde_json::Value =
            serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
        assert_eq!(
            state["selection"]["candidates"].as_array().unwrap().len(),
            1
        );
        assert!(
            state["selection"]["candidates"][0]["response"]
                .as_str()
                .is_some()
        );
        assert_eq!(state["attempts"]["candidate-0"], 0);
        let language = request_body(&server, index - 1);
        let prompt = language["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains("[CONVERSATION_CORE_STATE]"));
        assert!(prompt.contains("speech_act"));
        assert!(prompt.len() <= MAX_INPUT_BYTES * 8);
        let evidence = &state["evidence"]["content"];
        assert_eq!(
            evidence["observed_runtime"],
            prompt_section(prompt, "OBSERVED_RUNTIME")
        );
        assert_eq!(
            evidence["research_findings"],
            prompt_section(prompt, "RESEARCH_FINDINGS")
        );
        assert_eq!(
            state["evidence"]["snapshot_digest"],
            kamimusuhi_core::digest::json_digest(evidence)
        );
        if index == 5 {
            assert_eq!(
                evidence["conversation_history"],
                prompt_section(prompt, "CONVERSATION_HISTORY")
            );
            assert!(
                evidence["conversation_history"]
                    .to_string()
                    .contains(&first.response)
            );
            let finding = evidence["research_findings"]["selected"]
                .as_array()
                .unwrap()
                .iter()
                .find(|selected| selected["finding"]["id"] == "g0-grounding-lineage")
                .unwrap();
            assert_eq!(finding["finding"]["status"], "invalid");
            assert!(
                !finding["finding"]["limitations"]
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
            assert!(!finding["finding"]["sources"].as_array().unwrap().is_empty());
        }
        let trace = if index == 2 {
            &first.llm_jev
        } else {
            &second.llm_jev
        };
        let trace_json = serde_json::to_string(trace).unwrap();
        assert!(!trace_json.contains(&first.response));
        assert!(!trace_json.contains(&second.response));
    }
}

#[test]
fn jev_provider_choice_uses_a_typed_choice_and_declared_candidates() {
    let server = FixtureServer::start(vec![jev_provider_choice("backup")]).unwrap();
    let provider = JevDecisionProvider::new(TypesafeConfig {
        base_url: systemone_base_url(&server),
        model: "jev-1.13.0".to_owned(),
        auth_env: String::new(),
        timeout_ms: 2_000,
    });
    let result = provider
        .select_language_provider(&ProviderSelectionRequest {
            user_text: "質問です".to_owned(),
            speech_act: "answer_question".to_owned(),
            state: ConversationCoreState::default(),
            candidates: vec![
                LanguageProviderCandidate {
                    id: "primary".to_owned(),
                    provider: "grokbot".to_owned(),
                    model: "lfm2.5".to_owned(),
                    endpoint: "http://primary".to_owned(),
                    telemetry: LanguageProviderTelemetry::default().snapshot(),
                },
                LanguageProviderCandidate {
                    id: "backup".to_owned(),
                    provider: "openai-compatible".to_owned(),
                    model: "backup-model".to_owned(),
                    endpoint: "https://backup".to_owned(),
                    telemetry: LanguageProviderTelemetry::default().snapshot(),
                },
            ],
        })
        .unwrap();
    assert_eq!(result.provider_id, "backup");
    assert_eq!(result.provider, "typesafe-systemone");
    let request = server.requests().pop().unwrap();
    assert_eq!(request.path, "/v1/systemone");
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["questions"]["language_provider"]["type"], "choice");
    let state: serde_json::Value = serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
    assert_eq!(state["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(state["candidates"][0]["telemetry"]["calls"], 0);
    assert!(
        body["questions"]["language_provider"]["criteria"]["backup"]
            .as_str()
            .unwrap()
            .contains("ewma_latency_ms=unknown")
    );
    assert!(!body.to_string().contains("Authorization"));
}

#[test]
fn jev_selects_anonymous_generated_response_from_ready_race_batch() {
    let backup = FixtureServer::always(FixtureResponse::ok("backup response")).unwrap();
    let server = FixtureServer::start(vec![
        jev_preparation("SPEAK"),
        FixtureResponse::ok("primary response"),
        jev_assessment("backup", &[("primary", "ACCEPT"), ("backup", "ACCEPT")]),
    ])
    .unwrap();
    let (_dir, mut runtime) = prepared(&server);
    let mut config = runtime.config().clone();
    config
        .set_language_provider(
            "backup",
            PersonaProviderConfig {
                backend_id: PersonaBackendId::from_u128(0xBEEF),
                locality: LocalityClass::LocalHost,
                base_url: backup.base_url(),
                model: "backup-language".to_owned(),
                auth_env: None,
                timeout_ms: 2_000,
                tls_root_ca_path: None,
                system_instruction: None,
            },
        )
        .unwrap();
    runtime.save_config(config).unwrap();
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
        None,
    )
    .unwrap();
    session.set_debug_trace(true);
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.language_provider_id, "backup");
    assert_eq!(trace.provider_selection.unwrap().provider_id, "candidate-1");
    assert_eq!(trace.provider_candidates[1].telemetry.calls, 0);
    assert_eq!(trace.generated_candidates.len(), 2);
    assert!(
        trace
            .generated_candidates
            .iter()
            .all(|candidate| candidate.error_code.is_none())
    );
    let primary_telemetry = &trace.provider_telemetry["primary"];
    assert_eq!(primary_telemetry.calls, 1);
    assert_eq!(primary_telemetry.successes, 1);
    assert_eq!(primary_telemetry.failures, 0);
    let backup_telemetry = &trace.provider_telemetry["backup"];
    assert_eq!(backup_telemetry.calls, 1);
    assert_eq!(backup_telemetry.successes, 1);
    assert_eq!(backup_telemetry.failures, 0);
    assert_eq!(backup_telemetry.success_rate, Some(1.0));
    assert!(backup_telemetry.ewma_latency_ms.is_some());
    assert_eq!(reply.response, "backup response");
    let requests = server.requests();
    assert_eq!(backup.request_count(), 1);
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/systemone");
    assert_eq!(requests[1].path, "/v1/chat/completions");
    assert_eq!(requests[2].path, "/v1/systemone");
    let choice: serde_json::Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(choice["questions"]["response_candidate"]["type"], "choice");
    assert_eq!(choice["questions"].as_object().unwrap().len(), 11);
    let state: serde_json::Value = serde_json::from_str(choice["state"].as_str().unwrap()).unwrap();
    assert_eq!(
        state["selection"]["candidates"].as_array().unwrap().len(),
        2
    );
    for candidate in state["selection"]["candidates"].as_array().unwrap() {
        let candidate_id = candidate["id"].as_str().unwrap();
        assert!(matches!(candidate_id, "candidate-0" | "candidate-1"));
        for dimension in [
            "grounding",
            "attribution",
            "task_fit",
            "response_gate",
            "repair_reason",
        ] {
            let question = format!("{dimension}_{candidate_id}");
            let instructions = choice["questions"][&question]["instructions"]
                .as_str()
                .unwrap();
            assert!(instructions.contains(&format!("candidate ID {candidate_id:?}")));
            assert!(!instructions.contains("selection.candidates["));
        }
        let expected = match candidate_id {
            "candidate-0" => "primary response",
            "candidate-1" => "backup response",
            _ => unreachable!(),
        };
        assert_eq!(candidate["response"], expected);
        assert!(candidate.get("provider").is_none());
        assert!(candidate.get("model").is_none());
        assert_eq!(candidate["telemetry"]["calls"], 1);
    }
    let wire = choice.to_string();
    assert!(!wire.contains("backup-language"));
    assert!(!wire.contains("fixture-language"));
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
