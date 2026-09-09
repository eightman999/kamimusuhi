//! Canonical interaction evidence.
//!
//! Evidence is what actually reached the runtime: a user utterance, an agent
//! utterance, a system event, the result of a cognitive resource call, an
//! excerpt of imported Library text. It is stored *append-oriented* and is
//! never rewritten in place — a correction is a new record linked to the one
//! it corrects (see [`EvidenceRelation::Corrects`]).
//!
//! Derived material (summaries, reflections) is evidence of a different
//! [`EvidenceKind`] that links back to its sources with
//! [`EvidenceRelation::DerivedFrom`]. Following those links to their roots is
//! how the runtime refuses to count three summaries of one conversation as
//! three independent supports (audit A03, test T07).
//!
//! Nothing in this module grants authority. An evidence payload that contains
//! instructions is still data: it can support a proposal, and the proposal is
//! still judged by the mutation policy and the Continuity Kernel (test T06).

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use crate::mutation::{OriginClass, UnknownVocabulary};
use crate::time::UtcTimestamp;

/// What kind of thing an evidence record holds.
///
/// The distinction is structural, not a quality judgement: it decides which
/// domains a record may support, never how much it should be believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// Raw utterance of the person Kamimusuhi is talking to.
    UserUtterance,
    /// Raw utterance produced by Kamimusuhi itself.
    AgentUtterance,
    /// Runtime-observed event that is not speech (session opened, restart, ...).
    SystemEvent,
    /// Output of a cognitive resource call. External content, not testimony.
    ResourceResult,
    /// Text imported into the Library. External content, not testimony.
    LibraryExcerpt,
    /// Derived condensation of other evidence.
    Summary,
    /// Derived interpretation produced by internal cognition.
    Reflection,
    /// A record stating that earlier evidence was wrong. Links with
    /// [`EvidenceRelation::Corrects`] to what it corrects.
    Correction,
}

impl EvidenceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserUtterance => "user_utterance",
            Self::AgentUtterance => "agent_utterance",
            Self::SystemEvent => "system_event",
            Self::ResourceResult => "resource_result",
            Self::LibraryExcerpt => "library_excerpt",
            Self::Summary => "summary",
            Self::Reflection => "reflection",
            Self::Correction => "correction",
        }
    }

    /// Raw capture of something that reached the runtime, as it arrived.
    pub const fn is_raw_capture(self) -> bool {
        matches!(
            self,
            Self::UserUtterance | Self::AgentUtterance | Self::SystemEvent
        )
    }

    /// Content that came from outside the interaction (Library, resources).
    /// Such content may be recorded, but it is nobody's testimony.
    pub const fn is_external_content(self) -> bool {
        matches!(self, Self::ResourceResult | Self::LibraryExcerpt)
    }

    /// Produced by interpreting other evidence rather than by observing.
    pub const fn is_derived(self) -> bool {
        matches!(self, Self::Summary | Self::Reflection | Self::Correction)
    }

    /// May stand as first-party testimony about the person speaking.
    pub const fn is_first_party_testimony(self) -> bool {
        matches!(self, Self::UserUtterance)
    }
}

impl fmt::Display for EvidenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EvidenceKind {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "user_utterance" => Self::UserUtterance,
            "agent_utterance" => Self::AgentUtterance,
            "system_event" => Self::SystemEvent,
            "resource_result" => Self::ResourceResult,
            "library_excerpt" => Self::LibraryExcerpt,
            "summary" => Self::Summary,
            "reflection" => Self::Reflection,
            "correction" => Self::Correction,
            other => return Err(UnknownVocabulary::new("evidence_kind", other)),
        })
    }
}

/// How long a record is meant to be kept. W2 stores the declaration; the
/// forgetting pipeline that acts on it is a later wave (audit A04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Ordinary interaction record.
    Standard,
    /// Working material that may be dropped without losing history.
    Ephemeral,
    /// Requires explicit handling before export, sharing or training.
    Sensitive,
}

impl RetentionClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Ephemeral => "ephemeral",
            Self::Sensitive => "sensitive",
        }
    }
}

impl fmt::Display for RetentionClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RetentionClass {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "standard" => Self::Standard,
            "ephemeral" => Self::Ephemeral,
            "sensitive" => Self::Sensitive,
            other => return Err(UnknownVocabulary::new("retention_class", other)),
        })
    }
}

/// Where a record came from, in the sender's own numbering.
///
/// `source_sequence` is the sender's ordering, not the canonical commit order:
/// a late-arriving event keeps its original sequence instead of being recorded
/// as something new (audit A02).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSource {
    /// Opaque sender identifier (channel, sensor, adapter). Not a secret.
    pub source_id: Option<String>,
    pub source_sequence: Option<u64>,
    /// Integrity/dedup aid only. Not anonymisation and not a deletion
    /// substitute (plan §6.2).
    pub content_digest: Option<String>,
}

