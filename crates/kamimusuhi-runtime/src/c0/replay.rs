//! Before/after replay for derived-lane proposals.
//!
//! A replay re-runs recent user inputs through workspace assembly and the
//! configured persona under the baseline view and under the candidate view,
//! then compares deterministic metrics. It writes **nothing**: no sessions,
//! no turns, no evidence — a replay that changed state could not be an
//! evaluation of a candidate, it would be a commitment.

use std::collections::BTreeSet;

use kamimusuhi_core::c0::{
    ImprovementProposal, OperativeView, ReplayReport, apply_proposal, compare_replay,
};
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceLookup};
use kamimusuhi_core::ids::{EvidenceId, MemoryId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository, StateRecord};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_core::persona::{
    ConversationRole, CurrentInput, PersonaTurnInput, SessionWorkingState, TurnContext,
};
use kamimusuhi_core::workspace::WorkspaceBuilder;

use crate::c0::{self as lane, eval};
use crate::dialogue_recall_links::select_declared;
use crate::{Runtime, RuntimeError};

/// Assemble the workspace a replayed input would see under `view`, without
/// recording anything. Mirrors `DialogueSession::turn` assembly minus the
/// evidence writes.
fn assemble_replay(
    runtime: &Runtime,
    subject: &str,
    source_id: &str,
    input_text: &str,
    view: &OperativeView,
    activation_seq: u64,
) -> Result<
    (
        kamimusuhi_core::workspace::Workspace,
        Vec<kamimusuhi_core::persona::ConversationMessage>,
    ),
    RuntimeError,
> {
    let params = &view.params;
    // History under this view's window, from the same canonical record the
    // live path reads.
    let records = runtime.store().recent_conversation(
        runtime.individual_id(),
        source_id,
        params.retrieval.history_messages.clamp(1, 128),
    )?;
    let mut history = Vec::new();
    let facts = runtime
        .store()
        .facts(&records.iter().map(|r| r.evidence_id).collect::<Vec<_>>())?;
    let mut bytes = 0usize;
    for record in records.into_iter().rev() {
        if facts
            .get(record.evidence_id)
            .is_none_or(|f| f.is_corrected || f.root_evidence.is_empty())
        {
            continue;
        }
        let Some(text) = record.payload.get("text").and_then(|t| t.as_str()) else {
            continue;
        };
        if bytes + text.len() > 16_384 {
            break;
        }
        bytes += text.len();
        history.push(kamimusuhi_core::persona::ConversationMessage {
            evidence_id: record.evidence_id,
            role: if record.kind == EvidenceKind::UserUtterance {
                ConversationRole::User
            } else {
                ConversationRole::Assistant
            },
            text: text.to_owned(),
        });
    }
    history.reverse();

    let mut memories = lane::retrieve_memories(
        runtime.store(),
        runtime.individual_id(),
        subject,
        input_text,
        params,
    )?;
    // Declared-path expansion, mirroring the turn path: the same lexical
    // seeds grow through links/tags under the same budget and the same
    // validity window. A replay has no judge, so hits join the injected
    // memories as unjudged extras — and nothing is marked used, because
    // the model never ran.
    let now = runtime.now();
    memories.retain(|memory| memory.record.is_valid_at(now));
    if params.retrieval.episodic_top_k > 0 && params.retrieval.recall_declared_budget > 0 {
        let pool = MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(runtime.individual_id())
                .in_domain(MutationDomain::Episodic)
                .about(subject)
                .limited(64),
        )?;
        let admissible: Vec<_> = pool
            .into_iter()
            .filter(|memory| memory.record.is_valid_at(now))
            .filter(|memory| {
                crate::self_context::current_sources(
                    runtime.store(),
                    runtime.individual_id(),
                    &memory.record.evidence_refs,
                )
                .unwrap_or(false)
            })
            .collect();
        let individual_id = runtime.individual_id();
        let declared = select_declared(
            runtime.store(),
            runtime.store(),
            &admissible,
            |record: &StateRecord| {
                record.individual_id == individual_id
                    && record.subject_key.as_deref() == Some(subject)
                    && record.lifecycle_state.is_current()
                    && record.is_valid_at(now)
                    && crate::self_context::current_sources(
                        runtime.store(),
                        individual_id,
                        &record.evidence_refs,
                    )
                    .unwrap_or(false)
            },
            input_text,
            &params.retrieval,
            now,
        );
        let mut injected: BTreeSet<MemoryId> = memories
            .iter()
            .map(|memory| memory.record.state_record_id)
            .collect();
        for hit in declared.link.into_iter().chain(declared.tag) {
            if !injected.insert(hit.record.record.state_record_id) {
                continue;
            }
            let mut memory = hit.record;
            // `via` rides inside the staged payload — the same label the
            // turn path puts on offered candidates. It never reaches the
            // store: a replay writes nothing.
            if let Some(payload) = memory.record.payload.as_object_mut() {
                payload.insert("via".to_owned(), serde_json::Value::String(hit.via));
            }
            memories.push(memory);
        }
    }
    let exclude: BTreeSet<EvidenceId> = history.iter().map(|m| m.evidence_id).collect();
    let recalled = lane::recall_evidence(
        runtime.store(),
        runtime.individual_id(),
        source_id,
        input_text,
        params,
        &exclude,
    )?;
    let head = runtime.head()?;
    // Self notes ride the same lane the live path reads — a replay that
    // skipped them would measure a workspace the model never sees.
    let self_context = crate::self_context::prepare(runtime, &view.self_model)?;
    let self_notes = crate::self_context::working_notes(runtime, params.retrieval.self_notes_max)?;
    let workspace = WorkspaceBuilder::new(runtime.individual_id(), runtime.now())
        .with_continuity(&head)
        .with_current_input(&CurrentInput {
            evidence_id: EvidenceId::from_u128(0xFFFF_FFFF), // replay marker, never persisted
            text: input_text.to_owned(),
        })
        .with_memories(&memories)
        .with_self_state(&self_context.snapshot)
        .with_active_policy(params, activation_seq)
        .with_recalled_evidence(&recalled)
        .with_self_notes(&self_notes)
        .build();
    Ok((workspace, history))
}

