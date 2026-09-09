//! Read-only inspection.
//!
//! Every query here is a `SELECT`. `inspect` never claims a writer epoch,
//! never submits a proposal, and never imports anything, so running it cannot
//! move the head, add a commit, or change a row. The one thing it does write
//! is a line in the operational trace — which is not canonical state, and is
//! deliberately the only record that inspection happened.

use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::ids::IndividualId;
use kamimusuhi_core::library::LibraryRepository;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
