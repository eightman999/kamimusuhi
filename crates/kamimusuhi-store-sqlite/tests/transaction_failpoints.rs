//! Crash-window tests for `activate()` (plan §7.3).
//!
//! FP01-FP06 are injected as errors at each named point inside the
//! `BEGIN IMMEDIATE` transaction; each must leave the database exactly as
//! it was before the call (full rollback: no partial proposal, decision,
//! commit, head, or receipt row). FP07 fires *after* the SQL `COMMIT` has
//! already succeeded, simulating "the write landed but the caller's
//! acknowledgement was lost"; a retry with the same idempotency key must
//! then find the already-committed receipt rather than creating a second
//! commit.
//!
//! This is an in-process failpoint harness (`kamimusuhi_store_sqlite::failpoint`,
//! compiled under `debug_assertions`), not a literal child-process crash
//! test: it proves the transaction boundaries are correct without the
//! added complexity of an OS-level process kill. See the wave report for
//! this documented scope reduction.

mod support;

use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_store_sqlite::failpoint::Failpoint;
use kamimusuhi_testkit::fixed_ids::SequentialIdGenerator;

fn assert_no_partial_state(
    store: &kamimusuhi_store_sqlite::SqliteContinuityStore,
    individual_id: kamimusuhi_core::ids::IndividualId,
) {
    let head = store.load_head(individual_id).unwrap();
    assert_eq!(
        head.generation, 0,
        "failpoint rollback must leave head untouched"
    );
    assert_eq!(
        store.count_commits_for_individual(individual_id).unwrap(),
        1,
        "only the root commit may exist after a rolled-back activation"
    );
}

fn run_rollback_case(fp: Failpoint, idempotency_key: &str) {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(500);
    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        idempotency_key,
    );

    store.arm_failpoint(fp);
    let result = store.activate(proposal);
    assert!(
        result.is_err(),
        "{fp:?} must surface as an error to the caller"
    );
    assert_no_partial_state(&store, individual.individual_id);
}

#[test]
fn fp01_after_begin_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterBegin, "fp01");
}

#[test]
fn fp02_after_proposal_insert_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterProposalInsert, "fp02");
}

#[test]
fn fp03_after_state_insert_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterStateInsert, "fp03");
}

#[test]
fn fp04_after_commit_record_insert_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterCommitRecordInsert, "fp04");
}

#[test]
fn fp05_after_head_update_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterHeadUpdate, "fp05");
}

#[test]
fn fp06_after_receipt_insert_rolls_back_fully() {
    run_rollback_case(Failpoint::AfterReceiptInsert, "fp06");
}

#[test]
fn fp07_retry_after_commit_finds_existing_receipt_without_double_activation() {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(500);
    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "fp07",
    );

    store.arm_failpoint(Failpoint::AfterSqlCommitBeforeAck);
    let first_attempt = store.activate(proposal.clone());
    assert!(
        first_attempt.is_err(),
        "the caller's ack is lost, but the write already committed"
    );

    // The data is durable even though the caller saw an error: check it
    // directly before retrying.
    let head_after_lost_ack = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head_after_lost_ack.generation, 1);

    // Retry with the *same* idempotency key (a fresh proposal_id, as a
    // real retry after a lost ack would use) must not double-activate.
    let mut retry = proposal;
    retry.proposal_id = kamimusuhi_core::ids::ProposalId::new(&id_gen);
    let outcome = store.activate(retry).unwrap();
    let receipt = match outcome {
        ActivationOutcome::Activated(r) => r,
        other => panic!("expected activation replay, got {other:?}"),
    };
    assert_eq!(receipt.generation, 1);

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(
        head.generation, 1,
        "retry after lost ack must not advance generation again"
    );
    assert_eq!(
        store
            .count_commits_for_individual(individual.individual_id)
            .unwrap(),
        2,
        "root commit + exactly one activated commit, never two"
    );
}
