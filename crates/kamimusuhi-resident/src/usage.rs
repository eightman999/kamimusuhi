//! Privacy-safe local usage history and process-lifetime Prometheus metrics.
//!
//! History survives NAS delivery and restarts; counters deliberately start at
//! zero on restart and never decrease when history expires. No prompts, response
//! bodies, request IDs, credentials or upstream error messages are stored here.
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::RouteEvent;
use crate::util::unix_now;

const RETENTION_SECS: u64 = 30 * 24 * 60 * 60;
const MAX_RECORDS: usize = 100_000;
const MAX_MEMORY_RECORDS: usize = 1_000;
const MAX_SERIES: usize = 256;
const BUCKETS: [f64; 10] = [0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 120.0];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    pub at: u64,
    pub kind: String,
    pub tier: Option<String>,
    pub model: Option<String>,
    pub billing: Option<String>,
    pub outcome: String,
    pub ok: bool,
    pub latency_ms: u64,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_kind: Option<String>,
}

impl UsageRecord {
    fn from_event(event: &RouteEvent, kind: &str, outcome: &str) -> Self {
        let cost_kind = event
            .cost_kind
            .as_deref()
            .filter(|s| matches!(*s, "actual" | "estimate"));
        let cost_usd = event
            .cost_usd
            .filter(|v| v.is_finite() && *v >= 0.0 && cost_kind.is_some());
        Self {
            at: event.at,
            kind: kind.into(),
            tier: event.tier.as_deref().map(bounded_text),
            model: event.model.as_deref().map(bounded_text),
            billing: event
                .billing
                .as_deref()
                .filter(|s| matches!(*s, "local" | "subscription" | "free_tier" | "metered"))
                .map(str::to_owned),
            outcome: outcome.into(),
            ok: event.ok,
            latency_ms: event.latency_ms,
            prompt_tokens: event.prompt_tokens,
            completion_tokens: event.completion_tokens,
            cached_tokens: event.cached_tokens,
            cost_usd,
            cost_kind: cost_usd.and(cost_kind).map(str::to_owned),
        }
    }
}

fn bounded_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(160).collect()
}

#[derive(Clone, Default)]
struct Aggregate {
    count: u64,
    seconds: f64,
    buckets: [u64; 10],
    tokens: [u64; 3],
    unknown_tokens: [u64; 3],
    cost: [f64; 2],
    reported_costs: [u64; 2],
    unknown_cost: u64,
    last_at: u64,
}

type Labels = (String, String, String, String);

struct Inner {
    db: Option<Connection>,
    recent: VecDeque<UsageRecord>,
    series: BTreeMap<Labels, Aggregate>,
    persistence_errors: u64,
    persistence_healthy: bool,
}

pub struct UsageStore {
    inner: Mutex<Inner>,
    started_at: u64,
}

