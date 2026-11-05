//! The self-improvement lane end to end: `self_notes.jsonl` entries reach
//! the language workspace marked as self-authored scratch state (never
//! canonical sections), and `improve` proposals land pending on the C0 lane
//! after the closed-vocabulary check — never applied by the tool path.

use std::sync::{Arc, Mutex};

use kamimusuhi_core::c0::ProposalStatus;
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaTurnInput};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::self_note::SelfNoteKind;
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_core::workspace::SourceRef;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{
    ConversationError, LanguageProvider, LanguageRequest, LanguageResult, MockLanguageProvider,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, self_notes};
use serde_json::json;

/// Language organ that just records what the turn showed it.
struct CapturingLanguage {
    inputs: Arc<Mutex<Vec<PersonaTurnInput>>>,
}

impl LanguageProvider for CapturingLanguage {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.inputs
            .lock()
            .unwrap()
            .push(request.persona_input.clone());
        MockLanguageProvider.generate(request)
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        MockLanguageProvider.descriptor()
    }
}

fn runtime(dir: &std::path::Path, seed: u64) -> Runtime {
    Runtime::init(
        dir,
        RuntimeOptions::deterministic(seed),
        ResourceImplementation::FakeA,
    )
    .unwrap()
}

#[test]
fn self_notes_reach_the_envelope_under_their_own_lane() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = runtime(dir.path(), 91);
    let at = UtcTimestamp::from_unix_millis(1_700_000_000_000);
    let note = self_notes::append(
        &runtime.paths().dir,
        SelfNoteKind::Lesson,
        "長くなる前に確認する",
        vec!["ev-fixture".to_owned()],
        at,
    )
    .unwrap();
    // Older notes stay visible in sequence order; the file is the lane.
    self_notes::append(
        &runtime.paths().dir,
        SelfNoteKind::Tendency,
        "短く答えすぎる傾向",
        vec![],
        at,
    )
    .unwrap();

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(kamimusuhi_runtime::llm_jev::RuleBasedDecisionProvider),
        Some(Box::new(CapturingLanguage {
            inputs: Arc::clone(&captured),
        })),
    )
    .unwrap();
    session
        .turn(&mut runtime, "今日の調子は?", |_| Ok(()))
        .unwrap();

    let inputs = captured.lock().unwrap();
    let envelope = &inputs[0].envelope;
    assert_eq!(envelope.self_notes.len(), 2);
    let first = &envelope.self_notes[0];
    assert_eq!(first.inclusion_reason.as_str(), "self_note");
    assert_eq!(first.authority.as_str(), "self_authored");
    assert_eq!(first.domain.as_str(), "SELF_NOTE");
    let SourceRef::SelfNote { note_id } = &first.source_ref else {
        panic!("self notes must carry the note-lane source ref");
    };
    assert_eq!(*note_id, note.note_id());
    assert_eq!(
        first.content,
        kamimusuhi_core::workspace::WorkspaceContent::structured(json!({
            "kind": "lesson",
            "text": "長くなる前に確認する",
            "evidence_refs": ["ev-fixture"],
        }))
    );

    // Domain separation: the same bytes appear under no canonical heading —
    // not self-state, not evidence, not memory.
    for (name, items) in [
        ("durable_self", &envelope.durable_self),
        ("relationship", &envelope.relationship),
        ("episodic", &envelope.episodic),
        ("recalled_evidence", &envelope.recalled_evidence),
    ] {
        assert!(
            items.iter().all(|item| !item.content.eq(
                &kamimusuhi_core::workspace::WorkspaceContent::structured(
                    json!({"kind": "lesson", "text": "長くなる前に確認する",
                           "evidence_refs": ["ev-fixture"]})
                )
            )),
            "self note leaked into {name}"
        );
    }
}

#[test]
fn an_empty_note_file_stages_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = runtime(dir.path(), 92);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(kamimusuhi_runtime::llm_jev::RuleBasedDecisionProvider),
        Some(Box::new(CapturingLanguage {
            inputs: Arc::clone(&captured),
        })),
    )
    .unwrap();
    session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert!(captured.lock().unwrap()[0].envelope.self_notes.is_empty());
}

#[test]
fn propose_param_queues_pending_and_refuses_unknown_params() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), 93);

    // An unknown target is refused *before* a row exists — the bound error
    // goes back to the caller and the store stays empty.
    assert!(matches!(
        kamimusuhi_runtime::c0::propose_param(
            &runtime,
            "retrieval.made_up_knob",
            json!(4),
            Some("because".to_owned()),
        ),
        Err(kamimusuhi_runtime::RuntimeError::Usage(_))
    ));
    assert!(matches!(
        kamimusuhi_runtime::c0::propose_param(
            &runtime,
            "retrieval.self_notes_max",
            json!(10_000),
            None,
        ),
        Err(kamimusuhi_runtime::RuntimeError::Usage(_))
    ));
    assert!(
        runtime
            .store()
            .c0_proposals(runtime.individual_id(), None)
            .unwrap()
            .is_empty()
    );

    let proposal = kamimusuhi_runtime::c0::propose_param(
        &runtime,
        "retrieval.self_notes_max",
        json!(9),
        Some("more notes help continuity".to_owned()),
    )
    .unwrap();
    assert_eq!(proposal.status, ProposalStatus::Pending);
    assert_eq!(
        proposal.kind,
        kamimusuhi_core::c0::ImprovementKind::RetrievalUpdate
    );
    assert_eq!(proposal.target, "retrieval.self_notes_max");
    assert_eq!(proposal.old_value, Some(json!(5)));

    // What the operator sees: the same pending row `inspect` lists.
    let pending = runtime
        .store()
        .c0_proposals(runtime.individual_id(), Some(ProposalStatus::Pending))
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].proposal_id, proposal.proposal_id);
}
