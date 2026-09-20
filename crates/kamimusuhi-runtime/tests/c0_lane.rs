//! C0 lane experiments and invariants.
//!
//! C0.1 persistent recall across restart, C0.2 correction, C0.3 accepted
//! self-improvement reaching the next workspace, C0.4 rollback of the derived
//! head — plus the spec's required invariants: a proposal is not state, a
//! rejection is not an effect, malformed model output is not admitted, a
//! provider swap is not an identity change, subjects stay isolated, and
//! canonical events stay unique, ordered and append-only.

use kamimusuhi_core::c0::{
    ImprovementDraft, ImprovementKind, ImprovementProposal, OperativeView, ProposalStatus,
    SelfField, validate_improvement_draft,
};
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceRelation, EvidenceStore};
use kamimusuhi_core::ids::{C0ProposalId, EvidenceId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_runtime::c0::{self, reflection, replay};
use kamimusuhi_runtime::config::ReflectorBackend;
use kamimusuhi_runtime::dialogue::{DialogueReply, DialogueSession};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::fake_persona::FIXTURE_USER_SUBJECT;

fn init(dir: &std::path::Path) -> Runtime {
    let mut runtime = Runtime::init(
        dir,
        RuntimeOptions::deterministic(10),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut config = runtime.config().clone();
    // The deterministic fixture reflector; the actor stays the fake persona,
    // so evaluator and actor are distinct components here.
    config.c0.reflector.backend = ReflectorBackend::Fake;
    runtime.save_config(config).unwrap();
    runtime
}

fn reopen(dir: &std::path::Path) -> Runtime {
    Runtime::open(dir, RuntimeOptions::deterministic(10_000)).unwrap()
}

fn turn(session: &mut DialogueSession, runtime: &mut Runtime, text: &str) -> DialogueReply {
    session.turn(runtime, text, |_| Ok(())).unwrap()
}

fn relationship_memories(runtime: &Runtime, subject: &str) -> Vec<String> {
    MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Relationship)
            .about(subject),
    )
    .unwrap()
    .iter()
    .map(|m| m.record.payload.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// C0.1 — persistent recall
// ---------------------------------------------------------------------------

#[test]
fn c01_a_new_process_recalls_what_another_session_learned() {
    let dir = tempfile::tempdir().unwrap();
    let individual;
    {
        let mut runtime = init(dir.path());
        individual = runtime.individual_id();
        let mut session =
            DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
        let reply = turn(
            &mut session,
            &mut runtime,
            "私はほうじ茶が好き。覚えておいて",
        );
        // The persona's draft went through the kernel and activated.
        assert_eq!(reply.c0.as_ref().unwrap().drafts_activated, 1);
        assert_eq!(
            relationship_memories(&runtime, FIXTURE_USER_SUBJECT).len(),
            1
        );
        runtime.stopping();
    }
    {
        // A fresh process: the same individual, the same record, surfacing
        // into a new session's workspace.
        let mut runtime = reopen(dir.path());
        assert_eq!(runtime.individual_id(), individual);
        let mut session =
            DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
        let reply = turn(&mut session, &mut runtime, "私の好きなお茶は何だっけ？");
        assert!(reply.c0.as_ref().unwrap().memories_surfaced >= 1);
        let context = session.last_context().unwrap();
        let relationship = context["relationship_memories"].as_array().unwrap();
        assert!(
            relationship
                .iter()
                .any(|m| m.to_string().contains("ほうじ茶")),
            "restarted session must surface the remembered preference"
        );
        runtime.stopping();
    }
}

// ---------------------------------------------------------------------------
// C0.2 — correction
// ---------------------------------------------------------------------------

#[test]
fn c02_correction_supersedes_the_belief_not_the_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    let first = turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    let correction = turn(
        &mut session,
        &mut runtime,
        "さっきの発言は間違いです。訂正します。",
    );

    let mut writer = None;
    let report = reflection::run(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
    )
    .unwrap();
    assert_eq!(report.corrections_linked, 1);
    assert_eq!(report.canonical_drafts_activated, 1);

    // The correction event links back to what it corrects.
    let links = runtime
        .store()
        .links_from(correction.input_evidence_id)
        .unwrap();
    assert!(
        links
            .iter()
            .any(|l| l.relation == EvidenceRelation::Corrects
                && l.to_evidence_id == first.input_evidence_id),
        "correction must carry a Corrects link to the first utterance"
    );

    // The superseded record remains in history; the current view holds only
    // the correction's replacement.
    let history = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Relationship)
            .about(FIXTURE_USER_SUBJECT)
            .including_history(),
    )
    .unwrap();
    let current = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(MutationDomain::Relationship)
            .about(FIXTURE_USER_SUBJECT),
    )
    .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(current.len(), 1);
    assert!(current[0].record.payload.to_string().contains("訂正"));

    // Canonical evidence is untouched by the correction: both utterances
    // are still right where they were appended.
    assert!(
        runtime
            .store()
            .get(first.input_evidence_id)
            .unwrap()
            .is_some()
    );
    runtime.stopping();
}

