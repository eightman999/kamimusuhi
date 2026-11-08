//! Declared-path recall end to end: producer-declared `links`/`tags` in
//! state-record payloads reach the relevance judge as extra candidates, and
//! only a reply that actually references a surfaced record warms its usage
//! bookkeeping.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use kamimusuhi_core::c0::{ImprovementKind, ImprovementProposal, OperativeView, ProposalStatus};
use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{C0ProposalId, EvidenceId, MemoryId, ProposalId};
use kamimusuhi_core::memory::{MemoryLink, MemoryQuery, MemoryRepository, MemoryUsage};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaTurnInput};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::workspace::SourceRef;
use kamimusuhi_runtime::c0::{reflection, replay};
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{
    ConversationError, DecisionProvider, DecisionRequest, DecisionResult, LanguageProvider,
    LanguageRequest, LanguageResult, MockLanguageProvider, RecallRelevance, ResponseAssessment,
    ResponseAssessmentRequest, RuleBasedDecisionProvider, TurnPreparation, TurnPreparationRequest,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::FakeReflector;
use kamimusuhi_testkit::fake_persona::FIXTURE_USER_SUBJECT;
use serde_json::json;

const SOURCE_ID: &str = "text-chat:alice";
const QUERY: &str = "いつもの通勤手段は？";

#[derive(Default)]
struct Captured {
    preparations: Mutex<Vec<TurnPreparationRequest>>,
    language_inputs: Mutex<Vec<PersonaTurnInput>>,
}

/// Rule-based judge that additionally marks every declared-path candidate
/// relevant — the compatibility baseline leaves them all Uncertain.
struct LinkJudge {
    captured: Arc<Captured>,
}

impl DecisionProvider for LinkJudge {
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
        for candidate in &request.recall_candidates {
            if candidate.content["via"].is_string() {
                preparation
                    .recall
                    .insert(candidate.id.clone(), RecallRelevance::Relevant);
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

/// Language organ reading one scripted reply per turn.
struct ScriptedLanguage {
    replies: Mutex<VecDeque<String>>,
    captured: Arc<Captured>,
}

impl LanguageProvider for ScriptedLanguage {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.captured
            .language_inputs
            .lock()
            .unwrap()
            .push(request.persona_input.clone());
        let mut result = MockLanguageProvider.generate(request)?;
        result.persona.response_intent = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "了解しました".to_owned());
        Ok(result)
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

/// Activate one state record through the canonical path and return the id
/// the activation minted (looked up by the payload's marker).
fn activate(
    runtime: &Runtime,
    domain: MutationDomain,
    operation: MutationOperation,
    candidate: serde_json::Value,
    evidence_refs: Vec<EvidenceId>,
) -> MemoryId {
    let writer = runtime.claim_writer().unwrap();
    let proposal_id = ProposalId::generate(runtime.ids().as_ref());
    let proposal = MutationProposal {
        proposal_id,
        individual_id: runtime.individual_id(),
        domain,
        operation,
        subject_key: Some("alice".to_owned()),
        candidate: candidate.clone(),
        expected_head: runtime.head().unwrap().expected(),
        evidence_refs,
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("declared-fixture-{proposal_id}"),
        created_at: runtime.now(),
    };
    assert!(
        runtime
            .kernel()
            .submit(&proposal)
            .unwrap()
            .receipt()
            .is_some(),
        "{operation:?} in {domain} must activate"
    );
    MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(domain)
            .about("alice"),
    )
    .unwrap()
    .into_iter()
    .find(|memory| memory.record.payload == candidate)
    .map(|memory| memory.record.state_record_id)
    .expect("the just-activated record must be retrievable")
}

#[test]
fn a_linked_record_reaches_the_judge_and_only_a_real_reference_warms_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(71),
        ResourceImplementation::FakeA,
    )
    .unwrap();

    let evidence = append_utterance(&runtime, "通勤手段について話した");
    // A relationship fact: never in the episodic pool, so a declared link is
    // the only way it can reach the judge.
    let linked = activate(
        &runtime,
        MutationDomain::Relationship,
        MutationOperation::Fact,
        json!({"preference": "ロードバイク通勤", "marker": "linked"}),
        vec![evidence],
    );
    // The lexical seed, declaring the link and the shared tag.
    let seed = activate(
        &runtime,
        MutationDomain::Episodic,
        MutationOperation::Capture,
        json!({
            "text": "通勤手段はロードバイクです",
            "links": [format!("state:{linked}")],
            "tags": ["commute", "tea"],
            "marker": "seed",
        }),
        vec![evidence],
    );
    // Tagged with both seed tags: the top tag-frame pick. Its small id puts
    // it past the normal offer slots once the fillers exist.
    let tagged = activate(
        &runtime,
        MutationDomain::Episodic,
        MutationOperation::Capture,
        json!({"text": "pomegranate sprocket", "tags": ["commute", "tea"], "marker": "tagged"}),
        vec![evidence],
    );
    // Pool fillers: newer ids sort first and occupy the regular candidate
    // slots, so `tagged` can only be offered through the declared path.
    for index in 0..6 {
        activate(
            &runtime,
            MutationDomain::Episodic,
            MutationOperation::Capture,
            json!({"text": format!("filler {index}"), "tags": ["commute"], "marker": index}),
            vec![evidence],
        );
    }

    let captured = Arc::new(Captured::default());
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(LinkJudge {
            captured: Arc::clone(&captured),
        }),
        Some(Box::new(ScriptedLanguage {
            replies: Mutex::new(VecDeque::from(vec![
                format!("記録 {linked} を確認しました"),
                "今日も良い天気ですね".to_owned(),
            ])),
            captured: Arc::clone(&captured),
        })),
    )
    .unwrap();

    let reply = session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();
    let c0 = reply.c0.as_ref().unwrap();
    assert_eq!(c0.recall_link_hits, 1);
    assert_eq!(c0.recall_tag_hits, 1);
    assert_eq!(c0.used_marked, 1);

    // Offered under the same memory_<id> contract, labelled with how each
    // record arrived.
    let preparations = captured.preparations.lock().unwrap();
    let by_id: std::collections::BTreeMap<_, _> = preparations[0]
        .recall_candidates
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect();
    let linked_candidate = by_id[format!("memory_{linked}").as_str()];
    assert_eq!(linked_candidate.content["via"], "link_hop_1");
    assert_eq!(linked_candidate.content["source_type"], "durable_memory");
    let tagged_candidate = by_id[format!("memory_{tagged}").as_str()];
    assert_eq!(tagged_candidate.content["via"], "tag");
    drop(preparations);

    // The judge's verdict injected the linked record into the language
    // workspace under its own domain.
    let inputs = captured.language_inputs.lock().unwrap();
    let surfaced: Vec<MemoryId> = inputs[0]
        .envelope
        .relationship
        .iter()
        .chain(inputs[0].envelope.episodic.iter())
        .filter_map(|item| match item.source_ref {
            SourceRef::Memory {
                state_record_id, ..
            } => Some(state_record_id),
            _ => None,
        })
        .collect();
    assert!(
        surfaced.contains(&linked),
        "the linked record must reach the language workspace: {surfaced:?}"
    );
    // Both declared hits were judged relevant, so under episodic_top_k = 2
    // they displace the uncertain baseline seed — the documented rule.
    assert!(surfaced.contains(&tagged));
    assert!(!surfaced.contains(&seed));
    drop(inputs);

    // A real reference warms the usage bookkeeping exactly once.
    assert_eq!(runtime.store().usage(&linked).unwrap().use_count, 1);
    assert!(runtime.store().usage(&seed).is_none());

    // Second turn, same recall path, but the reply names nothing: an
    // injected-but-unreferenced record is never re-warmed.
    let reply = session.turn(&mut runtime, QUERY, |_| Ok(())).unwrap();
    assert_eq!(reply.c0.as_ref().unwrap().used_marked, 0);
    assert_eq!(runtime.store().usage(&linked).unwrap().use_count, 1);
}

