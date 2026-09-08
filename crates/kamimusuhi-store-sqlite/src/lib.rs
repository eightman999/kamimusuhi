//! `kamimusuhi-store-sqlite`: the SQLite adapter for
//! `kamimusuhi_core::continuity::ContinuityStore`.
//!
//! This is the only crate in the workspace allowed to depend on
//! `rusqlite` (plan §3.1). `kamimusuhi-core` public types never leak
//! `rusqlite::Row`/`Connection` through this crate's public API.

pub mod error;
pub mod failpoint;
pub mod migrations;
pub mod recovery;
pub mod store;
pub mod transaction;

pub use error::StoreError;
pub use store::SqliteContinuityStore;
