//! Deterministic in-process providers for tests, soak runs and offline
//! harnesses. No sockets and no wall-clock dependence beyond caller-chosen
//! delays; generation outcomes, gate verdicts and Jev outages are all
//! scripted or seeded so scenarios replay identically on every run.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaTurnResult};

use super::{
    CancellationToken, ConversationError, Decision, DecisionKind, DecisionProvider,
    DecisionRequest, DecisionResult, LanguageProvider, LanguageRequest, LanguageResult,
    MAX_JEV_CANDIDATE_RESPONSE_BYTES, ObservationNeed, ProviderSelectionResult, RecallRelevance,
    RepairReason, ResponseAssessment, ResponseAssessmentRequest, TurnPreparation,
    TurnPreparationRequest,
};

/// Small deterministic PRNG (xorshift64*). No dependency, fully seedable —
/// soak tests replay the same sequence on every run.
pub struct XorShift64(u64);

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Advance the state and return the next pseudo-random value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform draw in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Deterministic backend-id material for an in-process organ.
fn backend_id(id: &str) -> PersonaBackendId {
    let digest = content_digest(id.as_bytes());
    let value = u128::from_str_radix(&digest[..32], 16).unwrap_or(0);
    PersonaBackendId::from_u128(value)
}

fn descriptor(id: &str) -> PersonaBackendDescriptor {
    PersonaBackendDescriptor {
        backend_id: backend_id(id),
        kind: "scripted-language".to_owned(),
        name: format!("scripted-language-{id}"),
        version: "1".to_owned(),
    }
}

fn language_result(request: &LanguageRequest, id: &str, response: String) -> LanguageResult {
    LanguageResult {
        persona: PersonaTurnResult {
            context: request.persona_input.context,
            backend: descriptor(id),
            response_intent: response,
            proposals: Vec::new(),
        },
        provider: "scripted-language".to_owned(),
        provider_id: id.to_owned(),
        model: format!("scripted-model-{id}"),
        latency_ms: 0,
    }
}

/// Sleep in short slices so cooperative cancellation is observed quickly.
/// Returns `false` when the token fired before the delay elapsed.
fn interruptible_sleep(delay: Duration, cancellation: Option<&CancellationToken>) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(1)));
    }
}

