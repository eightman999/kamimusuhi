//! The interactive C0 chat surface: a turn loop plus the operator commands
//! the spec asks for (`/quit`, `/status`, `/memory`, `/reflect`, `/rollback`,
//! `/context`, `/help`).
//!
//! Commands are observations or lane operations — `/memory` reads,
//! `/reflect` runs the reflection cycle, `/rollback` moves the derived head
//! back. None of them touches canonical history.

use std::io::{BufRead, Write};

use kamimusuhi_core::c0::ProposalStatus;
use kamimusuhi_core::ids::MemoryId;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository, MemoryUsage, RecordUsage};
use kamimusuhi_core::mutation::MutationDomain;

use crate::c0::{self as lane, reflection};
use crate::dialogue::{DialogueSession, MAX_INPUT_BYTES};
use crate::{Runtime, RuntimeError};

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::Usage(format!("text input/output failed: {error}"))
}

fn json_error(error: serde_json::Error) -> RuntimeError {
    RuntimeError::Usage(format!("output encoding failed: {error}"))
}

fn emit_json(output: &mut impl Write, value: &serde_json::Value) -> Result<(), RuntimeError> {
    serde_json::to_writer_pretty(&mut *output, value).map_err(json_error)?;
    writeln!(output).map_err(io_error)?;
    output.flush().map_err(io_error)
}

/// `/status` — where this session and the two lanes currently stand.
fn status(runtime: &Runtime, session: &DialogueSession) -> Result<serde_json::Value, RuntimeError> {
    let head = runtime.head()?;
    let operative = lane::operative(runtime)?;
    let pending = runtime
        .store()
        .c0_proposals(runtime.individual_id(), Some(ProposalStatus::Pending))?;
    let persona = runtime.config().build_persona()?;
    let backend = persona.descriptor();
    Ok(serde_json::json!({
        "individual_id": runtime.individual_id(),
        "canonical_head": {
            "commit_id": head.commit_id,
            "generation": head.generation.0,
            "writer_epoch": head.writer_epoch.0,
        },
        "derived_lane": {
            "activation_seq": operative.activation_seq,
            "self_entries": operative.view.self_model.entry_count(),
            "params": operative.view.params,
        },
        "session_id": session.session_id(),
        "subject": session.subject(),
        "turns": session.turn_count(),
        "persona_backend": backend,
        "pending_proposals": pending.len(),
    }))
}

/// `/memory` — the retained state visible to this subject: canonical
/// relationship/episodic records, the derived self model, the operative
/// params and pending proposals. Read-only.
fn memory(runtime: &Runtime, session: &DialogueSession) -> Result<serde_json::Value, RuntimeError> {
    let individual = runtime.individual_id();
    let operative = lane::operative(runtime)?;
    let mut records = serde_json::Map::new();
    for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
        let memories = MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(individual)
                .in_domain(domain)
                .about(session.subject())
                .limited(32),
        )?;
        let ids: Vec<MemoryId> = memories.iter().map(|m| m.record.state_record_id).collect();
        let usages: std::collections::BTreeMap<MemoryId, RecordUsage> =
            runtime.store().usages(&ids).into_iter().collect();
        let rows: Vec<serde_json::Value> = memories
            .iter()
            .map(|m| {
                let mut row = serde_json::json!({
                    "state_record_id": m.record.state_record_id,
                    "subject_key": m.record.subject_key,
                    "payload": m.record.payload,
                    "evidence_refs": m.record.evidence_refs,
                    "created_at": m.record.created_at.to_rfc3339(),
                });
                // Declared metadata and usage bookkeeping surface only when
                // present — a record without them renders exactly as before.
                let tags = m.record.tags();
                if !tags.is_empty() {
                    row["tags"] = serde_json::json!(tags);
                }
                let links = m.record.links();
                if !links.is_empty() {
                    row["links"] = serde_json::json!(links);
                }
                if let Some(usage) = usages.get(&m.record.state_record_id) {
                    row["use_count"] = serde_json::json!(usage.use_count);
                }
                if let Some(valid_to) = m.record.valid_to() {
                    row["valid_to"] = serde_json::json!(valid_to.to_rfc3339());
                }
                row
            })
            .collect();
        records.insert(domain.as_str().to_owned(), serde_json::json!(rows));
    }
    let pending = runtime
        .store()
        .c0_proposals(individual, Some(ProposalStatus::Pending))?;
    let activations = runtime.store().c0_activations(individual)?;
    Ok(serde_json::json!({
        "activation_seq": operative.activation_seq,
        "relationship": records["relationship"],
        "episodic": records["episodic"],
        "self_model": operative.view.self_model,
        "operative_params": operative.view.params,
        "pending_proposals": pending
            .iter()
            .map(lane::ProposalReport::from)
            .collect::<Vec<_>>(),
        "activations": activations
            .iter()
            .map(|a| serde_json::json!({
                "seq": a.activation_seq,
                "proposal_id": a.proposal_id,
                "predecessor_seq": a.predecessor_seq,
                "rolled_back": a.rolled_back_at.is_some(),
            }))
            .collect::<Vec<_>>(),
    }))
}

