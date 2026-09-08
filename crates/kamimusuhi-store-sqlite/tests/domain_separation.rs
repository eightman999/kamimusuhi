//! Domain separation regression (plan §6, `kamimusuhi_core::domain_separation`).
//!
//! "Someone said something" and "Kamimusuhi holds this about itself" must
//! stay durably separate: a raw utterance and the fact derived from it live
//! in different tables, a user's stated preference never leaks into the self
//! domain, and evidence owned by one individual can never support another's
//! mutation. Every rejection here must be structural (IDs, kinds, ownership),
//! never a read of payload text.

mod common;

use common::*;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityStore, ExpectedHead, NewIndividual, WriterIdentity,
};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{
    BootId, CommitId, EvidenceId, IndividualId, ProposalId, SessionId, TurnId,
};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass, ReasonCode,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_testkit::FixedClock;

const OTHER_INDIVIDUAL: IndividualId = IndividualId::from_u128(0xA2);
const OTHER_ROOT: CommitId = CommitId::from_u128(0xC2);
const OTHER_BOOT: BootId = BootId::from_u128(0xB9);
const OTHER_SESSION: SessionId = SessionId::from_u128(0x52);
const OTHER_TURN: TurnId = TurnId::from_u128(0x72);
const OTHER_EVIDENCE: EvidenceId = EvidenceId::from_u128(0xE9);

fn proposal_with(
    proposal_id: u128,
    domain: MutationDomain,
    operation: MutationOperation,
    subject_key: Option<&str>,
    evidence_refs: Vec<EvidenceId>,
    expected_head: ExpectedHead,
    writer: WriterIdentity,
) -> MutationProposal {
    MutationProposal {
        proposal_id: ProposalId::from_u128(proposal_id),
        individual_id: INDIVIDUAL,
        domain,
        operation,
        subject_key: subject_key.map(str::to_owned),
        candidate: serde_json::json!({ "preference": "ほうじ茶" }),
        expected_head,
        evidence_refs,
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("idem-{proposal_id}"),
        created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
    }
}

#[test]
fn raw_utterance_and_derived_fact_are_separate_durable_records() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let before_evidence = kernel.store().get(EVIDENCE).unwrap().unwrap();

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let ActivationOutcome::Activated(_) = kernel.submit(&proposal).unwrap() else {
        panic!("expected activation");
    };

    // The evidence row is untouched by activation: a different table, a
    // different record, no rewrite in place.
    let after_evidence = kernel.store().get(EVIDENCE).unwrap().unwrap();
    assert_eq!(before_evidence, after_evidence);

    let memories = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).in_domain(MutationDomain::Relationship))
        .unwrap();
    assert_eq!(memories.len(), 1);
    let state_record = &memories[0].record;
    // The state record is a distinct id in a distinct table, but it cites the
    // evidence it rests on rather than duplicating it.
    assert_ne!(
        state_record.state_record_id.to_string(),
        EVIDENCE.to_string()
    );
    assert_eq!(state_record.evidence_refs, vec![EVIDENCE]);
}

#[test]
fn a_user_preference_never_appears_in_the_self_domain() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    assert!(matches!(
        kernel.submit(&proposal).unwrap(),
        ActivationOutcome::Activated(_)
    ));

    let self_memories = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).in_domain(MutationDomain::SelfModel))
        .unwrap();
    assert!(self_memories.is_empty());

    // The same user utterance is refused as testimony about Kamimusuhi itself.
    let head = kernel.store().load_head(INDIVIDUAL).unwrap();
    let self_proposal = proposal_with(
        101,
        MutationDomain::SelfModel,
        MutationOperation::Fact,
        None,
        vec![EVIDENCE],
        head.expected(),
        boot.writer,
    );
    let ActivationOutcome::Rejected(decision) = kernel.submit(&self_proposal).unwrap() else {
        panic!("expected rejection");
    };
    assert_eq!(decision.reason_code, ReasonCode::SelfDomainContamination);
    assert_eq!(kernel.store().load_head(INDIVIDUAL).unwrap(), head);
}

