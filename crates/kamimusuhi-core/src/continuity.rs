//! Continuity Kernel: which state is the authoritative continuation of the
//! individual.
//!
//! The canonical lineage of an individual is a chain of commits. Exactly one
//! commit is the *head*; its `generation` increases by one per accepted
//! mutation. A proposal names the head it was built against
//! ([`ExpectedHead`]); if the head moved in the meantime the proposal is
//! rejected as `STALE_PREDECESSOR` and never silently rebased.
//!
//! Writer fencing (single-writer phase): each individual has a current
//! [`WriterEpoch`]. A runtime claims a new epoch when it takes over the
//! individual; proposals carrying an older epoch are rejected even if their
//! expected head is current. This is the v0.1 stand-in for node fencing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::audit::AuditEvent;
use crate::ids::{BootId, CommitId, IndividualId, NodeId, ProposalId, ReceiptId, SchemaVersion};
use crate::mutation::{
    Disposition, MutationDecision, MutationPolicy, MutationProposal, PolicyContext, PolicyError,
};
use crate::time::{Clock, UtcTimestamp};

/// Position in the lineage. Root is generation 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(pub u64);

impl Generation {
    pub const ROOT: Self = Self(0);

    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "g{}", self.0)
    }
}

/// Monotonic writer authority counter per individual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WriterEpoch(pub u64);

impl WriterEpoch {
    pub const INITIAL: Self = Self(1);

    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for WriterEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "epoch{}", self.0)
    }
}

/// The runtime instance that proposes a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WriterIdentity {
    pub node_id: NodeId,
    pub boot_id: BootId,
    pub writer_epoch: WriterEpoch,
}

/// One individual. `root_commit_id` is generation 0 and never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Individual {
    pub individual_id: IndividualId,
    pub root_commit_id: CommitId,
    pub created_at: UtcTimestamp,
}

/// One node in the canonical lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalCommit {
    pub commit_id: CommitId,
    pub individual_id: IndividualId,
    pub generation: Generation,
    /// `None` only for the root commit.
    pub predecessor_commit_id: Option<CommitId>,
    /// `None` only for the root commit.
    pub proposal_id: Option<ProposalId>,
    pub created_at: UtcTimestamp,
}

/// The authoritative current state of an individual plus its writer fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityHead {
    pub individual_id: IndividualId,
    pub commit_id: CommitId,
    pub generation: Generation,
    pub writer_epoch: WriterEpoch,
    pub updated_at: UtcTimestamp,
}

impl ContinuityHead {
    pub const fn expected(&self) -> ExpectedHead {
        ExpectedHead {
            commit_id: self.commit_id,
            generation: self.generation,
        }
    }
}

/// The head a proposal was built against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExpectedHead {
    pub commit_id: CommitId,
    pub generation: Generation,
}

/// Lineage receipt for an accepted, activated mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationReceipt {
    pub receipt_id: ReceiptId,
    pub individual_id: IndividualId,
    pub proposal_id: ProposalId,
    pub commit_id: CommitId,
    pub predecessor_commit_id: CommitId,
    pub generation: Generation,
    pub created_at: UtcTimestamp,
}

/// Result of submitting a proposal to the kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivationOutcome {
    /// The proposal advanced the head in this call.
    Activated(ActivationReceipt),
    /// The same proposal (by ID or idempotency key) was already activated;
    /// the existing receipt is returned and no second commit was created.
    AlreadyActivated(ActivationReceipt),
    /// The proposal did not move the head. The decision is durable.
    Rejected(MutationDecision),
}

impl ActivationOutcome {
    pub fn receipt(&self) -> Option<&ActivationReceipt> {
        match self {
            Self::Activated(r) | Self::AlreadyActivated(r) => Some(r),
            Self::Rejected(_) => None,
        }
    }

    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

/// Request to create a new individual with its root commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewIndividual {
    pub individual_id: IndividualId,
    pub root_commit_id: CommitId,
    pub node_id: NodeId,
    pub boot_id: BootId,
}

