//! Sister-to-sister messaging ("intercom"): envelopes exchanged between the
//! individuals hosted on peer nodes.
//!
//! Each node may host its own individual (`dialogue` configured). Per
//! INV-006 these are *different* individuals — separate `individual_id`
//! and continuity root — who talk to each other as peers. The intercom is
//! the mail system between them: it carries envelopes, journals both ends
//! of every exchange under `conversations/intercom`, and hands each
//! message to the receiving individual as a normal dialogue turn on the
//! subject `sister-<from-node>`.
//!
//! Delivery:
//!
//! ```text
//! queue (peer_say / operator / auto-reply)
//!   → POST /v1/intercom on the peer (deduplicated accept, 32KiB cap)
//!   → on failure: relay mailbox POST /v1/send, if configured
//!   → retry with backoff until `max_attempts`, then dead-letter
//! ```
//!
//! A node that cannot be reached directly polls its relay mailbox on a
//! timer. Both paths land in the same `accept`, so redelivery is safe.
//!
//! Replying is the individual's own act — she calls `peer_say`. The
//! exception is a conversation the operator opened with an auto-reply
//! budget: then each turn's response is forwarded verbatim (a `[end]`
//! line closes it), `auto_left` travels with the envelopes so the budget
//! is shared by both ends, and `max_hops` bounds the unattended chain.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use kamimusuhi_resource_http::{Endpoint, Header, TrustAnchors, http};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{IntercomConfig, RelayConfig};
use crate::state::Shared;
use crate::util::{atomic_write, iso8601, unix_now, unix_now_ms, utc_date};

const JOURNAL: &str = "conversations/intercom";
const KEEP_SEEN: usize = 1024;
const KEEP_CONVERSATIONS: usize = 100;
const KEEP_OUTBOX_DONE: usize = 64;
const MAX_INBOUND: usize = 256;
const MAX_OUTBOX: usize = 512;
const MAX_BODY: usize = 32 * 1024;
/// Envelope ids / conversation ids are `m-`/`cv-` + node + ms + seq.
const MAX_ID_CHARS: usize = 128;
const RELAY_TIMEOUT: Duration = Duration::from_secs(30);

pub const TOOL_NAMES: &[&str] = &["peer_say", "peer_list"];

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A message in the conversation.
    #[default]
    Message,
    /// Final words: delivered as a turn, then the conversation is closed.
    Close,
}

impl Kind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Close => "close",
        }
    }
}

/// One message between sister individuals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Depth of the unattended auto-reply chain this message belongs to.
    /// Manual sends carry 0; each auto-forward increments it.
    #[serde(default)]
    pub hop: u64,
    #[serde(default)]
    pub kind: Kind,
    pub body: String,
    #[serde(default)]
    pub sent_at: u64,
    /// Auto-reply budget the sender's side still grants this conversation;
    /// the receiver clamps it to its own `auto_reply_turns`.
    #[serde(default)]
    pub auto_left: u64,
    /// True when this envelope was produced by an auto-reply.
    #[serde(default)]
    pub auto: bool,
    /// Internal bookkeeping: a failed turn requeued this envelope once.
    #[serde(default)]
    pub retried: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutState {
    #[default]
    Queued,
    Sent,
    /// Delivery stopped: permanent rejection or attempts exhausted. The
    /// journal keeps the record; the `retry` action requeues.
    Dead,
}

impl OutState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sent => "sent",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Outbound {
    envelope: Envelope,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    next_at: u64,
    #[serde(default)]
    state: OutState,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    sent_via: Option<String>,
    #[serde(default)]
    delivered_at: Option<u64>,
}

