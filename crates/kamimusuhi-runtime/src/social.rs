//! Two-phase social proposal persistence and verification for external
//! communication channels (e.g., Discord).
//!
//! A generated reply is stored as a pending proposal (`response_generated`)
//! until external delivery is verified (`verify_delivery`) or explicitly
//! aborted (`abort_proposal`). Only verified deliveries persist as
//! `EvidenceKind::AgentUtterance`, warm recalled memories, and submit C0 drafts.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use kamimusuhi_core::c0::{EVALUATOR_KIND, TurnMetrics};
use kamimusuhi_core::continuity::{ContinuityStore, WriterIdentity};
use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence, NewEvidenceLink,
    RetentionClass,
};
use kamimusuhi_core::ids::{C0EvaluationId, EvidenceId, IndividualId, MemoryId, SessionId, TurnId};
use kamimusuhi_core::memory::MemoryUsage;
use kamimusuhi_core::mutation::{MutationDomain, OriginClass};
use kamimusuhi_core::persona::{PersonaBackendDescriptor, ProposalDraft};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Runtime, RuntimeError};

const PROPOSALS_DIR: &str = "social-proposals";
const MAX_PROPOSAL_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialVisibility {
    #[default]
    Private,
    Public,
}

impl SocialVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

fn default_platform() -> String {
    "discord".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialContext {
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    #[serde(default)]
    pub message_id: Option<u64>,
    #[serde(default)]
    pub user_id: Option<u64>,
    #[serde(default)]
    pub reply_to_message_id: Option<u64>,
    #[serde(default)]
    pub visibility: SocialVisibility,
    #[serde(default)]
    pub is_operator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiativeInput {
    pub trigger_id: String,
    pub trigger_kind: String,
    pub reason: String,
    pub source_ref: String,
    pub evidence_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSocialOptions {
    #[serde(default)]
    pub defer_delivery: bool,
    #[serde(default)]
    pub social_context: Option<SocialContext>,
    #[serde(default)]
    pub initiative: Option<InitiativeInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryVerification {
    pub discord_message_id: u64,
    pub channel_id: u64,
    #[serde(default)]
    pub guild_id: Option<u64>,
    #[serde(default)]
    pub reply_to_message_id: Option<u64>,
    #[serde(default)]
    pub delivered_text_digest: Option<String>,
    #[serde(default)]
    pub visibility: Option<SocialVisibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSocialProposal {
    pub individual_id: IndividualId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub subject: String,
    pub source_id: String,
    pub is_initiative: bool,
    pub input_evidence_id: EvidenceId,
    pub generated_evidence_id: EvidenceId,
    pub response: String,
    pub response_digest: String,
    pub persona_backend: PersonaBackendDescriptor,
    pub roots: BTreeSet<EvidenceId>,
    pub surfaced_memory_ids: Vec<MemoryId>,
    pub used_memory_ids: Vec<MemoryId>,
    pub proposals: Vec<ProposalDraft>,
    pub metrics: TurnMetrics,
    pub social_context: Option<SocialContext>,
    pub initiative: Option<InitiativeInput>,
    pub created_at: UtcTimestamp,
}

fn directory(runtime: &Runtime) -> PathBuf {
    runtime.paths().dir.join(PROPOSALS_DIR)
}

fn io(e: std::io::Error) -> RuntimeError {
    RuntimeError::Usage(format!("SOCIAL_PROPOSAL_IO: {}", e.kind()))
}

fn invalid(code: &str) -> RuntimeError {
    RuntimeError::Usage(format!("SOCIAL_PROPOSAL_{code}"))
}

fn regular(path: &Path, is_dir: bool) -> Result<(), RuntimeError> {
    let meta = fs::symlink_metadata(path).map_err(io)?;
    if meta.is_symlink() || meta.is_dir() != is_dir || (!is_dir && !meta.is_file()) {
        return Err(invalid("INTEGRITY"));
    }
    Ok(())
}

pub fn save_pending_proposal(
    runtime: &Runtime,
    proposal: &PendingSocialProposal,
) -> Result<(), RuntimeError> {
    if proposal.individual_id != runtime.individual_id()
        || proposal.individual_id.is_nil()
        || proposal.session_id.is_nil()
        || proposal.turn_id.is_nil()
    {
        return Err(invalid("INVALID_IDS"));
    }
    let dir = directory(runtime);
    match fs::create_dir(&dir) {
        Ok(()) => File::open(&runtime.paths().dir)
            .and_then(|f| f.sync_all())
            .map_err(io)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io(e)),
    }
    regular(&dir, true)?;
    let bytes = serde_json::to_vec(proposal).map_err(|_| invalid("SERIALIZE"))?;
    let temp = dir.join(format!(".{}.tmp", proposal.turn_id));
    let _ = fs::remove_file(&temp);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(io)?;
    file.write_all(&bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    let dest = dir.join(format!("{}.json", proposal.turn_id));
    fs::hard_link(&temp, &dest).map_err(io)?;
    fs::remove_file(&temp).map_err(io)?;
    File::open(&dir).and_then(|f| f.sync_all()).map_err(io)?;
    Ok(())
}

fn load_proposal_from_path(
    runtime: &Runtime,
    path: &Path,
) -> Result<PendingSocialProposal, RuntimeError> {
    regular(path, false)?;
    if fs::metadata(path).map_err(io)?.len() > MAX_PROPOSAL_BYTES {
        return Err(invalid("TOO_LARGE"));
    }
    let bytes = fs::read(path).map_err(io)?;
    let proposal: PendingSocialProposal =
        serde_json::from_slice(&bytes).map_err(|_| invalid("CORRUPT"))?;
    if proposal.individual_id != runtime.individual_id()
        || path.file_name().and_then(|s| s.to_str()) != Some(&format!("{}.json", proposal.turn_id))
    {
        return Err(invalid("INTEGRITY"));
    }
    Ok(proposal)
}

pub fn load_pending_proposal(
    runtime: &Runtime,
    turn_id: TurnId,
) -> Result<PendingSocialProposal, RuntimeError> {
    let dir = directory(runtime);
    regular(&dir, true)?;
    let path = dir.join(format!("{turn_id}.json"));
    load_proposal_from_path(runtime, &path)
}

pub fn list_pending_proposals(
    runtime: &Runtime,
) -> Result<Vec<PendingSocialProposal>, RuntimeError> {
    let dir = directory(runtime);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(e)),
        Ok(_) => {}
    }
    regular(&dir, true)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        out.push(load_proposal_from_path(runtime, &path)?);
    }
    Ok(out)
}

fn contains_prohibited_relationship_analysis(value: &Value) -> bool {
    const FORBIDDEN_KEYS: &[&str] = &[
        "relationship_graph",
        "interpersonal_analysis",
        "third_party_profile",
    ];
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| {
            let lower = k.to_ascii_lowercase();
            FORBIDDEN_KEYS.contains(&lower.as_str()) || contains_prohibited_relationship_analysis(v)
        }),
        Value::Array(items) => items.iter().any(contains_prohibited_relationship_analysis),
        _ => false,
    }
}