/// Everything created by a successful [`ContinuityStore::create_individual`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndividualBootstrap {
    pub individual: Individual,
    pub head: ContinuityHead,
    pub writer: WriterIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContinuityError {
    #[error("individual {0} is not present in this store")]
    IndividualNotFound(IndividualId),
    #[error("individual {0} already exists")]
    IndividualAlreadyExists(IndividualId),
    #[error("store schema {found} is not supported by this runtime (supports up to {supported})")]
    SchemaVersionMismatch {
        found: SchemaVersion,
        supported: SchemaVersion,
    },
    #[error("canonical state is inconsistent: {detail}")]
    Corrupt { detail: String },
    #[error("proposal is structurally invalid: {reason}")]
    InvalidProposal { reason: String },
    #[error("activate() requires an accepting decision, got {0:?}")]
    DecisionNotAccepting(Disposition),
    #[error("mutation policy failed: {0}")]
    Policy(#[from] PolicyError),
    /// Lock contention or busy timeout. Distinct from identity conflicts:
    /// callers may retry; they must not treat this as a stale predecessor.
    #[error("canonical store is busy: {detail}")]
    Contended { detail: String },
    /// The backend failed and the outcome of the operation is not known.
    /// Callers must not infer success.
    #[error("canonical store backend error: {message}")]
    Backend { message: String },
}

/// Durable canonical continuity storage.
///
/// Implementations own the atomic transaction. `activate` and
/// `record_rejection` must persist their effects (or nothing) atomically and
/// must never be called while a model/tool/network call is in flight.
pub trait ContinuityStore: Send + Sync {
    fn create_individual(
        &self,
        request: NewIndividual,
    ) -> Result<IndividualBootstrap, ContinuityError>;

    /// Load and validate the head. Never creates an individual on miss.
    fn load_head(&self, individual_id: IndividualId) -> Result<ContinuityHead, ContinuityError>;

    /// Take over writer authority for a (re)started runtime. Older epochs are
    /// fenced out of `activate` from this point on.
    fn claim_writer_epoch(
        &self,
        individual_id: IndividualId,
        node_id: NodeId,
        boot_id: BootId,
    ) -> Result<WriterIdentity, ContinuityError>;

    fn find_receipt(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<ActivationReceipt>, ContinuityError>;

    fn find_decision(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<MutationDecision>, ContinuityError>;

    /// Atomically apply an accepted proposal: re-check head and writer epoch,
    /// honour idempotency, append commit, move head, write receipt and audit.
    fn activate(
        &self,
        proposal: &MutationProposal,
        decision: &MutationDecision,
    ) -> Result<ActivationOutcome, ContinuityError>;

    /// Durably record a non-accepting decision without moving the head.
    fn record_rejection(
        &self,
        proposal: &MutationProposal,
        decision: &MutationDecision,
    ) -> Result<(), ContinuityError>;

    fn commits(&self, individual_id: IndividualId)
    -> Result<Vec<CanonicalCommit>, ContinuityError>;

    fn audit_events(&self, individual_id: IndividualId)
    -> Result<Vec<AuditEvent>, ContinuityError>;
}

/// Orchestrates proposal → policy → activation over a [`ContinuityStore`].
///
/// The kernel performs no model or network calls, so no canonical transaction
/// is ever held open across one.
pub struct ContinuityKernel<S, P, C> {
    store: S,
    policy: P,
    clock: C,
}

impl<S: ContinuityStore, P: MutationPolicy, C: Clock> ContinuityKernel<S, P, C> {
    pub fn new(store: S, policy: P, clock: C) -> Self {
        Self {
            store,
            policy,
            clock,
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn policy(&self) -> &P {
        &self.policy
    }

    /// Submit a proposal. Retrying the same proposal returns the earlier
    /// outcome instead of evaluating it again against a moved head.
    pub fn submit(
        &self,
        proposal: &MutationProposal,
    ) -> Result<ActivationOutcome, ContinuityError> {
        proposal
            .validate_structure()
            .map_err(|reason| ContinuityError::InvalidProposal { reason })?;

        if let Some(receipt) = self.store.find_receipt(proposal.proposal_id)? {
            return Ok(ActivationOutcome::AlreadyActivated(receipt));
        }
        if let Some(decision) = self.store.find_decision(proposal.proposal_id)? {
            return Ok(ActivationOutcome::Rejected(decision));
        }

        let current_head = self.store.load_head(proposal.individual_id)?;
        let context = PolicyContext {
            current_head,
            now: self.clock.now_utc(),
        };
        let decision = self.policy.decide(proposal, &context)?;
        if decision.disposition == Disposition::Accept {
            self.store.activate(proposal, &decision)
        } else {
            self.store.record_rejection(proposal, &decision)?;
            Ok(ActivationOutcome::Rejected(decision))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::ids::{EvidenceId, ReceiptId};
    use crate::mutation::{
        MutationDomain, MutationOperation, MutationPolicyV0, OriginClass, ReasonCode,
    };

    /// Minimal in-memory store that records which entry points the kernel used.
    #[derive(Default)]
    struct MockStore {
        head: Mutex<Option<ContinuityHead>>,
        receipts: Mutex<Vec<ActivationReceipt>>,
        decisions: Mutex<Vec<MutationDecision>>,
        activate_calls: Mutex<u32>,
    }

    impl ContinuityStore for MockStore {
        fn create_individual(
            &self,
            _: NewIndividual,
        ) -> Result<IndividualBootstrap, ContinuityError> {
            unimplemented!("not exercised by kernel tests")
        }

        fn load_head(
            &self,
            individual_id: IndividualId,
        ) -> Result<ContinuityHead, ContinuityError> {
            self.head
                .lock()
                .unwrap()
                .ok_or(ContinuityError::IndividualNotFound(individual_id))
        }

        fn claim_writer_epoch(
            &self,
            _: IndividualId,
            _: NodeId,
            _: BootId,
        ) -> Result<WriterIdentity, ContinuityError> {
            unimplemented!("not exercised by kernel tests")
        }

        fn find_receipt(
            &self,
            proposal_id: ProposalId,
        ) -> Result<Option<ActivationReceipt>, ContinuityError> {
            Ok(self
                .receipts
                .lock()
                .unwrap()
                .iter()
                .copied()
                .find(|r| r.proposal_id == proposal_id))
        }

        fn find_decision(
            &self,
            proposal_id: ProposalId,
        ) -> Result<Option<MutationDecision>, ContinuityError> {
            Ok(self
                .decisions
                .lock()
                .unwrap()
                .iter()
                .find(|d| d.proposal_id == proposal_id)
                .cloned())
        }

        fn activate(
            &self,
            proposal: &MutationProposal,
            decision: &MutationDecision,
        ) -> Result<ActivationOutcome, ContinuityError> {
            assert_eq!(decision.disposition, Disposition::Accept);
            *self.activate_calls.lock().unwrap() += 1;
            let mut head = self.head.lock().unwrap();
            let current = head.unwrap();
            let commit_id = CommitId::from_u128(u128::from(current.generation.0) + 1000);
            let receipt = ActivationReceipt {
                receipt_id: ReceiptId::from_u128(u128::from(current.generation.0) + 2000),
                individual_id: proposal.individual_id,
                proposal_id: proposal.proposal_id,
                commit_id,
                predecessor_commit_id: current.commit_id,
                generation: current.generation.next(),
                created_at: decision.decided_at,
            };
            *head = Some(ContinuityHead {
                commit_id,
                generation: receipt.generation,
                updated_at: decision.decided_at,
                ..current
            });
            self.receipts.lock().unwrap().push(receipt);
            self.decisions.lock().unwrap().push(decision.clone());
            Ok(ActivationOutcome::Activated(receipt))
        }

        fn record_rejection(
            &self,
            _: &MutationProposal,
            decision: &MutationDecision,
        ) -> Result<(), ContinuityError> {
            self.decisions.lock().unwrap().push(decision.clone());
            Ok(())
        }

        fn commits(&self, _: IndividualId) -> Result<Vec<CanonicalCommit>, ContinuityError> {
            Ok(Vec::new())
        }

        fn audit_events(&self, _: IndividualId) -> Result<Vec<AuditEvent>, ContinuityError> {
            Ok(Vec::new())
        }
    }

    struct TestClock;

    impl Clock for TestClock {
        fn now_utc(&self) -> UtcTimestamp {
            UtcTimestamp::from_unix_millis(42)
        }
    }

    fn root_head() -> ContinuityHead {
        ContinuityHead {
            individual_id: IndividualId::from_u128(1),
            commit_id: CommitId::from_u128(10),
            generation: Generation::ROOT,
            writer_epoch: WriterEpoch::INITIAL,
            updated_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    fn proposal(id: u128, expected: ExpectedHead) -> MutationProposal {
        MutationProposal {
            proposal_id: ProposalId::from_u128(id),
            individual_id: IndividualId::from_u128(1),
            domain: MutationDomain::Relationship,
            operation: MutationOperation::Fact,
            subject_key: Some("user-fixture".to_owned()),
            candidate: serde_json::json!({ "preference": "ほうじ茶" }),
            expected_head: expected,
            evidence_refs: vec![EvidenceId::from_u128(50)],
            origin_class: OriginClass::Reported,
            requested_by: WriterIdentity {
                node_id: NodeId::from_u128(7),
                boot_id: BootId::from_u128(8),
                writer_epoch: WriterEpoch::INITIAL,
            },
            policy_version: MutationPolicyV0::VERSION,
            idempotency_key: format!("idem-{id}"),
            created_at: UtcTimestamp::from_unix_millis(0),
        }
    }

    fn kernel() -> ContinuityKernel<MockStore, MutationPolicyV0, TestClock> {
        let store = MockStore::default();
        *store.head.lock().unwrap() = Some(root_head());
        ContinuityKernel::new(store, MutationPolicyV0, TestClock)
    }

    #[test]
    fn accepted_proposal_is_activated_once_and_retry_returns_same_receipt() {
        let kernel = kernel();
        let p = proposal(100, root_head().expected());

        let first = kernel.submit(&p).unwrap();
        let ActivationOutcome::Activated(receipt) = first else {
            panic!("expected activation, got {first:?}");
        };
        assert_eq!(receipt.generation, Generation(1));

        let second = kernel.submit(&p).unwrap();
        assert_eq!(second, ActivationOutcome::AlreadyActivated(receipt));
        assert_eq!(*kernel.store().activate_calls.lock().unwrap(), 1);
    }

    #[test]
    fn stale_proposal_is_rejected_durably_without_activate() {
        let kernel = kernel();
        let root = root_head().expected();
        kernel.submit(&proposal(100, root)).unwrap();

        // Built against generation 0 after the head moved to generation 1 (T15).
        let stale = proposal(101, root);
        let outcome = kernel.submit(&stale).unwrap();
        let ActivationOutcome::Rejected(decision) = &outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(decision.reason_code, ReasonCode::StalePredecessor);
        assert_eq!(*kernel.store().activate_calls.lock().unwrap(), 1);
        assert_eq!(
            kernel
                .store()
                .load_head(stale.individual_id)
                .unwrap()
                .generation,
            Generation(1)
        );

        // Retrying a rejected proposal returns the recorded decision.
        assert_eq!(kernel.submit(&stale).unwrap(), outcome);
    }

    #[test]
    fn structurally_invalid_proposal_is_an_error_not_a_decision() {
        let kernel = kernel();
        let mut p = proposal(100, root_head().expected());
        p.proposal_id = ProposalId::from_u128(0);
        assert!(matches!(
            kernel.submit(&p),
            Err(ContinuityError::InvalidProposal { .. })
        ));
        assert!(kernel.store().decisions.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_individual_is_not_created_implicitly() {
        let kernel = ContinuityKernel::new(MockStore::default(), MutationPolicyV0, TestClock);
        let p = proposal(100, root_head().expected());
        assert_eq!(
            kernel.submit(&p),
            Err(ContinuityError::IndividualNotFound(
                IndividualId::from_u128(1)
            ))
        );
    }
}
