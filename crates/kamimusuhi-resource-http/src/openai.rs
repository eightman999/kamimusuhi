//! OpenAI-compatible chat-completions adapter.
//!
//! Turns a [`ResourceRequest`] into one HTTP call and the reply into a
//! [`ResourceResult`], classifying every way that can go wrong. Two boundaries
//! are load-bearing:
//!
//! **Retry lives here, not in the registry.** A retried call is still one
//! logical call: one `resource_calls` row, one attribution, one thing the
//! workspace can cite. If the registry retried, one question would become
//! several records and "which resource produced this" would stop being a
//! single fact. The adapter reports how many physical attempts it made and
//! the row counts them.
//!
//! **Whatever the provider says is external material.** A reply telling
//! Kamimusuhi to remember something is text, exactly like a Library excerpt:
//! it reaches the workspace as `EXTERNAL_RESOURCE_RESULT` and can only affect
//! durable state by going through evidence, a proposal and the policy — which
//! will refuse it, because a resource result is not first-party testimony.
//! Nothing in this file inspects the reply for instructions, because deciding
//! authority by reading content is the failure mode, not the defence.
//!
//! Secrets: the bearer token is read from the environment at call time and
//! never stored, echoed into an error, or written to any record.

use std::time::{Duration, Instant};

use kamimusuhi_core::ids::ResourceId;
use kamimusuhi_core::resources::{
    CognitiveResource, ResourceDescriptor, ResourceError, ResourceKind, ResourceRequest,
    ResourceResult,
};
use kamimusuhi_core::routing::{
    CostClass, HealthState, LatencyClass, LocalityClass, Modality, Precedence, QualityTier,
    ResourceCapabilities,
};

use crate::http::{Endpoint, Header, HttpError, HttpResponse, post_json};
use crate::tls::TrustAnchors;

/// Non-secret configuration of one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleConfig {
    pub resource_id: ResourceId,
    pub base_url: String,
    pub model: String,
    /// Name of the environment variable holding the bearer token, never the
    /// token itself.
    pub auth_env: Option<String>,
    /// Deadline for the whole logical call, retries included.
    pub timeout_ms: u64,
    pub max_attempts: u32,
    pub retry_backoff_ms: u64,
    /// Which trust anchors verify an `https://` endpoint. Ignored for plain
    /// HTTP. There is no variant that disables verification.
    pub trust_anchors: TrustAnchors,
    /// Declared capabilities, for routing. Operator-asserted configuration:
    /// nothing here is measured, and a provider does not get to describe
    /// itself.
    pub capabilities: ResourceCapabilities,
}

impl OpenAiCompatibleConfig {
    pub fn new(
        resource_id: ResourceId,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            resource_id,
            base_url: base_url.into(),
            model: model.into(),
            auth_env: None,
            timeout_ms: 30_000,
            max_attempts: 1,
            retry_backoff_ms: 200,
            trust_anchors: TrustAnchors::default(),
            capabilities: ResourceCapabilities {
                locality: LocalityClass::External,
                precedence: Precedence::Ordinary,
                modalities: [Modality::Text].into_iter().collect(),
                context_capacity: 8_192,
                latency: LatencyClass::Fast,
                cost: CostClass::Low,
                quality: QualityTier::Standard,
                health: HealthState::Healthy,
            },
        }
    }

    #[must_use]
    pub fn with_trust_anchors(mut self, trust_anchors: TrustAnchors) -> Self {
        self.trust_anchors = trust_anchors;
        self
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: ResourceCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    #[must_use]
    pub const fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    #[must_use]
    pub const fn with_retry_backoff_ms(mut self, retry_backoff_ms: u64) -> Self {
        self.retry_backoff_ms = retry_backoff_ms;
        self
    }

    #[must_use]
    pub fn with_auth_env(mut self, auth_env: Option<String>) -> Self {
        self.auth_env = auth_env;
        self
    }

    /// Check what can be checked before the first call, so a typo in a base
    /// URL is a configuration error rather than a runtime failure per turn.
    pub fn validate(&self) -> Result<(), String> {
        if self.resource_id.is_nil() {
            return Err("resource_id is nil".to_owned());
        }
        if self.model.trim().is_empty() {
            return Err("model is empty".to_owned());
        }
        if self.timeout_ms == 0 {
            return Err("timeout_ms is zero".to_owned());
        }
        if self.max_attempts == 0 {
            return Err("max_attempts is zero".to_owned());
        }
        self.endpoint().map(|_| ())
    }

    fn endpoint(&self) -> Result<Endpoint, String> {
        Endpoint::parse(&self.base_url, "/chat/completions")
    }
}

