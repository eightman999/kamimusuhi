//! Self notes: the individual's self-authored working notes.
//!
//! A note is a hypothesis the individual wrote down about itself — a
//! tendency it noticed, a lesson it wants to keep, a stated preference, a
//! plain observation. Notes are kept in a bounded JSONL file inside the
//! runtime directory, deliberately outside canonical state: they carry no
//! canonical id, activate nothing, and their loss loses nothing the store
//! holds. They are staged into the workspace under their own domain so the
//! model can tell "something I wrote down earlier" from evidence, activated
//! memory, or the operator's seed — the same domain separation every other
//! workspace lane keeps.
//!
//! This module is types and bounds only; the file format lives with the
//! runtime's `self_notes` module.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::time::UtcTimestamp;

/// Entries kept in `self_notes.jsonl`; the oldest lines rotate out once the
/// file grows past this. Small enough that a full file is a few hundred KiB.
pub const MAX_NOTES_KEPT: usize = 200;
/// One note's text bound — notes are working memory, not documents.
pub const MAX_TEXT_BYTES: usize = 1024;
/// Ids a note may carry as provenance hints. They are recorded, never
/// resolved — a note cannot turn a name into evidence.
pub const MAX_EVIDENCE_REFS: usize = 8;
/// Per-ref bound: refs are carried as opaque strings so a note can point at
/// evidence ids, record ids or turn ids without pretending a type.
pub const MAX_REF_CHARS: usize = 64;

/// What kind of claim a note makes. The vocabulary is closed — a note names
/// one of these or is not a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfNoteKind {
    /// A pattern the individual noticed in its own behaviour.
    Tendency,
    /// Something it wants to apply next time.
    Lesson,
    /// A stated preference about how it works.
    Preference,
    /// A plain observation that fits none of the above.
    Observation,
}

impl SelfNoteKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tendency => "tendency",
            Self::Lesson => "lesson",
            Self::Preference => "preference",
            Self::Observation => "observation",
        }
    }

    /// Every kind, for schemas and error messages.
    pub const ALL: &[SelfNoteKind] = &[
        Self::Tendency,
        Self::Lesson,
        Self::Preference,
        Self::Observation,
    ];
}

impl fmt::Display for SelfNoteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SelfNoteKind {
    type Err = UnknownSelfNoteKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "tendency" => Self::Tendency,
            "lesson" => Self::Lesson,
            "preference" => Self::Preference,
            "observation" => Self::Observation,
            other => return Err(UnknownSelfNoteKind(other.to_owned())),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown self-note kind {0:?} (expected tendency|lesson|preference|observation)")]
pub struct UnknownSelfNoteKind(pub String);

/// One self-authored note. `seq` is the note file's own ordering, assigned
/// by the writer; it deliberately shares no namespace with evidence ids,
/// state-record ids or turn ids so a note cannot be mistaken for any of
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfNote {
    pub seq: u64,
    pub kind: SelfNoteKind,
    pub text: String,
    /// Opaque ids this note draws on, as the caller named them. Provenance
    /// hints only: nothing in the lane resolves or trusts them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    pub created_at: UtcTimestamp,
}

impl SelfNote {
    /// The label a workspace item cites. `sn-` keeps the note lane visibly
    /// distinct from every canonical id namespace.
    pub fn note_id(&self) -> String {
        format!("sn-{}", self.seq)
    }

    /// Build a note after checking every bound. Rejects — never clips — so
    /// the caller (usually the `self_note` tool) gets the bound as an error
    /// it can act on.
    pub fn checked(
        seq: u64,
        kind: SelfNoteKind,
        text: impl Into<String>,
        evidence_refs: Vec<String>,
        created_at: UtcTimestamp,
    ) -> Result<Self, SelfNoteError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(SelfNoteError::EmptyText);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(SelfNoteError::TextTooLong {
                bytes: text.len(),
                max: MAX_TEXT_BYTES,
            });
        }
        if evidence_refs.len() > MAX_EVIDENCE_REFS {
            return Err(SelfNoteError::TooManyRefs {
                count: evidence_refs.len(),
                max: MAX_EVIDENCE_REFS,
            });
        }
        for (index, reference) in evidence_refs.iter().enumerate() {
            if reference.is_empty() {
                return Err(SelfNoteError::EmptyRef { index });
            }
            if reference.len() > MAX_REF_CHARS {
                return Err(SelfNoteError::RefTooLong {
                    index,
                    max: MAX_REF_CHARS,
                });
            }
        }
        Ok(Self {
            seq,
            kind,
            text,
            evidence_refs,
            created_at,
        })
    }
}

