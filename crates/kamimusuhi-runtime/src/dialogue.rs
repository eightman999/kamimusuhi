//! Text interaction over the existing individual and canonical evidence.
//! Raw conversation is recallable, but never activated as a belief. Generated
//! text and a successful write to the output surface have separate records.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use kamimusuhi_core::continuity::WriterIdentity;
use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::persona::{
    ConversationMessage, ConversationRole, CurrentInput, PersonaBackendDescriptor, PersonaEnvelope,
    PersonaTurnInput, ResponseGuidance, SessionWorkingState, TurnContext,
};
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_core::workspace::WorkspaceBuilder;
use serde::Serialize;

use crate::c0::{self, eval};
use crate::config::validate_language_provider_id;
use crate::dialogue_recall::RecallPool;
use crate::dialogue_setup::{
    language_provider_auth_available, language_provider_kind, public_endpoint_origin,
};
use crate::llm_jev::{
    ConversationCoreState, ConversationError, ConversationTrace, Decision, DecisionEvidence,
    DecisionKind, DecisionProvider, DecisionRequest, GeneratedLanguageCandidate, LanguageProvider,
    LanguageProviderCandidate, LanguageProviderTelemetry, LanguageProviderTelemetrySnapshot,
    LanguageResponseCandidate, LanguageResponseSelectionRequest, LanguageResult,
    MAX_JEV_CANDIDATE_RESPONSE_BYTES, ObservationNeed, PRIMARY_LANGUAGE_PROVIDER_ID,
    PersonaLanguageProvider, ResponseAssessment, ResponseAssessmentRequest, TurnPreparationRequest,
    configured_decision_provider, configured_language_provider,
};
use crate::research::ResearchCatalog;
use crate::{Runtime, RuntimeError};

pub const MAX_INPUT_BYTES: usize = 8_192;
const HISTORY_BYTES: usize = 16_384;

/// Only the already selected, attributed turn context is exposed to the
/// decision provider. Provider configuration and authentication never enter it.
fn decision_evidence(input: &PersonaTurnInput) -> Result<DecisionEvidence, ConversationError> {
    let envelope = &input.envelope;
    DecisionEvidence::new(serde_json::json!({
        "turn": input.context,
        "user_input": input.input,
        "conversation_history": envelope.conversation_history,
        "relationship": envelope.relationship,
        "episodic": envelope.episodic,
        "recalled_evidence": envelope.recalled_evidence,
        "durable_self": envelope.durable_self,
        "active_policy": envelope.active_policy,
        "observed_runtime": envelope.observed_runtime,
        "mio_observation": envelope.mio_observation,
        "research_findings": envelope.research_findings,
        "body_state": envelope.body_state,
        "library": envelope.library,
        "external_results": envelope.external_results,
        "response_guidance": envelope.response_guidance,
    }))
}

fn observation_guidance(
    need: ObservationNeed,
    recall_available: bool,
    research_available: bool,
) -> Option<ResponseGuidance> {
    let (reason_code, instruction) = match need {
        ObservationNeed::None => return None,
        ObservationNeed::Recall if recall_available => (
            "OBSERVE_RECALL",
            "今回選んだ記憶と発言記録を確認して答えてください。発言者と出典を保ち、根拠が足りなければ必要な情報を一つ質問してください。",
        ),
        ObservationNeed::Runtime => (
            "OBSERVE_RUNTIME",
            "このターンのOBSERVED_RUNTIMEとMIO_OBSERVATIONを確認して答えてください。未接続・記録済みの観測を現在の身体感覚と解釈せず、不明な点は確認してください。",
        ),
        ObservationNeed::Research if research_available => (
            "OBSERVE_RESEARCH",
            "今回取得したRESEARCH_FINDINGSの主張・失敗・制限を確認して答えてください。研究結果を自分の経験や獲得能力として述べないでください。",
        ),
        ObservationNeed::Recall | ObservationNeed::Research | ObservationNeed::Clarify => (
            "OBSERVE_CLARIFY",
            "回答に必要な根拠が不足しています。推測で回答せず、足りない情報を相手に一つだけ短く質問してください。",
        ),
    };
    Some(ResponseGuidance {
        reason_code: reason_code.to_owned(),
        instruction: instruction.to_owned(),
        previous_response_digest: None,
        previous_response: None,
    })
}

fn required_information(state: &ConversationCoreState) -> Vec<String> {
    match state.speech_act.as_str() {
        "report_problem" => vec![
            "the observed problem or failure".to_owned(),
            "one concrete next step or clarification".to_owned(),
        ],
        "answer_question" => vec!["the answer supported by the supplied context".to_owned()],
        "greeting" => vec!["a brief acknowledgement".to_owned()],
        _ => Vec::new(),
    }
}

/// Per-turn C0 bookkeeping, surfaced for inspection.
#[derive(Debug, Clone, Serialize)]
pub struct TurnC0 {
    /// Derived-lane head this turn ran under.
    pub activation_seq: u64,
    /// Memory items surfaced into the workspace.
    pub memories_surfaced: usize,
    /// Canonical drafts submitted and how many were activated.
    pub drafts_submitted: usize,
    pub drafts_activated: usize,
    /// Turn metrics recorded by the deterministic evaluator.
    pub metrics: kamimusuhi_core::c0::TurnMetrics,
}

#[derive(Debug, Serialize)]
pub struct DialogueReply {
    pub individual_id: IndividualId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub input_evidence_id: EvidenceId,
    pub generated_evidence_id: EvidenceId,
    pub history_messages: usize,
    pub observed_runtime: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mio_observation: Option<serde_json::Value>,
    pub research_findings: serde_json::Value,
    pub persona_backend: PersonaBackendDescriptor,
    pub response: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c0: Option<TurnC0>,
    /// The assembled context exactly as the backend received it. Only
    /// populated when debug-context output was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_context: Option<serde_json::Value>,
    /// Secret-free Jev/LLM/core trace, populated only in debug mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_jev: Option<ConversationTrace>,
}

struct LanguageAttempt {
    provider_id: String,
    elapsed_ms: u64,
    result: Result<crate::llm_jev::LanguageResult, crate::llm_jev::ConversationError>,
}

impl LanguageAttempt {
    fn generate(
        provider_id: &str,
        provider: &dyn LanguageProvider,
        request: &crate::llm_jev::LanguageRequest,
    ) -> Self {
        let started = Instant::now();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| provider.generate(request)))
                .unwrap_or(Err(ConversationError::Transport));
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let result = result.and_then(|mut result| {
            if result.persona.response_intent.trim().is_empty() {
                return Err(ConversationError::InvalidCandidate("EMPTY_RESPONSE"));
            }
            if result.persona.response_intent.len() > MAX_JEV_CANDIDATE_RESPONSE_BYTES {
                return Err(ConversationError::InvalidCandidate("RESPONSE_TOO_LARGE"));
            }
            // Identity and timing are supplied by the host, not by returned material.
            result.provider_id = provider_id.to_owned();
            result.latency_ms = elapsed_ms;
            Ok(result)
        });
        Self {
            provider_id: provider_id.to_owned(),
            elapsed_ms,
            result,
        }
    }
}

