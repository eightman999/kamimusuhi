//! Provider-neutral conversation orchestration boundaries.
//!
//! The conversation-side K-CORE decides what kind of language action is
//! needed. Jev only makes the bounded choices around that action; it never
//! writes canonical state and it never generates prose. The configured LLM is
//! the language organ, not the cognitive authority.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::persona::{
    ConversationMessage, PersonaBackendDescriptor, PersonaCore, PersonaError, PersonaTurnInput,
    PersonaTurnResult,
};
use kamimusuhi_resource_http::http::post_json;
use kamimusuhi_resource_http::{Endpoint, Header, HttpError, TrustAnchors};
use serde::{Deserialize, Serialize};

pub const TYPESAFE_BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
pub const TYPESAFE_MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";
pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";
pub const DEFAULT_TYPESAFE_BASE_URL: &str = "https://api.typesafe.ai";
pub const DEFAULT_TYPESAFE_MODEL: &str = "jev-latest";

pub const LLM_PROVIDER_ENV: &str = "KAMIMUSUHI_LLM_PROVIDER";
pub const LLM_BASE_URL_ENV: &str = "KAMIMUSUHI_LLM_BASE_URL";
pub const LLM_MODEL_ENV: &str = "KAMIMUSUHI_LLM_MODEL";
pub const GROKBOT_API_KEY_ENV: &str = "GBVM_API_KEY";

/// Transient state owned by the conversation-side K-CORE interface.
///
/// This state is intentionally process/session scoped. It is rendered into a
/// dedicated provider section, never treated as durable self-state or memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationCoreState {
    pub turn: u64,
    pub active_goal: Option<String>,
    pub attention: Vec<String>,
    pub uncertainty: f32,
    pub arousal: f32,
    pub speech_act: String,
    pub last_action: Option<String>,
    pub previous_result: Option<String>,
}

impl Default for ConversationCoreState {
    fn default() -> Self {
        Self {
            turn: 0,
            active_goal: None,
            attention: vec!["user".to_owned()],
            uncertainty: 0.5,
            arousal: 0.2,
            speech_act: "observe".to_owned(),
            last_action: None,
            previous_result: None,
        }
    }
}

impl ConversationCoreState {
    pub fn for_input(previous: &Self, sequence: u64, text: &str) -> Self {
        let speech_act = infer_speech_act(text);
        let (goal, uncertainty, arousal) = match speech_act.as_str() {
            "greeting" => ("respond_to_greeting", 0.2, 0.25),
            "report_problem" => ("understand_and_report_problem", 0.65, 0.75),
            "answer_question" => ("answer_the_user", 0.45, 0.45),
            _ => ("continue_dialogue", 0.5, 0.4),
        };
        Self {
            turn: sequence + 1,
            active_goal: Some(goal.to_owned()),
            attention: derive_attention(text),
            uncertainty,
            arousal,
            speech_act,
            last_action: previous.last_action.clone(),
            previous_result: previous.previous_result.clone(),
        }
    }

    pub fn after_response(&self, decision: Decision, response: &str) -> Self {
        let mut next = self.clone();
        next.last_action = Some(decision.as_str().to_owned());
        next.previous_result = Some(if response.trim().is_empty() {
            "empty_response_rejected".to_owned()
        } else {
            "language_response_accepted".to_owned()
        });
        next.uncertainty = (next.uncertainty * 0.5).max(0.05);
        next
    }
}

pub fn infer_speech_act(text: &str) -> String {
    let trimmed = text.trim();
    if ["こんにちは", "こんばんは", "おはよう", "やあ", "どうも"]
        .iter()
        .any(|word| trimmed.starts_with(word))
    {
        return "greeting".to_owned();
    }
    if ["エラー", "失敗", "壊れ", "困", "問題", "不具合"]
        .iter()
        .any(|word| trimmed.contains(word))
    {
        return "report_problem".to_owned();
    }
    if trimmed.contains('?') || trimmed.contains('？') || trimmed.ends_with('か') {
        return "answer_question".to_owned();
    }
    "continue_dialogue".to_owned()
}

fn derive_attention(text: &str) -> Vec<String> {
    let mut attention = vec!["user".to_owned()];
    if infer_speech_act(text) == "report_problem" {
        attention.push("system_error".to_owned());
    } else if infer_speech_act(text) == "greeting" {
        attention.push("social_contact".to_owned());
    } else {
        attention.push("current_input".to_owned());
    }
    attention
}

