//! Guarded canonical mutations: proposals, decisions and the deterministic
//! `MutationPolicyV0`.
//!
//! Nothing in this module writes state. A Persona Core or worker only ever
//! produces a proposal; the Continuity Kernel decides whether it is activated.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::continuity::{ContinuityHead, ExpectedHead, WriterIdentity};
use crate::domain_separation::{SeparationContext, check_domain_separation};
use crate::evidence::EvidenceSnapshot;
use crate::ids::{EvidenceId, IndividualId, MemoryId, PolicyVersion, ProposalId};
use crate::memory::StateRecordFacts;
use crate::persona::ProposalDraft;
use crate::time::UtcTimestamp;

/// Durable state domain a proposal targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationDomain {
    /// What happened: captured interaction episodes.
    Episodic,
    /// Facts about other people/agents, e.g. the user's stated preferences.
    Relationship,
    /// Kamimusuhi's own self-model. Reserved in phase 1; no operation accepts it yet.
    #[serde(rename = "self")]
    SelfModel,
}

impl MutationDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Relationship => "relationship",
            Self::SelfModel => "self",
        }
    }
}

impl fmt::Display for MutationDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MutationDomain {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "episodic" => Self::Episodic,
            "relationship" => Self::Relationship,
            "self" => Self::SelfModel,
            other => return Err(UnknownVocabulary::new("domain", other)),
        })
    }
}

/// Operation applied inside a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperation {
    /// Record an episode as it was observed.
    Capture,
    /// Assert a fact about a subject.
    Fact,
    /// Explicitly supersede a prior fact.
    Correction,
}

impl MutationOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Fact => "fact",
            Self::Correction => "correction",
        }
    }
}

impl fmt::Display for MutationOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MutationOperation {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "capture" => Self::Capture,
            "fact" => Self::Fact,
            "correction" => Self::Correction,
            other => return Err(UnknownVocabulary::new("operation", other)),
        })
    }
}

/// How the underlying content came to exist. `Dream`/`Simulated` content must
/// never be stored as an external event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginClass {
    Observed,
    Reported,
    Inferred,
    Simulated,
    Dream,
    Replayed,
}

impl OriginClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Reported => "reported",
            Self::Inferred => "inferred",
            Self::Simulated => "simulated",
            Self::Dream => "dream",
            Self::Replayed => "replayed",
        }
    }

    /// Origins that may stand in for something that happened in the world.
    pub const fn is_external_event(self) -> bool {
        matches!(self, Self::Observed | Self::Reported)
    }
}

impl fmt::Display for OriginClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OriginClass {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "observed" => Self::Observed,
            "reported" => Self::Reported,
            "inferred" => Self::Inferred,
            "simulated" => Self::Simulated,
            "dream" => Self::Dream,
            "replayed" => Self::Replayed,
            other => return Err(UnknownVocabulary::new("origin_class", other)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {field} value {value:?}")]
pub struct UnknownVocabulary {
    pub field: &'static str,
    pub value: String,
}

impl UnknownVocabulary {
    pub fn new(field: &'static str, value: &str) -> Self {
        Self {
            field,
            value: value.to_owned(),
        }
    }
}

/// A fully attributed request to change canonical state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationProposal {
    pub proposal_id: ProposalId,
    pub individual_id: IndividualId,
    pub domain: MutationDomain,
    pub operation: MutationOperation,
    pub subject_key: Option<String>,
    pub candidate: serde_json::Value,
    pub expected_head: ExpectedHead,
    pub evidence_refs: Vec<EvidenceId>,
    /// The state record this proposal replaces. Only a
    /// [`MutationOperation::Correction`] may set it, and the old record is
    /// marked superseded rather than rewritten.
    pub supersedes: Option<MemoryId>,
    pub origin_class: OriginClass,
    pub requested_by: WriterIdentity,
    pub policy_version: PolicyVersion,
    pub idempotency_key: String,
    pub created_at: UtcTimestamp,
}

/// The runtime-owned part of a proposal: everything a Persona Core draft is
/// not allowed to decide for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalAttribution {
    pub proposal_id: ProposalId,
    pub individual_id: IndividualId,
    pub expected_head: ExpectedHead,
    pub requested_by: WriterIdentity,
    pub policy_version: PolicyVersion,
    pub idempotency_key: String,
    pub created_at: UtcTimestamp,
}

