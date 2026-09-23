//! Task plane: external agent harnesses (Devin, OpenCode, Command Code,
//! anything else with a command line) as the individual's tools.
//!
//! The chat plane answers people within its latency budget; the task plane
//! thinks, researches and writes code for as long as it takes. Chat never
//! waits for a task: delegating returns as soon as the task exists.
//!
//! Invariants:
//! * An external agent is a tool, not the individual. Its result is
//!   external evidence, never a belief, and it never writes canonical
//!   identity or memory.
//! * Models are discovered, not hard-coded, and a model is always
//!   addressed together with its harness (`executor:model`).
//! * Credentials never reach a prompt, log or evidence record; harnesses
//!   run with a minimal environment.
//! * Write permission is granted per task; read-only is the default.
//! * Unknown cost is not zero cost; metered models are never chosen
//!   automatically.
//!
//! Executors come from configuration ([`ExecutorConfig`]); adding or
//! removing a harness is a configuration change.

pub mod acp;
pub mod catalog;
pub mod commandcode;
pub mod config;
pub mod devin;
pub mod executor;
pub mod history;
pub mod opencode;
pub mod output;
pub mod policy;
pub mod process;
pub mod types;

pub use catalog::{CatalogEntry, ModelCatalog};
pub use config::{AdapterKind, ExecutorConfig, ExecutorSpec, Protocol, resolve_all};
pub use executor::{ExecutorRegistry, SupervisorStatus, TaskExecutor};
pub use policy::{Candidate, Selection, select};
pub use types::{
    AgentBilling, ExecutorHealth, ModelCapabilities, ModelDescriptor, ModelRef, Quota, RunEnd,
    TaskEvent, TaskKind, TaskOutcome, TaskPermissions, TaskRequest, TaskUsage,
};
