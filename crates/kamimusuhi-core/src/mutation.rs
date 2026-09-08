//! Vocabulary of guarded canonical mutations.
//!
//! Nothing in this module writes state. A Persona Core or worker only ever
//! produces a proposal; the Continuity Kernel decides whether it is activated.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Durable state domain a proposal targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationDomain {
    /// What happened: captured interaction episodes.
    Episodic,
    /// Facts about other people/agents, e.g. the user's stated preferences.
    Relationship,
    /// Kamimusuhi's own self-model. Reserved in phase 1; no operation accepts it yet.
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