impl MutationProposal {
    /// Attach head, writer authority and policy version to a Persona Core draft.
    pub fn from_draft(draft: ProposalDraft, attribution: ProposalAttribution) -> Self {
        Self {
            proposal_id: attribution.proposal_id,
            individual_id: attribution.individual_id,
            domain: draft.domain,
            operation: draft.operation,
            subject_key: draft.subject_key,
            candidate: draft.candidate,
            expected_head: attribution.expected_head,
            evidence_refs: draft.evidence_refs,
            supersedes: draft.supersedes,
            origin_class: draft.origin_class,
            requested_by: attribution.requested_by,
            policy_version: attribution.policy_version,
            idempotency_key: attribution.idempotency_key,
            created_at: attribution.created_at,
        }
    }

    /// Checks that do not need any store access. Failing these means the
    /// proposal cannot even be recorded, so they are errors, not decisions.
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.proposal_id.is_nil() {
            return Err("proposal_id is nil".to_owned());
        }
        if self.individual_id.is_nil() {
            return Err("individual_id is nil".to_owned());
        }
        if self.expected_head.commit_id.is_nil() {
            return Err("expected_head.commit_id is nil".to_owned());
        }
        if self.idempotency_key.trim().is_empty() {
            return Err("idempotency_key is empty".to_owned());
        }
        if self.requested_by.node_id.is_nil() || self.requested_by.boot_id.is_nil() {
            return Err("requested_by has a nil node_id or boot_id".to_owned());
        }
        Ok(())
    }

    /// Canonical text of the content that must be identical for two proposals
    /// sharing an idempotency key. Anything else (IDs, timestamps, writer)
    /// may legitimately differ between retries.
    pub fn payload_fingerprint(&self) -> String {
        let mut evidence: Vec<String> =
            self.evidence_refs.iter().map(ToString::to_string).collect();
        evidence.sort_unstable();
        let fingerprint = serde_json::json!({
            "domain": self.domain,
            "operation": self.operation,
            "subject_key": self.subject_key,
            "candidate": self.candidate,
            "evidence_refs": evidence,
            "supersedes": self.supersedes,
            "origin_class": self.origin_class,
            "expected_head": self.expected_head,
        });
        // serde_json sorts object keys (BTreeMap) so this is deterministic.
        fingerprint.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Reject,
    Quarantine,
}

impl Disposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Quarantine => "quarantine",
        }
    }
}

impl fmt::Display for Disposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Disposition {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "accept" => Self::Accept,
            "reject" => Self::Reject,
            "quarantine" => Self::Quarantine,
            other => return Err(UnknownVocabulary::new("disposition", other)),
        })
    }
}