/// A cognitive resource backed by an OpenAI-compatible HTTP endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleResource {
    config: OpenAiCompatibleConfig,
}

impl OpenAiCompatibleResource {
    pub const fn new(config: OpenAiCompatibleConfig) -> Self {
        Self { config }
    }

    pub const fn config(&self) -> &OpenAiCompatibleConfig {
        &self.config
    }

    /// Headers for one attempt. The token is read here and dropped with the
    /// request; it exists nowhere else.
    fn headers(&self) -> Result<Vec<Header>, ResourceError> {
        let Some(name) = &self.config.auth_env else {
            return Ok(Vec::new());
        };
        let token = std::env::var(name).map_err(|_| ResourceError::InvalidRequest {
            // Names the variable, never a value.
            reason: format!("environment variable {name} is not set"),
        })?;
        if token.trim().is_empty() {
            return Err(ResourceError::InvalidRequest {
                reason: format!("environment variable {name} is empty"),
            });
        }
        Ok(vec![Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {token}"),
        }])
    }

    fn request_body(&self, request: &ResourceRequest) -> String {
        // The input is passed as a JSON string in one user message: the
        // adapter's job is transport, not prompt design.
        let content = match &request.input {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        serde_json::json!({
            "model": self.config.model,
            "messages": [{ "role": "user", "content": content }],
        })
        .to_string()
    }

    /// Whether another attempt could plausibly do better.
    ///
    /// Retrying a rejected credential or a malformed request just repeats the
    /// rejection; retrying congestion or a broken connection might not.
    const fn is_retryable(error: &ResourceError) -> bool {
        // TLS failures are absent on purpose: a certificate that does not
        // validate will not validate on the next attempt either, and retrying
        // would turn a clear security signal into a slow one.
        matches!(
            error,
            ResourceError::Timeout { .. }
                | ResourceError::Transport { .. }
                | ResourceError::RateLimited { .. }
                | ResourceError::HttpStatus { .. }
        )
    }
}

impl CognitiveResource for OpenAiCompatibleResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor {
            resource_id: self.config.resource_id,
            name: self.config.model.clone(),
            kind: ResourceKind::Generation,
            adapter: "openai-compatible".to_owned(),
            version: "1".to_owned(),
            // Phase 1 resources only produce material. Acting on the world
            // needs an Executor and an authorization path that does not exist.
            read_only: true,
            capabilities: self.config.capabilities.clone(),
        }
    }

    fn invoke(&self, request: &ResourceRequest) -> Result<ResourceResult, ResourceError> {
        self.config
            .validate()
            .map_err(|reason| ResourceError::InvalidRequest { reason })?;
        let endpoint = self
            .config
            .endpoint()
            .map_err(|reason| ResourceError::InvalidRequest { reason })?;
        let headers = self.headers()?;
        let body = self.request_body(request);
        let timeout = Duration::from_millis(self.config.timeout_ms);

        let started = Instant::now();
        let mut attempt = 0_u32;
        let deadline_error = |attempts| ResourceError::Timeout {
            resource_id: self.config.resource_id,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            attempts,
        };
        loop {
            let remaining = timeout
                .checked_sub(started.elapsed())
                .filter(|left| !left.is_zero())
                .ok_or_else(|| deadline_error(attempt))?;
            attempt += 1;
            let outcome = post_json(
                &endpoint,
                &body,
                &headers,
                remaining,
                &self.config.trust_anchors,
            )
            .map_err(|error| self.map_transport(error, attempt))
            .and_then(|response| self.map_response(response, attempt));

            match outcome {
                Ok(content) => {
                    return Ok(ResourceResult::new(self.config.resource_id, content)
                        .with_attempts(attempt));
                }
                Err(error) => {
                    // Retrying is this adapter's decision and stays inside this
                    // one logical call.
                    if attempt >= self.config.max_attempts || !Self::is_retryable(&error) {
                        return Err(error);
                    }
                    let backoff = Duration::from_millis(self.config.retry_backoff_ms);
                    let remaining = timeout
                        .checked_sub(started.elapsed())
                        .unwrap_or(Duration::ZERO);
                    if backoff >= remaining {
                        return Err(deadline_error(attempt));
                    }
                    if !backoff.is_zero() {
                        std::thread::sleep(backoff);
                    }
                }
            }
        }
    }
}

