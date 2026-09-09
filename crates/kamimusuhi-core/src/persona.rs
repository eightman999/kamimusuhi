//! Persona Core boundary.
//!
//! The Persona Core is the replaceable cognitive component that turns the
//! current input and workspace into a surface response and *drafts* of
//! canonical changes. It never owns identity: drafts carry evidence references
//! but no expected head, writer authority or policy version — the runtime
//! attaches those when it turns a draft into a `MutationProposal`, and the
//! Continuity Kernel decides whether anything is activated.
//!
//! Two boundaries this module exists to hold:
//!
//! **The user-facing expression comes from here.** Material produced by a
//! cognitive resource is attributed material that arrives *as input* to a
//! Persona turn. There is no path that returns it to the user directly. If
//! there were, the Persona Core would be decoration and the individual's voice
//! would be whichever endpoint was configured last.
//!
//! **A Persona backend has no more authority than any other drafter.** What it
//! produces is a [`ProposalDraft`], judged by the same policy as anything
//! else. "The model said so" is not a route into durable state.

use serde::{Deserialize, Serialize};

use crate::ids::{EvidenceId, IndividualId, MemoryId, PersonaBackendId, SessionId, TurnId};
use crate::mutation::{MutationDomain, MutationOperation, OriginClass};
use crate::workspace::{Workspace, WorkspaceDomain, WorkspaceItem};

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

/// Non-durable state of the session this turn belongs to.
///
/// Working state, not memory: it describes the conversation as a running
/// process and is never written to canonical storage. `resumed` is the one
/// field a backend might reasonably behave differently on, and it says the
/// session began after a restart — not that anything was remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionWorkingState {
    pub turn_sequence: u64,
    /// This session was opened by a process that resumed an existing
    /// individual rather than creating one.
    pub resumed: bool,
    /// Resource calls made while preparing this turn.
    pub delegations: u32,
}

/// What the Persona Core is given, in sections.
///
/// The workspace already keeps every item typed and attributed; the envelope
/// groups those items by the distinction a backend actually has to respect, so
/// that "this is the user's own state" and "this is text someone else wrote"
/// are separate fields rather than a flag to be checked.
///
/// The representation stays typed all the way here. Flattening happens at the
/// serialization step inside a backend, which is the only place the
/// distinction could be lost and therefore the only place worth guarding.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PersonaEnvelope {
    /// Lineage position, as orientation.
    pub continuity: Vec<WorkspaceItem>,
    /// The individual's own model of itself. Empty in phase 1: the self domain
    /// is reserved and nothing can write it. The section exists so a backend
    /// never has to infer self-state from relationship state.
    pub durable_self: Vec<WorkspaceItem>,
    /// What the individual holds about other people.
    pub relationship: Vec<WorkspaceItem>,
    pub episodic: Vec<WorkspaceItem>,
    /// Imported documents. External material: not the individual's belief.
    pub library: Vec<WorkspaceItem>,
    /// Output of delegated cognition. External material, and specifically not
    /// the individual speaking.
    pub external_results: Vec<WorkspaceItem>,
    pub session: SessionWorkingState,
}

impl PersonaEnvelope {
    /// Group an assembled workspace into sections.
    ///
    /// Routing is by [`WorkspaceDomain`] alone — no item's content is read,
    /// because content is exactly what an attacker controls.
    pub fn from_workspace(workspace: &Workspace, session: SessionWorkingState) -> Self {
        let of = |domain: WorkspaceDomain| -> Vec<WorkspaceItem> {
            workspace
                .items
                .iter()
                .filter(|item| item.domain == domain)
                .cloned()
                .collect()
        };
        Self {
            continuity: of(WorkspaceDomain::CurrentContinuityState),
            durable_self: Vec::new(),
            relationship: of(WorkspaceDomain::RelationshipMemory),
            episodic: of(WorkspaceDomain::EpisodicMemory),
            library: of(WorkspaceDomain::LibraryEvidence),
            external_results: of(WorkspaceDomain::ExternalResourceResult),
            session,
        }
    }

