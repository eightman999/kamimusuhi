//! Typed, one-invocation protocol for exec-isolated language/decision providers.
//!
//! Requests travel only through private host/child IPC, never through argv or a
//! model endpoint. The language provider's existing public serialization still
//! omits Persona material; this protocol adds it only to the local worker frame.

use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaCore, PersonaError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{PersonaProviderConfig, RuntimeConfig};
use crate::llm_jev::{
    ConversationError, DecisionProvider, DecisionRequest, DecisionResult, FallbackDecisionProvider,
    JevDecisionProvider, LanguageProvider, LanguageRequest, LanguageResponseSelectionRequest,
    LanguageResult, PersonaLanguageProvider, ProviderSelectionRequest, ProviderSelectionResult,
    ResponseAssessment, ResponseAssessmentRequest, RuleBasedDecisionProvider, TurnPreparation,
    TurnPreparationRequest, TypesafeConfig, configured_language_provider,
};
use crate::provider_process::ProcessCommand;

const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The closed set of worker operations. No database, session, or arbitrary
/// executable command is supplied to the worker through this frame.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum WorkerRequest {
    Language {
        config: Box<RuntimeConfig>,
        provider: Option<PersonaProviderConfig>,
        request: Value,
    },
    Decide {
        #[serde(default)]
        config: Option<TypesafeConfig>,
        request: DecisionRequest,
    },
    Prepare {
        #[serde(default)]
        config: Option<TypesafeConfig>,
        request: TurnPreparationRequest,
    },
    Assess {
        #[serde(default)]
        config: Option<TypesafeConfig>,
        request: ResponseAssessmentRequest,
    },
    SelectProvider {
        #[serde(default)]
        config: Option<TypesafeConfig>,
        request: ProviderSelectionRequest,
    },
    SelectResponse {
        #[serde(default)]
        config: Option<TypesafeConfig>,
        request: LanguageResponseSelectionRequest,
    },
}

/// No provider-supplied error strings, panic payloads, model names, or endpoint
/// details cross the worker error boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerFailure {
    InvalidConfig,
    Credential,
    Timeout,
    Transport,
    Tls,
    HttpStatus(u16),
    Malformed,
    InvalidDecision,
    Cancelled,
    DecisionUnavailable,
    EmptyResponse,
    OversizedResponse,
    InvalidCandidate,
    Serialization,
    GateRefused,
    Persona(WorkerPersonaFailure),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerPersonaFailure {
    InvalidInput,
    Timeout,
    Transport,
    Tls,
    Authentication(u16),
    RateLimited(u16),
    HttpStatus(u16),
    MalformedResponse,
    ProviderError,
    Backend,
}

impl WorkerFailure {
    fn from_error(error: ConversationError) -> Self {
        match error {
            ConversationError::Persona(error) => Self::Persona(match error {
                PersonaError::InvalidInput { .. } => WorkerPersonaFailure::InvalidInput,
                PersonaError::Timeout { .. } => WorkerPersonaFailure::Timeout,
                PersonaError::Transport { .. } => WorkerPersonaFailure::Transport,
                PersonaError::Tls { .. } => WorkerPersonaFailure::Tls,
                PersonaError::Authentication { status, .. } => {
                    WorkerPersonaFailure::Authentication(status)
                }
                PersonaError::RateLimited { status, .. } => {
                    WorkerPersonaFailure::RateLimited(status)
                }
                PersonaError::HttpStatus { status, .. } => WorkerPersonaFailure::HttpStatus(status),
                PersonaError::MalformedResponse { .. } => WorkerPersonaFailure::MalformedResponse,
                PersonaError::ProviderError { .. } => WorkerPersonaFailure::ProviderError,
                PersonaError::Backend { .. } => WorkerPersonaFailure::Backend,
            }),
            ConversationError::InvalidConfig(_) => Self::InvalidConfig,
            ConversationError::Credential(_) => Self::Credential,
            ConversationError::Timeout => Self::Timeout,
            ConversationError::Transport => Self::Transport,
            ConversationError::Tls(_) => Self::Tls,
            ConversationError::HttpStatus(status) => Self::HttpStatus(status),
            ConversationError::Malformed(_) => Self::Malformed,
            ConversationError::InvalidDecision(_) => Self::InvalidDecision,
            ConversationError::Cancelled => Self::Cancelled,
            ConversationError::DecisionUnavailable(_) => Self::DecisionUnavailable,
            ConversationError::InvalidCandidate("EMPTY_RESPONSE") => Self::EmptyResponse,
            ConversationError::InvalidCandidate("RESPONSE_TOO_LARGE") => Self::OversizedResponse,
            ConversationError::InvalidCandidate(_) => Self::InvalidCandidate,
            ConversationError::Serialization(_) => Self::Serialization,
            ConversationError::GateRefused(_) => Self::GateRefused,
        }
    }

