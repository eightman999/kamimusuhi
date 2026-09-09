//! Runtime errors.
//!
//! The important one is what is *missing*: there is no variant meaning "the
//! runtime was unusable so a fresh individual was created". A runtime that
//! cannot restore its individual fails closed and says why. Minting a new
//! identity to recover from corruption would be the one unrecoverable bug in
//! a system whose whole purpose is continuity.

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::evidence::EvidenceError;
use kamimusuhi_core::ids::IndividualId;
use kamimusuhi_core::library::LibraryError;
use kamimusuhi_core::memory::MemoryError;
use kamimusuhi_core::persona::PersonaError;
use kamimusuhi_core::resources::{RegistryError, ResourceCallLogError};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime directory {path} could not be prepared: {message}")]
    DirectoryIo { path: String, message: String },
    #[error("runtime config {path} could not be read: {message}")]
    ConfigIo { path: String, message: String },
    #[error("runtime config {path} is malformed: {message}")]
    ConfigMalformed { path: String, message: String },
    #[error("runtime config version {found} is not supported by this build (supports {supported})")]
    ConfigVersion { found: u32, supported: u32 },
    #[error("runtime directory {path} is not initialized; run `init` first")]
    NotInitialized { path: String },
    #[error("runtime directory {path} is already initialized")]
    AlreadyInitialized { path: String },
    /// The database holds no individual. `init` creates one; every other
    /// command refuses, because "there is nobody here" is not something to
    /// paper over by inventing somebody.
    #[error("runtime at {path} holds no individual; it cannot be resumed")]
    NoIndividual { path: String },
    /// More than one individual in one runtime directory. Phase 1 is
    /// single-individual, so this is ambiguity, not a menu.
    #[error("runtime at {path} holds {count} individuals; phase 1 expects exactly one")]
    AmbiguousIndividual { path: String, count: usize },
    #[error("individual {0} could not be restored from the canonical store")]
    UnrestorableIndividual(IndividualId),
    #[error("resource slot {slot} is not configured")]
    SlotNotConfigured { slot: String },
    #[error("resource registry could not be built: {message}")]
    ResourceRegistry { message: String },
    #[error("cognitive resource call failed: {0}")]
    Resource(#[from] RegistryError),
    #[error(transparent)]
    ResourceCallLog(#[from] ResourceCallLogError),
    #[error(transparent)]
    Continuity(#[from] ContinuityError),
    #[error(transparent)]
    Evidence(#[from] EvidenceError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error(transparent)]
    Persona(#[from] PersonaError),
    #[error("mutation was not activated: {reason}")]
    MutationNotActivated { reason: String },
    #[error("{0}")]
    Usage(String),
}
