//! Per-turn crash markers plus immutable, read-back-verified completion events.
//! Response delivery, evidence persistence and memory admission are distinct.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRecord, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence,
    NewEvidenceLink, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::mutation::OriginClass;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Runtime, RuntimeError};

const DIRECTORY: &str = "memory-writeback";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingWriteback {
    individual_id: IndividualId,
    session_id: SessionId,
    turn_id: TurnId,
    completion_id: EvidenceId,
}

/// A terminal failure is distinct from successful evidence writeback. The
/// actor identifies the local operator command, not a separately authenticated
/// human identity. The OS controls access to this maintenance command.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortAudit {
    event: String,
    resolution: String,
    actor: String,
    reason: String,
    marker_sha256: String,
    evidence_snapshot_digest: String,
    evidence_ids: Vec<EvidenceId>,
}

struct AbortReview {
    pending: PendingWriteback,
    audit: AbortAudit,
    token: String,
    existing: Option<EvidenceRecord>,
}

fn refused(code: &str) -> RuntimeError {
    RuntimeError::Usage(format!("MEMORY_WRITEBACK_{code}"))
}

fn digest_json(value: &impl Serialize) -> Result<String, RuntimeError> {
    Ok(content_digest(
        &serde_json::to_vec(value).map_err(|_| invalid())?,
    ))
}

fn checked_reason(reason: &str) -> Result<&str, RuntimeError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 2048 {
        return Err(refused("REASON_REQUIRED_1_TO_2048_BYTES"));
    }
    Ok(reason)
}

