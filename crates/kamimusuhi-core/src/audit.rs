//! Durable audit events.
//!
//! Durable audit is in the same correctness domain as the canonical
//! transaction it describes: it is written inside the same `BEGIN
//! IMMEDIATE` as the state/commit/head/receipt rows it documents (plan
//! §12). This is distinct from the JSONL *operational* trace, which is
//! best-effort observability and lives outside `kamimusuhi-core`.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::ids::{CommitId, IndividualId, ProposalId};
use crate::time::UtcTimestamp;

/// Stable, append-only audit event kinds. New kinds may be added; existing
/// ones are never renamed once persisted rows exist.
#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventKind {
    IndividualCreated,
    MutationProposed,
    MutationRejected,
    ContinuityActivated,
    StateSuperseded,
}

/// One durable audit row. `individual_id` is `None` only for events that
/// precede individual creation (there are none in this wave, but the
/// field is kept optional rather than using a sentinel ID).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub individual_id: Option<IndividualId>,
    pub kind: AuditEventKind,
    pub proposal_id: Option<ProposalId>,
    pub commit_id: Option<CommitId>,
    pub detail: JsonValue,
    pub created_at: UtcTimestamp,
}