/// One durable evidence record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub evidence_id: EvidenceId,
    /// The individual whose canonical store owns this record. Evidence owned
    /// by one individual can never support another individual's mutation.
    pub individual_id: IndividualId,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub kind: EvidenceKind,
    pub origin_class: OriginClass,
    pub payload: serde_json::Value,
    pub source: EvidenceSource,
    pub retention_class: RetentionClass,
    pub created_at: UtcTimestamp,
}

/// Request to append a record. The store assigns `created_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvidence {
    pub evidence_id: EvidenceId,
    pub individual_id: IndividualId,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub kind: EvidenceKind,
    pub origin_class: OriginClass,
    pub payload: serde_json::Value,
    pub source: EvidenceSource,
    pub retention_class: RetentionClass,
}

/// How two evidence records relate. Both directions are recorded from the
/// newer record: it is derived from, or corrects, the older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRelation {
    /// The source record was interpreted to produce this one. Support does
    /// not multiply along this edge.
    DerivedFrom,
    /// This record states that the source record was wrong.
    Corrects,
    /// This record carries the same content as the source (re-delivery).
    Duplicates,
}

impl EvidenceRelation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DerivedFrom => "derived_from",
            Self::Corrects => "corrects",
            Self::Duplicates => "duplicates",
        }
    }

    /// Edges along which independent support collapses onto the source.
    pub const fn collapses_support(self) -> bool {
        matches!(self, Self::DerivedFrom | Self::Duplicates)
    }
}

impl fmt::Display for EvidenceRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EvidenceRelation {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "derived_from" => Self::DerivedFrom,
            "corrects" => Self::Corrects,
            "duplicates" => Self::Duplicates,
            other => return Err(UnknownVocabulary::new("evidence_relation", other)),
        })
    }
}

/// A directed link from a newer record to the record it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLink {
    pub from_evidence_id: EvidenceId,
    pub to_evidence_id: EvidenceId,
    pub relation: EvidenceRelation,
    pub created_at: UtcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewEvidenceLink {
    pub from_evidence_id: EvidenceId,
    pub to_evidence_id: EvidenceId,
    pub relation: EvidenceRelation,
}

/// One interaction session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub session_id: SessionId,
    pub individual_id: IndividualId,
    pub started_at: UtcTimestamp,
    pub ended_at: Option<UtcTimestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewSession {
    pub session_id: SessionId,
    pub individual_id: IndividualId,
}

/// One turn inside a session. `sequence` is the sender's ordering within the
/// session and is unique per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub individual_id: IndividualId,
    pub sequence: u64,
    pub started_at: UtcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewTurn {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub individual_id: IndividualId,
    pub sequence: u64,
}

/// Everything the mutation policy is allowed to know about one cited record.
///
/// Deliberately payload-free: a policy decision must not depend on reading the
/// content of a record, because content is exactly what an attacker controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceFacts {
    pub evidence_id: EvidenceId,
    pub individual_id: IndividualId,
    pub kind: EvidenceKind,
    pub origin_class: OriginClass,
    /// Non-derived records this one ultimately rests on. A raw record is its
    /// own root; a summary of a summary resolves to the original.
    pub root_evidence: Vec<EvidenceId>,
    /// Records this one explicitly corrects.
    pub corrects: Vec<EvidenceId>,
    /// A later record corrects this one, or corrects something in its
    /// [`Self::root_evidence`]. Either way it is no longer current: correcting
    /// an utterance does not leave a summary of that utterance standing.
    pub is_corrected: bool,
}

impl EvidenceFacts {
    /// Facts for a record with no links, used when building fixtures and by
    /// stores that resolve lineage separately.
    pub fn standalone(
        evidence_id: EvidenceId,
        individual_id: IndividualId,
        kind: EvidenceKind,
        origin_class: OriginClass,
    ) -> Self {
        Self {
            evidence_id,
            individual_id,
            kind,
            origin_class,
            root_evidence: vec![evidence_id],
            corrects: Vec::new(),
            is_corrected: false,
        }
    }
}

/// The cited evidence of one proposal, resolved once before the decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSnapshot {
    facts: Vec<EvidenceFacts>,
}

impl EvidenceSnapshot {
    pub fn new(facts: Vec<EvidenceFacts>) -> Self {
        Self { facts }
    }

