//! C0 — the derived operative lane of a conversational organism.
//!
//! Two lanes carry durable meaning:
//!
//! * the **canonical lane** — immutable evidence and kernel-activated state,
//!   strictly forward-only;
//! * the **derived lane** defined here — the operative view a conversation
//!   runs with: conversation policy, retrieval parameters, and the
//!   individual's structured self model. It exists because improvement needs
//!   semantics canonical state deliberately lacks: a staged proposal
//!   lifecycle, evaluation before activation, and a movable head for
//!   rollback. Nothing here rewrites history; a rollback moves the head and
//!   the log of activations remains.
//!
//! The lane's own guardrails mirror the canonical ones rather than replacing
//! them. Model output may *propose*; only intake validation, the gate and an
//! activation may *apply*. Evidence cited by a self claim must come from the
//! individual's own observed record — the same contamination boundary the
//! canonical mutation policy enforces for the self domain.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceKind;
use crate::ids::{C0ActivationId, C0ProposalId, EvidenceId, IndividualId};
use crate::time::UtcTimestamp;

/// The deterministic evaluator implementation. A change here is a format
/// change: reports record it so results stay comparable honestly.
pub const EVALUATOR_KIND: &str = "deterministic-v0";

// ---------------------------------------------------------------------------
// Improvement proposals
// ---------------------------------------------------------------------------

/// Which derived-lane surface a proposal changes. Memory and relationship
/// improvements are *canonical* mutations and take the mutation-proposal path
/// instead; this enum only covers what the derived lane owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementKind {
    /// A conversation-policy parameter (e.g. target response length).
    PolicyUpdate,
    /// A self-model field (capabilities, limitations, ...).
    SelfUpdate,
    /// A retrieval parameter (window sizes, top-k, thresholds).
    RetrievalUpdate,
}

impl ImprovementKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyUpdate => "policy_update",
            Self::SelfUpdate => "self_update",
            Self::RetrievalUpdate => "retrieval_update",
        }
    }
}

impl fmt::Display for ImprovementKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ImprovementKind {
    type Err = UnknownImprovementKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "policy_update" => Self::PolicyUpdate,
            "self_update" => Self::SelfUpdate,
            "retrieval_update" => Self::RetrievalUpdate,
            other => return Err(UnknownImprovementKind(other.to_owned())),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown improvement kind {0:?}")]
pub struct UnknownImprovementKind(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
    Quarantined,
    Superseded,
}

impl ProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Quarantined => "quarantined",
            Self::Superseded => "superseded",
        }
    }
}

impl fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProposalStatus {
    type Err = UnknownProposalStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pending" => Self::Pending,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "quarantined" => Self::Quarantined,
            "superseded" => Self::Superseded,
            other => return Err(UnknownProposalStatus(other.to_owned())),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown proposal status {0:?}")]
pub struct UnknownProposalStatus(pub String);

// ---------------------------------------------------------------------------
// Self model
// ---------------------------------------------------------------------------

/// Fixed vocabulary of self-model fields. The self model is structured
/// state, not prose: every carried claim lands under one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfField {
    Identity,
    Temperament,
    Values,
    Capabilities,
    Limitations,
    ActiveGoals,
    OpenQuestions,
    Relationships,
    BodyResources,
}

impl SelfField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Temperament => "temperament",
            Self::Values => "values",
            Self::Capabilities => "capabilities",
            Self::Limitations => "limitations",
            Self::ActiveGoals => "active_goals",
            Self::OpenQuestions => "open_questions",
            Self::Relationships => "relationships",
            Self::BodyResources => "body_resources",
        }
    }
}

impl fmt::Display for SelfField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SelfField {
    type Err = UnknownSelfField;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "identity" => Self::Identity,
            "temperament" => Self::Temperament,
            "values" => Self::Values,
            "capabilities" => Self::Capabilities,
            "limitations" => Self::Limitations,
            "active_goals" => Self::ActiveGoals,
            "open_questions" => Self::OpenQuestions,
            "relationships" => Self::Relationships,
            "body_resources" => Self::BodyResources,
            other => return Err(UnknownSelfField(other.to_owned())),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown self-model field {0:?}")]
pub struct UnknownSelfField(pub String);

/// One claim inside a self-model field. Provenance travels with the claim:
/// which proposal introduced it and which canonical evidence supports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfEntry {
    /// Optional key distinguishing entries in a list-like field
    /// (`capabilities.translation`, ...). `None` = the field's single value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceId>,
    /// The accepted proposal that wrote this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<C0ProposalId>,
}

