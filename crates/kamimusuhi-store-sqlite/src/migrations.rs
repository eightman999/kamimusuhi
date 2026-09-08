//! Embedded, forward-only schema migrations.
//!
//! The runner refuses to touch a database whose recorded schema version is
//! newer than what this build understands (fail closed), applies each pending
//! migration in its own `BEGIN IMMEDIATE` transaction, and is idempotent.

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::ids::SchemaVersion;
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::map_sqlite;

struct Migration {
    version: SchemaVersion,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: SchemaVersion(1),
        name: "continuity",
        sql: include_str!("../migrations/0001_continuity.sql"),
    },
    Migration {
        version: SchemaVersion(2),
        name: "evidence_memory",
        sql: include_str!("../migrations/0002_evidence_memory.sql"),
    },
];

/// Newest schema this build can read and write.
pub const SUPPORTED_SCHEMA_VERSION: SchemaVersion = SchemaVersion(2);

const SCHEMA_VERSION_KEY: &str = "schema_version";

/// Schema version recorded in the database, or version 0 for an empty file.
pub fn current_version(conn: &Connection) -> Result<SchemaVersion, ContinuityError> {
    let has_meta: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'schema_meta'",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    if !has_meta {
        return Ok(SchemaVersion(0));
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = ?1",
            params![SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    match raw {
        None => Err(ContinuityError::Corrupt {
            detail: "schema_meta exists but has no schema_version row".to_owned(),
        }),
        Some(text) => {
            text.parse::<u32>()
                .map(SchemaVersion)
                .map_err(|_| ContinuityError::Corrupt {
                    detail: format!("schema_version {text:?} is not an integer"),
                })
        }
    }
}

/// Bring the database to [`SUPPORTED_SCHEMA_VERSION`].
pub fn migrate(conn: &mut Connection, now: UtcTimestamp) -> Result<SchemaVersion, ContinuityError> {
    let found = current_version(conn)?;
    if found > SUPPORTED_SCHEMA_VERSION {
        return Err(ContinuityError::SchemaVersionMismatch {
            found,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    for migration in MIGRATIONS.iter().filter(|m| m.version > found) {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        tx.execute_batch(migration.sql).map_err(map_sqlite)?;
        tx.execute(
            "INSERT INTO schema_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION_KEY, migration.version.0.to_string()],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "INSERT INTO schema_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![
                format!(
                    "migration_{:04}_{}_applied_at",
                    migration.version.0, migration.name
                ),
                now.unix_millis().to_string()
            ],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
    }

    let after = current_version(conn)?;
    if after != SUPPORTED_SCHEMA_VERSION {
        return Err(ContinuityError::Corrupt {
            detail: format!(
                "migrations ran but schema is {after}, expected {SUPPORTED_SCHEMA_VERSION}"
            ),
        });
    }
    Ok(after)
}