/// What a scripted organ does once its delay has elapsed.
#[derive(Debug, Clone, Copy)]
pub enum ScriptedOutcome {
    /// Answer with this text.
    Respond(&'static str),
    /// Fail with a freshly built error on every call.
    Fail(fn() -> ConversationError),
    /// Answer with an empty body — the host rejects it as `EMPTY_RESPONSE`.
    Empty,
    /// Answer with an oversized body — rejected as `RESPONSE_TOO_LARGE`.
    Oversized,
    /// Never answer on its own. Honouring cancellation returns `CANCELLED`
    /// promptly; a provider that cannot cancel returns `TIMEOUT` at
    /// `deadline`, which stands in for its own transport timeout.
    Hang {
        deadline: Duration,
        honour_cancellation: bool,
    },
}

/// A language organ with a fixed scripted behaviour per call. Every organ
/// honours the race cancellation token while it waits; whether `Hang`
/// respects it is part of the script.
pub struct ScriptedLanguageProvider {
    id: String,
    delay: Duration,
    outcome: ScriptedOutcome,
    calls: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
}

impl ScriptedLanguageProvider {
    pub fn new(id: impl Into<String>, delay: Duration, outcome: ScriptedOutcome) -> Self {
        Self {
            id: id.into(),
            delay,
            outcome,
            calls: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// ~5ms, well-formed answer carrying `scripted-good`, which a scripted
    /// gate can also match on.
    pub fn fast_good(id: impl Into<String>) -> Self {
        Self::new(
            id,
            Duration::from_millis(5),
            ScriptedOutcome::Respond("scripted-good"),
        )
    }

    /// ~5ms, but the response carries the `fixture-bad` marker a scripted
    /// gate can reject.
    pub fn fast_bad(id: impl Into<String>) -> Self {
        Self::new(
            id,
            Duration::from_millis(5),
            ScriptedOutcome::Respond("fixture-bad"),
        )
    }

    /// ~60ms, well-formed answer.
    pub fn slow_good(id: impl Into<String>) -> Self {
        Self::new(
            id,
            Duration::from_millis(60),
            ScriptedOutcome::Respond("scripted-good-slow"),
        )
    }

    /// Fails quickly with `TRANSPORT`.
    pub fn erroring(id: impl Into<String>) -> Self {
        Self::new(
            id,
            Duration::from_millis(5),
            ScriptedOutcome::Fail(|| ConversationError::Transport),
        )
    }

    /// Returns an empty response, rejected by the host as `EMPTY_RESPONSE`.
    pub fn empty(id: impl Into<String>) -> Self {
        Self::new(id, Duration::from_millis(5), ScriptedOutcome::Empty)
    }

    /// Returns an oversized response, rejected as `RESPONSE_TOO_LARGE`.
    pub fn oversized(id: impl Into<String>) -> Self {
        Self::new(id, Duration::from_millis(5), ScriptedOutcome::Oversized)
    }

    /// Never answers on its own. With `honour_cancellation` the organ exits
    /// promptly when the race cancels it; otherwise it only stops at
    /// `deadline`, standing in for its own transport timeout.
    pub fn hanging(id: impl Into<String>, deadline: Duration, honour_cancellation: bool) -> Self {
        Self::new(
            id,
            Duration::ZERO,
            ScriptedOutcome::Hang {
                deadline,
                honour_cancellation,
            },
        )
    }

    /// How many `generate` calls the organ has entered.
    pub fn calls(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.calls)
    }

    /// How many `generate` calls are in flight right now — the leak gauge.
    pub fn active(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.active)
    }
}

impl LanguageProvider for ScriptedLanguageProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _guard = ActiveGuard(&self.active);
        let cancellation = request.cancellation.as_ref();
        if !interruptible_sleep(self.delay, cancellation) {
            return Err(ConversationError::Cancelled);
        }
        match &self.outcome {
            ScriptedOutcome::Respond(text) => {
                Ok(language_result(request, &self.id, (*text).to_owned()))
            }
            ScriptedOutcome::Fail(error) => Err(error()),
            ScriptedOutcome::Empty => Ok(language_result(request, &self.id, String::new())),
            ScriptedOutcome::Oversized => Ok(language_result(
                request,
                &self.id,
                "x".repeat(MAX_JEV_CANDIDATE_RESPONSE_BYTES + 1),
            )),
            ScriptedOutcome::Hang {
                deadline,
                honour_cancellation,
            } => {
                let hang_deadline = Instant::now() + *deadline;
                loop {
                    if *honour_cancellation
                        && cancellation.is_some_and(CancellationToken::is_cancelled)
                    {
                        return Err(ConversationError::Cancelled);
                    }
                    if Instant::now() >= hang_deadline {
                        return Err(ConversationError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        descriptor(&self.id)
    }
}

struct ActiveGuard<'a>(&'a AtomicUsize);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn scripted_decision_result(decision: Decision, confidence: f32) -> DecisionResult {
    let choices = match decision {
        Decision::Speak | Decision::Wait | Decision::ObserveMore => DecisionKind::InvocationGate,
        Decision::Accept | Decision::Retry | Decision::Reject => DecisionKind::ResponseGate,
    };
    DecisionResult {
        decision,
        confidence,
        probabilities: choices
            .choices()
            .iter()
            .map(|choice| {
                (
                    (*choice).to_owned(),
                    if *choice == decision.as_str() {
                        confidence
                    } else {
                        (1.0 - confidence) / (choices.choices().len() as f32 - 1.0)
                    },
                )
            })
            .collect(),
        provider: "scripted-jev".to_owned(),
        model: "scripted-jev-v0".to_owned(),
        latency_ms: 0,
        fallback: false,
        fallback_reason: None,
    }
}

fn scripted_selection(
    request: &ResponseAssessmentRequest,
    candidate_id: &str,
) -> ProviderSelectionResult {
    ProviderSelectionResult {
        provider_id: candidate_id.to_owned(),
        decision: "SELECT_RESPONSE".to_owned(),
        confidence: 1.0,
        probabilities: request
            .selection
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.id.clone(),
                    if candidate.id == candidate_id {
                        1.0
                    } else {
                        0.0
                    },
                )
            })
            .collect(),
        provider: "scripted-jev".to_owned(),
        model: "scripted-jev-v0".to_owned(),
        latency_ms: 0,
        fallback: false,
        fallback_reason: None,
    }
}

