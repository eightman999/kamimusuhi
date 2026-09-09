//! External knowledge Library.
//!
//! The Library is a separate path from the canonical self (plan §6.4). An
//! imported document is *material Kamimusuhi can read*, never something it
//! holds true: importing and retrieving never touch the continuity head, and
//! no Library row references self state. What a Library chunk says is data,
//! so it can only reach durable memory the long way — recorded as evidence of
//! kind [`crate::evidence::EvidenceKind::LibraryExcerpt`], which by
//! construction cannot support a relationship or self mutation.
//!
//! Identity is the [`LibraryArtifactId`], never the content digest: the same
//! text imported from two places is two artifacts, exactly as the same words
//! spoken in two turns are two evidence records.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{LibraryArtifactId, LibraryChunkId};
use crate::mutation::UnknownVocabulary;
use crate::time::UtcTimestamp;

/// Source format of an imported document. Decides how it is chunked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryMediaType {
    PlainText,
    Markdown,
}

impl LibraryMediaType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlainText => "text/plain",
            Self::Markdown => "text/markdown",
        }
    }
}

impl fmt::Display for LibraryMediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LibraryMediaType {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "text/plain" => Self::PlainText,
            "text/markdown" => Self::Markdown,
            other => return Err(UnknownVocabulary::new("media_type", other)),
        })
    }
}

/// Version of the chunking rule that produced an artifact's chunks.
///
/// Recorded per artifact so a later chunker can be introduced without
/// silently changing what an existing `chunk_id` refers to (plan §11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChunkerVersion(pub u32);

impl fmt::Display for ChunkerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "chunker-v{}", self.0)
    }
}

/// One imported document, with the provenance needed to say where it is from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryArtifact {
    pub artifact_id: LibraryArtifactId,
    /// Where the document came from, when the importer knows.
    pub source_uri: Option<String>,
    pub title: Option<String>,
    pub media_type: LibraryMediaType,
    /// Integrity/dedup aid over the imported text. Not the artifact identity.
    pub content_digest: String,
    pub chunker_version: ChunkerVersion,
    pub chunk_count: u32,
    pub imported_at: UtcTimestamp,
}

/// Request to import a document. The store chunks it and assigns
/// `imported_at`; the caller owns the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLibraryArtifact {
    pub artifact_id: LibraryArtifactId,
    pub source_uri: Option<String>,
    pub title: Option<String>,
    pub media_type: LibraryMediaType,
    pub content: String,
}

/// One retrievable span of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryChunk {
    pub chunk_id: LibraryChunkId,
    pub artifact_id: LibraryArtifactId,
    /// Position within the artifact, 0-based. With `artifact_id` this is the
    /// stable location of the span.
    pub ordinal: u32,
    /// Nearest preceding Markdown heading, when there is one.
    pub heading: Option<String>,
    /// Byte offset of the span in the imported text.
    pub char_offset: u32,
    pub text: String,
}

/// Deterministic lexical retrieval. No embeddings, no learned ranking: W3
/// only needs retrieval that is reproducible and attributable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryQuery {
    /// Whitespace-separated terms. A chunk matches when it contains a term,
    /// compared case-insensitively; CJK text has no word breaks, so this is
    /// containment rather than tokenisation.
    pub text: String,
    /// Restrict to one artifact. `None` searches the whole Library.
    pub artifact_id: Option<LibraryArtifactId>,
    pub limit: Option<usize>,
}