/// Filter persona mutation drafts under social visibility and policy rules.
pub fn filter_social_drafts(
    drafts: Vec<ProposalDraft>,
    social_context: Option<&SocialContext>,
) -> Vec<ProposalDraft> {
    let disallow_relationship = social_context
        .is_some_and(|ctx| ctx.visibility == SocialVisibility::Public && !ctx.is_operator);
    drafts
        .into_iter()
        .filter(|draft| {
            if disallow_relationship && draft.domain == MutationDomain::Relationship {
                return false;
            }
            if contains_prohibited_relationship_analysis(&draft.candidate) {
                return false;
            }
            true
        })
        .collect()
}

/// Commit a previously proposed turn after external delivery succeeded and was verified.
pub fn verify_delivery(
    runtime: &mut Runtime,
    turn_id: TurnId,
    verification: &DeliveryVerification,
    cached_writer: &mut Option<WriterIdentity>,
) -> Result<Value, RuntimeError> {
    let _identity_lease = runtime.lock_identity()?;
    runtime.ensure_identity()?;
    if verification.discord_message_id == 0 {
        return Err(invalid("MESSAGE_ID_REQUIRED"));
    }

    let path = directory(runtime).join(format!("{turn_id}.json"));
    if !path.exists() {
        let existing = runtime
            .store()
            .evidence_for_turn(runtime.individual_id(), turn_id)?;
        if let Some(emitted) = existing.iter().find(|record| {
            record.kind == EvidenceKind::AgentUtterance
                && record.payload.get("delivery").and_then(Value::as_str)
                    == Some("discord_gateway_verified")
                && record
                    .payload
                    .pointer("/discord/message_id")
                    .and_then(Value::as_u64)
                    == Some(verification.discord_message_id)
        }) {
            return Ok(json!({
                "state": "persisted",
                "idempotent": true,
                "turn_id": turn_id,
                "emitted_evidence_id": emitted.evidence_id,
                "discord_message_id": verification.discord_message_id,
            }));
        }
        return Err(invalid("NOT_FOUND"));
    }

    let pending = load_proposal_from_path(runtime, &path)?;
    if let Some(digest) = &verification.delivered_text_digest
        && digest != &pending.response_digest
    {
        return Err(invalid("DIGEST_MISMATCH"));
    }
    let expected_input_kind = if pending.is_initiative {
        EvidenceKind::SystemEvent
    } else {
        EvidenceKind::UserUtterance
    };
    let input_record = runtime
        .store()
        .get(pending.input_evidence_id)?
        .ok_or_else(|| invalid("MISSING_INPUT_EVIDENCE"))?;
    if input_record.individual_id != pending.individual_id
        || input_record.kind != expected_input_kind
        || input_record.turn_id != Some(pending.turn_id)
    {
        return Err(invalid("INPUT_EVIDENCE_MISMATCH"));
    }
    let gen_record = runtime
        .store()
        .get(pending.generated_evidence_id)?
        .ok_or_else(|| invalid("MISSING_GENERATED_EVIDENCE"))?;
    if gen_record.individual_id != pending.individual_id
        || gen_record.kind != EvidenceKind::SystemEvent
        || gen_record.turn_id != Some(pending.turn_id)
    {
        return Err(invalid("GENERATED_EVIDENCE_MISMATCH"));
    }

    runtime
        .store()
        .mark_used(&pending.used_memory_ids, runtime.now());
    let filtered = filter_social_drafts(pending.proposals.clone(), pending.social_context.as_ref());
    let draft_outcomes = crate::c0::submit_drafts(
        runtime,
        Some(pending.session_id),
        Some(pending.turn_id),
        cached_writer,
        filtered,
    )?;
    let drafts_activated = draft_outcomes.iter().filter(|o| o.activated).count();

    let pending_wb = if pending.is_initiative {
        None
    } else {
        Some(crate::memory_writeback::begin(
            runtime,
            pending.session_id,
            pending.turn_id,
        )?)
    };

    let visibility = verification.visibility.unwrap_or_else(|| {
        pending
            .social_context
            .as_ref()
            .map_or(SocialVisibility::Private, |c| c.visibility)
    });
    let payload = json!({
        "text": pending.response,
        "generated_evidence_id": pending.generated_evidence_id,
        "backend": pending.persona_backend,
        "delivery": "discord_gateway_verified",
        "human_acknowledgement": false,
        "discord": {
            "guild_id": verification.guild_id.or_else(|| pending.social_context.as_ref().and_then(|c| c.guild_id)),
            "channel_id": verification.channel_id,
            "message_id": verification.discord_message_id,
            "reply_to_message_id": verification.reply_to_message_id.or_else(|| pending.social_context.as_ref().and_then(|c| c.reply_to_message_id)),
            "visibility": visibility,
        },
        "initiative": pending.initiative,
    });
    let emitted_evidence_id = EvidenceId::generate(runtime.ids().as_ref());
    let payload_digest = json_digest(&payload);
    let links = pending
        .roots
        .iter()
        .map(|root| NewEvidenceLink {
            from_evidence_id: emitted_evidence_id,
            to_evidence_id: *root,
            relation: EvidenceRelation::DerivedFrom,
        })
        .collect::<Vec<_>>();
    runtime.store().append_with_links(
        NewEvidence {
            evidence_id: emitted_evidence_id,
            individual_id: pending.individual_id,
            session_id: Some(pending.session_id),
            turn_id: Some(pending.turn_id),
            kind: EvidenceKind::AgentUtterance,
            origin_class: OriginClass::Observed,
            payload,
            source: EvidenceSource {
                source_id: Some(pending.source_id.clone()),
                source_sequence: None,
                content_digest: Some(payload_digest.clone()),
            },
            retention_class: RetentionClass::Standard,
        },
        &links,
    )?;

    runtime.trace().record_with(
        TraceEventKind::FinalExpression,
        TraceCorrelation {
            persona_backend_id: Some(pending.persona_backend.backend_id),
            ..TraceCorrelation::default()
        },
        json!({"expression_digest": content_digest(pending.response.as_bytes())}),
    );
    runtime
        .trace()
        .record(TraceEventKind::ResponseEmitted, TraceCorrelation::default());
    runtime
        .store()
        .c0_record_evaluation(&kamimusuhi_store_sqlite::c0::C0Evaluation {
            evaluation_id: C0EvaluationId::generate(runtime.ids().as_ref()),
            individual_id: pending.individual_id,
            scope: "turn".to_owned(),
            subject_key: Some(pending.subject.clone()),
            turn_id: Some(pending.turn_id),
            proposal_id: None,
            metrics: serde_json::to_value(&pending.metrics).unwrap_or(Value::Null),
            evaluator: EVALUATOR_KIND.to_owned(),
            created_at: runtime.now(),
        })?;

    let writeback = if let Some(pending_wb) = pending_wb {
        crate::memory_writeback::finish(
            runtime,
            pending_wb,
            pending.input_evidence_id,
            pending.generated_evidence_id,
            emitted_evidence_id,
            &draft_outcomes,
        )?
    } else {
        let mut receipts = Vec::new();
        for outcome in draft_outcomes.iter().filter(|o| o.activated) {
            let receipt = runtime
                .store()
                .find_receipt(outcome.proposal_id)?
                .ok_or_else(|| invalid("MISSING_RECEIPT"))?;
            receipts.push(receipt.receipt_id);
        }
        let submitted = draft_outcomes.len();
        let wb_payload = json!({
            "event": "memory_writeback_verified",
            "initiative": true,
            "input_evidence_id": pending.input_evidence_id,
            "generated_evidence_id": pending.generated_evidence_id,
            "emitted_evidence_id": emitted_evidence_id,
            "drafts_submitted": submitted,
            "drafts_admitted": drafts_activated,
            "verified_receipt_ids": receipts,
            "admission": if submitted == 0 {
                "no_drafts"
            } else if drafts_activated == submitted {
                "all_admitted"
            } else {
                "some_or_all_rejected"
            },
            "verification": "evidence_readback_not_semantic_memory_quality",
        });
        let wb_id = EvidenceId::generate(runtime.ids().as_ref());
        runtime.store().append(NewEvidence {
            evidence_id: wb_id,
            individual_id: pending.individual_id,
            session_id: Some(pending.session_id),
            turn_id: Some(pending.turn_id),
            kind: EvidenceKind::SystemEvent,
            origin_class: OriginClass::Observed,
            payload: wb_payload.clone(),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })?;
        wb_payload
    };

    fs::remove_file(&path).map_err(io)?;
    File::open(directory(runtime))
        .and_then(|f| f.sync_all())
        .map_err(io)?;

    Ok(json!({
        "state": "persisted",
        "turn_id": turn_id,
        "emitted_evidence_id": emitted_evidence_id,
        "discord_message_id": verification.discord_message_id,
        "drafts_submitted": draft_outcomes.len(),
        "drafts_activated": drafts_activated,
        "memory_writeback": writeback,
    }))
}