fn scripted_assessment(
    request: &ResponseAssessmentRequest,
    selected_id: &str,
    gate: Decision,
) -> Result<ResponseAssessment, ConversationError> {
    let candidate = request
        .selection
        .candidates
        .iter()
        .find(|candidate| candidate.id == selected_id)
        .ok_or_else(|| {
            ConversationError::InvalidDecision(
                "scripted assessment selected an undeclared candidate".to_owned(),
            )
        })?;
    Ok(ResponseAssessment {
        selection: scripted_selection(request, selected_id),
        gate: scripted_decision_result(gate, 0.9),
        raw_gate: gate,
        candidate_id: candidate.id.clone(),
        candidate_digest: candidate.response_digest.clone(),
        attempt: request.attempts[&candidate.id],
        evidence_digest: request.evidence.snapshot_digest.clone(),
        grounding: super::GroundingAssessment::NotEvaluated,
        attribution: super::AttributionAssessment::NotEvaluated,
        task_fit: super::TaskFitAssessment::NotEvaluated,
        repair_reason: if gate == Decision::Retry {
            RepairReason::Language
        } else {
            RepairReason::None
        },
        latency_ms: 0,
    })
}

/// A scripted decision gate. It answers the invocation gate with
/// `invocation`, evaluates each generated candidate against `gate_rules`
/// (first substring match wins, default `ACCEPT`), and can be told to fail
/// `prepare_turn` / `assess_responses` outright to model a Jev outage. Every
/// serialized request is captured in `requests` so tests can assert on
/// exactly what the gate was shown.
pub struct ScriptedDecisionProvider {
    pub invocation: Decision,
    /// `(substring, verdict)` rules evaluated against the selected
    /// candidate's response text.
    pub gate_rules: Vec<(String, Decision)>,
    /// When set, every `prepare_turn` returns this error — a Jev outage
    /// scripted at the invocation gate.
    pub prepare_error: Option<fn() -> ConversationError>,
    /// When set, every `assess_responses` returns this error — a Jev outage
    /// scripted at the response gate.
    pub assess_error: Option<fn() -> ConversationError>,
    /// Every serialized request this provider received, in order. Shared so
    /// a test can inspect the payloads after the provider moved into a
    /// session.
    pub requests: Arc<Mutex<Vec<String>>>,
}

