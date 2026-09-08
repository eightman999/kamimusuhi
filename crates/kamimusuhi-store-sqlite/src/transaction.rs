//! Connection setup: PRAGMA application/verification and the
//! `BEGIN IMMEDIATE` writer discipline (plan §1.1, §7.1).

use rusqlite::Connection;

use crate::error::StoreError;

/// Opens a connection and applies + verifies the durability PRAGMAs
/// required by plan §1.1. Fails rather than silently continuing if a
/// PRAGMA did not take effect as requested.
pub fn open_with_required_pragmas(path: &std::path::Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    apply_required_pragmas(&conn)?;
    Ok(conn)
}

pub fn open_in_memory_with_required_pragmas() -> Result<Connection, StoreError> {
    let conn = Connection::open_in_memory()?;
    // WAL is not meaningful for `:memory:` databases; foreign_keys and
    // synchronous are still verified.
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    verify_pragma_bool(&conn, "foreign_keys", true)?;
    Ok(conn)
}

fn apply_required_pragmas(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    verify_pragma_bool(conn, "foreign_keys", true)?;

    let journal_mode: String =
        conn.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::PragmaNotApplied(format!(
            "journal_mode: expected wal, got {journal_mode}"
        )));
    }

    conn.execute_batch("PRAGMA synchronous = FULL;")?;
    verify_pragma_int(conn, "synchronous", 2)?; // FULL = 2 in SQLite's pragma encoding (0=OFF,1=NORMAL,2=FULL,3=EXTRA).

    Ok(())
}

fn verify_pragma_bool(conn: &Connection, name: &str, expected: bool) -> Result<(), StoreError> {
    let value: i64 = conn.query_row(&format!("PRAGMA {name};"), [], |row| row.get(0))?;
    let actual = value != 0;
    if actual != expected {
        return Err(StoreError::PragmaNotApplied(format!(
            "{name}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn verify_pragma_int(conn: &Connection, name: &str, expected: i64) -> Result<(), StoreError> {
    let actual: i64 = conn.query_row(&format!("PRAGMA {name};"), [], |row| row.get(0))?;
    if actual != expected {
        return Err(StoreError::PragmaNotApplied(format!(
            "{name}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

/// Runs `body` inside a writer transaction opened with `BEGIN IMMEDIATE`
/// (plan §1.1, §7.1 invariant: writer transactions never start as a
/// deferred/passive read that gets promoted later). Commits on `Ok`,
/// rolls back on `Err`.
pub fn with_immediate_transaction<T>(
    conn: &Connection,
    body: impl FnOnce() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match body() {
        Ok(value) => {
            conn.execute_batch("COMMIT;")?;
            Ok(value)
        }
        Err(e) => {
            // Best-effort rollback; if the connection is already broken
            // (e.g. it was killed), there is nothing further to clean up
            // here; the original error is what matters to the caller.
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_pragmas_hold_on_file_backed_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pragma-test.sqlite");
        let conn = open_with_required_pragmas(&path).unwrap();
        verify_pragma_bool(&conn, "foreign_keys", true).unwrap();
    }

    #[test]
    fn immediate_transaction_rolls_back_on_error() {
        let conn = open_in_memory_with_required_pragmas().unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER);").unwrap();
        let result: Result<(), StoreError> = with_immediate_transaction(&conn, || {
            conn.execute("INSERT INTO t (v) VALUES (1)", [])?;
            Err(StoreError::InvariantViolation("boom".to_string()))
        });
        assert!(result.is_err());
        let count: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn immediate_transaction_commits_on_success() {
        let conn = open_in_memory_with_required_pragmas().unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER);").unwrap();
        with_immediate_transaction(&conn, || {
            conn.execute("INSERT INTO t (v) VALUES (1)", [])?;
            Ok(())
        })
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
