//! Mutation proposal / decision types and the `MutationPolicy` contract.
//!
//! A [`MutationProposal`] is the only way anything (Persona Core, a
//! background process, an operator) can ask the continuity kernel to
//! change canonical state. Policy decides accept/reject; only an accepted
//! decision may be turned into a canonical commit by
//! [`crate::continuity::ContinuityStore::activate`].

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::evidence::EvidenceKind;
use crate::ids::{CommitId, EvidenceId, IndividualId, MemoryId, PolicyVersion, ProposalId};
use crate::memory::LifecycleState;
use crate::time::UtcTimestamp;

/// Where a proposal's grounding evidence/content ultimately originated.
/// Used by policy to reject e.g. treating a dream/simulation as an
/// external event, or a Library excerpt as a relationship fact.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginClass {
    /// Directly observed canonical interaction (e.g. a user utterance).
    CanonicalInteraction,
    /// External Library content (non-canonical, provenance-tracked).
    LibraryEvidence,
    /// Output of an external cognitive resource call.
    ExternalResourceResult,
    /// Internal simulation/dream/hypothetical, never a canonical fact by
    /// itself.
    Simulation,
    /// Operator/administrative origin.
    Operator,
}

/// Domain a mutation targets. Kept as an open string-backed enum so new
/// domains can be added by later waves without breaking serialization of
/// existing rows, but the known set below is what `MutationPolicyV0`
/// currently understands.
#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationDomain {
    Episodic,
    Relationship,
    /// Reserved for future use; `MutationPolicyV0` does not accept
    /// mutations in this domain yet (see plan §6.3).
    SelfDomain,
    #[serde(other)]
    Unsupported,
}

/// Operation requested within a domain.
#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperation {
    Capture,
    Fact,
    Correction,
    #[serde(other)]
    Unsupported,
}

/// A typed request to change canonical state. Proposals are never applied
/// directly; they pass through [`crate::mutation::MutationPolicy`] and
/// then [`crate::continuity::ContinuityStore::activate`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationProposal {
    pub proposal_id: ProposalId,
    pub individual_id: IndividualId,
    pub domain: MutationDomain,
    pub operation: MutationOperation,
    /// Candidate payload; validated by policy, opaque to the continuity
    /// kernel itself beyond storage.
    pub candidate: JsonValue,
    /// Evidence this proposal is grounded in. Empty is rejected by
    /// `MutationPolicyV0` for domains that require grounding.
    pub evidence_refs: Vec<EvidenceId>,
    /// Who/what a `relationship.*` proposal is about. Required (and
    /// checked against `PolicyContext::subject_is_known_person`) for
    /// `MutationDomain::Relationship`; unused for `Episodic`. Keeping
    /// this a first-class field (rather than burying it in `candidate`)
    /// is what lets policy tell a user-origin relationship fact apart
    /// from a self-preference miscapture (plan §6.3, §8.1).
    pub subject_key: Option<String>,
    /// For `MutationOperation::Correction`: the prior state record this
    /// proposal explicitly supersedes. Required for corrections; the
    /// referenced record must exist, belong to the same individual, and
    /// currently be `LifecycleState::Active` (plan §8.1 "prior state と
    /// 新しい evidence を参照し、supersession を明示する").
    pub supersedes_state_record_id: Option<MemoryId>,
    /// The commit this proposal expects to apply on top of. Used for the
    /// expected-head CAS check in `activate()`.
    pub expected_commit_id: Option<CommitId>,
    pub expected_generation: u64,
    pub origin_class: OriginClass,
    pub requested_by: String,
    pub policy_version: PolicyVersion,
    /// Caller-supplied idempotency key. Retrying the same logical
    /// proposal (same key) must not create a second commit.
    pub idempotency_key: String,
    pub created_at: UtcTimestamp,
}

/// Outcome of a policy decision.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Reject,
    Quarantine,
}