impl ScriptedDecisionProvider {
    /// Accept every structurally valid candidate.
    pub fn accept_all() -> Self {
        Self {
            invocation: Decision::Speak,
            gate_rules: Vec::new(),
            prepare_error: None,
            assess_error: None,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Reject candidates whose response contains `marker`; accept the rest.
    pub fn reject_containing(marker: impl Into<String>) -> Self {
        Self {
            gate_rules: vec![(marker.into(), Decision::Reject)],
            ..Self::accept_all()
        }
    }

    /// Every call fails — a complete Jev outage.
    pub fn unavailable(error: fn() -> ConversationError) -> Self {
        Self {
            prepare_error: Some(error),
            assess_error: Some(error),
            ..Self::accept_all()
        }
    }

    fn gate_for(&self, response: &str) -> Decision {
        self.gate_rules
            .iter()
            .find(|(marker, _)| response.contains(marker.as_str()))
            .map_or(Decision::Accept, |(_, decision)| *decision)
    }
}

impl DecisionProvider for ScriptedDecisionProvider {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        let decision = match request.kind {
            DecisionKind::InvocationGate => self.invocation,
            DecisionKind::ResponseGate => request
                .candidate_response
                .as_deref()
                .map(|response| self.gate_for(response))
                .unwrap_or(Decision::Reject),
        };
        Ok(scripted_decision_result(decision, 0.9))
    }

    fn prepare_turn(
        &self,
        request: &TurnPreparationRequest,
    ) -> Result<TurnPreparation, ConversationError> {
        self.requests
            .lock()
            .expect("scripted requests")
            .push(serde_json::to_string(request).unwrap_or_default());
        if let Some(error) = self.prepare_error {
            return Err(error());
        }
        Ok(TurnPreparation {
            invocation: scripted_decision_result(self.invocation, 0.9),
            recall: request
                .recall_candidates
                .iter()
                .map(|candidate| (candidate.id.clone(), RecallRelevance::Uncertain))
                .collect(),
            observation: ObservationNeed::None,
            evidence_digest: request.evidence.snapshot_digest.clone(),
            latency_ms: 0,
        })
    }

    fn assess_responses(
        &self,
        request: &ResponseAssessmentRequest,
    ) -> Result<ResponseAssessment, ConversationError> {
        self.requests
            .lock()
            .expect("scripted requests")
            .push(serde_json::to_string(request).unwrap_or_default());
        if let Some(error) = self.assess_error {
            return Err(error());
        }
        // Select the first candidate that is not rejected by a rule; if every
        // candidate is rejected, select the first so its verdict is reported.
        let selected = request
            .selection
            .candidates
            .iter()
            .find(|candidate| self.gate_for(&candidate.response) != Decision::Reject)
            .or_else(|| request.selection.candidates.first())
            .ok_or_else(|| {
                ConversationError::InvalidDecision("no generated candidates".to_owned())
            })?;
        scripted_assessment(
            request,
            &selected.id.clone(),
            self.gate_for(&selected.response),
        )
    }
}

/// Per-call randomized outcome weights, in percent. Any remainder is a good
/// response. `hang` outcomes honour cancellation and fall back to a bounded
/// internal deadline.
#[derive(Debug, Clone, Copy)]
pub struct OutcomeWeights {
    pub error_pct: u64,
    pub empty_pct: u64,
    pub oversized_pct: u64,
    pub hang_pct: u64,
    /// Internal timeout for a hang that is never cancelled.
    pub hang_deadline: Duration,
}

impl Default for OutcomeWeights {
    fn default() -> Self {
        Self {
            error_pct: 0,
            empty_pct: 0,
            oversized_pct: 0,
            hang_pct: 0,
            hang_deadline: Duration::from_secs(2),
        }
    }
}

/// A language organ that draws latency and outcome from a seeded RNG on
/// every call — latency variation, timeouts, malformed/empty responses and
/// provider failures mixed under a fixed seed for the soak test.
pub struct SeededLanguageProvider {
    id: String,
    rng: Mutex<XorShift64>,
    latency_ms: (u64, u64),
    weights: OutcomeWeights,
    calls: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    counter: AtomicUsize,
}

impl SeededLanguageProvider {
    pub fn new(
        id: impl Into<String>,
        seed: u64,
        latency_ms: (u64, u64),
        weights: OutcomeWeights,
    ) -> Self {
        Self {
            id: id.into(),
            rng: Mutex::new(XorShift64::new(seed)),
            latency_ms,
            weights,
            calls: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            counter: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.calls)
    }

    pub fn active(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.active)
    }
}

