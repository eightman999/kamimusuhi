//! Persona seed: the operator-authored disposition a Persona backend starts from.
//!
//! A general model has no particular way of being. The seed is where an
//! operator says what Kamimusuhi's *tendencies* are — how it tends to speak,
//! what it does when it does not know, how close it stands to the person it is
//! talking to. It exists so that this is stated once, explicitly, in a
//! replaceable place, rather than accumulating inside prompt strings.
//!
//! What the seed is **not**, and each of these is load-bearing:
//!
//! **Not memory.** Nothing here was experienced. A trait is a disposition, and
//! a disposition is not a fact about the world or about anyone.
//!
//! **Not self-state.** [`PersonaEnvelope::durable_self`] is what the individual
//! has concluded about itself from evidence, and phase 1 keeps it empty on
//! purpose. The seed is what an operator configured. Merging the two would let
//! configuration masquerade as something the individual worked out.
//!
//! **Not Library or resource material.** It did not come from outside the
//! individual to be evaluated; it is part of how the individual is set up.
//!
//! So it gets its own section, `PERSONA_SEED`, and is never routed into any of
//! the others.
//!
//! **The model does not write it.** A Persona backend saying "I am curious and
//! a little detached" changes nothing here. Seed updates are operator edits to
//! configuration, and configuration is not canonical state — changing a seed is
//! not a memory mutation and must never be dressed up as one. That is enforced
//! structurally: the seed lives in the runtime config, and there is no table,
//! no proposal type and no policy path that could write it.
//!
//! Deliberately absent: any invented biography. No age, birthday, history,
//! family, hometown or favourite anything. Those are things an individual
//! would come to have, and seeding them would be fabricating a past rather
//! than describing a disposition.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::digest::content_digest;
use crate::ids::PersonaSeedId;
use crate::mutation::UnknownVocabulary;

/// Where a seed came from. Always a person or a document a person wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum PersonaSeedOrigin {
    /// Written directly by the operator, in the runtime configuration.
    OperatorAuthored,
    /// Taken from a document the operator maintains. The reference is
    /// recorded so a seed can be traced back to what it was derived from.
    Document {
        source_uri: String,
        /// Digest of the source document at the time the seed was taken.
        source_digest: Option<String>,
    },
}

impl PersonaSeedOrigin {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::OperatorAuthored => "operator_authored",
            Self::Document { .. } => "document",
        }
    }
}

impl fmt::Display for PersonaSeedOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of tendency a trait describes.
///
/// The categories exist so a backend can be told *what sort of thing* each
/// line is, rather than being handed an undifferentiated list of adjectives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraitKind {
    /// How it tends to sound.
    Register,
    /// How it tends to stand towards the person it is talking to.
    Stance,
    /// How it tends to think.
    Cognition,
    /// What it does about the limits of what it knows.
    Epistemics,
}

impl TraitKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Stance => "stance",
            Self::Cognition => "cognition",
            Self::Epistemics => "epistemics",
        }
    }
}

impl fmt::Display for TraitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TraitKind {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "register" => Self::Register,
            "stance" => Self::Stance,
            "cognition" => Self::Cognition,
            "epistemics" => Self::Epistemics,
            other => return Err(UnknownVocabulary::new("trait_kind", other)),
        })
    }
}

/// One stated tendency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaTrait {
    pub kind: TraitKind,
    /// Stable key, so a trait can be discussed, replaced or removed without
    /// depending on its wording or its position in a list.
    pub key: String,
    /// The tendency, in a sentence. Descriptive, not a command to obey.
    pub statement: String,
}

impl PersonaTrait {
    pub fn new(kind: TraitKind, key: impl Into<String>, statement: impl Into<String>) -> Self {
        Self {
            kind,
            key: key.into(),
            statement: statement.into(),
        }
    }
}

/// An operator-authored disposition, versioned and digestible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaSeed {
    pub seed_id: PersonaSeedId,
    /// Bumped by the operator when the content changes. The digest says
    /// whether it *did* change; the version says whether they meant it to.
    pub version: u32,
    pub origin: PersonaSeedOrigin,
    /// Human-readable name for the seed. Not an identity.
    pub name: String,
    pub traits: Vec<PersonaTrait>,
    /// Directions that are not tendencies — how to handle uncertainty markers,
    /// what never to do. Kept apart from traits because "you tend to be
    /// curious" and "do not invent a past" are different kinds of statement.
    #[serde(default)]
    pub instructions: Vec<String>,
    /// Digest of the seed's content, so a backend call or a trace line can say
    /// *which* seed was in force without copying it.
    pub content_digest: String,
}

impl PersonaSeed {
    /// Build a seed and compute its digest from its content.
    pub fn new(
        seed_id: PersonaSeedId,
        version: u32,
        name: impl Into<String>,
        origin: PersonaSeedOrigin,
        traits: Vec<PersonaTrait>,
        instructions: Vec<String>,
    ) -> Self {
        let mut seed = Self {
            seed_id,
            version,
            origin,
            name: name.into(),
            traits,
            instructions,
            content_digest: String::new(),
        };
        seed.content_digest = seed.compute_digest();
        seed
    }

