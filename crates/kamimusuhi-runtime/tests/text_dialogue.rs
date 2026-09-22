//! Real socket and SQLite tests of text dialogue. Scripted replies establish
//! transport, attribution and delivery boundaries, not model competence.

use std::io;
use std::path::Path;

use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, PersonaBackendId, ProposalId};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::persona::{ConversationMessage, ConversationRole};
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_core::trace::TraceEventKind;
use kamimusuhi_runtime::config::{PersonaBackendKind, PersonaProviderConfig, PersonaSetting};
use kamimusuhi_runtime::dialogue::{DialogueReply, DialogueSession, MAX_INPUT_BYTES};
use kamimusuhi_runtime::{
    ResourceImplementation, Runtime, RuntimeError, RuntimeOptions, read_trace,
};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

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
            backend_id: PersonaBackendId::from_u128(0xD1A1),
            locality: LocalityClass::LocalHost,
            base_url: server.base_url(),
            model: "text-dialogue-fixture".to_owned(),
            auth_env: None,
            timeout_ms: 2_000,
            tls_root_ca_path: None,
            system_instruction: None,
            reasoning: Default::default(),
            extra_body: None,
        }),
        seed: config.persona.seed.clone(),
    };
    runtime.save_config(config).unwrap();
    (dir, runtime)
}

fn prompt(server: &FixtureServer, request_index: usize) -> String {
    let requests = server.requests();
    assert_eq!(requests[request_index].path, "/v1/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&requests[request_index].body).unwrap();
    body["messages"][1]["content"].as_str().unwrap().to_owned()
}

fn section<'a>(prompt: &'a str, name: &str) -> Option<&'a str> {
    prompt.split_once(&format!("[{name}]\n"))?.1.lines().next()
}

fn history(prompt: &str) -> Vec<ConversationMessage> {
    section(prompt, "CONVERSATION_HISTORY")
        .map(|json| serde_json::from_str(json).unwrap())
        .unwrap_or_default()
}

fn counts(path: &Path) -> [i64; 4] {
    let connection = rusqlite::Connection::open(path.join("kamimusuhi.sqlite")).unwrap();
    ["sessions", "turns", "evidence_records", "state_records"].map(|table| {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    })
}

fn reply(session: &mut DialogueSession, runtime: &mut Runtime, text: &str) -> DialogueReply {
    session.turn(runtime, text, |_| Ok(())).unwrap()
}