// ---------------------------------------------------------------------------
// Producer side: the reflector declares metadata on its captures
// ---------------------------------------------------------------------------

/// Three dialogue turns that leave two uncaptured user utterances — the
/// minimum the fixture reflector's episodic-capture rule needs — plus one
/// relationship record for `state:` links to point at.
fn capture_setup(runtime: &mut Runtime) -> (DialogueSession, EvidenceId, MemoryId) {
    let mut session =
        DialogueSession::start(runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    session
        .turn(runtime, "私はほうじ茶が好き。覚えておいて", |_| Ok(()))
        .unwrap();
    session
        .turn(runtime, "今日はいい天気だった", |_| Ok(()))
        .unwrap();
    let third = session
        .turn(runtime, "夜は少し涼しかった", |_| Ok(()))
        .unwrap();
    let relationship = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Relationship)
            .about(FIXTURE_USER_SUBJECT),
    )
    .unwrap();
    let relationship_id = relationship[0].record.state_record_id;
    (session, third.input_evidence_id, relationship_id)
}

/// The episodic record the reflection cycle's capture draft produced.
fn captured_episode(runtime: &Runtime) -> kamimusuhi_core::memory::StateRecord {
    let episodic = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Episodic)
            .about(FIXTURE_USER_SUBJECT),
    )
    .unwrap();
    assert_eq!(
        episodic.len(),
        1,
        "the capture draft must produce one record"
    );
    episodic[0].record.clone()
}

