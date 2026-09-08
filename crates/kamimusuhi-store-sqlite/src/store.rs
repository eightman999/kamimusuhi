//! Connection lifecycle and durability policy.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::ids::{IdGenerator, SchemaVersion};
use kamimusuhi_core::time::Clock;
use rusqlite::Connection;

use crate::error::map_sqlite;
use crate::failpoints::{Failpoint, FailpointHook};
use crate::migrations;

/// Runtime-tunable knobs. None of these change identity semantics.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// How long a writer waits for the SQLite lock before reporting
    /// `ContinuityError::Contended`.
    pub busy_timeout: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(5),
        }
    }
}

/// Single-writer SQLite canonical store.
///
/// One connection guarded by a mutex: every canonical write goes through one
/// serialized path, and `BEGIN IMMEDIATE` takes the SQLite write lock up
/// front so a second process cannot interleave.
pub struct SqliteStore {
    conn: Mutex<Connection>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    schema_version: SchemaVersion,
    failpoint_hook: Option<Arc<dyn FailpointHook>>,
}

impl fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteStore")
            .field("schema_version", &self.schema_version)
            .field("failpoint_hook", &self.failpoint_hook.is_some())
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Open (creating if needed) and migrate a database file.
    ///
    /// Fails closed if the file records a schema newer than this build, if the
    /// required pragmas cannot be verified, or if the file is not a real file
    /// (in-memory databases cannot provide WAL durability).
    pub fn open(
        path: impl AsRef<Path>,
        config: &StoreConfig,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Result<Self, ContinuityError> {
        let mut conn = Connection::open(path).map_err(map_sqlite)?;
        conn.busy_timeout(config.busy_timeout).map_err(map_sqlite)?;
        apply_and_verify_pragmas(&conn)?;
        let schema_version = migrations::migrate(&mut conn, clock.now_utc())?;
        Ok(Self {
            conn: Mutex::new(conn),
            clock,
            ids,
            schema_version,
            failpoint_hook: None,
        })
    }

    /// Test harness only: inject failures/crashes at fixed points of the
    /// activation transaction. Production code never sets a hook.
    pub fn with_failpoint_hook(mut self, hook: Arc<dyn FailpointHook>) -> Self {
        self.failpoint_hook = Some(hook);
        self
    }

    pub fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    /// Names of user tables, sorted. For inspection and schema tests.
    pub fn table_names(&self) -> Result<Vec<String>, ContinuityError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(map_sqlite)?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(names)
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub(crate) fn ids(&self) -> &dyn IdGenerator {
        self.ids.as_ref()
    }

    pub(crate) fn conn(&self) -> Result<MutexGuard<'_, Connection>, ContinuityError> {
        self.conn.lock().map_err(|_| ContinuityError::Backend {
            message: "store mutex poisoned by an earlier panic".to_owned(),
        })
    }

    pub(crate) fn failpoint(&self, point: Failpoint) -> Result<(), ContinuityError> {
        match &self.failpoint_hook {
            Some(hook) => hook
                .hit(point)
                .map_err(|triggered| ContinuityError::Backend {
                    message: triggered.to_string(),
                }),
            None => Ok(()),
        }
    }
}

fn apply_and_verify_pragmas(conn: &Connection) -> Result<(), ContinuityError> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;",
    )
    .map_err(map_sqlite)?;

    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .map_err(map_sqlite)?;
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .map_err(map_sqlite)?;
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .map_err(map_sqlite)?;

    let mut problems = Vec::new();
    if foreign_keys != 1 {
        problems.push("foreign_keys is not ON".to_owned());
    }
    if !journal_mode.eq_ignore_ascii_case("wal") {
        problems.push(format!("journal_mode is {journal_mode:?}, expected wal"));
    }
    // 2 = FULL, 3 = EXTRA; both satisfy the phase-1 policy.
    if synchronous < 2 {
        problems.push(format!("synchronous is {synchronous}, expected FULL"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ContinuityError::Backend {
            message: format!("durability pragmas not satisfied: {}", problems.join("; ")),
        })
    }
}
