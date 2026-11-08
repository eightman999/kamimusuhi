//! FI2: conservative, source-bound validation before canonical projection.
//!
//! This is not a natural-language entailment oracle. A free paraphrase is
//! unverifiable. Supported projections are complete source-text quotations in stable chronological order,
//! explicit structured claims equal to pre-existing source claims, and the
//! legacy count-only episode projection. Derived sources are revalidated back
//! to raw roots; a model-supplied `faithful` flag grants no authority.

use std::collections::{BTreeMap, BTreeSet};

use kamimusuhi_core::evidence::{EvidenceKind, EvidenceLookup, EvidenceRecord, EvidenceStore};
use kamimusuhi_core::ids::{EvidenceId, MemoryId};
use kamimusuhi_core::memory::{MemoryLink, MemoryRepository};
use kamimusuhi_core::mutation::{MutationDomain, MutationOperation};
use kamimusuhi_core::persona::ProposalDraft;
use kamimusuhi_core::time::UtcTimestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Runtime, RuntimeError};

const MAX_REFS: usize = 64;
const MAX_BYTES: usize = 64 * 1024;
const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 16;
const REJECTION_MARKER: &str = "_fi2_intake_rejection";
const METADATA: &[&str] = &["tags", "links", "valid_from", "valid_to"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaithfulnessVerdict {
    Faithful,
    Unverifiable,
    Mutated,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaithfulnessReport {
    pub verdict: FaithfulnessVerdict,
    pub reason_code: &'static str,
    pub source_ids: Vec<EvidenceId>,
}

impl FaithfulnessReport {
    pub fn is_faithful(&self) -> bool {
        self.verdict == FaithfulnessVerdict::Faithful
    }
}

/// Source claims must already exist as structured data in immutable evidence.
/// The guard never extracts these fields by guessing from natural-language text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredClaim {
    pub subject: String,
    pub predicate: String,
    pub object: Value,
    pub polarity: bool,
    pub certainty: Certainty,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub sequence: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    Certain,
    Uncertain,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcedClaim {
    evidence_id: EvidenceId,
    claim: StructuredClaim,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceQuote {
    evidence_id: EvidenceId,
    quote: String,
}

#[derive(Clone)]
struct Source {
    record: EvidenceRecord,
    roots: Vec<EvidenceId>,
}

fn report(
    verdict: FaithfulnessVerdict,
    reason_code: &'static str,
    refs: &[EvidenceId],
) -> FaithfulnessReport {
    FaithfulnessReport {
        verdict,
        reason_code,
        source_ids: refs.to_vec(),
    }
}

fn unknown(reason: &'static str, refs: &[EvidenceId]) -> FaithfulnessReport {
    report(FaithfulnessVerdict::Unverifiable, reason, refs)
}

fn mutated(reason: &'static str, refs: &[EvidenceId]) -> FaithfulnessReport {
    report(FaithfulnessVerdict::Mutated, reason, refs)
}

/// Evidence actually presented through typed persona input fields. Arbitrary
/// payload/prose UUIDs cannot expand the citation authority of a model item.
pub fn presented_persona_ids(
    input: &kamimusuhi_core::persona::PersonaTurnInput,
) -> BTreeSet<EvidenceId> {
    use kamimusuhi_core::workspace::SourceRef;
    let envelope = &input.envelope;
    let mut ids = BTreeSet::from([input.input.evidence_id]);
    ids.extend(
        envelope
            .conversation_history
            .iter()
            .map(|message| message.evidence_id),
    );
    for item in envelope
        .continuity
        .iter()
        .chain(&envelope.durable_self)
        .chain(&envelope.active_policy)
        .chain(&envelope.relationship)
        .chain(&envelope.episodic)
        .chain(&envelope.recalled_evidence)
        .chain(&envelope.self_notes)
        .chain(&envelope.library)
        .chain(&envelope.external_results)
    {
        match &item.source_ref {
            SourceRef::Input { evidence_id } | SourceRef::Evidence { evidence_id, .. } => {
                ids.insert(*evidence_id);
            }
            SourceRef::Memory { evidence_refs, .. } => ids.extend(evidence_refs.iter().copied()),
            _ => {}
        }
    }
    ids.retain(|id| !id.is_nil());
    ids
}

/// Validate all declared links against typed, actually presented authority.
pub fn reject_persona_references(
    drafts: &mut [ProposalDraft],
    input: &kamimusuhi_core::persona::PersonaTurnInput,
) {
    use kamimusuhi_core::workspace::SourceRef;
    let evidence = presented_persona_ids(input);
    let envelope = &input.envelope;
    let states = envelope
        .continuity
        .iter()
        .chain(&envelope.durable_self)
        .chain(&envelope.active_policy)
        .chain(&envelope.relationship)
        .chain(&envelope.episodic)
        .chain(&envelope.recalled_evidence)
        .chain(&envelope.self_notes)
        .chain(&envelope.library)
        .chain(&envelope.external_results)
        .filter_map(|item| match item.source_ref {
            SourceRef::Memory {
                state_record_id, ..
            } => Some(state_record_id),
            _ => None,
        })
        .collect();
    reject_unpresented_references(drafts, &evidence);
    reject_unpresented_links(drafts, &evidence, &states);
}

pub(crate) fn reject_unpresented_links(
    drafts: &mut [ProposalDraft],
    evidence: &BTreeSet<EvidenceId>,
    states: &BTreeSet<MemoryId>,
) {
    fn invalid(
        value: &Value,
        evidence: &BTreeSet<EvidenceId>,
        states: &BTreeSet<MemoryId>,
        depth: usize,
    ) -> bool {
        if depth > MAX_DEPTH {
            return true;
        }
        let links_bad = value.get("links").is_some_and(|links| {
            links.as_array().is_none_or(|links| {
                links
                    .iter()
                    .any(|link| match link.as_str().and_then(MemoryLink::parse) {
                        Some(MemoryLink::Evidence(id)) => !evidence.contains(&id),
                        Some(MemoryLink::State(id)) => !states.contains(&id),
                        None => true,
                    })
            })
        });
        links_bad
            || value
                .get("projection")
                .is_some_and(|nested| invalid(nested, evidence, states, depth + 1))
    }
    for draft in drafts {
        if invalid(&draft.candidate, evidence, states, 0) {
            reject_intake(draft);
        }
    }
}

/// Mark the *whole* model item before metadata normalization can erase an
/// invented ID. The marker can only refuse a draft; a model cannot use it to
/// grant itself a pass. Callers supply exactly the evidence shown to the model.
pub fn reject_unpresented_references(
    drafts: &mut [ProposalDraft],
    presented: &BTreeSet<EvidenceId>,
) {
    for draft in drafts {
        if draft.evidence_refs.iter().any(|id| !presented.contains(id)) {
            if let Some(candidate) = draft.candidate.as_object_mut() {
                candidate.insert(REJECTION_MARKER.to_owned(), Value::Bool(true));
            } else {
                draft.candidate = serde_json::json!({REJECTION_MARKER: true});
            }
        }
    }
}

/// Preserve a refusal discovered by a caller's presentation-boundary check.
pub(crate) fn reject_intake(draft: &mut ProposalDraft) {
    if let Some(candidate) = draft.candidate.as_object_mut() {
        candidate.insert(REJECTION_MARKER.to_owned(), Value::Bool(true));
    } else {
        draft.candidate = serde_json::json!({REJECTION_MARKER: true});
    }
}

fn intake_reason(marker: &Value) -> &'static str {
    if marker.as_str() == Some("invalid_time_scope") {
        "FI2_UNSUPPORTED_TIME_SCOPE"
    } else {
        "FI2_UNPRESENTED_REFERENCE"
    }
}

pub(crate) fn reject_time_intake(draft: &mut ProposalDraft) {
    if let Some(candidate) = draft.candidate.as_object_mut() {
        candidate
            .entry(REJECTION_MARKER.to_owned())
            .or_insert_with(|| Value::String("invalid_time_scope".to_owned()));
    }
}

fn validate_direct_links(
    runtime: &Runtime,
    payload: &Value,
    refs: &[EvidenceId],
) -> Result<Option<FaithfulnessReport>, RuntimeError> {
    if let Some(links) = payload.get("links") {
        let Some(links) = links.as_array() else {
            return Ok(Some(unknown("FI2_INVALID_LINK", refs)));
        };
        for link in links {
            match link.as_str().and_then(MemoryLink::parse) {
                Some(MemoryLink::Evidence(id)) if refs.contains(&id) => {}
                Some(MemoryLink::State(id)) => {
                    if runtime
                        .store()
                        .get_state_record(id)?
                        .is_none_or(|record| record.individual_id != runtime.individual_id())
                    {
                        return Ok(Some(unknown("FI2_UNBOUND_LINK", refs)));
                    }
                }
                _ => return Ok(Some(unknown("FI2_UNBOUND_LINK", refs))),
            }
        }
    }
    Ok(None)
}

/// `None` means this is an existing non-summary fact/correction, whose
/// structural authority is still decided by the Continuity Kernel. Any draft
/// bearing an intake refusal is rejected regardless of its shape/domain.
pub fn assess_draft(
    runtime: &Runtime,
    draft: &ProposalDraft,
) -> Result<Option<FaithfulnessReport>, RuntimeError> {
    if let Some(marker) = draft.candidate.get(REJECTION_MARKER) {
        return Ok(Some(unknown(intake_reason(marker), &draft.evidence_refs)));
    }
    if let Some(refusal) = validate_direct_links(runtime, &draft.candidate, &draft.evidence_refs)? {
        return Ok(Some(refusal));
    }
    let projected = (draft.domain == MutationDomain::Episodic
        && draft.operation == MutationOperation::Capture)
        || ["summary", "claims", "projection"]
            .iter()
            .any(|key| draft.candidate.get(key).is_some());
    if !projected {
        return Ok(None);
    }
    let mut budget = MAX_NODES;
    let mut visiting = BTreeSet::new();
    Ok(Some(assess_payload(
        runtime,
        &draft.candidate,
        &draft.evidence_refs,
        draft.subject_key.as_deref(),
        0,
        &mut budget,
        &mut visiting,
    )?))
}

fn assess_payload(
    runtime: &Runtime,
    payload: &Value,
    refs: &[EvidenceId],
    subject: Option<&str>,
    depth: usize,
    budget: &mut usize,
    visiting: &mut BTreeSet<EvidenceId>,
) -> Result<FaithfulnessReport, RuntimeError> {
    if depth > MAX_DEPTH || *budget == 0 {
        return Ok(unknown("FI2_LINEAGE_BOUND", refs));
    }
    if refs.is_empty() || refs.len() > MAX_REFS {
        return Ok(unknown("FI2_MISSING_OR_EXCESS_SOURCES", refs));
    }
    let unique: BTreeSet<_> = refs.iter().copied().collect();
    if unique.len() != refs.len() {
        return Ok(unknown("FI2_DUPLICATE_SOURCE", refs));
    }
    if serde_json::to_vec(payload).map_or(true, |v| v.len() > MAX_BYTES) {
        return Ok(unknown("FI2_PROJECTION_TOO_LARGE", refs));
    }
    let Some(object) = payload.as_object() else {
        return Ok(unknown("FI2_INVALID_PROJECTION", refs));
    };
    if let Some(marker) = object.get(REJECTION_MARKER) {
        return Ok(unknown(intake_reason(marker), refs));
    }
    if object.keys().any(|key| {
        !METADATA.contains(&key.as_str())
            && ![
                "summary",
                "text",
                "turns",
                "source_quotes",
                "claims",
                "projection",
            ]
            .contains(&key.as_str())
    }) {
        return Ok(unknown("FI2_UNSUPPORTED_FIELD", refs));
    }
    if let Some(projection) = object.get("projection") {
        if object
            .keys()
            .any(|key| key != "projection" && !METADATA.contains(&key.as_str()))
        {
            return Ok(unknown("FI2_AMBIGUOUS_PROJECTION", refs));
        }
        if projection
            .as_object()
            .is_some_and(|inner| METADATA.iter().any(|key| inner.contains_key(*key)))
        {
            return Ok(unknown("FI2_NESTED_METADATA", refs));
        }
        let Some(mut merged) = projection.as_object().cloned() else {
            return Ok(unknown("FI2_INVALID_PROJECTION", refs));
        };
        for key in METADATA {
            if let Some(value) = object.get(*key) {
                merged.insert((*key).to_owned(), value.clone());
            }
        }
        return assess_payload(
            runtime,
            &Value::Object(merged),
            refs,
            subject,
            depth + 1,
            budget,
            visiting,
        );
    }
    if let Some(refusal) = validate_direct_links(runtime, payload, refs)? {
        return Ok(refusal);
    }
    let snapshot = runtime.store().facts(refs)?;
    let mut sources = BTreeMap::new();
    for id in refs {
        if *budget == 0 {
            return Ok(unknown("FI2_LINEAGE_BOUND", refs));
        }
        *budget -= 1;
        let Some(facts) = snapshot.get(*id) else {
            return Ok(unknown("FI2_UNKNOWN_SOURCE", refs));
        };
        if facts.individual_id != runtime.individual_id() {
            return Ok(unknown("FI2_FOREIGN_SOURCE", refs));
        }
        if facts.root_evidence.is_empty() {
            return Ok(unknown("FI2_MISSING_ROOT", refs));
        }
        if facts.is_corrected {
            return Ok(unknown("FI2_CORRECTED_SOURCE", refs));
        }
        let Some(record) = runtime.store().get(*id)? else {
            return Ok(unknown("FI2_UNKNOWN_SOURCE", refs));
        };
        if matches!(
            record.kind,
            EvidenceKind::UserUtterance | EvidenceKind::AgentUtterance
        ) {
            let source_subject = record
                .source
                .source_id
                .as_deref()
                .and_then(|s| s.strip_prefix("text-chat:"));
            match (source_subject, subject) {
                (Some(actual), Some(expected)) if actual == expected => {}
                (Some(_), Some(_)) => return Ok(mutated("FI2_SUBJECT_DRIFT", refs)),
                _ => return Ok(unknown("FI2_MISSING_SOURCE_SCOPE", refs)),
            }
        }
        if record.kind.is_derived() {
            if !visiting.insert(*id) {
                return Ok(unknown("FI2_LINEAGE_CYCLE", refs));
            }
            let parents = runtime
                .store()
                .links_from(*id)?
                .into_iter()
                .filter(|link| link.relation.collapses_support())
                .map(|link| link.to_evidence_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let parent_report = assess_payload(
                runtime,
                &record.payload,
                &parents,
                subject,
                depth + 1,
                budget,
                visiting,
            )?;
            visiting.remove(id);
            if !parent_report.is_faithful() {
                return Ok(unknown("FI2_UNVERIFIED_DERIVED_SOURCE", refs));
            }
        }
        sources.insert(
            *id,
            Source {
                record,
                roots: facts.root_evidence.clone(),
            },
        );
    }
    let parse_time = |value: &Value| {
        value
            .as_i64()
            .and_then(|seconds| seconds.checked_mul(1000))
            .or_else(|| {
                value
                    .as_str()
                    .and_then(UtcTimestamp::from_rfc3339)
                    .map(UtcTimestamp::unix_millis)
            })
    };
    let mut times = Vec::new();
    for key in ["valid_from", "valid_to"] {
        if let Some(value) = object.get(key) {
            let Some(time) = parse_time(value) else {
                return Ok(unknown("FI2_UNSUPPORTED_TIME_SCOPE", refs));
            };
            times.push((key, time));
        }
    }
    if times.len() == 2 && times[1].1 < times[0].1 {
        return Ok(unknown("FI2_UNSUPPORTED_TIME_SCOPE", refs));
    }
    for key in ["valid_from", "valid_to"] {
        if let Some(value) = object.get(key)
            && sources
                .values()
                .any(|source| source.record.payload.get(key) != Some(value))
        {
            return Ok(unknown("FI2_UNSUPPORTED_TIME_SCOPE", refs));
        }
    }
    if let Some(quotes) = object.get("source_quotes") {
        let Ok(quotes) = serde_json::from_value::<Vec<SourceQuote>>(quotes.clone()) else {
            return Ok(unknown("FI2_INVALID_QUOTES", refs));
        };
        let mut quoted = BTreeSet::new();
        for quote in quotes {
            let Some(source) = sources.get(&quote.evidence_id) else {
                return Ok(unknown("FI2_UNBOUND_QUOTE", refs));
            };
            if !quoted.insert(quote.evidence_id) {
                return Ok(unknown("FI2_DUPLICATE_QUOTE", refs));
            }
            if source_text(&source.record.payload) != Some(quote.quote.as_str()) {
                return Ok(mutated("FI2_QUOTE_DRIFT", refs));
            }
        }
        if quoted != unique {
            return Ok(unknown("FI2_INCOMPLETE_QUOTES", refs));
        }
    }
    let mut verified_content = false;
    if let Some(claims) = object.get("claims") {
        let Ok(claims) = serde_json::from_value::<Vec<SourcedClaim>>(claims.clone()) else {
            return Ok(unknown("FI2_INVALID_CLAIMS", refs));
        };
        if claims.is_empty() || claims.len() > MAX_REFS {
            return Ok(unknown("FI2_MISSING_OR_EXCESS_CLAIMS", refs));
        }
        for sourced in claims {
            if sourced.claim.subject.is_empty() || sourced.claim.predicate.is_empty() {
                return Ok(unknown("FI2_INVALID_CLAIM", refs));
            }
            let Some(source) = sources.get(&sourced.evidence_id) else {
                return Ok(unknown("FI2_UNBOUND_CLAIM", refs));
            };
            let Some(original) = source_claims(&source.record) else {
                return Ok(unknown("FI2_NO_STRUCTURED_SOURCE", refs));
            };
            if !original.contains(&sourced.claim) {
                return Ok(mutated(classify_claim(&sourced.claim, &original), refs));
            }
        }
        verified_content = true;
    }
    if object.contains_key("summary") && object.contains_key("text") {
        return Ok(unknown("FI2_AMBIGUOUS_TEXT", refs));
    }
    if let Some(summary) = object.get("summary").or_else(|| object.get("text")) {
        let Some(summary) = summary.as_str() else {
            return Ok(unknown("FI2_INVALID_SUMMARY", refs));
        };
        let roots: BTreeSet<_> = sources
            .values()
            .flat_map(|s| s.roots.iter().copied())
            .collect();
        let count = roots.len() as u64;
        let count_projection = object.get("turns").and_then(Value::as_u64) == Some(count)
            && sources
                .values()
                .all(|s| s.record.kind == EvidenceKind::UserUtterance)
            && summary == format!("{count} utterances from this subject");
        if !count_projection {
            if object.contains_key("turns") {
                return Ok(mutated("FI2_COUNT_DRIFT", refs));
            }
            let mut ordered = sources.values().collect::<Vec<_>>();
            ordered.sort_by_key(|source| (source.record.created_at, source.record.evidence_id));
            let originals = ordered
                .into_iter()
                .map(|source| source_text(&source.record.payload))
                .collect::<Option<Vec<_>>>();
            let Some(originals) = originals else {
                return Ok(unknown("FI2_NO_SOURCE_TEXT", refs));
            };
            if summary != originals.join("\n") {
                return Ok(unknown("FI2_UNVERIFIABLE_PARAPHRASE", refs));
            }
        }
        verified_content = true;
    } else if object.contains_key("turns") {
        return Ok(unknown("FI2_UNBOUND_COUNT", refs));
    }
    if !verified_content {
        return Ok(unknown("FI2_MISSING_CONTENT", refs));
    }
    Ok(report(FaithfulnessVerdict::Faithful, "FI2_FAITHFUL", refs))
}

fn source_text(payload: &Value) -> Option<&str> {
    payload
        .get("text")
        .or_else(|| payload.get("summary"))
        .and_then(Value::as_str)
}

fn source_claims(record: &EvidenceRecord) -> Option<Vec<StructuredClaim>> {
    if record.kind.is_derived() {
        serde_json::from_value::<Vec<SourcedClaim>>(record.payload.get("claims")?.clone())
            .ok()
            .map(|claims| claims.into_iter().map(|c| c.claim).collect())
    } else if let Some(claim) = record.payload.get("claim") {
        serde_json::from_value(claim.clone())
            .ok()
            .map(|claim| vec![claim])
    } else {
        serde_json::from_value(record.payload.get("claims")?.clone()).ok()
    }
}

fn classify_claim(claim: &StructuredClaim, originals: &[StructuredClaim]) -> &'static str {
    let same_slot: Vec<_> = originals
        .iter()
        .filter(|old| old.subject == claim.subject && old.predicate == claim.predicate)
        .collect();
    if same_slot.is_empty() {
        return "FI2_FABRICATED_CLAIM";
    }
    for old in &same_slot {
        if old.object == claim.object && old.polarity != claim.polarity {
            return "FI2_POLARITY_FLIP";
        }
        if old.object == claim.object && old.scope != claim.scope {
            return "FI2_SCOPE_DRIFT";
        }
        if old.object == claim.object && old.sequence != claim.sequence {
            return "FI2_SEQUENCE_DRIFT";
        }
        if old.object == claim.object && old.certainty != claim.certainty {
            return if old.certainty != Certainty::Certain && claim.certainty == Certainty::Certain {
                "FI2_UNCERTAINTY_NORMALIZED"
            } else {
                "FI2_CERTAINTY_DRIFT"
            };
        }
    }
    if same_slot.iter().any(|old| old.object != claim.object) {
        "FI2_OBJECT_DRIFT"
    } else {
        "FI2_CLAIM_DRIFT"
    }
}