pub struct DialogueSession {
    session_id: SessionId,
    individual_id: IndividualId,
    subject: String,
    source_id: String,
    sequence: u64,
    privacy: PrivacyConstraint,
    started: Instant,
    research: ResearchCatalog,
    /// Writer epoch, claimed lazily on the first canonical draft submission.
    /// A conversation that only reads never takes the epoch.
    writer: Option<WriterIdentity>,
    /// Emit the assembled context on each turn (`--debug-context`).
    debug_context: bool,
    /// Emit the secret-free closed-loop trace (`--debug`).
    debug_trace: bool,
    /// Conversation-side K-CORE state. This is not `KCore::tick()` state.
    core_state: ConversationCoreState,
    decision_provider: Box<dyn DecisionProvider>,
    language_providers: BTreeMap<String, Arc<dyn LanguageProvider>>,
    language_candidates: Vec<LanguageProviderCandidate>,
    last_generated_candidates: Vec<GeneratedLanguageCandidate>,
    last_generation_latency_ms: u64,
    /// The last assembled workspace, for `/context` inspection.
    last_context: Option<serde_json::Value>,
}

impl DialogueSession {
    pub fn start(
        runtime: &mut Runtime,
        subject: &str,
        privacy: PrivacyConstraint,
    ) -> Result<Self, RuntimeError> {
        Self::start_with_providers(
            runtime,
            subject,
            privacy,
            configured_decision_provider(),
            None,
        )
    }

    /// Start a session with a caller-selected decision provider. This keeps
    /// Jev behind the same interface while allowing fixture and mock tests to
    /// exercise the complete loop without contacting an external service.
    pub fn start_with_decision_provider(
        runtime: &mut Runtime,
        subject: &str,
        privacy: PrivacyConstraint,
        decision_provider: Box<dyn DecisionProvider>,
    ) -> Result<Self, RuntimeError> {
        Self::start_with_providers(runtime, subject, privacy, decision_provider, None)
    }