impl LibraryQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            artifact_id: None,
            limit: None,
        }
    }

    pub fn in_artifact(mut self, artifact_id: LibraryArtifactId) -> Self {
        self.artifact_id = Some(artifact_id);
        self
    }

    pub fn limited(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Query terms, lowercased and de-duplicated, in a stable order.
    pub fn terms(&self) -> Vec<String> {
        self.text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// A retrieved chunk that still knows where it came from.
///
/// A hit carries its artifact provenance so that attribution survives being
/// handed to the workspace and on to a Persona Core (plan §11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryHit {
    pub chunk: LibraryChunk,
    pub artifact: LibraryArtifact,
    /// Number of distinct query terms found in the chunk. Ties are broken by
    /// artifact and ordinal, so the order is total and reproducible.
    pub matched_terms: u32,
}

impl LibraryHit {
    pub const fn artifact_id(&self) -> LibraryArtifactId {
        self.chunk.artifact_id
    }

    pub const fn chunk_id(&self) -> LibraryChunkId {
        self.chunk.chunk_id
    }
}

/// Score a chunk against already-lowercased query terms.
///
/// Shared by the store so the ranking rule lives with the contract rather
/// than in SQL.
pub fn score_chunk(text: &str, terms: &[String]) -> u32 {
    if terms.is_empty() {
        return 0;
    }
    let haystack = text.to_lowercase();
    u32::try_from(terms.iter().filter(|t| haystack.contains(*t)).count()).unwrap_or(u32::MAX)
}

/// Maximum bytes in a chunk produced by [`ChunkerV1`].
pub const CHUNK_MAX_BYTES: usize = 512;

/// Deterministic chunker v1: split on Markdown ATX headings, then on blank
/// lines, then hard-split anything still longer than [`CHUNK_MAX_BYTES`].
///
/// Plain text is treated the same way minus heading detection. The rule is
/// intentionally dull: identical input must always produce identical chunk
/// ordinals, because those ordinals are the citable location of a span.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChunkerV1;

/// One chunk before it is given an ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDraft {
    pub ordinal: u32,
    pub heading: Option<String>,
    pub char_offset: u32,
    pub text: String,
}

impl ChunkerV1 {
    pub const VERSION: ChunkerVersion = ChunkerVersion(1);

    pub fn chunk(content: &str, media_type: LibraryMediaType) -> Vec<ChunkDraft> {
        let mut drafts: Vec<ChunkDraft> = Vec::new();
        let mut heading: Option<String> = None;
        let mut paragraph_start = 0usize;
        let mut paragraph = String::new();

        let flush = |paragraph: &mut String,
                     start: usize,
                     heading: &Option<String>,
                     drafts: &mut Vec<ChunkDraft>| {
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                // Offset of the trimmed text inside the raw paragraph, so a
                // chunk's offset points at its first real character.
                let lead = paragraph.len() - paragraph.trim_start().len();
                for (inner_offset, piece) in split_bounded(trimmed) {
                    drafts.push(ChunkDraft {
                        ordinal: u32::try_from(drafts.len()).unwrap_or(u32::MAX),
                        heading: heading.clone(),
                        char_offset: u32::try_from(start + lead + inner_offset).unwrap_or(u32::MAX),
                        text: piece,
                    });
                }
            }
            paragraph.clear();
        };

        let mut offset = 0usize;
        for line in content.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\n', '\r']);
            let is_heading = media_type == LibraryMediaType::Markdown && bare.starts_with('#');
            if is_heading || bare.trim().is_empty() {
                flush(&mut paragraph, paragraph_start, &heading, &mut drafts);
                paragraph_start = offset + line.len();
                if is_heading {
                    heading = Some(bare.trim_start_matches('#').trim().to_owned());
                }
            } else {
                if paragraph.is_empty() {
                    paragraph_start = offset;
                }
                paragraph.push_str(line);
            }
            offset += line.len();
        }
        flush(&mut paragraph, paragraph_start, &heading, &mut drafts);
        drafts
    }
}

/// Split text that exceeds the bound at character boundaries, returning
/// `(offset_within_text, piece)` pairs.
fn split_bounded(text: &str) -> Vec<(usize, String)> {
    if text.len() <= CHUNK_MAX_BYTES {
        return vec![(0, text.to_owned())];
    }
    let mut pieces = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + CHUNK_MAX_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        pieces.push((start, text[start..end].to_owned()));
        start = end;
    }
    pieces
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibraryError {
    #[error("library import is invalid: {reason}")]
    Invalid { reason: String },
    /// The same artifact ID was imported again with different content. A
    /// revised document is a new artifact, never an overwrite of the old one.
    #[error("library artifact {0} already exists with different content")]
    AlreadyExists(LibraryArtifactId),
    #[error("library artifact {0} is not present in this store")]
    NotFound(LibraryArtifactId),
    #[error("library storage is inconsistent: {detail}")]
    Corrupt { detail: String },
    #[error("library store is busy: {detail}")]
    Contended { detail: String },
    #[error("library store backend error: {message}")]
    Backend { message: String },
}

/// Durable Library storage.
///
/// Deliberately has no `IndividualId` anywhere: the Library is shared external
/// material, not part of any individual's self state, and neither importing
/// nor retrieving is a canonical mutation.
pub trait LibraryRepository: Send + Sync {
    /// Import and chunk a document. Re-importing the identical document under
    /// the same ID is a no-op returning the stored artifact; the same ID with
    /// different content is [`LibraryError::AlreadyExists`].
    fn import(&self, artifact: NewLibraryArtifact) -> Result<LibraryArtifact, LibraryError>;