impl Outbound {
    fn view(&self) -> Value {
        json!({
            "id": self.envelope.id, "to": self.envelope.to,
            "conversation": self.envelope.conversation,
            "state": self.state.as_str(), "attempts": self.attempts,
            "next_at": iso8601(self.next_at), "last_error": self.last_error,
            "sent_via": self.sent_via,
            "delivered_at": self.delivered_at.map(iso8601),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Conversation {
    id: String,
    peer: String,
    /// `operator` | `mio` | `peer` — who opened the conversation here.
    opened_by: String,
    /// Auto-reply budget remaining on this side of the conversation.
    auto_left: u64,
    closed: bool,
    /// Hop of the latest auto-forward seen in this conversation.
    hop: u64,
    created_at: u64,
    updated_at: u64,
    last_in: Option<String>,
    last_out: Option<String>,
}

impl Conversation {
    fn view(&self) -> Value {
        json!({
            "id": self.id, "peer": self.peer, "opened_by": self.opened_by,
            "auto_left": self.auto_left, "closed": self.closed, "hop": self.hop,
            "created": iso8601(self.created_at), "updated": iso8601(self.updated_at),
            "last_in": self.last_in, "last_out": self.last_out,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DailyUse {
    /// UTC date this counter covers; a new day resets it.
    #[serde(default)]
    date: String,
    #[serde(default)]
    sent: u64,
}

/// The persisted view: `current_state/intercom.json`. Accepted-but-unread
/// inbound envelopes live here too, so a restart does not lose a message
/// that was already acknowledged to the sender.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    inbound: VecDeque<Envelope>,
    #[serde(default)]
    outbox: VecDeque<Outbound>,
    /// Recently accepted envelope ids (dedup across direct + relay paths).
    #[serde(default)]
    seen: VecDeque<String>,
    #[serde(default)]
    conversations: Vec<Conversation>,
    /// Per-peer sent counters, keyed by peer id, reset each UTC day.
    #[serde(default)]
    daily: BTreeMap<String, DailyUse>,
    /// Relay mailbox high-water mark already acknowledged.
    #[serde(default)]
    relay_cursor: u64,
}

pub struct Intercom {
    path: PathBuf,
    node: String,
    cfg: IntercomConfig,
    state: Mutex<State>,
    changed: Condvar,
    seq: AtomicU64,
    /// Last relay poll outcome, for `/status`.
    relay_status: Mutex<Value>,
}

impl Intercom {
    pub fn load(path: PathBuf, cfg: IntercomConfig, node: String) -> Self {
        let state: State = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            node,
            cfg,
            state: Mutex::new(state),
            changed: Condvar::new(),
            seq: AtomicU64::new(0),
            relay_status: Mutex::new(Value::Null),
        }
    }

    fn persist(&self, s: &mut State) {
        while s.seen.len() > KEEP_SEEN {
            s.seen.pop_front();
        }
        if s.conversations.len() > KEEP_CONVERSATIONS {
            // Closed conversations drop first, then the least recently
            // updated; open ones sort last.
            s.conversations.sort_by_key(|c| (!c.closed, c.updated_at));
            s.conversations
                .drain(..s.conversations.len() - KEEP_CONVERSATIONS);
        }
        // Finished outbox entries are audit history; keep a bounded tail
        // of them alongside everything still queued.
        let mut done = s
            .outbox
            .iter()
            .filter(|o| o.state != OutState::Queued)
            .count();
        let mut i = 0;
        while done > KEEP_OUTBOX_DONE && i < s.outbox.len() {
            if s.outbox[i].state != OutState::Queued {
                s.outbox.remove(i);
                done -= 1;
            } else {
                i += 1;
            }
        }
        while s.outbox.len() > MAX_OUTBOX {
            s.outbox.pop_front();
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&*s) {
            let _ = atomic_write(&self.path, &bytes);
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{}-{}-{seq}", self.node, unix_now_ms())
    }

    fn conv_index(s: &State, id: &str) -> Option<usize> {
        s.conversations.iter().position(|c| c.id == id)
    }

    fn sister_label(&self, peer: &str) -> String {
        self.cfg
            .sisters
            .get(peer)
            .map(|s| match &s.note {
                Some(note) => format!("{}（{note}）", s.label),
                None => s.label.clone(),
            })
            .unwrap_or_else(|| peer.to_owned())
    }

    /// Peer → this node (`POST /v1/intercom`). Validates, deduplicates,
    /// queues the turn; the inbound worker runs the dialogue turn itself.
    /// `accepted:false` answers are permanent — the sender dead-letters
    /// rather than retrying forever; transport failures stay retriable.
    pub fn accept(&self, shared: &Shared, env: Envelope) -> (u16, Value) {
        if !self.cfg.enabled {
            return (503, json!({"error": "intercom is disabled"}));
        }
        if shared.config.dialogue.is_none() {
            // This host serves no individual; retrying changes nothing.
            return (
                200,
                json!({"accepted": false, "reason": "node hosts no individual"}),
            );
        }
        if env.to != shared.config.node.id {
            return (
                400,
                json!({"error": "envelope is not addressed to this node"}),
            );
        }
        if !shared.config.peers.iter().any(|p| p.id == env.from) {
            return (400, json!({"error": "sender is not a configured peer"}));
        }
        if !valid_id(&env.id)
            || env.conversation.as_deref().is_some_and(|c| !valid_id(c))
            || env.in_reply_to.as_deref().is_some_and(|r| !valid_id(r))
        {
            return (400, json!({"error": "invalid message/conversation id"}));
        }
        if env.body.is_empty() || env.body.len() > MAX_BODY {
            return (400, json!({"error": "body must be 1..32KiB"}));
        }
        if env.auto && env.hop > self.cfg.max_hops {
            let _ = shared.spool.append_sync(
                JOURNAL,
                json!({"kind": "dropped", "reason": "hop limit",
                       "id": env.id, "from": env.from, "hop": env.hop}),
            );
            return (
                200,
                json!({"accepted": false, "reason": "hop limit reached"}),
            );
        }
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.seen.iter().any(|id| id == &env.id) {
            return (200, json!({"accepted": false, "dedup": true}));
        }
        if s.inbound.len() >= MAX_INBOUND {
            return (503, json!({"error": "inbound queue is full"}));
        }
        let now = unix_now();
        if let Some(conv_id) = env.conversation.clone() {
            if Self::conv_index(&s, &conv_id).is_none() {
                s.conversations.push(Conversation {
                    id: conv_id.clone(),
                    peer: env.from.clone(),
                    opened_by: "peer".to_owned(),
                    auto_left: 0,
                    closed: false,
                    hop: 0,
                    created_at: now,
                    updated_at: now,
                    last_in: None,
                    last_out: None,
                });
            }
            let index = Self::conv_index(&s, &conv_id).expect("present");
            let conv = &mut s.conversations[index];
            conv.updated_at = now;
            conv.hop = conv.hop.max(env.hop);
            conv.last_in = Some(env.id.clone());
            // The sender's grant moves the local budget; an envelope can
            // never lift it past this node's configured auto_reply_turns.
            // An auto-forward carries the authoritative chain counter, so
            // it replaces the local value — a manual message (an operator
            // `open`, a peer_say) only ever raises it.
            conv.auto_left = if env.auto {
                env.auto_left.min(self.cfg.auto_reply_turns)
            } else {
                conv.auto_left
                    .max(env.auto_left.min(self.cfg.auto_reply_turns))
            };
            if env.kind == Kind::Close {
                conv.closed = true;
            }
        }
        let (id, from, conversation, hop, kind, auto) = (
            env.id.clone(),
            env.from.clone(),
            env.conversation.clone(),
            env.hop,
            env.kind,
            env.auto,
        );
        s.seen.push_back(id.clone());
        s.inbound.push_back(env);
        self.persist(&mut s);
        drop(s);
        self.changed.notify_all();
        let _ = shared.spool.append_sync(
            JOURNAL,
            json!({"kind": "received", "id": id, "from": from,
                   "conversation": conversation, "hop": hop,
                   "message_kind": kind.as_str(), "auto": auto}),
        );
        (200, json!({"accepted": true, "id": id}))
    }

    /// Enqueue an envelope under the state lock. `auto` marks the send as
    /// part of the unattended auto-reply chain (hop increments, budget
    /// decrements); `by` is `operator` | `mio` | `auto`.
    #[allow(clippy::too_many_arguments)]
    fn queue_locked(
        &self,
        s: &mut State,
        by: &str,
        to: &str,
        body: &str,
        conversation: Option<String>,
        in_reply_to: Option<String>,
        kind: Kind,
        auto: bool,
    ) -> Result<Value, String> {
        {
            // Scoped: the whole-state borrows below cannot overlap a live
            // `&mut` into s.daily.
            let today = utc_date(unix_now());
            let use_today = s.daily.entry(to.to_owned()).or_default();
            if use_today.date != today {
                use_today.date = today;
                use_today.sent = 0;
            }
            if use_today.sent >= self.cfg.per_peer_daily_limit {
                return Err(format!(
                    "daily limit to {to} reached ({} per day)",
                    self.cfg.per_peer_daily_limit
                ));
            }
        }
        let now = unix_now();
        let conv_id = conversation.unwrap_or_else(|| self.next_id("cv"));
        let index = match Self::conv_index(s, &conv_id) {
            Some(i) => i,
            None => {
                s.conversations.push(Conversation {
                    id: conv_id.clone(),
                    peer: to.to_owned(),
                    opened_by: by.to_owned(),
                    auto_left: 0,
                    closed: false,
                    hop: 0,
                    created_at: now,
                    updated_at: now,
                    last_in: None,
                    last_out: None,
                });
                s.conversations.len() - 1
            }
        };
        let conv = &mut s.conversations[index];
        if conv.peer != to {
            return Err(format!("conversation {conv_id} is with {}", conv.peer));
        }
        conv.updated_at = now;
        let hop = if auto {
            conv.hop += 1;
            conv.auto_left = conv.auto_left.saturating_sub(1);
            conv.hop
        } else {
            // A manual message is a fresh link, not part of the unattended
            // chain — the hop counter only bounds auto-forwards.
            0
        };
        if kind == Kind::Close {
            conv.closed = true;
        }
        let reply_to = in_reply_to.or_else(|| conv.last_in.clone());
        let auto_left = conv.auto_left;
        let id = self.next_id("m");
        conv.last_out = Some(id.clone());
        let envelope = Envelope {
            id: id.clone(),
            from: self.node.clone(),
            to: to.to_owned(),
            conversation: Some(conv_id.clone()),
            in_reply_to: reply_to,
            hop,
            kind,
            body: body.to_owned(),
            sent_at: now,
            auto_left,
            auto,
            retried: false,
        };
        s.daily.get_mut(to).expect("entry created").sent += 1;
        s.outbox.push_back(Outbound {
            envelope,
            attempts: 0,
            next_at: now,
            state: OutState::Queued,
            last_error: None,
            sent_via: None,
            delivered_at: None,
        });
        Ok(json!({"queued": id, "conversation": conv_id}))
    }

    /// Individual (`peer_say`) or operator (`send`) → peer.
    #[allow(clippy::too_many_arguments)]
    pub fn queue_send(
        &self,
        shared: &Shared,
        by: &str,
        to: &str,
        body: &str,
        conversation: Option<String>,
        in_reply_to: Option<String>,
        kind: Kind,
    ) -> Result<Value, String> {
        if !self.cfg.enabled {
            return Err("intercom is disabled".to_owned());
        }
        if body.trim().is_empty() || body.len() > MAX_BODY {
            return Err("body must be 1..32KiB".to_owned());
        }
        if to == shared.config.node.id {
            return Err("cannot send to this node".to_owned());
        }
        if !shared.config.peers.iter().any(|p| p.id == to) {
            return Err(format!("{to:?} is not a configured peer"));
        }
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let result =
            self.queue_locked(&mut s, by, to, body, conversation, in_reply_to, kind, false);
        if let Ok(queued) = &result {
            self.persist(&mut s);
            drop(s);
            self.changed.notify_all();
            let _ = shared.spool.append_sync(
                JOURNAL,
                json!({"kind": "queued", "id": queued["queued"], "to": to,
                       "by": by, "conversation": queued["conversation"],
                       "message_kind": kind.as_str()}),
            );
        }
        result
    }

    /// `/status` section.
    pub fn summary(&self) -> Value {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queued, mut dead) = (0u64, 0u64);
        for o in &s.outbox {
            match o.state {
                OutState::Queued => queued += 1,
                OutState::Dead => dead += 1,
                OutState::Sent => {}
            }
        }
        let relay = self
            .relay_status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        json!({
            "enabled": self.cfg.enabled,
            "inbound_queued": s.inbound.len(),
            "outbox": {"queued": queued, "dead": dead, "kept": s.outbox.len()},
            "conversations": s.conversations.iter().rev().take(20)
                .map(Conversation::view).collect::<Vec<_>>(),
            "daily": s.daily,
            "relay": {
                "configured": self.cfg.relay.is_some(),
                "cursor": s.relay_cursor,
                "status": relay,
            },
        })
    }

    /// The text the individual sees as the turn's message.
    fn frame_incoming(&self, s: &State, env: &Envelope) -> String {
        let who = self.sister_label(&env.from);
        let conv_id = env.conversation.as_deref().unwrap_or("-");
        let conv_auto = env
            .conversation
            .as_ref()
            .and_then(|id| Self::conv_index(s, id))
            .map(|i| s.conversations[i].auto_left)
            .unwrap_or(0);
        let head = if env.kind == Kind::Close {
            format!(
                "[intercom] {who}（ノード {}）がこの会話を閉じる最後のメッセージです。",
                env.from
            )
        } else {
            let mut line = format!(
                "[intercom] {who}（ノード {}）からあなたへのメッセージです。",
                env.from
            );
            if conv_auto > 0 {
                line.push_str(&format!(
                    "\nこの会話は自動返信モード（残り約{conv_auto}往復）: \
                     あなたの応答はそのまま {} へ届きます。\
                     途中で終わらせたいときは応答に [end] だけの行を入れてください。",
                    env.from
                ));
            } else {
                line.push_str(&format!(
                    "\n返信する場合は peer_say(to=\"{}\", conversation=\"{conv_id}\", body=\"…\") を呼んでください。\
                     会話を閉じるときは end=true を付けます。",
                    env.from
                ));
            }
            line
        };
        format!(
            "{head}\nconversation: {conv_id}\nmessage: {}\n\n{}",
            env.id, env.body
        )
    }

    /// Pop and process every waiting inbound envelope. Factored out of the
    /// blocking loop so tests can drain the queue directly.
    fn drain_inbound(&self, shared: &Shared) {
        let Some(dialogue) = shared.config.dialogue.clone() else {
            return;
        };
        loop {
            let env = {
                let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                match s.inbound.pop_front() {
                    Some(env) => {
                        self.persist(&mut s);
                        env
                    }
                    None => break,
                }
            };
            let subject = format!("sister-{}", env.from);
            let framed = {
                let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                self.frame_incoming(&s, &env)
            };
            let (status, reply) = crate::dialogue::talk(
                shared,
                &dialogue,
                &json!({"subject": subject, "message": framed}),
            );
            match status {
                200 => {
                    let text = reply["response"].as_str().unwrap_or("").to_owned();
                    let (body, wants_end) = strip_end_markers(&text);
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    // A close envelope ends the conversation: heard, not
                    // answered — auto-replying would reopen what was shut.
                    let mut closed_here = env.kind == Kind::Close;
                    let mut forwarded = None;
                    if !closed_here
                        && let Some(conv_id) = env.conversation.clone()
                        && let Some(i) = Self::conv_index(&s, &conv_id)
                        && !s.conversations[i].closed
                        && s.conversations[i].auto_left > 0
                    {
                        let kind = if wants_end {
                            Kind::Close
                        } else {
                            Kind::Message
                        };
                        if !body.is_empty() {
                            forwarded = self
                                .queue_locked(
                                    &mut s,
                                    "auto",
                                    &env.from,
                                    &body,
                                    Some(conv_id),
                                    Some(env.id.clone()),
                                    kind,
                                    true,
                                )
                                .ok();
                        }
                        closed_here = wants_end;
                    }
                    if closed_here
                        && let Some(conv_id) = &env.conversation
                        && let Some(i) = Self::conv_index(&s, conv_id)
                    {
                        s.conversations[i].closed = true;
                    }
                    self.persist(&mut s);
                    drop(s);
                    let _ = shared.spool.append_sync(
                        JOURNAL,
                        json!({"kind": "answered", "id": env.id, "from": env.from,
                               "conversation": env.conversation,
                               "forwarded": forwarded.as_ref().map(|f| f["queued"].clone()),
                               "closed": closed_here}),
                    );
                    if forwarded.is_some() {
                        self.changed.notify_all();
                    }
                }
                s5xx if s5xx >= 500 && !env.retried => {
                    // One retry — the intake record is durable either way.
                    let mut env = env;
                    env.retried = true;
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.inbound.push_back(env);
                    self.persist(&mut s);
                }
                _ => {
                    let _ = shared.spool.append_sync(
                        JOURNAL,
                        json!({"kind": "turn_failed", "id": env.id, "from": env.from,
                               "conversation": env.conversation, "status": status}),
                    );
                }
            }
        }
    }

    /// POST one envelope to the peer's `/v1/intercom`. `Ok` means the
    /// peer accepted (or already had it); `Err` starting with `rejected:`
    /// or `no peer` is permanent, everything else is retriable.
    fn deliver_direct(&self, shared: &Shared, env: &Envelope) -> Result<&'static str, String> {
        let Some(peer) = shared.config.peers.iter().find(|p| p.id == env.to) else {
            return Err("no peer in config".to_owned());
        };
        let body = serde_json::to_value(env).map_err(|e| e.to_string())?;
        match crate::tools::post_peer(
            &shared.peer_url(peer),
            "/v1/intercom",
            &body,
            shared.token.as_deref(),
            Duration::from_secs(self.cfg.send_timeout_secs),
        ) {
            Some((status, v)) if (200..300).contains(&status) => {
                if v["accepted"].as_bool().unwrap_or(false) || v["dedup"].as_bool().unwrap_or(false)
                {
                    Ok("direct")
                } else {
                    Err(format!(
                        "rejected: {}",
                        v["reason"].as_str().unwrap_or("refused")
                    ))
                }
            }
            Some((status, v)) => Err(format!(
                "HTTP {status}: {}",
                v["error"].as_str().unwrap_or("")
            )),
            None => Err("no response".to_owned()),
        }
    }

    /// POST to the relay mailbox (`/v1/send`).
    fn relay_send(&self, env: &Envelope) -> Result<(), String> {
        let Some(relay) = &self.cfg.relay else {
            return Err("no relay configured".to_owned());
        };
        relay_post(relay, "/v1/send", &json!({"to": env.to, "envelope": env})).map(|_| ())
    }

    /// Deliver due outbox entries once: direct POST, then the relay
    /// mailbox when configured. Factored out of the loop for tests.
    fn drain_outbox(&self, shared: &Shared) {
        let now = unix_now();
        let due: Vec<String> = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.outbox
                .iter()
                .filter(|o| o.state == OutState::Queued && o.next_at <= now)
                .map(|o| o.envelope.id.clone())
                .collect()
        };
        for id in due {
            let env = {
                let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                match s.outbox.iter().find(|o| o.envelope.id == id) {
                    Some(o) if o.state == OutState::Queued => o.envelope.clone(),
                    _ => continue,
                }
            };
            let outcome = match self.deliver_direct(shared, &env) {
                Ok(via) => Ok(via.to_owned()),
                Err(e) => match self.relay_send(&env) {
                    Ok(()) => Ok("relay".to_owned()),
                    Err(relay_err) => Err(format!("{e}; relay: {relay_err}")),
                },
            };
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let Some(index) = s.outbox.iter().position(|o| o.envelope.id == id) else {
                continue;
            };
            match outcome {
                Ok(via) => {
                    let o = &mut s.outbox[index];
                    o.state = OutState::Sent;
                    o.sent_via = Some(via.clone());
                    o.delivered_at = Some(unix_now());
                    o.last_error = None;
                    self.persist(&mut s);
                    drop(s);
                    let _ = shared.spool.append_sync(
                        JOURNAL,
                        json!({"kind": "sent", "id": env.id, "to": env.to, "via": via,
                               "conversation": env.conversation}),
                    );
                }
                Err(e) => {
                    let permanent =
                        e.starts_with("rejected:") || e.starts_with("no peer in config");
                    let o = &mut s.outbox[index];
                    o.attempts += 1;
                    o.last_error = Some(e.clone());
                    if permanent || o.attempts >= self.cfg.max_attempts {
                        o.state = OutState::Dead;
                    } else {
                        o.next_at = unix_now() + self.cfg.retry_secs * (1 << o.attempts.min(6));
                    }
                    let (state, attempts) = (o.state, o.attempts);
                    self.persist(&mut s);
                    drop(s);
                    let _ = shared.spool.append_sync(
                        JOURNAL,
                        json!({"kind": if state == OutState::Dead { "dead" } else { "retry" },
                               "id": env.id, "to": env.to, "attempts": attempts,
                               "error": e}),
                    );
                }
            }
        }
    }

    /// Poll this node's relay mailbox once: ingest every envelope after
    /// the cursor, then acknowledge them. Returns the ingested count.
    fn poll_relay(&self, shared: &Shared, relay: &RelayConfig) -> Result<usize, String> {
        let cursor = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .relay_cursor;
        let reply = relay_post(
            relay,
            "/v1/inbox",
            &json!({"node": self.node, "after": cursor, "limit": 50}),
        )?;
        let mut ingested = 0usize;
        let mut upto = cursor;
        for message in reply["messages"].as_array().into_iter().flatten() {
            let seq = message["seq"].as_u64().unwrap_or(0);
            match serde_json::from_value::<Envelope>(message["envelope"].clone()) {
                Ok(env) => {
                    // accept() deduplicates: redelivery after a crash
                    // between ingest and ack is safe. A non-200 (e.g.
                    // inbound queue full) leaves this and everything
                    // behind it for the next poll.
                    if self.accept(shared, env).0 != 200 {
                        break;
                    }
                    ingested += 1;
                    upto = upto.max(seq);
                }
                Err(_) => {
                    // Poisoned entry: drop it past the cursor so it does
                    // not block the mailbox forever.
                    let _ = shared.spool.append_sync(
                        JOURNAL,
                        json!({"kind": "dropped", "reason": "malformed relay entry",
                               "seq": seq}),
                    );
                    upto = upto.max(seq);
                }
            }
        }
        if upto > cursor {
            relay_post(relay, "/v1/ack", &json!({"node": self.node, "upto": upto}))?;
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.relay_cursor = upto;
            self.persist(&mut s);
        }
        Ok(ingested)
    }

    fn relay_loop(&self, shared: &Shared, relay: RelayConfig) {
        loop {
            let result = self.poll_relay(shared, &relay);
            let mut status = self.relay_status.lock().unwrap_or_else(|p| p.into_inner());
            *status = match result {
                Ok(n) => json!({"ok": true, "at": iso8601(unix_now()), "ingested": n}),
                Err(e) => json!({"ok": false, "at": iso8601(unix_now()), "error": e}),
            };
            drop(status);
            std::thread::sleep(Duration::from_secs(relay.poll_secs));
        }
    }

    fn inbound_loop(&self, shared: &Shared) {
        loop {
            self.drain_inbound(shared);
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            drop(self.changed.wait(s).unwrap_or_else(|p| p.into_inner()));
        }
    }

    fn outbound_loop(&self, shared: &Shared) {
        loop {
            self.drain_outbox(shared);
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let _ = self
                .changed
                .wait_timeout(s, Duration::from_secs(5))
                .unwrap_or_else(|p| p.into_inner());
        }
    }
}

/// Remove `[end]` lines the individual wrote to close a conversation in
/// auto-reply mode; returns the text without them and whether it closed.
fn strip_end_markers(text: &str) -> (String, bool) {
    let mut end = false;
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            if line.trim() == "[end]" {
                end = true;
                false
            } else {
                true
            }
        })
        .collect();
    (kept.join("\n").trim().to_owned(), end)
}

/// POST `body` to `relay.url + suffix` with the relay token. The token is
/// read from the named env var on each call — never stored.
fn relay_post(relay: &RelayConfig, suffix: &str, body: &Value) -> Result<Value, String> {
    let token = std::env::var(&relay.token_env)
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("credential env {} is not set", relay.token_env))?;
    let endpoint = Endpoint::parse(&relay.url, suffix)?;
    let headers = [Header {
        name: "Authorization".to_owned(),
        value: format!("Bearer {token}"),
    }];
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &headers,
        RELAY_TIMEOUT,
        &TrustAnchors::Webpki,
    )
    .map_err(|e| crate::probes::describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if response.is_success() {
        Ok(value)
    } else {
        Err(format!("HTTP {}: {value}", response.status))
    }
}

// ---------------------------------------------------------------------------
// HTTP / tool surface

/// `POST /v1/intercom` and `GET /v1/intercom`. A body carrying `from`/`to`
/// is a peer envelope — anything else is an operator action, the same
/// dispatch convention as `/v1/agents`.
pub fn handle(shared: &Shared, request: &Value, by: &str) -> (u16, Value) {
    let ic = &shared.intercom;
    if request.get("action").is_none() && request.get("from").is_some() {
        return match serde_json::from_value::<Envelope>(request.clone()) {
            Ok(env) => ic.accept(shared, env),
            Err(e) => (400, json!({"error": format!("envelope: {e}")})),
        };
    }
    let text = |k: &str| request[k].as_str().unwrap_or("").trim().to_owned();
    match request["action"].as_str().unwrap_or("status") {
        "status" => (200, ic.summary()),
        "list" => {
            let all = request["all"].as_bool().unwrap_or(false);
            let s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
            let convs: Vec<Value> = s
                .conversations
                .iter()
                .rev()
                .filter(|c| all || !c.closed)
                .map(Conversation::view)
                .collect();
            (200, json!({"conversations": convs}))
        }
        "show" => {
            let id = text("id");
            let s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
            match Intercom::conv_index(&s, &id) {
                Some(i) => {
                    let queued: Vec<Value> = s
                        .outbox
                        .iter()
                        .filter(|o| {
                            o.envelope.conversation.as_deref() == Some(id.as_str())
                                && o.state == OutState::Queued
                        })
                        .map(Outbound::view)
                        .collect();
                    (
                        200,
                        json!({"conversation": s.conversations[i].view(),
                               "queued": queued}),
                    )
                }
                None => (404, json!({"error": "no conversation with that id"})),
            }
        }
        "send" => {
            let kind = if request["end"].as_bool().unwrap_or(false) {
                Kind::Close
            } else {
                Kind::Message
            };
            let to = text("to");
            // No conversation named → continue the latest open one with
            // that peer (a fresh conversation is `open`).
            let conversation = request["conversation"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    let s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.conversations
                        .iter()
                        .rev()
                        .find(|c| c.peer == to && !c.closed)
                        .map(|c| c.id.clone())
                });
            match ic.queue_send(
                shared,
                by,
                &to,
                request["body"].as_str().unwrap_or(""),
                conversation,
                request["in_reply_to"].as_str().map(str::to_owned),
                kind,
            ) {
                Ok(v) => (200, v),
                Err(e) => (400, json!({"error": e})),
            }
        }
        "open" => {
            let peer = text("peer");
            if !shared.config.peers.iter().any(|p| p.id == peer) {
                return (
                    400,
                    json!({"error": format!("{peer:?} is not a configured peer")}),
                );
            }
            let turns = request["turns"]
                .as_u64()
                .unwrap_or(ic.cfg.auto_reply_turns)
                .clamp(1, ic.cfg.max_hops.max(1));
            let mut s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
            let now = unix_now();
            let conv_id = s
                .conversations
                .iter()
                .rev()
                .find(|c| c.peer == peer && !c.closed)
                .map(|c| c.id.clone())
                .unwrap_or_else(|| ic.next_id("cv"));
            if Intercom::conv_index(&s, &conv_id).is_none() {
                s.conversations.push(Conversation {
                    id: conv_id.clone(),
                    peer: peer.clone(),
                    opened_by: by.to_owned(),
                    auto_left: 0,
                    closed: false,
                    hop: 0,
                    created_at: now,
                    updated_at: now,
                    last_in: None,
                    last_out: None,
                });
            }
            let i = Intercom::conv_index(&s, &conv_id).expect("present");
            s.conversations[i].auto_left = s.conversations[i].auto_left.max(turns);
            s.conversations[i].closed = false;
            s.conversations[i].updated_at = now;
            ic.persist(&mut s);
            drop(s);
            let _ = shared.spool.append_sync(
                JOURNAL,
                json!({"kind": "opened", "conversation": conv_id, "peer": peer,
                       "by": by, "auto_left": turns}),
            );
            // An optional opening message carries the grant to the peer.
            let body = request["body"].as_str().unwrap_or("");
            let sent = if body.is_empty() {
                Value::Null
            } else {
                match ic.queue_send(
                    shared,
                    by,
                    &peer,
                    body,
                    Some(conv_id.clone()),
                    None,
                    Kind::Message,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        return (
                            400,
                            json!({"error": e, "conversation": conv_id, "opened": true}),
                        );
                    }
                }
            };
            (
                200,
                json!({"ok": true, "conversation": conv_id, "auto_left": turns,
                       "sent": sent}),
            )
        }
        "close" => {
            let id = text("id");
            let peer = {
                let mut s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
                match Intercom::conv_index(&s, &id) {
                    Some(i) => {
                        s.conversations[i].closed = true;
                        s.conversations[i].updated_at = unix_now();
                        let peer = s.conversations[i].peer.clone();
                        ic.persist(&mut s);
                        peer
                    }
                    None => return (404, json!({"error": "no conversation with that id"})),
                }
            };
            let _ = shared.spool.append_sync(
                JOURNAL,
                json!({"kind": "closed", "conversation": id, "by": by}),
            );
            // Optional final words travel as a close envelope.
            let body = request["body"].as_str().unwrap_or("");
            if !body.is_empty() {
                let _ = ic.queue_send(shared, by, &peer, body, Some(id.clone()), None, Kind::Close);
            }
            (200, json!({"ok": true, "closed": id}))
        }
        "retry" => {
            let id = text("id");
            let mut s = ic.state.lock().unwrap_or_else(|p| p.into_inner());
            let mut n = 0u64;
            for o in s.outbox.iter_mut() {
                if o.state == OutState::Dead && (id.is_empty() || o.envelope.id == id) {
                    o.state = OutState::Queued;
                    o.attempts = 0;
                    o.next_at = unix_now();
                    n += 1;
                }
            }
            ic.persist(&mut s);
            drop(s);
            ic.changed.notify_all();
            (200, json!({"ok": true, "requeued": n}))
        }
        other => (400, json!({"error": format!("unknown action {other:?}")})),
    }
}

