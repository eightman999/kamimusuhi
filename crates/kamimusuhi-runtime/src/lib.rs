//! Kamimusuhi runtime.
//!
//! The runtime is what turns the W0–W3 contracts into something that can be
//! started, stopped and started again. It owns no semantics of its own: every
//! canonical change still goes through the Continuity Kernel, every retrieval
//! still goes through the store contracts, and the runtime's job is to hold
//! the process lifetime together around them.
//!
//! Wave 4 provides three commands:
//!
//! ```text
//! init              prepare a runtime directory, and create an individual
//!                   only when the database holds none
//! inspect           read-only report; changes nothing
//! demo-continuity   the deterministic restart scenario, one phase per process
//! ```
//!
//! The property W4 exists to demonstrate is that a full process termination
//! loses nothing that matters. Process B is handed a directory and no
//! transcript, and rebuilds the individual from canonical state alone.

pub mod config;
pub mod error;
pub mod inspect;
pub mod runtime;
pub mod scenario;
pub mod trace;

pub use config::{ProviderConfig, ResourceImplementation, RuntimeConfig};
pub use error::RuntimeError;
pub use inspect::{InspectReport, inspect};
pub use runtime::{ClockMode, Runtime, RuntimeOptions, RuntimePaths};
pub use scenario::{DemoPhase, PhaseReport};
pub use trace::{JsonlTraceSink, TraceRecorder, read_trace};
