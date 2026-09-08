//! Failpoint injection for `activate()` crash-window tests (plan §7.3).
//!
//! Compiled only under `debug_assertions` (true for `dev`/`test`
//! profiles, false for `--release`), so this is never a production
//! semantic dependency (plan §20): a release build does not even contain
//! the arming API, and `hit()` is a constant `false` in that build. Under
//! `debug_assertions`, `hit()` is a no-op unless a test has explicitly
//! armed a failpoint on the current thread, so ordinary `cargo test`
//! behaviour for non-failpoint tests is unaffected.
//!
//! Each failpoint corresponds to one crash window inside `activate()`.
//! Arming one and observing the transaction roll back (FP01-FP06) or
//! observing that a retry after "commit succeeded, ack lost" replays the
//! same receipt (FP07) is how `tests/transaction_failpoints.rs` exercises
//! plan §7.3.

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Failpoint {
    AfterBegin,
    AfterProposalInsert,
    AfterStateInsert,
    AfterCommitRecordInsert,
    AfterHeadUpdate,
    AfterReceiptInsert,
    AfterSqlCommitBeforeAck,
}

impl Failpoint {
    pub fn code(self) -> &'static str {
        match self {
            Failpoint::AfterBegin => "FP01_after_begin",
            Failpoint::AfterProposalInsert => "FP02_after_proposal_insert",
            Failpoint::AfterStateInsert => "FP03_after_state_insert",
            Failpoint::AfterCommitRecordInsert => "FP04_after_commit_record_insert",
            Failpoint::AfterHeadUpdate => "FP05_after_head_update",
            Failpoint::AfterReceiptInsert => "FP06_after_receipt_insert",
            Failpoint::AfterSqlCommitBeforeAck => {
                "FP07_immediately_after_sql_commit_before_runtime_ack"
            }
        }
    }
}

#[cfg(debug_assertions)]
mod imp {
    use super::Failpoint;
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<Option<Failpoint>> = const { Cell::new(None) };
    }

    /// Test-only: arm a failpoint on the current thread. The store must
    /// be driven from the same thread in the test for this to take
    /// effect (our `ContinuityStore` implementation is single-writer and
    /// synchronous, so this holds for the test harness).
    pub fn arm(fp: Failpoint) {
        ARMED.with(|c| c.set(Some(fp)));
    }

    pub fn disarm() {
        ARMED.with(|c| c.set(None));
    }

    /// Returns true (and disarms) exactly once if `fp` is the currently
    /// armed failpoint.
    pub fn hit(fp: Failpoint) -> bool {
        ARMED.with(|c| {
            if c.get() == Some(fp) {
                c.set(None);
                true
            } else {
                false
            }
        })
    }
}

#[cfg(not(debug_assertions))]
mod imp {
    use super::Failpoint;

    #[inline(always)]
    pub fn arm(_fp: Failpoint) {}
    #[inline(always)]
    pub fn disarm() {}
    #[inline(always)]
    pub fn hit(_fp: Failpoint) -> bool {
        false
    }
}

pub use imp::{arm, disarm, hit};
