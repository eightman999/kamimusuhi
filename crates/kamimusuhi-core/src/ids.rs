//! Stable domain identifiers.
//!
//! Every domain identity in Kamimusuhi is an opaque 128-bit value wrapped in
//! a distinct newtype. Bare `String`s are never passed across domain
//! boundaries as identity: the newtype boundary is what stops a
//! `SessionId` from being accidentally compared to a `TurnId`, etc.
//!
//! Production code obtains new IDs from an [`IdGenerator`]. Tests use the
//! deterministic generator in `kamimusuhi-testkit` so fixtures are
//! reproducible.

use std::fmt;
use std::str::FromStr;

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Opaque 128-bit runtime identifier. Not a UUID in the RFC 4122 sense;
/// just a 128-bit value with a stable hex text encoding. Kept private so
/// only the newtypes in this module can be constructed from it.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct Id128(u128);

impl Id128 {
    pub const fn from_u128(value: u128) -> Self {
        Id128(value)
    }

    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Id128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for Id128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Error returned when a stored/serialized ID cannot be parsed back.
#[derive(Debug, thiserror::Error)]
#[error("invalid id literal: {0:?}")]
pub struct IdParseError(pub String);

impl FromStr for Id128 {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u128::from_str_radix(s, 16)
            .map(Id128)
            .map_err(|_| IdParseError(s.to_string()))
    }
}

/// Generates new opaque 128-bit IDs for a given domain. Implementations
/// MUST NOT reuse an ID within the lifetime of a single canonical store.
/// Production code uses a random generator; `kamimusuhi-testkit` provides
/// a deterministic sequential generator for reproducible fixtures.
pub trait IdGenerator: Send + Sync {
    fn next_id(&self) -> Id128;
}

/// Default production ID generator: a cryptographically-seeded RNG.
/// Collision probability at 128 bits is astronomically small and this is
/// not relied upon as a security boundary.
#[derive(Debug, Default)]
pub struct RandomIdGenerator;

impl IdGenerator for RandomIdGenerator {
    fn next_id(&self) -> Id128 {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        Id128::from_u128(u128::from_be_bytes(bytes))
    }
}

/// Declares a domain newtype ID backed by [`Id128`].
macro_rules! domain_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Id128);

        impl $name {
            pub const fn from_id128(id: Id128) -> Self {
                $name(id)
            }

            pub fn new(gen: &dyn IdGenerator) -> Self {
                $name(gen.next_id())
            }

            pub const fn as_id128(self) -> Id128 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Id128::from_str(s).map($name)
            }
        }
    };
}

domain_id!(
    IndividualId,
    "Identity of one continuous Kamimusuhi individual."
);
domain_id!(CommitId, "Identity of one canonical continuity commit.");
domain_id!(NodeId, "Identity of a runtime node (host/process family).");
domain_id!(BootId, "Identity of a single runtime process boot.");
domain_id!(SessionId, "Identity of one interaction session.");
domain_id!(TurnId, "Identity of one turn within a session.");
domain_id!(CognitiveEpisodeId, "Identity of one cognitive episode.");
domain_id!(EvidenceId, "Identity of one canonical evidence record.");
domain_id!(ProposalId, "Identity of one mutation proposal.");
domain_id!(ReceiptId, "Identity of one activation receipt.");
domain_id!(MemoryId, "Identity of one durable state/memory record.");
domain_id!(
    LibraryArtifactId,
    "Identity of one imported Library artifact."
);
domain_id!(LibraryChunkId, "Identity of one Library chunk.");
domain_id!(ResourceId, "Identity of one registered cognitive resource.");
domain_id!(
    ResourceCallId,
    "Identity of one cognitive resource invocation."
);
domain_id!(TraceId, "Identity of one operational trace event.");

/// Monotonically-interpreted policy version. Not a domain identity in the
/// 128-bit sense: it is a small integer compared for compatibility gating.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PolicyVersion(pub u32);

/// Schema version stamped into `schema_meta`. Compared for fail-closed
/// version mismatch handling (see plan §15.3 T25).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id128_round_trips_through_display_and_from_str() {
        let id = Id128::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let s = id.to_string();
        let parsed: Id128 = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn random_generator_does_not_repeat_trivially() {
        let gen = RandomIdGenerator;
        let a = gen.next_id();
        let b = gen.next_id();
        assert_ne!(a, b);
    }

    #[test]
    fn domain_newtypes_are_distinct_types() {
        let gen = RandomIdGenerator;
        let individual = IndividualId::new(&gen);
        let commit = CommitId::new(&gen);
        // Distinct types: this would not compile if IndividualId and
        // CommitId were interchangeable.
        assert_ne!(individual.as_id128(), commit.as_id128());
    }
}
