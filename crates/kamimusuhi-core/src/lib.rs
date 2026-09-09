//! Provider-neutral contracts for Kamimusuhi.
//!
//! This crate defines the domain types and trait boundaries that every other
//! crate implements or consumes. It deliberately knows nothing about SQLite,
//! HTTP, model providers, accelerators or speech stacks: those live in adapter
//! crates that depend on this one, never the other way around.
//!
//! Naming policy:
//! - Domain identifiers are newtypes (`IndividualId`, `TurnId`, ...) and are
//!   never passed around as bare `String`s.
//! - Each bounded domain exposes its own error enum (`PersonaError`, ...).
//!   Errors carry structured variants, not stringly-typed messages, except for
//!   an explicit `Backend` escape hatch used by adapters.
//! - Contracts are synchronous. Async runtimes and worker scheduling belong to
//!   the runtime layer, outside the canonical domain interface.

pub mod audit;
pub mod continuity;
pub mod digest;
pub mod domain_separation;
pub mod evidence;
pub mod ids;
pub mod library;
pub mod memory;
pub mod mutation;
pub mod persona;
pub mod persona_seed;
pub mod resources;
pub mod routing;
pub mod time;
pub mod trace;
pub mod workspace;
