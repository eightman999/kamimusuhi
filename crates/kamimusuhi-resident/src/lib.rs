//! Kamimusuhi resident node daemon.
//!
//! Runs on both the 24h continuity node (Raspberry Pi) and the daytime
//! cognition node (llm_master). It keeps the node observable, routes chat
//! requests across local / llm_master / HAI tiers, spools durable writes
//! locally and delivers them to the NAS, and on the continuity node
//! supervises K-CORE and snapshots the canonical store.
//!
//! It never initializes an individual and never writes canonical state.

pub mod approvals;
pub mod client;
pub mod config;
pub mod dialogue;
pub mod jobs;
pub mod kcore;
pub mod library;
pub mod mcp;
pub mod probes;
pub mod remote;
pub mod router;
pub mod server;
pub mod spool;
pub mod state;
pub mod status;
pub mod task_ledger;
pub mod task_orchestrator;
pub mod task_worktree;
pub mod tasks;
pub mod tools;
pub mod usage;
pub mod util;