/// Structured request handed to a language provider.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageRequest {
    pub user_text: String,
    pub speech_act: String,
    pub goal: Option<String>,
    pub core_state: ConversationCoreState,
    pub attention: Vec<String>,
    pub memories: Vec<String>,
    pub constraints: Vec<String>,
    pub recent_turns: Vec<ConversationMessage>,
    #[serde(skip_serializing)]
    pub persona_input: PersonaTurnInput,
}

#[derive(Debug, Clone)]
pub struct LanguageResult {
    pub persona: PersonaTurnResult,
    pub provider: String,
    pub model: String,
    pub latency_ms: u64,
}

pub trait LanguageProvider: Send + Sync {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError>;

    fn descriptor(&self) -> PersonaBackendDescriptor;
}

/// Adapter that preserves the existing PersonaCore contract while exposing
/// the language-provider interface used by the conversation loop.
pub struct PersonaLanguageProvider {
    persona: Box<dyn PersonaCore>,
    provider: String,
    model: String,
}

impl PersonaLanguageProvider {
    pub fn new(
        persona: Box<dyn PersonaCore>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            persona,
            provider: provider.into(),
            model: model.into(),
        }
    }
}

impl LanguageProvider for PersonaLanguageProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        let started = Instant::now();
        let persona = self
            .persona
            .turn(request.persona_input.clone())
            .map_err(ConversationError::Persona)?;
        Ok(LanguageResult {
            persona,
            provider: self.provider.clone(),
            model: self.model.clone(),
            latency_ms: elapsed_ms(started),
        })
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        self.persona.descriptor()
    }
}

/// Named language-organ adapters. Both currently use the repository's
/// existing OpenAI-compatible Persona transport; their names are attribution
/// and configuration labels, not separate protocol implementations.
pub struct GrokbotProvider(PersonaLanguageProvider);

impl GrokbotProvider {
    pub fn new(persona: Box<dyn PersonaCore>, model: impl Into<String>) -> Self {
        Self(PersonaLanguageProvider::new(persona, "grokbot", model))
    }
}

impl LanguageProvider for GrokbotProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.0.generate(request)
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        self.0.descriptor()
    }
}

pub struct HaiProvider(PersonaLanguageProvider);

impl HaiProvider {
    pub fn new(persona: Box<dyn PersonaCore>, model: impl Into<String>) -> Self {
        Self(PersonaLanguageProvider::new(persona, "hai", model))
    }
}

impl LanguageProvider for HaiProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.0.generate(request)
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        self.0.descriptor()
    }
}

/// API-free provider used by unit and closed-loop tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockLanguageProvider;

impl LanguageProvider for MockLanguageProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        let response = match request.speech_act.as_str() {
            "greeting" => "mock: こんにちは。状態を確認しています。",
            "report_problem" => "mock: 問題を確認しました。状況を整理します。",
            "answer_question" => "mock: 質問を受け取りました。",
            _ => "mock: 入力を受け取りました。",
        };
        let backend = PersonaBackendDescriptor {
            backend_id: PersonaBackendId::from_u128(0x4D4F434B),
            kind: "mock-language".to_owned(),
            name: "mock-language".to_owned(),
            version: "1".to_owned(),
        };
        Ok(LanguageResult {
            persona: PersonaTurnResult {
                context: request.persona_input.context,
                backend,
                response_intent: response.to_owned(),
                proposals: Vec::new(),
            },
            provider: "mock".to_owned(),
            model: "mock-language-v0".to_owned(),
            latency_ms: 0,
        })
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        PersonaBackendDescriptor {
            backend_id: PersonaBackendId::from_u128(0x4D4F434B),
            kind: "mock-language".to_owned(),
            name: "mock-language".to_owned(),
            version: "1".to_owned(),
        }
    }
}