    pub fn facts(&self) -> &[EvidenceFacts] {
        &self.facts
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    pub fn get(&self, evidence_id: EvidenceId) -> Option<&EvidenceFacts> {
        self.facts.iter().find(|f| f.evidence_id == evidence_id)
    }

    /// Distinct root records behind everything cited. Three summaries of one
    /// conversation collapse to one (T07).
    pub fn independent_support(&self) -> BTreeSet<EvidenceId> {
        self.facts
            .iter()
            .flat_map(|f| f.root_evidence.iter().copied())
            .collect()
    }

    pub fn independent_support_count(&self) -> usize {
        self.independent_support().len()
    }

    /// Records whose current status is contradicted by a later correction.
    pub fn corrected(&self) -> Vec<EvidenceId> {
        self.facts
            .iter()
            .filter(|f| f.is_corrected)
            .map(|f| f.evidence_id)
            .collect()
    }
}

/// Resolve root evidence for `id` by walking support-collapsing links.
///
/// `links` maps a record to the links it declares. Cycles terminate: a record
/// already on the path is not expanded again. A record with no collapsing link
/// is its own root.
pub fn resolve_roots(
    id: EvidenceId,
    links: &HashMap<EvidenceId, Vec<EvidenceLink>>,
) -> Vec<EvidenceId> {
    fn walk(
        id: EvidenceId,
        links: &HashMap<EvidenceId, Vec<EvidenceLink>>,
        seen: &mut BTreeSet<EvidenceId>,
        roots: &mut BTreeSet<EvidenceId>,
    ) {
        if !seen.insert(id) {
            return;
        }
        let sources: Vec<EvidenceId> = links
            .get(&id)
            .map(|ls| {
                ls.iter()
                    .filter(|l| l.relation.collapses_support())
                    .map(|l| l.to_evidence_id)
                    .collect()
            })
            .unwrap_or_default();
        if sources.is_empty() {
            roots.insert(id);
            return;
        }
        for source in sources {
            walk(source, links, seen, roots);
        }
    }

    let mut roots = BTreeSet::new();
    walk(id, links, &mut BTreeSet::new(), &mut roots);
    roots.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    #[error("evidence request is invalid: {reason}")]
    Invalid { reason: String },
    #[error("evidence {0} already exists with different content")]
    AlreadyExists(EvidenceId),
    #[error("evidence {0} is not present in this store")]
    NotFound(EvidenceId),
    #[error("individual {0} is not present in this store")]
    IndividualNotFound(IndividualId),
    #[error("evidence store is inconsistent: {detail}")]
    Corrupt { detail: String },
    #[error("evidence store is busy: {detail}")]
    Contended { detail: String },
    #[error("evidence store backend error: {message}")]
    Backend { message: String },
}

/// Read-only lineage lookup, the only evidence capability the Continuity
/// Kernel needs. Kept separate from the full store so the kernel cannot append
/// evidence while deciding a mutation.
pub trait EvidenceLookup: Send + Sync {
    /// Policy-facing facts for the cited records, with lineage resolved.
    /// Records that do not exist are simply absent from the result.
    fn facts(&self, evidence_ids: &[EvidenceId]) -> Result<EvidenceSnapshot, EvidenceError>;
}

/// Durable append-oriented evidence storage.
///
/// Appending evidence never advances the continuity head (plan §7): recording
/// what was said is not the same act as changing what Kamimusuhi holds true.
pub trait EvidenceStore: EvidenceLookup {
    fn open_session(&self, session: NewSession) -> Result<Session, EvidenceError>;

    /// Look up a session without creating one.
    ///
    /// Lets a caller that just minted an ID discover that the ID is already
    /// taken — which means its ID source is colliding with someone else's,
    /// not that it is retrying.
    fn session(&self, session_id: SessionId) -> Result<Option<Session>, EvidenceError>;

    fn record_turn(&self, turn: NewTurn) -> Result<Turn, EvidenceError>;

    /// Append a record. Re-appending the identical record is a no-op that
    /// returns the stored row; a differing record under the same ID is an error.
    fn append(&self, record: NewEvidence) -> Result<EvidenceRecord, EvidenceError>;

    fn get(&self, evidence_id: EvidenceId) -> Result<Option<EvidenceRecord>, EvidenceError>;

    /// Declare a lineage edge. Links are additive; nothing is overwritten.
    fn link(&self, link: NewEvidenceLink) -> Result<EvidenceLink, EvidenceError>;

    /// Links declared *by* `evidence_id`, i.e. what it depends on or corrects.
    fn links_from(&self, evidence_id: EvidenceId) -> Result<Vec<EvidenceLink>, EvidenceError>;

