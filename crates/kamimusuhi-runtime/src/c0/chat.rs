//! The interactive C0 chat surface: a turn loop plus the operator commands
//! the spec asks for (`/quit`, `/status`, `/memory`, `/reflect`, `/rollback`,
//! `/context`, `/help`).
//!
//! Commands are observations or lane operations — `/memory` reads,
//! `/reflect` runs the reflection cycle, `/rollback` moves the derived head
//! back. None of them touches canonical history.

use std::io::{BufRead, Write};

use kamimusuhi_core::c0::ProposalStatus;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
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
        let rows: Vec<serde_json::Value> = memories
            .iter()
            .map(|m| {
                serde_json::json!({
                    "state_record_id": m.record.state_record_id,
                    "subject_key": m.record.subject_key,
                    "payload": m.record.payload,
                    "evidence_refs": m.record.evidence_refs,
                    "created_at": m.record.created_at.to_rfc3339(),
                })
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

/// The interactive loop. `input`/`output` are the operator's terminal — or a
/// pipe, in which case prompts are suppressed and only responses/commands
/// print.
pub fn run_repl(
    runtime: &mut Runtime,
    session: &mut DialogueSession,
    input: &mut impl BufRead,
    output: &mut impl Write,
    terminal: bool,
    debug_context: bool,
) -> Result<(), RuntimeError> {
    let reflection_interval = runtime.config().c0.reflection_interval_turns;
    if terminal {
        eprintln!("かみむすび — テキスト対話。終了: /quit、コマンド一覧: /help");
    }
    loop {
        if terminal {
            eprint!("あなた > ");
            std::io::stderr().flush().map_err(io_error)?;
        }
        let mut bytes = Vec::new();
        let read = std::io::Read::take(&mut *input, (MAX_INPUT_BYTES + 2) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(io_error)?;
        if read == 0 {
            break;
        }
        if read == MAX_INPUT_BYTES + 2 && !bytes.ends_with(b"\n") {
            return Err(RuntimeError::Usage(format!(
                "input exceeds {MAX_INPUT_BYTES} bytes"
            )));
        }
        let line = String::from_utf8(bytes)
            .map_err(|_| RuntimeError::Usage("input must be UTF-8".to_owned()))?;
        let text = line.trim_end_matches(['\r', '\n']);
        if text.len() > MAX_INPUT_BYTES {
            return Err(RuntimeError::Usage(format!(
                "input exceeds {MAX_INPUT_BYTES} bytes"
            )));
        }
        if text.trim().is_empty() {
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
        session.turn(runtime, text, |reply| {
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
        // Interval-driven reflection: the same cycle `/reflect` runs, on a
        // configured cadence.
        if reflection_interval > 0
            && session
                .turn_count()
                .is_multiple_of(u64::from(reflection_interval))
        {
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