/// `peer_say` / `peer_list` for the individual.
pub fn tool(shared: &Shared, name: &str, request: &Value) -> (u16, Value) {
    let arguments = match &request["arguments"] {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(v) => v,
            Err(_) => {
                return (
                    200,
                    json!({"ok": false, "error": "arguments are not valid JSON"}),
                );
            }
        },
        Value::Null => json!({}),
        other => other.clone(),
    };
    match name {
        "peer_say" => {
            let kind = if arguments["end"].as_bool().unwrap_or(false) {
                Kind::Close
            } else {
                Kind::Message
            };
            match shared.intercom.queue_send(
                shared,
                "mio",
                arguments["to"].as_str().unwrap_or(""),
                arguments["body"].as_str().unwrap_or(""),
                arguments["conversation"].as_str().map(str::to_owned),
                arguments["in_reply_to"].as_str().map(str::to_owned),
                kind,
            ) {
                Ok(v) => (
                    200,
                    json!({"ok": true, "result": v,
                           "note": "queued for delivery — retried until the peer answers"}),
                ),
                Err(e) => (200, json!({"ok": false, "error": e})),
            }
        }
        "peer_list" => {
            let peers: Vec<Value> = shared
                .config
                .peers
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "role": p.role.as_str(),
                        "label": shared
                            .intercom
                            .cfg
                            .sisters
                            .get(&p.id)
                            .map(|s| s.label.clone()),
                        "reachable": shared.peer_healthy(&p.id),
                    })
                })
                .collect();
            (200, json!({"ok": true, "result": {"peers": peers}}))
        }
        _ => (
            400,
            json!({"ok": false, "error": format!("unknown tool {name:?}")}),
        ),
    }
}

