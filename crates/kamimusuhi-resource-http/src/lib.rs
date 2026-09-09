//! Generic OpenAI-compatible HTTP cognitive-resource adapter.
//!
//! Adapter crate: translates `kamimusuhi-core` resource requests into HTTP
//! calls against a configured base URL. It sits at the outermost edge of the
//! system, and the boundary it defends is that everything it returns is
//! *external material*:
//!
//! ```text
//! provider -> adapter -> ResourceResult -> workspace -> proposal -> policy -> canonical state
//! ```
//!
//! A provider can say anything it likes. It reaches durable state only by
//! becoming evidence and surviving the mutation policy, which refuses a
//! resource result as first-party testimony. No code here reads a reply
//! looking for instructions: authority is decided structurally, by where
//! something came from, never by what it says.
//!
//! Provider session state is never stored as identity, and secrets are never
//! written to the database, the config file or the trace — the bearer token is
//! read from the environment at call time and dropped with the request.
//!
//! **Scope:** plain HTTP/1.1 only. There is no TLS in this crate, so it reaches
//! local and in-cluster OpenAI-compatible servers but not `https://` endpoints.
//! See [`http`] for the rest of the transport's limits.

pub mod http;
pub mod openai;

pub use http::{Endpoint, Header, HttpError, HttpResponse};
pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleResource};
