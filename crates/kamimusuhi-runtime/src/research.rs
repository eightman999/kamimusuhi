//! Reviewed experiment findings are versioned Library material. Retrieval
//! preserves a whole finding, including its limits and negative status; it
//! neither changes the canonical self nor establishes an acquired ability.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::LibraryArtifactId;
use kamimusuhi_core::library::{
    ChunkerV1, LibraryArtifact, LibraryMediaType, LibraryRepository, NewLibraryArtifact,
};
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

pub const MAX_CATALOG_BYTES: usize = 256 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 12 * 1024;
pub const MAX_SELECTED_FINDINGS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Supported,
    Limited,
    Failed,
    Invalid,
    Pending,
}

impl FindingStatus {
    fn caution_priority(self) -> u8 {
        match self {
            Self::Invalid => 0,
            Self::Failed => 1,
            Self::Limited => 2,
            Self::Pending => 3,
            Self::Supported => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingSource {
    pub path: String,
    pub digest: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchFinding {
    pub artifact_id: String,
    pub id: String,
    pub revision: u32,
    pub experiment: String,
    pub status: FindingStatus,
    pub title: String,
    pub claim: String,
    pub limitations: String,
    pub keywords: Vec<String>,
    pub sources: Vec<FindingSource>,
    pub applications: Vec<String>,
}

impl ResearchFinding {
    fn library_id(&self) -> Result<LibraryArtifactId, RuntimeError> {
        // Runtime IDs historically use 32 lowercase hex characters. Accept
        // canonical UUID spelling in reviewed catalogs as the same identity.
        let raw = &self.artifact_id;
        let compact = if raw.len() == 36
            && raw.as_bytes()[8] == b'-'
            && raw.as_bytes()[13] == b'-'
            && raw.as_bytes()[18] == b'-'
            && raw.as_bytes()[23] == b'-'
        {
            raw.replace('-', "")
        } else {
            raw.clone()
        };
        let id: LibraryArtifactId = compact
            .parse()
            .map_err(|_| invalid("invalid artifact ID"))?;
        if id.is_nil() {
            return Err(invalid("nil artifact ID"));
        }
        Ok(id)
    }

    fn source_uri(&self) -> String {
        format!(
            "research://findings/{}/revisions/{}",
            self.id.to_ascii_lowercase(),
            self.revision
        )
    }

    fn payload(&self) -> Result<String, RuntimeError> {
        serde_json::to_string(self).map_err(|_| invalid("finding cannot be serialized"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchCatalog {
    pub schema_version: u32,
    pub revision: u32,
    pub findings: Vec<ResearchFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedFinding {
    pub artifact_id: LibraryArtifactId,
    pub content_digest: String,
    pub finding: ResearchFinding,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResearchContext {
    pub catalog_revision: u32,
    pub active_findings: usize,
    pub selected: Vec<SelectedFinding>,
}

fn invalid(reason: &str) -> RuntimeError {
    RuntimeError::Usage(format!("invalid research catalog: {reason}"))
}

fn bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.contains('\0')
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

fn valid_path(value: &str) -> bool {
    bounded_text(value, 512)
        && !value.contains('\\')
        && !value.contains(':')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl ResearchCatalog {
    pub fn bundled() -> Result<Self, RuntimeError> {
        Self::from_json(include_str!("../../../knowledge/experiment-findings.json"))
    }

    pub fn from_json(json: &str) -> Result<Self, RuntimeError> {
        if json.len() > MAX_CATALOG_BYTES {
            return Err(invalid("catalog exceeds byte limit"));
        }
        let catalog: Self =
            serde_json::from_str(json).map_err(|_| invalid("malformed or unsupported schema"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.schema_version != 1 || self.revision == 0 || self.findings.len() > 64 {
            return Err(invalid("unsupported version or finding count"));
        }
        if serde_json::to_vec(self)
            .map_err(|_| invalid("catalog cannot be serialized"))?
            .len()
            > MAX_CATALOG_BYTES
        {
            return Err(invalid("catalog exceeds byte limit"));
        }
        let mut ids = BTreeSet::new();
        let mut artifacts = BTreeSet::new();
        for finding in &self.findings {
            if !valid_id(&finding.id)
                || finding.revision == 0
                || !ids.insert(finding.id.to_lowercase())
                || !artifacts.insert(finding.library_id()?)
            {
                return Err(invalid("invalid or duplicate finding identity/revision"));
            }
            if !bounded_text(&finding.experiment, 160)
                || !bounded_text(&finding.title, 240)
                || !bounded_text(&finding.claim, 1_200)
                || !bounded_text(&finding.limitations, 1_200)
                || finding.keywords.is_empty()
                || finding.keywords.len() > 16
                || finding.sources.is_empty()
                || finding.sources.len() > 4
                || finding.applications.len() > 8
            {
                return Err(invalid("missing or oversized finding content"));
            }
            let mut keywords = BTreeSet::new();
            for keyword in &finding.keywords {
                if !bounded_text(keyword, 80)
                    || keyword.chars().any(char::is_control)
                    || !keywords.insert(keyword.to_lowercase())
                {
                    return Err(invalid("invalid or duplicate keyword"));
                }
            }
            if finding
                .applications
                .iter()
                .any(|application| !bounded_text(application, 400))
            {
                return Err(invalid("invalid application description"));
            }
            let mut sources = BTreeSet::new();
            for source in &finding.sources {
                if !valid_path(&source.path)
                    || !valid_digest(&source.digest)
                    || source.start_line == 0
                    || source.end_line < source.start_line
                    || !sources.insert((&source.path, source.start_line, source.end_line))
                {
                    return Err(invalid("invalid or duplicate source reference"));
                }
            }
        }
        Ok(())
    }

    /// CI/CLI provenance verification. Ordinary dialogue uses the reviewed
    /// bundled catalog and need not have the source checkout at runtime.
    pub fn validate_sources(&self, repo_root: &Path) -> Result<(), RuntimeError> {
        self.validate()?;
        let root = repo_root
            .canonicalize()
            .map_err(|_| invalid("source repository is unavailable"))?;
        for finding in &self.findings {
            for source in &finding.sources {
                let path = root
                    .join(&source.path)
                    .canonicalize()
                    .map_err(|_| invalid("source file is unavailable"))?;
                if !path.starts_with(&root) || !path.is_file() {
                    return Err(invalid("source escapes repository or is not a file"));
                }
                let bytes =
                    std::fs::read(&path).map_err(|_| invalid("source file cannot be read"))?;
                if content_digest(&bytes) != source.digest {
                    return Err(invalid("source file digest changed"));
                }
                let text =
                    std::str::from_utf8(&bytes).map_err(|_| invalid("source is not UTF-8 text"))?;
                if source.end_line as usize > text.lines().count() {
                    return Err(invalid("source line range is outside file"));
                }
            }
        }
        Ok(())
    }

    /// Append new versions; an identical re-import is a no-op. Preflight all
    /// known identity conflicts before importing any new material.
    pub fn sync(&self, store: &impl LibraryRepository) -> Result<usize, RuntimeError> {
        self.validate()?;
        let existing = store.artifacts()?;
        let mut missing = Vec::new();
        for finding in &self.findings {
            let id = finding.library_id()?;
            let uri = finding.source_uri();
            if existing.iter().any(|artifact| {
                artifact.source_uri.as_deref() == Some(uri.as_str()) && artifact.artifact_id != id
            }) {
                return Err(invalid(
                    "a finding revision already uses another artifact ID",
                ));
            }
            if let Some(artifact) = store.get_artifact(id)? {
                verify_artifact(store, finding, &artifact)?;
            } else {
                missing.push(finding);
            }
        }
        for finding in &missing {
            let artifact = store.import(NewLibraryArtifact {
                artifact_id: finding.library_id()?,
                source_uri: Some(finding.source_uri()),
                title: Some(finding.title.clone()),
                media_type: LibraryMediaType::PlainText,
                content: finding.payload()?,
            })?;
            verify_artifact(store, finding, &artifact)?;
        }
        Ok(missing.len())
    }

    /// Match known IDs/tags inside Japanese questions; the query is only
    /// matching data and is never rendered as a system instruction.
    pub fn context(
        &self,
        store: &impl LibraryRepository,
        text: &str,
    ) -> Result<ResearchContext, RuntimeError> {
        self.validate()?;
        let query = text.to_lowercase();
        let overview = ["実験", "成果", "研究", "experiment", "research"]
            .iter()
            .any(|tag| query.contains(tag));
        let mut ranked: Vec<_> = self
            .findings
            .iter()
            .filter_map(|finding| {
                // A reviewed lineage may name G0/G0-v5/G0-v6 together.
                // Match explicit experiment aliases above generic tags so
                // an invalid result is not buried by memory/learning tags.
                let experiment_match = finding
                    .experiment
                    .split('/')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .any(|part| query.contains(&part.to_lowercase()));
                let experiment_keyword = finding.keywords.iter().any(|keyword| {
                    valid_id(keyword)
                        && keyword.as_bytes()[0].is_ascii_alphabetic()
                        && keyword.bytes().any(|byte| byte.is_ascii_digit())
                        && query.contains(&keyword.to_lowercase())
                });
                let score = 100 * u32::from(query.contains(&finding.id.to_lowercase()))
                    + 80 * u32::from(experiment_match || experiment_keyword)
                    + 5 * finding
                        .keywords
                        .iter()
                        .filter(|keyword| query.contains(&keyword.to_lowercase()))
                        .count() as u32;
                (score > 0).then_some((score, finding))
            })
            .collect();
        if ranked.is_empty() && overview {
            // A short overview represents different outcomes instead of
            // filling every slot with either successes or negative results.
            let statuses = [
                FindingStatus::Supported,
                FindingStatus::Limited,
                FindingStatus::Invalid,
                FindingStatus::Failed,
                FindingStatus::Pending,
            ];
            let mut ordered: Vec<_> = self.findings.iter().collect();
            ordered.sort_by(|a, b| a.id.cmp(&b.id));
            for round in 0..self.findings.len() {
                for status in statuses {
                    if let Some(finding) = ordered
                        .iter()
                        .filter(|finding| finding.status == status)
                        .nth(round)
                    {
                        ranked.push((0, *finding));
                    }
                }
            }
        } else {
            ranked.sort_by(|(a_score, a), (b_score, b)| {
                b_score
                    .cmp(a_score)
                    .then(
                        a.status
                            .caution_priority()
                            .cmp(&b.status.caution_priority()),
                    )
                    .then(a.id.cmp(&b.id))
            });
        }
        let mut context = ResearchContext {
            catalog_revision: self.revision,
            active_findings: self.findings.len(),
            selected: Vec::new(),
        };
        for (_, finding) in ranked {
            let id = finding.library_id()?;
            let artifact = store
                .get_artifact(id)?
                .ok_or_else(|| invalid("active finding has not been imported"))?;
            verify_artifact(store, finding, &artifact)?;
            context.selected.push(SelectedFinding {
                artifact_id: id,
                content_digest: artifact.content_digest,
                finding: finding.clone(),
            });
            if serde_json::to_vec(&context)
                .map_err(|_| invalid("context cannot be serialized"))?
                .len()
                > MAX_CONTEXT_BYTES
            {
                context.selected.pop();
            }
            if context.selected.len() == MAX_SELECTED_FINDINGS {
                break;
            }
        }
        Ok(context)
    }
}

fn verify_artifact(
    store: &impl LibraryRepository,
    finding: &ResearchFinding,
    artifact: &LibraryArtifact,
) -> Result<(), RuntimeError> {
    let payload = finding.payload()?;
    let expected = ChunkerV1::chunk(&payload, LibraryMediaType::PlainText);
    if artifact.artifact_id != finding.library_id()?
        || artifact.content_digest != content_digest(payload.as_bytes())
        || artifact.source_uri.as_deref() != Some(finding.source_uri().as_str())
        || artifact.title.as_deref() != Some(finding.title.as_str())
        || artifact.media_type != LibraryMediaType::PlainText
        || artifact.chunker_version != ChunkerV1::VERSION
        || artifact.chunk_count as usize != expected.len()
    {
        return Err(invalid("stored finding metadata or payload digest differs"));
    }
    let chunks = store.chunks(artifact.artifact_id)?;
    if chunks.len() != expected.len()
        || chunks.iter().zip(&expected).any(|(chunk, draft)| {
            chunk.chunk_id.is_nil()
                || chunk.artifact_id != artifact.artifact_id
                || chunk.ordinal != draft.ordinal
                || chunk.char_offset != draft.char_offset
                || chunk.heading != draft.heading
                || chunk.text != draft.text
        })
    {
        return Err(invalid(
            "stored finding chunks differ from the reviewed payload",
        ));
    }
    Ok(())
}