const HELP: &str = "\
commands:
  /quit      end the session
  /status    session, canonical head and derived-lane position as JSON
  /memory    retained state visible to this subject as JSON
  /reflect   run one reflection cycle now
  /rollback  move the derived lane back one activation (canonical history is
             untouched)
  /context   the workspace assembled for the previous turn, as JSON
  /help      this list
anything else is a turn of conversation";

/// The automatic reflection cadence in force, in turns. Genome connection:
/// `cognition.reflection_depth` scales the configured interval. 0 (manual
/// only) stays 0; the operator's off switch wins.
pub fn effective_reflection_interval(runtime: &Runtime) -> u32 {
    runtime
        .phenotype()
        .reflection_interval(runtime.config().c0.reflection_interval_turns)
}

/// Whether the cadence says a reflection cycle is due after the session's
/// most recent turn.
pub fn reflection_due(runtime: &Runtime, session: &DialogueSession) -> bool {
    let interval = effective_reflection_interval(runtime);
    interval > 0 && session.turn_count().is_multiple_of(u64::from(interval))
}

/// The interactive loop. `input`/`output` are the operator's terminal — or a
/// pipe, in which case prompts are suppressed and only responses/commands
/// print.
///
/// `quiet` is the resident's protocol mode: every input line must be a JSON
/// object `{"message": "<text>"}` and every output line is one complete JSON
/// document — a turn reply, or `{"error": ...}` for an input that is not a
/// valid turn. Interactive chrome and `/` commands are not part of the
/// protocol.
#[derive(Debug, Default)]
pub struct ReplOptions {
    /// Attached to a real terminal: banner, prompts and plain-text replies.
    pub terminal: bool,
    /// Emit the assembled workspace context to stderr.
    pub debug_context: bool,
    /// Emit the secret-free turn trace to stderr.
    pub debug_trace: bool,
    /// JSON-lines protocol mode (see `run_repl`).
    pub quiet: bool,
}

