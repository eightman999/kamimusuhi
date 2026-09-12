//! Canonical evidence domain types and the `EvidenceStore` contract
//! (plan §4, §6.2).
//!
//! Evidence is append-oriented: once recorded, a row is never updated or
//! deleted by this contract. `source_time` (when the underlying event
//! happened, if known) and `received_at` (when this runtime observed it)
//! are kept as separate fields, never collapsed into one "timestamp"
//! (plan §5.2). `content_digest` is an integrity/dedup aid only; it is
//! not a substitute for anonymization or deletion (plan §6.2).

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use crate::mutation::OriginClass;
use crate::time::UtcTimestamp;

/// What kind of thing an evidence record captures. This is distinct from
/// [`OriginClass`]: `kind` says what shape/role the content has, while
/// `origin_class` says where it ultimately came from.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A raw user utterance, observed directly.
    UserUtterance,
    /// Fake/real Persona Core's own generated natural-language output.
    /// Never grounding for a self/relationship mutation by itself (plan
    /// §8.2, §9.1: an assistant copy of a turn is never added as
    /// evidence for that same turn).
    PersonaNarration,
    /// An excerpt copied out of an imported Library artifact for
    /// provenance purposes. Not canonical-interaction grounding.
    LibraryExcerpt,
    /// Raw output of an external cognitive resource call.
    ExternalResourceOutput,
    /// Operator/administrative note.
    OperatorNote,
    /// Any other system-observed event not covered above.
    SystemEvent,
}

/// How long/carefully a piece of evidence should be retained. This wave
/// does not implement retention enforcement; the field exists so it is
/// never bolted on as an afterthought to an append-only table.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    Standard,
    Sensitive,
    Ephemeral,
}

/// Request to append one new evidence record. `content_digest` is
/// computed by the store, not supplied by the caller: it must reflect
/// what was actually persisted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewEvidence {
    pub individual_id: IndividualId,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub kind: EvidenceKind,
    pub origin_class: OriginClass,
    pub payload: JsonValue,
    pub source_id: Option<String>,
    pub source_sequence: Option<i64>,
    /// When the underlying event happened upstream, if known. Never the
    /// same field as `received_at` on the persisted record (plan §5.2).
    pub source_time: Option<UtcTimestamp>,
    pub retention_class: RetentionClass,
}

/// A durable, append-only evidence row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub evidence_id: EvidenceId,
    pub individual_id: IndividualId,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub kind: EvidenceKind,
    pub origin_class: OriginClass,
    pub payload: JsonValue,
    pub source_id: Option<String>,
    pub source_sequence: Option<i64>,
    pub source_time: Option<UtcTimestamp>,
    /// Integrity/dedup aid derived from `payload` (plan §6.2). Not a
    /// substitute for anonymization or deletion.
    pub content_digest: String,
    pub retention_class: RetentionClass,
    /// When this runtime observed/persisted the record. Distinct from
    /// `source_time` (plan §5.2).
    pub received_at: UtcTimestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("individual not found: {0}")]
    IndividualNotFound(IndividualId),

    #[error("storage backend error: {0}")]
    Storage(String),

    #[error("invalid evidence record: {0}")]
    Invalid(String),
}

/// Canonical evidence append/read contract (plan §4). Appending evidence
/// never advances the continuity head (plan §7.2 invariant 6); it is a
/// separate correctness domain from `ContinuityStore::activate`.
pub trait EvidenceStore {
    fn append(&self, record: NewEvidence) -> Result<EvidenceRecord, EvidenceError>;
    fn get(&self, id: EvidenceId) -> Result<Option<EvidenceRecord>, EvidenceError>;
}
