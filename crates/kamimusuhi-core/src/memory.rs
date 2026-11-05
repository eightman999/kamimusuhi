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

/// A link declared inside a record payload: either to supporting evidence or
/// to another state record.
///
/// Links are candidate data emitted by producers (reflectors, tools) inside
/// `payload["links"]`. They are parsed lazily at recall time and confer no
/// authority — a malformed entry is skipped, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryLink {
    Evidence(EvidenceId),
    State(MemoryId),
}

impl MemoryLink {
    /// Parse exactly `evidence:<32-hex>` or `state:<32-hex>`. Anything else
    /// is `None` — declared links fail soft.
    pub fn parse(text: &str) -> Option<Self> {
        let (scheme, hex) = text.split_once(':')?;
        match scheme {
            "evidence" => hex.parse().ok().map(Self::Evidence),
            "state" => hex.parse().ok().map(Self::State),
            _ => None,
        }
    }

    /// Canonical text form; `MemoryLink::parse(&link.as_str())` is the
    /// identity on every `MemoryLink`.
    pub fn as_str(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MemoryLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(id) => write!(f, "evidence:{id}"),
            Self::State(id) => write!(f, "state:{id}"),
        }
    }
}

impl Serialize for MemoryLink {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MemoryLink {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format_args!(
                "invalid memory link {text:?}: expected \"evidence:<32-hex>\" or \"state:<32-hex>\""
            ))
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

impl StateRecord {
    /// Tags the producer declared in `payload["tags"]`.
    ///
    /// Declared metadata is candidate data, not canonical content: a missing
    /// or malformed field yields an empty list rather than an error, because
    /// the producer is a model and recall must fail soft.
    pub fn tags(&self) -> Vec<String> {
        self.payload
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Links the producer declared in `payload["links"]`. Entries that
    /// [`MemoryLink::parse`] rejects are skipped silently — the same
    /// fail-soft rule as [`tags`](Self::tags).
    pub fn links(&self) -> Vec<MemoryLink> {
        self.payload
            .get("links")
            .and_then(serde_json::Value::as_array)
            .map(|links| {
                links
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(MemoryLink::parse)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Start of the declared validity window (`payload["valid_from"]`), or
    /// `None` when the payload does not declare a usable one.
    pub fn valid_from(&self) -> Option<UtcTimestamp> {
        self.declared_time("valid_from")
    }

    /// End of the declared validity window (`payload["valid_to"]`),
    /// exclusive.
    pub fn valid_to(&self) -> Option<UtcTimestamp> {
        self.declared_time("valid_to")
    }

    /// Whether the declared validity window covers `now`. An unset bound is
    /// open: no `valid_to` means the record never expires, no `valid_from`
    /// means it has always been valid.
    pub fn is_valid_at(&self, now: UtcTimestamp) -> bool {
        self.valid_from().is_none_or(|from| from <= now)
            && self.valid_to().is_none_or(|to| to > now)
    }

    /// Declared times arrive as unix seconds (integer) or RFC 3339 text;
    /// both are producer-supplied candidates, so anything else is `None`.
    fn declared_time(&self, key: &str) -> Option<UtcTimestamp> {
        match self.payload.get(key) {
            Some(serde_json::Value::Number(seconds)) => seconds
                .as_i64()
                .map(|s| UtcTimestamp::from_unix_millis(s.saturating_mul(1_000))),
            Some(serde_json::Value::String(text)) => UtcTimestamp::from_rfc3339(text),
            _ => None,
        }
    }
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

    /// Batch fetch by id. The default loops [`get_state_record`]; stores
    /// that can answer with one `IN (...)` query should override it.
    /// Absent ids are skipped.
    fn get_state_records(&self, ids: &[MemoryId]) -> Vec<StateRecord> {
        ids.iter()
            .filter_map(|id| self.get_state_record(*id).ok().flatten())
            .collect()
    }
}

/// Mutable bookkeeping about how often a record has been served.
///
/// This is usage metadata, not canonical record content, so it never lives
/// on the `state_records` row itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordUsage {
    pub last_used_at: UtcTimestamp,
    pub use_count: u32,
}

/// Write side of usage bookkeeping — the deliberate companion to the
/// read-only [`MemoryRepository`].
///
/// `last_used_at`/`use_count` are mutable bookkeeping, not canonical record
/// content: they live outside `state_records`, and updating them never
/// rewrites what a record asserts. Canonical record writes still go through
/// the Continuity Kernel alone; this trait exists so recall can record that
/// a record was served without routing through a mutation proposal.
pub trait MemoryUsage: Send + Sync {
    /// Upsert one usage mark per id: `use_count + 1`, `last_used_at = now`.
    /// An empty slice is a no-op.
    ///
    /// `&self`, not `&mut`: callers reach the store through a shared
    /// reference, and implementations already serialize access internally.
    fn mark_used(&self, ids: &[MemoryId], now: UtcTimestamp);

    /// Usage for one record, or `None` if it was never marked.
    fn usage(&self, id: &MemoryId) -> Option<RecordUsage>;

    /// Usage for a batch of records; ids never marked are absent.
    fn usages(&self, ids: &[MemoryId]) -> Vec<(MemoryId, RecordUsage)>;
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

    fn record_with(payload: serde_json::Value) -> StateRecord {
        StateRecord {
            state_record_id: MemoryId::from_u128(1),
            individual_id: IndividualId::from_u128(2),
            domain: MutationDomain::Episodic,
            subject_key: None,
            kind: "episode".to_owned(),
            payload,
            lifecycle_state: LifecycleState::Active,
            created_commit_id: CommitId::from_u128(3),
            supersedes_state_record_id: None,
            superseded_by_state_record_id: None,
            evidence_refs: vec![],
            created_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    #[test]
    fn memory_link_round_trips_through_text_and_json() {
        let links = [
            MemoryLink::Evidence(EvidenceId::from_u128(0xE1)),
            MemoryLink::State(MemoryId::from_u128(0x51)),
        ];
        for link in links {
            let text = link.as_str();
            assert_eq!(MemoryLink::parse(&text), Some(link));
            assert_eq!(link.to_string(), text);
            let json = serde_json::to_string(&link).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            let back: MemoryLink = serde_json::from_str(&json).unwrap();
            assert_eq!(back, link);
        }
        assert_eq!(
            MemoryLink::Evidence(EvidenceId::from_u128(0xE1)).as_str(),
            "evidence:000000000000000000000000000000e1"
        );
        assert_eq!(
            MemoryLink::State(MemoryId::from_u128(0x51)).as_str(),
            "state:00000000000000000000000000000051"
        );
    }

    #[test]
    fn memory_link_rejects_anything_but_the_two_schemes() {
        for bad in [
            "",
            "evidence:",
            "evidence:not-hex",
            "evidence:000000000000000000000000000000E1",
            "state:00000000000000000000000000000051:extra",
            "commit:00000000000000000000000000000051",
            "state:51",
        ] {
            assert_eq!(MemoryLink::parse(bad), None, "{bad:?}");
            assert!(serde_json::from_str::<MemoryLink>(&format!("\"{bad}\"")).is_err());
        }
    }

    #[test]
    fn declared_metadata_parses_and_fails_soft() {
        let record = record_with(serde_json::json!({
            "text": "お茶の好みについて話した",
            "tags": ["roadmap", "hai", 7, null, {"nested": true}],
            "links": [
                "evidence:000000000000000000000000000000e1",
                "state:00000000000000000000000000000051",
                "evidence:not-hex",
                "commit:00000000000000000000000000000051",
                42
            ],
            "valid_from": 1_730_000_000,
            "valid_to": 1_732_000_000
        }));
        assert_eq!(record.tags(), vec!["roadmap", "hai"]);
        assert_eq!(
            record.links(),
            vec![
                MemoryLink::Evidence(EvidenceId::from_u128(0xE1)),
                MemoryLink::State(MemoryId::from_u128(0x51)),
            ]
        );
        assert_eq!(
            record.valid_from(),
            Some(UtcTimestamp::from_unix_millis(1_730_000_000_000))
        );
        assert_eq!(
            record.valid_to(),
            Some(UtcTimestamp::from_unix_millis(1_732_000_000_000))
        );

        // RFC 3339 strings are accepted for robustness.
        let rfc3339 = record_with(serde_json::json!({
            "valid_from": "2024-10-27T03:33:20Z"
        }));
        assert_eq!(
            rfc3339.valid_from(),
            Some(UtcTimestamp::from_unix_millis(1_730_000_000_000))
        );

        // Missing or malformed metadata is an empty answer, never an error.
        let bare = record_with(serde_json::json!({ "text": "…" }));
        assert!(bare.tags().is_empty());
        assert!(bare.links().is_empty());
        assert_eq!(bare.valid_from(), None);
        assert_eq!(bare.valid_to(), None);

        let malformed = record_with(serde_json::json!({
            "tags": "nope",
            "links": { "a": 1 },
            "valid_from": [],
            "valid_to": "not a time"
        }));
        assert!(malformed.tags().is_empty());
        assert!(malformed.links().is_empty());
        assert_eq!(malformed.valid_from(), None);
        assert_eq!(malformed.valid_to(), None);
    }

    #[test]
    fn validity_window_is_open_when_unset_and_exclusive_at_valid_to() {
        let windowed = record_with(serde_json::json!({
            "valid_from": 1_000,
            "valid_to": 2_000
        }));
        assert!(!windowed.is_valid_at(UtcTimestamp::from_unix_millis(999_999)));
        assert!(windowed.is_valid_at(UtcTimestamp::from_unix_millis(1_000_000)));
        assert!(windowed.is_valid_at(UtcTimestamp::from_unix_millis(1_999_999)));
        assert!(!windowed.is_valid_at(UtcTimestamp::from_unix_millis(2_000_000)));

        let open = record_with(serde_json::json!({ "text": "…" }));
        assert!(open.is_valid_at(UtcTimestamp::from_unix_millis(0)));
        assert!(open.is_valid_at(UtcTimestamp::from_unix_millis(i64::MAX / 2)));

        let expired = record_with(serde_json::json!({ "valid_to": 1_000 }));
        assert!(!expired.is_valid_at(UtcTimestamp::from_unix_millis(1_000_000)));
        assert!(expired.is_valid_at(UtcTimestamp::from_unix_millis(999_999)));

        let future = record_with(serde_json::json!({ "valid_from": 9_000 }));
        assert!(!future.is_valid_at(UtcTimestamp::from_unix_millis(8_999_999)));

        // A malformed bound reads as unset — fail soft, not fail closed.
        let bad = record_with(serde_json::json!({ "valid_to": "soon" }));
        assert!(bad.is_valid_at(UtcTimestamp::from_unix_millis(i64::MAX / 2)));
    }
}
