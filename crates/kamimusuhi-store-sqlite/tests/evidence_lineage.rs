//! T07: derived-evidence multiplication and correction.
//!
//! Summaries and reflections of one root record must not multiply
//! independent support (`EvidenceSnapshot::independent_support`,
//! `resolve_roots`), and a correction must retract cleanly: no destructive
//! rewrite of the corrected evidence, the superseded record kept for
//! inspection, and any other active record that rested on the corrected
//! evidence taken out of the current view rather than left standing (audit
//! A03).

mod common;

use common::*;
use kamimusuhi_core::audit::AuditKind;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityStore, ExpectedHead, WriterIdentity,
};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence, NewEvidenceLink,
    RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, MemoryId, ProposalId};
use kamimusuhi_core::memory::{LifecycleState, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_testkit::FixedClock;

const SUMMARY_1: EvidenceId = EvidenceId::from_u128(0xE10);
const SUMMARY_2: EvidenceId = EvidenceId::from_u128(0xE11);
const SUMMARY_3: EvidenceId = EvidenceId::from_u128(0xE12);
const SUMMARY_OF_SUMMARY: EvidenceId = EvidenceId::from_u128(0xE13);
const CORRECTION: EvidenceId = EvidenceId::from_u128(0xE14);
const FRESH_UTTERANCE: EvidenceId = EvidenceId::from_u128(0xE15);

const SUBJECT_1: &str = "user-fixture-1";
const SUBJECT_2: &str = "user-fixture-2";

#[allow(clippy::too_many_arguments)]
fn proposal_with(
    proposal_id: u128,
    operation: MutationOperation,
    subject_key: &str,
    candidate: serde_json::Value,
    evidence_refs: Vec<EvidenceId>,
    supersedes: Option<MemoryId>,
    expected_head: ExpectedHead,
    writer: WriterIdentity,
) -> MutationProposal {
    MutationProposal {
        proposal_id: ProposalId::from_u128(proposal_id),
        individual_id: INDIVIDUAL,
        domain: MutationDomain::Relationship,
        operation,
        subject_key: Some(subject_key.to_owned()),
        candidate,
        expected_head,
        evidence_refs,
        supersedes,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("idem-{proposal_id}"),
        created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
    }
}

fn derived(
    store: &kamimusuhi_store_sqlite::SqliteStore,
    evidence_id: EvidenceId,
    kind: EvidenceKind,
) {
    store
        .append(NewEvidence {
            evidence_id,
            individual_id: INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind,
            origin_class: OriginClass::Inferred,
            payload: serde_json::json!({ "text": "derived text" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
}

fn link(
    store: &kamimusuhi_store_sqlite::SqliteStore,
    from: EvidenceId,
    to: EvidenceId,
    relation: EvidenceRelation,
) {
    store
        .link(NewEvidenceLink {
            from_evidence_id: from,
            to_evidence_id: to,
            relation,
        })
        .unwrap();
}

fn current_record(
    store: &kamimusuhi_store_sqlite::SqliteStore,
    subject: &str,
) -> kamimusuhi_core::memory::AttributedMemory {
    let mut records = store
        .retrieve(&MemoryQuery::current(INDIVIDUAL).about(subject))
        .unwrap();
    assert_eq!(
        records.len(),
        1,
        "expected exactly one current record for {subject}"
    );
    records.remove(0)
}

#[test]
fn derived_summaries_do_not_multiply_independent_support() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);

    // Three summaries/reflections of the fixture user utterance, plus a
    // summary-of-a-summary chain.
    derived(&store, SUMMARY_1, EvidenceKind::Summary);
    derived(&store, SUMMARY_2, EvidenceKind::Reflection);
    derived(&store, SUMMARY_3, EvidenceKind::Summary);
    derived(&store, SUMMARY_OF_SUMMARY, EvidenceKind::Summary);
    link(&store, SUMMARY_1, EVIDENCE, EvidenceRelation::DerivedFrom);
    link(&store, SUMMARY_2, EVIDENCE, EvidenceRelation::DerivedFrom);
    link(&store, SUMMARY_3, EVIDENCE, EvidenceRelation::DerivedFrom);
    link(
        &store,
        SUMMARY_OF_SUMMARY,
        SUMMARY_3,
        EvidenceRelation::DerivedFrom,
    );

    let kernel = kernel(store);
    let proposal = proposal_with(
        100,
        MutationOperation::Fact,
        SUBJECT_1,
        serde_json::json!({ "preference": "ほうじ茶" }),
        vec![
            EVIDENCE,
            SUMMARY_1,
            SUMMARY_2,
            SUMMARY_3,
            SUMMARY_OF_SUMMARY,
        ],
        None,
        boot.head.expected(),
        boot.writer,
    );
    assert!(matches!(
        kernel.submit(&proposal).unwrap(),
        ActivationOutcome::Activated(_)
    ));

    let memory = current_record(kernel.store(), SUBJECT_1);
    // Five citations, one root: three summaries of one conversation are not
    // three independent supports.
    assert_eq!(memory.independent_evidence_count, 1);
    assert_eq!(memory.root_evidence, vec![EVIDENCE]);

    // Lineage stays inspectable in both directions.
    let from_summary_1 = kernel.store().links_from(SUMMARY_1).unwrap();
    assert_eq!(from_summary_1.len(), 1);
    assert_eq!(from_summary_1[0].to_evidence_id, EVIDENCE);

    let mut to_root: Vec<EvidenceId> = kernel
        .store()
        .links_to(EVIDENCE)
        .unwrap()
        .into_iter()
        .map(|l| l.from_evidence_id)
        .collect();
    to_root.sort_by_key(|id| id.to_string());
    let mut expected = vec![SUMMARY_1, SUMMARY_2, SUMMARY_3];
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(
        to_root, expected,
        "only directly-linked summaries name the root"
    );

    let from_chain = kernel.store().links_from(SUMMARY_OF_SUMMARY).unwrap();
    assert_eq!(from_chain.len(), 1);
    assert_eq!(from_chain[0].to_evidence_id, SUMMARY_3);
}

#[test]
fn correction_supersedes_its_target_and_invalidates_other_active_facts_on_the_corrected_evidence() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    // Fact 1: what the correction will explicitly target.
    let f1 = proposal_with(
        100,
        MutationOperation::Fact,
        SUBJECT_1,
        serde_json::json!({ "preference": "ほうじ茶" }),
        vec![EVIDENCE],
        None,
        boot.head.expected(),
        boot.writer,
    );
    assert!(matches!(
        kernel.submit(&f1).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    let target = current_record(kernel.store(), SUBJECT_1)
        .record
        .state_record_id;

    // Fact 2: a distinct fact that also rests on EVIDENCE but is not the
    // correction's named target.
    let head_after_f1 = kernel.store().load_head(INDIVIDUAL).unwrap();
    let f2 = proposal_with(
        101,
        MutationOperation::Fact,
        SUBJECT_2,
        serde_json::json!({ "preference": "玄米茶" }),
        vec![EVIDENCE],
        None,
        head_after_f1.expected(),
        boot.writer,
    );
    assert!(matches!(
        kernel.submit(&f2).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    let other = current_record(kernel.store(), SUBJECT_2)
        .record
        .state_record_id;

    let evidence_before = kernel.store().get(EVIDENCE).unwrap().unwrap();

    // The correction: a Correction record linked to the root, plus a fresh
    // user utterance, replacing fact 1.
    kernel
        .store()
        .append(NewEvidence {
            evidence_id: CORRECTION,
            individual_id: INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::Correction,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({ "text": "実はほうじ茶ではなかった" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    link(
        kernel.store(),
        CORRECTION,
        EVIDENCE,
        EvidenceRelation::Corrects,
    );
    kernel
        .store()
        .append(user_utterance(FRESH_UTTERANCE, "訂正: 抹茶が好き"))
        .unwrap();

    let head_after_f2 = kernel.store().load_head(INDIVIDUAL).unwrap();
    let correction_proposal = proposal_with(
        102,
        MutationOperation::Correction,
        SUBJECT_1,
        serde_json::json!({ "preference": "抹茶" }),
        vec![CORRECTION, FRESH_UTTERANCE],
        Some(target),
        head_after_f2.expected(),
        boot.writer,
    );
    let ActivationOutcome::Activated(_) = kernel.submit(&correction_proposal).unwrap() else {
        panic!("expected the correction to activate");
    };

    // The corrected evidence itself is never rewritten.
    assert_eq!(
        kernel.store().get(EVIDENCE).unwrap().unwrap(),
        evidence_before
    );

    // The old fact is gone from the current view but kept for inspection.
    let current_subject_1 = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).about(SUBJECT_1))
        .unwrap();
    assert_eq!(current_subject_1.len(), 1);
    let replacement = current_subject_1[0].record.state_record_id;
    assert_ne!(replacement, target);

    let history_subject_1 = kernel
        .store()
        .retrieve(
            &MemoryQuery::current(INDIVIDUAL)
                .about(SUBJECT_1)
                .including_history(),
        )
        .unwrap();
    let old = history_subject_1
        .iter()
        .find(|m| m.record.state_record_id == target)
        .expect("superseded record must still be inspectable");
    assert_eq!(old.record.lifecycle_state, LifecycleState::Superseded);
    assert_eq!(old.record.superseded_by_state_record_id, Some(replacement));

    // The other active fact that rested on the corrected evidence is taken
    // out of the current view, not silently left standing.
    let current_subject_2 = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).about(SUBJECT_2))
        .unwrap();
    assert!(current_subject_2.is_empty());
    let history_subject_2 = kernel
        .store()
        .retrieve(
            &MemoryQuery::current(INDIVIDUAL)
                .about(SUBJECT_2)
                .including_history(),
        )
        .unwrap();
    let invalidated = history_subject_2
        .iter()
        .find(|m| m.record.state_record_id == other)
        .expect("invalidated record must still be inspectable");
    assert_eq!(
        invalidated.record.lifecycle_state,
        LifecycleState::Invalidated
    );

    let kinds: Vec<AuditKind> = kernel
        .store()
        .audit_events(INDIVIDUAL)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&AuditKind::StateSuperseded));
}