/// Replay the most recent user inputs under baseline vs the candidate view
/// produced by `proposal`, and compare the deterministic metrics.
pub fn replay_proposal(
    runtime: &Runtime,
    subject: &str,
    proposal: &ImprovementProposal,
) -> Result<ReplayReport, RuntimeError> {
    let _identity_lease = runtime.lock_identity()?;
    runtime.ensure_identity()?;
    let operative = lane::operative(runtime)?;
    let candidate = apply_proposal(&operative.view, proposal);
    let source_id = format!("text-chat:{subject}");

    // Replay inputs: recent user utterances on this subject's channel,
    // oldest first for a stable comparison order.
    let pool = runtime
        .store()
        .recent_conversation(runtime.individual_id(), &source_id, 128)?;
    let mut inputs: Vec<String> = pool
        .iter()
        .rev()
        .filter(|r| {
            r.kind == EvidenceKind::UserUtterance
                && r.source.source_id.as_deref() == Some(source_id.as_str())
        })
        .filter_map(|r| {
            r.payload
                .get("text")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        })
        .take(runtime.config().c0.replay_turns.max(1))
        .collect();
    inputs.reverse();
    if inputs.is_empty() {
        // Nothing to replay is not a pass: a gate needs evidence of behaviour.
        return Ok(ReplayReport {
            turns: 0,
            all_completed: false,
            deltas: vec![],
            regressions: vec!["no replay inputs".to_owned()],
        });
    }

    let persona = runtime.config().build_replay_persona()?;
    let manifest = runtime.identity_manifest()?;
    let mut identity_context = runtime.identity_context().clone();
    if identity_context.is_null() {
        identity_context = serde_json::json!({});
    }
    identity_context["model_hashes"] = serde_json::json!(manifest.allowed_model_hashes);
    persona.set_identity_context(identity_context);
    let seed = runtime.config().persona_seed()?;
    let mut baseline = Vec::new();
    let mut candidate_metrics = Vec::new();
    for (index, text) in inputs.iter().enumerate() {
        for (view, seq, out) in [
            (&operative.view, operative.activation_seq, &mut baseline),
            (
                &candidate,
                operative.activation_seq + 1,
                &mut candidate_metrics,
            ),
        ] {
            let (workspace, history) =
                assemble_replay(runtime, subject, &source_id, text, view, seq)?;
            let mut turn_input = PersonaTurnInput::with_workspace(
                TurnContext {
                    individual_id: runtime.individual_id(),
                    session_id: kamimusuhi_core::ids::SessionId::generate(runtime.ids().as_ref()),
                    turn_id: kamimusuhi_core::ids::TurnId::generate(runtime.ids().as_ref()),
                },
                CurrentInput {
                    evidence_id: EvidenceId::from_u128(0xFFFF_FFFF),
                    text: text.clone(),
                },
                &workspace,
                SessionWorkingState {
                    turn_sequence: index as u64,
                    resumed: false,
                    delegations: 0,
                },
            );
            turn_input.envelope.conversation_history = history.clone();
            let self_context = crate::self_context::prepare(runtime, &view.self_model)?;
            turn_input.envelope.continuity_context = Some(crate::self_context::index(
                runtime,
                subject,
                &self_context,
                None,
            )?);
            if let Some(seed) = &seed {
                turn_input.envelope = turn_input.envelope.with_seed(seed.clone());
            }
            let result = persona.turn(turn_input)?;
            runtime.ensure_identity()?;
            out.push(eval::measure_turn(
                text,
                &result.response_intent,
                &history,
                &workspace,
                view.params.conversation.max_response_chars,
            ));
        }
    }
    Ok(compare_replay(&baseline, &candidate_metrics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamimusuhi_core::ids::PersonaBackendId;
    use kamimusuhi_core::mutation::OriginClass;
    use kamimusuhi_core::routing::LocalityClass;
    use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
    use serde_json::json;

    fn api_runtime(server: &FixtureServer, tools: &FixtureServer) -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::init(
            dir.path(),
            crate::RuntimeOptions::deterministic(0x1D1F),
            crate::ResourceImplementation::FakeA,
        )
        .unwrap();
        let mut config = runtime.config().clone();
        config.persona.backend = crate::config::PersonaBackendKind::OpenaiCompatible;
        config.persona.provider = Some(
            serde_json::from_value(json!({
                "backend_id": PersonaBackendId::from_u128(0x1D1F),
                "locality": LocalityClass::LocalHost,
                "base_url": server.base_url(), "model": "replay-fixture", "timeout_ms": 1000,
            }))
            .unwrap(),
        );
        config.tools = Some(
            serde_json::from_value(json!({
                "base_url": tools.base_url().trim_end_matches("/v1"),
            }))
            .unwrap(),
        );
        runtime.save_config(config).unwrap();
        let status = runtime.identity_status().unwrap();
        runtime.accept_identity(&status.review_token).unwrap();
        lane::narrate(
            &runtime,
            None,
            None,
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
            json!({"text": "過去の依頼を評価して"}),
            "text-chat:alice",
            &[],
        )
        .unwrap();
        (dir, runtime)
    }

    #[test]
    fn replay_never_discovers_or_calls_configured_tools_and_uses_subject_inputs() {
        let server = FixtureServer::always(FixtureResponse::ok("評価用の応答です。")).unwrap();
        let tools = FixtureServer::always(FixtureResponse::ok("{}")).unwrap();
        let (_dir, runtime) = api_runtime(&server, &tools);
        // Other people and unrelated events cannot crowd out Alice's input.
        for _ in 0..260 {
            lane::narrate(
                &runtime,
                None,
                None,
                EvidenceKind::UserUtterance,
                OriginClass::Reported,
                json!({"text": "bob-private"}),
                "text-chat:bob",
                &[],
            )
            .unwrap();
        }
        let proposal = lane::propose_param(
            &runtime,
            "conversation.max_response_chars",
            json!(200),
            None,
        )
        .unwrap();
        let report = replay_proposal(&runtime, "alice", &proposal).unwrap();
        assert_eq!(report.turns, 1);
        assert!(report.all_completed);
        assert_eq!(server.request_count(), 2);
        assert_eq!(tools.request_count(), 0);
        for request in server.requests() {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            assert!(body.get("tools").is_none());
            assert!(!request.body.contains("bob-private"));
        }
    }

    #[test]
    fn replay_binds_requests_to_reviewed_routes_and_rejects_undeclared_models() {
        let server = FixtureServer::always(FixtureResponse::ok("評価用の応答です。")).unwrap();
        let tools = FixtureServer::always(FixtureResponse::ok("{}")).unwrap();
        let (_dir, mut runtime) = api_runtime(&server, &tools);
        let origin = server.base_url().trim_end_matches("/v1").to_owned();
        let pin = format!("sha256:{}", "b".repeat(64));
        runtime
            .set_identity_context(json!({"route_pins": {origin.clone(): pin}}))
            .unwrap();
        let status = runtime.identity_status().unwrap();
        runtime.accept_identity(&status.review_token).unwrap();
        let proposal = lane::propose_param(
            &runtime,
            "conversation.max_response_chars",
            json!(200),
            None,
        )
        .unwrap();
        let head = runtime.head().unwrap();
        assert!(matches!(
            replay_proposal(&runtime, "alice", &proposal),
            Err(RuntimeError::Persona(_))
        ));
        assert_eq!(runtime.head().unwrap(), head);
        assert_eq!(tools.request_count(), 0);
        assert_eq!(server.request_count(), 1);
        let body: serde_json::Value = serde_json::from_str(&server.requests()[0].body).unwrap();
        assert_eq!(body["kamimusuhi_identity_pins"][&origin], pin);
    }
}
