//! Scheduled jobs: operator-configured commands run at a fixed interval
//! (e.g. `git pull` of a mirrored repository). Each job has a deadline, its
//! outcome is journaled under `logs/jobs/`, and a failing job never affects
//! anything else in the resident.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::Shared;
use crate::util::{iso8601, run_with_timeout, unix_now};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub interval_secs: u64,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Run once right after the resident starts.
    #[serde(default = "default_true")]
    pub run_at_start: bool,
}

const fn default_timeout() -> u64 {
    300
}

const fn default_true() -> bool {
    true
}

pub type JobStatus = Arc<RwLock<BTreeMap<String, Value>>>;

fn tail(text: &str, lines: usize) -> Vec<String> {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|l| (*l).to_owned())
        .collect()
}

pub fn spawn_all(shared: &Arc<Shared>) {
    for job in shared.config.jobs.clone() {
        let shared = Arc::clone(shared);
        let _ = thread::Builder::new()
            .name(format!("job-{}", job.name))
            .spawn(move || {
                if !job.run_at_start {
                    thread::sleep(Duration::from_secs(job.interval_secs));
                }
                let mut runs: u64 = 0;
                let task_id = shared
                    .tasks
                    .find_by_detail("job", "job", &job.name)
                    .unwrap_or_else(|| {
                        shared.tasks.create(
                            &shared.spool,
                            crate::tasks::NewTask {
                                title: &format!("定期ジョブ: {}", job.name),
                                kind: "job",
                                node: &shared.config.node.id,
                                owner: "system",
                                status: crate::tasks::TaskStatus::Waiting,
                                depends_on: Vec::new(),
                                detail: json!({"job": job.name, "interval_secs": job.interval_secs}),
                            },
                        )
                    });
                loop {
                    let started = Instant::now();
                    shared.tasks.update(
                        &shared.spool,
                        &task_id,
                        Some(crate::tasks::TaskStatus::InProgress),
                        None,
                        None,
                    );
                    let args: Vec<&str> = job.args.iter().map(String::as_str).collect();
                    let outcome = run_with_timeout(
                        &job.command,
                        &args,
                        Duration::from_secs(job.timeout_secs.max(1)),
                    );
                    runs += 1;
                    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);
                    let record = match &outcome {
                        Some(out) => json!({
                            "job": job.name, "ok": out.success, "latency_ms": latency_ms,
                            "stdout_tail": tail(&out.stdout, 5), "stderr_tail": tail(&out.stderr, 5),
                        }),
                        None => json!({"job": job.name, "ok": false, "latency_ms": latency_ms,
                                       "error": "timed out or could not start"}),
                    };
                    let _ = shared.spool.append("logs/jobs", record.clone());
                    let ok = record["ok"].as_bool().unwrap_or(false);
                    let line = record["stdout_tail"]
                        .as_array()
                        .and_then(|l| l.last())
                        .or_else(|| record["stderr_tail"].as_array().and_then(|l| l.last()))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    shared.tasks.update(
                        &shared.spool,
                        &task_id,
                        // Healthy jobs wait for their next run; failures stay visible.
                        Some(if ok {
                            crate::tasks::TaskStatus::Waiting
                        } else {
                            crate::tasks::TaskStatus::OnHold
                        }),
                        (!ok || runs <= 1).then_some(("system", line.as_str())),
                        Some(json!({"last_run": iso8601(unix_now()), "ok": ok})),
                    );
                    let mut status = record;
                    status["last_run"] = json!(iso8601(unix_now()));
                    status["runs"] = json!(runs);
                    status["interval_secs"] = json!(job.interval_secs);
                    shared
                        .jobs
                        .write()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(job.name.clone(), status);
                    thread::sleep(
                        Duration::from_secs(job.interval_secs.max(60))
                            .saturating_sub(started.elapsed()),
                    );
                }
            });
    }
}