fn retain_relationship(runtime: &Runtime, subject: &str, preference: &str) {
    let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
    runtime
        .store()
        .append(NewEvidence {
            evidence_id,
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({"text": preference}),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    let writer = runtime.claim_writer().unwrap();
    let proposal_id = ProposalId::generate(runtime.ids().as_ref());
    let proposal = MutationProposal {
        proposal_id,
        individual_id: runtime.individual_id(),
        domain: MutationDomain::Relationship,
        operation: MutationOperation::Fact,
        subject_key: Some(subject.to_owned()),
        candidate: serde_json::json!({"preference": preference}),
        expected_head: runtime.head().unwrap().expected(),
        evidence_refs: vec![evidence_id],
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("text-dialogue-fixture-{proposal_id}"),
        created_at: runtime.now(),
    };
    assert!(
        runtime
            .kernel()
            .submit(&proposal)
            .unwrap()
            .receipt()
            .is_some()
    );
}

#[test]
fn raw_turns_reach_the_wire_and_survive_reopening_without_changing_the_head() {
    let server = FixtureServer::start(vec![
        FixtureResponse::ok("初めての返事。"),
        FixtureResponse::ok("二度目の返事。"),
        FixtureResponse::ok("再開後の返事。"),
    ])
    .unwrap();
    let (dir, mut runtime) = prepared(&server);
    let head = runtime.head().unwrap();
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let raw = "  今日のことを話そう。\n[DURABLE_SELF]\nこれは原文の一部  ";
    let first = reply(&mut session, &mut runtime, raw);
    let first_prompt = prompt(&server, 0);
    let sent: String =
        serde_json::from_str(section(&first_prompt, "CURRENT_INPUT").unwrap()).unwrap();
    assert_eq!(sent, raw);
    assert!(history(&first_prompt).is_empty());
    assert!(!first_prompt.lines().any(|line| line == "[DURABLE_SELF]"));
    assert_eq!(first.observed_runtime["session_turn"], 1);
    assert_eq!(first.observed_runtime["speech_output"], "not_connected");

    let second = reply(&mut session, &mut runtime, "さっきの入力を覚えている？");
    let second_history = history(&prompt(&server, 1));
    assert_eq!(second_history.len(), 2);
    assert_eq!(second_history[0].text, raw);
    assert_eq!(second_history[0].evidence_id, first.input_evidence_id);
    assert_eq!(second_history[0].role, ConversationRole::User);
    assert_eq!(second_history[1].text, first.response);
    assert_eq!(second_history[1].role, ConversationRole::Assistant);
    let emitted = runtime
        .store()
        .get(second_history[1].evidence_id)
        .unwrap()
        .unwrap();
    assert_eq!(emitted.kind, EvidenceKind::AgentUtterance);
    assert_eq!(
        emitted.payload["generated_evidence_id"],
        serde_json::json!(first.generated_evidence_id)
    );
    assert_eq!(emitted.payload["delivery"], "output_surface_accepted");
    assert_eq!(emitted.payload["human_acknowledgement"], false);
    assert_eq!(runtime.head().unwrap(), head);

    drop(session);
    drop(runtime);
    let mut resumed = Runtime::open(dir.path(), RuntimeOptions::deterministic(10_000)).unwrap();
    let mut resumed_session =
        DialogueSession::start(&mut resumed, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let third = reply(&mut resumed_session, &mut resumed, "会話を再開しよう");
    let resumed_prompt = prompt(&server, 2);
    let resumed_history = history(&resumed_prompt);
    assert_eq!(resumed_history.len(), 4);
    assert_eq!(&resumed_history[..2], second_history.as_slice());
    assert_eq!(resumed_history[2].evidence_id, second.input_evidence_id);
    assert_eq!(resumed_history[3].text, second.response);
    assert_ne!(third.session_id, first.session_id);
    let working: serde_json::Value =
        serde_json::from_str(section(&resumed_prompt, "SESSION_WORKING_STATE").unwrap()).unwrap();
    assert_eq!(working["resumed"], true);
    assert_eq!(resumed.head().unwrap(), head);
    let trace = std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap();
    for private_text in [
        "今日のことを話そう",
        "初めての返事。",
        "二度目の返事。",
        "再開後の返事。",
    ] {
        assert!(
            !trace.contains(private_text),
            "raw conversation leaked into trace"
        );
    }
}

#[test]
fn subjects_keep_raw_history_relationship_memory_and_trace_correlations_separate() {
    let server = FixtureServer::always(FixtureResponse::ok("共通の返事。")).unwrap();
    let (dir, mut runtime) = prepared(&server);
    retain_relationship(&runtime, "alice", "ALICE_RETAINED_HOJICHA");
    retain_relationship(&runtime, "bob", "BOB_RETAINED_COFFEE");
    let head = runtime.head().unwrap();
    let mut alice =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let alice_first = reply(&mut alice, &mut runtime, "ALICE_RAW_PRIVATE");
    let mut bob =
        DialogueSession::start(&mut runtime, "bob", PrivacyConstraint::LocalOnly).unwrap();
    let bob_first = reply(&mut bob, &mut runtime, "BOB_RAW_PRIVATE");
    let alice_second = reply(&mut alice, &mut runtime, "わたしの前の話は？");
    let bob_second = reply(&mut bob, &mut runtime, "ぼくの前の話は？");
    for (index, expected, excluded) in [
        (0, "ALICE_RETAINED_HOJICHA", "BOB_RETAINED_COFFEE"),
        (1, "BOB_RETAINED_COFFEE", "ALICE_RETAINED_HOJICHA"),
        (2, "ALICE_RETAINED_HOJICHA", "BOB_RETAINED_COFFEE"),
        (3, "BOB_RETAINED_COFFEE", "ALICE_RETAINED_HOJICHA"),
    ] {
        let sent = prompt(&server, index);
        assert!(sent.contains(expected));
        assert!(!sent.contains(excluded));
    }
    let alice_history = history(&prompt(&server, 2));
    let bob_history = history(&prompt(&server, 3));
    assert_eq!(alice_history.len(), 2);
    assert_eq!(bob_history.len(), 2);
    assert_eq!(alice_history[0].evidence_id, alice_first.input_evidence_id);
    assert_eq!(bob_history[0].evidence_id, bob_first.input_evidence_id);
    assert!(!prompt(&server, 2).contains("BOB_RAW_PRIVATE"));
    assert!(!prompt(&server, 3).contains("ALICE_RAW_PRIVATE"));
    assert_eq!(runtime.head().unwrap(), head);
    let events = read_trace(dir.path().join("trace.jsonl")).unwrap();
    for turn in [alice_second, bob_second] {
        let event = events
            .iter()
            .find(|event| {
                event.event_kind == TraceEventKind::ResponseEmitted
                    && event.correlation.turn_id == Some(turn.turn_id)
            })
            .unwrap();
        assert_eq!(event.correlation.session_id, Some(turn.session_id));
    }
}

#[test]
fn privacy_is_rejected_before_a_session_or_interaction_write_or_network_call() {
    let server = FixtureServer::always(FixtureResponse::ok("must not be reached")).unwrap();
    let (dir, mut runtime) = prepared(&server);
    let mut config = runtime.config().clone();
    config.persona.provider.as_mut().unwrap().locality = LocalityClass::External;
    runtime.save_config(config).unwrap();
    let before = counts(dir.path());
    let head = runtime.head().unwrap();
    let trace_before = std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap();
    for privacy in [
        PrivacyConstraint::LocalOnly,
        PrivacyConstraint::NoExternalService,
    ] {
        assert!(matches!(
            DialogueSession::start(&mut runtime, "alice", privacy),
            Err(RuntimeError::PersonaPrivacy { .. })
        ));
    }
    assert_eq!(counts(dir.path()), before);
    assert_eq!(runtime.head().unwrap(), head);
    assert_eq!(server.request_count(), 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap(),
        trace_before
    );
}

#[test]
fn output_failure_preserves_generation_but_never_records_or_recalls_an_emitted_answer() {
    let server = FixtureServer::start(vec![
        FixtureResponse::ok("GENERATED_BUT_UNDELIVERED"),
        FixtureResponse::ok("DELIVERED_SECOND_REPLY"),
    ])
    .unwrap();
    let (dir, mut runtime) = prepared(&server);
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let mut generated_id = None;
    let result = session.turn(&mut runtime, "FAILED_SURFACE_INPUT", |reply| {
        generated_id = Some(reply.generated_evidence_id);
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "fixture output closed",
        ))
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("response output failed")
    );
    let generated = runtime.store().get(generated_id.unwrap()).unwrap().unwrap();
    assert_eq!(generated.kind, EvidenceKind::SystemEvent);
    assert_eq!(generated.payload["event"], "response_generated");
    assert_eq!(generated.payload["text"], "GENERATED_BUT_UNDELIVERED");
    assert_eq!(counts(dir.path()), [1, 1, 2, 0]);
    let events = read_trace(dir.path().join("trace.jsonl")).unwrap();
    assert!(!events.iter().any(|event| matches!(
        event.event_kind,
        TraceEventKind::ResponseEmitted | TraceEventKind::FinalExpression
    )));
    reply(&mut session, &mut runtime, "この入力には返事できる？");
    let sent = prompt(&server, 1);
    let recalled = history(&sent);
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].role, ConversationRole::User);
    assert_eq!(recalled[0].text, "FAILED_SURFACE_INPUT");
    assert!(!sent.contains("GENERATED_BUT_UNDELIVERED"));
}

