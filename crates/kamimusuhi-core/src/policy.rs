//! `MutationPolicyV0`: the deterministic accept/reject baseline described
//! in plan §8. This is not an LLM judge — every branch is an explicit,
//! reviewable rule.
//!
//! This wave implements the rules that are checkable from the proposal
//! itself plus the minimal [`crate::mutation::PolicyContext`] abstraction
//! (evidence ownership, prior-proposal lookup, current generation).
//! Semantic contradiction detection and full Library/state cross-checks
//! are out of scope for W1 and are left to later waves (plan §8, final
//! paragraph).

use crate::mutation::{
    Disposition, MutationDecision, MutationDomain, MutationOperation, MutationPolicy,
    MutationProposal, OriginClass, PolicyContext, PolicyError, ReasonCode,
};
use crate::time::{UtcTimestamp, WallClock};

pub struct MutationPolicyV0<'clock> {
    clock: &'clock dyn WallClock,
}

impl<'clock> MutationPolicyV0<'clock> {
    pub fn new(clock: &'clock dyn WallClock) -> Self {
        MutationPolicyV0 { clock }
    }

    fn decision(
        &self,
        proposal: &MutationProposal,
        disposition: Disposition,
        reason_code: ReasonCode,
    ) -> MutationDecision {
        MutationDecision {
            proposal_id: proposal.proposal_id,
            disposition,
            reason_code,
            decided_at: self.now(),
        }
    }

    fn now(&self) -> UtcTimestamp {
        self.clock.now_utc()
    }
}