fn parse_marker(
    runtime: &Runtime,
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<PendingWriteback, RuntimeError> {
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    let pending: PendingWriteback = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if pending.individual_id != runtime.individual_id()
        || pending.individual_id.is_nil()
        || pending.session_id.is_nil()
        || pending.turn_id.is_nil()
        || pending.completion_id.is_nil()
        || path.file_name().and_then(|s| s.to_str()) != Some(&format!("{}.json", pending.turn_id))
    {
        return Err(invalid());
    }
    Ok(pending)
}

fn terminal_binding(record: &EvidenceRecord, pending: &PendingWriteback) -> bool {
    record.evidence_id == pending.completion_id
        && record.individual_id == pending.individual_id
        && record.session_id == Some(pending.session_id)
        && record.turn_id == Some(pending.turn_id)
        && record.kind == EvidenceKind::SystemEvent
}

fn original_evidence(
    runtime: &Runtime,
    pending: &PendingWriteback,
) -> Result<Vec<EvidenceRecord>, RuntimeError> {
    let session = runtime
        .store()
        .session(pending.session_id)?
        .ok_or_else(invalid)?;
    if session.individual_id != pending.individual_id {
        return Err(invalid());
    }
    let records = runtime
        .store()
        .evidence_for_turn(pending.individual_id, pending.turn_id)?
        .into_iter()
        .filter(|record| record.evidence_id != pending.completion_id)
        .collect::<Vec<_>>();
    // Reserve one slot for the terminal audit row so the same bounded store
    // read remains valid after resolution as well as before it.
    if records.len() >= 4096 {
        return Err(refused("EVIDENCE_REVIEW_LIMIT"));
    }
    if records.iter().any(|record| {
        record.individual_id != pending.individual_id
            || record.session_id != Some(pending.session_id)
            || record.turn_id != Some(pending.turn_id)
    }) || !records
        .iter()
        .any(|record| record.kind == EvidenceKind::UserUtterance)
    {
        return Err(invalid());
    }
    // An emitted-but-unverified response requires a different recovery review;
    // this command only abandons an incomplete attempt without an emitted row.
    if records
        .iter()
        .any(|record| record.kind == EvidenceKind::AgentUtterance)
    {
        return Err(refused("EMITTED_TURN_REQUIRES_REVIEW"));
    }
    Ok(records)
}

fn expected_audit(
    records: &[EvidenceRecord],
    marker_sha256: String,
    reason: &str,
) -> Result<AbortAudit, RuntimeError> {
    Ok(AbortAudit {
        event: "memory_writeback_aborted".to_owned(),
        resolution: "operator_abandoned_incomplete_writeback".to_owned(),
        actor: "operator".to_owned(),
        reason: checked_reason(reason)?.to_owned(),
        marker_sha256,
        evidence_snapshot_digest: digest_json(&records)?,
        evidence_ids: records.iter().map(|record| record.evidence_id).collect(),
    })
}

fn validate_abort(
    runtime: &Runtime,
    pending: &PendingWriteback,
    marker_bytes: &[u8],
    terminal: &EvidenceRecord,
) -> Result<AbortAudit, RuntimeError> {
    if !terminal_binding(terminal, pending)
        || terminal.origin_class != OriginClass::Observed
        || terminal.retention_class != RetentionClass::Standard
        || terminal.source != EvidenceSource::default()
    {
        return Err(invalid());
    }
    let audit: AbortAudit =
        serde_json::from_value(terminal.payload.clone()).map_err(|_| invalid())?;
    let expected = expected_audit(
        &original_evidence(runtime, pending)?,
        content_digest(marker_bytes),
        &audit.reason,
    )?;
    if audit != expected {
        return Err(invalid());
    }
    let links = runtime.store().links_from(pending.completion_id)?;
    if links.len() != audit.evidence_ids.len()
        || audit.evidence_ids.iter().any(|id| {
            !links.iter().any(|link| {
                link.from_evidence_id == pending.completion_id
                    && link.to_evidence_id == *id
                    && link.relation == EvidenceRelation::DerivedFrom
            })
        })
    {
        return Err(invalid());
    }
    Ok(audit)
}

fn prepare_abort(
    runtime: &Runtime,
    turn_id: TurnId,
    reason: &str,
) -> Result<AbortReview, RuntimeError> {
    let reason = checked_reason(reason)?;
    let dir = directory(runtime);
    regular(&dir, true)?;
    let path = dir.join(format!("{turn_id}.json"));
    regular(&path, false)?;
    if fs::metadata(&path).map_err(io)?.len() > 4096 {
        return Err(invalid());
    }
    let bytes = fs::read(&path).map_err(io)?;
    let pending = parse_marker(runtime, &path, &bytes)?;
    let existing = runtime.store().get(pending.completion_id)?;
    if let Some(terminal) = &existing {
        if terminal_binding(terminal, &pending)
            && terminal.payload["event"] == "memory_writeback_verified"
        {
            return Err(refused("ALREADY_VERIFIED"));
        }
        let audit = validate_abort(runtime, &pending, &bytes, terminal)?;
        if audit.reason != reason {
            return Err(refused("ALREADY_ABORTED_WITH_DIFFERENT_REASON"));
        }
    }
    let audit = expected_audit(
        &original_evidence(runtime, &pending)?,
        content_digest(&bytes),
        reason,
    )?;
    let token = digest_json(&json!({
        "domain":"kamimusuhi.memory_writeback.abort.v1",
        "individual_id":pending.individual_id,
        "session_id":pending.session_id,
        "turn_id":pending.turn_id,
        "completion_id":pending.completion_id,
        "audit":audit,
    }))?;
    Ok(AbortReview {
        pending,
        audit,
        token,
        existing,
    })
}

fn review_report(review: &AbortReview, state: &str) -> Value {
    json!({"state":state,"review_token":review.token,
        "turn_id":review.pending.turn_id,"completion_id":review.pending.completion_id,
        "marker_sha256":review.audit.marker_sha256,"evidence_ids":review.audit.evidence_ids,
        "reason":review.audit.reason,"marker_preserved":true,
        "resolution":"operator_abandoned_incomplete_writeback"})
}

/// Read-only review under a nonblocking exclusive turn lease. The token binds
/// the exact marker, original evidence and reason, and is not authentication.
pub fn review_abort(
    runtime: &Runtime,
    turn_id: TurnId,
    reason: &str,
) -> Result<Value, RuntimeError> {
    let _lease = runtime.try_lock_identity_exclusive()?;
    runtime.ensure_identity()?;
    let review = prepare_abort(runtime, turn_id, reason)?;
    Ok(review_report(
        &review,
        if review.existing.is_some() {
            "aborted"
        } else {
            "abort_review"
        },
    ))
}

/// Resolve only the exact operator-reviewed failed attempt. This appends a
/// failure outcome at the marker's reserved terminal ID, with atomic lineage;
/// the original marker and evidence remain untouched. A repeated identical
/// call is a no-op, while a verified or changed outcome is never overwritten.
pub fn abort(
    runtime: &Runtime,
    turn_id: TurnId,
    reason: &str,
    review_token: &str,
) -> Result<Value, RuntimeError> {
    let _lease = runtime.try_lock_identity_exclusive()?;
    runtime.ensure_identity()?;
    let _writer = runtime.lock_writer()?;
    let review = prepare_abort(runtime, turn_id, reason)?;
    if review_token != review.token {
        return Err(refused("REVIEW_CHANGED"));
    }
    if review.existing.is_none() {
        let payload = serde_json::to_value(&review.audit).map_err(|_| invalid())?;
        let links = review
            .audit
            .evidence_ids
            .iter()
            .map(|id| NewEvidenceLink {
                from_evidence_id: review.pending.completion_id,
                to_evidence_id: *id,
                relation: EvidenceRelation::DerivedFrom,
            })
            .collect::<Vec<_>>();
        let stored = runtime.store().append_with_links(
            NewEvidence {
                evidence_id: review.pending.completion_id,
                individual_id: review.pending.individual_id,
                session_id: Some(review.pending.session_id),
                turn_id: Some(review.pending.turn_id),
                kind: EvidenceKind::SystemEvent,
                origin_class: OriginClass::Observed,
                payload,
                source: EvidenceSource::default(),
                retention_class: RetentionClass::Standard,
            },
            &links,
        )?;
        let landed = runtime
            .store()
            .get(review.pending.completion_id)?
            .ok_or_else(invalid)?;
        if landed != stored {
            return Err(invalid());
        }
        let marker = directory(runtime).join(format!("{turn_id}.json"));
        let bytes = fs::read(marker).map_err(io)?;
        validate_abort(runtime, &review.pending, &bytes, &landed)?;
    }
    Ok(review_report(&review, "aborted"))
}

fn invalid() -> RuntimeError {
    RuntimeError::Usage("MEMORY_WRITEBACK_INTEGRITY".to_owned())
}
fn io(e: std::io::Error) -> RuntimeError {
    RuntimeError::Usage(format!("MEMORY_WRITEBACK_IO: {}", e.kind()))
}
fn directory(runtime: &Runtime) -> PathBuf {
    runtime.paths().dir.join(DIRECTORY)
}
fn regular(path: &std::path::Path, is_dir: bool) -> Result<(), RuntimeError> {
    let meta = fs::symlink_metadata(path).map_err(io)?;
    if meta.is_symlink() || meta.is_dir() != is_dir || (!is_dir && !meta.is_file()) {
        return Err(invalid());
    }
    Ok(())
}

pub fn begin(
    runtime: &Runtime,
    session_id: SessionId,
    turn_id: TurnId,
) -> Result<PendingWriteback, RuntimeError> {
    let dir = directory(runtime);
    match fs::create_dir(&dir) {
        Ok(()) => File::open(&runtime.paths().dir)
            .and_then(|f| f.sync_all())
            .map_err(io)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io(e)),
    }
    regular(&dir, true)?;
    let pending = PendingWriteback {
        individual_id: runtime.individual_id(),
        session_id,
        turn_id,
        completion_id: EvidenceId::generate(runtime.ids().as_ref()),
    };
    let temp = dir.join(format!(".{turn_id}.tmp"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(io)?;
    file.write_all(&serde_json::to_vec(&pending).map_err(|_| invalid())?)
        .map_err(io)?;
    file.sync_all().map_err(io)?;
    // Publish a complete marker atomically without overwriting an existing ID.
    fs::hard_link(&temp, dir.join(format!("{turn_id}.json"))).map_err(io)?;
    fs::remove_file(&temp).map_err(io)?;
    File::open(&dir).and_then(|f| f.sync_all()).map_err(io)?;
    Ok(pending)
}

/// Read-only: a live concurrent turn is unresolved, not declared failed.
/// A completion committed before a crash is reconciled by its exact event ID.
pub fn status(runtime: &Runtime) -> Result<Value, RuntimeError> {
    status_excluding(runtime, None)
}

pub fn status_excluding(
    runtime: &Runtime,
    current_turn: Option<TurnId>,
) -> Result<Value, RuntimeError> {
    let dir = directory(runtime);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"state":"no_unresolved_turns","unresolved":0,
                "turn_ids":[],"aborted":0,"aborted_turn_ids":[]}));
        }
        Err(e) => return Err(io(e)),
        Ok(_) => {}
    }
    regular(&dir, true)?;
    let mut unresolved = Vec::new();
    let mut aborted = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io(e)),
        };
        if meta.is_symlink() || !meta.is_file() || meta.len() > 4096 {
            return Err(invalid());
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io(e)),
        };
        let pending = parse_marker(runtime, &path, &bytes)?;
        let terminal = runtime.store().get(pending.completion_id)?;
        let resolved = if let Some(record) = terminal {
            if terminal_binding(&record, &pending)
                && record.payload["event"] == "memory_writeback_verified"
            {
                true
            } else {
                validate_abort(runtime, &pending, &bytes, &record)?;
                aborted.push(pending.turn_id);
                true
            }
        } else {
            false
        };
        if !resolved && Some(pending.turn_id) != current_turn {
            unresolved.push(pending.turn_id);
        }
        if unresolved.len() > 128 || aborted.len() > 4096 {
            return Err(invalid());
        }
    }
    unresolved.sort_unstable();
    aborted.sort_unstable();
    Ok(
        json!({"state":if unresolved.is_empty(){"no_unresolved_turns"}else{"incomplete_or_in_flight"},
        "unresolved":unresolved.len(),"turn_ids":unresolved,
        "aborted":aborted.len(),"aborted_turn_ids":aborted}),
    )
}

