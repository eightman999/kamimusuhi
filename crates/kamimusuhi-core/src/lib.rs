//! Provider-neutral contracts for Kamimusuhi.
//!
//! This crate defines the domain types and trait boundaries that every other
//! crate implements or consumes. It deliberately knows nothing about SQLite,
//! HTTP, model providers, accelerators or speech stacks: those live in adapter
//! crates that depend on this one, never the other way around.