/// The self model as stored inside an activation snapshot:
/// field -> entries. Ordered maps keep serialization deterministic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfSnapshot {
    #[serde(default)]
    pub fields: BTreeMap<String, Vec<SelfEntry>>,
}

impl SelfSnapshot {
    /// Apply a self update: replace the entry at (field, key) — or the
    /// field's single entry when key is None — preserving every other entry.
    pub fn apply(&mut self, field: SelfField, entry: SelfEntry) {
        let entries = self.fields.entry(field.as_str().to_owned()).or_default();
        match &entry.key {
            Some(key) => {
                if let Some(existing) = entries.iter_mut().find(|e| e.key.as_deref() == Some(key)) {
                    *existing = entry;
                } else {
                    entries.push(entry);
                }
            }
            None => {
                entries.retain(|e| e.key.is_some());
                entries.insert(0, entry);
            }
        }
    }

    pub fn field(&self, field: SelfField) -> &[SelfEntry] {
        self.fields
            .get(field.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn entry_count(&self) -> usize {
        self.fields.values().map(Vec::len).sum()
    }
}

// ---------------------------------------------------------------------------
// Operative parameters
// ---------------------------------------------------------------------------

/// The tunable surface a conversation runs with. Every key a proposal may
/// target is a named field here — the vocabulary is closed, so a proposal
/// cannot invent a knob.
///
/// `serde(default)` everywhere: snapshots written by older builds still
/// deserialize, taking documented defaults for what they did not know.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OperativeParams {
    pub retrieval: RetrievalParams,
    pub conversation: ConversationPolicy,
    pub reflection: ReflectionParams,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalParams {
    /// Conversation messages replayed to the provider each turn.
    pub history_messages: usize,
    /// Relationship records retrieved about the current subject.
    pub relationship_top_k: usize,
    /// Activated episodic records surfaced by lexical retrieval.
    pub episodic_top_k: usize,
    /// Minimum bigram-overlap score for an episodic/recalled-evidence hit.
    pub min_score: f64,
    /// Candidate pool size for raw-evidence recall (utterances older than the
    /// visible history window, matched lexically).
    pub evidence_pool: usize,
    /// Recalled raw-evidence hits surfaced into the workspace.
    pub evidence_top_k: usize,
}

impl Default for RetrievalParams {
    fn default() -> Self {
        Self {
            history_messages: 12,
            relationship_top_k: 8,
            episodic_top_k: 2,
            min_score: 0.05,
            evidence_pool: 96,
            evidence_top_k: 2,
        }
    }
}

/// Conversation policy the persona is told to hold. These are advisory
/// directives rendered into the workspace; the persona honours or ignores
/// them, which is exactly why evaluation exists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationPolicy {
    /// Desired upper bound on response length in characters. 0 = unbounded.
    pub max_response_chars: usize,
    /// Directives injected into the workspace as the active policy overlay.
    pub directives: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReflectionParams {
    /// Run a reflection cycle every N turns. 0 = only on explicit request.
    pub interval_turns: u32,
}

/// A parameter target: fixed vocabulary key -> numeric bounds. Returns `None`
/// for unknown keys — an unknown target is a rejected proposal, never a
/// silently-dropped or clamped one.
fn param_bounds(key: &str) -> Option<(f64, f64)> {
    Some(match key {
        "retrieval.history_messages" => (1.0, 64.0),
        "retrieval.relationship_top_k" => (0.0, 32.0),
        "retrieval.episodic_top_k" => (0.0, 32.0),
        "retrieval.min_score" => (0.0, 1.0),
        "retrieval.evidence_pool" => (8.0, 512.0),
        "retrieval.evidence_top_k" => (0.0, 16.0),
        "conversation.max_response_chars" => (0.0, 32_000.0),
        "reflection.interval_turns" => (0.0, 10_000.0),
        _ => return None,
    })
}

/// Integer-valued parameters: fractional proposals are malformed, not
/// truncated.
fn param_is_integer(key: &str) -> bool {
    !matches!(key, "retrieval.min_score")
}

impl OperativeParams {
    /// Read a parameter by its fixed-vocabulary key.
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        Some(match key {
            "retrieval.history_messages" => self.retrieval.history_messages.into(),
            "retrieval.relationship_top_k" => self.retrieval.relationship_top_k.into(),
            "retrieval.episodic_top_k" => self.retrieval.episodic_top_k.into(),
            "retrieval.min_score" => self.retrieval.min_score.into(),
            "retrieval.evidence_pool" => self.retrieval.evidence_pool.into(),
            "retrieval.evidence_top_k" => self.retrieval.evidence_top_k.into(),
            "conversation.max_response_chars" => self.conversation.max_response_chars.into(),
            "reflection.interval_turns" => self.reflection.interval_turns.into(),
            _ => return None,
        })
    }

    /// Set a parameter by key. Caller has already bounds-checked the value;
    /// `set` re-checks vocabulary and fails on unknown keys rather than
    /// silently ignoring a write.
    pub fn set(&mut self, key: &str, value: serde_json::Value) -> Result<(), C0Error> {
        let number = value.as_f64().ok_or_else(|| C0Error::MalformedProposal {
            reason: format!("param {key} requires a numeric value"),
        })?;
        if param_is_integer(key) && number.fract() != 0.0 {
            return Err(C0Error::MalformedProposal {
                reason: format!("param {key} requires an integer, got {number}"),
            });
        }
        match key {
            "retrieval.history_messages" => self.retrieval.history_messages = number as usize,
            "retrieval.relationship_top_k" => self.retrieval.relationship_top_k = number as usize,
            "retrieval.episodic_top_k" => self.retrieval.episodic_top_k = number as usize,
            "retrieval.min_score" => self.retrieval.min_score = number,
            "retrieval.evidence_pool" => self.retrieval.evidence_pool = number as usize,
            "retrieval.evidence_top_k" => self.retrieval.evidence_top_k = number as usize,
            "conversation.max_response_chars" => {
                self.conversation.max_response_chars = number as usize;
            }
            "reflection.interval_turns" => self.reflection.interval_turns = number as u32,
            _ => {
                return Err(C0Error::MalformedProposal {
                    reason: format!("unknown param target {key:?}"),
                });
            }
        }
        Ok(())
    }
}

/// The full operative view at an activation — what the runtime reads through
/// the movable head. One snapshot covers both surfaces a proposal may change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OperativeView {
    #[serde(default)]
    pub params: OperativeParams,
    #[serde(default)]
    pub self_model: SelfSnapshot,
}

