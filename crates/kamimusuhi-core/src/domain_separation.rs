//! Domain separation: which evidence may support which durable domain.
//!
//! This is the fail-closed boundary between "someone said something" and
//! "Kamimusuhi holds this about itself". The checks here are deliberately
//! *structural*: they read IDs, kinds, origins and lineage, never payload
//! text. Payload text is the one thing an attacker fully controls, so it can
//! never be the thing that decides authority (audit A14, test T06).
//!
//! The rules:
//!
//! - every cited record must exist (`EVIDENCE_NOT_FOUND`);
//! - it must belong to the same individual (`EVIDENCE_OWNER_MISMATCH`);
//! - a relationship fact needs first-party testimony from the interaction,
//!   not Library text, resource output or a summary alone
//!   (`EVIDENCE_DOMAIN_MISMATCH`);
//! - an episodic capture needs a record of the interaction it claims to
//!   capture (`EVIDENCE_DOMAIN_MISMATCH`);
//! - nothing a user, a library or a resource said can support the self domain
//!   (`SELF_DOMAIN_CONTAMINATION`);
//! - a correction must name a live target in the same domain and about the
//!   same subject (`SUPERSEDE_TARGET_INVALID`);
//! - only a correction may name a supersession target (`MALFORMED_PROPOSAL`).

use crate::evidence::{EvidenceKind, EvidenceSnapshot};
use crate::memory::StateRecordFacts;
use crate::mutation::{MutationDomain, MutationOperation, MutationProposal, ReasonCode};

/// A refusal produced by a domain-separation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeparationViolation {
    pub reason_code: ReasonCode,
    pub detail: String,
}

impl SeparationViolation {
    fn new(reason_code: ReasonCode, detail: impl Into<String>) -> Self {
        Self {
            reason_code,
            detail: detail.into(),
        }
    }
}

/// What the check needs beyond the proposal itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeparationContext {
    pub evidence: EvidenceSnapshot,
    /// Facts about `proposal.supersedes`, if the store found it.
    pub supersedes_target: Option<StateRecordFacts>,
}

