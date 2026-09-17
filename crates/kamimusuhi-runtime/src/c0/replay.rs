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
use kamimusuhi_core::evidence::EvidenceKind;
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::persona::{
    ConversationRole, CurrentInput, PersonaTurnInput, SessionWorkingState, TurnContext,
};
use kamimusuhi_core::workspace::WorkspaceBuilder;

use crate::c0::{self as lane, eval};
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
    let mut bytes = 0usize;
    for record in records.into_iter().rev() {
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

    let memories = lane::retrieve_memories(
        runtime.store(),
        runtime.individual_id(),
        subject,
        input_text,
        params,
    )?;
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
    let workspace = WorkspaceBuilder::new(runtime.individual_id(), runtime.now())
        .with_continuity(&head)
        .with_current_input(&CurrentInput {
            evidence_id: EvidenceId::from_u128(0xFFFF_FFFF), // replay marker, never persisted
            text: input_text.to_owned(),
        })
        .with_memories(&memories)
        .with_self_state(&view.self_model)
        .with_active_policy(params, activation_seq)
        .with_recalled_evidence(&recalled)
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
    let operative = lane::operative(runtime)?;
    let candidate = apply_proposal(&operative.view, proposal);
    let source_id = format!("text-chat:{subject}");

    // Replay inputs: recent user utterances on this subject's channel,
    // oldest first for a stable comparison order.
    let pool = runtime
        .store()
        .c0_evidence_pool(runtime.individual_id(), 256)?;
    let mut inputs: Vec<String> = pool
        .iter()
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

    let persona = runtime.config().build_persona()?;
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
            if let Some(seed) = &seed {
                turn_input.envelope = turn_input.envelope.with_seed(seed.clone());
            }
            let result = persona.turn(turn_input)?;
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
