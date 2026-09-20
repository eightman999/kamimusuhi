//! Whole-dialogue recall wiring with in-process providers. These tests exercise
//! selection and provenance propagation, not Jev's semantic accuracy.

use std::sync::{Arc, Mutex};

use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRecord, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaTurnInput};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::workspace::{SourceRef, WorkspaceContent, WorkspaceDomain};
use kamimusuhi_runtime::c0;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{
    ConversationError, DecisionProvider, DecisionRequest, DecisionResult, LanguageProvider,
    LanguageRequest, LanguageResult, MockLanguageProvider, RecallRelevance, ResponseAssessment,
    ResponseAssessmentRequest, RuleBasedDecisionProvider, TurnPreparation, TurnPreparationRequest,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use serde_json::json;

const SOURCE_ID: &str = "text-chat:alice";
const QUERY: &str = "いつもの通勤手段は？";
const SELECTED_TEXT: &str = "会社には毎朝ロードバイクで通っています。\n雨天も同じです。";
const UNCERTAIN_TEXT: &str = "先週、焙煎所で珈琲豆を買いました。\n袋は青色でした。";

#[derive(Default)]
struct Captured {
    preparations: Mutex<Vec<TurnPreparationRequest>>,
    language_inputs: Mutex<Vec<PersonaTurnInput>>,
    assessments: Mutex<Vec<ResponseAssessmentRequest>>,
}

struct RecallJudge {
    selected_evidence_id: EvidenceId,
    captured: Arc<Captured>,
}

impl DecisionProvider for RecallJudge {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        RuleBasedDecisionProvider.decide(request)
    }

    fn prepare_turn(
        &self,
        request: &TurnPreparationRequest,
    ) -> Result<TurnPreparation, ConversationError> {
        self.captured
            .preparations
            .lock()
            .unwrap()
            .push(request.clone());
        let mut preparation = RuleBasedDecisionProvider.prepare_turn(request)?;
        let selected = request
            .recall_candidates
            .iter()
            .find(|candidate| {
                candidate.content["evidence"]["evidence_id"] == json!(self.selected_evidence_id)
            })
            .expect("the lexical miss must be offered for semantic recall");
        preparation
            .recall
            .insert(selected.id.clone(), RecallRelevance::Relevant);
        // The compatibility provider explicitly leaves every other candidate
        // Uncertain. They must not enter context merely because they were read.
        Ok(preparation)
    }

    fn assess_responses(
        &self,
        request: &ResponseAssessmentRequest,
    ) -> Result<ResponseAssessment, ConversationError> {
        self.captured
            .assessments
            .lock()
            .unwrap()
            .push(request.clone());
        RuleBasedDecisionProvider.assess_responses(request)
    }
}

struct CapturingLanguageProvider {
    captured: Arc<Captured>,
}

impl LanguageProvider for CapturingLanguageProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.captured
            .language_inputs
            .lock()
            .unwrap()
            .push(request.persona_input.clone());
        MockLanguageProvider.generate(request)
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        MockLanguageProvider.descriptor()
    }
}