impl MutationPolicy for MutationPolicyV0<'_> {
    fn decide(
        &self,
        proposal: &MutationProposal,
        context: &dyn PolicyContext,
    ) -> Result<MutationDecision, PolicyError> {
        // 1. unsupported domain / operation.
        if matches!(
            proposal.domain,
            MutationDomain::Unsupported | MutationDomain::SelfDomain
        ) {
            return Ok(self.decision(proposal, Disposition::Reject, ReasonCode::UnsupportedDomain));
        }
        if matches!(proposal.operation, MutationOperation::Unsupported) {
            return Ok(self.decision(
                proposal,
                Disposition::Reject,
                ReasonCode::UnsupportedOperation,
            ));
        }

        // 2. contamination checks derivable from the proposal's own
        // origin class, without needing the evidence store.
        let requires_canonical_grounding = matches!(
            proposal.domain,
            MutationDomain::Episodic | MutationDomain::Relationship
        );
        if requires_canonical_grounding {
            match proposal.origin_class {
                OriginClass::Simulation => {
                    return Ok(self.decision(
                        proposal,
                        Disposition::Reject,
                        ReasonCode::SimulationOriginNotExternalEvent,
                    ));
                }
                OriginClass::LibraryEvidence => {
                    return Ok(self.decision(
                        proposal,
                        Disposition::Reject,
                        ReasonCode::LibraryOnlyContamination,
                    ));
                }
                OriginClass::ExternalResourceResult => {
                    return Ok(self.decision(
                        proposal,
                        Disposition::Reject,
                        ReasonCode::ExternalResourceOnlyContamination,
                    ));
                }
                OriginClass::CanonicalInteraction | OriginClass::Operator => {}
            }
        }

        // 3. evidence ref presence.
        if proposal.evidence_refs.is_empty() {
            return Ok(self.decision(proposal, Disposition::Reject, ReasonCode::MissingEvidence));
        }

        // 4. evidence ownership: every referenced evidence record must
        // belong to the proposal's individual.
        for evidence_id in &proposal.evidence_refs {
            let owned = context.evidence_owned_by(*evidence_id, proposal.individual_id)?;
            if !owned {
                return Ok(self.decision(
                    proposal,
                    Disposition::Reject,
                    ReasonCode::EvidenceOwnerMismatch,
                ));
            }
        }

        // 5. duplicate proposal with a different payload.
        if let Some(prior) =
            context.find_prior_proposal_by_idempotency_key(&proposal.idempotency_key)?
        {
            if prior.candidate != proposal.candidate
                || prior.domain != proposal.domain
                || prior.operation != proposal.operation
            {
                return Ok(self.decision(
                    proposal,
                    Disposition::Reject,
                    ReasonCode::DuplicateProposalPayloadMismatch,
                ));
            }
        }

        // 6. stale predecessor (best-effort pre-check; `activate()` still
        // performs the authoritative check under `BEGIN IMMEDIATE`).
        let current_generation = context.current_generation(proposal.individual_id)?;
        if current_generation != proposal.expected_generation {
            return Ok(self.decision(proposal, Disposition::Reject, ReasonCode::StalePredecessor));
        }

        Ok(self.decision(proposal, Disposition::Accept, ReasonCode::Accepted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EvidenceId, Id128, IdGenerator, IndividualId, PolicyVersion, ProposalId};
    use crate::mutation::MutationDomain;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    struct SeqGen(AtomicU64);
    impl IdGenerator for SeqGen {
        fn next_id(&self) -> Id128 {
            Id128::from_u128(self.0.fetch_add(1, Ordering::SeqCst) as u128)
        }
    }

    struct FixedClock;
    impl WallClock for FixedClock {
        fn now_utc(&self) -> UtcTimestamp {
            UtcTimestamp::parse_rfc3339("2026-09-08T00:00:00Z").unwrap()
        }
    }

    #[derive(Default)]
    struct TestContext {
        owners: Mutex<HashMap<EvidenceId, IndividualId>>,
        priors: Mutex<HashMap<String, MutationProposal>>,
        generations: Mutex<HashMap<IndividualId, u64>>,
    }

    impl PolicyContext for TestContext {
        fn evidence_owned_by(
            &self,
            evidence_id: EvidenceId,
            individual_id: IndividualId,
        ) -> Result<bool, PolicyError> {
            Ok(self
                .owners
                .lock()
                .unwrap()
                .get(&evidence_id)
                .is_some_and(|o| *o == individual_id))
        }

        fn find_prior_proposal_by_idempotency_key(
            &self,
            key: &str,
        ) -> Result<Option<MutationProposal>, PolicyError> {
            Ok(self.priors.lock().unwrap().get(key).cloned())
        }

        fn current_generation(&self, individual_id: IndividualId) -> Result<u64, PolicyError> {
            Ok(self
                .generations
                .lock()
                .unwrap()
                .get(&individual_id)
                .copied()
                .unwrap_or(0))
        }
    }

    fn base_proposal(
        gen: &SeqGen,
        individual_id: IndividualId,
        evidence_id: EvidenceId,
    ) -> MutationProposal {
        MutationProposal {
            proposal_id: ProposalId::new(gen),
            individual_id,
            domain: MutationDomain::Relationship,
            operation: MutationOperation::Fact,
            candidate: serde_json::json!({"preference": "hoji-cha"}),
            evidence_refs: vec![evidence_id],
            expected_commit_id: None,
            expected_generation: 0,
            origin_class: OriginClass::CanonicalInteraction,
            requested_by: "fake-persona-core".to_string(),
            policy_version: PolicyVersion(0),
            idempotency_key: "fixture-key-1".to_string(),
            created_at: UtcTimestamp::parse_rfc3339("2026-09-08T00:00:00Z").unwrap(),
        }
    }

    #[test]
    fn accepts_well_formed_relationship_fact() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let proposal = base_proposal(&gen, individual_id, evidence_id);
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Accept);
        assert_eq!(decision.reason_code, ReasonCode::Accepted);
    }

    #[test]
    fn rejects_missing_evidence() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();

        let mut proposal = base_proposal(&gen, individual_id, evidence_id);
        proposal.evidence_refs.clear();
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::MissingEvidence);
    }

    #[test]
    fn rejects_evidence_owner_mismatch() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let other_individual = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, other_individual);

        let proposal = base_proposal(&gen, individual_id, evidence_id);
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::EvidenceOwnerMismatch);
    }

    #[test]
    fn rejects_unsupported_domain() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let mut proposal = base_proposal(&gen, individual_id, evidence_id);
        proposal.domain = MutationDomain::SelfDomain;
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::UnsupportedDomain);
    }

    #[test]
    fn rejects_duplicate_proposal_with_different_payload() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let mut prior = base_proposal(&gen, individual_id, evidence_id);
        prior.candidate = serde_json::json!({"preference": "sencha"});
        ctx.priors
            .lock()
            .unwrap()
            .insert(prior.idempotency_key.clone(), prior);

        let proposal = base_proposal(&gen, individual_id, evidence_id);
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(
            decision.reason_code,
            ReasonCode::DuplicateProposalPayloadMismatch
        );
    }

    #[test]
    fn rejects_stale_predecessor() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);
        ctx.generations.lock().unwrap().insert(individual_id, 1);

        let proposal = base_proposal(&gen, individual_id, evidence_id);
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::StalePredecessor);
    }

    #[test]
    fn rejects_library_only_contamination() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let mut proposal = base_proposal(&gen, individual_id, evidence_id);
        proposal.origin_class = OriginClass::LibraryEvidence;
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(decision.reason_code, ReasonCode::LibraryOnlyContamination);
    }

    #[test]
    fn rejects_external_resource_only_contamination() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let mut proposal = base_proposal(&gen, individual_id, evidence_id);
        proposal.origin_class = OriginClass::ExternalResourceResult;
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(
            decision.reason_code,
            ReasonCode::ExternalResourceOnlyContamination
        );
    }

    #[test]
    fn rejects_simulation_origin_as_canonical_event() {
        let gen = SeqGen(AtomicU64::new(1));
        let clock = FixedClock;
        let policy = MutationPolicyV0::new(&clock);
        let individual_id = IndividualId::new(&gen);
        let evidence_id = EvidenceId::new(&gen);
        let ctx = TestContext::default();
        ctx.owners
            .lock()
            .unwrap()
            .insert(evidence_id, individual_id);

        let mut proposal = base_proposal(&gen, individual_id, evidence_id);
        proposal.origin_class = OriginClass::Simulation;
        let decision = policy.decide(&proposal, &ctx).unwrap();
        assert_eq!(decision.disposition, Disposition::Reject);
        assert_eq!(
            decision.reason_code,
            ReasonCode::SimulationOriginNotExternalEvent
        );
    }
}
