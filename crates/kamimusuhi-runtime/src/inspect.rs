//! Read-only inspection.
//!
//! Every query here is a `SELECT`. `inspect` never claims a writer epoch,
//! never submits a proposal, and never imports anything, so running it cannot
//! move the head, add a commit, or change a row. The one thing it does write
//! is a line in the operational trace — which is not canonical state, and is
//! deliberately the only record that inspection happened.

use kamimusuhi_core::c0::ProposalStatus;
use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::ids::{IndividualId, MemoryId};
use kamimusuhi_core::library::LibraryRepository;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository, MemoryUsage, RecordUsage};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_core::resources::{ResourceCallLog, ResourceOutcome};
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;
use crate::runtime::Runtime;
use crate::scenario::HeadReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySummary {
    pub active: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySummary {
    pub artifacts: usize,
    pub chunks: u32,
}

/// What the durable call log says, summarized.
///
/// `total` counts *logical* calls — one row per question asked. `attempts`
/// counts the physical tries behind them, so a gap between the two is retry,
/// not duplicated attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCallSummary {
    pub total: usize,
    pub ok: usize,
    pub errors: usize,
    pub attempts: u32,
    pub retried_calls: usize,
    pub timeouts: usize,
    /// error code → how many logical calls ended that way.
    pub error_codes: std::collections::BTreeMap<String, usize>,
    pub max_latency_ms: u64,
    /// Logical calls correlated to a turn in the database.
    pub correlated_to_turn: usize,
    pub resources_used: Vec<String>,
}

/// What `inspect` reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectReport {
    pub individual_id: IndividualId,
    pub root_commit_id: kamimusuhi_core::ids::CommitId,
    pub head: HeadReport,
    pub schema_version: u32,
    pub config_version: u32,
    pub node_id: kamimusuhi_core::ids::NodeId,
    pub resources: std::collections::BTreeMap<String, String>,
    pub canonical_commits: usize,
    pub episodic: MemorySummary,
    pub relationship: MemorySummary,
    pub library: LibrarySummary,
    pub resource_calls: ResourceCallSummary,
    /// The derived C0 lane, summarized separately from canonical state: it
    /// has a movable head and is the only part of this report rollback can
    /// change.
    pub c0: C0Summary,
    /// The heritable disposition beside the config. Not canonical state and
    /// not memory: only its identity and derived settings are reported.
    #[serde(default)]
    pub genome: Option<GenomeSummary>,
}

/// Genome identity plus the runtime settings it currently derives to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenomeSummary {
    pub genome_id: String,
    pub version: u32,
    pub hash: String,
    pub phenotype: kamimusuhi_core::genome::Phenotype,
}

/// Derived-lane position and lifecycle counts. All `SELECT`s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C0Summary {
    pub activation_seq: u64,
    pub self_entries: usize,
    /// lifecycle status → proposal count.
    pub proposals: std::collections::BTreeMap<String, usize>,
    pub evaluations: usize,
    pub rolled_back_activations: usize,
}