/// Tool definitions for the individual.
pub fn definitions() -> Vec<Value> {
    let f = |name: &str, description: &str, parameters: Value| {
        json!({"type": "function", "function": {
            "name": name, "description": description, "parameters": parameters}})
    };
    vec![
        f(
            "peer_say",
            "Send a message to a sister individual on a peer node — the other individual hosted on another machine (peer_list shows who is who). The message reaches her as a dialogue turn she can answer with her own peer_say. Pass conversation to continue a thread, omit it to start a new one; end=true sends final words and closes the conversation. Delivery is queued and retried: 'queued' means accepted for delivery, not yet read.",
            json!({"type": "object", "properties": {
                "to": {"type": "string", "description": "peer node id (see peer_list)"},
                "body": {"type": "string", "description": "the message, up to 32KiB"},
                "conversation": {"type": "string", "description": "existing conversation id"},
                "in_reply_to": {"type": "string"},
                "end": {"type": "boolean", "description": "send final words and close"}},
                "required": ["to", "body"]}),
        ),
        f(
            "peer_list",
            "List the sister individuals on peer nodes: id, role, display label, and whether they are reachable right now.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

/// The inbound turn worker (only when this node hosts an individual), the
/// outbound delivery loop, and — with `intercom.relay` — the mailbox
/// poller.
pub fn spawn(shared: &Arc<Shared>) {
    if !shared.config.intercom.enabled {
        return;
    }
    if shared.config.dialogue.is_some() {
        let s = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("intercom-in".to_owned())
            .spawn(move || s.intercom.inbound_loop(&s));
    }
    let s = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("intercom-out".to_owned())
        .spawn(move || s.intercom.outbound_loop(&s));
    if let Some(relay) = shared.config.intercom.relay.clone() {
        let s = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("intercom-relay".to_owned())
            .spawn(move || s.intercom.relay_loop(&s, relay));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{Receiver, channel};
    use std::thread;

    /// A resident whose `runtime_binary` is a shell script, like
    /// dialogue::tests.
    fn shared_with(dir: &std::path::Path, script_body: &str, extra: Value) -> (Shared, PathBuf) {
        let rt = dir.join("rt");
        std::fs::create_dir_all(&rt).expect("rt dir");
        std::fs::write(rt.join("kamimusuhi.sqlite"), b"").expect("db marker");
        let bin = dir.join("fake-runtime");
        std::fs::write(&bin, format!("#!/bin/sh\n{script_body}\n")).expect("script");
        let mut perms = std::fs::metadata(&bin).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&bin, perms).expect("chmod");
        let mut cfg = json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir},
            "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 5},
            "peers": [{"id": "mac", "role": "cognition", "url": "http://127.0.0.1:1"}],
            "intercom": {"sisters": {"mac": {"label": "長女"}}},
        });
        json_patch(cfg.as_object_mut().unwrap(), extra);
        let config: Config = serde_json::from_value(cfg).expect("config");
        config.validate().expect("valid");
        let spool = crate::spool::Spool::new(dir.join("spool"), "pi").expect("spool");
        let shared = Shared::new(config, spool, 1, None);
        (shared, dir.to_path_buf())
    }

    fn json_patch(target: &mut serde_json::Map<String, Value>, extra: Value) {
        if let Value::Object(map) = extra {
            for (k, v) in map {
                target.insert(k, v);
            }
        }
    }

    fn envelope(id: &str, body: &str) -> Envelope {
        Envelope {
            id: id.to_owned(),
            from: "mac".to_owned(),
            to: "pi".to_owned(),
            conversation: None,
            in_reply_to: None,
            hop: 0,
            kind: Kind::Message,
            body: body.to_owned(),
            sent_at: unix_now(),
            auto_left: 0,
            auto: false,
            retried: false,
        }
    }

    /// Read one HTTP request (head + body) and return it as a string.
    fn read_request(stream: &TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut head = String::new();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("line");
            if line.trim().is_empty() {
                break;
            }
            head.push_str(&line);
            if let Some(v) = line.strip_prefix("Content-Length:") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).expect("body");
        format!("{head}\n{}", String::from_utf8_lossy(&body))
    }

    /// HTTP responder: serves `count` connections, answers each with
    /// (status, json body) from `respond`, and forwards every request to
    /// the returned channel.
    fn json_server<R>(
        count: usize,
        respond: R,
    ) -> (String, Receiver<String>, thread::JoinHandle<()>)
    where
        R: Fn(&str) -> (u16, &'static str) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let (tx, rx) = channel();
        let handle = thread::spawn(move || {
            for _ in 0..count {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_request(&stream);
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (status, body) = respond(&request);
                let _ = tx.send(format!("{path}\n{request}"));
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, rx, handle)
    }

    #[test]
    fn accept_validates_and_deduplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(dir.path(), "exit 1", json!({}));
        let (_, body) = shared.intercom.accept(&shared, envelope("m-1", "hi"));
        assert_eq!(body["accepted"], json!(true));
        // Same id again: dedup, no second queue entry.
        let (_, body) = shared.intercom.accept(&shared, envelope("m-1", "hi"));
        assert_eq!(body["dedup"], json!(true), "{body}");
        // Wrong addressee / unknown sender / bad id are refused.
        let mut wrong = envelope("m-2", "hi");
        wrong.to = "llm_master".to_owned();
        assert_eq!(shared.intercom.accept(&shared, wrong).0, 400);
        let mut stranger = envelope("m-3", "hi");
        stranger.from = "intruder".to_owned();
        assert_eq!(shared.intercom.accept(&shared, stranger).0, 400);
        // An auto envelope past the hop limit is dropped, not retried.
        let mut deep = envelope("m-4", "hi");
        deep.auto = true;
        deep.hop = 99;
        let (_, body) = shared.intercom.accept(&shared, deep);
        assert_eq!(body["accepted"], json!(false));
        // Everything else sits in the durable inbound queue.
        let s = shared.intercom.state.lock().expect("state");
        assert_eq!(s.inbound.len(), 1);
    }

    #[test]
    fn inbound_turn_auto_replies_within_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"よろしくね\",\"turn_id\":\"t1\"}'",
            json!({}),
        );
        let mut env = envelope("m-1", "こんにちは");
        env.conversation = Some("cv-mac-1".to_owned());
        env.auto_left = 3;
        assert_eq!(shared.intercom.accept(&shared, env).0, 200);
        shared.intercom.drain_inbound(&shared);
        let s = shared.intercom.state.lock().expect("state");
        let conv = s
            .conversations
            .iter()
            .find(|c| c.id == "cv-mac-1")
            .expect("conv");
        // Granted 3, one reply consumed → 2 left, reply queued to mac.
        assert_eq!(conv.auto_left, 2);
        let out = s.outbox.back().expect("queued reply");
        assert_eq!(out.envelope.to, "mac");
        assert_eq!(out.envelope.body, "よろしくね");
        assert!(out.envelope.auto);
        assert_eq!(out.envelope.hop, 1);
        assert_eq!(out.envelope.in_reply_to.as_deref(), Some("m-1"));
    }

    #[test]
    fn inbound_tool_mode_does_not_auto_reply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"あとでね\",\"turn_id\":\"t1\"}'",
            json!({}),
        );
        let mut env = envelope("m-1", "質問です");
        env.conversation = Some("cv-mac-9".to_owned());
        // No auto grant.
        shared.intercom.accept(&shared, env);
        shared.intercom.drain_inbound(&shared);
        let s = shared.intercom.state.lock().expect("state");
        assert!(s.outbox.is_empty(), "no reply is sent without a grant");
    }

    #[test]
    fn end_marker_closes_the_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"またね\\n[end]\",\"turn_id\":\"t1\"}'",
            json!({}),
        );
        let mut env = envelope("m-1", "そろそろ終わりにしよう");
        env.conversation = Some("cv-mac-5".to_owned());
        env.auto_left = 5;
        shared.intercom.accept(&shared, env);
        shared.intercom.drain_inbound(&shared);
        let s = shared.intercom.state.lock().expect("state");
        let conv = &s.conversations[0];
        assert!(conv.closed);
        let out = s.outbox.back().expect("close reply");
        assert_eq!(out.envelope.kind, Kind::Close);
        assert_eq!(out.envelope.body, "またね");
    }

    #[test]
    fn close_envelope_is_heard_not_answered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(
            dir.path(),
            "printf '%s\\n' '{\"response\":\"うん\",\"turn_id\":\"t1\"}'",
            json!({}),
        );
        let mut env = envelope("m-7", "じゃあね");
        env.kind = Kind::Close;
        env.conversation = Some("cv-mac-7".to_owned());
        env.auto_left = 9;
        shared.intercom.accept(&shared, env);
        shared.intercom.drain_inbound(&shared);
        let s = shared.intercom.state.lock().expect("state");
        assert!(s.outbox.is_empty(), "a close never gets an auto-reply");
        assert!(s.conversations[0].closed);
    }

    #[test]
    fn queue_send_caps_the_daily_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(
            dir.path(),
            "exit 0",
            json!({"intercom": {"per_peer_daily_limit": 2}}),
        );
        for _ in 0..2 {
            shared
                .intercom
                .queue_send(&shared, "mio", "mac", "hi", None, None, Kind::Message)
                .expect("queued");
        }
        let err = shared
            .intercom
            .queue_send(&shared, "mio", "mac", "hi", None, None, Kind::Message)
            .expect_err("capped");
        assert!(err.contains("daily limit"), "{err}");
    }

    #[test]
    fn outbox_delivers_direct_and_dead_letters_rejections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let accept = |_: &str| (200, r#"{"accepted": true}"#);
        let (url, rx, server) = json_server(2, accept);
        let (shared, _) = shared_with(
            dir.path(),
            "exit 0",
            json!({"peers": [{"id": "mac", "role": "cognition", "url": url}]}),
        );
        shared
            .intercom
            .queue_send(
                &shared,
                "operator",
                "mac",
                "ping",
                None,
                None,
                Kind::Message,
            )
            .expect("queued");
        shared.intercom.drain_outbox(&shared);
        {
            let s = shared.intercom.state.lock().expect("state");
            assert_eq!(s.outbox[0].state, OutState::Sent);
            assert_eq!(s.outbox[0].sent_via.as_deref(), Some("direct"));
        }
        let request = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("peer got the POST");
        assert!(request.contains("/v1/intercom"), "{request}");
        assert!(request.contains("ping"), "{request}");
        let _ = server;

        // A peer that refuses permanently dead-letters without retrying.
        let reject = |_: &str| (200, r#"{"accepted": false, "reason": "hop limit reached"}"#);
        let (url, _rx2, _s2) = json_server(1, reject);
        let (shared2, _) = shared_with(
            dir.path().join("two").as_path(),
            "exit 0",
            json!({"peers": [{"id": "mac", "role": "cognition", "url": url}]}),
        );
        shared2
            .intercom
            .queue_send(
                &shared2,
                "operator",
                "mac",
                "ping",
                None,
                None,
                Kind::Message,
            )
            .expect("queued");
        shared2.intercom.drain_outbox(&shared2);
        let s = shared2.intercom.state.lock().expect("state");
        assert_eq!(s.outbox[0].state, OutState::Dead);
    }

    #[test]
    fn outbox_retries_then_relays() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Peer refuses connections; the relay accepts.
        let relay_respond = |req: &str| {
            if req.contains("/v1/send") {
                (200, r#"{"seq": 7}"#)
            } else {
                (404, "{}")
            }
        };
        let (relay_url, rx, _relay) = json_server(2, relay_respond);
        let (shared, _) = shared_with(
            dir.path(),
            "exit 0",
            json!({
                "intercom": {"retry_secs": 1,
                             "relay": {"url": relay_url, "token_env": "PATH"}},
            }),
        );
        // The crate forbids `unsafe` (and `set_var` needs it in edition
        // 2024), so tests point token_env at a variable that is already
        // set — any non-empty one proves the header path.
        shared
            .intercom
            .queue_send(
                &shared,
                "operator",
                "mac",
                "ping",
                None,
                None,
                Kind::Message,
            )
            .expect("queued");
        shared.intercom.drain_outbox(&shared);
        let s = shared.intercom.state.lock().expect("state");
        assert_eq!(s.outbox[0].state, OutState::Sent);
        assert_eq!(s.outbox[0].sent_via.as_deref(), Some("relay"));
        let request = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("relay got the POST");
        assert!(request.contains("Bearer "), "{request}");
    }

    #[test]
    fn relay_poll_ingests_and_acks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inbox = r#"{"messages": [{"seq": 3, "envelope":
            {"id": "m-mac-9", "from": "mac", "to": "pi", "body": "relay経由"}}]}"#;
        let respond = move |req: &str| {
            if req.contains("/v1/inbox") {
                (200, inbox)
            } else {
                (200, r#"{"ok": true}"#)
            }
        };
        let (url, rx, _server) = json_server(2, respond);
        let (shared, _) = shared_with(
            dir.path(),
            "exit 0",
            json!({
                "intercom": {"relay": {"url": url, "token_env": "PATH"}},
            }),
        );
        // The crate forbids `unsafe` (and `set_var` needs it in edition
        // 2024), so tests point token_env at a variable that is already
        // set — any non-empty one proves the header path.
        let relay = shared.config.intercom.relay.clone().expect("relay");
        assert_eq!(shared.intercom.poll_relay(&shared, &relay), Ok(1));
        let s = shared.intercom.state.lock().expect("state");
        assert_eq!(s.relay_cursor, 3);
        assert_eq!(s.inbound.len(), 1);
        assert_eq!(s.inbound[0].body, "relay経由");
        let _inbox = rx.recv_timeout(Duration::from_secs(10)).expect("inbox");
        let ack = rx.recv_timeout(Duration::from_secs(10)).expect("ack");
        assert!(ack.contains("/v1/ack"), "{ack}");
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(dir.path(), "exit 0", json!({}));
        shared
            .intercom
            .queue_send(
                &shared,
                "operator",
                "mac",
                "ping",
                None,
                None,
                Kind::Message,
            )
            .expect("queued");
        shared.intercom.accept(&shared, envelope("m-1", "hi"));
        drop(shared);
        // Reload from the persisted file.
        let path = dir.path().join("current_state/intercom.json");
        let again = Intercom::load(path, IntercomConfig::default(), "pi".to_owned());
        let s = again.state.lock().expect("state");
        assert_eq!(s.outbox.len(), 1);
        assert!(s.seen.iter().any(|id| id == "m-1"));
        assert!(!s.conversations.is_empty());
    }

    #[test]
    fn open_grants_auto_budget_and_sends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(dir.path(), "exit 0", json!({}));
        let (status, body) = handle(
            &shared,
            &json!({"action": "open", "peer": "mac", "turns": 4, "body": "姉さん、相談があります"}),
            "operator",
        );
        assert_eq!(status, 200, "{body}");
        let s = shared.intercom.state.lock().expect("state");
        let conv = &s.conversations[0];
        assert_eq!(conv.auto_left, 4);
        assert_eq!(conv.opened_by, "operator");
        assert_eq!(s.outbox[0].envelope.auto_left, 4);
        assert!(!s.outbox[0].envelope.auto);
    }

    #[test]
    fn peer_say_and_list_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shared, _) = shared_with(dir.path(), "exit 0", json!({}));
        let (status, reply) = tool(
            &shared,
            "peer_say",
            &json!({"name": "peer_say",
                    "arguments": {"to": "mac", "body": "元気？"}}),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], json!(true));
        let s = shared.intercom.state.lock().expect("state");
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0].envelope.body, "元気？");
        drop(s);
        let (_, reply) = tool(&shared, "peer_list", &json!({"name": "peer_list"}));
        assert_eq!(reply["result"]["peers"][0]["label"], json!("長女"));
        assert_eq!(reply["result"]["peers"][0]["id"], json!("mac"));
    }
}