pub fn configured_language_provider(
    persona: Box<dyn PersonaCore>,
    model: impl Into<String>,
) -> Result<Box<dyn LanguageProvider>, ConversationError> {
    let provider =
        std::env::var(LLM_PROVIDER_ENV).unwrap_or_else(|_| "openai-compatible".to_owned());
    let model = model.into();
    match provider.to_ascii_lowercase().as_str() {
        "grokbot" => Ok(Box::new(GrokbotProvider::new(persona, model))),
        "hai" => Ok(Box::new(HaiProvider::new(persona, model))),
        "mock" => Ok(Box::new(MockLanguageProvider)),
        "openai-compatible" => Ok(Box::new(PersonaLanguageProvider::new(
            persona,
            "openai-compatible",
            model,
        ))),
        other => Err(ConversationError::InvalidConfig(format!(
            "unsupported {LLM_PROVIDER_ENV} value {other:?}"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    InvocationGate,
    ResponseGate,
}

impl DecisionKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::InvocationGate => "invocation_gate",
            Self::ResponseGate => "response_gate",
        }
    }

    pub const fn choices(self) -> &'static [&'static str] {
        match self {
            Self::InvocationGate => &["SPEAK", "WAIT", "OBSERVE_MORE"],
            Self::ResponseGate => &["ACCEPT", "RETRY", "REJECT"],
        }
    }

    fn instructions(self) -> &'static str {
        match self {
            Self::InvocationGate => "Choose the appropriate next dialogue action.",
            Self::ResponseGate => "Choose whether the candidate response is acceptable.",
        }
    }

    fn criteria(self) -> BTreeMap<&'static str, &'static str> {
        match self {
            Self::InvocationGate => BTreeMap::from([
                ("SPEAK", "A response should be produced now."),
                ("WAIT", "Do not produce a response yet."),
                (
                    "OBSERVE_MORE",
                    "More information should be observed before responding.",
                ),
            ]),
            Self::ResponseGate => BTreeMap::from([
                ("ACCEPT", "The candidate response follows the core intent."),
                ("RETRY", "Generate one replacement candidate."),
                ("REJECT", "Do not deliver this candidate."),
            ]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    Speak,
    Wait,
    ObserveMore,
    Accept,
    Retry,
    Reject,
}

impl Decision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Speak => "SPEAK",
            Self::Wait => "WAIT",
            Self::ObserveMore => "OBSERVE_MORE",
            Self::Accept => "ACCEPT",
            Self::Retry => "RETRY",
            Self::Reject => "REJECT",
        }
    }

    fn parse(kind: DecisionKind, raw: &str) -> Option<Self> {
        let valid = match (kind, raw) {
            (DecisionKind::InvocationGate, "SPEAK") => Self::Speak,
            (DecisionKind::InvocationGate, "WAIT") => Self::Wait,
            (DecisionKind::InvocationGate, "OBSERVE_MORE") => Self::ObserveMore,
            (DecisionKind::ResponseGate, "ACCEPT") => Self::Accept,
            (DecisionKind::ResponseGate, "RETRY") => Self::Retry,
            (DecisionKind::ResponseGate, "REJECT") => Self::Reject,
            _ => return None,
        };
        Some(valid)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionRequest {
    pub kind: DecisionKind,
    pub user_text: String,
    pub speech_act: String,
    pub required_information: Vec<String>,
    pub candidate_response: Option<String>,
    /// Digest binding a response-gate decision to the exact candidate the
    /// host is about to accept, retry or reject. The response text itself is
    /// carried in `state` for Jev, but this digest is safe to retain in trace.
    pub candidate_digest: Option<String>,
    pub state: ConversationCoreState,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionResult {
    pub decision: Decision,
    pub confidence: f32,
    pub probabilities: BTreeMap<String, f32>,
    pub provider: String,
    pub model: String,
    pub latency_ms: u64,
    pub fallback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

pub trait DecisionProvider: Send + Sync {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError>;

    fn is_external(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuleBasedDecisionProvider;

impl DecisionProvider for RuleBasedDecisionProvider {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        let decision = match request.kind {
            DecisionKind::InvocationGate => {
                if request.user_text.trim().is_empty() {
                    Decision::Wait
                } else {
                    Decision::Speak
                }
            }
            DecisionKind::ResponseGate => {
                if request
                    .candidate_response
                    .as_deref()
                    .is_none_or(|response| response.trim().is_empty())
                {
                    Decision::Reject
                } else {
                    Decision::Accept
                }
            }
        };
        Ok(DecisionResult {
            decision,
            confidence: 1.0,
            probabilities: probabilities(request.kind, decision.as_str(), 1.0),
            provider: "rule-based".to_owned(),
            model: "deterministic-v0".to_owned(),
            latency_ms: 0,
            fallback: false,
            fallback_reason: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TypesafeConfig {
    pub base_url: String,
    pub model: String,
    pub auth_env: String,
    pub timeout_ms: u64,
}

impl Default for TypesafeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_TYPESAFE_BASE_URL.to_owned(),
            model: DEFAULT_TYPESAFE_MODEL.to_owned(),
            auth_env: TYPESAFE_API_KEY_ENV.to_owned(),
            timeout_ms: 30_000,
        }
    }
}

impl TypesafeConfig {
    pub fn from_env() -> Option<Self> {
        let key = std::env::var(TYPESAFE_API_KEY_ENV).ok()?;
        if key.trim().is_empty() {
            return None;
        }
        Some(Self {
            base_url: std::env::var(TYPESAFE_BASE_URL_ENV)
                .unwrap_or_else(|_| DEFAULT_TYPESAFE_BASE_URL.to_owned()),
            model: std::env::var(TYPESAFE_MODEL_ENV)
                .unwrap_or_else(|_| DEFAULT_TYPESAFE_MODEL.to_owned()),
            ..Self::default()
        })
    }

    fn endpoint(&self) -> Result<Endpoint, ConversationError> {
        Endpoint::parse(&self.base_url, "/v1/systemone").map_err(ConversationError::InvalidConfig)
    }

    fn validate(&self) -> Result<(), ConversationError> {
        if self.model.trim().is_empty() || self.timeout_ms == 0 {
            return Err(ConversationError::InvalidConfig(
                "TypeSafe model and timeout must be configured".to_owned(),
            ));
        }
        self.endpoint().map(|_| ())
    }
}

#[derive(Debug, Clone)]
pub struct JevDecisionProvider {
    config: TypesafeConfig,
}

impl JevDecisionProvider {
    pub fn new(config: TypesafeConfig) -> Self {
        Self { config }
    }

    fn headers(&self) -> Result<Vec<Header>, ConversationError> {
        if self.config.auth_env.is_empty() {
            return Ok(Vec::new());
        }
        let token = std::env::var(&self.config.auth_env).map_err(|_| {
            ConversationError::Credential(format!(
                "environment variable {} is not set",
                self.config.auth_env
            ))
        })?;
        if token.trim().is_empty() {
            return Err(ConversationError::Credential(format!(
                "environment variable {} is empty",
                self.config.auth_env
            )));
        }
        Ok(vec![Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {token}"),
        }])
    }

    fn request_body(
        &self,
        request: &DecisionRequest,
        feedback: Option<&str>,
    ) -> Result<String, ConversationError> {
        let mut instructions = request.kind.instructions().to_owned();
        if let Some(feedback) = feedback {
            instructions.push_str(" The previous answer was rejected by the client: ");
            instructions.push_str(feedback);
            instructions.push_str(". Return one valid choice answer.");
        }
        let question = serde_json::json!({
            "type": "choice",
            "instructions": instructions,
            "criteria": request.kind.criteria(),
        });
        let state = serde_json::to_string(request)
            .map_err(|error| ConversationError::Serialization(error.to_string()))?;
        Ok(serde_json::json!({
            "model": self.config.model,
            "state": state,
            "questions": { request.kind.key(): question },
        })
        .to_string())
    }

    fn parse_response(
        &self,
        kind: DecisionKind,
        body: &str,
        latency_ms: u64,
    ) -> Result<DecisionResult, ConversationError> {
        let parsed: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            ConversationError::Malformed("response body is not valid JSON".to_owned())
        })?;
        let answer = parsed
            .get("answers")
            .and_then(|answers| answers.get(kind.key()))
            .ok_or_else(|| {
                ConversationError::Malformed("missing typed choice answer".to_owned())
            })?;
        if answer.get("type").and_then(serde_json::Value::as_str) != Some("choice") {
            return Err(ConversationError::InvalidDecision(
                "answer type is not choice".to_owned(),
            ));
        }
        let choice = answer
            .get("choice")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ConversationError::InvalidDecision("choice is missing".to_owned()))?;
        let decision = Decision::parse(kind, choice).ok_or_else(|| {
            ConversationError::InvalidDecision(
                "choice is outside the declared vocabulary".to_owned(),
            )
        })?;
        let confidence = finite_unit(answer.get("confidence"), "confidence")?;
        let probabilities_value = answer
            .get("probabilities")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                ConversationError::InvalidDecision("probabilities are missing".to_owned())
            })?;
        let allowed: std::collections::BTreeSet<&str> = kind.choices().iter().copied().collect();
        if probabilities_value
            .keys()
            .any(|key| !allowed.contains(key.as_str()))
        {
            return Err(ConversationError::InvalidDecision(
                "probabilities contain an unknown choice".to_owned(),
            ));
        }
        let mut probabilities = BTreeMap::new();
        for candidate in kind.choices() {
            let value = finite_unit(probabilities_value.get(*candidate), "choice probability")?;
            probabilities.insert((*candidate).to_owned(), value);
        }
        let total: f32 = probabilities.values().sum();
        if (total - 1.0).abs() > 0.05 {
            return Err(ConversationError::InvalidDecision(
                "choice probabilities do not form a distribution".to_owned(),
            ));
        }
        Ok(DecisionResult {
            decision,
            confidence,
            probabilities,
            provider: "typesafe-systemone".to_owned(),
            model: self.config.model.clone(),
            latency_ms,
            fallback: false,
            fallback_reason: None,
        })
    }
}

