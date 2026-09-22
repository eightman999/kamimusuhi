//! Resident mode worker: talks to the always-on Kamimusuhi individual on the
//! Pi through its resident HTTP API. The UI thread never blocks on the
//! network: a turn runs on one thread, decisions on short-lived threads and
//! status/approval polling on a background refresher.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use kamimusuhi_resident::client;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Explicit resident URL; `None` discovers (LAN, then Tailscale).
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
    Refresh,
    Shutdown,
}

#[derive(Debug)]
pub enum RemoteEvent {
    Connected { url: String, history: Vec<Value> },
    Reply(Value),
    Status(Value),
    Approvals(Vec<Value>),
    Tools(Value),
    Tasks(Vec<Value>),
    Decision { id: String, reply: Value },
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
    match kamimusuhi_resident::status::fetch(url, token) {
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
    if let Ok(list) = client::approvals(url, token) {
        return events.send(RemoteEvent::Approvals(list)).is_ok();
    }
    true
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
    let history = client::history(&url, token.as_deref(), &config.subject, 60).unwrap_or_default();
    if events
        .send(RemoteEvent::Connected {
            url: url.clone(),
            history,
        })
        .is_err()
    {
        return;
    }
    if let Ok(tools) = client::tools(&url, token.as_deref()) {
        let _ = events.send(RemoteEvent::Tools(tools));
    }

    // Background refresher: status, tasks and approvals every 5 s.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (url, token, events, stop) = (
            url.clone(),
            token.clone(),
            events.clone(),
            Arc::clone(&stop),
        );
        let _ = thread::Builder::new()
            .name("kamimusuhi-refresh".to_owned())
            .spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    if !refresh(&url, token.as_deref(), &events) {
                        break;
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
        let (url, token, events, subject) = (
            url.clone(),
            token.clone(),
            events.clone(),
            config.subject.clone(),
        );
        let _ = thread::Builder::new()
            .name("kamimusuhi-turn".to_owned())
            .spawn(move || {
                while let Ok(text) = turn_rx.recv() {
                    let event = match client::talk(&url, token.as_deref(), &subject, &text) {
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
            RemoteCommand::Refresh => {
                let (url, token, events) = (url.clone(), token.clone(), events.clone());
                thread::spawn(move || {
                    if let Ok(tools) = client::tools(&url, token.as_deref()) {
                        let _ = events.send(RemoteEvent::Tools(tools));
                    }
                    refresh(&url, token.as_deref(), &events);
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