impl LanguageProvider for SeededLanguageProvider {
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _guard = ActiveGuard(&self.active);
        let (delay_ms, draw) = {
            let mut rng = self.rng.lock().expect("seeded rng");
            let (lo, hi) = self.latency_ms;
            (lo + rng.below(hi.saturating_sub(lo) + 1), rng.below(100))
        };
        let cancellation = request.cancellation.as_ref();
        let outcome = if draw < self.weights.hang_pct {
            4
        } else if draw < self.weights.hang_pct + self.weights.error_pct {
            3
        } else if draw < self.weights.hang_pct + self.weights.error_pct + self.weights.empty_pct {
            2
        } else if draw
            < self.weights.hang_pct
                + self.weights.error_pct
                + self.weights.empty_pct
                + self.weights.oversized_pct
        {
            1
        } else {
            0
        };
        match outcome {
            4 => {
                // Hang: honour cancellation, else stop at the internal
                // deadline as a stand-in for the provider's own timeout.
                let deadline =
                    Instant::now() + self.weights.hang_deadline + Duration::from_millis(delay_ms);
                loop {
                    if cancellation.is_some_and(CancellationToken::is_cancelled) {
                        return Err(ConversationError::Cancelled);
                    }
                    if Instant::now() >= deadline {
                        return Err(ConversationError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            3 => {
                interruptible_sleep(Duration::from_millis(delay_ms), cancellation);
                Err(ConversationError::Transport)
            }
            2 => {
                interruptible_sleep(Duration::from_millis(delay_ms), cancellation);
                Ok(language_result(request, &self.id, String::new()))
            }
            1 => {
                interruptible_sleep(Duration::from_millis(delay_ms), cancellation);
                Ok(language_result(
                    request,
                    &self.id,
                    "x".repeat(MAX_JEV_CANDIDATE_RESPONSE_BYTES + 1),
                ))
            }
            _ => {
                if !interruptible_sleep(Duration::from_millis(delay_ms), cancellation) {
                    return Err(ConversationError::Cancelled);
                }
                let call = self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(language_result(
                    request,
                    &self.id,
                    format!("seeded response {} #{call}", self.id),
                ))
            }
        }
    }

    fn descriptor(&self) -> PersonaBackendDescriptor {
        descriptor(&self.id)
    }
}

/// Gate-verdict weights in percent. The remainder is `ACCEPT`.
#[derive(Debug, Clone, Copy)]
pub struct VerdictWeights {
    pub retry_pct: u64,
    pub reject_pct: u64,
    /// Assess calls that fail outright — exercises the degrade path.
    pub outage_pct: u64,
}

/// A decision provider that draws verdicts from a seeded RNG. `prepare_turn`
/// answers `SPEAK` unless `wait_pct` draws a `WAIT`, which exercises the
/// controlled invocation refusal path.
pub struct SeededDecisionProvider {
    rng: Mutex<XorShift64>,
    wait_pct: u64,
    verdicts: VerdictWeights,
}

impl SeededDecisionProvider {
    pub fn new(seed: u64, wait_pct: u64, verdicts: VerdictWeights) -> Self {
        Self {
            rng: Mutex::new(XorShift64::new(seed)),
            wait_pct,
            verdicts,
        }
    }

    fn draw(&self) -> u64 {
        self.rng.lock().expect("seeded rng").below(100)
    }
}

impl DecisionProvider for SeededDecisionProvider {
    fn decide(&self, request: &DecisionRequest) -> Result<DecisionResult, ConversationError> {
        match request.kind {
            DecisionKind::InvocationGate => Ok(scripted_decision_result(
                if self.draw() < self.wait_pct {
                    Decision::Wait
                } else {
                    Decision::Speak
                },
                0.9,
            )),
            DecisionKind::ResponseGate => {
                let draw = self.draw();
                let decision = if draw < self.verdicts.retry_pct {
                    Decision::Retry
                } else if draw < self.verdicts.retry_pct + self.verdicts.reject_pct {
                    Decision::Reject
                } else {
                    Decision::Accept
                };
                Ok(scripted_decision_result(decision, 0.9))
            }
        }
    }

    fn prepare_turn(
        &self,
        request: &TurnPreparationRequest,
    ) -> Result<TurnPreparation, ConversationError> {
        Ok(TurnPreparation {
            invocation: scripted_decision_result(
                if self.draw() < self.wait_pct {
                    Decision::Wait
                } else {
                    Decision::Speak
                },
                0.9,
            ),
            recall: request
                .recall_candidates
                .iter()
                .map(|candidate| (candidate.id.clone(), RecallRelevance::Uncertain))
                .collect(),
            observation: ObservationNeed::None,
            evidence_digest: request.evidence.snapshot_digest.clone(),
            latency_ms: 0,
        })
    }

    fn assess_responses(
        &self,
        request: &ResponseAssessmentRequest,
    ) -> Result<ResponseAssessment, ConversationError> {
        let draw = self.draw();
        if draw < self.verdicts.outage_pct {
            return Err(ConversationError::Transport);
        }
        let selected = request.selection.candidates.first().ok_or_else(|| {
            ConversationError::InvalidDecision("no generated candidates".to_owned())
        })?;
        let decision = if draw < self.verdicts.outage_pct + self.verdicts.retry_pct {
            Decision::Retry
        } else if draw
            < self.verdicts.outage_pct + self.verdicts.retry_pct + self.verdicts.reject_pct
        {
            Decision::Reject
        } else {
            Decision::Accept
        };
        scripted_assessment(request, &selected.id.clone(), decision)
    }
}