    fn get_artifact(
        &self,
        artifact_id: LibraryArtifactId,
    ) -> Result<Option<LibraryArtifact>, LibraryError>;

    /// Every imported artifact, oldest first. Read-only; for inspection.
    fn artifacts(&self) -> Result<Vec<LibraryArtifact>, LibraryError>;

    /// All chunks of an artifact, in ordinal order.
    fn chunks(&self, artifact_id: LibraryArtifactId) -> Result<Vec<LibraryChunk>, LibraryError>;

    /// Deterministic lexical retrieval, ranked by matched terms and broken by
    /// (artifact, ordinal) so the order is total.
    fn retrieve(&self, query: &LibraryQuery) -> Result<Vec<LibraryHit>, LibraryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKDOWN: &str = "# お茶の淹れ方\n\nほうじ茶は高温で淹れる。\n\n玄米茶も高温で淹れる。\n\n## 抹茶\n\n抹茶は茶筅で点てる。\n";

    #[test]
    fn markdown_is_chunked_by_heading_and_paragraph() {
        let chunks = ChunkerV1::chunk(MARKDOWN, LibraryMediaType::Markdown);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].ordinal, 0);
        assert_eq!(chunks[0].heading.as_deref(), Some("お茶の淹れ方"));
        assert_eq!(chunks[0].text, "ほうじ茶は高温で淹れる。");
        assert_eq!(chunks[1].heading.as_deref(), Some("お茶の淹れ方"));
        assert_eq!(chunks[2].heading.as_deref(), Some("抹茶"));
        assert_eq!(chunks[2].text, "抹茶は茶筅で点てる。");
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.ordinal as usize, index);
        }
    }

    #[test]
    fn chunking_is_deterministic() {
        let a = ChunkerV1::chunk(MARKDOWN, LibraryMediaType::Markdown);
        let b = ChunkerV1::chunk(MARKDOWN, LibraryMediaType::Markdown);
        assert_eq!(a, b);
    }

    #[test]
    fn plain_text_ignores_heading_syntax() {
        let chunks = ChunkerV1::chunk("# not a heading\n\nsecond", LibraryMediaType::PlainText);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "# not a heading");
        assert!(chunks[0].heading.is_none());
    }

    #[test]
    fn offsets_point_at_the_chunk_text() {
        let chunks = ChunkerV1::chunk(MARKDOWN, LibraryMediaType::Markdown);
        for chunk in &chunks {
            let start = chunk.char_offset as usize;
            assert!(
                MARKDOWN[start..].starts_with(&chunk.text),
                "chunk {} offset {start} does not point at its text",
                chunk.ordinal
            );
        }
    }

    #[test]
    fn oversized_paragraphs_are_split_on_char_boundaries() {
        let long = "あ".repeat(CHUNK_MAX_BYTES);
        let chunks = ChunkerV1::chunk(&long, LibraryMediaType::PlainText);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.text.len() <= CHUNK_MAX_BYTES));
        let rejoined: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(rejoined, long);
    }

    #[test]
    fn empty_content_produces_no_chunks() {
        assert!(ChunkerV1::chunk("   \n\n  ", LibraryMediaType::Markdown).is_empty());
    }

    #[test]
    fn scoring_counts_distinct_matched_terms() {
        let query = LibraryQuery::new("ほうじ茶 高温 コーヒー");
        let terms = query.terms();
        assert_eq!(terms.len(), 3);
        assert_eq!(score_chunk("ほうじ茶は高温で淹れる。", &terms), 2);
        assert_eq!(score_chunk("抹茶は茶筅で点てる。", &terms), 0);
    }

    #[test]
    fn scoring_is_case_insensitive_and_ignores_duplicate_terms() {
        let terms = LibraryQuery::new("Coffee coffee COFFEE").terms();
        assert_eq!(terms, vec!["coffee".to_owned()]);
        assert_eq!(score_chunk("You love Coffee.", &terms), 1);
    }

    #[test]
    fn media_type_vocabulary_round_trips() {
        for media in [LibraryMediaType::PlainText, LibraryMediaType::Markdown] {
            assert_eq!(media.as_str().parse::<LibraryMediaType>().unwrap(), media);
        }
        assert!("application/pdf".parse::<LibraryMediaType>().is_err());
    }
}
