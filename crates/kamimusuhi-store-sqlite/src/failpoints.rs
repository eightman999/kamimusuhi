//! Crash windows of the activation transaction (phase-1 plan §7.3).
//!
//! A [`FailpointHook`] is a test-harness seam: it may return an error (the
//! transaction rolls back) or abort the process outright (crash test). No
//! production semantics depend on failpoints; a store without a hook never
//! consults them.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Failpoint {
    /// FP01: transaction opened, nothing written.
    AfterBegin,
    /// FP02: proposal row inserted.
    AfterProposalInsert,
    /// FP03: derived state row inserted (Wave 2 writes state here).
    AfterStateInsert,
    /// FP04: canonical commit row inserted.
    AfterCommitRecordInsert,
    /// FP05: continuity head moved.
    AfterHeadUpdate,
    /// FP06: activation receipt inserted.
    AfterReceiptInsert,
    /// FP07: SQL COMMIT succeeded but the caller has not been acknowledged.
    AfterSqlCommitBeforeAck,
}

impl Failpoint {
    pub const ALL: [Self; 7] = [
        Self::AfterBegin,
        Self::AfterProposalInsert,
        Self::AfterStateInsert,
        Self::AfterCommitRecordInsert,
        Self::AfterHeadUpdate,
        Self::AfterReceiptInsert,
        Self::AfterSqlCommitBeforeAck,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::AfterBegin => "FP01",
            Self::AfterProposalInsert => "FP02",
            Self::AfterStateInsert => "FP03",
            Self::AfterCommitRecordInsert => "FP04",
            Self::AfterHeadUpdate => "FP05",
            Self::AfterReceiptInsert => "FP06",
            Self::AfterSqlCommitBeforeAck => "FP07",
        }
    }

    /// Whether the transaction has already been committed when this point is hit.
    pub const fn is_after_commit(self) -> bool {
        matches!(self, Self::AfterSqlCommitBeforeAck)
    }
}

impl fmt::Display for Failpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Failpoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|fp| fp.code() == s)
            .ok_or_else(|| format!("unknown failpoint {s:?}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailpointTriggered(pub Failpoint);

impl fmt::Display for FailpointTriggered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failpoint {} triggered", self.0)
    }
}

impl std::error::Error for FailpointTriggered {}

pub trait FailpointHook: Send + Sync {
    /// Return `Err` to inject a failure. Implementations may also abort the
    /// process to simulate a crash.
    fn hit(&self, point: Failpoint) -> Result<(), FailpointTriggered>;
}