    /// Items that came from outside the individual, in a stable order.
    ///
    /// A backend that wants to mark borrowed material asks for this rather
    /// than guessing from the text.
    pub fn external_material(&self) -> Vec<&WorkspaceItem> {
        self.library
            .iter()
            .chain(self.external_results.iter())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.continuity.is_empty()
            && self.durable_self.is_empty()
            && self.relationship.is_empty()
            && self.episodic.is_empty()
            && self.library.is_empty()
            && self.external_results.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaTurnInput {
    pub context: TurnContext,
    pub input: CurrentInput,
    /// Everything else the runtime brought to this turn, still typed and
    /// sectioned. Nothing in it grants authority to propose (see
    /// [`crate::workspace::AuthorityClass`]).
    #[serde(default)]
    pub envelope: PersonaEnvelope,
}

impl PersonaTurnInput {
    /// A turn with nothing but the current input. For paths that draft without
    /// context — and for tests.
    pub fn bare(context: TurnContext, input: CurrentInput) -> Self {
        Self {
            context,
            input,
            envelope: PersonaEnvelope::default(),
        }
    }

    pub fn with_workspace(
        context: TurnContext,
        input: CurrentInput,
        workspace: &Workspace,
        session: SessionWorkingState,
    ) -> Self {
        Self {
            context,
            input,
            envelope: PersonaEnvelope::from_workspace(workspace, session),
        }
    }
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

/// Which Persona Core produced a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaBackendDescriptor {
    pub backend_id: PersonaBackendId,
    /// Implementation family, e.g. `fixture` or `openai-compatible`. Never a
    /// credential or an endpoint.
    pub kind: String,
    /// Human-readable name for operators. Not an identity.
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaTurnResult {
    pub context: TurnContext,
    /// The backend that produced this. Set by the Persona Core itself, so the
    /// final expression can always be traced to what generated it.
    pub backend: PersonaBackendDescriptor,
    /// What to say back. Surface planning/speech is a later organ.
    ///
    /// This is the *only* thing that becomes a user-facing expression. No
    /// caller may substitute material that arrived in the envelope.
    pub response_intent: String,
    pub proposals: Vec<ProposalDraft>,
}

/// How a Persona turn failed.
///
/// Classified rather than transcribed, for the same reason as
/// [`crate::resources::ResourceError`]: an operator needs the class, and a
/// backend's prose or a prompt echoed into an error is a leak.
///
/// None of these is ever a reason to change canonical state. A Persona that
/// could not generate has produced nothing, and producing nothing is not an
/// event the individual's lineage records.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersonaError {
    #[error("persona core rejected input: {reason}")]
    InvalidInput { reason: String },
    #[error("persona backend {backend_id} timed out after {elapsed_ms}ms")]
    Timeout {
        backend_id: PersonaBackendId,
        elapsed_ms: u64,
    },
    #[error("persona backend {backend_id} transport failure: {message}")]
    Transport {
        backend_id: PersonaBackendId,
        message: String,
    },
    #[error("persona backend {backend_id} TLS failure ({kind}): {detail}")]
    Tls {
        backend_id: PersonaBackendId,
        kind: String,
        detail: String,
    },
    #[error("persona backend {backend_id} rejected authentication (status {status})")]
    Authentication {
        backend_id: PersonaBackendId,
        status: u16,
    },
    #[error("persona backend {backend_id} rate limited (status {status})")]
    RateLimited {
        backend_id: PersonaBackendId,
        status: u16,
    },
    #[error("persona backend {backend_id} returned status {status}")]
    HttpStatus {
        backend_id: PersonaBackendId,
        status: u16,
    },
    #[error("persona backend {backend_id} returned an unreadable response: {detail}")]
    MalformedResponse {
        backend_id: PersonaBackendId,
        detail: String,
    },
    #[error("persona backend {backend_id} reported error {code}")]
    ProviderError {
        backend_id: PersonaBackendId,
        code: String,
    },
    #[error("persona core backend failure: {message}")]
    Backend { message: String },
}

impl PersonaError {
    /// Stable code for the trace and for tests.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "INVALID_INPUT",
            Self::Timeout { .. } => "TIMEOUT",
            Self::Transport { .. } => "TRANSPORT",
            Self::Tls { .. } => "TLS",
            Self::Authentication { .. } => "AUTHENTICATION",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::HttpStatus { .. } => "HTTP_STATUS",
            Self::MalformedResponse { .. } => "MALFORMED_RESPONSE",
            Self::ProviderError { .. } => "PROVIDER_ERROR",
            Self::Backend { .. } => "BACKEND",
        }
    }
}

/// The replaceable component that speaks as the individual.
///
/// Swapping one implementation for another changes how Kamimusuhi expresses
/// itself. It does not change who Kamimusuhi is: identity lives in the
/// canonical store, and nothing here can reach it.
pub trait PersonaCore: Send + Sync {
    /// Which backend this is. Reported alongside every turn so a final
    /// expression can always be attributed to what generated it.
    fn descriptor(&self) -> PersonaBackendDescriptor;

    fn turn(&self, input: PersonaTurnInput) -> Result<PersonaTurnResult, PersonaError>;
}