    fn into_error(self) -> ConversationError {
        let fixed = || "provider worker refused the operation".to_owned();
        match self {
            Self::InvalidConfig => ConversationError::InvalidConfig(fixed()),
            Self::Credential => ConversationError::Credential(fixed()),
            Self::Timeout => ConversationError::Timeout,
            Self::Transport => ConversationError::Transport,
            Self::Tls => ConversationError::Tls("WORKER_TLS".to_owned()),
            Self::HttpStatus(status) => ConversationError::HttpStatus(status),
            Self::Malformed => ConversationError::Malformed(fixed()),
            Self::InvalidDecision => ConversationError::InvalidDecision(fixed()),
            Self::Cancelled => ConversationError::Cancelled,
            Self::DecisionUnavailable => ConversationError::DecisionUnavailable(fixed()),
            Self::EmptyResponse => ConversationError::InvalidCandidate("EMPTY_RESPONSE"),
            Self::OversizedResponse => ConversationError::InvalidCandidate("RESPONSE_TOO_LARGE"),
            Self::InvalidCandidate => ConversationError::InvalidCandidate("INVALID_CANDIDATE"),
            Self::Serialization => ConversationError::Serialization(fixed()),
            Self::GateRefused => ConversationError::GateRefused("REJECT".to_owned()),
            Self::Persona(error) => {
                // Host attribution stays in host telemetry; errors do not carry
                // a child-provided backend identity into an anonymous judge.
                let backend_id = PersonaBackendId::from_u128(0);
                ConversationError::Persona(match error {
                    WorkerPersonaFailure::InvalidInput => {
                        PersonaError::InvalidInput { reason: fixed() }
                    }
                    WorkerPersonaFailure::Timeout => PersonaError::Timeout {
                        backend_id,
                        elapsed_ms: 0,
                    },
                    WorkerPersonaFailure::Transport => PersonaError::Transport {
                        backend_id,
                        message: fixed(),
                    },
                    WorkerPersonaFailure::Tls => PersonaError::Tls {
                        backend_id,
                        kind: "WORKER_TLS".to_owned(),
                        detail: fixed(),
                    },
                    WorkerPersonaFailure::Authentication(status) => {
                        PersonaError::Authentication { backend_id, status }
                    }
                    WorkerPersonaFailure::RateLimited(status) => {
                        PersonaError::RateLimited { backend_id, status }
                    }
                    WorkerPersonaFailure::HttpStatus(status) => {
                        PersonaError::HttpStatus { backend_id, status }
                    }
                    WorkerPersonaFailure::MalformedResponse => PersonaError::MalformedResponse {
                        backend_id,
                        detail: fixed(),
                    },
                    WorkerPersonaFailure::ProviderError => PersonaError::ProviderError {
                        backend_id,
                        code: "WORKER_PROVIDER_ERROR".to_owned(),
                    },
                    WorkerPersonaFailure::Backend => PersonaError::Backend { message: fixed() },
                })
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum WorkerReply {
    Ok(Value),
    Error(WorkerFailure),
}

impl WorkerReply {
    fn from_result<T: Serialize>(result: Result<T, ConversationError>) -> Self {
        match result {
            Ok(value) => serde_json::to_value(value)
                .map_or(Self::Error(WorkerFailure::Serialization), Self::Ok),
            Err(error) => Self::Error(WorkerFailure::from_error(error)),
        }
    }

    fn decode<T: DeserializeOwned>(self) -> Result<T, ConversationError> {
        match self {
            Self::Ok(value) => serde_json::from_value(value).map_err(|_| {
                ConversationError::Malformed("invalid provider worker result".to_owned())
            }),
            Self::Error(error) => Err(error.into_error()),
        }
    }
}

fn decision_provider(config: Option<TypesafeConfig>) -> Box<dyn DecisionProvider> {
    match config {
        Some(config) => Box::new(FallbackDecisionProvider::new(Box::new(
            JevDecisionProvider::new(config),
        ))),
        None => Box::new(RuleBasedDecisionProvider),
    }
}

/// Execute one typed operation. The worker opens no canonical SQLite store.
/// This is also useful to fixture executables that delegate healthy operations
/// while intentionally hanging another operation.
pub fn execute(request: WorkerRequest) -> WorkerReply {
    match request {
        WorkerRequest::Language {
            config,
            provider,
            request,
        } => WorkerReply::from_result((|| {
            let request: LanguageRequest = serde_json::from_value(request).map_err(|_| {
                ConversationError::Malformed("invalid worker language request".to_owned())
            })?;
            let language: Box<dyn LanguageProvider> = if let Some(provider) = provider {
                let persona = config.build_language_provider(&provider).map_err(|_| {
                    ConversationError::InvalidConfig(
                        "invalid language worker configuration".to_owned(),
                    )
                })?;
                Box::new(PersonaLanguageProvider::new(
                    persona,
                    crate::dialogue_setup::language_provider_kind(&provider),
                    provider.model,
                ))
            } else {
                let persona = config.build_persona().map_err(|_| {
                    ConversationError::InvalidConfig(
                        "invalid persona worker configuration".to_owned(),
                    )
                })?;
                let model = config
                    .persona
                    .provider
                    .as_ref()
                    .map(|p| p.model.clone())
                    .unwrap_or_else(|| persona.descriptor().name);
                configured_language_provider(persona, model)?
            };
            language.generate(&request)
        })()),
        WorkerRequest::Decide { config, request } => {
            WorkerReply::from_result(decision_provider(config).decide(&request))
        }
        WorkerRequest::Prepare { config, request } => {
            WorkerReply::from_result(decision_provider(config).prepare_turn(&request))
        }
        WorkerRequest::Assess { config, request } => {
            WorkerReply::from_result(decision_provider(config).assess_responses(&request))
        }
        WorkerRequest::SelectProvider { config, request } => {
            WorkerReply::from_result(decision_provider(config).select_language_provider(&request))
        }
        WorkerRequest::SelectResponse { config, request } => {
            WorkerReply::from_result(decision_provider(config).select_language_response(&request))
        }
    }
}

/// Run one request then exit, so all provider-owned threads and memory are
/// reclaimed even after a successful response. Panic text stays off stdout.
pub fn run_stdio() -> Result<(), ConversationError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| ConversationError::Transport)?;
    let reply = if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        WorkerReply::Error(WorkerFailure::Malformed)
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(request) => execute(request),
            Err(_) => WorkerReply::Error(WorkerFailure::Malformed),
        }
    };
    let mut output = serde_json::to_vec(&reply)
        .map_err(|_| ConversationError::Serialization("worker frame".to_owned()))?;
    if output.len().saturating_add(1) > MAX_FRAME_BYTES {
        output = serde_json::to_vec(&WorkerReply::Error(WorkerFailure::Malformed))
            .map_err(|_| ConversationError::Serialization("worker frame".to_owned()))?;
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&output)
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(|_| ConversationError::Transport)
}