/// Stable machine-readable reason codes. New codes may be appended; do not
/// renumber/rename existing ones once persisted.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    Accepted,
    StalePredecessor,
    UnsupportedDomain,
    UnsupportedOperation,
    IndividualMismatch,
    MissingEvidence,
    EvidenceOwnerMismatch,
    LibraryOnlyContamination,
    ExternalResourceOnlyContamination,
    PersonaNarrationMisattributedAsEvidence,
    SimulationOriginNotExternalEvent,
    DuplicateProposalPayloadMismatch,
    SchemaOrPolicyVersionMismatch,
    MissingSubject,
    InvalidRelationshipSubject,
    MissingSupersedesTarget,
    CorrectionTargetNotFound,
    CorrectionTargetNotActive,
}

/// Durable decision record for one proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationDecision {
    pub proposal_id: ProposalId,
    pub disposition: Disposition,
    pub reason_code: ReasonCode,
    pub decided_at: UtcTimestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy context lookup failed: {0}")]
    ContextLookup(String),
}

/// What policy needs to know about one referenced evidence record,
/// without depending on the full [`crate::evidence::EvidenceStore`]
/// contract. `None` from
/// [`PolicyContext::evidence_provenance`] means the evidence does not
/// exist at all.
#[derive(Clone, Debug)]
pub struct EvidenceProvenance {
    pub individual_id: IndividualId,
    pub origin_class: OriginClass,
    pub kind: EvidenceKind,
}

/// Everything `MutationPolicyV0` needs to know about the world beyond the
/// proposal itself, without depending on a concrete evidence/state store.
/// `kamimusuhi-store-sqlite` implements this against real tables;
/// `kamimusuhi-testkit` provides an in-memory fixture implementation.
pub trait PolicyContext {
    /// Provenance of one referenced evidence record, if it exists at
    /// all. Used to check ownership, reject Library/external/simulation
    /// grounding, and reject a Persona Core narration mistaken for
    /// evidence (plan §8.2).
    fn evidence_provenance(
        &self,
        evidence_id: EvidenceId,
    ) -> Result<Option<EvidenceProvenance>, PolicyError>;

    /// Whether `subject_key` names a valid `relationship.*` subject for
    /// this individual (the user, or another known-person domain
    /// subject) rather than the individual itself (plan §8.1:
    /// relationship subject must not collapse into `self`).
    fn subject_is_known_person(
        &self,
        individual_id: IndividualId,
        subject_key: &str,
    ) -> Result<bool, PolicyError>;

    /// Current lifecycle of a prior state record, scoped to the given
    /// individual. `None` if it does not exist or belongs to a
    /// different individual. Used by `relationship.correction` to
    /// reject targeting an already-superseded or nonexistent record.
    fn state_record_lifecycle(
        &self,
        state_record_id: MemoryId,
        individual_id: IndividualId,
    ) -> Result<Option<LifecycleState>, PolicyError>;

    /// Look up a previously-decided proposal with the same idempotency
    /// key, if any (used to detect duplicate submissions with a
    /// different payload).
    fn find_prior_proposal_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<MutationProposal>, PolicyError>;

    /// Current continuity generation for the individual, used to decide
    /// staleness independently of the transactional CAS re-check inside
    /// `activate()` (this is a pre-transaction, best-effort check; the
    /// authoritative check happens under `BEGIN IMMEDIATE`).
    fn current_generation(&self, individual_id: IndividualId) -> Result<u64, PolicyError>;
}

/// Deterministic accept/reject baseline. Not an LLM judge: every branch is
/// an explicit rule so behaviour is reviewable and testable (see plan
/// §8).
pub trait MutationPolicy {
    fn decide(
        &self,
        proposal: &MutationProposal,
        context: &dyn PolicyContext,
    ) -> Result<MutationDecision, PolicyError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_code_serializes_stably() {
        let json = serde_json::to_string(&ReasonCode::StalePredecessor).unwrap();
        assert_eq!(json, "\"STALE_PREDECESSOR\"");
    }
}
