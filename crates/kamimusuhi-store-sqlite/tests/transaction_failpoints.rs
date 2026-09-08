//! Activation failure windows are atomic and restart-safe.

mod common;

use std::path::Path;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::*;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityError, ContinuityStore, Generation,
};
use kamimusuhi_core::mutation::MutationDecision;
use kamimusuhi_store_sqlite::failpoints::{Failpoint, FailpointHook, FailpointTriggered};
use kamimusuhi_store_sqlite::{SqliteStore, StoreConfig};
use kamimusuhi_testkit::FixedIdGenerator;

#[derive(Debug)]
struct FailAt(Failpoint);

impl FailpointHook for FailAt {
    fn hit(&self, point: Failpoint) -> Result<(), FailpointTriggered> {
        if point == self.0 {
            Err(FailpointTriggered(point))
        } else {
            Ok(())
        }
    }
}

fn accept(proposal: &kamimusuhi_core::mutation::MutationProposal) -> MutationDecision {
    MutationDecision::accept(proposal, proposal.policy_version, proposal.created_at)
}

#[derive(Debug)]
struct AbortAt(Failpoint);

impl FailpointHook for AbortAt {
    fn hit(&self, point: Failpoint) -> Result<(), FailpointTriggered> {
        if point == self.0 {
            std::process::abort();
        }
        Ok(())
    }
}

fn reopen_after_child(path: &Path) -> SqliteStore {
    let config = StoreConfig {
        busy_timeout: Duration::from_millis(50),
    };
    let mut last_busy = None;
    for attempt in 1..=10 {
        match SqliteStore::open(path, &config, clock(), Arc::new(FixedIdGenerator::new(2))) {
            Ok(store) => return store,
            Err(ContinuityError::Contended { detail }) => {
                last_busy = Some(detail);
                if attempt < 10 {
                    thread::sleep(Duration::from_millis(50));
                }
            }
            Err(error) => panic!("reopen after child crash failed: {error}"),
        }
    }
    panic!(
        "SQLite remained busy after 10 retries following child abort: {}",
        last_busy.unwrap_or_else(|| "no busy detail returned".to_owned())
    );
}

#[test]
fn child_crash_worker() {
    let (Ok(path), Ok(point)) = (
        std::env::var("KAMIMUSUHI_FP_DB"),
        std::env::var("KAMIMUSUHI_FP_POINT"),
    ) else {
        return;
    };
    let point = Failpoint::from_str(&point).expect("KAMIMUSUHI_FP_POINT must name a failpoint");
    let store = open(Path::new(&path), 1).with_failpoint_hook(Arc::new(AbortAt(point)));
    let head = store.load_head(INDIVIDUAL).unwrap();
    let writer = kamimusuhi_core::continuity::WriterIdentity {
        node_id: NODE,
        boot_id: BOOT_A,
        writer_epoch: kamimusuhi_core::continuity::WriterEpoch::INITIAL,
    };
    let proposal = relationship_fact(100, head.expected(), writer, "ほうじ茶");
    let result = store.activate(&proposal, &accept(&proposal));
    panic!("child failpoint {point} returned instead of aborting: {result:?}");
}

