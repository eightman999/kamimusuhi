//! Restart / recovery helpers used by `kamimusuhi-runtime`'s `inspect`
//! command and by the restart integration tests. Opening a
//! [`crate::store::SqliteContinuityStore`] against an existing database
//! file already performs full recovery (PRAGMA re-verification +
//! migration-is-a-no-op); this module adds read-only introspection over
//! that recovered state without requiring callers to know the schema.

use rusqlite::{params, Connection};

use kamimusuhi_core::continuity::Individual;
use kamimusuhi_core::ids::CommitId;
use kamimusuhi_core::time::UtcTimestamp;

use crate::error::StoreError;

/// One row of `individuals`, for `inspect`/tests. A thin read model kept
/// separate from `kamimusuhi_core::continuity::Individual` only in that
/// it is produced straight from SQL without going through the store's
/// writer path.
pub fn list_individuals(conn: &Connection) -> Result<Vec<Individual>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT individual_id, created_at, root_commit_id FROM individuals ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |row| {
        let individual_id: String = row.get(0)?;
        let created_at: String = row.get(1)?;
        let root_commit_id: String = row.get(2)?;
        Ok((individual_id, created_at, root_commit_id))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (individual_id, created_at, root_commit_id) = row?;
        out.push(Individual {
            individual_id: individual_id.parse()?,
            created_at: UtcTimestamp::parse_rfc3339(&created_at)?,
            root_commit_id: root_commit_id.parse()?,
        });
    }
    Ok(out)
}

/// Total number of canonical commits recorded for one individual's
/// lineage, independent of the current head (used by tests/inspect to
/// sanity-check lineage length after restart).
pub fn count_commits_for_individual(
    conn: &Connection,
    individual_id: kamimusuhi_core::ids::IndividualId,
) -> Result<u64, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM canonical_commits WHERE individual_id = ?1",
        params![individual_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(count as u64)
}

/// Fetches one commit's predecessor chain length back to the root
/// (inclusive), purely for diagnostic/inspection use.
pub fn commit_exists(conn: &Connection, commit_id: CommitId) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM canonical_commits WHERE commit_id = ?1",
        params![commit_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
