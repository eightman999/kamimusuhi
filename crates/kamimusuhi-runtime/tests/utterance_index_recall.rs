//! The derived full-text utterance index end to end: an utterance that has
//! scrolled out of the bounded recent pool is still *offered* to the
//! relevance judge, and only the judge's verdict lets it into the workspace.

use std::sync::{Arc, Mutex};

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaTurnInput};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{
    ConversationError, DecisionProvider, DecisionRequest, DecisionResult, LanguageProvider,
    LanguageRequest, LanguageResult, MockLanguageProvider, RecallRelevance, ResponseAssessment,
    ResponseAssessmentRequest, RuleBasedDecisionProvider, TurnPreparation, TurnPreparationRequest,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use serde_json::json;

const SOURCE_ID: &str = "text-chat:alice";
const QUERY: &str = "ロードバイクの通勤について教えて";
/// More than the default `retrieval.evidence_pool`, so the target utterance
/// is no longer in the recent pool the lexical lane scores.
const FILLERS: usize = 110;

#[derive(Default)]
struct Captured {
    preparations: Mutex<Vec<TurnPreparationRequest>>,
    language_inputs: Mutex<Vec<PersonaTurnInput>>,
}

/// Rule-based judge that optionally marks every candidate carrying a `via`
/// label relevant — the compatibility baseline leaves them all Uncertain.
struct IndexJudge {
    accept_via: bool,
    captured: Arc<Captured>,
}

impl DecisionProvider for IndexJudge {
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
        if self.accept_via {
            for candidate in &request.recall_candidates {
                if candidate.content["via"].is_string() {
                    preparation
                        .recall
                        .insert(candidate.id.clone(), RecallRelevance::Relevant);
                }
            }
        }
        Ok(preparation)
    }

    fn assess_responses(
        &self,
        request: &ResponseAssessmentRequest,
    ) -> Result<ResponseAssessment, ConversationError> {
        RuleBasedDecisionProvider.assess_responses(request)
    }
}

struct CapturingLanguage {
    captured: Arc<Captured>,
}

impl LanguageProvider for CapturingLanguage {
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

fn append_utterance(runtime: &Runtime, text: &str) -> EvidenceId {
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
            payload: json!({"text": text}),
            source: EvidenceSource {
                source_id: Some(SOURCE_ID.to_owned()),
                source_sequence: None,
                content_digest: Some(content_digest(text.as_bytes())),
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    evidence_id
}

/// One old utterance, then enough unrelated ones to push it out of the pool.
fn seeded_runtime(seed: u64) -> (tempfile::TempDir, Runtime, EvidenceId) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(seed),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let target = append_utterance(&runtime, "通勤手段はロードバイクです");
    for index in 0..FILLERS {
        append_utterance(&runtime, &format!("メモ{index}: 特に何もなかった"));
    }
    (dir, runtime, target)
}

fn session(runtime: &mut Runtime, accept_via: bool, captured: &Arc<Captured>) -> DialogueSession {
    DialogueSession::start_with_providers(
        runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(IndexJudge {
            accept_via,
            captured: Arc::clone(captured),
        }),
        Some(Box::new(CapturingLanguage {
            captured: Arc::clone(captured),
        })),
    )
    .unwrap()
}

fn recalled_text(captured: &Captured, turn: usize) -> String {
    serde_json::to_string(
        &captured.language_inputs.lock().unwrap()[turn]
            .envelope
            .recalled_evidence,
    )
    .unwrap()
}

#[test]
fn an_utterance_past_the_recent_pool_is_offered_through_the_index_and_recalled() {
    let (_dir, mut runtime, target) = seeded_runtime(81);
    // The bounded pool alone no longer reaches the target.
    let pool = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 96)
        .unwrap();
    assert!(pool.iter().all(|record| record.evidence_id != target));

    let captured = Arc::new(Captured::default());
    let mut session = session(&mut runtime, true, &captured);
    let reply = session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();
    assert_eq!(reply.c0.as_ref().unwrap().recall_index_hits, 1);

    // Offered under the usual evidence_<id> contract, labelled with its lane.
    let preparations = captured.preparations.lock().unwrap();
    let candidate = preparations[0]
        .recall_candidates
        .iter()
        .find(|candidate| candidate.id == format!("evidence_{target}"))
        .expect("the index hit must reach the judge");
    assert_eq!(candidate.content["via"], "utterance_index");
    assert_eq!(candidate.content["source_type"], "raw_utterance");
    assert_eq!(candidate.content["is_durable_belief"], false);
    assert!(!candidate.baseline);
    drop(preparations);

    // The judge's verdict put the old utterance into the language workspace.
    assert!(recalled_text(&captured, 0).contains(&target.to_string()));

    // The first turn caught the index up with the whole backlog; the trace
    // says so without naming any utterance.
    let trace = &session.last_context().unwrap()["recall"]["utterance_index"];
    assert!(
        trace["synced"].as_u64().unwrap() > FILLERS as u64,
        "{trace}"
    );
    assert_eq!(trace["sync_pending"], false);
    assert_eq!(trace["sync_failed"], false);
    assert_eq!(trace["unavailable"], false);
    assert_eq!(trace["offered"], 1);

    // Later turns only index what was said since.
    session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();
    let trace = &session.last_context().unwrap()["recall"]["utterance_index"];
    assert!(trace["synced"].as_u64().unwrap() <= 4, "{trace}");
}

#[test]
fn an_undecided_judge_leaves_an_index_hit_out_of_the_workspace() {
    let (_dir, mut runtime, target) = seeded_runtime(82);
    let captured = Arc::new(Captured::default());
    let mut session = session(&mut runtime, false, &captured);
    let reply = session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();

    // Still offered — the index did its job —
    assert_eq!(reply.c0.as_ref().unwrap().recall_index_hits, 1);
    assert!(
        captured.preparations.lock().unwrap()[0]
            .recall_candidates
            .iter()
            .any(|candidate| candidate.id == format!("evidence_{target}"))
    );
    // — but an Uncertain verdict never injects a non-baseline candidate.
    assert!(!recalled_text(&captured, 0).contains(&target.to_string()));
}
