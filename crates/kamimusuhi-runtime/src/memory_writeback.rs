//! Per-turn crash markers plus immutable, read-back-verified completion events.
//! Response delivery, evidence persistence and memory admission are distinct.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::mutation::OriginClass;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Runtime, RuntimeError};

const DIRECTORY: &str = "memory-writeback";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingWriteback {
    individual_id: IndividualId,
    session_id: SessionId,
    turn_id: TurnId,
    completion_id: EvidenceId,
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
            return Ok(json!({"state":"no_unresolved_turns","unresolved":0}));
        }
        Err(e) => return Err(io(e)),
        Ok(_) => {}
    }
    regular(&dir, true)?;
    let mut unresolved = Vec::new();
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
        let pending: PendingWriteback = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if pending.individual_id != runtime.individual_id()
            || path.file_name().and_then(|s| s.to_str())
                != Some(&format!("{}.json", pending.turn_id))
        {
            return Err(invalid());
        }
        let completed = runtime
            .store()
            .get(pending.completion_id)?
            .is_some_and(|r| {
                r.individual_id == pending.individual_id
                    && r.turn_id == Some(pending.turn_id)
                    && r.session_id == Some(pending.session_id)
                    && r.kind == EvidenceKind::SystemEvent
                    && r.payload["event"] == "memory_writeback_verified"
            });
        if !completed && Some(pending.turn_id) != current_turn {
            unresolved.push(pending.turn_id);
        }
        if unresolved.len() > 128 {
            return Err(invalid());
        }
    }
    Ok(
        json!({"state":if unresolved.is_empty(){"no_unresolved_turns"}else{"incomplete_or_in_flight"},
        "unresolved":unresolved.len(),"turn_ids":unresolved}),
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
