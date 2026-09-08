//! In-memory `PolicyContext` fixture used by unit tests for
//! `MutationPolicyV0` implementations, without needing a real store.

use std::collections::HashMap;
use std::sync::Mutex;

use kamimusuhi_core::ids::{EvidenceId, IndividualId};
use kamimusuhi_core::mutation::{MutationProposal, PolicyContext, PolicyError};

#[derive(Default)]
pub struct FixturePolicyContext {
    evidence_owners: Mutex<HashMap<EvidenceId, IndividualId>>,
    prior_proposals: Mutex<HashMap<String, MutationProposal>>,
    generations: Mutex<HashMap<IndividualId, u64>>,
}

impl FixturePolicyContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_evidence(self, evidence_id: EvidenceId, owner: IndividualId) -> Self {
        self.evidence_owners
            .lock()
            .unwrap()
            .insert(evidence_id, owner);
        self
    }

    pub fn with_prior_proposal(self, idempotency_key: &str, proposal: MutationProposal) -> Self {
        self.prior_proposals
            .lock()
            .unwrap()
            .insert(idempotency_key.to_string(), proposal);
        self
    }

    pub fn with_generation(self, individual_id: IndividualId, generation: u64) -> Self {
        self.generations
            .lock()
            .unwrap()
            .insert(individual_id, generation);
        self
    }
}

impl PolicyContext for FixturePolicyContext {
    fn evidence_owned_by(
        &self,
        evidence_id: EvidenceId,
        individual_id: IndividualId,
    ) -> Result<bool, PolicyError> {
        Ok(self
            .evidence_owners
            .lock()
            .unwrap()
            .get(&evidence_id)
            .is_some_and(|owner| *owner == individual_id))
    }

    fn find_prior_proposal_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<MutationProposal>, PolicyError> {
        Ok(self
            .prior_proposals
            .lock()
            .unwrap()
            .get(idempotency_key)
            .cloned())
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