#[test]
fn child_process_abort_preserves_the_failpoint_crash_contract() {
    for point in Failpoint::ALL {
        let db = TempDb::new();
        let store = open(&db.path, 1);
        let boot = bootstrap(&store, BOOT_A);
        let post_bootstrap = row_counts(&db.path);
        drop(store);

        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_crash_worker",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("KAMIMUSUHI_FP_DB", &db.path)
            .env("KAMIMUSUHI_FP_POINT", point.code())
            .status()
            .unwrap_or_else(|error| panic!("spawn child for {point}: {error}"));
        assert!(!status.success(), "{point} child did not abort");

        let restarted = reopen_after_child(&db.path);
        let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
        if point.is_after_commit() {
            let receipt = restarted
                .find_receipt(proposal.proposal_id)
                .unwrap()
                .unwrap_or_else(|| panic!("{point} committed but has no receipt"));
            assert_eq!(
                restarted.load_head(INDIVIDUAL).unwrap().generation,
                Generation(1)
            );
            assert_eq!(row_counts(&db.path).receipts, 1);
            assert_eq!(
                restarted.activate(&proposal, &accept(&proposal)).unwrap(),
                ActivationOutcome::AlreadyActivated(receipt),
                "{point} retry did not return the durable receipt"
            );
        } else {
            assert_eq!(
                row_counts(&db.path),
                post_bootstrap,
                "{point} left rows behind"
            );
            let head = restarted.load_head(INDIVIDUAL).unwrap();
            assert_eq!(head.generation, Generation::ROOT, "{point}");
            assert_eq!(head.commit_id, ROOT_COMMIT, "{point}");
            assert!(matches!(
                restarted.activate(&proposal, &accept(&proposal)).unwrap(),
                ActivationOutcome::Activated(_)
            ));
        }
        assert_eq!(row_counts(&db.path).commits, 2, "{point}");
        assert_eq!(
            restarted.load_head(INDIVIDUAL).unwrap().generation,
            Generation(1),
            "{point}"
        );
    }
}

#[test]
fn failures_before_commit_roll_back_every_activation_row_and_recover_on_retry() {
    for point in Failpoint::ALL
        .into_iter()
        .filter(|point| !point.is_after_commit())
    {
        let db = TempDb::new();
        let store = open(&db.path, 1);
        let boot = bootstrap(&store, BOOT_A);
        let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
        let store = store.with_failpoint_hook(Arc::new(FailAt(point)));

        let error = store.activate(&proposal, &accept(&proposal)).unwrap_err();
        assert!(
            matches!(error, ContinuityError::Backend { .. }),
            "{point} returned {error:?}"
        );
        drop(store);

        // Read through a new connection, as a restarted process would.
        let restarted = open(&db.path, 2);
        assert_eq!(
            restarted.load_head(INDIVIDUAL).unwrap(),
            boot.head,
            "{point}"
        );
        assert_eq!(
            restarted.find_receipt(proposal.proposal_id).unwrap(),
            None,
            "{point}"
        );
        assert_eq!(
            restarted.find_decision(proposal.proposal_id).unwrap(),
            None,
            "{point}"
        );
        assert_eq!(
            row_counts(&db.path),
            RowCounts {
                commits: 1,
                heads: 1,
                proposals: 0,
                decisions: 0,
                receipts: 0,
                evidence: 1,
                state_records: 0,
                audits: 2,
                head_generation: 0,
            },
            "{point} left a partial transaction"
        );

        let outcome = restarted.activate(&proposal, &accept(&proposal)).unwrap();
        assert!(
            matches!(outcome, ActivationOutcome::Activated(receipt) if receipt.generation == Generation(1)),
            "{point}: {outcome:?}"
        );
    }
}

#[test]
fn failure_after_commit_is_recovered_as_the_original_receipt() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let store = store.with_failpoint_hook(Arc::new(FailAt(Failpoint::AfterSqlCommitBeforeAck)));

    assert!(matches!(
        store.activate(&proposal, &accept(&proposal)),
        Err(ContinuityError::Backend { .. })
    ));
    drop(store);

    let restarted = open(&db.path, 2);
    let receipt = restarted
        .find_receipt(proposal.proposal_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        restarted.load_head(INDIVIDUAL).unwrap().commit_id,
        receipt.commit_id
    );
    assert_eq!(
        restarted.load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
    assert_eq!(
        restarted.activate(&proposal, &accept(&proposal)).unwrap(),
        ActivationOutcome::AlreadyActivated(receipt)
    );
    assert_eq!(row_counts(&db.path).commits, 2);
    assert_eq!(row_counts(&db.path).receipts, 1);
}