pub fn run_repl(
    runtime: &mut Runtime,
    session: &mut DialogueSession,
    input: &mut impl BufRead,
    output: &mut impl Write,
    options: ReplOptions,
) -> Result<(), RuntimeError> {
    let ReplOptions {
        terminal,
        debug_context,
        debug_trace,
        quiet,
    } = options;
    let interactive = terminal && !quiet;
    if interactive {
        eprintln!("かみむすび — テキスト対話。終了: /quit、コマンド一覧: /help");
    }
    loop {
        if interactive {
            eprint!("あなた > ");
            std::io::stderr().flush().map_err(io_error)?;
        }
        let frame_limit = if quiet { 128 * 1024 } else { MAX_INPUT_BYTES };
        let mut bytes = Vec::new();
        let read = std::io::Read::take(&mut *input, (frame_limit + 2) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(io_error)?;
        if read == 0 {
            break;
        }
        if read == frame_limit + 2 && !bytes.ends_with(b"\n") {
            return Err(RuntimeError::Usage(format!(
                "input exceeds {MAX_INPUT_BYTES} bytes"
            )));
        }
        let line = String::from_utf8(bytes)
            .map_err(|_| RuntimeError::Usage("input must be UTF-8".to_owned()))?;
        let text = line.trim_end_matches(['\r', '\n']);
        if text.len() > frame_limit {
            return Err(RuntimeError::Usage(format!(
                "input exceeds {MAX_INPUT_BYTES} bytes"
            )));
        }
        if text.trim().is_empty() {
            continue;
        }
        if quiet {
            // Protocol mode: one JSON turn in, one JSON reply out — a message
            // may itself contain newlines or start with '/', which raw-line
            // framing could not carry.
            let request = serde_json::from_str::<serde_json::Value>(text);
            let message = request
                .as_ref()
                .ok()
                .and_then(|v| v.get("message"))
                .and_then(serde_json::Value::as_str);
            match message {
                Some(message) if !message.trim().is_empty() && message.len() <= MAX_INPUT_BYTES => {
                    // An optional "turn" names the resident's task id for
                    // this turn, so the model/tool calls the session makes
                    // back to the resident are attributed per turn even
                    // though the process env is fixed at spawn.
                    if let Some(turn) = request
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("turn"))
                        .and_then(serde_json::Value::as_str)
                    {
                        // One turn at a time on this thread: the slot is
                        // read only by this process's own HTTP calls during
                        // the turn below.
                        kamimusuhi_resource_http::http::set_turn(Some(turn));
                    }
                    if let Some(context) = request
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("identity_context"))
                    {
                        runtime.set_identity_context(context.clone())?;
                    }
                    let reply = session.turn(runtime, message, |reply| {
                        serde_json::to_writer(&mut *output, reply)?;
                        writeln!(output)?;
                        output.flush()
                    })?;
                    if debug_context && let Some(context) = session.last_context() {
                        serde_json::to_writer_pretty(std::io::stderr(), context)
                            .map_err(json_error)?;
                        writeln!(std::io::stderr()).map_err(io_error)?;
                    }
                    if debug_trace && let Some(trace) = reply.llm_jev {
                        let value = serde_json::to_value(trace).map_err(json_error)?;
                        emit_json(&mut std::io::stderr(), &value)?;
                    }
                    // Reflection runs on cadence but never prints: one reply
                    // line per turn keeps the protocol one-to-one.
                    if reflection_due(runtime, session) {
                        let subject = session.subject().to_owned();
                        reflection::run(
                            runtime,
                            &subject,
                            Some(session.session_id()),
                            None,
                            session.writer_cache(),
                        )?;
                    }
                }
                _ => {
                    writeln!(
                        output,
                        "{}",
                        serde_json::json!({"error": "quiet mode expects one JSON object per line: {\"message\": \"<text>\"}"})
                    )
                    .map_err(io_error)?;
                    output.flush().map_err(io_error)?;
                }
            }
            continue;
        }
        if let Some(command) = text.strip_prefix('/') {
            match command.trim() {
                "quit" => break,
                "status" => emit_json(output, &status(runtime, session)?)?,
                "memory" => emit_json(output, &memory(runtime, session)?)?,
                "reflect" => {
                    let subject = session.subject().to_owned();
                    let session_id = session.session_id();
                    let report = reflection::run(
                        runtime,
                        &subject,
                        Some(session_id),
                        None,
                        session.writer_cache(),
                    )?;
                    emit_json(output, &serde_json::json!(report))?;
                }
                "rollback" => match lane::rollback(runtime, Some(session.session_id()), None) {
                    Ok(outcome) => emit_json(
                        output,
                        &serde_json::json!({
                            "rolled_back_seq": outcome.rolled_back_seq,
                            "restored_seq": outcome.restored_seq,
                            "proposal_id": outcome.proposal_id,
                        }),
                    )?,
                    Err(e) => emit_json(
                        output,
                        &serde_json::json!({"rollback": "refused", "reason": e.to_string()}),
                    )?,
                },
                "context" => match session.last_context() {
                    Some(context) => emit_json(output, context)?,
                    None => emit_json(output, &serde_json::json!({"context": null}))?,
                },
                "help" => {
                    writeln!(output, "{HELP}").map_err(io_error)?;
                }
                other => emit_json(
                    output,
                    &serde_json::json!({"unknown_command": other, "see": "/help"}),
                )?,
            }
            continue;
        }
        let reply = session.turn(runtime, text, |reply| {
            if terminal {
                write!(output, "かみむすび > ")?;
            }
            writeln!(output, "{}", reply.response)?;
            output.flush()
        })?;
        if debug_context && let Some(context) = session.last_context() {
            serde_json::to_writer_pretty(std::io::stderr(), context).map_err(json_error)?;
            writeln!(std::io::stderr()).map_err(io_error)?;
        }
        if debug_trace && let Some(trace) = reply.llm_jev {
            let value = serde_json::to_value(trace).map_err(json_error)?;
            emit_json(&mut std::io::stderr(), &value)?;
        }
        // Interval-driven reflection: the same cycle `/reflect` runs, on a
        // configured cadence.
        if reflection_due(runtime, session) {
            let subject = session.subject().to_owned();
            let report = reflection::run(
                runtime,
                &subject,
                Some(session.session_id()),
                None,
                session.writer_cache(),
            )?;
            if terminal {
                emit_json(output, &serde_json::json!({"reflection": report}))?;
            }
        }
    }
    Ok(())
}
