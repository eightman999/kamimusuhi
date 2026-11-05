//! Declared-link recall evaluation — the kamimusuhi analog of FIO's
//! `recall_lab.py`.
//!
//! For every current episodic [`StateRecord`] R that declared at least one
//! link at its creation time T, the evaluator rebuilds the world as it
//! existed just before T — records created earlier, still inside their
//! declared validity window, and current as of T — then replays the input
//! that immediately preceded T through the same recall path a turn uses:
//! [`c0::rank_episodic`] lexical seeds plus [`select_declared`] link/tag
//! expansion. The links R declared are the ground truth; recall is the
//! share of their targets the replayed recall would have offered the
//! relevance judge.
//!
//! The measurement is deliberately a lower bound, for the same reasons FIO
//! documents for itself (see `docs/fio-memory-architecture-measurements-
//! 2026-11-03.md` §3.3): the ground truth is only "the links declared at
//! T", not every memory the turn actually needed, and the replay cannot
//! reconstruct the hot/usage seeds the original turn saw — the usage table
//! keeps only the latest mark, not a history. Evaluation therefore runs
//! with `recall_hot_window_secs = 0`, restricting seeds to the lexical
//! path. Every admitted approximation is listed again in the report's
//! `limits` field so a reader never has to trust the numbers blind.
//!
//! The command is read-only: it opens the runtime like `inspect`, never
//! claims a writer epoch, and issues only `SELECT`s.

use std::collections::BTreeMap;

use kamimusuhi_core::c0::RetrievalParams;
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceRecord, EvidenceStore};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, MemoryId};
use kamimusuhi_core::memory::{
    AttributedMemory, MemoryLink, MemoryQuery, MemoryRepository, StateRecord,
};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::SqliteStore;
use serde::Serialize;
use serde_json::Value;

use crate::dialogue_recall_links::{KNOT_MIN_IN_DEGREE, select_declared};
use crate::{Runtime, RuntimeError, c0};

/// How far back the evidence scan may reach when looking for the utterance
/// that preceded a record. Beyond it the row falls back to the record's own
/// text — bounded work beats silent correctness at this store size.
const EVIDENCE_SCAN_LIMIT: usize = 8_192;

/// What this measurement cannot claim, repeated into every report.
const LIMITS: &[&str] = &[
    "ground truth is only the links declared at creation time T, not every memory the turn needed; recall is a lower bound",
    "the replayed query is the newest user utterance before T on the record's subject channel; rows without one fall back to the record's own text (see query_source)",
    "hot/used seeds are excluded: usage history is not reconstructible, so recall_hot_window_secs is forced to 0",
    "lifecycle is approximated: a record superseded only by a post-T record counts as current at T, but an invalidated record is always excluded because its invalidation time is unknown",
    "the relevance judge is not replayed: 'surfaced' means the candidate offer set (lexical seeds + link/tag frames), not what passed judgment",
    "subject scope mirrors the turn path: only records about the evaluated record's subject_key can surface, so cross-subject declared targets always miss",
    "the retrieval parameters are the operative view in force today, not necessarily the ones in force at T",
];

/// How the replayed query for one row was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuerySource {
    /// The newest `UserUtterance` before T on `text-chat:<subject>`.
    PriorUserUtterance,
    /// No prior utterance existed; the record's own text stood in.
    RecordTextFallback,
}

/// One record the replayed recall offered, and the lane that offered it.
#[derive(Debug, Clone, Serialize)]
pub struct SurfacedRecord {
    pub state_record_id: MemoryId,
    /// `lexical_seed`, `link_hop_<n>` or `tag`.
    pub via: String,
}

/// One declared link, resolved against the pre-T world and scored.
#[derive(Debug, Clone, Serialize)]
pub struct EvalTarget {
    /// The declared link text: `state:<32-hex>` or `evidence:<32-hex>`.
    pub link: String,
    /// State records the link can reach at T: the `state:` target itself,
    /// or every pre-T record citing the `evidence:` target (empty when the
    /// evidence exists but nothing in the world cites it).
    pub resolved_to: Vec<MemoryId>,
    /// The best lane that surfaced a resolved record, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// In-degree of the surfaced record (link/tag frames only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_degree: Option<usize>,
    pub hit: bool,
}

