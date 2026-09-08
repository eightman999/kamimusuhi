//! Content digests.
//!
//! A digest is an integrity and correlation aid: it answers "is this the same
//! bytes as that", and nothing else. It is never an identity — two documents
//! with the same text are still two artifacts, and two requests with the same
//! shape are still two calls — and it is never an anonymisation or a deletion
//! substitute (plan §6.2, §6.4).

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`, prefixed with the algorithm so a stored
/// digest stays readable if the algorithm is ever changed.
pub fn content_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut hex = String::with_capacity(7 + 64);
    hex.push_str("sha256:");
    for byte in hasher.finalize() {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Digest of a JSON value in serde_json's canonical (key-sorted) rendering.
///
/// Used to correlate a request with its call record without storing the
/// request itself. Callers must not put secrets in the value: digesting is
/// not redaction.
pub fn json_digest(value: &serde_json::Value) -> String {
    content_digest(value.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_prefixed() {
        let a = content_digest(b"kamimusuhi");
        assert_eq!(a, content_digest(b"kamimusuhi"));
        assert!(a.starts_with("sha256:"));
        assert_eq!(a.len(), "sha256:".len() + 64);
        assert_ne!(a, content_digest(b"kamimusuh1"));
    }

    #[test]
    fn json_digest_ignores_key_order() {
        let a = serde_json::json!({ "b": 1, "a": 2 });
        let b = serde_json::json!({ "a": 2, "b": 1 });
        assert_eq!(json_digest(&a), json_digest(&b));
        assert_ne!(json_digest(&a), json_digest(&serde_json::json!({ "a": 2 })));
    }
}
