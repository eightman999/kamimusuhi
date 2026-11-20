//! Resident-mode client worker: talks to the always-on Kamimusuhi individual
//! through its resident HTTP API. The UI thread never blocks on the network:
//! a turn runs on one thread, decisions on short-lived threads and
//! status/approval polling on a background refresher.
//!
//! Shared by the desktop (egui) and terminal (ratatui) frontends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::client;

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Explicit resident URL; `None` uses `$KAMIMUSUHI_URL` or
    /// `~/.config/kamimusuhi/nodes`.
    pub url: Option<String>,
    pub subject: String,
}

pub enum RemoteCommand {
    Send {
        text: String,
    },
    Decide {
        id: String,
        approve: bool,
    },
    /// `POST /v1/tasks` body (create / update).
    Task(Value),
    /// `POST /v1/agents` body (task plane: cancel / feedback / continue /
    /// refresh). The reply arrives as [`RemoteEvent::AgentsReply`].
    Agents(Value),
    /// Switch active dialogue/history/memory subject.
    SwitchSubject(String),
    /// Immediately re-fetch memory report and self_notes.
    FetchMemory,
    Refresh,
    Shutdown,
}

#[derive(Debug)]
pub enum RemoteEvent {
    Connected {
        url: String,
        history: Vec<Value>,
    },
    History {
        subject: String,
        subjects: Vec<String>,
        turns: Vec<Value>,
    },
    Memory {
        memory: Option<Value>,
        self_notes: Option<Value>,
    },
    Reply(Value),
    Status(Value),
    Approvals(Vec<Value>),
    Tools(Value),
    Tasks(Vec<Value>),
    /// Task-plane panel document (`{"action":"panel"}`); `Value::Null` when
    /// the node has no task plane (the endpoint answered 404).
    Agents(Value),
    /// Reply to a [`RemoteCommand::Agents`] action.
    AgentsReply(Value),
    Decision {
        id: String,
        reply: Value,
    },
    Error(String),
}

pub fn spawn(config: RemoteConfig) -> (Sender<RemoteCommand>, Receiver<RemoteEvent>) {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::Builder::new()
        .name("kamimusuhi-remote".to_owned())
        .spawn(move || run(config, command_rx, event_tx))
        .expect("remote worker thread must start");
    (command_tx, event_rx)
}

fn refresh(url: &str, token: Option<&str>, events: &Sender<RemoteEvent>) -> bool {
    match crate::status::fetch(url, token) {
        Ok(status) => {
            if events.send(RemoteEvent::Status(status)).is_err() {
                return false;
            }
        }
        Err(e) => {
            let _ = events.send(RemoteEvent::Error(format!("状態を取得できません: {e}")));
        }
    }
    if let Ok(tasks) = client::tasks(url, token)
        && events.send(RemoteEvent::Tasks(tasks)).is_err()
    {
        return false;
    }
    // Task plane: a 404 means this node has none; other failures are
    // transient and keep the last panel (status errors already surface).
    let panel = match client::agents(url, token, &serde_json::json!({"action": "panel"})) {
        Ok(panel) => Some(panel),
        Err(e) if e.starts_with("HTTP 404") => Some(Value::Null),
        Err(_) => None,
    };
    if let Some(panel) = panel
        && events.send(RemoteEvent::Agents(panel)).is_err()
    {
        return false;
    }
    if let Ok(list) = client::approvals(url, token) {
        return events.send(RemoteEvent::Approvals(list)).is_ok();
    }
    true
}

fn refresh_memory(
    url: &str,
    token: Option<&str>,
    subject: &str,
    events: &Sender<RemoteEvent>,
) -> bool {
    let mem_subject = (!matches!(subject, "*" | "all")).then_some(subject);
    let memory = client::memory(url, token, mem_subject, 48).ok();
    let self_notes = client::self_notes(url, token, 20).ok();
    events
        .send(RemoteEvent::Memory { memory, self_notes })
        .is_ok()
}

