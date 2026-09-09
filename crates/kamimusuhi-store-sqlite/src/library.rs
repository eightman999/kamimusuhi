//! `LibraryRepository` on SQLite (plan §6.4, §11.2).
//!
//! Import chunks a document with the deterministic chunker and writes the
//! artifact and its spans in one transaction. Nothing here touches
//! `continuity_heads`, `state_records` or `individuals`: the Library is not
//! part of any individual's canonical state, and a retrieval is a read.

use std::str::FromStr;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::{LibraryArtifactId, LibraryChunkId};
use kamimusuhi_core::library::{
    ChunkerV1, LibraryArtifact, LibraryChunk, LibraryError, LibraryHit, LibraryMediaType,
    LibraryQuery, LibraryRepository, NewLibraryArtifact, score_chunk,
};
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::store::SqliteStore;

fn corrupt(detail: impl Into<String>) -> LibraryError {
    LibraryError::Corrupt {
        detail: detail.into(),
    }
}

fn invalid(reason: impl Into<String>) -> LibraryError {
    LibraryError::Invalid {
        reason: reason.into(),
    }
}

fn map_sqlite(error: rusqlite::Error) -> LibraryError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            LibraryError::Contended {
                detail: error.to_string(),
            }
        }
        _ => LibraryError::Backend {
            message: error.to_string(),
        },
    }
}

fn parse<T: FromStr>(field: &str, text: &str) -> Result<T, LibraryError>
where
    T::Err: std::fmt::Display,
{
    text.parse::<T>()
        .map_err(|e| corrupt(format!("column {field} holds {text:?}: {e}")))
}

fn u32_from(value: i64, field: &str) -> Result<u32, LibraryError> {
    u32::try_from(value).map_err(|_| corrupt(format!("column {field} holds {value}")))
}

const ARTIFACT_COLUMNS: &str = "artifact_id, source_uri, title, media_type, content_digest, \
     chunker_version, chunk_count, imported_at";

type ArtifactRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    i64,
    i64,
    i64,
);

fn read_artifact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn build_artifact(row: ArtifactRow) -> Result<LibraryArtifact, LibraryError> {
    let (
        artifact_id,
        source_uri,
        title,
        media_type,
        content_digest,
        chunker_version,
        chunk_count,
        imported_at,
    ) = row;
    Ok(LibraryArtifact {
        artifact_id: parse("library_artifacts.artifact_id", &artifact_id)?,
        source_uri,
        title,
        media_type: parse::<LibraryMediaType>("library_artifacts.media_type", &media_type)?,
        content_digest,
        chunker_version: kamimusuhi_core::library::ChunkerVersion(u32_from(
            chunker_version,
            "library_artifacts.chunker_version",
        )?),
        chunk_count: u32_from(chunk_count, "library_artifacts.chunk_count")?,
        imported_at: UtcTimestamp::from_unix_millis(imported_at),
    })
}

fn get_artifact_in(
    conn: &Connection,
    artifact_id: LibraryArtifactId,
) -> Result<Option<LibraryArtifact>, LibraryError> {
    conn.query_row(
        &format!("SELECT {ARTIFACT_COLUMNS} FROM library_artifacts WHERE artifact_id = ?1"),
        params![artifact_id.to_string()],
        read_artifact_row,
    )
    .optional()
    .map_err(map_sqlite)?
    .map(build_artifact)
    .transpose()
}

const CHUNK_COLUMNS: &str = "chunk_id, artifact_id, ordinal, heading, char_offset, text";

fn build_chunk(
    row: (String, String, i64, Option<String>, i64, String),
) -> Result<LibraryChunk, LibraryError> {
    let (chunk_id, artifact_id, ordinal, heading, char_offset, text) = row;
    Ok(LibraryChunk {
        chunk_id: parse("library_chunks.chunk_id", &chunk_id)?,
        artifact_id: parse("library_chunks.artifact_id", &artifact_id)?,
        ordinal: u32_from(ordinal, "library_chunks.ordinal")?,
        heading,
        char_offset: u32_from(char_offset, "library_chunks.char_offset")?,
        text,
    })
}

fn read_chunk_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, i64, Option<String>, i64, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