/// One evaluated record: its replayed query, its declared targets, and
/// what the replayed recall surfaced.
#[derive(Debug, Clone, Serialize)]
pub struct RecallEvalRow {
    pub state_record_id: MemoryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_key: Option<String>,
    pub created_at_unix_ms: i64,
    pub query_source: QuerySource,
    pub query_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_evidence_id: Option<EvidenceId>,
    /// Declared links resolving to a record or evidence created before T —
    /// the ground truth. Dangling or post-T targets land in
    /// `unresolvable_links` instead (FIO's unresolved-declaration cases).
    pub targets: Vec<EvalTarget>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolvable_links: Vec<String>,
    /// `|targets ∩ surfaced| / |targets|` — the FIO recall for this
    /// record, counting every surfacing lane.
    pub hit_ratio: f64,
    /// The same ratio counting only lexical-seed and link-frame hits — the
    /// 宣言 column; targets only the tag frame surfaced do not count.
    pub declared_hit_ratio: f64,
    pub any_hit: bool,
    /// Everything the replayed recall offered, for transparency.
    pub surfaced: Vec<SurfacedRecord>,
}

/// Aggregate metrics over the evaluated rows.
#[derive(Debug, Clone, Serialize)]
pub struct RecallEvalSummary {
    /// Records evaluated: episodic, current, declaring at least one link.
    pub evaluated: usize,
    /// Mean `hit_ratio` — recall counting all surfacing lanes.
    pub mean_recall: f64,
    /// Mean `declared_hit_ratio` — the 宣言 column.
    pub declared_link_recall: f64,
    /// Extra recall the tag frame alone contributed
    /// (`mean_recall - declared_link_recall`) — the 自動 column.
    pub tag_frame_recall: f64,
    /// Share of rows where at least one declared target surfaced.
    pub any_hit_rate: f64,
    /// Declared targets hit through the link frame, across all rows.
    pub link_frame_hits: usize,
    /// Declared targets hit through the tag frame, across all rows.
    pub tag_frame_hits: usize,
    /// Link-frame hits whose in-degree reached [`KNOT_MIN_IN_DEGREE`].
    pub knot_hits: usize,
    /// Rows that fell back to the record's own text.
    pub no_prior_input_count: usize,
}

/// The full report `recall-eval` prints as JSON.
#[derive(Debug, Clone, Serialize)]
pub struct RecallEvalReport {
    pub individual_id: IndividualId,
    /// The retrieval parameters the evaluation ran under — the operative
    /// view in force, with `recall_hot_window_secs` forced to 0.
    pub retrieval: RetrievalParams,
    pub summary: RecallEvalSummary,
    /// One row per evaluated record, newest first.
    pub records: Vec<RecallEvalRow>,
    /// What the numbers cannot claim — see the module docs.
    pub limits: Vec<String>,
}

/// The record's own text for the fallback query: its `text` or `summary`
/// field, else the serialized payload — the same string the lexical scorer
/// would have scored it on.
fn own_text(record: &StateRecord) -> String {
    ["text", "summary"]
        .iter()
        .find_map(|key| record.payload.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| record.payload.to_string())
}

/// Lane preference when several surfaced citers could claim one
/// `evidence:` hit: lexical seed, then shallower link hops, then tag.
fn lane_rank(via: &str) -> usize {
    if via == "lexical_seed" {
        0
    } else if let Some(hop) = via.strip_prefix("link_hop_") {
        1 + hop.parse::<usize>().unwrap_or(usize::MAX)
    } else {
        usize::MAX
    }
}