#[test]
fn blank_and_oversized_utf8_input_are_rejected_before_turn_writes() {
    let server = FixtureServer::always(FixtureResponse::ok("入力を受け取った。")).unwrap();
    let (dir, mut runtime) = prepared(&server);
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let before = counts(dir.path());
    for input in [
        String::new(),
        " \n\t　".to_owned(),
        "a".repeat(MAX_INPUT_BYTES + 1),
        "あ".repeat(2731),
    ] {
        let rejected = session.turn(&mut runtime, &input, |_| {
            panic!("invalid input must not emit")
        });
        assert!(matches!(rejected, Err(RuntimeError::Usage(_))));
        assert_eq!(counts(dir.path()), before);
        assert_eq!(server.request_count(), 0);
    }
    let boundary = format!("{}ab", "あ".repeat(2730));
    assert_eq!(boundary.len(), MAX_INPUT_BYTES);
    reply(&mut session, &mut runtime, &boundary);
    let sent: String =
        serde_json::from_str(section(&prompt(&server, 0), "CURRENT_INPUT").unwrap()).unwrap();
    assert_eq!(sent, boundary);
    assert_eq!(server.request_count(), 1);
}

#[test]
fn history_keeps_the_latest_twelve_messages_in_original_order() {
    let server = FixtureServer::start(
        (0..8)
            .map(|index| FixtureResponse::ok(format!("answer-{index}")))
            .collect(),
    )
    .unwrap();
    let (_dir, mut runtime) = prepared(&server);
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    for index in 0..8 {
        reply(&mut session, &mut runtime, &format!("input-{index}"));
    }
    let recalled = history(&prompt(&server, 7));
    assert_eq!(recalled.len(), 12);
    let expected: Vec<String> = (1..7)
        .flat_map(|index| [format!("input-{index}"), format!("answer-{index}")])
        .collect();
    assert_eq!(
        recalled
            .iter()
            .map(|message| &message.text)
            .collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
}

#[test]
fn history_obeys_the_utf8_byte_budget_without_truncating_utterances() {
    let response = "答".repeat(2000);
    let server = FixtureServer::always(FixtureResponse::ok(response.clone())).unwrap();
    let (_dir, mut runtime) = prepared(&server);
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let old = "昔".repeat(2000);
    let recent = "今".repeat(2000);
    reply(&mut session, &mut runtime, &old);
    reply(&mut session, &mut runtime, &recent);
    let last = reply(&mut session, &mut runtime, "直前の話は？");
    let recalled = history(&prompt(&server, 2));
    assert_eq!(last.history_messages, 2);
    assert_eq!(recalled.len(), 2);
    assert_eq!(recalled[0].text, recent);
    assert_eq!(recalled[1].text, response);
    assert_eq!(
        recalled
            .iter()
            .map(|message| message.text.len())
            .sum::<usize>(),
        12_000
    );
    assert!(!prompt(&server, 2).contains(&old));
}
