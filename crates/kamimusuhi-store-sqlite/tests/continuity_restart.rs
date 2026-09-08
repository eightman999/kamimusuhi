//! Continuity integration tests (plan §15.2): migration, root creation,
//! accepted/rejected activation, idempotent retry, and restart recovery
//! across a fresh `SqliteContinuityStore` instance backed by the same
//! file (simulating a new process/connection).

mod support;

use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_core::mutation::ReasonCode;
use kamimusuhi_testkit::fixed_ids::SequentialIdGenerator;

#[test]
fn empty_db_migrates_and_root_individual_is_created_atomically() {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head.generation, 0);
    assert_eq!(head.commit_id, individual.root_commit_id);

    let root_commit = store.load_commit(individual.root_commit_id).unwrap();
    assert!(root_commit.is_some());
    let root_commit = root_commit.unwrap();
    assert_eq!(root_commit.generation, 0);
    assert!(root_commit.predecessor_commit_id.is_none());
    assert!(root_commit.proposal_id.is_none());
}

#[test]
fn foreign_keys_are_enforced() {
    let conn =
        kamimusuhi_store_sqlite::transaction::open_in_memory_with_required_pragmas().unwrap();
    kamimusuhi_store_sqlite::migrations::migrate(&conn).unwrap();

    // canonical_commits.individual_id references individuals(individual_id);
    // inserting a commit for a non-existent individual must fail closed
    // rather than silently succeeding.
    let result = conn.execute(
        "INSERT INTO canonical_commits (commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
         VALUES ('deadbeef', 'does-not-exist', 0, NULL, NULL, '2026-09-08T00:00:00Z')",
        [],
    );
    assert!(result.is_err(), "foreign key violation must be rejected");
}

#[test]
fn accepted_activation_increments_generation() {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(99);

    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-1",
    );

    let outcome = store.activate(proposal).unwrap();
    let receipt = match outcome {
        ActivationOutcome::Activated(r) => r,
        ActivationOutcome::Rejected { reason_code, .. } => {
            panic!("expected activation, got rejection: {reason_code:?}")
        }
    };
    assert_eq!(receipt.generation, 1);

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head.generation, 1);
    assert_eq!(head.commit_id, receipt.commit_id);
}

#[test]
fn rejected_proposal_does_not_move_head() {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(99);

    // Wrong expected generation -> STALE_PREDECESSOR, head must not move.
    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        7, // wrong
        "key-reject-1",
    );

    let outcome = store.activate(proposal).unwrap();
    match outcome {
        ActivationOutcome::Rejected { reason_code, .. } => {
            assert_eq!(reason_code, ReasonCode::StalePredecessor);
        }
        ActivationOutcome::Activated(_) => panic!("expected rejection"),
    }

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head.generation, 0);
    assert_eq!(head.commit_id, individual.root_commit_id);
}

#[test]
fn duplicate_proposal_retry_returns_same_receipt() {
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(99);

    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-retry-1",
    );

    let first = match store.activate(proposal.clone()).unwrap() {
        ActivationOutcome::Activated(r) => r,
        other => panic!("expected activation, got {other:?}"),
    };

    // Retry with a *new* proposal_id but the same idempotency key must
    // replay the same receipt, not create a second commit.
    let mut retry = proposal;
    retry.proposal_id = kamimusuhi_core::ids::ProposalId::new(&id_gen);
    let second = match store.activate(retry).unwrap() {
        ActivationOutcome::Activated(r) => r,
        other => panic!("expected activation, got {other:?}"),
    };

    assert_eq!(first.receipt_id, second.receipt_id);
    assert_eq!(first.commit_id, second.commit_id);

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(
        head.generation, 1,
        "retry must not advance generation twice"
    );
}

#[test]
fn stale_expected_head_after_generation_advance_is_rejected() {
    // T15: a proposal built against generation 0 must be rejected once
    // generation 1 has already been applied.
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(99);

    let first_proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-t15-first",
    );
    let first_receipt = match store.activate(first_proposal).unwrap() {
        ActivationOutcome::Activated(r) => r,
        other => panic!("expected activation, got {other:?}"),
    };
    assert_eq!(first_receipt.generation, 1);

    // Second proposal still expects generation 0 / the root commit.
    let stale_proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-t15-second",
    );
    let outcome = store.activate(stale_proposal).unwrap();
    match outcome {
        ActivationOutcome::Rejected { reason_code, .. } => {
            assert_eq!(reason_code, ReasonCode::StalePredecessor);
        }
        ActivationOutcome::Activated(_) => panic!("expected STALE_PREDECESSOR rejection"),
    }

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(
        head.generation, 1,
        "rejected stale proposal must not move head"
    );
}

#[test]
fn t02_two_proposals_from_same_generation_reversed_order_only_one_wins() {
    // T02: two proposals both built against generation 0; whichever one
    // activates first advances the head, the other must then observe a
    // stale predecessor and be rejected -- regardless of which one was
    // *constructed* first, only the one *applied* first can win.
    let store = support::open_memory_store();
    let individual = support::create_root(&store);
    let id_gen = SequentialIdGenerator::new(99);

    let proposal_a = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-t02-a",
    );
    let proposal_b = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "key-t02-b",
    );

    // Apply B first (reversed order relative to construction).
    let outcome_b = store.activate(proposal_b).unwrap();
    assert!(matches!(outcome_b, ActivationOutcome::Activated(_)));

    let outcome_a = store.activate(proposal_a).unwrap();
    match outcome_a {
        ActivationOutcome::Rejected { reason_code, .. } => {
            assert_eq!(reason_code, ReasonCode::StalePredecessor);
        }
        ActivationOutcome::Activated(_) => panic!("only one of the two proposals may win"),
    }

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head.generation, 1);
}

#[test]
fn restart_recovers_same_individual_and_head_from_a_fresh_store_instance() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("restart.sqlite");

    let (individual_id, root_commit_id, activated_commit_id) = {
        let store = support::open_store_at(&db_path);
        let individual = support::create_root(&store);
        let id_gen = SequentialIdGenerator::new(1000);
        let proposal = support::relationship_fact_proposal(
            &id_gen,
            individual.individual_id,
            individual.root_commit_id,
            0,
            "key-restart-1",
        );
        let receipt = match store.activate(proposal).unwrap() {
            ActivationOutcome::Activated(r) => r,
            other => panic!("expected activation, got {other:?}"),
        };
        (
            individual.individual_id,
            individual.root_commit_id,
            receipt.commit_id,
        )
    };
    // `store` is dropped here -- simulating full process termination for
    // this connection.

    // Fresh store instance over the same file: this is what a brand-new
    // process would do on restart.
    let reopened = support::open_store_at(&db_path);
    let head = reopened.load_head(individual_id).unwrap();
    assert_eq!(head.generation, 1);
    assert_eq!(head.commit_id, activated_commit_id);

    let individuals = reopened.list_individuals().unwrap();
    assert_eq!(individuals.len(), 1);
    assert_eq!(individuals[0].individual_id, individual_id);
    assert_eq!(individuals[0].root_commit_id, root_commit_id);

    let commit_count = reopened
        .count_commits_for_individual(individual_id)
        .unwrap();
    assert_eq!(commit_count, 2, "root commit + one activated commit");
}
