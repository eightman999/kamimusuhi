//! Deterministic turn evaluation.
//!
//! These metrics are measured by code, not judged by a model: given the same
//! turn the same numbers come out. Axes a heuristic cannot measure are
//! recorded as null rather than faked — an eval that invents precision is
//! worse than one that admits it lacks it.

use std::collections::BTreeSet;

use kamimusuhi_core::c0::TurnMetrics;
use kamimusuhi_core::persona::{ConversationMessage, ConversationRole};
use kamimusuhi_core::workspace::{Workspace, WorkspaceDomain};

/// Question-ish inputs: interrogative punctuation or a leading question word.
/// Deliberately shallow — it only feeds the `retrieval_missed` flag, which
/// says "the turn looked like a recall question and nothing was surfaced".
fn looks_like_recall_question(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.contains('?')
        || trimmed.contains('？')
        || [
            "なぜ", "何", "いつ", "どこ", "だれ", "誰", "どう", "覚え", "前に", "前の",
        ]
        .iter()
        .any(|marker| trimmed.contains(marker))
}

/// Sentence-ish fragments for repetition/claim heuristics.
fn sentences(text: &str) -> Vec<String> {
    text.split(['。', '！', '？', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Bigram set over normalized text — shared with retrieval scoring.
fn bigrams(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    chars.windows(2).map(|w| w.iter().collect()).collect()
}

/// Measure one completed turn. Inputs are the pieces the turn already had:
/// the assembled workspace, the emitted response, the visible history and the
/// policy bound in force.
pub fn measure_turn(
    input_text: &str,
    response: &str,
    history: &[ConversationMessage],
    workspace: &Workspace,
    max_response_chars: usize,
) -> TurnMetrics {
    let memories_surfaced = workspace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item.domain,
                WorkspaceDomain::RelationshipMemory
                    | WorkspaceDomain::EpisodicMemory
                    | WorkspaceDomain::RecalledEvidence
                    | WorkspaceDomain::SelfMemory
            )
        })
        .count();

    // Repetition: share of this response's sentences already emitted verbatim
    // by the assistant in the visible history.
    let prior: BTreeSet<String> = history
        .iter()
        .filter(|m| m.role == ConversationRole::Assistant)
        .flat_map(|m| sentences(&m.text))
        .collect();
    let current = sentences(response);
    let repetition_ratio = if current.is_empty() {
        0.0
    } else {
        current.iter().filter(|s| prior.contains(*s)).count() as f64 / current.len() as f64
    };

    // Unsupported-claim proxy: assertions addressed at the user ("あなた…")
    // whose bigrams share nothing with anything presented to the model. A
    // heuristic upper bound — flagged candidates, not proven fabrications.
    let mut grounded = bigrams(input_text);
    for message in history {
        grounded.extend(bigrams(&message.text));
    }
    for item in &workspace.items {
        match &item.content {
            kamimusuhi_core::workspace::WorkspaceContent::Text { text } => {
                grounded.extend(bigrams(text));
            }
            kamimusuhi_core::workspace::WorkspaceContent::Structured { value } => {
                grounded.extend(bigrams(&value.to_string()));
            }
        }
    }
    let unsupported_marker_count = sentences(response)
        .iter()
        .filter(|s| s.contains("あなた"))
        .filter(|s| {
            let grams = bigrams(s);
            !grams.is_empty() && grams.iter().all(|g| !grounded.contains(g))
        })
        .count();

    TurnMetrics {
        response_chars: response.chars().count(),
        repetition_ratio,
        memories_surfaced,
        retrieval_missed: looks_like_recall_question(input_text) && memories_surfaced == 0,
        unsupported_marker_count,
        length_deviation: (max_response_chars > 0)
            .then(|| response.chars().count() as i64 - max_response_chars as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repetition_counts_only_verbatim_assistant_sentences() {
        let history = vec![
            ConversationMessage {
                evidence_id: kamimusuhi_core::ids::EvidenceId::from_u128(1),
                role: ConversationRole::Assistant,
                text: "静かな夜ですね。そう思います。".to_owned(),
            },
            ConversationMessage {
                evidence_id: kamimusuhi_core::ids::EvidenceId::from_u128(2),
                role: ConversationRole::User,
                text: "静かな夜ですね。".to_owned(),
            },
        ];
        // One of two sentences repeats verbatim an earlier assistant turn.
        let metrics = measure_turn(
            "質問です",
            "静かな夜ですね。新しい返事です。",
            &history,
            &kamimusuhi_core::workspace::Workspace {
                individual_id: kamimusuhi_core::ids::IndividualId::from_u128(1),
                assembled_at: kamimusuhi_core::time::UtcTimestamp::from_unix_millis(0),
                items: vec![],
            },
            0,
        );
        assert_eq!(metrics.repetition_ratio, 0.5);
    }
}