    /// Links pointing *at* `evidence_id`, i.e. what depends on or corrects it.
    fn links_to(&self, evidence_id: EvidenceId) -> Result<Vec<EvidenceLink>, EvidenceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(from: u128, to: u128, relation: EvidenceRelation) -> EvidenceLink {
        EvidenceLink {
            from_evidence_id: EvidenceId::from_u128(from),
            to_evidence_id: EvidenceId::from_u128(to),
            relation,
            created_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    fn graph(links: Vec<EvidenceLink>) -> HashMap<EvidenceId, Vec<EvidenceLink>> {
        let mut map: HashMap<EvidenceId, Vec<EvidenceLink>> = HashMap::new();
        for l in links {
            map.entry(l.from_evidence_id).or_default().push(l);
        }
        map
    }

    #[test]
    fn raw_record_is_its_own_root() {
        let roots = resolve_roots(EvidenceId::from_u128(1), &graph(Vec::new()));
        assert_eq!(roots, vec![EvidenceId::from_u128(1)]);
    }

    #[test]
    fn chained_derivation_resolves_to_the_original() {
        // 3 summarises 2, which summarises 1.
        let links = graph(vec![
            link(3, 2, EvidenceRelation::DerivedFrom),
            link(2, 1, EvidenceRelation::DerivedFrom),
        ]);
        assert_eq!(
            resolve_roots(EvidenceId::from_u128(3), &links),
            vec![EvidenceId::from_u128(1)]
        );
    }

    #[test]
    fn correction_does_not_collapse_onto_what_it_corrects() {
        // A correction is a new, independent statement about the world.
        let links = graph(vec![link(2, 1, EvidenceRelation::Corrects)]);
        assert_eq!(
            resolve_roots(EvidenceId::from_u128(2), &links),
            vec![EvidenceId::from_u128(2)]
        );
    }

    #[test]
    fn cyclic_links_terminate() {
        let links = graph(vec![
            link(1, 2, EvidenceRelation::DerivedFrom),
            link(2, 1, EvidenceRelation::DerivedFrom),
        ]);
        assert!(resolve_roots(EvidenceId::from_u128(1), &links).is_empty());
    }

    #[test]
    fn three_summaries_of_one_source_are_one_independent_support() {
        let root = EvidenceId::from_u128(1);
        let derived = |id: u128| EvidenceFacts {
            evidence_id: EvidenceId::from_u128(id),
            individual_id: IndividualId::from_u128(9),
            kind: EvidenceKind::Summary,
            origin_class: OriginClass::Inferred,
            root_evidence: vec![root],
            corrects: Vec::new(),
            is_corrected: false,
        };
        let snapshot = EvidenceSnapshot::new(vec![derived(2), derived(3), derived(4)]);
        assert_eq!(snapshot.independent_support_count(), 1);
        assert_eq!(
            snapshot.independent_support().into_iter().next(),
            Some(root)
        );
    }

    #[test]
    fn two_distinct_sources_are_two_supports() {
        let a = EvidenceFacts::standalone(
            EvidenceId::from_u128(1),
            IndividualId::from_u128(9),
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
        );
        let b = EvidenceFacts::standalone(
            EvidenceId::from_u128(2),
            IndividualId::from_u128(9),
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
        );
        assert_eq!(
            EvidenceSnapshot::new(vec![a, b]).independent_support_count(),
            2
        );
    }

    #[test]
    fn vocabulary_round_trips() {
        for kind in [
            EvidenceKind::UserUtterance,
            EvidenceKind::AgentUtterance,
            EvidenceKind::SystemEvent,
            EvidenceKind::ResourceResult,
            EvidenceKind::LibraryExcerpt,
            EvidenceKind::Summary,
            EvidenceKind::Reflection,
            EvidenceKind::Correction,
        ] {
            assert_eq!(kind.as_str().parse::<EvidenceKind>().unwrap(), kind);
        }
        for retention in [
            RetentionClass::Standard,
            RetentionClass::Ephemeral,
            RetentionClass::Sensitive,
        ] {
            assert_eq!(
                retention.as_str().parse::<RetentionClass>().unwrap(),
                retention
            );
        }
        for relation in [
            EvidenceRelation::DerivedFrom,
            EvidenceRelation::Corrects,
            EvidenceRelation::Duplicates,
        ] {
            assert_eq!(
                relation.as_str().parse::<EvidenceRelation>().unwrap(),
                relation
            );
        }
        assert!("gossip".parse::<EvidenceKind>().is_err());
    }

    #[test]
    fn kind_classification_is_exclusive() {
        for kind in [
            EvidenceKind::UserUtterance,
            EvidenceKind::AgentUtterance,
            EvidenceKind::SystemEvent,
            EvidenceKind::ResourceResult,
            EvidenceKind::LibraryExcerpt,
            EvidenceKind::Summary,
            EvidenceKind::Reflection,
            EvidenceKind::Correction,
        ] {
            let flags = u8::from(kind.is_raw_capture())
                + u8::from(kind.is_external_content())
                + u8::from(kind.is_derived());
            assert_eq!(
                flags, 1,
                "{kind} must be exactly one of raw/external/derived"
            );
        }
        assert!(EvidenceKind::UserUtterance.is_first_party_testimony());
        assert!(!EvidenceKind::LibraryExcerpt.is_first_party_testimony());
        assert!(!EvidenceKind::Summary.is_first_party_testimony());
    }
}