impl OpenAiCompatibleResource {
    fn map_transport(&self, error: HttpError, attempts: u32) -> ResourceError {
        let resource_id = self.config.resource_id;
        match error {
            HttpError::InvalidRequest(reason) => ResourceError::InvalidRequest { reason },
            HttpError::Timeout { elapsed_ms, .. } => ResourceError::Timeout {
                resource_id,
                elapsed_ms,
                attempts,
            },
            HttpError::Transport(message) => ResourceError::Transport {
                resource_id,
                attempts,
                message,
            },
            HttpError::Malformed(detail) => ResourceError::MalformedResponse {
                resource_id,
                detail,
                attempts,
            },
            HttpError::Tls { kind, detail } => ResourceError::Tls {
                resource_id,
                kind: kind.as_str().to_owned(),
                detail,
                attempts,
            },
        }
    }

    /// Classify the reply and extract the message content.
    fn map_response(
        &self,
        response: HttpResponse,
        attempts: u32,
    ) -> Result<serde_json::Value, ResourceError> {
        let resource_id = self.config.resource_id;
        let status = response.status;
        if !response.is_success() {
            // The body is not carried into the error: it may echo the prompt
            // back, and an operator needs the class, not the content.
            return Err(match status {
                401 | 403 => ResourceError::Authentication {
                    resource_id,
                    status,
                },
                429 => ResourceError::RateLimited {
                    resource_id,
                    status,
                    attempts,
                },
                _ => ResourceError::HttpStatus {
                    resource_id,
                    status,
                    attempts,
                },
            });
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&response.body).map_err(|_| ResourceError::MalformedResponse {
                resource_id,
                detail: "response body is not valid JSON".to_owned(),
                attempts,
            })?;

        // A 200 carrying the provider's own error object is a provider error,
        // not a success with odd content.
        if let Some(code) = crate::openai_response::error_code(&parsed) {
            return Err(ResourceError::ProviderError {
                resource_id,
                code: code.to_owned(),
                attempts,
            });
        }

        let content = parsed
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ResourceError::MalformedResponse {
                resource_id,
                detail: "no /choices/0/message/content in response".to_owned(),
                attempts,
            })?;

