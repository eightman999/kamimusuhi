//! Read-only, bounded projection of accepted self reports. Exclusion never
//! erases history and never promotes a self report into an independent fact.

use kamimusuhi_core::c0::{SELF_MAX_ENTRIES, SELF_SNAPSHOT_MAX_BYTES, SelfSnapshot};
use kamimusuhi_core::digest::json_digest;
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceLookup};
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::MutationDomain;
use serde_json::{Value, json};

use crate::{Runtime, RuntimeError};

pub struct SelfContext {
    pub snapshot: SelfSnapshot,
    pub audit: Value,
}

/// Scratch remains unverified; a recognized, corrected provenance hint is
/// nevertheless sufficient to retire it from the current context.
pub fn working_notes(
    runtime: &Runtime,
    max: usize,
) -> Result<Vec<kamimusuhi_core::self_note::SelfNote>, RuntimeError> {
    let notes = crate::self_notes::latest(&runtime.paths().dir, max);
    let mut current = Vec::new();
    for note in notes {
        let refs = note
            .evidence_refs
            .iter()
            .filter_map(|s| {
                s.strip_prefix("evidence:")
                    .unwrap_or(s)
                    .parse::<EvidenceId>()
                    .ok()
            })
            .collect::<Vec<_>>();
        let facts = runtime.store().facts(&refs)?;
        if facts.corrected().is_empty() {
            current.push(note);
        }
    }
    Ok(current)
}

/// Roots, not just the direct citation, must remain current. A generated
/// assertion is evidence of what was said; this function cannot prove its truth.
pub fn current_evidence(runtime: &Runtime, refs: &[EvidenceId]) -> Result<bool, RuntimeError> {
    if !current_sources(runtime.store(), runtime.individual_id(), refs)? {
        return Ok(false);
    }
    let facts = runtime.store().facts(refs)?;
    Ok(facts.facts().iter().all(|f| {
        matches!(
            f.kind,
            EvidenceKind::AgentUtterance | EvidenceKind::Reflection | EvidenceKind::SystemEvent
        )
    }))
}

pub fn current_sources(
    store: &kamimusuhi_store_sqlite::SqliteStore,
    individual: kamimusuhi_core::ids::IndividualId,
    refs: &[EvidenceId],
) -> Result<bool, RuntimeError> {
    if refs.is_empty() {
        return Ok(false);
    }
    let facts = store.facts(refs)?;
    let direct_valid = refs.iter().all(|id| {
        facts.get(*id).is_some_and(|f| {
            f.individual_id == individual && !f.is_corrected && !f.root_evidence.is_empty()
        })
    });
    if !direct_valid {
        return Ok(false);
    }
    let roots = facts.independent_support().into_iter().collect::<Vec<_>>();
    let root_facts = store.facts(&roots)?;
    Ok(roots.iter().all(|id| {
        root_facts
            .get(*id)
            .is_some_and(|f| f.individual_id == individual && !f.is_corrected)
    }))
}

/// The same projection is used for live dialogue, reflection and replay.
pub fn prepare(runtime: &Runtime, stored: &SelfSnapshot) -> Result<SelfContext, RuntimeError> {
    let mut snapshot = SelfSnapshot::default();
    let mut excluded = Vec::new();
    let mut excluded_count = 0;
    for (field, entries) in &stored.fields {
        for entry in entries {
            let reason = if snapshot.entry_count() >= SELF_MAX_ENTRIES {
                Some("self_context_entry_budget")
            } else if !current_evidence(runtime, &entry.evidence_refs)? {
                Some("missing_foreign_or_corrected_evidence")
            } else {
                let mut candidate = snapshot.clone();
                candidate
                    .fields
                    .entry(field.clone())
                    .or_default()
                    .push(entry.clone());
                if serde_json::to_vec(&candidate)
                    .map_err(|e| RuntimeError::Usage(e.to_string()))?
                    .len()
                    > SELF_SNAPSHOT_MAX_BYTES
                {
                    Some("self_context_byte_budget")
                } else {
                    snapshot = candidate;
                    None
                }
            };
            if let Some(reason) = reason {
                excluded_count += 1;
                if excluded.len() < SELF_MAX_ENTRIES {
                    excluded.push(json!({"field_prefix":field.chars().take(128).collect::<String>(),"key_prefix":entry.key.as_ref().map(|key| key.chars().take(128).collect::<String>()),"proposal_id":entry.proposal_id,"evidence_refs":entry.evidence_refs.iter().take(8).collect::<Vec<_>>(),"reason":reason}));
                }
            }
        }
    }
    let audit = json!({
        "authority":"accepted_self_report_not_independent_fact",
        "stored_entries":stored.entry_count(),
        "surfaced_entries":snapshot.entry_count(),
        "stored_digest":json_digest(&json!(stored)),
        "surfaced_digest":json_digest(&json!(snapshot)),
        "byte_limit":SELF_SNAPSHOT_MAX_BYTES,
        "excluded":excluded,
        "excluded_count":excluded_count,
        "excluded_detail_limit":SELF_MAX_ENTRIES,
    });
    Ok(SelfContext { snapshot, audit })
}

/// Small always-present pointers, separate from the retained core and from
/// query-selected episodic material. No retrieved text is turned into policy.
pub fn index(
    runtime: &Runtime,
    subject: &str,
    context: &SelfContext,
    current_turn: Option<kamimusuhi_core::ids::TurnId>,
) -> Result<Value, RuntimeError> {
    let mut memory_pointers = Vec::new();
    for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
        let records = MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(runtime.individual_id())
                .in_domain(domain)
                .about(subject)
                .limited(16),
        )?;
        for memory in records {
            let facts = runtime.store().facts(&memory.record.evidence_refs)?;
            if !memory.record.is_valid_at(runtime.now()) || !facts.corrected().is_empty() {
                continue;
            }
            memory_pointers.push(
                json!({"state_record_id":memory.record.state_record_id,"domain":domain,
                "evidence_refs":memory.record.evidence_refs.iter().take(8).collect::<Vec<_>>(),
                "additional_evidence_refs":memory.record.evidence_refs.len().saturating_sub(8)}),
            );
        }
    }
    Ok(json!({
        "authority":"navigation_only_no_mutation_authority",
        "history_policy":"corrected_direct_or_derived_sources_excluded_from_active_context",
        "subject":subject,
        "active_goals":context.snapshot.fields.get("active_goals"),
        "open_questions":context.snapshot.fields.get("open_questions"),
        "memory_pointers":memory_pointers,
        "memory_pointer_limit_per_domain":16,
        "self_projection":context.audit,
        "prior_writeback":crate::memory_writeback::status_excluding(runtime,current_turn)?,
    }))
}