pub fn finish(
    runtime: &Runtime,
    pending: PendingWriteback,
    input: EvidenceId,
    generated: EvidenceId,
    emitted: EvidenceId,
    outcomes: &[crate::c0::DraftOutcome],
) -> Result<Value, RuntimeError> {
    let submitted = outcomes.len();
    let admitted = outcomes.iter().filter(|o| o.activated).count();
    if pending.individual_id != runtime.individual_id() || admitted > submitted {
        return Err(invalid());
    }
    let mut receipts = Vec::new();
    for outcome in outcomes.iter().filter(|o| o.activated) {
        let receipt = runtime
            .store()
            .find_receipt(outcome.proposal_id)?
            .ok_or_else(invalid)?;
        if receipt.individual_id != pending.individual_id
            || receipt.proposal_id != outcome.proposal_id
        {
            return Err(invalid());
        }
        receipts.push(receipt.receipt_id);
    }
    for (id, kind) in [
        (input, EvidenceKind::UserUtterance),
        (generated, EvidenceKind::SystemEvent),
        (emitted, EvidenceKind::AgentUtterance),
    ] {
        let record = runtime.store().get(id)?.ok_or_else(invalid)?;
        if record.individual_id != pending.individual_id
            || record.turn_id != Some(pending.turn_id)
            || record.session_id != Some(pending.session_id)
            || record.kind != kind
        {
            return Err(invalid());
        }
    }
    let payload = json!({"event":"memory_writeback_verified","input_evidence_id":input,
        "generated_evidence_id":generated,"emitted_evidence_id":emitted,
        "drafts_submitted":submitted,"drafts_admitted":admitted,
        "verified_receipt_ids":receipts,
        "admission":if submitted==0{"no_drafts"}else if admitted==submitted{"all_admitted"}else{"some_or_all_rejected"},
        "verification":"evidence_readback_not_semantic_memory_quality"});
    let stored = runtime.store().append(NewEvidence {
        evidence_id: pending.completion_id,
        individual_id: pending.individual_id,
        session_id: Some(pending.session_id),
        turn_id: Some(pending.turn_id),
        kind: EvidenceKind::SystemEvent,
        origin_class: OriginClass::Observed,
        payload: payload.clone(),
        source: EvidenceSource::default(),
        retention_class: RetentionClass::Standard,
    })?;
    if runtime
        .store()
        .get(stored.evidence_id)?
        .is_none_or(|r| r != stored)
    {
        return Err(invalid());
    }
    // Only the disposable crash marker is removed; the completion is immutable evidence.
    let dir = directory(runtime);
    fs::remove_file(dir.join(format!("{}.json", pending.turn_id))).map_err(io)?;
    File::open(&dir).and_then(|f| f.sync_all()).map_err(io)?;
    Ok(payload)
}
