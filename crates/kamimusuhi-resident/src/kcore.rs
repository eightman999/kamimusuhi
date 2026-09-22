//! Supervises the model-free K-CORE loop as a child process.
//!
//! K-CORE resumes the existing individual from its runtime directory and
//! never initializes one; if the directory is missing or corrupt it fails
//! closed and this supervisor only reports that, with backoff. Tick output
//! (JSONL on stdout) is journaled under `logs/kcore/<node>/`.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::KCoreConfig;
use crate::state::Shared;
use crate::util::{iso8601, unix_now};

const MAX_BACKOFF: Duration = Duration::from_secs(300);

fn set(shared: &Shared, value: Value) {
    *shared.kcore.write().unwrap_or_else(|p| p.into_inner()) = value;
}

pub fn spawn(shared: &Arc<Shared>, config: KCoreConfig) {
    let shared = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("kcore-supervisor".to_owned())
        .spawn(move || supervise(&shared, &config));
}

fn supervise(shared: &Shared, config: &KCoreConfig) {
    let mut backoff = Duration::from_secs(5);
    let mut restarts: u64 = 0;
    loop {
        let initialized = config.dir.join("kamimusuhi.sqlite").is_file();
        if !initialized {
            set(
                shared,
                json!({
                    "state": "not_initialized",
                    "dir": config.dir,
                    "hint": "initialize once with kamimusuhi-runtime init (see deploy/README.md)",
                    "checked_at": iso8601(unix_now()),
                }),
            );
            thread::sleep(Duration::from_secs(60));
            continue;
        }
        let started = Instant::now();
        let child = Command::new(&config.binary)
            .arg("--dir")
            .arg(&config.dir)
            .arg("--interval-ms")
            .arg(config.interval_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                set(
                    shared,
                    json!({"state": "spawn_failed", "error": e.to_string(),
                           "binary": config.binary, "restarts": restarts}),
                );
                thread::sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        set(
            shared,
            json!({"state": "running", "pid": child.id(), "since": iso8601(unix_now()),
                   "restarts": restarts, "interval_ms": config.interval_ms}),
        );
        let stderr_tail = child.stderr.take().map(|stderr| {
            thread::spawn(move || {
                let mut tail: Vec<String> = Vec::new();
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    tail.push(line);
                    if tail.len() > 20 {
                        tail.remove(0);
                    }
                }
                tail
            })
        });
        let mut last_tick = Value::Null;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let record =
                    serde_json::from_str::<Value>(&line).unwrap_or_else(|_| json!({"raw": line}));
                let _ = shared.spool.append("logs/kcore", json!({"tick": record}));
                last_tick = json!({"at": iso8601(unix_now())});
                let mut state = shared.kcore.write().unwrap_or_else(|p| p.into_inner());
                if let Value::Object(map) = &mut *state {
                    map.insert("last_output".into(), last_tick.clone());
                }
            }
        }
        let status = child.wait();
        let tail = stderr_tail.and_then(|h| h.join().ok()).unwrap_or_default();
        restarts += 1;
        let _ = shared.spool.append(
            "logs/kcore",
            json!({"event": "exited", "status": format!("{status:?}"), "stderr_tail": tail}),
        );
        if started.elapsed() > Duration::from_secs(600) {
            backoff = Duration::from_secs(5);
        }
        set(
            shared,
            json!({"state": "restarting", "exit": format!("{status:?}"), "restarts": restarts,
                   "backoff_secs": backoff.as_secs(), "stderr_tail": tail,
                   "last_output": last_tick}),
        );
        thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