/// Fail-closed check of a proposal against its resolved evidence.
///
/// Structural validity (nil IDs, empty evidence list) is
/// [`MutationProposal::validate_structure`]'s job and is assumed to have run.
pub fn check_domain_separation(
    proposal: &MutationProposal,
    context: &SeparationContext,
) -> Result<(), SeparationViolation> {
    for evidence_id in &proposal.evidence_refs {
        let Some(facts) = context.evidence.get(*evidence_id) else {
            return Err(SeparationViolation::new(
                ReasonCode::EvidenceNotFound,
                format!("cited evidence {evidence_id} does not exist"),
            ));
        };
        if facts.individual_id != proposal.individual_id {
            return Err(SeparationViolation::new(
                ReasonCode::EvidenceOwnerMismatch,
                format!(
                    "evidence {evidence_id} belongs to {} but the proposal targets {}",
                    facts.individual_id, proposal.individual_id
                ),
            ));
        }
    }

    // The self domain is not fed from the outside. Even once self mutation is
    // implemented, a user utterance, a Library excerpt or a resource result is
    // never a statement about Kamimusuhi's own preferences.
    if proposal.domain == MutationDomain::SelfModel
        && let Some(foreign) = context.evidence.facts().iter().find(|f| {
            !matches!(
                f.kind,
                EvidenceKind::AgentUtterance | EvidenceKind::Reflection
            )
        })
    {
        return Err(SeparationViolation::new(
            ReasonCode::SelfDomainContamination,
            format!(
                "evidence {} of kind {} cannot support the self domain",
                foreign.evidence_id, foreign.kind
            ),
        ));
    }

    match proposal.domain {
        MutationDomain::Relationship => {
            if !context
                .evidence
                .facts()
                .iter()
                .any(|f| f.kind.is_first_party_testimony())
            {
                return Err(SeparationViolation::new(
                    ReasonCode::EvidenceDomainMismatch,
                    "a relationship fact needs first-party testimony; library text, \
                     resource output and derived summaries alone are not enough"
                        .to_owned(),
                ));
            }
        }
        MutationDomain::Episodic => {
            if !context
                .evidence
                .facts()
                .iter()
                .any(|f| f.kind.is_raw_capture())
            {
                return Err(SeparationViolation::new(
                    ReasonCode::EvidenceDomainMismatch,
                    "an episodic capture needs a raw record of the interaction it captures"
                        .to_owned(),
                ));
            }
        }
        MutationDomain::SelfModel => {}
    }

    match (proposal.operation, proposal.supersedes) {
        (MutationOperation::Correction, None) => {
            return Err(SeparationViolation::new(
                ReasonCode::SupersedeTargetInvalid,
                "a correction must name the state record it supersedes".to_owned(),
            ));
        }
        (MutationOperation::Correction, Some(target_id)) => {
            let Some(target) = &context.supersedes_target else {
                return Err(SeparationViolation::new(
                    ReasonCode::SupersedeTargetInvalid,
                    format!("supersession target {target_id} does not exist"),
                ));
            };
            if target.individual_id != proposal.individual_id {
                return Err(SeparationViolation::new(
                    ReasonCode::SupersedeTargetInvalid,
                    format!("supersession target {target_id} belongs to another individual"),
                ));
            }
            if target.domain != proposal.domain {
                return Err(SeparationViolation::new(
                    ReasonCode::SupersedeTargetInvalid,
                    format!(
                        "supersession target {target_id} is in domain {} but the correction is in {}",
                        target.domain, proposal.domain
                    ),
                ));
            }
            if target.subject_key != proposal.subject_key {
                return Err(SeparationViolation::new(
                    ReasonCode::SupersedeTargetInvalid,
                    format!(
                        "supersession target {target_id} is about {:?} but the correction is about {:?}",
                        target.subject_key, proposal.subject_key
                    ),
                ));
            }
            if !target.lifecycle_state.is_current() {
                return Err(SeparationViolation::new(
                    ReasonCode::SupersedeTargetInvalid,
                    format!(
                        "supersession target {target_id} is already {}",
                        target.lifecycle_state
                    ),
                ));
            }
        }
        (_, Some(target_id)) => {
            return Err(SeparationViolation::new(
                ReasonCode::MalformedProposal,
                format!(
                    "{} may not supersede {target_id}; only a correction supersedes",
                    proposal.operation
                ),
            ));
        }
        (_, None) => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::continuity::{ExpectedHead, Generation, WriterEpoch, WriterIdentity};
    use crate::evidence::EvidenceFacts;
    use crate::ids::{
        BootId, CommitId, EvidenceId, IndividualId, MemoryId, NodeId, PolicyVersion, ProposalId,
    };
    use crate::memory::LifecycleState;
    use crate::mutation::OriginClass;
    use crate::time::UtcTimestamp;

    const INDIVIDUAL: IndividualId = IndividualId::from_u128(1);
    const OTHER_INDIVIDUAL: IndividualId = IndividualId::from_u128(2);
    const UTTERANCE: EvidenceId = EvidenceId::from_u128(50);
    const TARGET: MemoryId = MemoryId::from_u128(70);

    fn facts(kind: EvidenceKind, owner: IndividualId) -> EvidenceFacts {
        EvidenceFacts::standalone(UTTERANCE, owner, kind, OriginClass::Reported)
    }

    fn context(facts: Vec<EvidenceFacts>) -> SeparationContext {
        SeparationContext {
            evidence: EvidenceSnapshot::new(facts),
            supersedes_target: None,
        }
    }

    fn proposal() -> MutationProposal {
        MutationProposal {
            proposal_id: ProposalId::from_u128(100),
            individual_id: INDIVIDUAL,
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
            policy_version: PolicyVersion(0),
            idempotency_key: "turn-1/draft-0".to_owned(),
            created_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    fn target(domain: MutationDomain, lifecycle: LifecycleState) -> StateRecordFacts {
        StateRecordFacts {
            state_record_id: TARGET,
            individual_id: INDIVIDUAL,
            domain,
            subject_key: Some("user-fixture".to_owned()),
            lifecycle_state: lifecycle,
        }
    }

    #[test]
    fn user_utterance_supports_a_relationship_fact() {
        let ctx = context(vec![facts(EvidenceKind::UserUtterance, INDIVIDUAL)]);
        assert_eq!(check_domain_separation(&proposal(), &ctx), Ok(()));
    }

    #[test]
    fn missing_evidence_is_rejected() {
        let violation =
            check_domain_separation(&proposal(), &context(Vec::new())).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::EvidenceNotFound);
    }

    #[test]
    fn evidence_of_another_individual_cannot_support_this_one() {
        let ctx = context(vec![facts(EvidenceKind::UserUtterance, OTHER_INDIVIDUAL)]);
        let violation = check_domain_separation(&proposal(), &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::EvidenceOwnerMismatch);
    }

    #[test]
    fn library_resource_and_derived_evidence_cannot_alone_make_a_relationship_fact() {
        for kind in [
            EvidenceKind::LibraryExcerpt,
            EvidenceKind::ResourceResult,
            EvidenceKind::Summary,
            EvidenceKind::Reflection,
            EvidenceKind::AgentUtterance,
        ] {
            let ctx = context(vec![facts(kind, INDIVIDUAL)]);
            let violation = check_domain_separation(&proposal(), &ctx).expect_err("must reject");
            assert_eq!(
                violation.reason_code,
                ReasonCode::EvidenceDomainMismatch,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_user_preference_cannot_be_recorded_as_a_self_preference() {
        let mut p = proposal();
        p.domain = MutationDomain::SelfModel;
        let ctx = context(vec![facts(EvidenceKind::UserUtterance, INDIVIDUAL)]);
        let violation = check_domain_separation(&p, &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::SelfDomainContamination);
    }

    #[test]
    fn library_text_cannot_be_recorded_as_a_self_preference() {
        let mut p = proposal();
        p.domain = MutationDomain::SelfModel;
        let ctx = context(vec![facts(EvidenceKind::LibraryExcerpt, INDIVIDUAL)]);
        let violation = check_domain_separation(&p, &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::SelfDomainContamination);
    }

    #[test]
    fn episodic_capture_needs_a_raw_record() {
        let mut p = proposal();
        p.domain = MutationDomain::Episodic;
        p.operation = MutationOperation::Capture;
        p.subject_key = None;
        let ctx = context(vec![facts(EvidenceKind::Summary, INDIVIDUAL)]);
        let violation = check_domain_separation(&p, &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::EvidenceDomainMismatch);

        let ctx = context(vec![facts(EvidenceKind::UserUtterance, INDIVIDUAL)]);
        assert_eq!(check_domain_separation(&p, &ctx), Ok(()));
    }

    #[test]
    fn correction_requires_a_live_target_in_the_same_domain_and_subject() {
        let mut p = proposal();
        p.operation = MutationOperation::Correction;
        let evidence = vec![facts(EvidenceKind::UserUtterance, INDIVIDUAL)];

        // No target named.
        let violation = check_domain_separation(&p, &context(evidence.clone()))
            .expect_err("correction without a target must be rejected");
        assert_eq!(violation.reason_code, ReasonCode::SupersedeTargetInvalid);

        p.supersedes = Some(TARGET);
        let mut ctx = context(evidence);

        // Named but not found.
        let violation = check_domain_separation(&p, &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::SupersedeTargetInvalid);

        // Wrong domain.
        ctx.supersedes_target = Some(target(MutationDomain::Episodic, LifecycleState::Active));
        assert_eq!(
            check_domain_separation(&p, &ctx)
                .expect_err("must reject")
                .reason_code,
            ReasonCode::SupersedeTargetInvalid
        );

        // Already superseded.
        ctx.supersedes_target = Some(target(
            MutationDomain::Relationship,
            LifecycleState::Superseded,
        ));
        assert_eq!(
            check_domain_separation(&p, &ctx)
                .expect_err("must reject")
                .reason_code,
            ReasonCode::SupersedeTargetInvalid
        );

        // Different subject.
        let mut other_subject = target(MutationDomain::Relationship, LifecycleState::Active);
        other_subject.subject_key = Some("someone-else".to_owned());
        ctx.supersedes_target = Some(other_subject);
        assert_eq!(
            check_domain_separation(&p, &ctx)
                .expect_err("must reject")
                .reason_code,
            ReasonCode::SupersedeTargetInvalid
        );

        // Live, same domain, same subject.
        ctx.supersedes_target = Some(target(MutationDomain::Relationship, LifecycleState::Active));
        assert_eq!(check_domain_separation(&p, &ctx), Ok(()));
    }

    #[test]
    fn only_a_correction_may_supersede() {
        let mut p = proposal();
        p.supersedes = Some(TARGET);
        let ctx = context(vec![facts(EvidenceKind::UserUtterance, INDIVIDUAL)]);
        let violation = check_domain_separation(&p, &ctx).expect_err("must reject");
        assert_eq!(violation.reason_code, ReasonCode::MalformedProposal);
    }
}
