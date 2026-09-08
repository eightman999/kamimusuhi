//! Generic OpenAI-compatible HTTP cognitive-resource adapter.
//!
//! Adapter crate: translates `kamimusuhi-core` resource requests into HTTP
//! calls against a configured base URL. Provider session state is never
//! stored as identity; secrets are never written to DB or trace. The adapter
//! itself is implemented in Wave 5.