impl LibraryRepository for SqliteStore {
    fn import(&self, artifact: NewLibraryArtifact) -> Result<LibraryArtifact, LibraryError> {
        if artifact.artifact_id.is_nil() {
            return Err(invalid("library artifact has a nil id"));
        }
        let digest = content_digest(artifact.content.as_bytes());
        let drafts = ChunkerV1::chunk(&artifact.content, artifact.media_type);
        let now = self.clock().now_utc();

        let stored = LibraryArtifact {
            artifact_id: artifact.artifact_id,
            source_uri: artifact.source_uri,
            title: artifact.title,
            media_type: artifact.media_type,
            content_digest: digest,
            chunker_version: ChunkerV1::VERSION,
            chunk_count: u32::try_from(drafts.len())
                .map_err(|_| invalid("document produced more chunks than can be counted"))?,
            imported_at: now,
        };

        let mut conn = self.conn().map_err(|e| LibraryError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;

        // Identity is the artifact ID. A byte-identical re-import under the
        // same ID is a retry and is a no-op; different content under the same
        // ID is a conflict, because a revised document is a new artifact, not
        // an overwrite of the one already cited by chunk ID.
        if let Some(existing) = get_artifact_in(&tx, artifact.artifact_id)? {
            let same = existing.content_digest == stored.content_digest
                && existing.source_uri == stored.source_uri
                && existing.title == stored.title
                && existing.media_type == stored.media_type
                && existing.chunker_version == stored.chunker_version
                && existing.chunk_count == stored.chunk_count;
            return if same {
                Ok(existing)
            } else {
                Err(LibraryError::AlreadyExists(artifact.artifact_id))
            };
        }

        tx.execute(
            &format!(
                "INSERT INTO library_artifacts({ARTIFACT_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                stored.artifact_id.to_string(),
                stored.source_uri,
                stored.title,
                stored.media_type.as_str(),
                stored.content_digest,
                i64::from(stored.chunker_version.0),
                i64::from(stored.chunk_count),
                stored.imported_at.unix_millis(),
            ],
        )
        .map_err(map_sqlite)?;

        for draft in drafts {
            let chunk_id = LibraryChunkId::generate(self.ids());
            tx.execute(
                &format!(
                    "INSERT INTO library_chunks({CHUNK_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                ),
                params![
                    chunk_id.to_string(),
                    stored.artifact_id.to_string(),
                    i64::from(draft.ordinal),
                    draft.heading,
                    i64::from(draft.char_offset),
                    draft.text,
                ],
            )
            .map_err(map_sqlite)?;
        }
        tx.commit().map_err(map_sqlite)?;

        get_artifact_in(&conn, artifact.artifact_id)?
            .ok_or_else(|| corrupt("imported artifact is missing right after commit"))
    }

    fn get_artifact(
        &self,
        artifact_id: LibraryArtifactId,
    ) -> Result<Option<LibraryArtifact>, LibraryError> {
        let conn = self.conn().map_err(|e| LibraryError::Backend {
            message: e.to_string(),
        })?;
        get_artifact_in(&conn, artifact_id)
    }

    fn artifacts(&self) -> Result<Vec<LibraryArtifact>, LibraryError> {
        let conn = self.conn().map_err(|e| LibraryError::Backend {
            message: e.to_string(),
        })?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ARTIFACT_COLUMNS} FROM library_artifacts
                 ORDER BY imported_at, artifact_id"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], read_artifact_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter().map(build_artifact).collect()
    }

    fn chunks(&self, artifact_id: LibraryArtifactId) -> Result<Vec<LibraryChunk>, LibraryError> {
        let conn = self.conn().map_err(|e| LibraryError::Backend {
            message: e.to_string(),
        })?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CHUNK_COLUMNS} FROM library_chunks
                 WHERE artifact_id = ?1 ORDER BY ordinal"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![artifact_id.to_string()], read_chunk_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter().map(build_chunk).collect()
    }

    fn retrieve(&self, query: &LibraryQuery) -> Result<Vec<LibraryHit>, LibraryError> {
        let terms = query.terms();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn().map_err(|e| LibraryError::Backend {
            message: e.to_string(),
        })?;

        // Scoring lives in the core contract, so the SQL only has to produce a
        // stable candidate order: (artifact, ordinal) breaks every tie.
        let mut sql = format!("SELECT {CHUNK_COLUMNS} FROM library_chunks");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(artifact_id) = query.artifact_id {
            args.push(Box::new(artifact_id.to_string()));
            sql.push_str(" WHERE artifact_id = ?1");
        }
        sql.push_str(" ORDER BY artifact_id, ordinal");

        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt
            .query_map(params.as_slice(), read_chunk_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;

        let mut hits: Vec<LibraryHit> = Vec::new();
        for row in rows {
            let chunk = build_chunk(row)?;
            let matched_terms = score_chunk(&chunk.text, &terms);
            if matched_terms == 0 {
                continue;
            }
            let artifact = get_artifact_in(&conn, chunk.artifact_id)?.ok_or_else(|| {
                corrupt(format!(
                    "chunk {} references missing artifact {}",
                    chunk.chunk_id, chunk.artifact_id
                ))
            })?;
            hits.push(LibraryHit {
                chunk,
                artifact,
                matched_terms,
            });
        }

        // Best match first; the candidate order already breaks ties, and a
        // stable sort preserves it.
        hits.sort_by(|a, b| b.matched_terms.cmp(&a.matched_terms));
        if let Some(limit) = query.limit {
            hits.truncate(limit);
        }
        Ok(hits)
    }
}