/// Discard an undelivered or policy-rejected social proposal without persisting
/// an `AgentUtterance`.
pub fn abort_proposal(
    runtime: &mut Runtime,
    turn_id: TurnId,
    reason: &str,
) -> Result<Value, RuntimeError> {
    let _identity_lease = runtime.lock_identity()?;
    runtime.ensure_identity()?;
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 512 {
        return Err(invalid("ABORT_REASON_REQUIRED"));
    }
    let path = directory(runtime).join(format!("{turn_id}.json"));
    if !path.exists() {
        let existing = runtime
            .store()
            .evidence_for_turn(runtime.individual_id(), turn_id)?;
        if let Some(aborted) = existing.iter().find(|record| {
            record.kind == EvidenceKind::SystemEvent
                && record.payload.get("event").and_then(Value::as_str)
                    == Some("social_proposal_aborted")
        }) {
            return Ok(json!({
                "state": "aborted",
                "idempotent": true,
                "turn_id": turn_id,
                "generated_evidence_id": aborted.payload.get("generated_evidence_id"),
                "reason": aborted.payload.get("reason"),
            }));
        }
        return Err(invalid("NOT_FOUND"));
    }
    let pending = load_proposal_from_path(runtime, &path)?;
    let abort_id = EvidenceId::generate(runtime.ids().as_ref());
    let abort_payload = json!({
        "event": "social_proposal_aborted",
        "turn_id": turn_id,
        "generated_evidence_id": pending.generated_evidence_id,
        "reason": reason,
    });
    runtime.store().append_with_links(
        NewEvidence {
            evidence_id: abort_id,
            individual_id: pending.individual_id,
            session_id: Some(pending.session_id),
            turn_id: Some(pending.turn_id),
            kind: EvidenceKind::SystemEvent,
            origin_class: OriginClass::Observed,
            payload: abort_payload,
            source: EvidenceSource {
                source_id: Some(pending.source_id.clone()),
                source_sequence: None,
                content_digest: None,
            },
            retention_class: RetentionClass::Standard,
        },
        &[NewEvidenceLink {
            from_evidence_id: abort_id,
            to_evidence_id: pending.generated_evidence_id,
            relation: EvidenceRelation::DerivedFrom,
        }],
    )?;
    fs::remove_file(&path).map_err(io)?;
    File::open(directory(runtime))
        .and_then(|f| f.sync_all())
        .map_err(io)?;
    Ok(json!({
        "state": "aborted",
        "turn_id": turn_id,
        "generated_evidence_id": pending.generated_evidence_id,
        "reason": reason,
    }))
}
