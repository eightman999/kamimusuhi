//! Root creation and the atomic activation transaction.

mod common;

use common::*;
use kamimusuhi_core::audit::AuditKind;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityError, ContinuityStore, Generation, NewIndividual, WriterEpoch,
};
use kamimusuhi_core::ids::{CommitId, IndividualId, ProposalId};
use kamimusuhi_core::mutation::{Disposition, MutationDecision, ReasonCode};

#[test]
fn root_individual_is_created_with_generation_zero_head_and_audit() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);

    assert_eq!(boot.individual.individual_id, INDIVIDUAL);
    assert_eq!(boot.individual.root_commit_id, ROOT_COMMIT);
    assert_eq!(boot.head.generation, Generation::ROOT);
    assert_eq!(boot.head.commit_id, ROOT_COMMIT);
    assert_eq!(boot.head.writer_epoch, WriterEpoch::INITIAL);
    assert_eq!(boot.writer.boot_id, BOOT_A);

    assert_eq!(store.load_head(INDIVIDUAL).unwrap(), boot.head);

    let commits = store.commits(INDIVIDUAL).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].commit_id, ROOT_COMMIT);
    assert_eq!(commits[0].predecessor_commit_id, None);
    assert_eq!(commits[0].proposal_id, None);

    let kinds: Vec<AuditKind> = store
        .audit_events(INDIVIDUAL)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![AuditKind::IndividualCreated, AuditKind::WriterEpochClaimed]
    );
}

#[test]
fn creating_the_same_individual_twice_is_refused_and_leaves_no_partial_rows() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    let before = row_counts(&db.path);

    let err = store
        .create_individual(NewIndividual {
            individual_id: INDIVIDUAL,
            root_commit_id: CommitId::from_u128(0xC9),
            node_id: NODE,
            boot_id: BOOT_B,
        })
        .unwrap_err();
    assert_eq!(err, ContinuityError::IndividualAlreadyExists(INDIVIDUAL));
    assert_eq!(row_counts(&db.path), before);
}

#[test]
fn unknown_individual_is_not_found_and_not_created() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let missing = IndividualId::from_u128(0xFF);
    assert_eq!(
        store.load_head(missing),
        Err(ContinuityError::IndividualNotFound(missing))
    );
    assert_eq!(row_counts(&db.path).heads, 0);
}

#[test]
fn accepted_activation_increments_generation_and_records_receipt_and_audit() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let outcome = kernel.submit(&proposal).unwrap();
    let ActivationOutcome::Activated(receipt) = outcome else {
        panic!("expected activation, got {outcome:?}");
    };
    assert_eq!(receipt.generation, Generation(1));
    assert_eq!(receipt.predecessor_commit_id, ROOT_COMMIT);
    assert_eq!(receipt.proposal_id, proposal.proposal_id);

    let head = kernel.store().load_head(INDIVIDUAL).unwrap();
    assert_eq!(head.generation, Generation(1));
    assert_eq!(head.commit_id, receipt.commit_id);
    assert_eq!(head.writer_epoch, WriterEpoch::INITIAL);

    let commits = kernel.store().commits(INDIVIDUAL).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1].predecessor_commit_id, Some(ROOT_COMMIT));
    assert_eq!(commits[1].proposal_id, Some(proposal.proposal_id));

    let decision = kernel
        .store()
        .find_decision(proposal.proposal_id)
        .unwrap()
        .unwrap();
    assert_eq!(decision.disposition, Disposition::Accept);
    assert_eq!(decision.reason_code, ReasonCode::Accepted);

    let events = kernel.store().audit_events(INDIVIDUAL).unwrap();
    let kinds: Vec<AuditKind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            AuditKind::IndividualCreated,
            AuditKind::WriterEpochClaimed,
            AuditKind::MutationProposed,
            AuditKind::ContinuityActivated,
        ]
    );
    let activated = events.last().unwrap();
    assert_eq!(activated.commit_id, Some(receipt.commit_id));
    assert_eq!(activated.proposal_id, Some(proposal.proposal_id));
    // Audit carries IDs and reason codes, never the candidate content.
    assert!(!activated.payload.to_string().contains("ほうじ茶"));

    let counts = row_counts(&db.path);
    assert_eq!(
        counts,
        RowCounts {
            commits: 2,
            heads: 1,
            proposals: 1,
            decisions: 1,
            receipts: 1,
            audits: 4,
            head_generation: 1,
        }
    );
}

#[test]
fn retrying_the_same_proposal_returns_the_same_receipt_without_a_second_commit() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let first = kernel.submit(&proposal).unwrap();
    let receipt = *first.receipt().unwrap();
    let after_first = row_counts(&db.path);

    // Kernel-level retry (finds the receipt before touching the policy).
    assert_eq!(
        kernel.submit(&proposal).unwrap(),
        ActivationOutcome::AlreadyActivated(receipt)
    );
    // Store-level retry (a caller that skipped the kernel still cannot fork).
    let accept = MutationDecision::accept(&proposal, proposal.policy_version, receipt.created_at);
    assert_eq!(
        kernel.store().activate(&proposal, &accept).unwrap(),
        ActivationOutcome::AlreadyActivated(receipt)
    );
    assert_eq!(row_counts(&db.path), after_first);
}