impl DecisionProvider for JevDecisionProvider {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        self.config.validate()?;
        let endpoint = self.config.endpoint()?;
        let headers = self.headers()?;
        let started = Instant::now();
        let mut feedback: Option<String> = None;
        for _attempt in 0..2 {
            let body = self.request_body(request, feedback.as_deref())?;
            let response = post_json(
                &endpoint,
                &body,
                &headers,
                Duration::from_millis(self.config.timeout_ms),
                &TrustAnchors::Webpki,
            )
            .map_err(map_http_error)?;
            if !response.is_success() {
                return Err(ConversationError::HttpStatus(response.status));
            }
            match self.parse_response(request.kind, &response.body, elapsed_ms(started)) {
                Ok(result) => return Ok(result),
                Err(error) => {
                    feedback = Some(error.code().to_owned());
                }
            }
        }
        Err(ConversationError::Malformed(
            "TypeSafe response failed validation after one retry".to_owned(),
        ))
    }

    fn is_external(&self) -> bool {
        true
    }
}

/// If Jev is configured, use it; a failed Jev call degrades to deterministic
/// choices and is visible in the normalized result. The language organ is not
/// called by this fallback.
pub struct FallbackDecisionProvider {
    primary: Box<dyn DecisionProvider>,
    fallback: RuleBasedDecisionProvider,
}

