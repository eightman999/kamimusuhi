//! Task-plane bookkeeping: the cost ledger that enforces quotas, and the
//! outcome history that evaluation and routing read.
//!
//! The ledger counts a task when it is admitted (so parallel delegations
//! cannot overshoot a task quota) and adds its tokens and cost when it
//! finishes. A cost that is neither reported nor estimable is counted as
//! `unpriced`, never as zero. Both files live in `current_state/`; the
//! history is also spooled to `logs/task-history/` for the NAS.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use kamimusuhi_runtime::agent_exec::Quota;
use kamimusuhi_runtime::agent_exec::history::{Feedback, HistoryRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::{atomic_write, utc_date};

/// Key for totals over every executor.
pub const TOTAL: &str = "_total";
const KEEP_DAYS: usize = 62;
const KEEP_MONTHS: usize = 24;
/// History lines read back (newest).
const HISTORY_WINDOW: usize = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub tasks: u64,
    pub usd: f64,
    pub tokens: u64,
    /// Tasks whose cost could be neither read nor estimated.
    pub unpriced: u64,
}

impl Usage {
    fn add(&mut self, other: &Self) {
        self.tasks += other.tasks;
        self.usd += other.usd;
        self.tokens += other.tokens;
        self.unpriced += other.unpriced;
    }
}

type Buckets = BTreeMap<String, BTreeMap<String, Usage>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    days: Buckets,
    #[serde(default)]
    months: Buckets,
}

/// Where a reservation was booked, so settling lands in the same period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub executor: String,
    pub day: String,
    pub month: String,
}

pub struct CostLedger {
    path: PathBuf,
    ledger: Mutex<Ledger>,
}

fn total(bucket: Option<&BTreeMap<String, Usage>>) -> Usage {
    let mut sum = Usage::default();
    for usage in bucket.into_iter().flat_map(BTreeMap::values) {
        sum.add(usage);
    }
    sum
}

fn check(scope: &str, quota: &Quota, day: &Usage, month: &Usage) -> Result<(), String> {
    let over = |what: &str, used: String, limit: String| {
        Err(format!("{scope} の上限に達している: {what} {used}/{limit}"))
    };
    if let Some(max) = quota.max_tasks_per_day
        && day.tasks >= max
    {
        return over("今日のタスク数", day.tasks.to_string(), max.to_string());
    }
    if let Some(max) = quota.max_tasks_per_month
        && month.tasks >= max
    {
        return over("今月のタスク数", month.tasks.to_string(), max.to_string());
    }
    if let Some(max) = quota.max_usd_per_day
        && day.usd >= max
    {
        return over("今日の費用", format!("${:.4}", day.usd), format!("${max}"));
    }
    if let Some(max) = quota.max_usd_per_month
        && month.usd >= max
    {
        return over(
            "今月の費用",
            format!("${:.4}", month.usd),
            format!("${max}"),
        );
    }
    if let Some(max) = quota.max_tokens_per_day
        && day.tokens >= max
    {
        return over("今日のトークン", day.tokens.to_string(), max.to_string());
    }
    Ok(())
}

