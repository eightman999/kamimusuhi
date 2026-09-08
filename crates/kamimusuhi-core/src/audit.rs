//! Durable audit events.
//!
//! Audit events live in the same correctness domain as the canonical
//! transaction: a significant transition and its audit row are committed
//! together or not at all. Operational JSONL tracing is a separate concern.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{AuditEventId, CommitId, IndividualId, ProposalId};
use crate::time::UtcTimestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    IndividualCreated,
    WriterEpochClaimed,
    MutationProposed,
    MutationRejected,
    ContinuityActivated,
    StateSuperseded,
}

impl AuditKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndividualCreated => "individual.created",
            Self::WriterEpochClaimed => "writer.epoch_claimed",
            Self::MutationProposed => "mutation.proposed",
            Self::MutationRejected => "mutation.rejected",
            Self::ContinuityActivated => "continuity.activated",
            Self::StateSuperseded => "state.superseded",
        }
    }
}

impl fmt::Display for AuditKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown audit kind {0:?}")]
pub struct UnknownAuditKind(pub String);

impl FromStr for AuditKind {
    type Err = UnknownAuditKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "individual.created" => Self::IndividualCreated,
            "writer.epoch_claimed" => Self::WriterEpochClaimed,
            "mutation.proposed" => Self::MutationProposed,
            "mutation.rejected" => Self::MutationRejected,
            "continuity.activated" => Self::ContinuityActivated,
            "state.superseded" => Self::StateSuperseded,
            other => return Err(UnknownAuditKind(other.to_owned())),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub audit_event_id: AuditEventId,
    pub individual_id: IndividualId,
    pub kind: AuditKind,
    pub commit_id: Option<CommitId>,
    pub proposal_id: Option<ProposalId>,
    /// Structured detail (IDs, reason codes, versions). Never raw private
    /// payloads or secrets.
    pub payload: serde_json::Value,
    pub created_at: UtcTimestamp,
}