fn append_utterance(runtime: &Runtime, kind: EvidenceKind, text: &str) -> EvidenceRecord {
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: EvidenceId::generate(runtime.ids().as_ref()),
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind,
            origin_class: if kind == EvidenceKind::UserUtterance {
                OriginClass::Reported
            } else {
                OriginClass::Inferred
            },
            payload: json!({"text": text}),
            source: EvidenceSource {
                source_id: Some(SOURCE_ID.to_owned()),
                source_sequence: None,
                content_digest: Some(content_digest(text.as_bytes())),
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap()
}

fn assert_recall_reaches_generation_and_assessment(selected_kind: EvidenceKind) {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(70),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    assert_eq!(c0::lexical_score(QUERY, SELECTED_TEXT), 0.0);
    assert_eq!(c0::lexical_score(QUERY, UNCERTAIN_TEXT), 0.0);

    let selected = append_utterance(&runtime, selected_kind, SELECTED_TEXT);
    let uncertain = append_utterance(&runtime, EvidenceKind::UserUtterance, UNCERTAIN_TEXT);
    // Both candidate records are deliberately older than the visible history
    // window, so the test cannot pass by reading them from recent conversation.
    let history_limit = c0::operative(&runtime)
        .unwrap()
        .view
        .params
        .retrieval
        .history_messages;
    for index in 0..history_limit {
        append_utterance(
            &runtime,
            EvidenceKind::UserUtterance,
            &format!("直近の連絡 {index}"),
        );
    }

    let captured = Arc::new(Captured::default());
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(RecallJudge {
            selected_evidence_id: selected.evidence_id,
            captured: Arc::clone(&captured),
        }),
        Some(Box::new(CapturingLanguageProvider {
            captured: Arc::clone(&captured),
        })),
    )
    .unwrap();
    let reply = session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();
    assert!(!reply.response.is_empty());

    let preparations = captured.preparations.lock().unwrap();
    let inputs = captured.language_inputs.lock().unwrap();
    let assessments = captured.assessments.lock().unwrap();
    assert_eq!(preparations.len(), 1);
    assert_eq!(inputs.len(), 1);
    assert_eq!(assessments.len(), 1);
    let preparation = &preparations[0];
    let input = &inputs[0];
    let assessment = &assessments[0];
    assert_eq!(preparation.evidence.content["recalled_evidence"], json!([]));
    assert_eq!(preparation.recall_candidates.len(), 2);
    for record in [&selected, &uncertain] {
        let candidate = preparation
            .recall_candidates
            .iter()
            .find(|candidate| {
                candidate.content["evidence"]["evidence_id"] == json!(record.evidence_id)
            })
            .unwrap();
        assert!(
            !candidate.baseline,
            "a lexical miss is not a retained baseline"
        );
        assert_eq!(candidate.content["source_type"], "raw_utterance");
        assert_eq!(candidate.content["evidence"], json!(record));
        assert_eq!(
            candidate.content["speaker"],
            if record.kind == EvidenceKind::UserUtterance {
                "user"
            } else {
                "assistant"
            }
        );
    }

    let envelope = &input.envelope;
    assert_eq!(envelope.recalled_evidence.len(), 1);
    let recalled = &envelope.recalled_evidence[0];
    assert_eq!(recalled.domain, WorkspaceDomain::RecalledEvidence);
    assert_eq!(
        recalled.source_ref,
        SourceRef::Evidence {
            evidence_id: selected.evidence_id,
            kind: selected_kind
        }
    );
    assert_eq!(recalled.content, WorkspaceContent::text(SELECTED_TEXT));
    assert_eq!(recalled.freshness.source_time, Some(selected.created_at));
    assert!(envelope.relationship.is_empty());
    assert!(envelope.episodic.is_empty());
    assert!(envelope.conversation_history.iter().all(|message| {
        message.evidence_id != selected.evidence_id && message.evidence_id != uncertain.evidence_id
    }));

    // The judge must see the actual language input *after* recall selection,
    // not the earlier lexical-only snapshot used to prepare the turn.
    let expected_context = json!({
        "turn": input.context,
        "user_input": input.input,
        "conversation_history": envelope.conversation_history,
        "relationship": envelope.relationship,
        "episodic": envelope.episodic,
        "recalled_evidence": envelope.recalled_evidence,
        "durable_self": envelope.durable_self,
        "active_policy": envelope.active_policy,
        "observed_runtime": envelope.observed_runtime,
        "mio_observation": envelope.mio_observation,
        "research_findings": envelope.research_findings,
        "body_state": envelope.body_state,
        "library": envelope.library,
        "external_results": envelope.external_results,
        "response_guidance": envelope.response_guidance,
    });
    assert_eq!(assessment.evidence.content, expected_context);
    assert_eq!(
        assessment.evidence.snapshot_digest,
        json_digest(&expected_context)
    );
    assert_ne!(
        assessment.evidence.snapshot_digest,
        preparation.evidence.snapshot_digest
    );
    assert!(
        !expected_context
            .to_string()
            .contains(&uncertain.evidence_id.to_string())
    );
    assert_eq!(
        session.last_context().unwrap()["recalled_evidence"],
        json!(envelope.recalled_evidence)
    );

    // Relevance changes only this prompt view. Both canonical utterances stay
    // byte-for-byte the original records and no durable memory is created.
    assert_eq!(
        runtime.store().get(selected.evidence_id).unwrap(),
        Some(selected)
    );
    assert_eq!(
        runtime.store().get(uncertain.evidence_id).unwrap(),
        Some(uncertain)
    );
    assert!(
        MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(runtime.individual_id())
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn selected_user_utterance_keeps_provenance_in_language_and_assessment_context() {
    assert_recall_reaches_generation_and_assessment(EvidenceKind::UserUtterance);
}

#[test]
fn selected_agent_utterance_keeps_provenance_in_language_and_assessment_context() {
    assert_recall_reaches_generation_and_assessment(EvidenceKind::AgentUtterance);
}