impl CostLedger {
    pub fn load(path: PathBuf) -> Self {
        let ledger = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            ledger: Mutex::new(ledger),
        }
    }

    fn save(&self, ledger: &mut Ledger) {
        while ledger.days.len() > KEEP_DAYS {
            ledger.days.pop_first();
        }
        while ledger.months.len() > KEEP_MONTHS {
            ledger.months.pop_first();
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(ledger) {
            let _ = atomic_write(&self.path, &bytes);
        }
    }

    /// Admit one task for `executor` unless a quota is used up.
    pub fn reserve(
        &self,
        executor: &str,
        executor_quota: &Quota,
        total_quota: &Quota,
        now: u64,
    ) -> Result<Reservation, String> {
        let day = utc_date(now);
        let month = day[..7].to_owned();
        let mut guard = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let ledger = &mut *guard;
        let mine = |b: &Buckets, k: &str| {
            b.get(k)
                .and_then(|m| m.get(executor))
                .cloned()
                .unwrap_or_default()
        };
        check(
            executor,
            executor_quota,
            &mine(&ledger.days, &day),
            &mine(&ledger.months, &month),
        )?;
        check(
            "Task Plane 全体",
            total_quota,
            &total(ledger.days.get(&day)),
            &total(ledger.months.get(&month)),
        )?;
        for (bucket, key) in [(&mut ledger.days, &day), (&mut ledger.months, &month)] {
            bucket
                .entry(key.clone())
                .or_default()
                .entry(executor.to_owned())
                .or_default()
                .tasks += 1;
        }
        self.save(ledger);
        Ok(Reservation {
            executor: executor.to_owned(),
            day,
            month,
        })
    }

    /// Book what a finished task used. `refund` returns the reservation
    /// of a task that never started.
    pub fn settle(
        &self,
        reservation: &Reservation,
        usd: Option<f64>,
        tokens: Option<u64>,
        refund: bool,
    ) {
        let mut guard = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let ledger = &mut *guard;
        for (bucket, key) in [
            (&mut ledger.days, &reservation.day),
            (&mut ledger.months, &reservation.month),
        ] {
            let usage = bucket
                .entry(key.clone())
                .or_default()
                .entry(reservation.executor.clone())
                .or_default();
            if refund {
                usage.tasks = usage.tasks.saturating_sub(1);
                continue;
            }
            match usd {
                Some(v) if v.is_finite() && v >= 0.0 => usage.usd += v,
                _ => usage.unpriced += 1,
            }
            usage.tokens += tokens.unwrap_or(0);
        }
        self.save(ledger);
    }

    /// Today and this month per executor, with the configured quotas.
    pub fn view(&self, now: u64, quotas: &BTreeMap<String, Quota>) -> Value {
        let day = utc_date(now);
        let month = day[..7].to_owned();
        let ledger = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        json!({
            "day": day,
            "month": month,
            "today": ledger.days.get(&day).cloned().unwrap_or_default(),
            "this_month": ledger.months.get(&month).cloned().unwrap_or_default(),
            "quotas": quotas,
        })
    }
}

/// Append-only outcome history and feedback.
pub struct HistoryStore {
    history: PathBuf,
    feedback: PathBuf,
    lock: Mutex<()>,
}

fn append_line(path: &PathBuf, value: &impl Serialize) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut line) = serde_json::to_string(value) else {
        return;
    };
    line.push('\n');
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Vec<T> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(HISTORY_WINDOW)..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

impl HistoryStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            history: dir.join("task-history.jsonl"),
            feedback: dir.join("task-feedback.jsonl"),
            lock: Mutex::new(()),
        }
    }

    pub fn record(&self, record: &HistoryRecord) {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        append_line(&self.history, record);
    }

    pub fn feedback(&self, feedback: &Feedback) {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        append_line(&self.feedback, feedback);
    }

    pub fn load(&self) -> (Vec<HistoryRecord>, Vec<Feedback>) {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        (read_lines(&self.history), read_lines(&self.feedback))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotas_block_at_the_limit_and_unknown_cost_is_not_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.json");
        let ledger = CostLedger::load(path.clone());
        let quota = Quota {
            max_tasks_per_day: Some(2),
            ..Quota::default()
        };
        let none = Quota::default();
        let a = ledger
            .reserve("devin", &quota, &none, 86_400)
            .expect("first");
        let b = ledger
            .reserve("devin", &quota, &none, 86_400)
            .expect("second");
        let err = ledger
            .reserve("devin", &quota, &none, 86_400)
            .expect_err("third");
        assert!(err.contains("2/2"), "{err}");
        // Another executor is not limited by devin's quota…
        ledger
            .reserve("opencode", &quota, &none, 86_400)
            .expect("other");
        // …but the node-wide one covers everyone.
        let total_quota = Quota {
            max_tasks_per_day: Some(3),
            ..Quota::default()
        };
        assert!(ledger.reserve("cc", &none, &total_quota, 86_400).is_err());

        ledger.settle(&a, None, Some(100), false);
        ledger.settle(&b, Some(0.25), Some(50), false);
        let reloaded = CostLedger::load(path);
        let view = reloaded.view(86_400, &BTreeMap::new());
        assert_eq!(view["today"]["devin"]["tasks"], 2);
        assert_eq!(view["today"]["devin"]["unpriced"], 1);
        assert_eq!(view["today"]["devin"]["usd"], 0.25);
        assert_eq!(view["today"]["devin"]["tokens"], 150);

        let usd_quota = Quota {
            max_usd_per_day: Some(0.2),
            ..Quota::default()
        };
        assert!(
            reloaded
                .reserve("devin", &usd_quota, &none, 86_400)
                .is_err()
        );
        // A refund returns a task that never started.
        let r = reloaded
            .reserve("opencode", &none, &none, 86_400)
            .expect("r");
        reloaded.settle(&r, None, None, true);
        assert_eq!(
            reloaded.view(86_400, &BTreeMap::new())["today"]["opencode"]["tasks"],
            1
        );
    }
}