fn millis(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

fn invoke<T: DeserializeOwned>(
    command: &ProcessCommand,
    request: WorkerRequest,
    timeout: Duration,
    cancellation: Option<&crate::llm_jev::CancellationToken>,
) -> Result<T, ConversationError> {
    let frame = serde_json::to_value(request)
        .map_err(|_| ConversationError::Serialization("worker request".to_owned()))?;
    let response = command.invoke(&frame, timeout, cancellation)?;
    let reply: WorkerReply = serde_json::from_value(response)
        .map_err(|_| ConversationError::Malformed("invalid provider worker reply".to_owned()))?;
    reply.decode()
}

pub struct ProcessLanguageProvider {
    command: ProcessCommand,
    config: RuntimeConfig,
    provider: Option<PersonaProviderConfig>,
    timeout_ms: AtomicU64,
}

impl ProcessLanguageProvider {
    pub fn new(
        command: ProcessCommand,
        config: RuntimeConfig,
        provider: Option<PersonaProviderConfig>,
        timeout: Duration,
    ) -> Self {
        Self {
            command,
            config,
            provider,
            timeout_ms: AtomicU64::new(millis(timeout)),
        }
    }
}

impl LanguageProvider for ProcessLanguageProvider {
    fn is_process_isolated(&self) -> bool {
        true
    }

    fn set_host_timeout(&self, timeout: Duration) {
        self.timeout_ms.store(millis(timeout), Ordering::Relaxed);
    }

    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        let mut payload = serde_json::to_value(request)
            .map_err(|_| ConversationError::Serialization("worker language request".to_owned()))?;
        payload
            .as_object_mut()
            .ok_or_else(|| ConversationError::Serialization("worker language object".to_owned()))?
            .insert(
                "persona_input".to_owned(),
                serde_json::to_value(&request.persona_input).map_err(|_| {
                    ConversationError::Serialization("worker persona input".to_owned())
                })?,
            );
        invoke(
            &self.command,
            WorkerRequest::Language {
                config: Box::new(self.config.clone()),
                provider: self.provider.clone(),
                request: payload,
            },
            Duration::from_millis(self.timeout_ms.load(Ordering::Relaxed)),
            request.cancellation.as_ref(),
        )
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        let provider = self
            .provider
            .as_ref()
            .or(self.config.persona.provider.as_ref());
        if let Some(provider) = provider {
            PersonaBackendDescriptor {
                backend_id: provider.backend_id,
                kind: "openai-compatible".to_owned(),
                name: provider.model.clone(),
                version: "1".to_owned(),
            }
        } else {
            kamimusuhi_testkit::FakePersonaCore.descriptor()
        }
    }
}

