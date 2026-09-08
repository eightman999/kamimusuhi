//! Mapping from SQLite failures to core errors.
//!
//! Lock contention becomes [`ContinuityError::Contended`] so callers never
//! mistake a busy database for an identity conflict; everything else is an
//! opaque backend error whose outcome must not be guessed.

use kamimusuhi_core::continuity::ContinuityError;
use rusqlite::ErrorCode;

pub(crate) fn map_sqlite(error: rusqlite::Error) -> ContinuityError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) =>
        {
            ContinuityError::Contended {
                detail: error.to_string(),
            }
        }
        _ => ContinuityError::Backend {
            message: error.to_string(),
        },
    }
}