// ---------------------------------------------------------------------------
// C0.3 — self improvement: proposal → gate → activation → next workspace
// ---------------------------------------------------------------------------

#[test]
fn c03_an_accepted_proposal_changes_the_next_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    turn(&mut session, &mut runtime, "今日はいい天気だった");
    assert_eq!(c0::operative(&runtime).unwrap().activation_seq, 0);
    assert_eq!(
        c0::operative(&runtime)
            .unwrap()
            .view
            .self_model
            .entry_count(),
        0
    );

    let mut writer = None;
    let report = reflection::run(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
    )
    .unwrap();
    assert!(
        report
            .proposals_created
            .iter()
            .any(|p| p.kind == ImprovementKind::SelfUpdate),
        "the fixture reflector must draft a self-model update"
    );
    assert!(!report.activations.is_empty(), "gate accepted nothing");

    // The operative view now carries the self entry, and the next turn's
    // workspace actually surfaces it — acceptance is observable, not just
    // recorded.
    let operative = c0::operative(&runtime).unwrap();
    assert!(operative.activation_seq >= 1);
    let capabilities = operative.view.self_model.field(SelfField::Capabilities);
    assert!(
        capabilities
            .iter()
            .any(|e| e.key.as_deref() == Some("relationship_recall"))
    );
    turn(&mut session, &mut runtime, "次の話題に移ろう");
    let context = session.last_context().unwrap();
    assert!(
        context["self_state"]
            .to_string()
            .contains("relationship_recall"),
        "accepted self update must appear in the next workspace"
    );
    runtime.stopping();
}

// ---------------------------------------------------------------------------
// C0.4 — rollback
// ---------------------------------------------------------------------------

#[test]
fn c04_rollback_moves_the_derived_head_and_keeps_canonical_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    turn(&mut session, &mut runtime, "振り返り用の二つ目の発話");
    let mut writer = None;
    reflection::run(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
    )
    .unwrap();
    let before = c0::operative(&runtime).unwrap();
    assert!(before.activation_seq >= 1);
    let canonical_before = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 512)
        .unwrap()
        .len();

    let outcome = c0::rollback(&runtime, Some(session.session_id()), None).unwrap();
    assert_eq!(outcome.rolled_back_seq, before.activation_seq);
    let after = c0::operative(&runtime).unwrap();
    assert_eq!(after.activation_seq, outcome.restored_seq);
    assert_eq!(after.activation_seq + 1, before.activation_seq);
    // The crossed activation is marked, not deleted.
    let crossed = runtime
        .store()
        .c0_activation(runtime.individual_id(), outcome.rolled_back_seq)
        .unwrap()
        .unwrap();
    assert!(crossed.rolled_back_at.is_some());
    // Canonical history only grew (the rollback narration) — nothing was
    // rewritten.
    let canonical_after = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 512)
        .unwrap()
        .len();
    assert!(canonical_after > canonical_before);
    runtime.stopping();
}

// ---------------------------------------------------------------------------
// Spec invariants
// ---------------------------------------------------------------------------

fn pending_proposal(
    runtime: &Runtime,
    target: &str,
    value: serde_json::Value,
) -> ImprovementProposal {
    ImprovementProposal {
        proposal_id: C0ProposalId::generate(runtime.ids().as_ref()),
        individual_id: runtime.individual_id(),
        kind: ImprovementKind::RetrievalUpdate,
        target: target.to_owned(),
        target_key: None,
        old_value: OperativeView::default().params.get(target),
        proposed_value: value,
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
    }
}

#[test]
fn a_proposal_row_is_not_state_and_a_rejection_is_not_an_effect() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let baseline = c0::operative(&runtime).unwrap().view;

    let proposal = pending_proposal(&runtime, "retrieval.evidence_top_k", serde_json::json!(6));
    assert!(runtime.store().c0_insert_proposal(&proposal).unwrap());
    // A pending row changed nothing the conversation runs under.
    assert_eq!(c0::operative(&runtime).unwrap().view, baseline);
    assert_eq!(c0::operative(&runtime).unwrap().activation_seq, 0);

    runtime
        .store()
        .c0_decide(
            proposal.proposal_id,
            ProposalStatus::Rejected,
            Some("test rejection"),
            runtime.now(),
        )
        .unwrap();
    assert_eq!(c0::operative(&runtime).unwrap().view, baseline);
    // Rejected proposals do not come back.
    assert!(
        runtime
            .store()
            .c0_decide(
                proposal.proposal_id,
                ProposalStatus::Accepted,
                None,
                runtime.now(),
            )
            .is_err()
    );
    runtime.stopping();
}