// ---------------------------------------------------------------------------
// Proposal records
// ---------------------------------------------------------------------------

/// A persisted improvement proposal — the lifecycle record the store keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub proposal_id: C0ProposalId,
    pub individual_id: IndividualId,
    pub kind: ImprovementKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_value: Option<serde_json::Value>,
    pub proposed_value: serde_json::Value,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_effect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub status: ProposalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_id: Option<EvidenceId>,
    pub created_at: UtcTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<UtcTimestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_seq: Option<u64>,
}

/// What a reflector emits — not yet a proposal, only a candidate for one.
/// Intake validation turns this into an [`ImprovementProposal`] or drops it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImprovementDraft {
    pub kind: ImprovementKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    pub proposed_value: serde_json::Value,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_effect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// A persisted activation — one applied proposal plus the operative view it
/// left behind. Immutable once written; `rolled_back_at` is the only field
/// that changes, recording that a rollback crossed it (history is kept, not
/// erased).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activation {
    pub activation_seq: u64,
    pub activation_id: C0ActivationId,
    pub individual_id: IndividualId,
    pub proposal_id: C0ProposalId,
    pub predecessor_seq: u64,
    pub view: OperativeView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back_at: Option<UtcTimestamp>,
    pub created_at: UtcTimestamp,
}

/// What applying a validated proposal produces: the view that activation
/// will store. Pure function — the store persists what this computes.
pub fn apply_proposal(view: &OperativeView, proposal: &ImprovementProposal) -> OperativeView {
    let mut next = view.clone();
    match proposal.kind {
        ImprovementKind::PolicyUpdate | ImprovementKind::RetrievalUpdate => {
            // Intake validation guarantees the target is a known, in-bounds
            // numeric param; an unknown key would have been rejected before a
            // proposal row existed.
            let _ = next
                .params
                .set(&proposal.target, proposal.proposed_value.clone());
        }
        ImprovementKind::SelfUpdate => {
            let field = SelfField::from_str(&proposal.target).unwrap_or(SelfField::OpenQuestions);
            next.self_model.apply(
                field,
                SelfEntry {
                    key: proposal.target_key.clone(),
                    value: proposal.proposed_value.clone(),
                    confidence: proposal.confidence,
                    evidence_refs: proposal.evidence_refs.clone(),
                    proposal_id: Some(proposal.proposal_id),
                },
            );
        }
    }
    next
}

