//! `kamimusuhi-core`: pure domain contracts.
//!
//! This crate MUST NOT depend on SQLite, HTTP, any model provider SDK, a
//! GPU backend, a vector database, or TTS/ASR (plan §3.1). Its only
//! dependencies are pure value/serialization/error crates. Adapters
//! (`kamimusuhi-store-sqlite`, `kamimusuhi-resource-http`,
//! `kamimusuhi-testkit`, `kamimusuhi-runtime`) depend on this crate, never
//! the other way around.

pub mod audit;
pub mod continuity;
pub mod evidence;
pub mod ids;
pub mod library;
pub mod memory;
pub mod mutation;
pub mod persona;
pub mod policy;
pub mod resources;
pub mod runtime;
pub mod time;
pub mod workspace;