    /// Start a session with explicit providers. The language provider is
    /// optional because production sessions construct it from the configured
    /// Persona endpoint and `KAMIMUSUHI_LLM_*` settings.
    pub fn start_with_providers(
        runtime: &mut Runtime,
        subject: &str,
        privacy: PrivacyConstraint,
        decision_provider: Box<dyn DecisionProvider>,
        language_provider_override: Option<Box<dyn LanguageProvider>>,
    ) -> Result<Self, RuntimeError> {
        if subject.is_empty()
            || subject.len() > 80
            || !subject
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(RuntimeError::Usage(
                "--subject must be 1-80 ASCII letters, digits, '.', '_' or '-'".to_owned(),
            ));
        }
        // Configuration and destination failures precede any interaction write.
        runtime.config().persona.check_privacy(privacy)?;
        let persona = runtime.config().build_persona()?;
        let language_model = runtime
            .config()
            .persona
            .provider
            .as_ref()
            .map(|provider| provider.model.clone())
            .unwrap_or_else(|| persona.descriptor().name.clone());
        let override_selected = language_provider_override.is_some();
        let language_provider = match language_provider_override {
            Some(provider) => provider,
            None => configured_language_provider(persona, language_model.clone())?,
        };
        let primary_provider = if override_selected {
            "override".to_owned()
        } else {
            std::env::var(crate::llm_jev::LLM_PROVIDER_ENV)
                .unwrap_or_else(|_| "in-process".to_owned())
        };
        let mut language_providers = BTreeMap::new();
        language_providers.insert(
            PRIMARY_LANGUAGE_PROVIDER_ID.to_owned(),
            Arc::from(language_provider),
        );
        let mut language_candidates = vec![LanguageProviderCandidate {
            id: PRIMARY_LANGUAGE_PROVIDER_ID.to_owned(),
            provider: primary_provider,
            model: language_model,
            endpoint: runtime.config().persona.provider.as_ref().map_or_else(
                || "in-process".to_owned(),
                |provider| public_endpoint_origin(&provider.base_url),
            ),
            telemetry: LanguageProviderTelemetry::default().snapshot(),
        }];
        if !override_selected {
            for (id, provider_config) in &runtime.config().language_providers {
                validate_language_provider_id(id)?;
                if id == PRIMARY_LANGUAGE_PROVIDER_ID {
                    return Err(RuntimeError::PersonaConfig {
                        message: "language provider id 'primary' is reserved".to_owned(),
                    });
                }
                if !privacy.admits(provider_config.locality)
                    || !language_provider_auth_available(provider_config)
                {
                    continue;
                }
                let persona = provider_config.build_persona()?;
                let model = provider_config.model.clone();
                let provider_kind = language_provider_kind(provider_config);
                let provider = PersonaLanguageProvider::with_id(
                    persona,
                    id.clone(),
                    provider_kind,
                    model.clone(),
                );
                language_providers.insert(id.clone(), Box::new(provider));
                language_candidates.push(LanguageProviderCandidate {
                    id: id.clone(),
                    provider: provider_kind.to_owned(),
                    model,
                    endpoint: public_endpoint_origin(&provider_config.base_url),
                    telemetry: LanguageProviderTelemetry::default().snapshot(),
                });
            }
        }
        if decision_provider.is_external() && !privacy.admits(LocalityClass::External) {
            return Err(RuntimeError::PersonaPrivacy {
                privacy,
                locality: LocalityClass::External,
            });
        }
        runtime.config().persona_seed()?;
        if let Some(mio) = &runtime.config().mio {
            mio.validate()?;
        }
        let research = ResearchCatalog::bundled()?;
        let session_id = SessionId::generate(runtime.ids().as_ref());
        if runtime.store().session(session_id)?.is_some() {
            return Err(RuntimeError::IdCollision {
                kind: "session",
                id: session_id.to_string(),
            });
        }
        research.sync(runtime.store())?;
        runtime.store().open_session(NewSession {
            session_id,
            individual_id: runtime.individual_id(),
        })?;
        runtime.set_trace_base(TraceCorrelation {
            session_id: Some(session_id),
            turn_id: None,
            episode_id: None,
            ..runtime.trace().base()
        });
        runtime
            .trace()
            .record(TraceEventKind::SessionStarted, TraceCorrelation::default());
        Ok(Self {
            session_id,
            individual_id: runtime.individual_id(),
            subject: subject.to_owned(),
            source_id: format!("text-chat:{subject}"),
            sequence: 0,
            privacy,
            started: Instant::now(),
            research,
            writer: None,
            debug_context: false,
            debug_trace: false,
            core_state: ConversationCoreState::default(),
            decision_provider,
            language_providers,
            language_candidates,
            last_generated_candidates: Vec::new(),
            last_generation_latency_ms: 0,
            last_context: None,
        })
    }

    /// Surface the assembled workspace on each turn (`--debug-context`).
    pub fn set_debug_context(&mut self, enabled: bool) {
        self.debug_context = enabled;
    }

    /// Include the secret-free Jev/LLM/core decision trace in replies.
    pub fn set_debug_trace(&mut self, enabled: bool) {
        self.debug_trace = enabled;
    }

    /// Latest attempt metadata remains available even when Jev rejects or fails.
    pub fn last_generated_candidates(&self) -> &[GeneratedLanguageCandidate] {
        &self.last_generated_candidates
    }

    pub fn last_generation_latency_ms(&self) -> u64 {
        self.last_generation_latency_ms
    }

    /// The workspace assembled for the most recent turn, as inspectable JSON.
    pub fn last_context(&self) -> Option<&serde_json::Value> {
        self.last_context.as_ref()
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn turn_count(&self) -> u64 {
        self.sequence
    }

    /// Return the subject-scoped conversation history for a frontend that
    /// needs to restore its transcript without taking ownership of the
    /// canonical store.  This is display/context data, not durable persona
    /// state.
    pub fn history_for_display(
        &self,
        runtime: &Runtime,
    ) -> Result<Vec<ConversationMessage>, RuntimeError> {
        let operative = c0::operative(runtime)?;
        self.history(runtime, operative.view.params.retrieval.history_messages)
    }

    /// Add or replace an operator-registered language organ for subsequent
    /// turns. The provider is usable only when the session privacy scope
    /// admits its declared locality; registration itself remains a config/UI
    /// concern and may be retained for a later unconstrained session.
    pub fn set_language_provider(
        &mut self,
        id: &str,
        provider_config: &crate::config::PersonaProviderConfig,
    ) -> Result<(), RuntimeError> {
        validate_language_provider_id(id)?;
        if id == PRIMARY_LANGUAGE_PROVIDER_ID {
            return Err(RuntimeError::PersonaConfig {
                message: "language provider id 'primary' is reserved".to_owned(),
            });
        }
        let provider = provider_config.build_persona()?;
        self.language_providers.remove(id);
        self.language_candidates
            .retain(|candidate| candidate.id != id);
        if self.privacy.admits(provider_config.locality)
            && language_provider_auth_available(provider_config)
        {
            let model = provider_config.model.clone();
            let provider_kind = language_provider_kind(provider_config);
            self.language_providers.insert(
                id.to_owned(),
                Arc::new(PersonaLanguageProvider::with_id(
                    provider,
                    id,
                    provider_kind,
                    model.clone(),
                )),
            );
            self.language_candidates.push(LanguageProviderCandidate {
                id: id.to_owned(),
                provider: provider_kind.to_owned(),
                model,
                endpoint: public_endpoint_origin(&provider_config.base_url),
                telemetry: LanguageProviderTelemetry::default().snapshot(),
            });
        }
        Ok(())
    }

    /// Remove an operator-registered language organ from subsequent turns.
    pub fn remove_language_provider(&mut self, id: &str) -> Result<(), RuntimeError> {
        if id == PRIMARY_LANGUAGE_PROVIDER_ID {
            return Err(RuntimeError::PersonaConfig {
                message: "the primary language provider cannot be removed".to_owned(),
            });
        }
        self.language_providers.remove(id);
        self.language_candidates
            .retain(|candidate| candidate.id != id);
        Ok(())
    }

    /// Secret-free provider observations for the desktop connection panel.
    pub fn language_provider_telemetry(
        &self,
    ) -> BTreeMap<String, LanguageProviderTelemetrySnapshot> {
        self.language_candidates
            .iter()
            .map(|candidate| (candidate.id.clone(), candidate.telemetry.clone()))
            .collect()
    }

    /// Start every admitted language organ concurrently and return results in
    /// completion order. The caller may accept an early candidate without
    /// waiting for slower organs; late sends are simply dropped once the turn
    /// no longer needs them. Provider calls themselves are not force-cancelled.
    fn start_language_race(
        &self,
        request: &crate::llm_jev::LanguageRequest,
    ) -> (mpsc::Receiver<LanguageAttempt>, usize) {
        let (tx, rx) = mpsc::channel();
        let count = self.language_providers.len();
        for (provider_id, provider) in &self.language_providers {
            let tx = tx.clone();
            let provider_id = provider_id.clone();
            let provider = Arc::clone(provider);
            let request = request.clone();
            std::thread::spawn(move || {
                let attempt = LanguageAttempt::generate(&provider_id, provider.as_ref(), &request);
                let _ = tx.send(attempt);
            });
        }
        drop(tx);
        (rx, count)
    }

    fn record_language_attempt(&mut self, attempt: &LanguageAttempt, attempt_index: u8) {
        let (response_bytes, response_digest, error_code) = match &attempt.result {
            Ok(result) => {
                self.record_provider_success(&attempt.provider_id, attempt.elapsed_ms);
                let response = &result.persona.response_intent;
                (
                    Some(response.len()),
                    Some(content_digest(response.as_bytes())),
                    None,
                )
            }
            Err(error) => {
                self.record_provider_failure(
                    &attempt.provider_id,
                    attempt.elapsed_ms,
                    error.code(),
                );
                (None, None, Some(error.code().to_owned()))
            }
        };
        let metadata = self
            .language_candidates
            .iter()
            .find(|candidate| candidate.id == attempt.provider_id)
            .expect("generated provider is registered");
        self.last_generated_candidates
            .push(GeneratedLanguageCandidate {
                id: attempt.provider_id.clone(),
                attempt: attempt_index,
                provider: attempt.result.as_ref().map_or_else(
                    |_| metadata.provider.clone(),
                    |result| result.provider.clone(),
                ),
                model: metadata.model.clone(),
                latency_ms: attempt.elapsed_ms,
                response_bytes,
                response_digest,
                error_code,
                telemetry: metadata.telemetry.clone(),
            });
    }

    fn record_provider_success(&mut self, provider_id: &str, latency_ms: u64) {
        if let Some(candidate) = self
            .language_candidates
            .iter_mut()
            .find(|candidate| candidate.id == provider_id)
        {
            let mut telemetry = LanguageProviderTelemetry {
                calls: candidate.telemetry.calls,
                successes: candidate.telemetry.successes,
                failures: candidate.telemetry.failures,
                last_latency_ms: candidate.telemetry.last_latency_ms,
                ewma_latency_ms: candidate.telemetry.ewma_latency_ms,
                last_error: candidate.telemetry.last_error.clone(),
            };
            telemetry.record_success(latency_ms);
            candidate.telemetry = telemetry.snapshot();
        }
    }

    fn record_provider_failure(&mut self, provider_id: &str, latency_ms: u64, error_code: &str) {
        if let Some(candidate) = self
            .language_candidates
            .iter_mut()
            .find(|candidate| candidate.id == provider_id)
        {
            let mut telemetry = LanguageProviderTelemetry {
                calls: candidate.telemetry.calls,
                successes: candidate.telemetry.successes,
                failures: candidate.telemetry.failures,
                last_latency_ms: candidate.telemetry.last_latency_ms,
                ewma_latency_ms: candidate.telemetry.ewma_latency_ms,
                last_error: candidate.telemetry.last_error.clone(),
            };
            telemetry.record_failure(latency_ms, error_code);
            candidate.telemetry = telemetry.snapshot();
        }
    }

    fn generate_language(
        &mut self,
        provider_id: &str,
        request: &crate::llm_jev::LanguageRequest,
    ) -> Result<crate::llm_jev::LanguageResult, RuntimeError> {
        let provider = self.language_providers.get(provider_id).ok_or_else(|| {
            RuntimeError::Conversation(crate::llm_jev::ConversationError::InvalidDecision(
                "selected language provider is not registered".to_owned(),
            ))
        })?;
        let attempt = LanguageAttempt::generate(provider_id, provider.as_ref(), request);
        self.record_language_attempt(&attempt, 1);
        attempt.result.map_err(RuntimeError::Conversation)
    }

    fn assess_language_responses(
        &self,
        request: &crate::llm_jev::LanguageRequest,
        results: &[LanguageResult],
        evidence: &DecisionEvidence,
    ) -> Result<(ResponseAssessment, usize), RuntimeError> {
        let telemetry = self.language_provider_telemetry();
        let candidate_ids = results
            .iter()
            .map(|result| {
                self.language_candidates
                    .iter()
                    .position(|candidate| candidate.id == result.provider_id)
                    .map(|index| format!("candidate-{index}"))
                    .ok_or_else(|| {
                        ConversationError::InvalidDecision(
                            "response provider has no anonymous candidate identity".to_owned(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, ConversationError>>()?;
        let candidates = results
            .iter()
            .enumerate()
            .map(|(index, result)| LanguageResponseCandidate {
                id: candidate_ids[index].clone(),
                provider: result.provider.clone(),
                model: result.model.clone(),
                latency_ms: result.latency_ms,
                response_bytes: result.persona.response_intent.len(),
                response_digest: content_digest(result.persona.response_intent.as_bytes()),
                response: result.persona.response_intent.clone(),
                telemetry: telemetry[&result.provider_id].clone(),
            })
            .collect();
        let attempts: BTreeMap<String, u8> = results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let digest = content_digest(result.persona.response_intent.as_bytes());
                let attempt = self
                    .last_generated_candidates
                    .iter()
                    .rev()
                    .find(|candidate| {
                        candidate.id == result.provider_id
                            && candidate.error_code.is_none()
                            && candidate.response_digest.as_deref() == Some(digest.as_str())
                    })
                    .ok_or_else(|| {
                        ConversationError::InvalidDecision(
                            "candidate attempt is missing".to_owned(),
                        )
                    })?;
                Ok((candidate_ids[index].clone(), attempt.attempt))
            })
            .collect::<Result<_, ConversationError>>()?;
        let assessment = self
            .decision_provider
            .assess_responses(&ResponseAssessmentRequest {
                selection: LanguageResponseSelectionRequest {
                    user_text: request.user_text.clone(),
                    speech_act: request.speech_act.clone(),
                    state: request.core_state.clone(),
                    candidates,
                },
                evidence: evidence.clone(),
                attempts: attempts.clone(),
            })?;
        if assessment.selection.fallback || assessment.gate.fallback {
            return Err(ConversationError::DecisionUnavailable(
                "Jev fallback is not allowed for response assessment".to_owned(),
            )
            .into());
        }
        let selected_index = candidate_ids
            .iter()
            .position(|id| id == &assessment.selection.provider_id)
            .ok_or_else(|| {
                ConversationError::InvalidDecision(
                    "selected anonymous response candidate is not available".to_owned(),
                )
            })?;
        let selected = &results[selected_index];
        if assessment.candidate_id != candidate_ids[selected_index]
            || assessment.candidate_digest
                != content_digest(selected.persona.response_intent.as_bytes())
            || assessment.evidence_digest != evidence.snapshot_digest
            || attempts.get(&candidate_ids[selected_index]) != Some(&assessment.attempt)
        {
            return Err(ConversationError::InvalidDecision(
                "response assessment does not match the current candidate and evidence".to_owned(),
            )
            .into());
        }
        Ok((assessment, selected_index))
    }

    /// The writer cache, for reflection cycles run inside this session.
    pub fn writer_cache(&mut self) -> &mut Option<WriterIdentity> {
        &mut self.writer
    }

    /// `emit` must return only after the output surface has accepted/flushed
    /// the response. This is delivery to that surface, not human acknowledgement.
    pub fn turn(
        &mut self,
        runtime: &mut Runtime,
        text: &str,
        emit: impl FnOnce(&DialogueReply) -> std::io::Result<()>,
    ) -> Result<DialogueReply, RuntimeError> {
        self.last_generated_candidates.clear();
        self.last_generation_latency_ms = 0;
        if runtime.individual_id() != self.individual_id {
            return Err(RuntimeError::Usage(
                "dialogue belongs to another individual".to_owned(),
            ));
        }
        if text.trim().is_empty() || text.len() > MAX_INPUT_BYTES {
            return Err(RuntimeError::Usage(format!(
                "input must be nonempty and at most {MAX_INPUT_BYTES} UTF-8 bytes"
            )));
        }
        runtime.config().persona.check_privacy(self.privacy)?;
        if self.decision_provider.is_external() && !self.privacy.admits(LocalityClass::External) {
            return Err(RuntimeError::PersonaPrivacy {
                privacy: self.privacy,
                locality: LocalityClass::External,
            });
        }
        for id in self.language_providers.keys() {
            if let Some(config) = runtime.config().language_providers.get(id)
                && !self.privacy.admits(config.locality)
            {
                return Err(RuntimeError::PersonaPrivacy {
                    privacy: self.privacy,
                    locality: config.locality,
                });
            }
        }
        let backend = self
            .language_providers
            .get(PRIMARY_LANGUAGE_PROVIDER_ID)
            .expect("primary language provider is always registered")
            .descriptor();
        // The operative view in force this turn: retrieval knobs, conversation
        // policy and the self model all come from the derived lane's head.
        let operative = c0::operative(runtime)?;
        let history = self.history(runtime, operative.view.params.retrieval.history_messages)?;
        let turn_id = TurnId::generate(runtime.ids().as_ref());
        runtime.store().record_turn(NewTurn {
            turn_id,
            session_id: self.session_id,
            individual_id: self.individual_id,
            sequence: self.sequence,
        })?;
        let sequence = self.sequence;
        self.sequence += 1;
        runtime.set_trace_base(TraceCorrelation {
            session_id: Some(self.session_id),
            turn_id: Some(turn_id),
            episode_id: None,
            ..runtime.trace().base()
        });
        runtime
            .trace()
            .record(TraceEventKind::TurnStarted, TraceCorrelation::default());
        let input_evidence_id = self.append(
            runtime,
            turn_id,
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
            serde_json::json!({"text": text}),
        )?;
        let current_input = CurrentInput {
            evidence_id: input_evidence_id,
            text: text.to_owned(),
        };
        // Retrieval is subject-bound everywhere: relationship and episodic
        // records are scoped to this interlocutor, and recalled evidence is
        // filtered to this channel's source id.
        let mut exclude: BTreeSet<EvidenceId> = history.iter().map(|m| m.evidence_id).collect();
        exclude.insert(input_evidence_id);
        let recall_pool = RecallPool::load(
            runtime.store(),
            self.individual_id,
            &self.subject,
            &self.source_id,
            text,
            &operative.view.params,
            &exclude,
        )?;
        let (mut memories, mut recalled_evidence) = recall_pool.baseline();
        let head = runtime.head()?;
        let mut workspace = WorkspaceBuilder::new(self.individual_id, runtime.now())
            .with_continuity(&head)
            .with_current_input(&current_input)
            .with_memories(&memories)
            .with_self_state(&operative.view.self_model)
            .with_active_policy(&operative.view.params, operative.activation_seq)
            .with_recalled_evidence(&recalled_evidence)
            .build();
        let mio = runtime
            .config()
            .mio
            .as_ref()
            .map(|binding| binding.observe(runtime.clocks().wall.as_ref()));
        let mio_context = if let Some(observation) = &mio {
            let value = serde_json::json!(observation);
            let evidence_id = self.append(
                runtime,
                turn_id,
                EvidenceKind::SystemEvent,
                OriginClass::Observed,
                serde_json::json!({"event": "mio_observed", "observation": value}),
            )?;
            Some(serde_json::json!({"evidence_id": evidence_id, "observation": value}))
        } else {
            None
        };
        // Reuse this turn's already validated MIO snapshot for recall cues.
        let mut research_query = text.to_owned();
        if ["mio", "状態", "調子", "個体"]
            .iter()
            .any(|term| text.to_lowercase().contains(term))
            && let Some(observation) = &mio
        {
            research_query.push_str(observation.research_topics());
        }
        let mut research_context =
            serde_json::json!(self.research.context(runtime.store(), &research_query)?);
        let recalled = research_context["selected"].as_array().map_or(0, Vec::len);
        if recalled > 0 {
            let evidence_id = self.append(
                runtime,
                turn_id,
                EvidenceKind::LibraryExcerpt,
                OriginClass::Reported,
                serde_json::json!({"event": "research_recalled", "context": research_context}),
            )?;
            research_context["evidence_id"] = serde_json::json!(evidence_id);
        }
        let core_state = ConversationCoreState::for_input(&self.core_state, sequence, text);
        let mut observed = serde_json::json!({
            "observed_at": runtime.now(),
            "individual_id": self.individual_id,
            "continuity_generation": head.generation.0,
            "session_turn": sequence + 1,
            "session_uptime_seconds": self.started.elapsed().as_secs(),
            "os": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "interface": "text",
            "history_messages_available": history.len(),
            "retained_memory_records_available": memories.len(),
            "research_findings_recalled": recalled,
            "conversation_core_state": core_state.clone(),
            "speech_output": "not_connected",
            "body_sensors": "not_connected_to_this_interface",
            "experimental_neural_state": match mio.as_ref().map(|m| m.connection.as_str()) {
                Some("connected") => "recorded_mio_evaluations_only",
                Some(_) => "mio_unavailable",
                None => "not_connected_to_this_interface",
            }
        });
        let mut input = PersonaTurnInput::with_workspace(
            TurnContext {
                individual_id: self.individual_id,
                session_id: self.session_id,
                turn_id,
            },
            current_input,
            &workspace,
            SessionWorkingState {
                turn_sequence: sequence,
                resumed: sequence == 0 && !history.is_empty(),
                delegations: 0,
            },
        );
        if let Some(seed) = runtime.config().persona_seed()? {
            input.envelope = input.envelope.with_seed(seed);
        }
        input.envelope.conversation_history = history.clone();
        input.envelope.observed_runtime = Some(observed.clone());
        input.envelope.mio_observation = mio_context.clone();
        input.envelope.research_findings = Some(research_context.clone());
        input.envelope.conversation_core =
            Some(serde_json::to_value(&core_state).map_err(|error| {
                RuntimeError::Conversation(crate::llm_jev::ConversationError::Serialization(
                    error.to_string(),
                ))
            })?);
        let preparation_evidence = decision_evidence(&input)?;
        let preparation = self
            .decision_provider
            .prepare_turn(&TurnPreparationRequest {
                invocation: DecisionRequest {
                    kind: DecisionKind::InvocationGate,
                    user_text: text.to_owned(),
                    speech_act: core_state.speech_act.clone(),
                    required_information: required_information(&core_state),
                    candidate_response: None,
                    candidate_digest: None,
                    state: core_state.clone(),
                },
                evidence: preparation_evidence.clone(),
                recall_candidates: recall_pool.candidates(),
            })?;
        if preparation.evidence_digest != preparation_evidence.snapshot_digest {
            return Err(ConversationError::InvalidDecision(
                "preparation does not match the current evidence".to_owned(),
            )
            .into());
        }
        let invocation_gate = preparation.invocation.clone();
        if invocation_gate.fallback {
            return Err(ConversationError::DecisionUnavailable(
                "Jev fallback is not allowed for turn preparation".to_owned(),
            )
            .into());
        }
        if !matches!(
            invocation_gate.decision,
            Decision::Speak | Decision::ObserveMore
        ) || (matches!(invocation_gate.decision, Decision::ObserveMore)
            && matches!(preparation.observation, ObservationNeed::None))
        {
            self.core_state = core_state;
            return Err(ConversationError::GateRefused(
                invocation_gate.decision.as_str().to_owned(),
            )
            .into());
        }
        (memories, recalled_evidence) = recall_pool.apply(&preparation.recall)?;
        // Rebuild the actual language workspace from the chosen original
        // records, preserving their types and provenance. No memory is written.
        workspace = WorkspaceBuilder::new(self.individual_id, runtime.now())
            .with_continuity(&head)
            .with_current_input(&input.input)
            .with_memories(&memories)
            .with_self_state(&operative.view.self_model)
            .with_active_policy(&operative.view.params, operative.activation_seq)
            .with_recalled_evidence(&recalled_evidence)
            .build();
        let selected = PersonaEnvelope::from_workspace(&workspace, input.envelope.session);
        input.envelope.relationship = selected.relationship;
        input.envelope.episodic = selected.episodic;
        input.envelope.recalled_evidence = selected.recalled_evidence;
        observed["retained_memory_records_available"] = serde_json::json!(memories.len());
        if matches!(preparation.observation, ObservationNeed::Runtime) {
            // One bounded local refresh. No sensor or cloud polling is implied.
            observed["observed_at"] = serde_json::json!(runtime.now());
            observed["session_uptime_seconds"] =
                serde_json::json!(self.started.elapsed().as_secs());
        }
        input.envelope.observed_runtime = Some(observed.clone());
        input.envelope.response_guidance = observation_guidance(
            preparation.observation,
            !memories.is_empty() || !recalled_evidence.is_empty(),
            recalled > 0,
        );
        let assessment_evidence = decision_evidence(&input)?;
        let mut context_digest = json_digest(&serde_json::json!(input));
        let history_messages = input.envelope.conversation_history.len();
        // The inspectable context: what the model was actually shown, in the
        // shape the C0 spec asks `--debug-context` to expose.
        let mut workspace_context = serde_json::json!({
            "activation_seq": operative.activation_seq,
            "recent_context": input.envelope.conversation_history,
            "relationship_memories": input.envelope.relationship,
            "episodic_memories": input.envelope.episodic,
            "recalled_evidence": input.envelope.recalled_evidence,
            "self_state": input.envelope.durable_self,
            "active_policy": input.envelope.active_policy,
            "open_threads": self.open_threads(runtime, &operative.view)?,
            "body_state": input.envelope.body_state,
            "conversation_core": input.envelope.conversation_core,
            "response_guidance": input.envelope.response_guidance,
            "decision_evidence_digest": assessment_evidence.snapshot_digest,
            "generation_input_digest": context_digest,
            "workspace_digest": workspace.digest(),
        });
        self.last_context = Some(workspace_context.clone());
        runtime.trace().record_with(
            TraceEventKind::PersonaInvoked,
            TraceCorrelation {
                persona_backend_id: Some(backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({
                "input_digest": context_digest,
                "history_messages": history_messages,
                "core_state": core_state.clone(),
                "invocation_gate": invocation_gate.clone(),
                "preparation": preparation,
            }),
        );
        let provider_candidates = self.language_candidates.clone();
        let language_request = crate::llm_jev::LanguageRequest {
            user_text: text.to_owned(),
            speech_act: core_state.speech_act.clone(),
            goal: core_state.active_goal.clone(),
            core_state: core_state.clone(),
            attention: core_state.attention.clone(),
            memories: vec![
                format!("relationship_records={}", memories.len()),
                format!("recalled_evidence={}", recalled_evidence.len()),
                format!("research_findings={recalled}"),
            ],
            constraints: vec![
                "respond_in_short_japanese".to_owned(),
                "do_not_invent_memory_or_observation".to_owned(),
                "do_not_mutate_canonical_state_from_prose".to_owned(),
            ],
            recent_turns: history.clone(),
            persona_input: input,
        };
        let generation_started = Instant::now();
        let (attempt_rx, provider_count) = self.start_language_race(&language_request);
        let mut first_error = None;
        let mut accepted = None;
        let mut retry_fallback = None;
        let mut rejected_fallback = None;
        let mut race_assessments = Vec::new();
        let mut received = 0_usize;

        'race: while received < provider_count {
            let first = match attempt_rx.recv() {
                Ok(attempt) => attempt,
                Err(_) => break,
            };
            received += 1;
            let mut ready = vec![first];

            // Give simultaneously-finishing local/fast organs a tiny shared
            // grace window. This is one deadline for the whole ready batch,
            // never one delay per configured provider.
            let grace_deadline = Instant::now() + Duration::from_millis(10);
            while received < provider_count && ready.len() < 2 {
                let remaining = grace_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match attempt_rx.recv_timeout(remaining) {
                    Ok(attempt) => {
                        received += 1;
                        ready.push(attempt);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }

            let mut ready_results = Vec::new();
            for attempt in ready {
                self.record_language_attempt(&attempt, 0);
                match attempt.result {
                    Ok(result) => ready_results.push(result),
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }

            if ready_results.is_empty() {
                continue;
            }

            // Compare at most two simultaneously-ready responses. The overall
            // provider count is unbounded, but every Jev assessment remains
            // bounded to this tiny anonymous active pool.
            let (candidate_assessment, selected_ready) = self.assess_language_responses(
                &language_request,
                &ready_results,
                &assessment_evidence,
            )?;
            let selected_result = ready_results.swap_remove(selected_ready);
            let gate = candidate_assessment.gate.decision;
            race_assessments.push(candidate_assessment.clone());
            match gate {
                Decision::Accept => {
                    accepted = Some((selected_result, candidate_assessment));
                    break 'race;
                }
                Decision::Retry => {
                    if retry_fallback.is_none() {
                        retry_fallback = Some((selected_result, candidate_assessment));
                    }
                }
                Decision::Reject => {
                    rejected_fallback = Some((selected_result, candidate_assessment));
                }
                Decision::Speak | Decision::Wait | Decision::ObserveMore => {
                    return Err(RuntimeError::Conversation(
                        ConversationError::InvalidDecision(
                            "response assessment returned an invocation choice".to_owned(),
                        ),
                    ));
                }
            }
        }
        // Stop accepting late race results immediately. In-flight HTTP calls
        // may still finish, but their send fails instead of growing a queue.
        drop(attempt_rx);
        self.last_generation_latency_ms =
            u64::try_from(generation_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (final_result, mut assessment) = if let Some(accepted) = accepted {
            accepted
        } else if let Some(retry) = retry_fallback {
            retry
        } else if let Some(rejected) = rejected_fallback {
            rejected
        } else {
            return Err(RuntimeError::Conversation(first_error.unwrap_or(
                ConversationError::InvalidDecision("NO_VALID_CANDIDATES".to_owned()),
            )));
        };
        let mut language_results = vec![final_result];
        let mut selected_index = 0_usize;
        let mut assessments = race_assessments;
        let mut repaired_context = None;
        let mut response_gate = None;
        let mut retry_count = 0_u8;
        for attempt in 0..=1 {
            if attempt > 0 {
                // Replace only the retried organ's candidate, then let Jev compare
                // the repaired candidate. The old candidate can no longer be accepted.
                let mut repair_request = language_request.clone();
                let repair = assessment.repair_reason.as_instruction().unwrap_or(
                    "先の候補を見直し、相手の質問と提示された根拠に合う短い日本語の回答を作り直してください。",
                );
                // Preserve any observation/clarification instruction as well
                // as the original input. The rejected prose is wire-only.
                let instruction = match &language_request.persona_input.envelope.response_guidance {
                    Some(guidance) => format!("{}\n{repair}", guidance.instruction),
                    None => repair.to_owned(),
                };
                repair_request.persona_input.envelope.response_guidance = Some(ResponseGuidance {
                    reason_code: assessment.repair_reason.as_str().to_owned(),
                    instruction,
                    previous_response_digest: Some(assessment.candidate_digest.clone()),
                    previous_response: Some(
                        language_results[selected_index]
                            .persona
                            .response_intent
                            .clone(),
                    ),
                });
                repair_request.constraints.push(repair.to_owned());
                let retried_provider_id = language_results[selected_index].provider_id.clone();
                repaired_context = Some((
                    retried_provider_id.clone(),
                    json_digest(&serde_json::json!(repair_request.persona_input)),
                    repair_request
                        .persona_input
                        .envelope
                        .response_guidance
                        .clone(),
                ));
                language_results[selected_index] =
                    self.generate_language(&retried_provider_id, &repair_request)?;
                (assessment, selected_index) = self.assess_language_responses(
                    &language_request,
                    &language_results,
                    &assessment_evidence,
                )?;
            }
            if attempt > 0 || assessments.is_empty() {
                assessments.push(assessment.clone());
            }
            let gate = assessment.gate.clone();
            match gate.decision {
                Decision::Accept => {
                    response_gate = Some(gate);
                    break;
                }
                Decision::Retry if attempt == 0 => {
                    retry_count = 1;
                }
                Decision::Retry => {
                    return Err(RuntimeError::Conversation(
                        crate::llm_jev::ConversationError::GateRefused(
                            "RETRY_AFTER_LIMIT".to_owned(),
                        ),
                    ));
                }
                Decision::Reject => {
                    return Err(RuntimeError::Conversation(
                        crate::llm_jev::ConversationError::GateRefused("REJECT".to_owned()),
                    ));
                }
                Decision::Speak | Decision::Wait | Decision::ObserveMore => {
                    return Err(RuntimeError::Conversation(
                        crate::llm_jev::ConversationError::InvalidDecision(
                            "response gate returned an invocation choice".to_owned(),
                        ),
                    ));
                }
            }
        }
        let response_gate = response_gate.ok_or_else(|| {
            RuntimeError::Conversation(crate::llm_jev::ConversationError::GateRefused(
                "RESPONSE_GATE_MISSING".to_owned(),
            ))
        })?;
        let provider_selection = assessment.selection.clone();
        let selected_provider_id = language_results[selected_index].provider_id.clone();
        // The factual assessment snapshot remains fixed across retries, while
        // the selected generator's actual input also includes repair guidance.
        // Keep that input's provenance accurate when the repaired candidate wins.
        if let Some((repaired_id, repaired_digest, guidance)) = repaired_context
            && repaired_id == selected_provider_id
            && assessment.attempt == 1
        {
            context_digest = repaired_digest;
            workspace_context["generation_input_digest"] = serde_json::json!(context_digest);
            workspace_context["response_guidance"] = serde_json::json!(guidance);
        }
        self.last_context = Some(workspace_context.clone());
        let language_result = language_results.swap_remove(selected_index);
        let generated_candidates = self.last_generated_candidates.clone();
        let provider_selection_trace = provider_selection.clone();
        let next_core_state = core_state.after_response(
            response_gate.decision,
            &language_result.persona.response_intent,
        );
        self.core_state = next_core_state.clone();
        let conversation_trace = ConversationTrace {
            core_state_before: core_state.clone(),
            invocation_gate: invocation_gate.clone(),
            preparation: Some(preparation.clone()),
            assessments: assessments.clone(),
            provider_candidates: provider_candidates.clone(),
            provider_selection: Some(provider_selection),
            provider_telemetry: self.language_provider_telemetry(),
            generation_latency_ms: self.last_generation_latency_ms,
            generated_candidates: generated_candidates.clone(),
            language_provider_id: selected_provider_id,
            language_provider: language_result.provider.clone(),
            language_model: language_result.model.clone(),
            language_latency_ms: language_result.latency_ms,
            response_gate: response_gate.clone(),
            candidate_digest: content_digest(language_result.persona.response_intent.as_bytes()),
            retry_count,
            core_state_after: next_core_state,
        };
        let language_provider_name = language_result.provider.clone();
        let language_model = language_result.model.clone();
        let language_latency_ms = language_result.latency_ms;
        let result = language_result.persona;
        // Persona drafts are proposals, not state: submit each through the
        // canonical kernel so the mutation policy decides. The writer epoch
        // is claimed lazily here — a turn with no drafts never takes it.
        let draft_outcomes = c0::submit_drafts(
            runtime,
            Some(self.session_id),
            Some(turn_id),
            &mut self.writer,
            result.proposals.clone(),
        )?;
        let drafts_activated = draft_outcomes.iter().filter(|o| o.activated).count();
        runtime.trace().record_with(
            TraceEventKind::PersonaCompleted,
            TraceCorrelation {
                persona_backend_id: Some(result.backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({
                "draft_count": draft_outcomes.len(),
                "drafts_activated": drafts_activated,
                "invocation_gate": invocation_gate,
                "provider_selection": provider_selection_trace,
                "provider_candidates": conversation_trace.provider_candidates.clone(),
                "provider_telemetry": conversation_trace.provider_telemetry.clone(),
                "generated_candidates": generated_candidates,
                "generation_latency_ms": self.last_generation_latency_ms,
                "response_gate": response_gate,
                "assessments": assessments,
                "language_provider": language_provider_name,
                "language_model": language_model,
                "language_latency_ms": language_latency_ms,
                "retry_count": retry_count,
                "core_state_after": self.core_state,
            }),
        );
        let generated_evidence_id = self.append(
            runtime,
            turn_id,
            EvidenceKind::SystemEvent,
            OriginClass::Observed,
            serde_json::json!({
                "event": "response_generated", "text": result.response_intent,
                "backend": result.backend, "input_evidence_id": input_evidence_id,
                "input_digest": context_digest, "observed_runtime": observed,
                "mio_observation": mio_context,
                "research_findings": research_context,
                "conversation_trace": conversation_trace.clone(),
                "delivery": "not_yet_emitted"
            }),
        )?;
        let reply = DialogueReply {
            individual_id: self.individual_id,
            session_id: self.session_id,
            turn_id,
            input_evidence_id,
            generated_evidence_id,
            history_messages,
            observed_runtime: observed,
            mio_observation: mio_context,
            research_findings: research_context,
            persona_backend: result.backend,
            response: result.response_intent,
            c0: None,
            debug_context: None,
            llm_jev: self.debug_trace.then_some(conversation_trace),
        };
        emit(&reply)
            .map_err(|error| RuntimeError::Usage(format!("response output failed: {error}")))?;
        let emitted_evidence_id = self.append(
            runtime,
            turn_id,
            EvidenceKind::AgentUtterance,
            OriginClass::Observed,
            serde_json::json!({
                "text": reply.response, "generated_evidence_id": generated_evidence_id,
                "backend": reply.persona_backend, "delivery": "output_surface_accepted",
                "human_acknowledgement": false
            }),
        )?;
        runtime.trace().record_with(
            TraceEventKind::FinalExpression,
            TraceCorrelation {
                persona_backend_id: Some(reply.persona_backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({"expression_digest": content_digest(reply.response.as_bytes())}),
        );
        runtime
            .trace()
            .record(TraceEventKind::ResponseEmitted, TraceCorrelation::default());
        // Deterministic per-turn metrics, recorded in the lane — the
        // reflection cycle reads them, and they are the before/after record.
        let metrics = eval::measure_turn(
            text,
            &reply.response,
            &history,
            &workspace,
            operative.view.params.conversation.max_response_chars,
        );
        runtime
            .store()
            .c0_record_evaluation(&kamimusuhi_store_sqlite::c0::C0Evaluation {
                evaluation_id: kamimusuhi_core::ids::C0EvaluationId::generate(
                    runtime.ids().as_ref(),
                ),
                individual_id: self.individual_id,
                scope: "turn".to_owned(),
                subject_key: Some(self.subject.clone()),
                turn_id: Some(turn_id),
                proposal_id: None,
                metrics: serde_json::to_value(&metrics).unwrap_or(serde_json::Value::Null),
                evaluator: kamimusuhi_core::c0::EVALUATOR_KIND.to_owned(),
                created_at: runtime.now(),
            })?;
        let _ = emitted_evidence_id;
        Ok(DialogueReply {
            c0: Some(TurnC0 {
                activation_seq: operative.activation_seq,
                memories_surfaced: metrics.memories_surfaced,
                drafts_submitted: draft_outcomes.len(),
                drafts_activated,
                metrics,
            }),
            debug_context: self.debug_context.then_some(workspace_context),
            ..reply
        })
    }

    /// Open threads for the inspectable context: pending derived-lane
    /// proposals plus the self model's open questions. Deterministic — read
    /// from state, not inferred from prose.
    fn open_threads(
        &self,
        runtime: &Runtime,
        view: &kamimusuhi_core::c0::OperativeView,
    ) -> Result<serde_json::Value, RuntimeError> {
        let pending = runtime.store().c0_proposals(
            self.individual_id,
            Some(kamimusuhi_core::c0::ProposalStatus::Pending),
        )?;
        let open_questions: Vec<serde_json::Value> = view
            .self_model
            .field(kamimusuhi_core::c0::SelfField::OpenQuestions)
            .iter()
            .map(|e| e.value.clone())
            .collect();
        Ok(serde_json::json!({
            "pending_proposals": pending
                .iter()
                .map(|p| serde_json::json!({"proposal_id": p.proposal_id, "target": p.target}))
                .collect::<Vec<_>>(),
            "open_questions": open_questions,
        }))
    }

    fn history(
        &self,
        runtime: &Runtime,
        limit: usize,
    ) -> Result<Vec<ConversationMessage>, RuntimeError> {
        let records = runtime.store().recent_conversation(
            self.individual_id,
            &self.source_id,
            limit.clamp(1, 128),
        )?;
        let mut bytes = 0;
        let mut messages = Vec::new();
        for record in records.into_iter().rev() {
            let Some(text) = record.payload.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            if bytes + text.len() > HISTORY_BYTES {
                break;
            }
            bytes += text.len();
            messages.push(ConversationMessage {
                evidence_id: record.evidence_id,
                role: if record.kind == EvidenceKind::UserUtterance {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                },
                text: text.to_owned(),
            });
        }
        messages.reverse();
        Ok(messages)
    }

    fn append(
        &self,
        runtime: &Runtime,
        turn_id: TurnId,
        kind: EvidenceKind,
        origin_class: OriginClass,
        payload: serde_json::Value,
    ) -> Result<EvidenceId, RuntimeError> {
        let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
        let digest = json_digest(&payload);
        runtime.store().append(NewEvidence {
            evidence_id,
            individual_id: self.individual_id,
            session_id: Some(self.session_id),
            turn_id: Some(turn_id),
            kind,
            origin_class,
            payload,
            source: EvidenceSource {
                source_id: Some(self.source_id.clone()),
                source_sequence: None,
                content_digest: Some(digest.clone()),
            },
            retention_class: RetentionClass::Standard,
        })?;
        runtime.trace().record_with(
            TraceEventKind::EvidenceRecorded,
            TraceCorrelation {
                evidence_id: Some(evidence_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({"kind": kind, "content_digest": digest}),
        );
        Ok(evidence_id)
    }
}

#[cfg(test)]
mod fanout_tests {
    use super::*;
    use crate::llm_jev::{LanguageRequest, MockLanguageProvider, RuleBasedDecisionProvider};
    use crate::{ResourceImplementation, RuntimeOptions};
    use kamimusuhi_core::mutation::{MutationDomain, MutationOperation};
    use kamimusuhi_core::persona::ProposalDraft;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    struct CoordinatedProvider {
        id: String,
        started: mpsc::Sender<String>,
        released: Arc<AtomicBool>,
    }

    impl LanguageProvider for CoordinatedProvider {
        fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
            self.started.send(self.id.clone()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.released.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    return Err(ConversationError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut result = MockLanguageProvider.generate(request)?;
            result.persona.response_intent = format!("{} response", self.id);
            if self.id != "primary" {
                result.persona.proposals.push(ProposalDraft {
                    domain: MutationDomain::Relationship,
                    operation: MutationOperation::Fact,
                    subject_key: Some("alice".to_owned()),
                    candidate: serde_json::json!({ "marker": "UNSELECTED_PROPOSAL" }),
                    evidence_refs: vec![request.persona_input.input.evidence_id],
                    supersedes: None,
                    origin_class: OriginClass::Inferred,
                });
            }
            // Deliberately retains MockLanguageProvider's primary ID. The host
            // must bind this response to the registered organ that made it.
            Ok(result)
        }

        fn descriptor(&self) -> PersonaBackendDescriptor {
            MockLanguageProvider.descriptor()
        }
    }

    #[test]
    fn all_organs_start_before_release_and_only_selected_proposals_reach_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::init(
            dir.path(),
            RuntimeOptions::deterministic(42),
            ResourceImplementation::FakeA,
        )
        .unwrap();
        let mut session = DialogueSession::start_with_providers(
            &mut runtime,
            "alice",
            PrivacyConstraint::LocalOnly,
            Box::new(RuleBasedDecisionProvider),
            Some(Box::new(MockLanguageProvider)),
        )
        .unwrap();
        session.set_debug_trace(true);
        let (started_tx, started_rx) = mpsc::channel();
        let released = Arc::new(AtomicBool::new(false));
        for id in ["primary", "extra-a", "extra-b"] {
            if id != "primary" {
                let mut metadata = session.language_candidates[0].clone();
                metadata.id = id.to_owned();
                session.language_candidates.push(metadata);
            }
            session.language_providers.insert(
                id.to_owned(),
                Box::new(CoordinatedProvider {
                    id: id.to_owned(),
                    started: started_tx.clone(),
                    released: Arc::clone(&released),
                }),
            );
        }
        let controller = std::thread::spawn(move || {
            let mut seen = BTreeSet::new();
            for _ in 0..3 {
                match started_rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(id) => {
                        seen.insert(id);
                    }
                    Err(_) => break,
                }
            }
            released.store(true, Ordering::SeqCst);
            seen
        });
        let reply = session
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap();
        assert_eq!(
            controller.join().unwrap(),
            BTreeSet::from([
                "primary".to_owned(),
                "extra-a".to_owned(),
                "extra-b".to_owned()
            ])
        );
        assert_eq!(reply.response, "primary response");
        assert_eq!(reply.c0.as_ref().unwrap().drafts_submitted, 0);
        assert!(session.writer.is_none());
        let trace = reply.llm_jev.unwrap();
        assert_eq!(trace.generated_candidates.len(), 3);
        for telemetry in trace.provider_telemetry.values() {
            assert_eq!(telemetry.calls, 1);
            assert_eq!(telemetry.successes, 1);
        }
        let metadata = serde_json::to_string(&trace).unwrap();
        assert!(!metadata.contains("UNSELECTED_PROPOSAL"));
        assert!(!metadata.contains("extra-a response"));
        let history = session.history_for_display(&runtime).unwrap();
        assert!(
            history
                .iter()
                .all(|message| !message.text.contains("extra-"))
        );
    }
}
