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
//! **TLS comes from `rustls`.** Certificate validation, hostname verification
//! and the handshake are the library's job; this crate chooses trust anchors
//! and classifies failures, and contains no verification logic of its own.
//! Plain `http://` endpoints keep working unchanged. See [`http`] and [`tls`]
//! for the transport's remaining limits.

pub mod http;
pub mod openai;
pub mod tls;

pub use http::{Endpoint, Header, HttpError, HttpResponse};
pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleResource};
pub use tls::{TlsFailureKind, TrustAnchors};