impl FallbackDecisionProvider {
    pub fn new(primary: Box<dyn DecisionProvider>) -> Self {
        Self {
            primary,
            fallback: RuleBasedDecisionProvider,
        }
    }
}

impl DecisionProvider for FallbackDecisionProvider {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        match self.primary.decide(request) {
            Ok(result) => Ok(result),
            Err(error) => {
                let mut result = self.fallback.decide(request)?;
                result.fallback = true;
                result.fallback_reason = Some(error.code().to_owned());
                Ok(result)
            }
        }
    }

    fn is_external(&self) -> bool {
        self.primary.is_external()
    }
}

/// Debug-only, secret-free record of one closed-loop conversation turn.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationTrace {
    pub core_state_before: ConversationCoreState,
    pub invocation_gate: DecisionResult,
    pub language_provider: String,
    pub language_model: String,
    pub language_latency_ms: u64,
    pub response_gate: DecisionResult,
    pub candidate_digest: String,
    pub retry_count: u8,
    pub core_state_after: ConversationCoreState,
}

pub fn configured_decision_provider() -> Box<dyn DecisionProvider> {
    match TypesafeConfig::from_env() {
        Some(config) => Box::new(FallbackDecisionProvider::new(Box::new(
            JevDecisionProvider::new(config),
        ))),
        None => Box::new(RuleBasedDecisionProvider),
    }
}

fn probabilities(kind: DecisionKind, selected: &str, confidence: f32) -> BTreeMap<String, f32> {
    let remainder = if kind.choices().len() > 1 {
        (1.0 - confidence) / (kind.choices().len() as f32 - 1.0)
    } else {
        0.0
    };
    kind.choices()
        .iter()
        .map(|choice| {
            (
                (*choice).to_owned(),
                if *choice == selected {
                    confidence
                } else {
                    remainder
                },
            )
        })
        .collect()
}

fn finite_unit(value: Option<&serde_json::Value>, label: &str) -> Result<f32, ConversationError> {
    let value = value
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| ConversationError::InvalidDecision(format!("{label} is missing")))?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ConversationError::InvalidDecision(format!(
            "{label} is outside 0..1"
        )));
    }
    Ok(value as f32)
}

