//! Operator-only Discord gateway. All individual state belongs to the resident.
use serde_json::{Value, json};
use serenity::{
    async_trait,
    builder::CreateMessage,
    http::Http,
    model::{channel::Message, gateway::GatewayIntents, id::ChannelId, id::MessageId, id::UserId},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, Notify, oneshot},
    task::JoinSet,
};

pub const INTENTS: GatewayIntents = GatewayIntents::from_bits_truncate(
    GatewayIntents::GUILDS.bits() | GatewayIntents::GUILD_MESSAGES.bits(),
);
const FAILURE: &str = "澪の応答を受け取れませんでした。Pi側で処理が続いている可能性があります。自動再送はしていません。";
const MESSAGE_LIMIT: usize = 1900;

// Deliberately no Debug: this object owns the bot credential.
pub struct Config {
    pub token: String,
    pub guild: u64,
    pub users: HashSet<u64>,
    pub channels: HashSet<u64>,
    pub operator_subject: Option<String>,
    // Who is mentioned, and where, when a task needs the operator's decision.
    // Both None disables those notices.
    pub notify_user: Option<u64>,
    pub notify_channel: Option<u64>,
}
fn snowflakes(value: &str) -> Result<HashSet<u64>, &'static str> {
    value
        .split(',')
        .map(|part| {
            let part = part.trim();
            if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
                return Err("Discord IDs must be positive 64-bit integers");
            }
            part.parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or("Discord IDs must be positive 64-bit integers")
        })
        .collect()
}
/// An explicit ID from `raw` (which must be allowed), else the only allowed one.
fn one_of(raw: &str, allowed: &HashSet<u64>) -> Result<Option<u64>, ()> {
    let raw = raw.trim();
    if !raw.is_empty() {
        let ids = snowflakes(raw).map_err(|_| ())?;
        if ids.len() != 1 || !ids.is_subset(allowed) {
            return Err(());
        }
        return Ok(ids.iter().next().copied());
    }
    Ok(if allowed.len() == 1 {
        allowed.iter().next().copied()
    } else {
        None
    })
}
impl Config {
    pub fn from_env() -> Result<Self, &'static str> {
        Self::parse(|key| std::env::var(key).unwrap_or_default())
    }
    pub fn parse(get: impl Fn(&str) -> String) -> Result<Self, &'static str> {
        let token = get("DISCORD_BOT_TOKEN").trim().to_owned();
        if token.is_empty() {
            return Err("DISCORD_BOT_TOKEN is required");
        }
        let guilds = snowflakes(&get("DISCORD_GUILD_ID"))?;
        if guilds.len() != 1 {
            return Err("DISCORD_GUILD_ID must contain one ID");
        }
        let users = snowflakes(&get("DISCORD_ALLOWED_USER_IDS"))?;
        let channels = snowflakes(&get("DISCORD_ALLOWED_CHANNEL_IDS"))?;
        let operator = get("DISCORD_OPERATOR_SUBJECT").trim().to_owned();
        let operator_subject = if operator.is_empty() {
            None
        } else {
            if operator.len() > 64
                || !operator.as_bytes()[0].is_ascii_alphanumeric()
                || !operator
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            {
                return Err("invalid DISCORD_OPERATOR_SUBJECT");
            }
            if users.len() != 1 {
                return Err("DISCORD_OPERATOR_SUBJECT requires exactly one allowed user");
            }
            Some(operator)
        };
        let (notify_user, notify_channel) =
            match get("DISCORD_NOTIFY").trim().to_ascii_lowercase().as_str() {
                "off" | "0" | "false" | "no" => (None, None),
                _ => (
                    one_of(&get("DISCORD_NOTIFY_USER_ID"), &users).map_err(
                        |_| "DISCORD_NOTIFY_USER_ID must be one of the allowed user IDs",
                    )?,
                    one_of(&get("DISCORD_NOTIFY_CHANNEL_ID"), &channels).map_err(
                        |_| "DISCORD_NOTIFY_CHANNEL_ID must be one of the allowed channel IDs",
                    )?,
                ),
            };
        Ok(Self {
            token,
            guild: *guilds.iter().next().unwrap(),
            users,
            channels,
            operator_subject,
            notify_user,
            notify_channel,
        })
    }
    /// Task notices only run when both targets resolve.
    pub fn notifies(&self) -> bool {
        self.notify_user.is_some() && self.notify_channel.is_some()
    }
    fn allows(&self, m: &Event) -> bool {
        m.guild == Some(self.guild)
            && self.users.contains(&m.user)
            && self.channels.contains(&m.channel)
            && !m.bot
            && !m.webhook
    }
    fn subject(&self, m: &Event) -> String {
        self.operator_subject
            .clone()
            .unwrap_or_else(|| subject_for(m))
    }
}
/// The message a reply points at: its author and text.
#[derive(Clone)]
pub struct Reference {
    pub author: u64,
    pub content: String,
}
#[derive(Clone)]
pub struct Event {
    pub id: u64,
    pub guild: Option<u64>,
    pub channel: u64,
    pub user: u64,
    pub bot: bool,
    pub webhook: bool,
    pub content: String,
    /// `message.reference`: the replied-to message's ID, when this is a reply.
    pub reference: Option<u64>,
    /// The referenced message when Discord resolved it inline; a fetch is
    /// needed otherwise.
    pub resolved: Option<Reference>,
}
impl From<Message> for Event {
    fn from(m: Message) -> Self {
        Self {
            id: m.id.0,
            guild: m.guild_id.map(|g| g.0),
            channel: m.channel_id.0,
            user: m.author.id.0,
            bot: m.author.bot,
            webhook: m.webhook_id.is_some(),
            content: m.content,
            reference: m
                .message_reference
                .as_ref()
                .and_then(|r| r.message_id.map(|id| id.0)),
            resolved: m.referenced_message.map(|r| Reference {
                author: r.author.id.0,
                content: r.content,
            }),
        }
    }
}
pub fn subject_for(m: &Event) -> String {
    let digest =
        Sha256::digest(format!("{}:{}:{}", m.guild.unwrap_or(0), m.channel, m.user).as_bytes());
    format!(
        "discord-{}",
        digest
            .iter()
            .take(24)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
/// Flatten whitespace, then cut at `limit` characters with an ellipsis.
fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        let mut cut: String = flat.chars().take(limit - 1).collect();
        cut.push('…');
        cut
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Text(String),
    /// The full response as an attachment; `note` follows the caption.
    File {
        body: String,
        note: String,
    },
}
pub fn response_reply(response: String, note: &str) -> Reply {
    // UTF-16 code units, matching Discord's server-side count.
    if response.encode_utf16().count() + note.encode_utf16().count() <= MESSAGE_LIMIT {
        Reply::Text(response + note)
    } else {
        Reply::File {
            body: response,
            note: note.to_owned(),
        }
    }
}
/// The resident's answer: the reply plus what it left running.
pub struct Talk {
    pub response: String,
    pub background: Option<Value>,
}
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// Reply to `event`; returns the sent message's ID when known.
    async fn reply(&self, event: &Event, reply: Reply, mention: Option<u64>) -> Option<u64>;
    /// Post `text` in the notice channel, mentioning only the operator.
    async fn notice(&self, text: &str) -> Option<u64>;
    /// The message `message_id` that `event` replies to, when reachable.
    async fn resolve(&self, event: &Event, message_id: u64) -> Option<Reference>;
    async fn talk(&self, text: &str, subject: &str) -> Result<Talk, ResidentError>;
    async fn tasks(&self) -> Result<Vec<Value>, ResidentError>;
    /// Delegated tasks from the task plane (this node's or a peer's).
    async fn delegated(&self) -> Result<Vec<Value>, ResidentError>;
    async fn task_action(&self, body: &Value) -> Result<(u16, Value), ResidentError>;
}
/// A `開始`/`取消` directive (`COMMAND_RE` in the reference implementation).
#[derive(Debug, PartialEq, Eq)]
struct Command {
    start: bool,
    id: Option<String>,
    executor: Option<String>,
}
fn task_id(token: &str) -> bool {
    token.len() >= 7
        && token.len() <= 33
        && matches!(token.as_bytes()[0], b't' | b'T')
        && token[1..].bytes().all(|c| c.is_ascii_hexdigit())
}
fn executor_name(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token.as_bytes()[0].is_ascii_alphanumeric()
        && token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
/// `@澪 開始 t1a0… devin` / `取消` — the ID may come from the notice replied to.
fn parse_command(text: &str) -> Option<Command> {
    let mut tokens = text.split_whitespace();
    let start = match tokens.next()?.to_ascii_lowercase().as_str() {
        "開始" | "start" => true,
        "取消" | "取り消し" | "cancel" => false,
        _ => return None,
    };
    let mut id = None;
    let mut executor = None;
    if let Some(token) = tokens.next() {
        if task_id(token) {
            id = Some(token.to_owned());
            if let Some(token) = tokens.next() {
                if !executor_name(token) {
                    return None;
                }
                executor = Some(token.to_lowercase());
            }
        } else {
            if !executor_name(token) {
                return None;
            }
            executor = Some(token.to_lowercase());
        }
    }
    if tokens.next().is_some() {
        return None;
    }
    Some(Command {
        start,
        id,
        executor,
    })
}
/// Which tasks were already announced, kept across restarts when a state
/// directory is available (systemd `StateDirectory=`).
#[derive(Default)]
pub struct NoticeState {
    path: Option<PathBuf>,
    attention: Vec<(String, Notice)>,
    finished: Vec<(String, u64)>,
    delegated_since: Option<f64>,
}
#[derive(Clone)]
struct Notice {
    message: u64,
    kind: Option<String>,
}
impl NoticeState {
    const KEEP: usize = 500;
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut state = Self {
            path,
            ..Self::default()
        };
        let Some(path) = &state.path else {
            return state;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return state;
        };
        let Ok(saved) = serde_json::from_str::<Value>(&text) else {
            return state;
        };
        if let Some(attention) = saved["attention"].as_object() {
            for (id, entry) in attention {
                if let Some(message) = entry["message"].as_u64() {
                    state.attention.push((
                        id.clone(),
                        Notice {
                            message,
                            kind: entry["kind"].as_str().map(str::to_owned),
                        },
                    ));
                }
            }
        }
        if let Some(finished) = saved["finished"].as_object() {
            for (id, message) in finished {
                if let Some(message) = message.as_u64() {
                    state.finished.push((id.clone(), message));
                }
            }
        }
        state.delegated_since = saved["delegated_since"].as_f64();
        state.trim();
        state
    }
    fn claim(&mut self, task_id: &str, message: u64, kind: Option<String>) {
        if let Some((_, entry)) = self.attention.iter_mut().find(|(id, _)| id == task_id) {
            *entry = Notice { message, kind };
        } else {
            self.attention
                .push((task_id.to_owned(), Notice { message, kind }));
        }
        self.trim();
    }
    fn attention(&self, task_id: &str) -> Option<&Notice> {
        self.attention
            .iter()
            .find(|(id, _)| id == task_id)
            .map(|(_, n)| n)
    }
    fn task_for_message(&self, message_id: u64) -> Option<String> {
        self.attention
            .iter()
            .rev()
            .find(|(_, n)| n.message == message_id)
            .map(|(id, _)| id.clone())
    }
    fn announced(&self, task_id: &str) -> bool {
        self.attention.iter().any(|(id, _)| id == task_id)
    }
    fn finished_seen(&self, task_id: &str) -> bool {
        self.finished.iter().any(|(id, _)| id == task_id)
    }
    fn finish(&mut self, task_id: &str, message: u64) {
        if let Some((_, entry)) = self.finished.iter_mut().find(|(id, _)| id == task_id) {
            *entry = message;
        } else {
            self.finished.push((task_id.to_owned(), message));
        }
        self.trim();
    }
    fn trim(&mut self) {
        while self.attention.len() > Self::KEEP {
            self.attention.remove(0);
        }
        while self.finished.len() > Self::KEEP {
            self.finished.remove(0);
        }
    }
    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let attention: serde_json::Map<String, Value> = self
            .attention
            .iter()
            .map(|(id, n)| (id.clone(), json!({"message": n.message, "kind": n.kind})))
            .collect();
        let finished: serde_json::Map<String, Value> = self
            .finished
            .iter()
            .map(|(id, message)| (id.clone(), json!(message)))
            .collect();
        let payload = json!({
            "attention": Value::Object(attention),
            "finished": Value::Object(finished),
            "delegated_since": self.delegated_since,
        });
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        if std::fs::write(&tmp, payload.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}
fn commitment_actions(task_id: &str) -> String {
    format!(
        "`@澪 開始 {task_id}` で外部エージェントに委譲（`@澪 開始 {task_id} devin` のように実行者も指定可）／ `@澪 取消 {task_id}`"
    )
}
/// A notice for a task waiting on the operator.
fn attention_text(task: &Value, user: u64) -> String {
    let id = task["id"].as_str().unwrap_or("?");
    let detail = if task["detail"].is_object() {
        &task["detail"]
    } else {
        &Value::Null
    };
    match task["kind"].as_str() {
        Some("promise") => format!(
            "<@{user}> 判断が必要です：澪が引き受けた作業がタスクになっていません（{id}）\n依頼：{}\n澪の返事：{}\n→ {}",
            clip(detail["message"].as_str().unwrap_or(""), 300),
            clip(detail["response"].as_str().unwrap_or(""), 300),
            commitment_actions(id)
        ),
        Some("approval") => format!(
            "<@{user}> 承認待ちです（{id}）：{}\n→ デスクトップの「承認」画面で判断してください。",
            clip(task["title"].as_str().unwrap_or(""), 200)
        ),
        _ => format!(
            "<@{user}> 判断待ちのタスクがあります（{id}）：{}",
            clip(task["title"].as_str().unwrap_or(""), 200)
        ),
    }
}
/// A notice for a delegated task that has ended.
fn finished_text(task: &Value, user: u64) -> String {
    let detail = if task["detail"].is_object() {
        &task["detail"]
    } else {
        &Value::Null
    };
    let result = if task["result"].is_object() {
        &task["result"]
    } else {
        &Value::Null
    };
    let state = match task["status"].as_str() {
        Some("done") => "完了",
        Some("failed") => "失敗",
        Some("cancelled") => "取り消し",
        _ => "終了",
    };
    let executor = detail["executor"].as_str().or(task["executor"].as_str());
    let who = executor.map(|e| format!(" · {e}")).unwrap_or_default();
    let request = clip(
        task["title"]
            .as_str()
            .unwrap_or("")
            .strip_prefix("委譲: ")
            .unwrap_or(task["title"].as_str().unwrap_or("")),
        300,
    );
    let mut lines = vec![
        format!(
            "<@{user}> 委譲タスクが{state}しました（{}{who}）",
            task["id"].as_str().unwrap_or("?")
        ),
        format!("依頼：{request}"),
    ];
    let review = &detail["review"];
    if review.is_object() {
        let label = match review["verdict"].as_str() {
            Some("accept") => "採用",
            Some("reject") => "不採用",
            _ => "保留",
        };
        let note = review["note"]
            .as_str()
            .map(|n| format!(" — {}", clip(n, 200)))
            .unwrap_or_default();
        lines.push(format!("澪の評価：{label}{note}"));
    }
    if let Some(summary) = result["summary"].as_str() {
        lines.push(format!("報告：{}", clip(summary, 700)));
    }
    if let Some(branch) = result["branch"].as_str() {
        lines.push(format!(
            "変更はブランチ `{}` にあります。取り込むかはデスクトップの「外部エージェント」で判断してください。",
            clip(branch, 100)
        ));
    }
    lines.join("\n").chars().take(MESSAGE_LIMIT).collect()
}
struct Item {
    event: Event,
    text: String,
    subject: String,
    ack: oneshot::Receiver<bool>,
}
#[derive(Default)]
struct State {
    pending: VecDeque<Item>,
    seen: HashSet<u64>,
    seen_order: VecDeque<u64>,
}
pub struct Bridge<B: Backend> {
    config: Config,
    backend: Arc<B>,
    state: Mutex<State>,
    notices: Mutex<NoticeState>,
    notify: Notify,
    capacity: usize,
}
impl<B: Backend> Bridge<B> {
    pub fn new(config: Config, backend: Arc<B>, capacity: usize) -> Arc<Self> {
        Self::with_notices(config, backend, capacity, NoticeState::default())
    }
    pub fn with_notices(
        config: Config,
        backend: Arc<B>,
        capacity: usize,
        notices: NoticeState,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            backend,
            state: Mutex::new(State::default()),
            notices: Mutex::new(notices),
            notify: Notify::new(),
            capacity,
        })
    }
    fn remember_locked(state: &mut State, id: u64) {
        if state.seen.insert(id) {
            state.seen_order.push_back(id);
            if state.seen_order.len() > 4096
                && let Some(old) = state.seen_order.pop_front()
            {
                state.seen.remove(&old);
            }
        }
    }
    async fn remember(&self, event: &Event) {
        let mut state = self.state.lock().await;
        Self::remember_locked(&mut state, event.id);
    }
    pub async fn handle(&self, event: Event, bot_id: u64) {
        if !self.config.allows(&event) {
            return;
        }
        {
            let state = self.state.lock().await;
            if state.seen.contains(&event.id) {
                return;
            }
        }
        let mention = format!("<@{bot_id}>");
        let nickname = format!("<@!{bot_id}>");
        let mut hint = String::new();
        // Set when this handle already claimed the ID (reply path); the dedup
        // check at enqueue applies to the mention path only.
        let mut claimed = false;
        let text = if event.content.contains(&mention) || event.content.contains(&nickname) {
            event
                .content
                .replace(&mention, "")
                .replace(&nickname, "")
                .trim()
                .to_owned()
        } else {
            // A reply to one of the bot's own messages is addressed to it even
            // without a mention. Claim the ID first: resolving the reference
            // awaits, and a Gateway replay must not slip a second copy through.
            let Some(reference_id) = event.reference else {
                return;
            };
            self.remember(&event).await;
            claimed = true;
            let referenced = match event.resolved.clone() {
                Some(r) => Some(r),
                None => self.backend.resolve(&event, reference_id).await,
            };
            let Some(referenced) = referenced.filter(|r| r.author == bot_id) else {
                return;
            };
            let quoted = clip(&referenced.content, 300);
            if !quoted.is_empty() {
                hint = format!("[reply to: {quoted}]\n");
            }
            event.content.trim().to_owned()
        };
        if text.is_empty() || hint.len() + text.len() > 32768 {
            self.backend
                .reply(
                    &event,
                    Reply::Text(
                        "メンションか澪の発言への返信に続けて、32KiB以内の文章を送ってください。"
                            .into(),
                    ),
                    None,
                )
                .await;
            return;
        }
        if let Some(command) = parse_command(&text) {
            self.remember(&event).await;
            self.command(&event, command).await;
            return;
        }
        let mut state = self.state.lock().await;
        if !claimed && state.seen.contains(&event.id) {
            return;
        }
        if state.pending.len() >= self.capacity {
            drop(state);
            self.backend
                .reply(
                    &event,
                    Reply::Text(
                        "待ち行列がいっぱいです。少し後で新しいメッセージを送ってください。".into(),
                    ),
                    None,
                )
                .await;
            return;
        }
        let (tx, rx) = oneshot::channel();
        state.pending.push_back(Item {
            subject: self.config.subject(&event),
            event: event.clone(),
            text: hint + &text,
            ack: rx,
        });
        Self::remember_locked(&mut state, event.id);
        let waiting = state.pending.len();
        drop(state);
        self.notify.notify_one();
        let accepted = self
            .backend
            .reply(
                &event,
                Reply::Text(format!(
                    "受け付けました。順番に返事します（待ち {waiting} 件）。"
                )),
                None,
            )
            .await
            .is_some();
        let _ = tx.send(accepted);
    }
    /// Start or cancel a commitment the operator was told about.
    async fn command(&self, event: &Event, command: Command) {
        let task_id = match command.id {
            Some(id) => Some(id),
            None => {
                let notices = self.notices.lock().await;
                event.reference.and_then(|id| notices.task_for_message(id))
            }
        };
        let Some(task_id) = task_id else {
            self.backend
                .reply(
                    event,
                    Reply::Text(
                        "どのタスクか分かりませんでした。通知に返信するか、`@澪 開始 t…` のようにIDを付けてください。"
                            .into(),
                    ),
                    None,
                )
                .await;
            return;
        };
        {
            let notices = self.notices.lock().await;
            if let Some(entry) = notices.attention(&task_id)
                && !matches!(entry.kind.as_deref(), None | Some("promise"))
            {
                drop(notices);
                self.backend
                    .reply(
                        event,
                        Reply::Text(format!("{task_id} は開始・取消できる引受ではありません。")),
                        None,
                    )
                    .await;
                return;
            }
        }
        let body = if command.start {
            let mut body = json!({"action": "start", "id": task_id});
            if let Some(executor) = &command.executor {
                body["executor"] = json!(executor);
            }
            body
        } else {
            let tasks = self.backend.tasks().await.unwrap_or_default();
            let found = tasks.iter().any(|t| {
                t["id"].as_str() == Some(task_id.as_str())
                    && t["kind"].as_str() == Some("promise")
                    && t["status"].as_str() == Some("awaiting_operator")
            });
            if !found {
                self.backend
                    .reply(
                        event,
                        Reply::Text(format!(
                            "{task_id} は未着手の引受として見つかりませんでした（既に処理済みかもしれません）。"
                        )),
                        None,
                    )
                    .await;
                return;
            }
            json!({"action": "update", "id": task_id, "status": "cancelled",
                   "note": "操作者が取り消し（Discord）"})
        };
        let (status, payload) = match self.backend.task_action(&body).await {
            Ok(result) => result,
            Err(ResidentError) => {
                self.backend
                    .reply(
                        event,
                        Reply::Text("Pi側に届きませんでした。自動再送はしていません。".into()),
                        None,
                    )
                    .await;
                return;
            }
        };
        if status != 200 {
            let reason = match status {
                404 => "見つからないか、外部エージェントの実行ノードに届きません".to_owned(),
                409 => "既に処理済みです".to_owned(),
                other => format!("HTTP {other}"),
            };
            self.backend
                .reply(
                    event,
                    Reply::Text(format!(
                        "{task_id} を{}できませんでした（{reason}）。詳細はデスクトップのタスク画面で確認してください。",
                        if command.start { "開始" } else { "取り消し" }
                    )),
                    None,
                )
                .await;
            return;
        }
        let text = if command.start {
            let delegated = &payload["delegated"];
            format!(
                "委譲しました → {}（{} @ {}）。終わったらここでお知らせします。",
                delegated["task_id"].as_str().unwrap_or("?"),
                delegated["executor"].as_str().unwrap_or("?"),
                payload["node"].as_str().unwrap_or("?")
            )
        } else {
            format!("{task_id} を取り消しました。")
        };
        self.backend.reply(event, Reply::Text(text), None).await;
    }
    /// What the reply left running, as a line under it, and the commitment it
    /// announces (if any).
    fn background_note(&self, background: &Option<Value>) -> (String, Option<String>) {
        let Some(background) = background else {
            return (String::new(), None);
        };
        match background["state"].as_str() {
            Some("started") => {
                let labels = background["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t.is_object())
                    .map(|t| {
                        format!(
                            "{}{}",
                            t["id"].as_str().unwrap_or("?"),
                            t["executor"]
                                .as_str()
                                .map(|e| format!("（{e}）"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("、");
                (
                    format!("\n\n▶ 裏で実行中：{labels}。終わったらお知らせします。"),
                    None,
                )
            }
            Some("unstarted") => match (background["task_id"].as_str(), self.config.notifies()) {
                (Some(task_id), true) => (
                    format!(
                        "\n\n⚠ <@{}> 引き受けましたが、タスクはまだ作成されていません（{task_id}）。\n→ {}",
                        self.config.notify_user.unwrap_or(0),
                        commitment_actions(task_id)
                    ),
                    Some(task_id.to_owned()),
                ),
                _ => (String::new(), None),
            },
            _ => (String::new(), None),
        }
    }
    async fn answer(&self, item: Item) {
        if !item.ack.await.unwrap_or(false) {
            return;
        }
        match self.backend.talk(&item.text, &item.subject).await {
            Ok(talk) => {
                let (note, commitment) = self.background_note(&talk.background);
                let mention = commitment.as_ref().and(self.config.notify_user);
                let reply = response_reply(talk.response, &note);
                if let (Some(commitment), Some(sent)) = (
                    commitment,
                    self.backend.reply(&item.event, reply, mention).await,
                ) {
                    let mut notices = self.notices.lock().await;
                    notices.claim(&commitment, sent, Some("promise".to_owned()));
                    notices.save();
                }
            }
            Err(ResidentError) => {
                self.backend
                    .reply(&item.event, Reply::Text(FAILURE.into()), None)
                    .await;
            }
        }
    }
    /// Fixed two-worker scheduler; never spawn a dialogue task per gateway event.
    pub async fn run(self: Arc<Self>) {
        let mut running = JoinSet::new();
        let mut active = HashSet::new();
        loop {
            while running.len() < 2 {
                let mut state = self.state.lock().await;
                let Some(index) = state
                    .pending
                    .iter()
                    .position(|i| !active.contains(&i.subject))
                else {
                    break;
                };
                let item = state.pending.remove(index).unwrap();
                let subject = item.subject.clone();
                active.insert(subject.clone());
                let bridge = self.clone();
                running.spawn(async move {
                    bridge.answer(item).await;
                    subject
                });
            }
            tokio::select! {
                _ = self.notify.notified() => {},
                result = running.join_next(), if !running.is_empty() => {
                    match result { Some(Ok(subject)) => { active.remove(&subject); }, Some(Err(_)) => { eprintln!("Discord worker stopped unexpectedly"); return; }, None => {} }
                }
            }
        }
    }
    /// Tells the operator, with a mention, when a task needs their decision
    /// or delegated work has ended. Polls the resident; sends nothing else.
    pub async fn notice_loop(self: Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(30);
        loop {
            if self.poll_notices().await.is_err() {
                eprintln!("notice poll failed");
            }
            tokio::time::sleep(EVERY).await;
        }
    }
    async fn poll_notices(&self) -> Result<(), ResidentError> {
        // A promise made in a Discord turn is announced in that turn's reply.
        const GRACE: f64 = 90.0;
        // How long a finished delegated task waits for the individual's review.
        const REVIEW_WAIT: f64 = 600.0;
        let Some(user) = self.config.notify_user else {
            return Ok(());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut changed = false;
        for task in self.backend.tasks().await? {
            let Some(id) = task["id"].as_str() else {
                continue;
            };
            if task["status"].as_str() != Some("awaiting_operator")
                || self.notices.lock().await.announced(id)
            {
                continue;
            }
            if task["kind"].as_str() == Some("promise")
                && now - task["created_at"].as_f64().unwrap_or(0.0) < GRACE
            {
                continue;
            }
            if let Some(message) = self.backend.notice(&attention_text(&task, user)).await {
                self.notices.lock().await.claim(
                    id,
                    message,
                    task["kind"].as_str().map(str::to_owned),
                );
                changed = true;
            }
        }
        if let Ok(delegated) = self.backend.delegated().await {
            {
                let mut notices = self.notices.lock().await;
                if notices.delegated_since.is_none() {
                    // Start from now: never announce what ended before notices existed.
                    notices.delegated_since = Some(now);
                    changed = true;
                }
            }
            for task in delegated {
                let Some(id) = task["id"].as_str() else {
                    continue;
                };
                let status = task["status"].as_str().unwrap_or("");
                let finished = task["finished_at"].as_f64().unwrap_or(0.0);
                if !matches!(status, "done" | "failed" | "cancelled") {
                    continue;
                }
                {
                    let notices = self.notices.lock().await;
                    if notices.finished_seen(id)
                        || finished < notices.delegated_since.unwrap_or(f64::MAX)
                    {
                        continue;
                    }
                }
                let detail = &task["detail"];
                if status == "done" && !detail["review"].is_object() && now - finished < REVIEW_WAIT
                {
                    continue;
                }
                if let Some(message) = self.backend.notice(&finished_text(&task, user)).await {
                    self.notices.lock().await.finish(id, message);
                    changed = true;
                }
            }
        }
        if changed {
            self.notices.lock().await.save();
        }
        Ok(())
    }
}

/// Client has no proxy inheritance, redirect following, or automatic retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentError;

pub struct Resident {
    client: reqwest::Client,
    url: String,
}
impl Resident {
    pub fn new(url: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(660))
                .build()?,
            url: url.into(),
        })
    }
    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        total: u64,
    ) -> Result<(u16, Value), ResidentError> {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.url, path))
            .timeout(Duration::from_secs(total));
        if let Some(body) = body {
            request = request.json(body);
        }
        let mut response = request.send().await.map_err(|_| ResidentError)?;
        let status = response.status().as_u16();
        let mut raw = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ResidentError)? {
            if raw.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(ResidentError);
            }
            raw.extend_from_slice(&chunk);
        }
        let payload: Value = serde_json::from_slice(&raw).map_err(|_| ResidentError)?;
        Ok((
            status,
            if payload.is_object() {
                payload
            } else {
                json!({})
            },
        ))
    }
    fn task_list(payload: &Value) -> Result<Vec<Value>, ResidentError> {
        Ok(payload["tasks"]
            .as_array()
            .ok_or(ResidentError)?
            .iter()
            .filter(|t| t.is_object())
            .cloned()
            .collect())
    }
    pub async fn tasks(&self) -> Result<Vec<Value>, ResidentError> {
        let (status, payload) = self
            .json(reqwest::Method::GET, "/v1/tasks", None, 30)
            .await?;
        if status != 200 {
            return Err(ResidentError);
        }
        Self::task_list(&payload)
    }
    pub async fn delegated(&self) -> Result<Vec<Value>, ResidentError> {
        let (status, payload) = self
            .json(
                reqwest::Method::POST,
                "/v1/agents",
                Some(&json!({"action": "panel"})),
                60,
            )
            .await?;
        if status != 200 {
            return Err(ResidentError);
        }
        Self::task_list(&payload)
    }
    pub async fn task_action(&self, body: &Value) -> Result<(u16, Value), ResidentError> {
        self.json(reqwest::Method::POST, "/v1/tasks", Some(body), 120)
            .await
    }
    pub async fn talk(&self, text: &str, subject: &str) -> Result<Talk, ResidentError> {
        let mut response = self
            .client
            .post(format!("{}/v1/kamimusuhi/talk", self.url))
            .json(&json!({"message":text,"subject":subject}))
            .send()
            .await
            .map_err(|_| ResidentError)?;
        if response.status() != 200 {
            return Err(ResidentError);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ResidentError)? {
            if body.len() + chunk.len() > 1024 * 1024 {
                return Err(ResidentError);
            }
            body.extend_from_slice(&chunk);
        }
        let payload: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| ResidentError)?;
        let response = payload
            .get("response")
            .and_then(|r| r.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or(ResidentError)?;
        let background = payload.get("background").filter(|b| b.is_object()).cloned();
        Ok(Talk {
            response,
            background,
        })
    }
}
/// Explicit empty mention parsing applies to acknowledgments, errors and files alike.
pub fn message_builder(
    reply: &Reply,
    event: &Event,
    mention: Option<u64>,
) -> CreateMessage<'static> {
    let mut builder = CreateMessage::default();
    builder
        .reference_message((
            ChannelId(event.channel),
            serenity::model::id::MessageId(event.id),
        ))
        .allowed_mentions(|m| {
            let m = m.empty_parse().replied_user(false);
            match mention {
                Some(user) => m.users([UserId(user)]),
                None => m,
            }
        })
        .flags(serenity::model::channel::MessageFlags::SUPPRESS_EMBEDS);
    match reply {
        Reply::Text(text) => {
            builder.content(text);
        }
        Reply::File { body, note } => {
            builder
                .content(format!("長い応答をファイルにまとめました。{note}"))
                .add_file(serenity::model::channel::AttachmentType::Bytes {
                    data: std::borrow::Cow::Owned(body.as_bytes().to_vec()),
                    filename: "mio-response.txt".into(),
                });
        }
    }
    builder
}
pub struct LiveBackend {
    pub http: Arc<Http>,
    pub resident: Resident,
    /// The notice (channel, operator) pair, when task notices are enabled.
    pub notify: Option<(u64, u64)>,
}
#[async_trait]
impl Backend for LiveBackend {
    async fn reply(&self, event: &Event, reply: Reply, mention: Option<u64>) -> Option<u64> {
        let result = ChannelId(event.channel)
            .send_message(&self.http, |m| {
                *m = message_builder(&reply, event, mention);
                m
            })
            .await;
        if result.is_err() {
            eprintln!("Discord reply failed");
        }
        result.ok().map(|m| m.id.0)
    }
    async fn notice(&self, text: &str) -> Option<u64> {
        let (channel, user) = self.notify?;
        let content: String = text.chars().take(MESSAGE_LIMIT).collect();
        let result = ChannelId(channel)
            .send_message(&self.http, |m| {
                m.content(content)
                    .allowed_mentions(|a| a.empty_parse().replied_user(false).users([UserId(user)]))
                    .flags(serenity::model::channel::MessageFlags::SUPPRESS_EMBEDS)
            })
            .await;
        if result.is_err() {
            eprintln!("Discord notice failed");
        }
        result.ok().map(|m| m.id.0)
    }
    async fn resolve(&self, event: &Event, message_id: u64) -> Option<Reference> {
        match ChannelId(event.channel)
            .message(&self.http, MessageId(message_id))
            .await
        {
            Ok(m) => Some(Reference {
                author: m.author.id.0,
                content: m.content,
            }),
            Err(_) => {
                eprintln!("reply reference lookup failed");
                None
            }
        }
    }
    async fn talk(&self, text: &str, subject: &str) -> Result<Talk, ResidentError> {
        self.resident.talk(text, subject).await
    }
    async fn tasks(&self) -> Result<Vec<Value>, ResidentError> {
        self.resident.tasks().await
    }
    async fn delegated(&self) -> Result<Vec<Value>, ResidentError> {
        self.resident.delegated().await
    }
    async fn task_action(&self, body: &Value) -> Result<(u16, Value), ResidentError> {
        self.resident.task_action(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    fn config() -> Config {
        Config::parse(|key| {
            match key {
                "DISCORD_BOT_TOKEN" => "fake",
                "DISCORD_GUILD_ID" => "10",
                "DISCORD_ALLOWED_USER_IDS" => "20,21,22",
                "DISCORD_ALLOWED_CHANNEL_IDS" => "30",
                _ => "",
            }
            .into()
        })
        .unwrap()
    }
    /// A single-user, single-channel config: notices resolve by default.
    fn notifying() -> Config {
        Config::parse(|key| {
            match key {
                "DISCORD_BOT_TOKEN" => "fake",
                "DISCORD_GUILD_ID" => "10",
                "DISCORD_ALLOWED_USER_IDS" => "20",
                "DISCORD_ALLOWED_CHANNEL_IDS" => "30",
                _ => "",
            }
            .into()
        })
        .unwrap()
    }
    fn event(id: u64, user: u64, content: &str) -> Event {
        Event {
            id,
            guild: Some(10),
            channel: 30,
            user,
            bot: false,
            webhook: false,
            content: content.into(),
            reference: None,
            resolved: None,
        }
    }
    fn reference(author: u64, content: &str) -> Option<Reference> {
        Some(Reference {
            author,
            content: content.into(),
        })
    }
    #[derive(Default)]
    struct Mock {
        replies: Mutex<Vec<(u64, Reply, Option<u64>)>>,
        sent: AtomicU64,
        notices: Mutex<Vec<String>>,
        calls: Mutex<Vec<(String, String)>>,
        resolves: AtomicUsize,
        resolve_wait: AtomicBool,
        resolve_gate: Notify,
        resolve_result: Mutex<Option<Reference>>,
        actions: Mutex<Vec<Value>>,
        action_result: Mutex<(u16, Value)>,
        action_fail: AtomicBool,
        listed: Mutex<Vec<Value>>,
        listed_fail: AtomicBool,
        delegated_list: Mutex<Vec<Value>>,
        delegated_fail: AtomicBool,
        ack_fail: AtomicBool,
        reply_fail: AtomicBool,
        talk_fail: AtomicBool,
        talk_background: Mutex<Option<Value>>,
        gate: AtomicBool,
        release: Notify,
        active: AtomicUsize,
        maximum: AtomicUsize,
    }
    #[async_trait]
    impl Backend for Mock {
        async fn reply(&self, event: &Event, reply: Reply, mention: Option<u64>) -> Option<u64> {
            let ack = matches!(&reply, Reply::Text(t) if t.starts_with("受け付け"));
            self.replies.lock().await.push((event.id, reply, mention));
            if if ack {
                self.ack_fail.load(Ordering::SeqCst)
            } else {
                self.reply_fail.load(Ordering::SeqCst)
            } {
                None
            } else {
                Some(self.sent.fetch_add(1, Ordering::SeqCst) + 600)
            }
        }
        async fn notice(&self, text: &str) -> Option<u64> {
            self.notices.lock().await.push(text.into());
            Some(self.sent.fetch_add(1, Ordering::SeqCst) + 600)
        }
        async fn resolve(&self, _: &Event, _: u64) -> Option<Reference> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            if self.resolve_wait.load(Ordering::SeqCst) {
                self.resolve_gate.notified().await;
            }
            self.resolve_result.lock().await.clone()
        }
        async fn talk(&self, text: &str, subject: &str) -> Result<Talk, ResidentError> {
            self.calls.lock().await.push((text.into(), subject.into()));
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            if self.gate.load(Ordering::SeqCst) {
                self.release.notified().await;
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            if self.talk_fail.swap(false, Ordering::SeqCst) {
                Err(ResidentError)
            } else {
                Ok(Talk {
                    response: text.into(),
                    background: self.talk_background.lock().await.clone(),
                })
            }
        }
        async fn tasks(&self) -> Result<Vec<Value>, ResidentError> {
            if self.listed_fail.load(Ordering::SeqCst) {
                return Err(ResidentError);
            }
            Ok(self.listed.lock().await.clone())
        }
        async fn delegated(&self) -> Result<Vec<Value>, ResidentError> {
            if self.delegated_fail.load(Ordering::SeqCst) {
                return Err(ResidentError);
            }
            Ok(self.delegated_list.lock().await.clone())
        }
        async fn task_action(&self, body: &Value) -> Result<(u16, Value), ResidentError> {
            self.actions.lock().await.push(body.clone());
            if self.action_fail.load(Ordering::SeqCst) {
                return Err(ResidentError);
            }
            Ok(self.action_result.lock().await.clone())
        }
    }
    async fn wait_calls(mock: &Mock, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if mock.calls.lock().await.len() >= count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn wait_replies(mock: &Mock, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if mock.replies.lock().await.len() >= count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn config_fails_closed() {
        for bad in ["", "0", "-1", "bad", "18446744073709551616", "10,"] {
            assert!(snowflakes(bad).is_err());
        }
        for key in [
            "DISCORD_BOT_TOKEN",
            "DISCORD_GUILD_ID",
            "DISCORD_ALLOWED_USER_IDS",
            "DISCORD_ALLOWED_CHANNEL_IDS",
        ] {
            assert!(Config::parse(|k| if k == key { "" } else { "10" }.into()).is_err());
        }
        for subject in ["--dir", "a b", "a@b", &"x".repeat(65)] {
            assert!(
                Config::parse(|k| match k {
                    "DISCORD_BOT_TOKEN" => "fake",
                    "DISCORD_OPERATOR_SUBJECT" => subject,
                    _ => "10",
                }
                .into())
                .is_err()
            );
        }
        assert!(
            Config::parse(|k| match k {
                "DISCORD_BOT_TOKEN" => "fake",
                "DISCORD_OPERATOR_SUBJECT" => "eightman",
                "DISCORD_ALLOWED_USER_IDS" => "20,21",
                _ => "10",
            }
            .into())
            .is_err()
        );
        assert!(
            Config::parse(|k| match k {
                "DISCORD_BOT_TOKEN" => "fake",
                "DISCORD_NOTIFY_USER_ID" => "99",
                _ => "10",
            }
            .into())
            .is_err()
        );
    }
    #[test]
    fn notify_defaults_and_opt_out() {
        let single = notifying();
        assert!(single.notifies());
        assert_eq!(single.notify_user, Some(20));
        assert_eq!(single.notify_channel, Some(30));
        for off in ["off", "0", "false", "NO"] {
            let c = Config::parse(|k| {
                match k {
                    "DISCORD_BOT_TOKEN" => "fake",
                    "DISCORD_NOTIFY" => off,
                    _ => "10",
                }
                .into()
            })
            .unwrap();
            assert!(!c.notifies());
        }
        // Several allowed users: an explicit target is required, else no notices.
        let multi = config();
        assert!(!multi.notifies());
        let pinned = Config::parse(|k| {
            match k {
                "DISCORD_BOT_TOKEN" => "fake",
                "DISCORD_GUILD_ID" => "10",
                "DISCORD_ALLOWED_USER_IDS" => "20,21",
                "DISCORD_ALLOWED_CHANNEL_IDS" => "30",
                "DISCORD_NOTIFY_USER_ID" => "21",
                _ => "",
            }
            .into()
        })
        .unwrap();
        assert_eq!(pinned.notify_user, Some(21));
        assert!(pinned.notifies());
    }
    #[test]
    fn subject_stable_and_operator_mapping() {
        let first = event(1, 20, "");
        assert_eq!(
            subject_for(&first),
            "discord-1db16fc1b60adabf90ce30dd665e9e0a49a7449d7df282f0"
        );
        let mut subjects = HashSet::from([subject_for(&first)]);
        let mut m = first.clone();
        m.guild = Some(11);
        subjects.insert(subject_for(&m));
        m = first.clone();
        m.channel = 31;
        subjects.insert(subject_for(&m));
        subjects.insert(subject_for(&event(1, 21, "")));
        assert_eq!(subjects.len(), 4);
        assert!(subjects.iter().all(|s| s.len() == 56));
        let mut c = config();
        c.operator_subject = Some("eightman".into());
        assert_eq!(c.subject(&first), "eightman");
        assert!(!INTENTS.contains(GatewayIntents::MESSAGE_CONTENT));
        assert!(!INTENTS.contains(GatewayIntents::GUILD_MEMBERS));
    }
    #[tokio::test]
    async fn unauthorized_messages_ignored() {
        let mock = Arc::new(Mock::default());
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let base = event(1, 20, "<@99> hello");
        let mut inputs = vec![event(1, 20, "hello"), event(1, 20, "<@98> hi")];
        let mut m = base.clone();
        m.guild = None;
        inputs.push(m);
        let mut m = base.clone();
        m.guild = Some(11);
        inputs.push(m);
        let mut m = base.clone();
        m.channel = 31;
        inputs.push(m);
        let mut m = base.clone();
        m.user = 23;
        inputs.push(m);
        let mut m = base.clone();
        m.bot = true;
        inputs.push(m);
        let mut m = base;
        m.webhook = true;
        inputs.push(m);
        for e in inputs {
            bridge.handle(e, 99).await;
        }
        assert!(mock.replies.lock().await.is_empty());
    }
    #[tokio::test]
    async fn ack_then_reply_and_dedupe() {
        let mock = Arc::new(Mock::default());
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        let m = event(1, 20, "<@!99> こんにちは @everyone");
        tokio::join!(bridge.handle(m.clone(), 99), bridge.handle(m.clone(), 99));
        wait_replies(&mock, 2).await;
        bridge.handle(m, 99).await;
        assert_eq!(mock.calls.lock().await.len(), 1);
        assert_eq!(mock.calls.lock().await[0].0, "こんにちは @everyone");
        assert_eq!(mock.replies.lock().await.len(), 2);
        worker.abort();
    }
    #[test]
    fn outgoing_mentions_embeds_and_attachment() {
        let m = event(1, 20, "");
        let reply = response_reply("😀".repeat(1800) + "\n日本語", "");
        assert!(matches!(reply, Reply::File { .. }));
        let b = message_builder(&reply, &m, None);
        assert_eq!(
            b.0["allowed_mentions"],
            json!({"parse":[],"replied_user":false})
        );
        assert_eq!(b.0["flags"], json!(4));
        match &b.2[0] {
            serenity::model::channel::AttachmentType::Bytes { data, filename } => {
                assert_eq!(filename, "mio-response.txt");
                assert_eq!(
                    std::str::from_utf8(data).unwrap(),
                    "😀".repeat(1800) + "\n日本語"
                );
            }
            _ => panic!("expected bytes"),
        }
        let b = message_builder(&Reply::Text("x".into()), &m, Some(20));
        assert_eq!(
            b.0["allowed_mentions"],
            json!({"parse":[],"replied_user":false,"users":["20"]})
        );
    }
    #[tokio::test]
    async fn capacity_and_invalid_input() {
        let mock = Arc::new(Mock::default());
        let bridge = Bridge::new(config(), mock.clone(), 1);
        bridge.handle(event(1, 20, "<@99> first"), 99).await;
        bridge.handle(event(2, 20, "<@99> second"), 99).await;
        assert!(!bridge.state.lock().await.seen.contains(&2));
        assert_eq!(bridge.state.lock().await.pending.len(), 1);
        for s in ["<@99>  ".into(), format!("<@99>{}", "あ".repeat(11000))] {
            bridge.handle(event(3, 20, &s), 99).await;
        }
        assert!(mock.calls.lock().await.is_empty());
    }
    #[tokio::test]
    async fn fifo_one_subject_and_pool_limit() {
        let mock = Arc::new(Mock::default());
        mock.gate.store(true, Ordering::SeqCst);
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        bridge.handle(event(1, 20, "<@99> first"), 99).await;
        wait_calls(&mock, 1).await;
        bridge.handle(event(2, 20, "<@99> second"), 99).await;
        bridge.handle(event(3, 20, "<@99> third"), 99).await;
        bridge.handle(event(4, 21, "<@99> other"), 99).await;
        bridge.handle(event(5, 22, "<@99> last"), 99).await;
        wait_calls(&mock, 2).await;
        assert_eq!(mock.maximum.load(Ordering::SeqCst), 2);
        assert_eq!(mock.calls.lock().await[1].0, "other");
        mock.gate.store(false, Ordering::SeqCst);
        mock.release.notify_waiters();
        wait_replies(&mock, 10).await;
        let order: Vec<_> = mock
            .calls
            .lock()
            .await
            .iter()
            .filter(|(_, s)| s == &subject_for(&event(1, 20, "")))
            .map(|(t, _)| t.clone())
            .collect();
        assert_eq!(order, ["first", "second", "third"]);
        assert_eq!(mock.maximum.load(Ordering::SeqCst), 2);
        worker.abort();
    }
    #[tokio::test]
    async fn failure_no_retry_and_worker_survives() {
        let mock = Arc::new(Mock::default());
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        mock.ack_fail.store(true, Ordering::SeqCst);
        bridge.handle(event(1, 20, "<@99> skipped"), 99).await;
        mock.ack_fail.store(false, Ordering::SeqCst);
        mock.talk_fail.store(true, Ordering::SeqCst);
        bridge.handle(event(2, 20, "<@99> fails"), 99).await;
        wait_replies(&mock, 3).await;
        mock.reply_fail.store(true, Ordering::SeqCst);
        bridge.handle(event(3, 20, "<@99> done"), 99).await;
        wait_replies(&mock, 5).await;
        assert_eq!(mock.calls.lock().await.len(), 2);
        assert_eq!(mock.calls.lock().await[0].0, "fails");
        assert!(
            mock.replies.lock().await.iter().any(
                |(_, r, _)| matches!(r,Reply::Text(t) if t.contains("自動再送はしていません"))
            )
        );
        assert!(!worker.is_finished());
        worker.abort();
    }
    #[tokio::test]
    async fn reply_to_bot_is_admitted_with_hint() {
        let mock = Arc::new(Mock::default());
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        let mut m = event(1, 20, "そうなんだ");
        m.reference = Some(800);
        m.resolved = reference(99, "受け付けました。");
        bridge.handle(m, 99).await;
        wait_calls(&mock, 1).await;
        assert_eq!(
            mock.calls.lock().await[0].0,
            "[reply to: 受け付けました。]\nそうなんだ"
        );
        worker.abort();
    }
    #[tokio::test]
    async fn replies_to_others_and_missing_parents_are_ignored() {
        let mock = Arc::new(Mock::default());
        *mock.resolve_result.lock().await = reference(20, "x");
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let mut m = event(1, 20, "そうなんだ");
        m.reference = Some(800);
        m.resolved = reference(20, "別の人の発言");
        bridge.handle(m.clone(), 99).await;
        // No resolved message: the fetch path runs, finds another author.
        m.id = 2;
        m.resolved = None;
        bridge.handle(m, 99).await;
        assert!(mock.replies.lock().await.is_empty());
        assert!(mock.calls.lock().await.is_empty());
        assert_eq!(mock.resolves.load(Ordering::SeqCst), 1);
        // A replay after the failed fetch is deduped without another fetch.
        let mut again = event(3, 20, "そうなんだ");
        again.reference = Some(800);
        let first = bridge.handle(again.clone(), 99);
        let second = bridge.handle(again, 99);
        tokio::join!(first, second);
        assert_eq!(mock.resolves.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn reply_reference_fetch_is_deduped() {
        let mock = Arc::new(Mock::default());
        mock.resolve_wait.store(true, Ordering::SeqCst);
        *mock.resolve_result.lock().await = reference(99, "前の返事");
        let bridge = Bridge::new(config(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        let mut m = event(1, 20, "続きを頼む");
        m.reference = Some(801);
        let first = tokio::spawn({
            let bridge = bridge.clone();
            let m = m.clone();
            async move { bridge.handle(m, 99).await }
        });
        let second = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.handle(m, 99).await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        mock.resolve_gate.notify_waiters();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(mock.resolves.load(Ordering::SeqCst), 1);
        wait_calls(&mock, 1).await;
        assert_eq!(
            mock.calls.lock().await[0].0,
            "[reply to: 前の返事]\n続きを頼む"
        );
        worker.abort();
    }
    #[test]
    fn command_parsing() {
        let verb = |start| Command {
            start,
            id: None,
            executor: None,
        };
        for word in ["開始", "START", "start"] {
            assert_eq!(parse_command(word), Some(verb(true)));
        }
        for word in ["取消", "取り消し", "cancel", "Cancel"] {
            assert_eq!(parse_command(word), Some(verb(false)));
        }
        assert_eq!(
            parse_command("開始 t1a0dc0000c1 devin"),
            Some(Command {
                start: true,
                id: Some("t1a0dc0000c1".into()),
                executor: Some("devin".into()),
            })
        );
        // An executor without an ID resolves the task from the replied notice.
        assert_eq!(
            parse_command("開始 Devin"),
            Some(Command {
                start: true,
                id: None,
                executor: Some("devin".into()),
            })
        );
        // One bare word after the verb is the executor, not a task ID.
        assert_eq!(
            parse_command("開始 bad"),
            Some(Command {
                start: true,
                id: None,
                executor: Some("bad".into()),
            })
        );
        for bad in [
            "",
            "こんにちは",
            "開始する",
            "開始 t1a0dc0000c1 bad name",
            "開始 t1a0dc0000c1 devin extra",
            "開始 bad executor extra",
        ] {
            assert_eq!(parse_command(bad), None, "{bad}");
        }
    }
    #[tokio::test]
    async fn command_reply_to_notice_needs_no_mention() {
        let mock = Arc::new(Mock::default());
        *mock.listed.lock().await = vec![json!({"id": "t1a0dc0000c1",
            "kind": "promise", "status": "awaiting_operator"})];
        *mock.action_result.lock().await = (200, json!({"ok": true}));
        let mut notices = NoticeState::default();
        notices.claim("t1a0dc0000c1", 700, Some("promise".into()));
        let bridge = Bridge::with_notices(notifying(), mock.clone(), 16, notices);
        let mut m = event(1, 20, "取消");
        m.reference = Some(700);
        m.resolved = reference(99, "判断が必要です");
        bridge.handle(m, 99).await;
        assert_eq!(
            mock.actions.lock().await.as_slice(),
            [json!({"action": "update", "id": "t1a0dc0000c1",
                    "status": "cancelled", "note": "操作者が取り消し（Discord）"})]
        );
        assert!(
            mock.replies
                .lock()
                .await
                .iter()
                .any(|(_, r, _)| matches!(r, Reply::Text(t) if t.contains("取り消しました")))
        );
        assert!(mock.calls.lock().await.is_empty());
    }
    #[tokio::test]
    async fn command_start_and_failures() {
        let mock = Arc::new(Mock::default());
        *mock.action_result.lock().await = (
            200,
            json!({"ok": true, "node": "pi",
                   "delegated": {"task_id": "tzzz", "executor": "devin"}}),
        );
        let bridge = Bridge::new(notifying(), mock.clone(), 16);
        bridge
            .handle(event(1, 20, "<@99> 開始 t1a0dc0000c1 Devin"), 99)
            .await;
        assert_eq!(
            mock.actions.lock().await.as_slice(),
            [json!({"action": "start", "id": "t1a0dc0000c1", "executor": "devin"})]
        );
        // Unknown task ID via a plain reply (the referenced message is not a
        // notice) is explained, not queued.
        let mut m = event(2, 20, "開始");
        m.reference = Some(999);
        m.resolved = reference(99, "任意の返事");
        bridge.handle(m, 99).await;
        // Not-a-promise tasks cannot be cancelled from Discord.
        *mock.listed.lock().await = vec![json!({"id": "t1a0dc0000c1",
            "kind": "approval", "status": "awaiting_operator"})];
        bridge
            .handle(event(3, 20, "<@99> 取消 t1a0dc0000c1"), 99)
            .await;
        // The upstream node reports a conflict.
        *mock.listed.lock().await = vec![json!({"id": "t1a0dc0000c1",
            "kind": "promise", "status": "awaiting_operator"})];
        *mock.action_result.lock().await = (409, json!({}));
        bridge
            .handle(event(4, 20, "<@99> 取消 t1a0dc0000c1"), 99)
            .await;
        // The resident is unreachable.
        mock.action_fail.store(true, Ordering::SeqCst);
        bridge
            .handle(event(5, 20, "<@99> 開始 t1a0dc0000c1"), 99)
            .await;
        let texts: Vec<String> = mock
            .replies
            .lock()
            .await
            .iter()
            .filter_map(|(_, r, _)| match r {
                Reply::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("委譲しました → tzzz（devin @ pi）"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("どのタスクか分かりませんでした"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("未着手の引受として見つかりませんでした"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("取り消しできませんでした（既に処理済みです）"))
        );
        assert!(texts.iter().any(|t| t.contains("Pi側に届きませんでした")));
        assert!(mock.calls.lock().await.is_empty());
    }
    #[tokio::test]
    async fn notice_poll_claims_attention_and_finished() {
        let mock = Arc::new(Mock::default());
        let old = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            - 3600.0;
        *mock.listed.lock().await = vec![
            json!({"id": "t1a0dc0000c1", "kind": "promise",
                   "status": "awaiting_operator", "created_at": old,
                   "detail": {"message": "調べて", "response": "やります"}}),
            json!({"id": "t1a0dc0000c2", "kind": "approval",
                   "status": "awaiting_operator", "title": "書き込み"}),
            json!({"id": "skip", "status": "done"}),
        ];
        *mock.delegated_list.lock().await = vec![
            json!({"id": "tdone", "status": "done", "finished_at": old,
                   "title": "委譲: 調査", "detail": {"review": {"verdict": "accept"}},
                   "result": {"summary": "終わった"}}),
            json!({"id": "tnew", "status": "done", "finished_at": old + 7200.0,
                   "title": "委譲: 未評価", "detail": {}}),
        ];
        let bridge = Bridge::new(notifying(), mock.clone(), 16);
        bridge.poll_notices().await.unwrap();
        let notices = mock.notices.lock().await;
        // Delegated tasks that ended before the first poll are never announced.
        assert_eq!(notices.len(), 2);
        assert!(notices[0].contains("判断が必要です"));
        assert!(notices[0].contains("t1a0dc0000c1"));
        assert!(notices[1].contains("承認待ちです"));
        drop(notices);
        {
            let notices = bridge.notices.lock().await;
            assert!(notices.announced("t1a0dc0000c1"));
            assert!(notices.announced("t1a0dc0000c2"));
            assert!(!notices.finished_seen("tdone"));
        }
        // A second poll announces nothing twice, but the still-running task can
        // finish and — once the review arrives or the wait lapses — be sent.
        bridge.poll_notices().await.unwrap();
        assert_eq!(mock.notices.lock().await.len(), 2);
        *mock.listed.lock().await = vec![];
        *mock.delegated_list.lock().await = vec![json!({"id": "tnew",
            "status": "done", "finished_at": old + 7200.0, "title": "委譲: 未評価",
            "detail": {"review": {"verdict": "reject", "note": "だめ"}},
            "result": {"summary": "s", "branch": "feat/x"}})];
        bridge.poll_notices().await.unwrap();
        let notices = mock.notices.lock().await;
        assert_eq!(notices.len(), 3);
        assert!(notices[2].contains("委譲タスクが完了しました（tnew）"));
        assert!(notices[2].contains("澪の評価：不採用 — だめ"));
        assert!(notices[2].contains("feat/x"));
    }
    #[tokio::test]
    async fn reply_carries_background_and_commitment() {
        let mock = Arc::new(Mock::default());
        *mock.talk_background.lock().await = Some(json!({"state": "started",
            "tasks": [{"id": "tx", "executor": "devin"}, {"id": "ty"}]}));
        let bridge = Bridge::new(notifying(), mock.clone(), 16);
        let worker = tokio::spawn(bridge.clone().run());
        bridge.handle(event(1, 20, "<@99> go"), 99).await;
        wait_replies(&mock, 2).await;
        let replies = mock.replies.lock().await;
        assert!(
            matches!(&replies[1].1, Reply::Text(t) if t.contains("▶ 裏で実行中：tx（devin）、ty。終わったらお知らせします。"))
        );
        drop(replies);
        *mock.talk_background.lock().await =
            Some(json!({"state": "unstarted", "task_id": "t1a0dc0000c1"}));
        bridge.handle(event(2, 20, "<@99> more"), 99).await;
        wait_replies(&mock, 4).await;
        let replies = mock.replies.lock().await;
        assert!(
            matches!(&replies[3].1, Reply::Text(t) if t.contains("⚠ <@20> 引き受けましたが") && t.contains("@澪 開始 t1a0dc0000c1"))
        );
        assert_eq!(replies[3].2, Some(20));
        // The sent message's ID is remembered so `開始` replies resolve the task.
        {
            let notices = bridge.notices.lock().await;
            let sent = mock.sent.load(Ordering::SeqCst) + 599;
            assert_eq!(
                notices.task_for_message(sent).as_deref(),
                Some("t1a0dc0000c1")
            );
        }
        worker.abort();
    }
    async fn http_fixture(status: &str, body: String) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut scratch = [0; 4096];
            loop {
                let n = socket.read(&mut scratch).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&scratch[..n]);
                if let Some(split) = buf.windows(4).position(|p| p == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..split]);
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|s| s.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            let header = format!(
                "HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            for chunk in body.as_bytes().chunks(10000) {
                let wire = format!("{:x}\r\n", chunk.len());
                if socket.write_all(wire.as_bytes()).await.is_err() {
                    break;
                }
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
                if socket.write_all(b"\r\n").await.is_err() {
                    break;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
            String::from_utf8(buf).unwrap()
        });
        (url, task)
    }
    #[tokio::test]
    async fn resident_http_contract_and_bounds() {
        let text = "あ".repeat(50000);
        let (url, server) = http_fixture(
            "200 OK",
            json!({"response":text,"stderr_tail":"private"}).to_string(),
        )
        .await;
        assert_eq!(
            Resident::new(&url)
                .unwrap()
                .talk("こんにちは", "discord-test")
                .await
                .unwrap()
                .response,
            text
        );
        let request = server.await.unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            json!({"message":"こんにちは","subject":"discord-test"})
        );
        for (status, body) in [
            ("503 Unavailable", json!({"error":"private"}).to_string()),
            ("302 Found", "{}".into()),
            ("200 OK", "[]".into()),
            ("200 OK", json!({"response":2}).to_string()),
            ("200 OK", json!({"response":" "}).to_string()),
            (
                "200 OK",
                json!({"response":"a".repeat(1024*1024)}).to_string(),
            ),
        ] {
            let (url, server) = http_fixture(status, body).await;
            assert!(
                Resident::new(&url)
                    .unwrap()
                    .talk("test", "subject")
                    .await
                    .is_err()
            );
            let _ = server.await;
        }
        // Task-plane endpoints: list/delegate require a 200 task list; actions
        // surface the upstream status and object body.
        let (url, server) = http_fixture(
            "200 OK",
            json!({"tasks": [{"id": "t1"}, "junk"]}).to_string(),
        )
        .await;
        assert_eq!(Resident::new(&url).unwrap().tasks().await.unwrap().len(), 1);
        let _ = server.await;
        let (url, server) = http_fixture("404 Not Found", json!({"error": "x"}).to_string()).await;
        let resident = Resident::new(&url).unwrap();
        assert!(resident.tasks().await.is_err());
        let _ = server.await;
        let (url, server) =
            http_fixture("409 Conflict", json!({"error": "already"}).to_string()).await;
        let resident = Resident::new(&url).unwrap();
        assert_eq!(
            resident
                .task_action(&json!({"action": "start", "id": "t1"}))
                .await,
            Ok((409, json!({"error": "already"})))
        );
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/tasks "));
    }
    #[test]
    fn notice_state_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("kamimusuhi-discord-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notices.json");
        std::fs::write(
            &path,
            json!({"attention": {"t1a": {"message": 700, "kind": "promise"}},
                   "finished": {"t2b": 701}, "delegated_since": 12.5})
            .to_string(),
        )
        .unwrap();
        let state = NoticeState::load(Some(path.clone()));
        assert_eq!(state.task_for_message(700).as_deref(), Some("t1a"));
        assert!(state.finished_seen("t2b"));
        assert_eq!(state.delegated_since, Some(12.5));
        let mut state = NoticeState::load(Some(path.clone()));
        state.claim("t3c", 702, Some("approval".into()));
        state.finish("t4d", 703);
        state.save();
        let reloaded = NoticeState::load(Some(path));
        assert_eq!(reloaded.task_for_message(702).as_deref(), Some("t3c"));
        assert!(reloaded.finished_seen("t4d"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(NoticeState::load(None).attention.is_empty());
    }
}