#[test]
fn evidence_owned_by_another_individual_cannot_support_this_ones_mutation() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);

    // A second individual in the same database with its own root commit and
    // its own utterance.
    store
        .create_individual(NewIndividual {
            individual_id: OTHER_INDIVIDUAL,
            root_commit_id: OTHER_ROOT,
            node_id: NODE,
            boot_id: OTHER_BOOT,
        })
        .unwrap();
    store
        .open_session(NewSession {
            session_id: OTHER_SESSION,
            individual_id: OTHER_INDIVIDUAL,
        })
        .unwrap();
    store
        .record_turn(NewTurn {
            turn_id: OTHER_TURN,
            session_id: OTHER_SESSION,
            individual_id: OTHER_INDIVIDUAL,
            sequence: 0,
        })
        .unwrap();
    store
        .append(NewEvidence {
            evidence_id: OTHER_EVIDENCE,
            individual_id: OTHER_INDIVIDUAL,
            session_id: Some(OTHER_SESSION),
            turn_id: Some(OTHER_TURN),
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({ "text": "別人の発話" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();

    let kernel = kernel(store);
    let proposal = proposal_with(
        100,
        MutationDomain::Relationship,
        MutationOperation::Fact,
        Some("user-fixture"),
        vec![OTHER_EVIDENCE],
        boot.head.expected(),
        boot.writer,
    );
    let ActivationOutcome::Rejected(decision) = kernel.submit(&proposal).unwrap() else {
        panic!("expected rejection");
    };
    assert_eq!(decision.reason_code, ReasonCode::EvidenceOwnerMismatch);
    assert_eq!(
        kernel.store().find_decision(proposal.proposal_id).unwrap(),
        Some(decision)
    );
    assert_eq!(kernel.store().load_head(INDIVIDUAL).unwrap(), boot.head);
}

#[test]
fn library_excerpt_and_resource_result_alone_cannot_make_a_relationship_fact() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);

    let library_id = EvidenceId::from_u128(0xE3);
    let resource_id = EvidenceId::from_u128(0xE4);
    store
        .append(NewEvidence {
            evidence_id: library_id,
            individual_id: INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::LibraryExcerpt,
            origin_class: OriginClass::Inferred,
            payload: serde_json::json!({ "text": "図書館からの引用" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    store
        .append(NewEvidence {
            evidence_id: resource_id,
            individual_id: INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::ResourceResult,
            origin_class: OriginClass::Observed,
            payload: serde_json::json!({ "text": "外部リソースの結果" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();

    let kernel = kernel(store);
    let proposal = proposal_with(
        100,
        MutationDomain::Relationship,
        MutationOperation::Fact,
        Some("user-fixture"),
        vec![library_id, resource_id],
        boot.head.expected(),
        boot.writer,
    );
    let ActivationOutcome::Rejected(decision) = kernel.submit(&proposal).unwrap() else {
        panic!("expected rejection");
    };
    assert_eq!(decision.reason_code, ReasonCode::EvidenceDomainMismatch);
    assert_eq!(kernel.store().load_head(INDIVIDUAL).unwrap(), boot.head);
}

#[test]
fn a_proposal_citing_evidence_that_was_never_appended_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let never_appended = EvidenceId::from_u128(0xDEAD);
    let proposal = proposal_with(
        100,
        MutationDomain::Relationship,
        MutationOperation::Fact,
        Some("user-fixture"),
        vec![never_appended],
        boot.head.expected(),
        boot.writer,
    );
    let ActivationOutcome::Rejected(decision) = kernel.submit(&proposal).unwrap() else {
        panic!("expected rejection");
    };
    assert_eq!(decision.reason_code, ReasonCode::EvidenceNotFound);

    // Durable: the decision is retrievable and the head did not move.
    assert_eq!(
        kernel.store().find_decision(proposal.proposal_id).unwrap(),
        Some(decision)
    );
    assert_eq!(kernel.store().load_head(INDIVIDUAL).unwrap(), boot.head);
}