fn current_subject(state: &Arc<Mutex<String>>) -> String {
    state.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn run(config: RemoteConfig, commands: Receiver<RemoteCommand>, events: Sender<RemoteEvent>) {
    let token = client::token();
    let url = match client::discover(&client::candidates(config.url.as_deref())) {
        Ok(url) => url,
        Err(e) => {
            let _ = events.send(RemoteEvent::Error(format!("常駐個体に接続できません: {e}")));
            return;
        }
    };
    let subject_state = Arc::new(Mutex::new(config.subject.clone()));
    let history_doc =
        client::history_full(&url, token.as_deref(), &config.subject, 60).unwrap_or(Value::Null);
    let history = history_doc["turns"].as_array().cloned().unwrap_or_default();
    let subjects: Vec<String> = history_doc["subjects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    if events
        .send(RemoteEvent::Connected {
            url: url.clone(),
            history: history.clone(),
        })
        .is_err()
    {
        return;
    }
    if events
        .send(RemoteEvent::History {
            subject: config.subject.clone(),
            subjects,
            turns: history,
        })
        .is_err()
    {
        return;
    }
    if let Ok(tools) = client::tools(&url, token.as_deref()) {
        let _ = events.send(RemoteEvent::Tools(tools));
    }

    // Initial memory/self_notes fetch off the main path so Connected/Status
    // are never delayed by a cold runtime report.
    {
        let (url, token, events, subject) = (
            url.clone(),
            token.clone(),
            events.clone(),
            config.subject.clone(),
        );
        let _ = thread::Builder::new()
            .name("kamimusuhi-memory-init".to_owned())
            .spawn(move || {
                let _ = refresh_memory(&url, token.as_deref(), &subject, &events);
            });
    }

    // Background refresher: status, tasks and approvals every 5 s; memory
    // every 4th tick (~20 s, matching the resident report cache TTL).
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (url, token, events, stop, subject_state) = (
            url.clone(),
            token.clone(),
            events.clone(),
            Arc::clone(&stop),
            Arc::clone(&subject_state),
        );
        let _ = thread::Builder::new()
            .name("kamimusuhi-refresh".to_owned())
            .spawn(move || {
                let mut tick = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    if !refresh(&url, token.as_deref(), &events) {
                        break;
                    }
                    tick = tick.wrapping_add(1);
                    if tick.is_multiple_of(4) {
                        let subj = current_subject(&subject_state);
                        if !refresh_memory(&url, token.as_deref(), &subj, &events) {
                            break;
                        }
                    }
                    for _ in 0..10 {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            });
    }

    // Turns run one at a time on their own thread so decisions and refreshes
    // are never queued behind a slow reply.
    let (turn_tx, turn_rx) = mpsc::channel::<String>();
    {
        let (url, token, events, subject_state) = (
            url.clone(),
            token.clone(),
            events.clone(),
            Arc::clone(&subject_state),
        );
        let _ = thread::Builder::new()
            .name("kamimusuhi-turn".to_owned())
            .spawn(move || {
                while let Ok(text) = turn_rx.recv() {
                    let subj = current_subject(&subject_state);
                    let event = match client::talk(&url, token.as_deref(), &subj, &text) {
                        Ok(reply) => RemoteEvent::Reply(reply),
                        Err(e) => RemoteEvent::Error(format!("応答に失敗しました: {e}")),
                    };
                    if events.send(event).is_err() {
                        break;
                    }
                    // A turn may have queued approvals: show them promptly.
                    if !refresh(&url, token.as_deref(), &events) {
                        break;
                    }
                }
            });
    }

    while let Ok(command) = commands.recv() {
        match command {
            RemoteCommand::Shutdown => break,
            RemoteCommand::Send { text } => {
                if turn_tx.send(text).is_err() {
                    break;
                }
            }
            RemoteCommand::SwitchSubject(new_subject) => {
                {
                    let mut guard = subject_state.lock().unwrap_or_else(|p| p.into_inner());
                    *guard = new_subject.clone();
                }
                let (url, token, events) = (url.clone(), token.clone(), events.clone());
                thread::spawn(move || {
                    match client::history_full(&url, token.as_deref(), &new_subject, 60) {
                        Ok(doc) => {
                            let turns = doc["turns"].as_array().cloned().unwrap_or_default();
                            let subjects = doc["subjects"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect();
                            let _ = events.send(RemoteEvent::History {
                                subject: new_subject.clone(),
                                subjects,
                                turns,
                            });
                        }
                        Err(e) => {
                            let _ = events
                                .send(RemoteEvent::Error(format!("履歴の切替に失敗しました: {e}")));
                        }
                    }
                    let _ = refresh_memory(&url, token.as_deref(), &new_subject, &events);
                });
            }
            RemoteCommand::FetchMemory => {
                let (url, token, events, subj) = (
                    url.clone(),
                    token.clone(),
                    events.clone(),
                    current_subject(&subject_state),
                );
                thread::spawn(move || {
                    let _ = refresh_memory(&url, token.as_deref(), &subj, &events);
                });
            }
            RemoteCommand::Refresh => {
                let (url, token, events, subj) = (
                    url.clone(),
                    token.clone(),
                    events.clone(),
                    current_subject(&subject_state),
                );
                thread::spawn(move || {
                    if let Ok(tools) = client::tools(&url, token.as_deref()) {
                        let _ = events.send(RemoteEvent::Tools(tools));
                    }
                    refresh(&url, token.as_deref(), &events);
                    let _ = refresh_memory(&url, token.as_deref(), &subj, &events);
                });
            }
            RemoteCommand::Task(body) => {
                let (url, token, events) = (url.clone(), token.clone(), events.clone());
                thread::spawn(move || {
                    if let Err(e) = client::task_action(&url, token.as_deref(), &body) {
                        let _ = events
                            .send(RemoteEvent::Error(format!("タスク操作に失敗しました: {e}")));
                    }
                    refresh(&url, token.as_deref(), &events);
                });
            }
            RemoteCommand::Agents(body) => {
                let (url, token, events) = (url.clone(), token.clone(), events.clone());
                thread::spawn(move || {
                    let event = match client::agents(&url, token.as_deref(), &body) {
                        Ok(reply) => RemoteEvent::AgentsReply(reply),
                        Err(e) => {
                            RemoteEvent::Error(format!("外部エージェント操作に失敗しました: {e}"))
                        }
                    };
                    let _ = events.send(event);
                    refresh(&url, token.as_deref(), &events);
                });
            }
            RemoteCommand::Decide { id, approve } => {
                let (url, token, events) = (url.clone(), token.clone(), events.clone());
                thread::spawn(move || {
                    let event = match client::decide(&url, token.as_deref(), &id, approve) {
                        Ok(reply) => RemoteEvent::Decision { id, reply },
                        Err(e) => RemoteEvent::Error(format!("承認操作に失敗しました: {e}")),
                    };
                    let _ = events.send(event);
                    refresh(&url, token.as_deref(), &events);
                });
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
}