#[test]
fn a_reflector_may_declare_recall_metadata_on_a_capture() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(72),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let (session, third_evidence, relationship_id) = capture_setup(&mut runtime);

    let reflector = FakeReflector::default().with_episodic_declared(json!({
        "tags": ["weather", "preference"],
        "links": [
            format!("evidence:{third_evidence}"),
            format!("state:{relationship_id}"),
        ],
    }));
    let mut writer = None;
    reflection::run_with_reflector(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
        &reflector,
    )
    .unwrap();

    let record = captured_episode(&runtime);
    assert_eq!(
        record.tags(),
        vec!["preference".to_owned(), "weather".to_owned()]
    );
    assert_eq!(
        record.links(),
        vec![
            MemoryLink::Evidence(third_evidence),
            MemoryLink::State(relationship_id),
        ]
    );
    assert_eq!(record.valid_to(), None);
    assert_eq!(record.valid_from(), None);
}

#[test]
fn malformed_declared_metadata_refuses_the_whole_capture() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(73),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let (session, _evidence, _relationship) = capture_setup(&mut runtime);

    let reflector = FakeReflector::default().with_episodic_declared(json!({
        "tags": "not-an-array",
        "links": [
            42,
            "state:not-hex",
            // Well-formed but never presented: the whole item must be refused.
            format!("state:{}", MemoryId::from_u128(0xDEAD)),
        ],
        "valid_from": "not-a-time",
        "valid_to": "whenever",
    }));
    let mut writer = None;
    reflection::run_with_reflector(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
        &reflector,
    )
    .unwrap();

    let records = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Episodic)
            .about(FIXTURE_USER_SUBJECT),
    )
    .unwrap();
    assert!(
        records.is_empty(),
        "malformed identifiers must not be repaired into accepted state"
    );
}

// ---------------------------------------------------------------------------
// Replay coverage: replayed turns expand through the declared path
// ---------------------------------------------------------------------------

#[test]
fn replay_expands_recalled_seeds_through_declared_links() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(74),
        ResourceImplementation::FakeA,
    )
    .unwrap();

    let evidence = append_utterance(&runtime, "通勤手段はロードバイクです");
    // Reachable only through the declared link: no lexical overlap, no tags.
    let linked = activate(
        &runtime,
        MutationDomain::Episodic,
        MutationOperation::Capture,
        json!({"text": "xyzzy plugh", "marker": "linked"}),
        vec![evidence],
    );
    let seed = activate(
        &runtime,
        MutationDomain::Episodic,
        MutationOperation::Capture,
        json!({
            "text": "通勤手段はロードバイクです",
            "links": [format!("state:{linked}")],
            "marker": "seed",
        }),
        vec![evidence],
    );

    // Turn the declared budget off in the candidate view: the linked
    // record's workspace item must be exactly what disappears.
    let proposal = ImprovementProposal {
        proposal_id: C0ProposalId::generate(runtime.ids().as_ref()),
        individual_id: runtime.individual_id(),
        kind: ImprovementKind::RetrievalUpdate,
        target: "retrieval.recall_declared_budget".to_owned(),
        target_key: None,
        old_value: OperativeView::default()
            .params
            .get("retrieval.recall_declared_budget"),
        proposed_value: json!(0),
        evidence_refs: vec![],
        expected_effect: None,
        risk: None,
        confidence: None,
        status: ProposalStatus::Pending,
        rejection_reason: None,
        reflection_id: None,
        created_at: runtime.now(),
        decided_at: None,
        activation_seq: None,
    };
    let report = replay::replay_proposal(&runtime, "alice", &proposal).unwrap();
    assert!(report.all_completed);
    let surfaced = report
        .deltas
        .iter()
        .find(|delta| delta.axis == "memories_surfaced")
        .unwrap();
    assert!(
        surfaced.baseline > surfaced.candidate,
        "the declared-path extra must surface under the baseline view only: {surfaced:?}"
    );

    // Replay is not a turn: no record warms because the model never ran.
    assert!(runtime.store().usage(&linked).is_none());
    assert!(runtime.store().usage(&seed).is_none());
}
