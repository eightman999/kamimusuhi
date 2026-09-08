//! Continuity domain types and the `ContinuityStore` contract.
//!
//! This is the most important boundary in the whole system (plan §7):
//! `activate()` is the only path by which canonical state may change, and
//! it must behave as an atomic, idempotent, expected-head-checked
//! transaction. Everything else in this module exists to describe its
//! inputs and outputs precisely.

use serde::{Deserialize, Serialize};

use crate::ids::{CommitId, IndividualId, ProposalId, ReceiptId};
use crate::mutation::{MutationProposal, ReasonCode};
use crate::time::UtcTimestamp;

/// One continuous Kamimusuhi individual. `root_commit_id` is the
/// generation-0 commit created at individual-creation time and never
/// changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Individual {
    pub individual_id: IndividualId,
    pub created_at: UtcTimestamp,
    pub root_commit_id: CommitId,
}

/// The current tip of an individual's canonical lineage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContinuityHead {
    pub individual_id: IndividualId,
    pub commit_id: CommitId,
    pub generation: u64,
    pub updated_at: UtcTimestamp,
}

/// One immutable canonical commit in an individual's lineage. The root
/// commit has `predecessor_commit_id = None` and `proposal_id = None`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalCommit {
    pub commit_id: CommitId,
    pub individual_id: IndividualId,
    pub generation: u64,
    pub predecessor_commit_id: Option<CommitId>,
    pub proposal_id: Option<ProposalId>,
    pub created_at: UtcTimestamp,
}

/// Receipt proving one proposal was activated into exactly one commit.
/// `proposal_id` and `commit_id` are both unique: retrying the same
/// proposal returns the same receipt rather than minting a new one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivationReceipt {
    pub receipt_id: ReceiptId,
    pub proposal_id: ProposalId,
    pub commit_id: CommitId,
    pub predecessor_commit_id: Option<CommitId>,
    pub generation: u64,
    pub created_at: UtcTimestamp,
}

/// Result of calling `activate()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ActivationOutcome {
    /// Proposal was accepted and committed (or a prior identical
    /// commit/receipt was found via idempotency replay).
    Activated(ActivationReceipt),
    /// Proposal was rejected by policy or by the expected-head check;
    /// canonical head did not move.
    Rejected {
        proposal_id: ProposalId,
        reason_code: ReasonCode,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ContinuityError {
    #[error("individual not found: {0}")]
    IndividualNotFound(IndividualId),

    #[error("storage backend error: {0}")]
    Storage(String),

    /// The underlying storage reported a transient lock/busy condition.
    /// Callers MUST NOT interpret this as an identity/CAS conflict (plan
    /// §7.2 invariant 7); it means "try again", not "rejected".
    #[error("storage busy/locked, retry: {0}")]
    Busy(String),

    #[error("invalid proposal: {0}")]
    InvalidProposal(String),
}

/// The continuity kernel's storage contract. Implemented against SQLite in
/// `kamimusuhi-store-sqlite`; `kamimusuhi-core` itself has no knowledge of
/// SQL.
pub trait ContinuityStore {
    fn load_head(&self, individual: IndividualId) -> Result<ContinuityHead, ContinuityError>;

    /// Apply one mutation proposal atomically. See plan §7.1/§7.2 for the
    /// required flow and invariants: no long-running call may happen
    /// while a canonical transaction is open, expected-head mismatches
    /// are never silently resolved, and retries of the same
    /// proposal/idempotency key never create a second commit.
    fn activate(&self, proposal: MutationProposal) -> Result<ActivationOutcome, ContinuityError>;
}