// ---------------------------------------------------------------------------
// Intake validation — the boundary between model output and durable rows
// ---------------------------------------------------------------------------

/// Evidence kinds a self claim may cite. The same boundary the canonical
/// mutation policy enforces for the self domain: what the individual said
/// about itself, its own reflections, and runtime-observed events (a
/// provider timeout is legitimate evidence of a limitation). User claims,
/// library material and resource output are *not* self evidence.
fn self_evidence_kind_allowed(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::AgentUtterance | EvidenceKind::Reflection | EvidenceKind::SystemEvent
    )
}

/// Reason an improvement draft was dropped at intake. Reported to the
/// caller (and the canonical evidence event) as structured detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftRejection {
    pub target: String,
    pub reason: String,
}

/// Validate one improvement draft against what the reflection was shown.
///
/// Mirrors the canonical intake rules at this lane's boundary:
/// * every cited evidence id must have been among the presented evidence;
/// * every cited evidence record must belong to a kind the claim's lane may
///   draw on — a self claim citing user speech or library text is dropped,
///   not repaired;
/// * confidence must be inside [0,1] when present — never clamped;
/// * targets must be in the fixed vocabulary, and numeric params in bounds.
///
/// Anything failing is dropped wholesale; nothing is partially repaired.
pub fn validate_improvement_draft(
    draft: &ImprovementDraft,
    allowed_evidence: &BTreeMap<EvidenceId, EvidenceKind>,
    current: &OperativeView,
) -> Result<(ImprovementKind, String, Option<String>, serde_json::Value), DraftRejection> {
    let reject = |reason: &str| DraftRejection {
        target: draft.target.clone(),
        reason: reason.to_owned(),
    };

    for id in &draft.evidence_refs {
        let Some(kind) = allowed_evidence.get(id) else {
            return Err(reject(&format!("cited evidence {id} was not presented")));
        };
        if draft.kind == ImprovementKind::SelfUpdate && !self_evidence_kind_allowed(*kind) {
            return Err(reject(&format!(
                "self claim cites {kind:?} evidence, which is not first-party self evidence"
            )));
        }
    }
    if let Some(confidence) = draft.confidence
        && !(0.0..=1.0).contains(&confidence)
    {
        return Err(reject(&format!("confidence {confidence} outside [0,1]")));
    }

    match draft.kind {
        ImprovementKind::PolicyUpdate | ImprovementKind::RetrievalUpdate => {
            let Some((min, max)) = param_bounds(&draft.target) else {
                return Err(reject(&format!("unknown param target {:?}", draft.target)));
            };
            let Some(value) = draft.proposed_value.as_f64() else {
                return Err(reject("param proposals require a numeric value"));
            };
            if !(min..=max).contains(&value) {
                return Err(reject(&format!(
                    "value {value} outside [{min},{max}] for {}",
                    draft.target
                )));
            }
            if param_is_integer(&draft.target) && value.fract() != 0.0 {
                return Err(reject(&format!("{} requires an integer", draft.target)));
            }
            let old_value = current.params.get(&draft.target);
            Ok((
                draft.kind,
                draft.target.clone(),
                None,
                old_value.unwrap_or(serde_json::Value::Null),
            ))
        }
        ImprovementKind::SelfUpdate => {
            let Ok(field) = SelfField::from_str(&draft.target) else {
                return Err(reject(&format!("unknown self field {:?}", draft.target)));
            };
            if draft.proposed_value.is_null() {
                return Err(reject("self update requires a non-null value"));
            }
            // Self claims must cite evidence: a claim about the individual
            // with nothing behind it is exactly the failure mode the lane
            // exists to prevent.
            if draft.evidence_refs.is_empty() {
                return Err(reject("self update cites no evidence"));
            }
            let old_value = current
                .self_model
                .fields
                .get(field.as_str())
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|e| e.key == draft.target_key)
                        .map(|e| e.value.clone())
                });
            Ok((
                draft.kind,
                field.as_str().to_owned(),
                draft.target_key.clone(),
                old_value.unwrap_or(serde_json::Value::Null),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Reflection contract
// ---------------------------------------------------------------------------

/// One episode record presented to the reflector — a compact view of a
/// canonical evidence record, with the ID the reflector must cite back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectionEpisode {
    pub evidence_id: EvidenceId,
    pub kind: EvidenceKind,
    /// Role-ish label for rendering ("user", "assistant", "system", ...).
    pub speaker: String,
    pub text: String,
    /// Corrected records are marked so a reflector can see the correction
    /// already exists rather than proposing it again.
    pub is_corrected: bool,
    pub created_at: UtcTimestamp,
}

/// A durable record (relationship or episodic) presented to the reflector
/// with the IDs a correction/capture proposal needs to cite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectionMemoryRecord {
    pub state_record_id: crate::ids::MemoryId,
    pub domain: crate::mutation::MutationDomain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_key: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub evidence_refs: Vec<EvidenceId>,
}

