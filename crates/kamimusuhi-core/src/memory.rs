//! Durable episodic/relationship state and the `MemoryRepository`
//! contract (plan §4, §6.3).
//!
//! A [`StateRecord`] is never written directly by this contract: it is
//! only ever produced as a side effect of an accepted
//! [`crate::mutation::MutationProposal`] inside
//! [`crate::continuity::ContinuityStore::activate`], in the same
//! transaction as the canonical commit/head/receipt (plan §7.2 invariant
//! 4). `MemoryRepository` here is read-only. Superseding a record never
//! deletes or rewrites the prior row (plan §7.2 invariant 4 / §8, "prior
//! history"): the prior row's `lifecycle_state` moves to `Superseded`
//! and a new row is inserted with `supersedes_state_record_id` pointing
//! back at it.
//!
//! `self` domain schema space is reserved (plan §6.3) but
//! `MutationPolicyV0` does not accept mutations into it yet.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::ids::{CommitId, EvidenceId, IndividualId, MemoryId};
use crate::time::UtcTimestamp;

/// Durable state domain. `self` is reserved schema space (plan §6.3) and
/// intentionally not exposed here yet: no code in this wave can
/// construct a `MemoryDomain::SelfDomain` state record.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDomain {
    Episodic,
    Relationship,
}

/// Whether a state record is the current truth for its subject/kind, or
/// has been superseded by a later correction. Superseded rows are never
/// deleted (plan §7.2 invariant 4).
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Superseded,
}

/// One durable episodic/relationship state row (plan §6.3
/// `state_records`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateRecord {
    pub state_record_id: MemoryId,
    pub individual_id: IndividualId,
    pub domain: MemoryDomain,
    /// Who/what this record is about. Required for `Relationship`,
    /// meaningless for most `Episodic` capture.
    pub subject_key: Option<String>,
    pub kind: String,
    pub payload: JsonValue,
    pub lifecycle_state: LifecycleState,
    pub created_commit_id: CommitId,
    pub supersedes_state_record_id: Option<MemoryId>,
    pub created_at: UtcTimestamp,
}

/// A state record together with the evidence it is grounded in. Every
/// durable memory has at least one evidence ref (plan §19 "every durable
/// memory has evidence ref"); `MemoryRepository` implementations must
/// never synthesize an `AttributedMemory` without them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttributedMemory {
    pub record: StateRecord,
    pub evidence_refs: Vec<EvidenceId>,
}

/// Deterministic retrieval filter (plan §11.1): domain + subject +
/// lifecycle + optional lexical containment, no embedding dependency.
#[derive(Clone, Debug)]
pub struct MemoryQuery {
    pub individual_id: IndividualId,
    pub domain: Option<MemoryDomain>,
    pub subject_key: Option<String>,
    /// When true, only `LifecycleState::Active` rows are returned.
    pub only_active: bool,
    /// Deterministic lexical containment check against the record's
    /// serialized payload, when present.
    pub text_contains: Option<String>,
}

impl MemoryQuery {
    pub fn for_individual(individual_id: IndividualId) -> Self {
        MemoryQuery {
            individual_id,
            domain: None,
            subject_key: None,
            only_active: true,
            text_contains: None,
        }
    }

    pub fn with_domain(mut self, domain: MemoryDomain) -> Self {
        self.domain = Some(domain);
        self
    }

    pub fn with_subject(mut self, subject_key: impl Into<String>) -> Self {
        self.subject_key = Some(subject_key.into());
        self
    }

    pub fn including_superseded(mut self) -> Self {
        self.only_active = false;
        self
    }

    pub fn with_text_contains(mut self, text: impl Into<String>) -> Self {
        self.text_contains = Some(text.into());
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("individual not found: {0}")]
    IndividualNotFound(IndividualId),

    #[error("storage backend error: {0}")]
    Storage(String),
}

/// Read-only durable-memory retrieval contract (plan §4, §11.1). State
/// records themselves are written only as a side effect of
/// `ContinuityStore::activate`, never through this trait.
pub trait MemoryRepository {
    fn retrieve(&self, query: MemoryQuery) -> Result<Vec<AttributedMemory>, MemoryError>;
}
