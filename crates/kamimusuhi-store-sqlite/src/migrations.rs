//! Schema migration runner.
//!
//! `schema_meta` is bootstrap infrastructure, created by this runner (not
//! by a versioned migration file) before any versioned migration is
//! considered. Versioned migrations are embedded at compile time via
//! `include_str!` and applied in ascending order, each inside its own
//! transaction, bumping `schema_meta.schema_version` as it goes. Once a
//! migration file has shipped it is never edited; schema changes are new
//! migration files appended to this list (plan §21 footer, §6).

use rusqlite::Connection;

use crate::error::StoreError;

/// One versioned migration. `version` is the schema version *after*
/// applying `sql`.
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_continuity",
    sql: include_str!("../migrations/0001_continuity.sql"),
}];

/// The highest schema version this build of the crate knows how to
/// create/read. If an on-disk database reports a *higher* version than
/// this, we refuse to touch it rather than guess (plan §15.3 T25 / §20
/// "fail closed").
pub const SUPPORTED_SCHEMA_VERSION: u32 = match MIGRATIONS.last() {
    Some(m) => m.version,
    None => 0,
};

fn ensure_schema_meta_table(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            schema_version INTEGER NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn current_schema_version(conn: &Connection) -> Result<u32, StoreError> {
    ensure_schema_meta_table(conn)?;
    let version: Option<u32> = conn
        .query_row(
            "SELECT schema_version FROM schema_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(version.unwrap_or(0))
}

/// Applies every migration with `version > current`, in ascending order,
/// each within its own transaction. Idempotent: calling this again on an
/// already-migrated database is a no-op. Fails closed if the on-disk
/// version is newer than anything this build knows about.
pub fn migrate(conn: &Connection) -> Result<u32, StoreError> {
    let mut current = current_schema_version(conn)?;

    if current > SUPPORTED_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: current,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    for migration in MIGRATIONS {
        if migration.version <= current {
            continue;
        }
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<(), StoreError> {
            conn.execute_batch(migration.sql)?;
            conn.execute(
                "INSERT INTO schema_meta (id, schema_version, applied_at)
                 VALUES (1, ?1, datetime('now'))
                 ON CONFLICT(id) DO UPDATE SET schema_version = excluded.schema_version,
                                                applied_at = excluded.applied_at",
                rusqlite::params![migration.version],
            )?;
            Ok(())
        })();
        match applied {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
                current = migration.version;
            }
            Err(e) => {
                conn.execute_batch("ROLLBACK;")?;
                return Err(StoreError::MigrationFailed {
                    name: migration.name.to_string(),
                    source: Box::new(e),
                });
            }
        }
    }

    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn migrate_from_empty_db_reaches_supported_version() {
        let conn = open_memory();
        let version = migrate(&conn).unwrap();
        assert_eq!(version, SUPPORTED_SCHEMA_VERSION);

        // Core tables exist.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='individuals'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_memory();
        let first = migrate(&conn).unwrap();
        let second = migrate(&conn).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn future_schema_version_is_refused() {
        let conn = open_memory();
        migrate(&conn).unwrap();
        conn.execute(
            "UPDATE schema_meta SET schema_version = ?1 WHERE id = 1",
            rusqlite::params![SUPPORTED_SCHEMA_VERSION + 1],
        )
        .unwrap();

        let result = migrate(&conn);
        assert!(matches!(
            result,
            Err(StoreError::UnsupportedSchemaVersion { .. })
        ));
    }
}
