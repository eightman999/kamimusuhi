//! SQLite single-writer canonical store.
//!
//! Adapter crate: implements the `kamimusuhi-core` storage contracts on top of
//! SQLite. No core public type exposes a `rusqlite` type. Schema, migrations
//! and the continuity transaction are added in Wave 1.