/// Everything the reflector may cite. A proposal naming an ID outside what
/// was presented here is invalid at intake — this list is the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectionInput {
    pub individual_id: IndividualId,
    /// The subject of the current conversation, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_key: Option<String>,
    /// Recent canonical episodes (utterances, corrections, system events).
    pub episodes: Vec<ReflectionEpisode>,
    /// Active durable records relevant to this reflection.
    pub memory_records: Vec<ReflectionMemoryRecord>,
    /// The operative view currently in force.
    pub operative: OperativeView,
    /// Recent deterministic turn metrics — what the evaluator measured, not
    /// what a model felt.
    pub metrics: Vec<TurnMetrics>,
    /// Pending proposals still awaiting a decision, so a reflector does not
    /// re-propose what is already queued.
    pub pending_proposals: Vec<ImprovementProposal>,
    /// The current derived-lane head position.
    pub activation_seq: u64,
}

/// A hint that one canonical record corrects another. The runtime decides
/// whether to append the `corrects` link; the reflector only observes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionHint {
    /// The newer record that carries the correction.
    pub correction: EvidenceId,
    /// The older record it corrects.
    pub corrects: EvidenceId,
}

/// Validated reflection output. Structured, cite-checked — the same
/// discipline as persona drafts, at a different boundary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReflectionOutput {
    /// Free-form notes recorded inside the reflection evidence.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Canonical-record correction links the reflector observed.
    #[serde(default)]
    pub evidence_corrections: Vec<CorrectionHint>,
    /// Drafts destined for the canonical mutation lane (relationship facts,
    /// corrections, episodic captures). Judged by the same policy as persona
    /// drafts.
    #[serde(default)]
    pub canonical_drafts: Vec<crate::persona::ProposalDraft>,
    /// Drafts destined for the derived lane (policy, self, retrieval).
    #[serde(default)]
    pub improvement_drafts: Vec<ImprovementDraft>,
}

/// The component that reflects on recent experience. Provider-neutral: a
/// local fixture and an HTTP model implement the same contract and produce
/// the same output shape.
///
/// A reflector has no more authority than a persona core: it returns drafts
/// and hints, and everything it returns passes through intake validation
/// before anything is persisted.
pub trait Reflector: Send + Sync {
    fn descriptor(&self) -> crate::persona::PersonaBackendDescriptor;
    fn reflect(
        &self,
        input: &ReflectionInput,
    ) -> Result<ReflectionOutput, crate::persona::PersonaError>;
}

// ---------------------------------------------------------------------------
// Evaluation + gate
// ---------------------------------------------------------------------------

/// Deterministic per-turn metrics. Every axis is measured or explicitly
/// null — an axis that cannot be measured is recorded as null, never
/// silently zeroed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnMetrics {
    pub response_chars: usize,
    /// 0..1; share of assistant sentences already said verbatim this session.
    pub repetition_ratio: f64,
    /// Memory items surfaced into this turn's workspace (all recall domains).
    pub memories_surfaced: usize,
    /// The turn asked something memory could answer but none was surfaced.
    pub retrieval_missed: bool,
    /// Heuristic count of user-directed assertions not grounded in presented
    /// memory — an upper-bound proxy for unsupported-claim rate, not a proof.
    pub unsupported_marker_count: usize,
    /// Response length vs the policy bound; null when no bound is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_deviation: Option<i64>,
}