impl UsageStore {
    /// Failure to persist never breaks inference; health and error counters
    /// expose degradation and the most recent 1,000 observations stay in RAM.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let db = (|| -> Result<Connection, Box<dyn std::error::Error>> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let db = Connection::open(path)?;
            db.busy_timeout(std::time::Duration::from_millis(250))?;
            db.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
                CREATE TABLE IF NOT EXISTS usage_events (
                    id INTEGER PRIMARY KEY, at INTEGER NOT NULL, record TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS usage_events_at ON usage_events(at);",
            )?;
            prune(&db, unix_now(), MAX_RECORDS)?;
            Ok(db)
        })()
        .ok();
        let healthy = db.is_some();
        Self {
            inner: Mutex::new(Inner {
                db,
                recent: VecDeque::new(),
                series: BTreeMap::new(),
                persistence_errors: u64::from(!healthy),
                persistence_healthy: healthy,
            }),
            started_at: unix_now(),
        }
    }

    pub fn record_request(&self, event: &RouteEvent, invalid: bool) {
        self.record(UsageRecord::from_event(
            event,
            "request",
            if invalid {
                "invalid"
            } else if event.ok {
                "success"
            } else {
                "error"
            },
        ));
    }

    /// One attempted tier invocation, excluding cost-guard policy rejections.
    pub fn record_attempt(&self, event: &RouteEvent) {
        self.record(UsageRecord::from_event(
            event,
            "attempt",
            if event.ok { "success" } else { "error" },
        ));
    }

    fn record(&self, record: UsageRecord) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut labels = (
            record.kind.clone(),
            record.tier.clone().unwrap_or_else(|| "unknown".into()),
            record.model.clone().unwrap_or_else(|| "unknown".into()),
            record.outcome.clone(),
        );
        // Entire label tuples are capped. A caller may select arbitrary model
        // aliases; these remain in bounded history but cannot explode metrics.
        if !inner.series.contains_key(&labels) && inner.series.len() >= MAX_SERIES {
            labels.1 = "other".into();
            labels.2 = "other".into();
        }
        let aggregate = inner.series.entry(labels).or_default();
        aggregate.count = aggregate.count.saturating_add(1);
        let seconds = record.latency_ms as f64 / 1000.0;
        aggregate.seconds += seconds;
        for (i, bound) in BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                aggregate.buckets[i] = aggregate.buckets[i].saturating_add(1);
            }
        }
        for (i, tokens) in [
            record.prompt_tokens,
            record.completion_tokens,
            record.cached_tokens,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(tokens) = tokens {
                aggregate.tokens[i] = aggregate.tokens[i].saturating_add(tokens);
            } else {
                aggregate.unknown_tokens[i] = aggregate.unknown_tokens[i].saturating_add(1);
            }
        }
        match (record.cost_usd, record.cost_kind.as_deref()) {
            (Some(cost), Some("actual")) => {
                aggregate.cost[0] += cost;
                aggregate.reported_costs[0] = aggregate.reported_costs[0].saturating_add(1);
            }
            (Some(cost), Some("estimate")) => {
                aggregate.cost[1] += cost;
                aggregate.reported_costs[1] = aggregate.reported_costs[1].saturating_add(1);
            }
            _ => aggregate.unknown_cost = aggregate.unknown_cost.saturating_add(1),
        }
        aggregate.last_at = aggregate.last_at.max(record.at);
        let result = inner.db.as_ref().map(|db| -> rusqlite::Result<()> {
            let body = serde_json::to_string(&record).expect("finite usage record");
            db.execute(
                "INSERT INTO usage_events(at, record) VALUES (?1, ?2)",
                params![sql_timestamp(record.at), body],
            )?;
            prune(db, unix_now(), MAX_RECORDS)
        });
        match result {
            Some(Ok(())) => inner.persistence_healthy = true,
            _ => {
                inner.persistence_errors = inner.persistence_errors.saturating_add(1);
                inner.persistence_healthy = false;
            }
        }
        inner.recent.push_front(record);
        inner.recent.truncate(MAX_MEMORY_RECORDS);
    }

    /// Newest-first history; since is inclusive Unix seconds. At most 1,000
    /// records are returned. RAM fallback is explicitly marked as incomplete.
    pub fn history(&self, limit: usize, since: Option<u64>) -> Value {
        let limit = limit.clamp(1, MAX_MEMORY_RECORDS);
        let since = since
            .unwrap_or(0)
            .max(unix_now().saturating_sub(RETENTION_SECS));
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let persisted = inner
            .db
            .as_ref()
            .map(|db| -> rusqlite::Result<Vec<UsageRecord>> {
                let mut query = db.prepare(
                    "SELECT record FROM usage_events WHERE at >= ?1 ORDER BY id DESC LIMIT ?2",
                )?;
                query
                    .query_map(params![sql_timestamp(since), limit as i64], |row| {
                        let body: String = row.get(0)?;
                        serde_json::from_str(&body).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                    })?
                    .collect()
            });
        let (records, source) = match persisted {
            Some(Ok(records)) if inner.persistence_healthy => (records, "sqlite"),
            other => {
                if matches!(other, Some(Err(_))) {
                    inner.persistence_errors = inner.persistence_errors.saturating_add(1);
                    inner.persistence_healthy = false;
                }
                (
                    inner
                        .recent
                        .iter()
                        .filter(|r| r.at >= since)
                        .take(limit)
                        .cloned()
                        .collect(),
                    "memory_fallback",
                )
            }
        };
        json!({"records": records, "source": source, "limit": limit,
            "since": since, "retention_days": 30, "max_records": MAX_RECORDS,
            "persistence_healthy": inner.persistence_healthy,
            "persistence_errors": inner.persistence_errors,
            "truncated": records.len() == limit})
    }

    pub fn snapshot(&self) -> Value {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let series: Vec<Value> = inner.series.iter().map(|((kind, tier, model, outcome), a)| json!({
            "kind": kind, "tier": tier, "model": model, "outcome": outcome,
            "count": a.count, "latency_seconds_sum": a.seconds,
            "prompt_tokens_known": a.tokens[0], "completion_tokens_known": a.tokens[1], "cached_tokens_known": a.tokens[2],
            "prompt_tokens_unknown": a.unknown_tokens[0], "completion_tokens_unknown": a.unknown_tokens[1], "cached_tokens_unknown": a.unknown_tokens[2],
            "actual_cost_usd_known": a.cost[0], "estimated_cost_usd_known": a.cost[1], "cost_unknown": a.unknown_cost,
            "actual_cost_reports": a.reported_costs[0], "estimated_cost_reports": a.reported_costs[1],
            "last_at": a.last_at,
        })).collect();
        json!({"counter_scope": "process", "started_at": self.started_at, "series": series,
            "retention_days": 30, "max_records": MAX_RECORDS,
            "persistence_healthy": inner.persistence_healthy, "persistence_errors": inner.persistence_errors})
    }

    pub fn prometheus(&self) -> String {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = String::new();
        for (name, kind, help) in [
            (
                "events_total",
                "counter",
                "Completed routed requests or attempted tier invocations since process start.",
            ),
            (
                "latency_seconds",
                "histogram",
                "End to end request or individual attempt latency.",
            ),
            (
                "tokens_total",
                "counter",
                "Known reported tokens; consult tokens_unknown_total for missing reports.",
            ),
            (
                "tokens_unknown_total",
                "counter",
                "Observations without a reported token count.",
            ),
            (
                "cost_usd_total",
                "counter",
                "Known costs separated by actual and estimate; do not sum both kinds of event.",
            ),
            (
                "cost_reports_total",
                "counter",
                "Observations with a known actual or estimated cost, including explicitly reported zero.",
            ),
            (
                "cost_unknown_total",
                "counter",
                "Observations without a known cost.",
            ),
            (
                "last_request_timestamp_seconds",
                "gauge",
                "Latest observation Unix timestamp by event kind and model.",
            ),
            (
                "persistence_errors_total",
                "counter",
                "Local usage history persistence failures since process start.",
            ),
            (
                "persistence_healthy",
                "gauge",
                "Whether the local usage history is being persisted successfully.",
            ),
        ] {
            let _ = writeln!(
                out,
                "# HELP kamimusuhi_usage_{name} {help}\n# TYPE kamimusuhi_usage_{name} {kind}"
            );
        }
        let _ = writeln!(
            out,
            "kamimusuhi_usage_persistence_errors_total {}\nkamimusuhi_usage_persistence_healthy {}",
            inner.persistence_errors,
            u8::from(inner.persistence_healthy)
        );
        for ((kind, tier, model, outcome), a) in &inner.series {
            let labels = format!(
                "kind=\"{}\",tier=\"{}\",model=\"{}\",outcome=\"{}\"",
                escape(kind),
                escape(tier),
                escape(model),
                escape(outcome)
            );
            let mut metric = |name: &str, extra: &str, value: String| {
                let _ = writeln!(out, "kamimusuhi_usage_{name}{{{labels}{extra}}} {value}");
            };
            metric("events_total", "", a.count.to_string());
            metric("latency_seconds_sum", "", a.seconds.to_string());
            metric("latency_seconds_count", "", a.count.to_string());
            for (i, bound) in BUCKETS.iter().enumerate() {
                metric(
                    "latency_seconds_bucket",
                    &format!(",le=\"{bound}\""),
                    a.buckets[i].to_string(),
                );
            }
            metric(
                "latency_seconds_bucket",
                ",le=\"+Inf\"",
                a.count.to_string(),
            );
            for (i, name) in ["prompt", "completion", "cached"].iter().enumerate() {
                let label = format!(",token_type=\"{name}\"");
                metric("tokens_total", &label, a.tokens[i].to_string());
                metric(
                    "tokens_unknown_total",
                    &label,
                    a.unknown_tokens[i].to_string(),
                );
            }
            for (i, name) in ["actual", "estimate"].iter().enumerate() {
                metric(
                    "cost_usd_total",
                    &format!(",cost_kind=\"{name}\""),
                    a.cost[i].to_string(),
                );
                metric(
                    "cost_reports_total",
                    &format!(",cost_kind=\"{name}\""),
                    a.reported_costs[i].to_string(),
                );
            }
            metric("cost_unknown_total", "", a.unknown_cost.to_string());
            metric("last_request_timestamp_seconds", "", a.last_at.to_string());
        }
        out
    }
}

