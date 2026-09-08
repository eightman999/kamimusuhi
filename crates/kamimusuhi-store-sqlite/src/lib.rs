//! SQLite single-writer canonical store.
//!
//! Adapter crate: implements the `kamimusuhi-core` storage contracts on top of
//! SQLite. No core public type exposes a `rusqlite` type.
//!
//! Durability policy (phase-1 plan §1.1): `foreign_keys = ON`,
//! `journal_mode = WAL`, `synchronous = FULL`, verified at open. Writer
//! transactions start with `BEGIN IMMEDIATE`. WAL is not a claim of complete
//! power-loss durability; the tests here cover transaction atomicity, crash
//! windows, retry idempotency and restart recovery.

mod continuity;
mod error;
mod evidence;
pub mod failpoints;
mod library;
mod memory;
pub mod migrations;
mod resources;
mod store;

pub use store::{SqliteStore, StoreConfig};
