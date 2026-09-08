//! Durable derived state: what Kamimusuhi holds, as distinct from what it saw.
//!
//! Every [`StateRecord`] is produced by an activated mutation, so it always
//! names the commit that created it and the evidence it rests on. Records are
//! never rewritten: a correction inserts a new record and moves the old one to
//! a non-current [`LifecycleState`], which keeps history inspectable while
//! keeping the non-current version out of ordinary retrieval.
//!
//! Retrieval here is a plain indexed lookup. It performs no model call, so a
//! restarted runtime can answer from durable state alone.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{CommitId, EvidenceId, IndividualId, MemoryId};
use crate::mutation::{MutationDomain, UnknownVocabulary};
use crate::time::UtcTimestamp;

/// Whether a record is the current view, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Current. Returned by ordinary retrieval.
    Active,
    /// Explicitly replaced by a later record via a correction.
    Superseded,
    /// Supporting evidence was corrected, so this record is no longer treated
    /// as current. It is not asserted to be false — it needs re-evaluation
    /// (audit A03). Kept for inspection.
    Invalidated,
}

impl LifecycleState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Invalidated => "invalidated",
        }
    }

    /// Only active records are part of the current view.
    pub const fn is_current(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LifecycleState {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "active" => Self::Active,
            "superseded" => Self::Superseded,
            "invalidated" => Self::Invalidated,
            other => return Err(UnknownVocabulary::new("lifecycle_state", other)),
        })
    }
}

/// One durable derived record in one domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRecord {
    pub state_record_id: MemoryId,
    pub individual_id: IndividualId,
    pub domain: MutationDomain,
    /// Who or what the record is about. Required for relationship records so
    /// a fact about the user can never be read as a fact about the self.
    pub subject_key: Option<String>,
    /// Minimal W2 vocabulary: `episode` or `fact`.
    pub kind: String,
    pub payload: serde_json::Value,
    pub lifecycle_state: LifecycleState,
    /// The canonical commit that activated this record.
    pub created_commit_id: CommitId,
    pub supersedes_state_record_id: Option<MemoryId>,
    pub superseded_by_state_record_id: Option<MemoryId>,
    /// Evidence cited by the proposal that created this record.
    pub evidence_refs: Vec<EvidenceId>,
    pub created_at: UtcTimestamp,
}

/// Everything the mutation policy is allowed to know about a supersession
/// target. Payload-free for the same reason as [`crate::evidence::EvidenceFacts`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRecordFacts {
    pub state_record_id: MemoryId,
    pub individual_id: IndividualId,
    pub domain: MutationDomain,
    pub subject_key: Option<String>,
    pub lifecycle_state: LifecycleState,
}

/// A durable record together with its provenance.
///
/// Retrieved content is data. Holding an [`AttributedMemory`] confers no
/// authority on its payload: a proposal built after reading one still goes
/// through the ordinary policy and continuity checks (test T06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributedMemory {
    pub record: StateRecord,
    /// Distinct root evidence behind this record. Repeated summaries of one
    /// conversation do not raise this number (test T07).
    pub independent_evidence_count: usize,
    pub root_evidence: Vec<EvidenceId>,
}

impl AttributedMemory {
    /// Retrieved memory never authorises anything. Present as a method so the
    /// intent is visible at the call site rather than only in prose.
    pub const fn confers_authority(&self) -> bool {
        false
    }

    pub const fn is_current(&self) -> bool {
        self.record.lifecycle_state.is_current()
    }
}

/// Indexed lookup over durable state. No embeddings, no model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuery {
    pub individual_id: IndividualId,
    pub domain: Option<MutationDomain>,
    pub subject_key: Option<String>,
    /// Include superseded and invalidated records. Off by default so the
    /// current view never silently contains stale interpretations.
    pub include_non_current: bool,
    pub limit: Option<usize>,
}

impl MemoryQuery {
    pub fn current(individual_id: IndividualId) -> Self {
        Self {
            individual_id,
            domain: None,
            subject_key: None,
            include_non_current: false,
            limit: None,
        }
    }

    pub fn in_domain(mut self, domain: MutationDomain) -> Self {
        self.domain = Some(domain);
        self
    }

    pub fn about(mut self, subject_key: impl Into<String>) -> Self {
        self.subject_key = Some(subject_key.into());
        self
    }

    /// Include non-current history. Used for inspection, not for the current
    /// view.
    pub fn including_history(mut self) -> Self {
        self.include_non_current = true;
        self
    }

    pub fn limited(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    #[error("memory query is invalid: {reason}")]
    Invalid { reason: String },
    #[error("individual {0} is not present in this store")]
    IndividualNotFound(IndividualId),
    #[error("durable memory is inconsistent: {detail}")]
    Corrupt { detail: String },
    #[error("memory store is busy: {detail}")]
    Contended { detail: String },
    #[error("memory store backend error: {message}")]
    Backend { message: String },
}

/// Payload-free lookup of a supersession target, the only memory capability
/// the Continuity Kernel needs.
pub trait StateLookup: Send + Sync {
    fn state_facts(
        &self,
        state_record_id: MemoryId,
    ) -> Result<Option<StateRecordFacts>, MemoryError>;
}

/// Read side of durable memory. Writes only happen through the Continuity
/// Kernel's activation path; there is deliberately no `insert` here.
pub trait MemoryRepository: StateLookup {
    fn retrieve(&self, query: &MemoryQuery) -> Result<Vec<AttributedMemory>, MemoryError>;

    fn get_state_record(
        &self,
        state_record_id: MemoryId,
    ) -> Result<Option<StateRecord>, MemoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_records_are_current() {
        assert!(LifecycleState::Active.is_current());
        assert!(!LifecycleState::Superseded.is_current());
        assert!(!LifecycleState::Invalidated.is_current());
    }

    #[test]
    fn lifecycle_vocabulary_round_trips() {
        for state in [
            LifecycleState::Active,
            LifecycleState::Superseded,
            LifecycleState::Invalidated,
        ] {
            assert_eq!(state.as_str().parse::<LifecycleState>().unwrap(), state);
        }
        assert!("deleted".parse::<LifecycleState>().is_err());
    }

    #[test]
    fn query_defaults_to_the_current_view_only() {
        let query = MemoryQuery::current(IndividualId::from_u128(1))
            .in_domain(MutationDomain::Relationship)
            .about("user-fixture");
        assert!(!query.include_non_current);
        assert_eq!(query.subject_key.as_deref(), Some("user-fixture"));
        assert!(query.including_history().include_non_current);
    }
}