fn sql_timestamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn prune(db: &Connection, now: u64, max_records: usize) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM usage_events WHERE at < ?1",
        [sql_timestamp(now.saturating_sub(RETENTION_SECS))],
    )?;
    db.execute("DELETE FROM usage_events WHERE id <= (SELECT id FROM usage_events ORDER BY id DESC LIMIT 1 OFFSET ?1)", [max_records as i64])?;
    Ok(())
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> RouteEvent {
        RouteEvent {
            at: unix_now(),
            tier: Some("local".into()),
            model: Some("tiny".into()),
            ok: true,
            latency_ms: 100,
            attempts: vec!["secret raw failure must not be persisted".into()],
            ..Default::default()
        }
    }

    #[test]
    fn usage_history_survives_restart_without_replaying_process_counters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        {
            let store = UsageStore::open(&path);
            store.record_request(&event(), false);
            assert_eq!(store.history(10, None)["source"], "sqlite");
        }
        let store = UsageStore::open(&path);
        let history = store.history(10, None);
        assert_eq!(history["records"].as_array().unwrap().len(), 1);
        assert!(history["records"][0]["prompt_tokens"].is_null());
        assert!(history["records"][0]["cost_usd"].is_null());
        assert!(!history.to_string().contains("secret"));
        assert_eq!(store.snapshot()["series"], json!([]));
    }

    #[test]
    fn usage_separates_requests_attempts_cost_basis_and_unknown_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite"));
        let mut failed = event();
        failed.ok = false;
        store.record_attempt(&failed);
        let mut success = event();
        success.prompt_tokens = Some(20);
        success.completion_tokens = Some(0);
        success.cost_usd = Some(0.25);
        success.cost_kind = Some("actual".into());
        store.record_attempt(&success);
        store.record_request(&success, false);
        let series = store.snapshot()["series"].as_array().unwrap().clone();
        assert_eq!(series.len(), 3);
        let request = series.iter().find(|s| s["kind"] == "request").unwrap();
        assert_eq!(request["count"], 1);
        assert_eq!(request["actual_cost_usd_known"], 0.25);
        assert_eq!(request["estimated_cost_usd_known"], 0.0);
        assert_eq!(request["completion_tokens_unknown"], 0);
        assert_eq!(request["cached_tokens_unknown"], 1);
        let metrics = store.prometheus();
        assert!(metrics.contains("outcome=\"success\",le=\"0.1\"} 1"));
        assert!(metrics.contains("outcome=\"error\",token_type=\"prompt\"} 1"));
    }

    #[test]
    fn usage_distinguishes_reported_zero_cost_from_missing_cost() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite"));
        let mut e = event();
        store.record_request(&e, false);
        let before = store.snapshot();
        assert_eq!(before["series"][0]["actual_cost_reports"], 0);
        e.cost_usd = Some(0.0);
        e.cost_kind = Some("actual".into());
        store.record_request(&e, false);
        let after = store.snapshot();
        assert_eq!(after["series"][0]["actual_cost_usd_known"], 0.0);
        assert_eq!(after["series"][0]["actual_cost_reports"], 1);
        assert_eq!(after["series"][0]["estimated_cost_reports"], 0);
        assert_eq!(after["series"][0]["cost_unknown"], 1);
        assert!(store.prometheus().contains("kamimusuhi_usage_cost_reports_total{kind=\"request\",tier=\"local\",model=\"tiny\",outcome=\"success\",cost_kind=\"actual\"} 1"));
    }

    #[test]
    fn usage_metrics_escape_labels_and_cap_series_cardinality() {
        assert_eq!(escape("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite"));
        for i in 0..MAX_SERIES + 20 {
            let mut e = event();
            e.model = Some(format!("model-{i}\\\""));
            store.record_request(&e, false);
        }
        assert_eq!(
            store.snapshot()["series"].as_array().unwrap().len(),
            MAX_SERIES + 1
        );
        assert!(store.prometheus().contains("model=\"model-0\\\\\\\"\""));
        assert!(
            store
                .prometheus()
                .contains("tier=\"other\",model=\"other\"")
        );
    }

    #[test]
    fn usage_persistence_failure_is_observable_and_keeps_memory_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path()); // a directory cannot be a SQLite file
        store.record_request(&event(), false);
        assert_eq!(store.history(5, None)["source"], "memory_fallback");
        assert_eq!(
            store.history(5, None)["records"].as_array().unwrap().len(),
            1
        );
        assert_eq!(store.snapshot()["persistence_healthy"], false);
        assert_eq!(store.snapshot()["persistence_errors"], 2);
        assert!(
            store
                .prometheus()
                .contains("kamimusuhi_usage_persistence_healthy 0")
        );
    }

    #[test]
    fn usage_retention_prunes_age_and_count_without_decreasing_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite"));
        let mut old = event();
        old.at = unix_now() - RETENTION_SECS - 1;
        store.record_request(&old, false);
        for _ in 0..5 {
            store.record_request(&event(), false);
        }
        {
            let inner = store.inner.lock().unwrap();
            prune(inner.db.as_ref().unwrap(), unix_now(), 2).unwrap();
        }
        assert_eq!(
            store.history(20, None)["records"].as_array().unwrap().len(),
            2
        );
        assert_eq!(store.snapshot()["series"][0]["count"], 6);
    }
}
