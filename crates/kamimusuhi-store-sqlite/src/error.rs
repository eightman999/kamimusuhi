//! Store-local error type. `kamimusuhi_core` error types (e.g.
//! `ContinuityError`) never expose `rusqlite` types directly (plan §3.1);
//! this type is the seam where SQLite-specific errors get converted into
//! the core's opaque error variants at the trait boundary.

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("required PRAGMA was not applied as expected: {0}")]
    PragmaNotApplied(String),

    #[error(
        "on-disk schema version {found} is newer than this build supports ({supported}); refusing to touch the database"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("migration '{name}' failed and was rolled back: {source}")]
    MigrationFailed {
        name: String,
        #[source]
        source: Box<StoreError>,
    },

    #[error("data invariant violated: {0}")]
    InvariantViolation(String),

    #[error("failpoint triggered: {0}")]
    Failpoint(String),

    #[error("id parse error: {0}")]
    IdParse(#[from] kamimusuhi_core::ids::IdParseError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("time parse error: {0}")]
    TimeParse(#[from] time::error::Parse),
}

impl StoreError {
    /// Whether this error represents a transient lock/busy condition
    /// rather than a data/identity conflict (plan §7.2 invariant 7:
    /// SQLite lock failure must never be misread as an identity
    /// conflict).
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            StoreError::Sqlite(rusqlite::Error::SqliteFailure(err, _))
                if matches!(
                    err.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    }
}

impl From<StoreError> for kamimusuhi_core::continuity::ContinuityError {
    fn from(err: StoreError) -> Self {
        if err.is_busy() {
            kamimusuhi_core::continuity::ContinuityError::Busy(err.to_string())
        } else {
            kamimusuhi_core::continuity::ContinuityError::Storage(err.to_string())
        }
    }
}