        if content.trim().is_empty() {
            return Err(ResourceError::MalformedResponse {
                resource_id,
                detail: "provider returned empty content".to_owned(),
                attempts,
            });
        }
        Ok(serde_json::json!({
            "answer": content,
            "model": parsed
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&self.config.model),
        }))
    }
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::ids::IndividualId;

    use super::*;

    const RESOURCE: ResourceId = ResourceId::from_u128(0x0B01);

    fn config() -> OpenAiCompatibleConfig {
        OpenAiCompatibleConfig::new(RESOURCE, "http://127.0.0.1:8080/v1", "test-model")
    }

    fn resource() -> OpenAiCompatibleResource {
        OpenAiCompatibleResource::new(config())
    }

    fn request() -> ResourceRequest {
        ResourceRequest::new(
            IndividualId::from_u128(1),
            "summarize-turn",
            serde_json::json!({ "evidence_id": "abc" }),
        )
    }

    fn success_body(content: &str) -> String {
        serde_json::json!({
            "model": "test-model",
            "choices": [{ "message": { "role": "assistant", "content": content } }],
        })
        .to_string()
    }

    #[test]
    fn the_descriptor_names_the_adapter_and_stays_read_only() {
        let descriptor = resource().descriptor();
        assert_eq!(descriptor.adapter, "openai-compatible");
        assert_eq!(descriptor.resource_id, RESOURCE);
        assert!(descriptor.read_only);
    }

    #[test]
    fn a_successful_reply_becomes_the_answer() {
        let content = resource()
            .map_response(
                HttpResponse {
                    status: 200,
                    body: success_body("hello"),
                },
                1,
            )
            .unwrap();
        assert_eq!(content["answer"], "hello");
        assert_eq!(content["model"], "test-model");
    }

    #[test]
    fn statuses_map_to_distinct_classifications() {
        let cases = [
            (401, "AUTHENTICATION"),
            (403, "AUTHENTICATION"),
            (429, "RATE_LIMITED"),
            (500, "HTTP_STATUS"),
            (503, "HTTP_STATUS"),
            (404, "HTTP_STATUS"),
        ];
        for (status, code) in cases {
            let error = resource()
                .map_response(
                    HttpResponse {
                        status,
                        body: "{\"error\":\"nope\"}".to_owned(),
                    },
                    1,
                )
                .expect_err("non-success must not be a result");
            assert_eq!(error.code(), code, "status {status}");
            assert_eq!(error.status(), Some(status));
        }
    }

    #[test]
    fn a_body_that_is_not_json_is_malformed() {
        let error = resource()
            .map_response(
                HttpResponse {
                    status: 200,
                    body: "<html>oops</html>".to_owned(),
                },
                2,
            )
            .unwrap_err();
        assert_eq!(error.code(), "MALFORMED_RESPONSE");
        assert_eq!(error.attempts(), 2);
    }

    #[test]
    fn json_without_the_expected_shape_is_malformed() {
        let error = resource()
            .map_response(
                HttpResponse {
                    status: 200,
                    body: "{\"choices\":[]}".to_owned(),
                },
                1,
            )
            .unwrap_err();
        assert_eq!(error.code(), "MALFORMED_RESPONSE");
    }

    #[test]
    fn a_provider_error_object_in_a_200_is_a_provider_error() {
        let error = resource()
            .map_response(
                HttpResponse {
                    status: 200,
                    body: serde_json::json!({
                        "error": { "code": "context_length_exceeded", "message": "too long" }
                    })
                    .to_string(),
                },
                1,
            )
            .unwrap_err();
        assert_eq!(error.code(), "PROVIDER_ERROR");
        assert!(error.to_string().contains("context_length_exceeded"));
        // The provider's prose is not carried into the error.
        assert!(!error.to_string().contains("too long"));
    }

    #[test]
    fn transport_failures_keep_their_class_and_attempt_count() {
        let timeout = resource().map_transport(
            HttpError::Timeout {
                elapsed_ms: 250,
                phase: "read",
            },
            3,
        );
        assert_eq!(timeout.code(), "TIMEOUT");
        assert_eq!(timeout.attempts(), 3);

        let transport =
            resource().map_transport(HttpError::Transport("connect: refused".to_owned()), 2);
        assert_eq!(transport.code(), "TRANSPORT");
        assert_eq!(transport.attempts(), 2);
    }

    #[test]
    fn only_transient_failures_are_worth_another_attempt() {
        // Repeating a rejected credential just repeats the rejection.
        assert!(!OpenAiCompatibleResource::is_retryable(
            &ResourceError::Authentication {
                resource_id: RESOURCE,
                status: 401
            }
        ));
        assert!(!OpenAiCompatibleResource::is_retryable(
            &ResourceError::MalformedResponse {
                resource_id: RESOURCE,
                detail: "x".to_owned(),
                attempts: 1
            }
        ));
        assert!(!OpenAiCompatibleResource::is_retryable(
            &ResourceError::ProviderError {
                resource_id: RESOURCE,
                code: "context_length_exceeded".to_owned(),
                attempts: 1
            }
        ));
        assert!(OpenAiCompatibleResource::is_retryable(
            &ResourceError::Timeout {
                resource_id: RESOURCE,
                elapsed_ms: 1,
                attempts: 1
            }
        ));
        assert!(OpenAiCompatibleResource::is_retryable(
            &ResourceError::RateLimited {
                resource_id: RESOURCE,
                status: 429,
                attempts: 1
            }
        ));
    }

    #[test]
    fn the_token_is_read_from_the_environment_and_named_only_by_variable() {
        let config = config().with_auth_env(Some("KAMIMUSUHI_ABSENT_TOKEN_FOR_TEST".to_owned()));
        let error = OpenAiCompatibleResource::new(config)
            .headers()
            .expect_err("an unset variable must fail before any request");
        assert!(
            error
                .to_string()
                .contains("KAMIMUSUHI_ABSENT_TOKEN_FOR_TEST")
        );
        assert_eq!(error.code(), "INVALID_REQUEST");
    }

    #[test]
    fn no_auth_env_means_no_authorization_header() {
        assert!(resource().headers().unwrap().is_empty());
    }

    #[test]
    fn the_request_body_names_the_model_and_carries_the_input() {
        let body = resource().request_body(&request());
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["model"], "test-model");
        assert_eq!(parsed["messages"][0]["role"], "user");
        assert!(
            parsed["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("evidence_id")
        );
    }

    #[test]
    fn configuration_is_validated_before_any_call() {
        assert!(config().validate().is_ok());
        // https is now a supported scheme, verified by rustls at call time.
        assert!(
            OpenAiCompatibleConfig::new(RESOURCE, "https://api.example.test/v1", "m")
                .validate()
                .is_ok()
        );
        // A scheme that is neither is still refused at configuration time.
        assert!(
            OpenAiCompatibleConfig::new(RESOURCE, "ftp://api.example.test/v1", "m")
                .validate()
                .is_err()
        );
        assert!(config().with_timeout_ms(0).validate().is_err());
        assert!(config().with_max_attempts(0).validate().is_err());
        assert!(
            OpenAiCompatibleConfig::new(RESOURCE, "http://h/v1", " ")
                .validate()
                .is_err()
        );
    }
}