/// Why a proposal was accepted or refused. Stable codes for audit and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    Accepted,
    /// `expected_head` no longer matches the current head.
    StalePredecessor,
    /// The proposing writer's epoch has been superseded by a newer claimant.
    StaleWriterEpoch,
    MissingEvidence,
    /// A cited evidence record does not exist in this store.
    EvidenceNotFound,
    /// A cited evidence record belongs to a different individual.
    EvidenceOwnerMismatch,
    /// The cited evidence cannot support the targeted domain (Library text or
    /// resource output offered as first-party testimony, a derived summary
    /// offered as a raw capture, ...).
    EvidenceDomainMismatch,
    /// Outside content was offered as a statement about Kamimusuhi itself.
    SelfDomainContamination,
    /// A correction named no target, or a target that cannot be superseded.
    SupersedeTargetInvalid,
    /// The cited evidence, or something it rests on, has been corrected. It
    /// cannot support a new current fact until fresh evidence re-establishes
    /// one.
    EvidenceCorrected,
    UnsupportedOperation,
    PolicyVersionMismatch,
    IndividualMismatch,
    DuplicateProposalPayloadMismatch,
    /// Dream/simulated/inferred/replayed content offered as an external fact.
    OriginNotExternal,
    MalformedProposal,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::StalePredecessor => "STALE_PREDECESSOR",
            Self::StaleWriterEpoch => "STALE_WRITER_EPOCH",
            Self::MissingEvidence => "MISSING_EVIDENCE",
            Self::EvidenceNotFound => "EVIDENCE_NOT_FOUND",
            Self::EvidenceOwnerMismatch => "EVIDENCE_OWNER_MISMATCH",
            Self::EvidenceDomainMismatch => "EVIDENCE_DOMAIN_MISMATCH",
            Self::SelfDomainContamination => "SELF_DOMAIN_CONTAMINATION",
            Self::SupersedeTargetInvalid => "SUPERSEDE_TARGET_INVALID",
            Self::EvidenceCorrected => "EVIDENCE_CORRECTED",
            Self::UnsupportedOperation => "UNSUPPORTED_OPERATION",
            Self::PolicyVersionMismatch => "POLICY_VERSION_MISMATCH",
            Self::IndividualMismatch => "INDIVIDUAL_MISMATCH",
            Self::DuplicateProposalPayloadMismatch => "DUPLICATE_PROPOSAL_PAYLOAD_MISMATCH",
            Self::OriginNotExternal => "ORIGIN_NOT_EXTERNAL",
            Self::MalformedProposal => "MALFORMED_PROPOSAL",
        }
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReasonCode {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ACCEPTED" => Self::Accepted,
            "STALE_PREDECESSOR" => Self::StalePredecessor,
            "STALE_WRITER_EPOCH" => Self::StaleWriterEpoch,
            "MISSING_EVIDENCE" => Self::MissingEvidence,
            "EVIDENCE_NOT_FOUND" => Self::EvidenceNotFound,
            "EVIDENCE_OWNER_MISMATCH" => Self::EvidenceOwnerMismatch,
            "EVIDENCE_DOMAIN_MISMATCH" => Self::EvidenceDomainMismatch,
            "SELF_DOMAIN_CONTAMINATION" => Self::SelfDomainContamination,
            "SUPERSEDE_TARGET_INVALID" => Self::SupersedeTargetInvalid,
            "EVIDENCE_CORRECTED" => Self::EvidenceCorrected,
            "UNSUPPORTED_OPERATION" => Self::UnsupportedOperation,
            "POLICY_VERSION_MISMATCH" => Self::PolicyVersionMismatch,
            "INDIVIDUAL_MISMATCH" => Self::IndividualMismatch,
            "DUPLICATE_PROPOSAL_PAYLOAD_MISMATCH" => Self::DuplicateProposalPayloadMismatch,
            "ORIGIN_NOT_EXTERNAL" => Self::OriginNotExternal,
            "MALFORMED_PROPOSAL" => Self::MalformedProposal,
            other => return Err(UnknownVocabulary::new("reason_code", other)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationDecision {
    pub proposal_id: ProposalId,
    pub disposition: Disposition,
    pub reason_code: ReasonCode,
    /// Human-readable detail for operators. Never raw private content.
    pub detail: Option<String>,
    pub policy_version: PolicyVersion,
    pub decided_at: UtcTimestamp,
}

impl MutationDecision {
    pub fn accept(
        proposal: &MutationProposal,
        policy_version: PolicyVersion,
        decided_at: UtcTimestamp,
    ) -> Self {
        Self {
            proposal_id: proposal.proposal_id,
            disposition: Disposition::Accept,
            reason_code: ReasonCode::Accepted,
            detail: None,
            policy_version,
            decided_at,
        }
    }

    pub fn reject(
        proposal: &MutationProposal,
        reason_code: ReasonCode,
        detail: impl Into<String>,
        policy_version: PolicyVersion,
        decided_at: UtcTimestamp,
    ) -> Self {
        Self {
            proposal_id: proposal.proposal_id,
            disposition: Disposition::Reject,
            reason_code,
            detail: Some(detail.into()),
            policy_version,
            decided_at,
        }
    }
}

/// What the policy may look at. Loaded before the decision, outside any
/// long-running call, so no canonical transaction is held open while a model
/// or network call runs.
///
/// The evidence and supersession facts are payload-free by construction: a
/// decision must not depend on the text of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyContext {
    pub current_head: ContinuityHead,
    pub now: UtcTimestamp,
    /// Resolved facts for `proposal.evidence_refs`, lineage included.
    pub evidence: EvidenceSnapshot,
    /// Facts about `proposal.supersedes`, if it names an existing record.
    pub supersedes_target: Option<StateRecordFacts>,
}

impl PolicyContext {
    /// Context for a proposal that cites no stored evidence. Such a proposal
    /// is rejected, so this is only useful for head-level tests.
    pub fn without_evidence(current_head: ContinuityHead, now: UtcTimestamp) -> Self {
        Self {
            current_head,
            now,
            evidence: EvidenceSnapshot::default(),
            supersedes_target: None,
        }
    }

    fn separation(&self) -> SeparationContext {
        SeparationContext {
            evidence: self.evidence.clone(),
            supersedes_target: self.supersedes_target.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("mutation policy backend error: {message}")]
    Backend { message: String },
}

pub trait MutationPolicy: Send + Sync {
    fn version(&self) -> PolicyVersion;

    fn decide(
        &self,
        proposal: &MutationProposal,
        context: &PolicyContext,
    ) -> Result<MutationDecision, PolicyError>;
}

/// Deterministic allow/deny baseline. Not an LLM judge.
///
/// Wave 1 scope: structural, version, vocabulary, origin, writer-epoch and
/// predecessor checks. Wave 2 adds evidence existence, evidence ownership,
/// domain separation (Library/resource/derived content cannot become
/// first-party or self state) and correction/supersession target checks, all
/// delegated to [`check_domain_separation`]. Everything not explicitly
/// allowed is still rejected.
#[derive(Debug, Default, Clone, Copy)]
pub struct MutationPolicyV0;

impl MutationPolicyV0 {
    pub const VERSION: PolicyVersion = PolicyVersion(0);

    const fn supports(domain: MutationDomain, operation: MutationOperation) -> bool {
        matches!(
            (domain, operation),
            (MutationDomain::Episodic, MutationOperation::Capture)
                | (MutationDomain::Relationship, MutationOperation::Fact)
                | (MutationDomain::Relationship, MutationOperation::Correction)
        )
    }
}

impl MutationPolicy for MutationPolicyV0 {
    fn version(&self) -> PolicyVersion {
        Self::VERSION
    }

    fn decide(
        &self,
        proposal: &MutationProposal,
        context: &PolicyContext,
    ) -> Result<MutationDecision, PolicyError> {
        let reject = |code, detail: String| {
            Ok(MutationDecision::reject(
                proposal,
                code,
                detail,
                Self::VERSION,
                context.now,
            ))
        };

        if let Err(reason) = proposal.validate_structure() {
            return reject(ReasonCode::MalformedProposal, reason);
        }
        if proposal.individual_id != context.current_head.individual_id {
            return reject(
                ReasonCode::IndividualMismatch,
                format!(
                    "proposal targets {} but head belongs to {}",
                    proposal.individual_id, context.current_head.individual_id
                ),
            );
        }
        if proposal.policy_version != Self::VERSION {
            return reject(
                ReasonCode::PolicyVersionMismatch,
                format!(
                    "proposal was built for {} but this runtime enforces {}",
                    proposal.policy_version,
                    Self::VERSION
                ),
            );
        }
        // Domain separation runs before the supported-operation check for the
        // self domain. "this evidence cannot support self state" stays the
        // accurate reason once self mutation is implemented, whereas
        // UNSUPPORTED_OPERATION would silently change meaning.
        if proposal.domain == MutationDomain::SelfModel
            && let Err(violation) = check_domain_separation(proposal, &context.separation())
        {
            return reject(violation.reason_code, violation.detail);
        }
        if !Self::supports(proposal.domain, proposal.operation) {
            return reject(
                ReasonCode::UnsupportedOperation,
                format!(
                    "{}.{} is not accepted by {}",
                    proposal.domain,
                    proposal.operation,
                    Self::VERSION
                ),
            );
        }
        if proposal.evidence_refs.is_empty() || proposal.evidence_refs.iter().any(|e| e.is_nil()) {
            return reject(
                ReasonCode::MissingEvidence,
                "proposal cites no valid evidence".to_owned(),
            );
        }
        if !proposal.origin_class.is_external_event() {
            return reject(
                ReasonCode::OriginNotExternal,
                format!(
                    "origin_class {} cannot be recorded as an external fact",
                    proposal.origin_class
                ),
            );
        }
        if proposal.domain == MutationDomain::Relationship
            && proposal
                .subject_key
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
        {
            return reject(
                ReasonCode::MalformedProposal,
                "relationship proposal has no subject_key".to_owned(),
            );
        }
        if let Err(violation) = check_domain_separation(proposal, &context.separation()) {
            return reject(violation.reason_code, violation.detail);
        }
        if proposal.requested_by.writer_epoch != context.current_head.writer_epoch {
            return reject(
                ReasonCode::StaleWriterEpoch,
                format!(
                    "writer {} is fenced; current is {}",
                    proposal.requested_by.writer_epoch, context.current_head.writer_epoch
                ),
            );
        }
        if proposal.expected_head != context.current_head.expected() {
            return reject(
                ReasonCode::StalePredecessor,
                format!(
                    "expected {}@{} but head is {}@{}",
                    proposal.expected_head.commit_id,
                    proposal.expected_head.generation,
                    context.current_head.commit_id,
                    context.current_head.generation
                ),
            );
        }
        Ok(MutationDecision::accept(
            proposal,
            Self::VERSION,
            context.now,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::continuity::{Generation, WriterEpoch};
    use crate::evidence::{EvidenceFacts, EvidenceKind};
    use crate::ids::{BootId, CommitId, NodeId};
    use crate::memory::LifecycleState;

    use super::*;

    const UTTERANCE: EvidenceId = EvidenceId::from_u128(50);
    const PRIOR_FACT: MemoryId = MemoryId::from_u128(70);

    fn head() -> ContinuityHead {
        ContinuityHead {
            individual_id: IndividualId::from_u128(1),
            commit_id: CommitId::from_u128(10),
            generation: Generation(3),
            writer_epoch: WriterEpoch(2),
            updated_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    fn proposal() -> MutationProposal {
        MutationProposal {
            proposal_id: ProposalId::from_u128(100),
            individual_id: IndividualId::from_u128(1),
            domain: MutationDomain::Relationship,
            operation: MutationOperation::Fact,
            subject_key: Some("user-fixture".to_owned()),
            candidate: serde_json::json!({ "preference": "ほうじ茶" }),
            expected_head: ExpectedHead {
                commit_id: CommitId::from_u128(10),
                generation: Generation(3),
            },
            evidence_refs: vec![UTTERANCE],
            supersedes: None,
            origin_class: OriginClass::Reported,
            requested_by: WriterIdentity {
                node_id: NodeId::from_u128(7),
                boot_id: BootId::from_u128(8),
                writer_epoch: WriterEpoch(2),
            },
            policy_version: MutationPolicyV0::VERSION,
            idempotency_key: "turn-1/draft-0".to_owned(),
            created_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    /// The fixture utterance, owned by the individual under test.
    fn utterance_facts() -> EvidenceFacts {
        EvidenceFacts::standalone(
            UTTERANCE,
            IndividualId::from_u128(1),
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
        )
    }

    fn context() -> PolicyContext {
        PolicyContext {
            current_head: head(),
            now: UtcTimestamp::from_unix_millis(5),
            evidence: EvidenceSnapshot::new(vec![utterance_facts()]),
            supersedes_target: None,
        }
    }

    fn decide(p: &MutationProposal) -> MutationDecision {
        decide_with(p, &context())
    }

    fn decide_with(p: &MutationProposal, context: &PolicyContext) -> MutationDecision {
        MutationPolicyV0.decide(p, context).unwrap()
    }

    #[test]
    fn accepts_well_formed_relationship_fact() {
        let d = decide(&proposal());
        assert_eq!(d.disposition, Disposition::Accept);
        assert_eq!(d.reason_code, ReasonCode::Accepted);
        assert_eq!(d.policy_version, MutationPolicyV0::VERSION);
        assert_eq!(d.decided_at, UtcTimestamp::from_unix_millis(5));
    }

    type Mutate = Box<dyn Fn(&mut MutationProposal)>;

    #[test]
    fn rejection_table() {
        let cases: Vec<(&str, Mutate, ReasonCode)> = vec![
            (
                "nil proposal id",
                Box::new(|p| p.proposal_id = ProposalId::from_u128(0)),
                ReasonCode::MalformedProposal,
            ),
            (
                "empty idempotency key",
                Box::new(|p| p.idempotency_key = "  ".to_owned()),
                ReasonCode::MalformedProposal,
            ),
            (
                "other individual",
                Box::new(|p| p.individual_id = IndividualId::from_u128(2)),
                ReasonCode::IndividualMismatch,
            ),
            (
                "future policy version",
                Box::new(|p| p.policy_version = PolicyVersion(1)),
                ReasonCode::PolicyVersionMismatch,
            ),
            (
                "self domain fed from outside",
                Box::new(|p| p.domain = MutationDomain::SelfModel),
                ReasonCode::SelfDomainContamination,
            ),
            (
                "episodic correction is not supported",
                Box::new(|p| {
                    p.domain = MutationDomain::Episodic;
                    p.operation = MutationOperation::Correction;
                }),
                ReasonCode::UnsupportedOperation,
            ),
            (
                "correction without a target",
                Box::new(|p| p.operation = MutationOperation::Correction),
                ReasonCode::SupersedeTargetInvalid,
            ),
            (
                "fact that claims to supersede",
                Box::new(|p| p.supersedes = Some(PRIOR_FACT)),
                ReasonCode::MalformedProposal,
            ),
            (
                "evidence that does not exist",
                Box::new(|p| p.evidence_refs = vec![EvidenceId::from_u128(51)]),
                ReasonCode::EvidenceNotFound,
            ),
            (
                "no evidence",
                Box::new(|p| p.evidence_refs.clear()),
                ReasonCode::MissingEvidence,
            ),
            (
                "nil evidence",
                Box::new(|p| p.evidence_refs = vec![EvidenceId::from_u128(0)]),
                ReasonCode::MissingEvidence,
            ),
            (
                "dream origin",
                Box::new(|p| p.origin_class = OriginClass::Dream),
                ReasonCode::OriginNotExternal,
            ),
            (
                "simulated origin",
                Box::new(|p| p.origin_class = OriginClass::Simulated),
                ReasonCode::OriginNotExternal,
            ),
            (
                "relationship without subject",
                Box::new(|p| p.subject_key = None),
                ReasonCode::MalformedProposal,
            ),
            (
                "old writer epoch",
                Box::new(|p| p.requested_by.writer_epoch = WriterEpoch(1)),
                ReasonCode::StaleWriterEpoch,
            ),
            (
                "stale generation",
                Box::new(|p| p.expected_head.generation = Generation(2)),
                ReasonCode::StalePredecessor,
            ),
            (
                "stale commit",
                Box::new(|p| p.expected_head.commit_id = CommitId::from_u128(9)),
                ReasonCode::StalePredecessor,
            ),
        ];
        for (name, mutate, expected) in cases {
            let mut p = proposal();
            mutate(&mut p);
            let d = decide(&p);
            assert_eq!(d.disposition, Disposition::Reject, "{name}");
            assert_eq!(d.reason_code, expected, "{name}");
            assert!(d.detail.is_some(), "{name}");
        }
    }

    #[test]
    fn evidence_of_another_individual_is_rejected() {
        let mut context = context();
        context.evidence = EvidenceSnapshot::new(vec![EvidenceFacts::standalone(
            UTTERANCE,
            IndividualId::from_u128(2),
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
        )]);
        let decision = decide_with(&proposal(), &context);
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::EvidenceOwnerMismatch);
    }

    #[test]
    fn library_text_alone_cannot_create_a_relationship_fact() {
        let mut context = context();
        context.evidence = EvidenceSnapshot::new(vec![EvidenceFacts::standalone(
            UTTERANCE,
            IndividualId::from_u128(1),
            EvidenceKind::LibraryExcerpt,
            OriginClass::Reported,
        )]);
        let decision = decide_with(&proposal(), &context);
        assert_eq!(decision.reason_code, ReasonCode::EvidenceDomainMismatch);
    }

    #[test]
    fn a_user_preference_never_reaches_the_self_domain() {
        let mut p = proposal();
        p.domain = MutationDomain::SelfModel;
        let decision = decide(&p);
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::SelfDomainContamination);
    }

    #[test]
    fn relationship_correction_with_a_live_target_is_supported() {
        let mut p = proposal();
        p.operation = MutationOperation::Correction;
        p.supersedes = Some(PRIOR_FACT);
        let mut context = context();
        context.supersedes_target = Some(crate::memory::StateRecordFacts {
            state_record_id: PRIOR_FACT,
            individual_id: IndividualId::from_u128(1),
            domain: MutationDomain::Relationship,
            subject_key: Some("user-fixture".to_owned()),
            lifecycle_state: LifecycleState::Active,
        });
        assert_eq!(decide_with(&p, &context).disposition, Disposition::Accept);
    }

    #[test]
    fn episodic_capture_is_supported() {
        let mut p = proposal();
        p.domain = MutationDomain::Episodic;
        p.operation = MutationOperation::Capture;
        p.subject_key = None;
        assert_eq!(decide(&p).disposition, Disposition::Accept);
    }

    #[test]
    fn fingerprint_ignores_ids_and_evidence_order() {
        let a = proposal();
        let mut b = proposal();
        b.proposal_id = ProposalId::from_u128(101);
        b.created_at = UtcTimestamp::from_unix_millis(99);
        b.requested_by.boot_id = BootId::from_u128(9);
        assert_eq!(a.payload_fingerprint(), b.payload_fingerprint());

        let mut c = proposal();
        c.evidence_refs = vec![EvidenceId::from_u128(51), UTTERANCE];
        let mut d = proposal();
        d.evidence_refs = vec![UTTERANCE, EvidenceId::from_u128(51)];
        assert_eq!(c.payload_fingerprint(), d.payload_fingerprint());

        let mut f = proposal();
        f.supersedes = Some(PRIOR_FACT);
        assert_ne!(a.payload_fingerprint(), f.payload_fingerprint());

        let mut e = proposal();
        e.candidate = serde_json::json!({ "preference": "緑茶" });
        assert_ne!(a.payload_fingerprint(), e.payload_fingerprint());
    }

    #[test]
    fn vocabulary_round_trips() {
        for code in [
            ReasonCode::Accepted,
            ReasonCode::StalePredecessor,
            ReasonCode::StaleWriterEpoch,
            ReasonCode::MissingEvidence,
            ReasonCode::EvidenceNotFound,
            ReasonCode::EvidenceOwnerMismatch,
            ReasonCode::EvidenceDomainMismatch,
            ReasonCode::SelfDomainContamination,
            ReasonCode::SupersedeTargetInvalid,
            ReasonCode::EvidenceCorrected,
            ReasonCode::UnsupportedOperation,
            ReasonCode::PolicyVersionMismatch,
            ReasonCode::IndividualMismatch,
            ReasonCode::DuplicateProposalPayloadMismatch,
            ReasonCode::OriginNotExternal,
            ReasonCode::MalformedProposal,
        ] {
            assert_eq!(code.as_str().parse::<ReasonCode>().unwrap(), code);
        }
        assert_eq!(
            "self".parse::<MutationDomain>().unwrap(),
            MutationDomain::SelfModel
        );
        assert_eq!(
            serde_json::to_string(&MutationDomain::SelfModel).unwrap(),
            "\"self\""
        );
        assert!("persona".parse::<MutationDomain>().is_err());
    }
}