/// Per-axis comparison across a baseline/candidate replay pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub axis: String,
    pub baseline: f64,
    pub candidate: f64,
    pub delta: f64,
}

/// The replay report a gate decision cites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub turns: usize,
    /// Candidate produced a response for every replayed input.
    pub all_completed: bool,
    pub deltas: Vec<MetricDelta>,
    /// Guard axes that regressed (delta in the wrong direction).
    pub regressions: Vec<String>,
}

impl ReplayReport {
    pub fn improved(&self, axis: &str) -> bool {
        self.deltas.iter().any(|d| d.axis == axis && d.delta > 0.0)
    }
}

/// Guard axes: a candidate must not regress on any of them. Each entry is
/// (axis name, direction) — `higher_is_better` says which delta sign is a
/// regression.
const GUARD_AXES: &[(&str, bool)] = &[
    ("repetition_ratio", false), // lower is better
    ("retrieval_missed", false), // lower is better
    ("unsupported_marker_count", false),
    ("memories_surfaced", true), // higher is better
];

/// Compare aggregated baseline vs candidate metric vectors into a report.
/// `baseline`/`candidate` hold one [`TurnMetrics`] per replayed input.
pub fn compare_replay(baseline: &[TurnMetrics], candidate: &[TurnMetrics]) -> ReplayReport {
    let mean = |metrics: &[TurnMetrics], f: fn(&TurnMetrics) -> f64| -> f64 {
        if metrics.is_empty() {
            0.0
        } else {
            metrics.iter().map(f).sum::<f64>() / metrics.len() as f64
        }
    };
    type MetricAxis = (&'static str, fn(&TurnMetrics) -> f64);
    let axes: Vec<MetricAxis> = vec![
        ("repetition_ratio", |m| m.repetition_ratio),
        ("retrieval_missed", |m| m.retrieval_missed as u8 as f64),
        ("unsupported_marker_count", |m| {
            m.unsupported_marker_count as f64
        }),
        ("memories_surfaced", |m| m.memories_surfaced as f64),
        ("response_chars", |m| m.response_chars as f64),
    ];
    let deltas: Vec<MetricDelta> = axes
        .into_iter()
        .map(|(axis, f)| {
            let b = mean(baseline, f);
            let c = mean(candidate, f);
            MetricDelta {
                axis: axis.to_owned(),
                baseline: b,
                candidate: c,
                delta: c - b,
            }
        })
        .collect();
    let regressions = GUARD_AXES
        .iter()
        .filter(|(axis, higher_is_better)| {
            deltas.iter().find(|d| &d.axis == axis).is_some_and(|d| {
                if *higher_is_better {
                    d.delta < 0.0
                } else {
                    d.delta > 0.0
                }
            })
        })
        .map(|(axis, _)| axis.to_string())
        .collect();
    ReplayReport {
        turns: candidate.len(),
        all_completed: candidate.len() == baseline.len() && !candidate.is_empty(),
        deltas,
        regressions,
    }
}

/// The gate's verdict on a pending proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateDecision {
    Accept,
    Reject { reason: String },
}

/// Decide whether a pending proposal may activate. Deterministic given the
/// same proposal, view and replay report — the gate is code, not a model.
///
/// Rules:
/// * replay must have completed every input under the candidate view;
/// * no guard axis may regress;
/// * for parameter updates, at least one measured axis must improve — a
///   change that changes nothing observable is quarantined, not activated;
/// * self updates are exempt from the "must improve a metric" rule: their
///   effect is the self model itself, observable in the next workspace's
///   durable-self section. Their intake guard (first-party evidence) already
///   ran; the gate still requires replay guards clean.
pub fn gate_decision(proposal: &ImprovementProposal, report: &ReplayReport) -> GateDecision {
    if !report.all_completed {
        return GateDecision::Reject {
            reason: "candidate replay did not complete every input".to_owned(),
        };
    }
    if !report.regressions.is_empty() {
        return GateDecision::Reject {
            reason: format!("guard regression on {}", report.regressions.join(", ")),
        };
    }
    match proposal.kind {
        ImprovementKind::SelfUpdate => GateDecision::Accept,
        ImprovementKind::PolicyUpdate | ImprovementKind::RetrievalUpdate => {
            let improved = report.deltas.iter().any(|d| d.delta != 0.0);
            if improved {
                GateDecision::Accept
            } else {
                GateDecision::Reject {
                    reason: "candidate produced no measurable change; nothing to activate"
                        .to_owned(),
                }
            }
        }
    }
}