/// Collect the report. Read-only by construction.
pub fn inspect(runtime: &Runtime) -> Result<InspectReport, RuntimeError> {
    let individual = runtime.individual();
    let store = runtime.store();
    let head = runtime.head()?;

    let summarize = |domain: MutationDomain| -> Result<MemorySummary, RuntimeError> {
        let active = MemoryRepository::retrieve(
            store,
            &MemoryQuery::current(individual.individual_id).in_domain(domain),
        )?
        .len();
        let total = MemoryRepository::retrieve(
            store,
            &MemoryQuery::current(individual.individual_id)
                .in_domain(domain)
                .including_history(),
        )?
        .len();
        Ok(MemorySummary { active, total })
    };

    // The Library is not owned by the individual, so it is summarized as what
    // this runtime can read, not as part of anyone's state.
    let artifacts = LibraryRepository::artifacts(store)?;
    let library = LibrarySummary {
        artifacts: artifacts.len(),
        chunks: artifacts.iter().map(|a| a.chunk_count).sum(),
    };

    let calls = store.calls(individual.individual_id)?;
    let mut resources_used: Vec<String> = calls
        .iter()
        .map(|call| call.resource_id.to_string())
        .collect();
    resources_used.sort_unstable();
    resources_used.dedup();
    let mut error_codes: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for code in calls.iter().filter_map(|c| c.error_code.as_deref()) {
        *error_codes.entry(code.to_owned()).or_default() += 1;
    }
    let resource_calls = ResourceCallSummary {
        total: calls.len(),
        ok: calls
            .iter()
            .filter(|c| c.outcome == ResourceOutcome::Ok)
            .count(),
        errors: calls
            .iter()
            .filter(|c| c.outcome == ResourceOutcome::Error)
            .count(),
        attempts: calls.iter().map(|c| c.attempts).sum(),
        retried_calls: calls.iter().filter(|c| c.attempts > 1).count(),
        timeouts: calls
            .iter()
            .filter(|c| c.error_code.as_deref() == Some("TIMEOUT"))
            .count(),
        error_codes,
        max_latency_ms: calls.iter().map(|c| c.latency_ms).max().unwrap_or(0),
        correlated_to_turn: calls.iter().filter(|c| c.turn_id.is_some()).count(),
        resources_used,
    };

    let operative = crate::c0::operative(runtime).unwrap_or_default();
    let proposals = store.c0_proposals(individual.individual_id, None)?;
    let mut proposal_counts: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for proposal in &proposals {
        *proposal_counts
            .entry(proposal.status.as_str().to_owned())
            .or_default() += 1;
    }
    let activations = store.c0_activations(individual.individual_id)?;
    let c0 = C0Summary {
        activation_seq: operative.activation_seq,
        self_entries: operative.view.self_model.entry_count(),
        proposals: proposal_counts,
        evaluations: store
            .c0_evaluations(individual.individual_id, None, 1024)?
            .len(),
        rolled_back_activations: activations
            .iter()
            .filter(|a| a.rolled_back_at.is_some())
            .count(),
    };

    let report = InspectReport {
        individual_id: individual.individual_id,
        root_commit_id: individual.root_commit_id,
        head: head.into(),
        schema_version: runtime.schema_version().0,
        config_version: runtime.config().config_version,
        node_id: runtime.config().node_id,
        resources: runtime
            .config()
            .resources
            .iter()
            .map(|(slot, implementation)| (slot.clone(), implementation.as_str().to_owned()))
            .collect(),
        canonical_commits: store.commits(individual.individual_id)?.len(),
        episodic: summarize(MutationDomain::Episodic)?,
        relationship: summarize(MutationDomain::Relationship)?,
        library,
        resource_calls,
        c0,
        genome: Some(GenomeSummary {
            genome_id: runtime.genome().genome_id.clone(),
            version: runtime.genome().version,
            hash: runtime.genome().hash.clone(),
            phenotype: runtime.phenotype(),
        }),
    };

    // Operational only. Inspection is not a canonical event and gets no audit
    // row; this line is how it stays visible anyway.
    runtime.trace().record_with(
        TraceEventKind::InspectInvoked,
        TraceCorrelation::default(),
        serde_json::json!({
            "generation": report.head.generation,
            "canonical_commits": report.canonical_commits,
        }),
    );
    Ok(report)
}

/// Read-only memory report: per-domain counts plus the newest current
/// records, the derived self model, operative params and pending C0
/// proposals. `subject` scopes records to one interlocutor key (as dialogue
/// does); `None` reports across subjects. All SELECTs — the same guarantee
/// as [`inspect`]. Served to the operator dashboard by the resident's
/// `/v1/kamimusuhi/memory` endpoint.
pub fn memory_report(
    runtime: &Runtime,
    subject: Option<&str>,
    limit: usize,
) -> Result<serde_json::Value, RuntimeError> {
    let individual = runtime.individual_id();
    let store = runtime.store();
    let limit = limit.clamp(1, 200);
    let operative = crate::c0::operative(runtime).unwrap_or_default();

    let query = |domain: MutationDomain| {
        let q = MemoryQuery::current(individual).in_domain(domain);
        match subject {
            Some(subject) => q.about(subject),
            None => q,
        }
    };
    let mut domains = serde_json::Map::new();
    for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
        let active =
            MemoryRepository::retrieve(store, &query(domain))?.len();
        let total = MemoryRepository::retrieve(store, &query(domain).including_history())?.len();
        let memories = MemoryRepository::retrieve(store, &query(domain).limited(limit))?;
        let ids: Vec<MemoryId> = memories.iter().map(|m| m.record.state_record_id).collect();
        let usages: std::collections::BTreeMap<MemoryId, RecordUsage> =
            store.usages(&ids).into_iter().collect();
        let records: Vec<serde_json::Value> = memories
            .iter()
            .map(|m| {
                let mut row = serde_json::json!({
                    "state_record_id": m.record.state_record_id,
                    "subject_key": m.record.subject_key,
                    "payload": m.record.payload,
                    "evidence_refs": m.record.evidence_refs,
                    "independent_evidence_count": m.independent_evidence_count,
                    "created_at": m.record.created_at.to_rfc3339(),
                });
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
        domains.insert(
            domain.as_str().to_owned(),
            serde_json::json!({"active": active, "total": total, "records": records}),
        );
    }

    let pending = store.c0_proposals(individual, Some(ProposalStatus::Pending))?;
    Ok(serde_json::json!({
        "individual_id": individual,
        "subject": subject,
        "generated_at": runtime.now().to_rfc3339(),
        "activation_seq": operative.activation_seq,
        "self_model": operative.view.self_model,
        "operative_params": operative.view.params,
        "pending_proposals": pending
            .iter()
            .map(crate::c0::ProposalReport::from)
            .collect::<Vec<_>>(),
        "domains": domains,
    }))
}