/// Why a candidate note was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelfNoteError {
    #[error("text is empty")]
    EmptyText,
    #[error("text is {bytes} bytes, over the {max}-byte bound")]
    TextTooLong { bytes: usize, max: usize },
    #[error("{count} evidence refs, over the {max} bound")]
    TooManyRefs { count: usize, max: usize },
    #[error("evidence ref {index} is empty")]
    EmptyRef { index: usize },
    #[error("evidence ref {index} is over {max} chars")]
    RefTooLong { index: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> UtcTimestamp {
        UtcTimestamp::from_unix_millis(1_000)
    }

    #[test]
    fn the_kind_vocabulary_is_closed() {
        assert_eq!(
            "tendency".parse::<SelfNoteKind>(),
            Ok(SelfNoteKind::Tendency)
        );
        assert_eq!("lesson".parse::<SelfNoteKind>(), Ok(SelfNoteKind::Lesson));
        assert_eq!(
            "preference".parse::<SelfNoteKind>(),
            Ok(SelfNoteKind::Preference)
        );
        assert_eq!(
            "observation".parse::<SelfNoteKind>(),
            Ok(SelfNoteKind::Observation)
        );
        for kind in SelfNoteKind::ALL {
            assert_eq!(kind.as_str().parse::<SelfNoteKind>(), Ok(*kind));
        }
        assert!("fact".parse::<SelfNoteKind>().is_err());
        assert!("".parse::<SelfNoteKind>().is_err());
    }

    #[test]
    fn checked_enforces_the_bounds_without_clipping() {
        let note = SelfNote::checked(
            7,
            SelfNoteKind::Lesson,
            "待つ前に一度確認する",
            vec!["ev-1".to_owned()],
            at(),
        )
        .expect("in-bounds note");
        assert_eq!(note.note_id(), "sn-7");

        assert!(matches!(
            SelfNote::checked(1, SelfNoteKind::Lesson, "   ", vec![], at()),
            Err(SelfNoteError::EmptyText)
        ));
        assert!(matches!(
            SelfNote::checked(1, SelfNoteKind::Lesson, "x".repeat(1025), vec![], at()),
            Err(SelfNoteError::TextTooLong { .. })
        ));
        assert!(SelfNote::checked(1, SelfNoteKind::Lesson, "x".repeat(1024), vec![], at()).is_ok());
        assert!(matches!(
            SelfNote::checked(
                1,
                SelfNoteKind::Lesson,
                "t",
                (0..9).map(|i| i.to_string()).collect(),
                at(),
            ),
            Err(SelfNoteError::TooManyRefs { .. })
        ));
        assert!(matches!(
            SelfNote::checked(1, SelfNoteKind::Lesson, "t", vec!["e".repeat(65)], at()),
            Err(SelfNoteError::RefTooLong { .. })
        ));
        assert!(matches!(
            SelfNote::checked(1, SelfNoteKind::Lesson, "t", vec![String::new()], at()),
            Err(SelfNoteError::EmptyRef { .. })
        ));
    }

    #[test]
    fn serialized_notes_keep_their_lane_marker() {
        let note = SelfNote::checked(3, SelfNoteKind::Tendency, "傾向", vec![], at()).unwrap();
        let value = serde_json::to_value(&note).expect("serialize");
        assert_eq!(value["kind"], "tendency");
        assert_eq!(value["seq"], 3);
        assert!(value.get("evidence_refs").is_none());
        let back: SelfNote = serde_json::from_value(value).expect("round trip");
        assert_eq!(back, note);
        // An unknown kind on disk is not a note — the line is dropped by the
        // reader, never reinterpreted.
        assert!(
            serde_json::from_str::<SelfNote>(
                r#"{"seq":1,"kind":"fact","text":"x","created_at":0}"#
            )
            .is_err()
        );
    }
}