pub struct ProcessDecisionProvider {
    command: ProcessCommand,
    timeout_ms: AtomicU64,
    config: Option<TypesafeConfig>,
}

impl ProcessDecisionProvider {
    pub fn new(command: ProcessCommand, timeout: Duration) -> Self {
        Self {
            command,
            timeout_ms: AtomicU64::new(millis(timeout)),
            config: TypesafeConfig::from_env(),
        }
    }

    fn call<T: DeserializeOwned>(&self, request: WorkerRequest) -> Result<T, ConversationError> {
        invoke(
            &self.command,
            request,
            Duration::from_millis(self.timeout_ms.load(Ordering::Relaxed)),
            None,
        )
    }
}

impl DecisionProvider for ProcessDecisionProvider {
    fn is_process_isolated(&self) -> bool {
        true
    }
    fn set_host_timeout(&self, timeout: Duration) {
        self.timeout_ms.store(millis(timeout), Ordering::Relaxed);
    }
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        self.call(WorkerRequest::Decide {
            config: self.config.clone(),
            request: request.clone(),
        })
    }
    fn prepare_turn(
        &self,
        request: &TurnPreparationRequest,
    ) -> Result<TurnPreparation, ConversationError> {
        self.call(WorkerRequest::Prepare {
            config: self.config.clone(),
            request: request.clone(),
        })
    }
    fn assess_responses(
        &self,
        request: &ResponseAssessmentRequest,
    ) -> Result<ResponseAssessment, ConversationError> {
        self.call(WorkerRequest::Assess {
            config: self.config.clone(),
            request: request.clone(),
        })
    }
    fn select_language_provider(
        &self,
        request: &ProviderSelectionRequest,
    ) -> Result<ProviderSelectionResult, ConversationError> {
        self.call(WorkerRequest::SelectProvider {
            config: self.config.clone(),
            request: request.clone(),
        })
    }
    fn select_language_response(
        &self,
        request: &LanguageResponseSelectionRequest,
    ) -> Result<ProviderSelectionResult, ConversationError> {
        self.call(WorkerRequest::SelectResponse {
            config: self.config.clone(),
            request: request.clone(),
        })
    }
    fn is_external(&self) -> bool {
        self.config.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_errors_do_not_serialize_raw_provider_material() {
        let reply = WorkerReply::from_result::<Value>(Err(ConversationError::Malformed(
            "secret-token vendor/model private endpoint".to_owned(),
        )));
        let frame = serde_json::to_string(&reply).unwrap();
        assert_eq!(frame, r#"{"status":"error","value":"MALFORMED"}"#);
        let error = serde_json::from_str::<WorkerReply>(&frame)
            .unwrap()
            .decode::<Value>()
            .unwrap_err();
        assert_eq!(error.code(), "MALFORMED");
        assert!(!error.is_unavailable());
        assert!(!error.to_string().contains("secret-token"));
    }

    #[test]
    fn worker_outage_classes_preserve_fail_soft_and_fail_closed() {
        for error in [
            ConversationError::Timeout,
            ConversationError::Transport,
            ConversationError::HttpStatus(429),
            ConversationError::HttpStatus(503),
        ] {
            let decoded = WorkerReply::from_result::<Value>(Err(error))
                .decode::<Value>()
                .unwrap_err();
            assert!(decoded.is_unavailable());
        }
        for error in [
            ConversationError::InvalidConfig("secret".to_owned()),
            ConversationError::Credential("secret".to_owned()),
            ConversationError::Malformed("secret".to_owned()),
            ConversationError::InvalidDecision("secret".to_owned()),
            ConversationError::HttpStatus(401),
        ] {
            let decoded = WorkerReply::from_result::<Value>(Err(error))
                .decode::<Value>()
                .unwrap_err();
            assert!(!decoded.is_unavailable());
            assert!(!decoded.to_string().contains("secret"));
        }
    }
}