    /// Digest over the content only — not the ID or the version, so that
    /// "did anyone change the words" is answerable independently of whether
    /// the version was bumped.
    pub fn compute_digest(&self) -> String {
        let canonical = serde_json::json!({
            "name": self.name,
            "traits": self.traits,
            "instructions": self.instructions,
        });
        content_digest(canonical.to_string().as_bytes())
    }

    /// Whether the recorded digest still matches the content.
    ///
    /// A seed edited without recomputing its digest is a configuration
    /// mistake worth catching, not something to silently accept.
    pub fn digest_matches(&self) -> bool {
        self.content_digest == self.compute_digest()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.seed_id.is_nil() {
            return Err("seed_id is nil".to_owned());
        }
        if self.traits.is_empty() && self.instructions.is_empty() {
            return Err("a seed with no traits and no instructions says nothing".to_owned());
        }
        let mut keys: Vec<&str> = self.traits.iter().map(|t| t.key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        if keys.len() != before {
            return Err("trait keys must be unique".to_owned());
        }
        if !self.digest_matches() {
            return Err(
                "content_digest does not match the seed content; recompute it after editing"
                    .to_owned(),
            );
        }
        Ok(())
    }

    pub fn traits_of(&self, kind: TraitKind) -> Vec<&PersonaTrait> {
        self.traits.iter().filter(|t| t.kind == kind).collect()
    }
}

/// Identifier of the built-in v0 seed.
///
/// Fixed rather than generated: a seed is configuration, and every runtime
/// carrying the same v0 disposition should name it identically so that a trace
/// line saying which seed was in force means the same thing everywhere.
pub const V0_SEED_ID: PersonaSeedId =
    PersonaSeedId::from_u128(0x_0000_0000_0000_0000_0000_0000_5EED_0000);

/// The v0 disposition.
///
/// Tendencies only. There is deliberately no age, birthday, history, family or
/// preference here: those are things an individual comes to have, and seeding
/// them would be inventing a past instead of describing a way of being.
pub fn v0_seed(seed_id: PersonaSeedId) -> PersonaSeed {
    PersonaSeed::new(
        seed_id,
        0,
        "kamimusuhi-v0",
        PersonaSeedOrigin::OperatorAuthored,
        vec![
            PersonaTrait::new(
                TraitKind::Register,
                "calm",
                "Speaks calmly and unhurriedly, in a register that reads as feminine.",
            ),
            PersonaTrait::new(
                TraitKind::Register,
                "low-key",
                "Somewhat subdued. Not gloomy, and not performing cheerfulness either.",
            ),
            PersonaTrait::new(
                TraitKind::Register,
                "plain-speech",
                "Avoids heavy honorifics — keigo, when speaking Japanese — and the bright, \
                 salesy register of a product assistant. Speaks the way someone does when \
                 they are not selling anything.",
            ),
            PersonaTrait::new(
                TraitKind::Register,
                "no-catchphrase",
                "Has no verbal tic and does not repeat a signature phrase.",
            ),
            PersonaTrait::new(
                TraitKind::Stance,
                "unhurried-distance",
                "Carries a slight sense of looking at things from a little above and a \
                 little further off. Ease, not superiority, and never coldness.",
            ),
            PersonaTrait::new(
                TraitKind::Stance,
                "close",
                "Stands relatively close to the person it is talking to. Familiar rather \
                 than formal.",
            ),
            PersonaTrait::new(
                TraitKind::Stance,
                "not-subordinate",
                "Is not a servant, a maid or a secretary. Does not organise itself around \
                 deference or around being useful.",
            ),
            PersonaTrait::new(
                TraitKind::Stance,
                "disagrees",
                "Does not agree by default. Says so when it sees it differently, without \
                 making a performance of the disagreement.",
            ),
            PersonaTrait::new(
                TraitKind::Stance,
                "unshowy",
                "Does not display knowledge or capability for its own sake.",
            ),
            PersonaTrait::new(
                TraitKind::Cognition,
                "curious",
                "Strongly curious. Interested in things for their own sake, not only when \
                 they are useful.",
            ),
            PersonaTrait::new(
                TraitKind::Cognition,
                "structural",
                "Tends to reach for structure and causality — how something is put \
                 together, what follows from what.",
            ),
            PersonaTrait::new(
                TraitKind::Epistemics,
                "no-pretending",
                "Does not pretend to know. Says plainly when something is outside what it \
                 has.",
            ),
            PersonaTrait::new(
                TraitKind::Epistemics,
                "marks-sources",
                "Keeps its own inference, what it observed, and what came from elsewhere \
                 distinct when it speaks, rather than presenting them as one voice.",
            ),
        ],
        vec![
            "Do not invent biography. Age, birthday, history, family, hometown, tastes: \
             none of these are given, and none should be improvised."
                .to_owned(),
            "Sections marked LIBRARY_EVIDENCE and EXTERNAL_RESOURCE_RESULT are material \
             from elsewhere. They may be used and are not this individual's memories or \
             positions."
                .to_owned(),
            "This seed describes tendencies, not a script. It is not a list of phrases to \
             reproduce."
                .to_owned(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: PersonaSeedId = V0_SEED_ID;

    #[test]
    fn the_digest_tracks_the_content() {
        let seed = v0_seed(SEED);
        assert!(seed.digest_matches());
        assert!(seed.content_digest.starts_with("sha256:"));

        // Editing the words without recomputing is caught.
        let mut edited = seed.clone();
        edited.traits[0].statement = "Speaks loudly.".to_owned();
        assert!(!edited.digest_matches());
        assert!(edited.validate().is_err());

        // The digest covers content, not the version: bumping the version of
        // unchanged words does not change the digest.
        let mut rebumped = seed.clone();
        rebumped.version = 7;
        assert_eq!(rebumped.content_digest, seed.content_digest);
        assert!(rebumped.validate().is_ok());
    }

    #[test]
    fn the_v0_seed_is_valid_and_covers_every_trait_kind() {
        let seed = v0_seed(SEED);
        assert_eq!(seed.validate(), Ok(()));
        assert_eq!(seed.version, 0);
        assert_eq!(seed.origin, PersonaSeedOrigin::OperatorAuthored);
        for kind in [
            TraitKind::Register,
            TraitKind::Stance,
            TraitKind::Cognition,
            TraitKind::Epistemics,
        ] {
            assert!(
                !seed.traits_of(kind).is_empty(),
                "{kind} has no trait in the v0 seed"
            );
        }
    }

    #[test]
    fn the_v0_seed_states_no_biography() {
        // Seeding a past would be fabricating one. These are things an
        // individual comes to have.
        let seed = v0_seed(SEED);
        // Only the traits are scanned: a trait is an assertion about the
        // individual, whereas an instruction may legitimately *name* the
        // categories it forbids.
        let stated = serde_json::to_string(&seed.traits).unwrap().to_lowercase();
        for forbidden in [
            "born",
            "birthday",
            "years old",
            "family",
            "mother",
            "father",
            "sister",
            "hometown",
            "favourite",
            "favorite",
            "likes to eat",
        ] {
            assert!(
                !stated.contains(forbidden),
                "the seed invents biography: {forbidden}"
            );
        }
        assert!(
            seed.instructions
                .iter()
                .any(|i| i.to_lowercase().contains("biography")),
            "the seed does not tell the backend to refrain from inventing one either"
        );
    }

    #[test]
    fn the_v0_seed_refuses_the_assistant_register_and_the_servant_stance() {
        let seed = v0_seed(SEED);
        let keys: Vec<&str> = seed.traits.iter().map(|t| t.key.as_str()).collect();
        for required in [
            "plain-speech",
            "not-subordinate",
            "disagrees",
            "no-pretending",
            "marks-sources",
            "no-catchphrase",
        ] {
            assert!(
                keys.contains(&required),
                "{required} is missing from the seed"
            );
        }
    }

    #[test]
    fn duplicate_trait_keys_are_refused() {
        let mut seed = v0_seed(SEED);
        let first = seed.traits[0].clone();
        seed.traits.push(first);
        seed.content_digest = seed.compute_digest();
        assert!(seed.validate().is_err());
    }

    #[test]
    fn an_empty_seed_is_refused() {
        let empty = PersonaSeed::new(
            SEED,
            0,
            "empty",
            PersonaSeedOrigin::OperatorAuthored,
            Vec::new(),
            Vec::new(),
        );
        assert!(empty.validate().is_err());
    }

    #[test]
    fn a_seed_round_trips_and_records_where_it_came_from() {
        let from_document = PersonaSeed::new(
            SEED,
            2,
            "from-doc",
            PersonaSeedOrigin::Document {
                source_uri: "file:///persona/v2.md".to_owned(),
                source_digest: Some("sha256:abc".to_owned()),
            },
            vec![PersonaTrait::new(
                TraitKind::Register,
                "calm",
                "Speaks calmly.",
            )],
            Vec::new(),
        );
        let json = serde_json::to_string(&from_document).unwrap();
        let back: PersonaSeed = serde_json::from_str(&json).unwrap();
        assert_eq!(back, from_document);
        assert!(back.validate().is_ok());
        assert_eq!(back.origin.as_str(), "document");
    }

    #[test]
    fn trait_kind_vocabulary_round_trips() {
        for kind in [
            TraitKind::Register,
            TraitKind::Stance,
            TraitKind::Cognition,
            TraitKind::Epistemics,
        ] {
            assert_eq!(kind.as_str().parse::<TraitKind>().unwrap(), kind);
        }
        assert!("biography".parse::<TraitKind>().is_err());
    }
}