/// Evaluate one link-declaring record against its pre-T world.
#[allow(clippy::too_many_arguments)]
fn evaluate_record(
    store: &SqliteStore,
    params: &RetrievalParams,
    individual_id: IndividualId,
    record: &StateRecord,
    episodic_pool: &[AttributedMemory],
    created_at_of: &BTreeMap<MemoryId, UtcTimestamp>,
    cites: &BTreeMap<EvidenceId, Vec<MemoryId>>,
    evidence_created: &BTreeMap<EvidenceId, UtcTimestamp>,
    utterances: &BTreeMap<String, Vec<EvidenceRecord>>,
) -> Result<RecallEvalRow, RuntimeError> {
    let t = record.created_at;

    // The replayed input: the newest user utterance on this subject's
    // channel before T. Without one the record's own text stands in and
    // the row is flagged — a reflection with no conversation behind it is
    // not the same recall situation.
    let prior = record.subject_key.as_ref().and_then(|subject| {
        utterances
            .get(&format!("text-chat:{subject}"))
            .and_then(|list| {
                list.iter().rev().find(|e| {
                    e.created_at < t && e.payload.get("text").is_some_and(Value::is_string)
                })
            })
    });
    let (query_text, query_evidence_id, query_source) = match prior {
        Some(evidence) => (
            evidence
                .payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            Some(evidence.evidence_id),
            QuerySource::PriorUserUtterance,
        ),
        None => (own_text(record), None, QuerySource::RecordTextFallback),
    };

    // The world at T, under the same gates the turn path applies: same
    // individual and subject, created before T, inside the declared
    // validity window, and current as of T. A record superseded only by a
    // post-T record still counted as current then; an invalidated one
    // carries no timestamp and stays excluded.
    let current_at_t = |r: &StateRecord| {
        r.lifecycle_state.is_current()
            || r.superseded_by_state_record_id
                .and_then(|id| created_at_of.get(&id))
                .is_some_and(|at| *at >= t)
    };
    let in_world = |r: &StateRecord| {
        r.individual_id == individual_id
            && r.subject_key == record.subject_key
            && r.created_at < t
            && r.is_valid_at(t)
            && current_at_t(r)
    };
    let pool: Vec<AttributedMemory> = episodic_pool
        .iter()
        .filter(|memory| in_world(&memory.record))
        .cloned()
        .collect();

    // Surfaced = the offer set the turn would have sent its judge: the
    // lexical seeds plus the declared link/tag frames. The seed set is the
    // exact one `select_declared` derives internally for the same pool,
    // query and parameters.
    let mut surfaced: BTreeMap<MemoryId, (String, usize)> =
        c0::rank_episodic(pool.clone(), &query_text, params.min_score)
            .into_iter()
            .take(params.episodic_top_k)
            .map(|(_, memory)| {
                (
                    memory.record.state_record_id,
                    ("lexical_seed".to_owned(), 0usize),
                )
            })
            .collect();
    let selection = select_declared(
        store,
        store,
        &pool,
        |r: &StateRecord| in_world(r),
        &query_text,
        params,
        t,
    );
    for hit in selection.link.iter().chain(&selection.tag) {
        surfaced.insert(
            hit.record.record.state_record_id,
            (hit.via.clone(), hit.in_degree),
        );
    }

    // Ground truth: the links R declared at T, restricted to targets that
    // existed at T. A `state:` link resolves to the named record; an
    // `evidence:` link resolves to every record citing it and counts as a
    // hit when any of them surfaced.
    let mut targets = Vec::new();
    let mut unresolvable_links = Vec::new();
    for link in record.links() {
        let (resolvable, resolved_to): (bool, Vec<MemoryId>) = match link {
            MemoryLink::State(target) => match store.get_state_record(target)? {
                Some(found) if found.created_at < t => (true, vec![target]),
                _ => (false, Vec::new()),
            },
            MemoryLink::Evidence(evidence_id) => {
                let before_t = match evidence_created.get(&evidence_id) {
                    Some(at) => *at < t,
                    None => EvidenceStore::get(store, evidence_id)?
                        .is_some_and(|found| found.created_at < t),
                };
                let citers = if before_t {
                    cites
                        .get(&evidence_id)
                        .map(|ids| {
                            ids.iter()
                                .copied()
                                .filter(|id| created_at_of.get(id).is_some_and(|at| *at < t))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                (before_t, citers)
            }
        };
        if !resolvable {
            unresolvable_links.push(link.as_str());
            continue;
        }
        let offered = resolved_to
            .iter()
            .filter_map(|id| surfaced.get(id))
            .min_by_key(|(via, _)| lane_rank(via));
        targets.push(EvalTarget {
            link: link.as_str(),
            resolved_to,
            hit: offered.is_some(),
            via: offered.map(|(via, _)| via.clone()),
            in_degree: offered.map(|(_, degree)| *degree),
        });
    }

    let hits = targets.iter().filter(|target| target.hit).count();
    let declared_hits = targets
        .iter()
        .filter(|target| target.hit && target.via.as_deref() != Some("tag"))
        .count();
    let denominator = targets.len().max(1) as f64;
    Ok(RecallEvalRow {
        state_record_id: record.state_record_id,
        subject_key: record.subject_key.clone(),
        created_at_unix_ms: t.unix_millis(),
        query_source,
        query_text,
        query_evidence_id,
        targets,
        unresolvable_links,
        hit_ratio: hits as f64 / denominator,
        declared_hit_ratio: declared_hits as f64 / denominator,
        any_hit: hits > 0,
        surfaced: surfaced
            .into_iter()
            .map(|(state_record_id, (via, _))| SurfacedRecord {
                state_record_id,
                via,
            })
            .collect(),
    })
}

fn summarize(rows: &[RecallEvalRow]) -> RecallEvalSummary {
    let evaluated = rows.len();
    let mut summary = RecallEvalSummary {
        evaluated,
        mean_recall: 0.0,
        declared_link_recall: 0.0,
        tag_frame_recall: 0.0,
        any_hit_rate: 0.0,
        link_frame_hits: 0,
        tag_frame_hits: 0,
        knot_hits: 0,
        no_prior_input_count: 0,
    };
    if evaluated == 0 {
        return summary;
    }
    for row in rows {
        summary.mean_recall += row.hit_ratio;
        summary.declared_link_recall += row.declared_hit_ratio;
        summary.any_hit_rate += if row.any_hit { 1.0 } else { 0.0 };
        summary.no_prior_input_count +=
            usize::from(row.query_source == QuerySource::RecordTextFallback);
        for target in &row.targets {
            if !target.hit {
                continue;
            }
            match target.via.as_deref() {
                Some("tag") => summary.tag_frame_hits += 1,
                Some(via) if via.starts_with("link_hop_") => {
                    summary.link_frame_hits += 1;
                    if target.in_degree.unwrap_or(0) >= KNOT_MIN_IN_DEGREE {
                        summary.knot_hits += 1;
                    }
                }
                _ => {}
            }
        }
    }
    let n = evaluated as f64;
    summary.mean_recall /= n;
    summary.declared_link_recall /= n;
    summary.tag_frame_recall = summary.mean_recall - summary.declared_link_recall;
    summary.any_hit_rate /= n;
    summary
}

/// Run the evaluation over the opened runtime. Read-only: every access is
/// a `SELECT` through the store traits, and no writer epoch is claimed.
///
/// `limit` caps the evaluated records to the newest N link-declaring
/// episodic records.
pub fn evaluate(runtime: &Runtime, limit: Option<usize>) -> Result<RecallEvalReport, RuntimeError> {
    let store = runtime.store();
    let individual_id = runtime.individual_id();
    // The parameters a turn would run under today — the operative view —
    // minus the hot window, whose inputs a replay cannot reconstruct.
    let mut params = c0::operative(runtime)
        .map(|operative| operative.view.params.retrieval)
        .unwrap_or_default();
    params.recall_hot_window_secs = 0;

    // The full record set, history included: evaluation targets must be
    // current, but the world at T may contain records since superseded.
    let fetch = |domain| -> Result<Vec<AttributedMemory>, RuntimeError> {
        Ok(MemoryRepository::retrieve(
            store,
            &MemoryQuery::current(individual_id)
                .in_domain(domain)
                .including_history(),
        )?)
    };
    let episodic = fetch(MutationDomain::Episodic)?;
    let relationship = fetch(MutationDomain::Relationship)?;

    // Shared indexes: record creation times (for the current-at-T rule)
    // and the evidence -> citing-records map `evidence:` links resolve
    // through.
    let mut created_at_of: BTreeMap<MemoryId, UtcTimestamp> = BTreeMap::new();
    let mut cites: BTreeMap<EvidenceId, Vec<MemoryId>> = BTreeMap::new();
    for memory in episodic.iter().chain(&relationship) {
        created_at_of.insert(memory.record.state_record_id, memory.record.created_at);
        for evidence_id in &memory.record.evidence_refs {
            cites
                .entry(*evidence_id)
                .or_default()
                .push(memory.record.state_record_id);
        }
    }

    // Evaluation targets: current episodic records that declared at least
    // one link, newest first so --limit keeps the most recent N.
    let mut evaluated: Vec<StateRecord> = episodic
        .iter()
        .filter(|memory| memory.is_current() && !memory.record.links().is_empty())
        .map(|memory| memory.record.clone())
        .collect();
    evaluated.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(b.state_record_id.cmp(&a.state_record_id))
    });
    if let Some(limit) = limit {
        evaluated.truncate(limit);
    }

    // Evidence index: existence times for `evidence:` link resolution and
    // the per-channel utterance lists the replayed queries come from.
    let evidence_pool = store.c0_evidence_pool(individual_id, EVIDENCE_SCAN_LIMIT)?;
    let mut evidence_created: BTreeMap<EvidenceId, UtcTimestamp> = BTreeMap::new();
    let mut utterances: BTreeMap<String, Vec<EvidenceRecord>> = BTreeMap::new();
    for record in evidence_pool {
        evidence_created.insert(record.evidence_id, record.created_at);
        if record.kind == EvidenceKind::UserUtterance
            && let Some(source_id) = record.source.source_id.clone()
        {
            utterances.entry(source_id).or_default().push(record);
        }
    }
    for records in utterances.values_mut() {
        records.sort_by_key(|record| (record.created_at, record.evidence_id));
    }

    let mut rows = Vec::with_capacity(evaluated.len());
    for record in &evaluated {
        rows.push(evaluate_record(
            store,
            &params,
            individual_id,
            record,
            &episodic,
            &created_at_of,
            &cites,
            &evidence_created,
            &utterances,
        )?);
    }

    Ok(RecallEvalReport {
        individual_id,
        retrieval: params,
        summary: summarize(&rows),
        records: rows,
        limits: LIMITS.iter().map(|line| line.to_string()).collect(),
    })
}
