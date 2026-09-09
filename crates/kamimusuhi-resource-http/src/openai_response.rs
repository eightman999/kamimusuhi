//! Shared wire-error handling, not shared Persona/resource authority.
//! Provider fields are untrusted text even when named `code` or `type`.

/// Any non-null error envelope is a failure, including one without a code.
/// Only this closed vocabulary may cross into diagnostic records.
pub fn error_code(response: &serde_json::Value) -> Option<&'static str> {
    let error = response.get("error").filter(|error| !error.is_null())?;
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .or_else(|| error.get("type").and_then(serde_json::Value::as_str));
    Some(match code {
        Some("context_length_exceeded") => "context_length_exceeded",
        Some("invalid_api_key") => "invalid_api_key",
        Some("model_not_found") => "model_not_found",
        Some("rate_limit_exceeded") => "rate_limit_exceeded",
        Some("insufficient_quota") => "insufficient_quota",
        Some("server_error") => "server_error",
        Some("invalid_request_error") => "invalid_request_error",
        Some("authentication_error") => "authentication_error",
        Some("overloaded") => "overloaded",
        _ => "provider_error",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_closed_vocabulary_reaches_diagnostics() {
        for field in ["code", "type"] {
            let mut error = serde_json::Map::new();
            error.insert(
                field.to_owned(),
                serde_json::json!("PRIVATE_PROMPT_OR_TOKEN"),
            );
            assert_eq!(
                error_code(&serde_json::json!({"error": error})),
                Some("provider_error")
            );
        }
        assert_eq!(
            error_code(&serde_json::json!({"error":{"code":"context_length_exceeded"}})),
            Some("context_length_exceeded")
        );
    }

    #[test]
    fn every_non_null_error_is_a_failure_even_with_choices() {
        for error in [
            serde_json::json!({}),
            serde_json::json!("secret"),
            serde_json::json!(false),
            serde_json::json!(42),
        ] {
            assert_eq!(
                error_code(
                    &serde_json::json!({"error": error, "choices": [{"message":{"content":"not success"}}]})
                ),
                Some("provider_error")
            );
        }
        assert_eq!(error_code(&serde_json::json!({"error": null})), None);
        assert_eq!(error_code(&serde_json::json!({"choices": []})), None);
    }
}