#[test]
fn same_idempotency_key_with_same_content_under_new_id_is_deduplicated() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let receipt = *kernel.submit(&proposal).unwrap().receipt().unwrap();

    // The runtime lost the ack and re-drafted with a fresh proposal ID but the
    // same idempotency key. The head has moved, yet this is the same mutation.
    let mut redraft = proposal.clone();
    redraft.proposal_id = ProposalId::from_u128(101);
    let accept = MutationDecision::accept(&redraft, redraft.policy_version, receipt.created_at);
    assert_eq!(
        kernel.store().activate(&redraft, &accept).unwrap(),
        ActivationOutcome::AlreadyActivated(receipt)
    );
    assert_eq!(row_counts(&db.path).commits, 2);
}

#[test]
fn same_idempotency_key_with_different_content_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    kernel.submit(&proposal).unwrap();

    let mut conflicting = relationship_fact(101, boot.head.expected(), boot.writer, "緑茶");
    conflicting.idempotency_key = proposal.idempotency_key.clone();
    let accept = MutationDecision::accept(
        &conflicting,
        conflicting.policy_version,
        proposal.created_at,
    );
    let outcome = kernel.store().activate(&conflicting, &accept).unwrap();
    let ActivationOutcome::Rejected(decision) = outcome else {
        panic!("expected rejection, got {outcome:?}");
    };
    assert_eq!(
        decision.reason_code,
        ReasonCode::DuplicateProposalPayloadMismatch
    );
    assert_eq!(
        kernel.store().load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
    // The rejection is durable.
    assert_eq!(
        kernel
            .store()
            .find_decision(conflicting.proposal_id)
            .unwrap(),
        Some(decision)
    );
}

#[test]
fn rejected_proposal_is_durable_and_does_not_move_head() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let mut proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    proposal.evidence_refs.clear();
    let outcome = kernel.submit(&proposal).unwrap();
    let ActivationOutcome::Rejected(decision) = &outcome else {
        panic!("expected rejection, got {outcome:?}");
    };
    assert_eq!(decision.reason_code, ReasonCode::MissingEvidence);

    let head = kernel.store().load_head(INDIVIDUAL).unwrap();
    assert_eq!(head.generation, Generation::ROOT);
    assert_eq!(head.commit_id, ROOT_COMMIT);

    let events = kernel.store().audit_events(INDIVIDUAL).unwrap();
    assert_eq!(events[events.len() - 2].kind, AuditKind::MutationProposed);
    assert_eq!(events[events.len() - 1].kind, AuditKind::MutationRejected);
    assert_eq!(
        row_counts(&db.path),
        RowCounts {
            commits: 1,
            heads: 1,
            proposals: 1,
            decisions: 1,
            receipts: 0,
            audits: 4,
            head_generation: 0,
        }
    );

    // Retrying a rejected proposal returns the recorded decision.
    assert_eq!(kernel.submit(&proposal).unwrap(), outcome);
    assert_eq!(row_counts(&db.path).decisions, 1);
}

#[test]
fn activate_refuses_non_accepting_decision() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let reject = MutationDecision::reject(
        &proposal,
        ReasonCode::MissingEvidence,
        "test",
        proposal.policy_version,
        proposal.created_at,
    );
    assert_eq!(
        store.activate(&proposal, &reject),
        Err(ContinuityError::DecisionNotAccepting(Disposition::Reject))
    );
    assert_eq!(row_counts(&db.path).proposals, 0);
}

#[test]
fn sequential_activations_form_a_linear_lineage() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let mut head = boot.head;
    for (i, pref) in ["ほうじ茶", "玄米茶", "麦茶"].iter().enumerate() {
        let proposal = relationship_fact(100 + i as u128, head.expected(), boot.writer, pref);
        let receipt = *kernel.submit(&proposal).unwrap().receipt().unwrap();
        assert_eq!(receipt.predecessor_commit_id, head.commit_id);
        assert_eq!(receipt.generation, head.generation.next());
        head = kernel.store().load_head(INDIVIDUAL).unwrap();
        assert_eq!(head.commit_id, receipt.commit_id);
    }
    let commits = kernel.store().commits(INDIVIDUAL).unwrap();
    assert_eq!(commits.len(), 4);
    for pair in commits.windows(2) {
        assert_eq!(pair[1].predecessor_commit_id, Some(pair[0].commit_id));
        assert_eq!(pair[1].generation, pair[0].generation.next());
    }
}