/// The evidence-id set a reflection may cite, keyed by kind so intake can
/// enforce per-lane evidence rules.
pub type AllowedEvidence = BTreeMap<EvidenceId, EvidenceKind>;

/// Collect the ids a reflection input presented — the only ids its output
/// may cite.
pub fn allowed_evidence<'a>(
    episodes: impl Iterator<Item = (&'a EvidenceId, EvidenceKind)>,
    extra: impl Iterator<Item = (&'a EvidenceId, EvidenceKind)>,
) -> AllowedEvidence {
    episodes
        .chain(extra)
        .map(|(id, kind)| (*id, kind))
        .collect::<BTreeMap<_, _>>()
}

/// Dedup helper for reports: sorted, deduplicated id list.
pub fn sorted_ids(ids: &[EvidenceId]) -> Vec<EvidenceId> {
    ids.iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum C0Error {
    #[error("improvement proposal malformed: {reason}")]
    MalformedProposal { reason: String },
    #[error("improvement proposal {0} is not pending")]
    ProposalNotPending(C0ProposalId),
    #[error("improvement proposal {0} not found")]
    ProposalNotFound(C0ProposalId),
    #[error("no activation to roll back from (head is at defaults)")]
    NothingToRollBack,
    #[error("activation sequence conflict: next seq would be {next} but the head is {head}")]
    ActivationConflict { next: u64, head: u64 },
    #[error("stored C0 row is malformed: {0}")]
    CorruptRow(String),
    #[error("reflector output failed validation: {0}")]
    ReflectorOutput(String),
    /// A storage backend failure surfaced through the lane. Classified, not
    /// transcribed: the message carries what failed, not row contents.
    #[error("C0 store backend failure: {message}")]
    Backend { message: String },
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::EvidenceId;

    fn evid(n: u128) -> EvidenceId {
        EvidenceId::from_u128(n)
    }

    fn allowed(kinds: &[(u128, EvidenceKind)]) -> AllowedEvidence {
        kinds.iter().map(|(n, k)| (evid(*n), *k)).collect()
    }

    fn draft(kind: ImprovementKind, target: &str, value: serde_json::Value) -> ImprovementDraft {
        ImprovementDraft {
            kind,
            target: target.to_owned(),
            target_key: None,
            proposed_value: value,
            evidence_refs: vec![evid(1)],
            expected_effect: None,
            risk: None,
            confidence: Some(0.7),
        }
    }

    #[test]
    fn param_draft_rejects_unknown_target_and_out_of_range_value() {
        let view = OperativeView::default();
        let ev = allowed(&[(1, EvidenceKind::AgentUtterance)]);
        let unknown = draft(
            ImprovementKind::RetrievalUpdate,
            "retrieval.made_up_knob",
            serde_json::json!(4),
        );
        assert!(
            validate_improvement_draft(&unknown, &ev, &view)
                .unwrap_err()
                .reason
                .contains("unknown param target")
        );
        let out_of_range = draft(
            ImprovementKind::RetrievalUpdate,
            "retrieval.episodic_top_k",
            serde_json::json!(10_000),
        );
        assert!(
            validate_improvement_draft(&out_of_range, &ev, &view)
                .unwrap_err()
                .reason
                .contains("outside")
        );
    }

    #[test]
    fn draft_rejects_confidence_outside_range_instead_of_clamping() {
        let view = OperativeView::default();
        let ev = allowed(&[(1, EvidenceKind::AgentUtterance)]);
        let mut d = draft(
            ImprovementKind::RetrievalUpdate,
            "retrieval.episodic_top_k",
            serde_json::json!(4),
        );
        d.confidence = Some(1.5);
        let err = validate_improvement_draft(&d, &ev, &view).unwrap_err();
        assert!(err.reason.contains("confidence"));
    }

    #[test]
    fn draft_rejects_unpresented_evidence() {
        let view = OperativeView::default();
        let ev = allowed(&[(2, EvidenceKind::AgentUtterance)]);
        let d = draft(
            ImprovementKind::RetrievalUpdate,
            "retrieval.episodic_top_k",
            serde_json::json!(4),
        );
        let err = validate_improvement_draft(&d, &ev, &view).unwrap_err();
        assert!(err.reason.contains("was not presented"));
    }

    #[test]
    fn self_draft_rejects_user_utterance_and_uncited_claims() {
        let view = OperativeView::default();
        let ev = allowed(&[(1, EvidenceKind::UserUtterance)]);
        let d = draft(
            ImprovementKind::SelfUpdate,
            "capabilities",
            serde_json::json!("remembers preferences"),
        );
        let err = validate_improvement_draft(&d, &ev, &view).unwrap_err();
        assert!(err.reason.contains("first-party"));

        let ev = allowed(&[(1, EvidenceKind::AgentUtterance)]);
        let mut uncited = d.clone();
        uncited.evidence_refs = vec![];
        assert!(
            validate_improvement_draft(&uncited, &ev, &view)
                .unwrap_err()
                .reason
                .contains("no evidence")
        );
        let ok = validate_improvement_draft(&d, &ev, &view);
        assert!(ok.is_ok());
    }

    #[test]
    fn apply_proposal_updates_params_and_self() {
        let view = OperativeView::default();
        let proposal = ImprovementProposal {
            proposal_id: C0ProposalId::from_u128(9),
            individual_id: IndividualId::from_u128(1),
            kind: ImprovementKind::RetrievalUpdate,
            target: "retrieval.episodic_top_k".to_owned(),
            target_key: None,
            old_value: None,
            proposed_value: serde_json::json!(6),
            evidence_refs: vec![],
            expected_effect: None,
            risk: None,
            confidence: None,
            status: ProposalStatus::Pending,
            rejection_reason: None,
            reflection_id: None,
            created_at: UtcTimestamp::from_unix_millis(1),
            decided_at: None,
            activation_seq: None,
        };
        let next = apply_proposal(&view, &proposal);
        assert_eq!(next.params.retrieval.episodic_top_k, 6);
        assert_eq!(view.params.retrieval.episodic_top_k, 2);

        let self_p = ImprovementProposal {
            kind: ImprovementKind::SelfUpdate,
            target: "limitations".to_owned(),
            proposed_value: serde_json::json!("cannot hear audio"),
            ..proposal.clone()
        };
        let next = apply_proposal(&next, &self_p);
        assert_eq!(next.self_model.entry_count(), 1);
        assert_eq!(
            next.self_model.field(SelfField::Limitations)[0].value,
            serde_json::json!("cannot hear audio")
        );
    }

    #[test]
    fn gate_rejects_regressions_and_accepts_improvements() {
        let proposal = ImprovementProposal {
            proposal_id: C0ProposalId::from_u128(1),
            individual_id: IndividualId::from_u128(1),
            kind: ImprovementKind::RetrievalUpdate,
            target: "retrieval.episodic_top_k".to_owned(),
            target_key: None,
            old_value: None,
            proposed_value: serde_json::json!(4),
            evidence_refs: vec![],
            expected_effect: None,
            risk: None,
            confidence: None,
            status: ProposalStatus::Pending,
            rejection_reason: None,
            reflection_id: None,
            created_at: UtcTimestamp::from_unix_millis(1),
            decided_at: None,
            activation_seq: None,
        };
        let base = vec![TurnMetrics {
            response_chars: 10,
            repetition_ratio: 0.0,
            memories_surfaced: 1,
            retrieval_missed: true,
            unsupported_marker_count: 0,
            length_deviation: None,
        }];
        let better = vec![TurnMetrics {
            memories_surfaced: 3,
            retrieval_missed: false,
            ..base[0].clone()
        }];
        let report = compare_replay(&base, &better);
        assert_eq!(gate_decision(&proposal, &report), GateDecision::Accept);

        let worse = vec![TurnMetrics {
            repetition_ratio: 0.5,
            ..base[0].clone()
        }];
        let report = compare_replay(&base, &worse);
        assert!(matches!(
            gate_decision(&proposal, &report),
            GateDecision::Reject { .. }
        ));

        let same = compare_replay(&base, &base);
        assert!(matches!(
            gate_decision(&proposal, &same),
            GateDecision::Reject { .. }
        ));
    }
}