fn map_http_error(error: HttpError) -> ConversationError {
    match error {
        HttpError::InvalidRequest(reason) => ConversationError::InvalidConfig(reason),
        HttpError::Timeout { .. } => ConversationError::Timeout,
        HttpError::Transport(_) => ConversationError::Transport,
        HttpError::Malformed(_) => ConversationError::Malformed("invalid HTTP response".to_owned()),
        HttpError::Tls { kind, .. } => ConversationError::Tls(kind.as_str().to_owned()),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("conversation language provider failed: {0}")]
    Persona(#[from] PersonaError),
    #[error("conversation provider configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("conversation provider credential is unavailable: {0}")]
    Credential(String),
    #[error("conversation provider timed out")]
    Timeout,
    #[error("conversation provider transport failed")]
    Transport,
    #[error("conversation provider TLS failed: {0}")]
    Tls(String),
    #[error("conversation provider returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("conversation provider returned malformed data: {0}")]
    Malformed(String),
    #[error("conversation provider returned an invalid decision: {0}")]
    InvalidDecision(String),
    #[error("conversation provider response could not be serialized: {0}")]
    Serialization(String),
    #[error("conversation gate selected {0}")]
    GateRefused(String),
}

impl ConversationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Persona(error) => error.code(),
            Self::InvalidConfig(_) => "INVALID_CONFIG",
            Self::Credential(_) => "CREDENTIAL",
            Self::Timeout => "TIMEOUT",
            Self::Transport => "TRANSPORT",
            Self::Tls(_) => "TLS",
            Self::HttpStatus(_) => "HTTP_STATUS",
            Self::Malformed(_) => "MALFORMED",
            Self::InvalidDecision(_) => "INVALID_DECISION",
            Self::Serialization(_) => "SERIALIZATION",
            Self::GateRefused(_) => "GATE_REFUSED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: DecisionKind) -> DecisionRequest {
        DecisionRequest {
            kind,
            user_text: "こんにちは".to_owned(),
            speech_act: "greeting".to_owned(),
            required_information: vec!["a brief acknowledgement".to_owned()],
            candidate_response: None,
            candidate_digest: None,
            state: ConversationCoreState::default(),
        }
    }

    #[test]
    fn systemone_request_is_typed_choice_with_json_state_string() {
        let provider = JevDecisionProvider::new(TypesafeConfig {
            base_url: "https://api.typesafe.ai".to_owned(),
            model: "jev-1.13.0".to_owned(),
            auth_env: String::new(),
            timeout_ms: 1_000,
        });
        let body: serde_json::Value = serde_json::from_str(
            &provider
                .request_body(&request(DecisionKind::InvocationGate), None)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["model"], "jev-1.13.0");
        let state: serde_json::Value =
            serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
        assert_eq!(state["speech_act"], "greeting");
        assert_eq!(body["questions"]["invocation_gate"]["type"], "choice");
        assert_eq!(
            body["questions"]["invocation_gate"]["criteria"]["SPEAK"],
            "A response should be produced now."
        );
    }

    #[test]
    fn typed_choice_parser_rejects_unknown_or_incomplete_probabilities() {
        let provider = JevDecisionProvider::new(TypesafeConfig::default());
        let response = serde_json::json!({
            "answers": {
                "invocation_gate": {
                    "type": "choice",
                    "choice": "SPEAK",
                    "probabilities": {"SPEAK": 1.0, "WAIT": 0.0, "SURPRISE": 0.0},
                    "confidence": 1.0
                }
            }
        });
        let error = provider
            .parse_response(DecisionKind::InvocationGate, &response.to_string(), 0)
            .unwrap_err();
        assert_eq!(error.code(), "INVALID_DECISION");
    }

    #[test]
    fn rule_based_fallback_preserves_gate_vocabulary() {
        let provider = RuleBasedDecisionProvider;
        let invocation = provider
            .decide(&request(DecisionKind::InvocationGate))
            .unwrap();
        assert_eq!(invocation.decision, Decision::Speak);
        let mut response_request = request(DecisionKind::ResponseGate);
        response_request.candidate_response = Some("返答".to_owned());
        let response = provider.decide(&response_request).unwrap();
        assert_eq!(response.decision, Decision::Accept);
        assert!(response.probabilities.contains_key("RETRY"));
    }

    struct FailingDecisionProvider;

    impl DecisionProvider for FailingDecisionProvider {
        fn decide(&self, _request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
            Err(ConversationError::Transport)
        }

        fn is_external(&self) -> bool {
            true
        }
    }

    #[test]
    fn decision_fallback_is_explicit_and_secret_free() {
        let provider = FallbackDecisionProvider::new(Box::new(FailingDecisionProvider));
        let result = provider
            .decide(&request(DecisionKind::InvocationGate))
            .unwrap();
        assert!(result.fallback);
        assert_eq!(result.fallback_reason.as_deref(), Some("TRANSPORT"));
        assert_eq!(result.provider, "rule-based");
    }
}
