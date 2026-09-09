//! Persona Core boundary.
//!
//! The Persona Core is the replaceable cognitive component that turns the
//! current input and workspace into a surface response and *drafts* of
//! canonical changes. It never owns identity: drafts carry evidence references
//! but no expected head, writer authority or policy version — the runtime
//! attaches those when it turns a draft into a `MutationProposal`, and the
//! Continuity Kernel decides whether anything is activated.

use serde::{Deserialize, Serialize};

use crate::ids::{EvidenceId, IndividualId, MemoryId, SessionId, TurnId};
use crate::mutation::{MutationDomain, MutationOperation, OriginClass};
use crate::workspace::Workspace;

/// Correlation IDs for one cognitive turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnContext {
    pub individual_id: IndividualId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
}

/// The current user input, already recorded as canonical evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInput {
    /// Evidence record holding the raw utterance. Drafts must cite this ID,
    /// never a paraphrase produced by the Persona Core itself.
    pub evidence_id: EvidenceId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaTurnInput {
    pub context: TurnContext,
    pub input: CurrentInput,
    /// Everything else the runtime brought to this turn, still typed and
    /// attributed. The Persona Core reads domains and source refs from it; it
    /// must never have to parse text to learn where something came from, and
    /// nothing in it grants authority to propose (see
    /// [`crate::workspace::AuthorityClass`]).
    #[serde(default)]
    pub workspace: Option<Workspace>,
}

/// A candidate canonical change drafted by the Persona Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDraft {
    pub domain: MutationDomain,
    pub operation: MutationOperation,
    /// Stable key of the subject (e.g. a user handle) for relationship facts.
    pub subject_key: Option<String>,
    pub candidate: serde_json::Value,
    pub evidence_refs: Vec<EvidenceId>,
    /// Set only for a correction: the durable record this draft replaces.
    /// The Persona Core may point at a record it retrieved, but it cannot
    /// decide that the replacement happens.
    #[serde(default)]
    pub supersedes: Option<MemoryId>,
    pub origin_class: OriginClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaTurnResult {
    pub context: TurnContext,
    /// What to say back. Surface planning/speech is a later organ.
    pub response_intent: String,
    pub proposals: Vec<ProposalDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersonaError {
    #[error("persona core rejected input: {reason}")]
    InvalidInput { reason: String },
    #[error("persona core backend failure: {message}")]
    Backend { message: String },
}

pub trait PersonaCore: Send + Sync {
    fn turn(&self, input: PersonaTurnInput) -> Result<PersonaTurnResult, PersonaError>;
}