#[test]
fn malformed_improvement_drafts_are_rejected_at_the_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let view = OperativeView::default();
    let allowed: std::collections::BTreeMap<EvidenceId, EvidenceKind> = Default::default();
    let draft = ImprovementDraft {
        kind: ImprovementKind::PolicyUpdate,
        target: "personality.make_me_smarter".to_owned(), // not in vocabulary
        target_key: None,
        proposed_value: serde_json::json!(1),
        evidence_refs: vec![],
        expected_effect: None,
        risk: None,
        confidence: None,
    };
    assert!(validate_improvement_draft(&draft, &allowed, &view).is_err());
    let out_of_bounds = ImprovementDraft {
        kind: ImprovementKind::RetrievalUpdate,
        target: "retrieval.evidence_top_k".to_owned(),
        target_key: None,
        proposed_value: serde_json::json!(9_999),
        evidence_refs: vec![],
        expected_effect: None,
        risk: None,
        confidence: None,
    };
    assert!(validate_improvement_draft(&out_of_bounds, &allowed, &view).is_err());
    assert!(
        runtime
            .store()
            .c0_proposals(runtime.individual_id(), None)
            .unwrap()
            .is_empty(),
        "rejected drafts persist nothing"
    );
    runtime.stopping();
}

#[test]
fn swapping_the_persona_backend_preserves_identity_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let individual;
    let evidence_before;
    {
        let mut runtime = init(dir.path());
        individual = runtime.individual_id();
        let mut session =
            DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
        turn(&mut session, &mut runtime, "プロバイダを跨いで覚えているか");
        evidence_before = runtime
            .store()
            .c0_evidence_pool(individual, 512)
            .unwrap()
            .len();
        // Swap the configured backend. Config rewrite only — no canonical
        // write, no new identity.
        let mut config = runtime.config().clone();
        config.persona.backend = kamimusuhi_runtime::config::PersonaBackendKind::OpenaiCompatible;
        config.persona.provider = None; // not called in this test
        runtime.save_config(config).unwrap();
        runtime.stopping();
    }
    let runtime = reopen(dir.path());
    assert_eq!(runtime.individual_id(), individual);
    assert_eq!(
        runtime
            .store()
            .c0_evidence_pool(individual, 512)
            .unwrap()
            .len(),
        evidence_before,
        "a provider swap must not lose or rewrite history"
    );
    runtime.stopping();
}

#[test]
fn subjects_do_not_share_retained_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    assert_eq!(
        relationship_memories(&runtime, FIXTURE_USER_SUBJECT).len(),
        1
    );
    assert!(relationship_memories(&runtime, "someone-else").is_empty());
    let retrieved = c0::retrieve_memories(
        runtime.store(),
        runtime.individual_id(),
        "someone-else",
        "ほうじ茶",
        &OperativeView::default().params,
    )
    .unwrap();
    assert!(retrieved.is_empty());
    runtime.stopping();
}

#[test]
fn canonical_evidence_ids_are_unique_and_monotonic() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    for text in [
        "私はほうじ茶が好き。覚えておいて",
        "今日は暑かった",
        "三つ目の発話",
    ] {
        turn(&mut session, &mut runtime, text);
    }
    let pool = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 512)
        .unwrap();
    assert!(pool.len() >= 6); // 3 user + 3 agent utterances at minimum
    let ids: std::collections::BTreeSet<_> = pool.iter().map(|r| r.evidence_id).collect();
    assert_eq!(ids.len(), pool.len(), "evidence ids must be unique");
    // Pool is newest-first: timestamps must be non-increasing.
    for pair in pool.windows(2) {
        assert!(pair[0].created_at >= pair[1].created_at);
    }
    assert!(pool.iter().all(|r| matches!(
        r.kind,
        EvidenceKind::UserUtterance | EvidenceKind::AgentUtterance | EvidenceKind::SystemEvent
    )));
    runtime.stopping();
}

#[test]
fn retrieval_is_deterministic_for_the_same_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    turn(&mut session, &mut runtime, "今日の天気について話そう");
    let params = OperativeView::default().params;
    let run = || {
        c0::retrieve_memories(
            runtime.store(),
            runtime.individual_id(),
            FIXTURE_USER_SUBJECT,
            "好きなお茶",
            &params,
        )
        .unwrap()
        .iter()
        .map(|m| m.record.state_record_id)
        .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
    runtime.stopping();
}

#[test]
fn a_replay_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    turn(
        &mut session,
        &mut runtime,
        "私はほうじ茶が好き。覚えておいて",
    );
    turn(&mut session, &mut runtime, "リプレイ対象の二つ目の発話");
    let proposal = pending_proposal(&runtime, "retrieval.evidence_top_k", serde_json::json!(6));
    runtime.store().c0_insert_proposal(&proposal).unwrap();
    let evidence_before = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 512)
        .unwrap()
        .len();
    let sessions_before: i64 = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    let report = replay::replay_proposal(&runtime, FIXTURE_USER_SUBJECT, &proposal).unwrap();
    assert_eq!(report.turns, 2);
    assert!(report.all_completed);
    let evidence_after = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 512)
        .unwrap()
        .len();
    let sessions_after: i64 = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(evidence_after, evidence_before);
    assert_eq!(sessions_after, sessions_before);
    runtime.stopping();
}
