//! Operator-only Discord gateway. All individual state belongs to the resident.
use serde_json::json;
use serenity::{
    async_trait,
    builder::CreateMessage,
    http::Http,
    model::{channel::Message, gateway::GatewayIntents, id::ChannelId},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, Notify, oneshot},
    task::JoinSet,
};

pub const INTENTS: GatewayIntents = GatewayIntents::from_bits_truncate(
    GatewayIntents::GUILDS.bits() | GatewayIntents::GUILD_MESSAGES.bits(),
);
const FAILURE: &str = "澪の応答を受け取れませんでした。Pi側で処理が続いている可能性があります。自動再送はしていません。";

// Deliberately no Debug: this object owns the bot credential.
pub struct Config {
    pub token: String,
    pub guild: u64,
    pub users: HashSet<u64>,
    pub channels: HashSet<u64>,
    pub operator_subject: Option<String>,
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
        Ok(Self {
            token,
            guild: *guilds.iter().next().unwrap(),
            users,
            channels,
            operator_subject,
        })
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
#[derive(Clone)]
pub struct Event {
    pub id: u64,
    pub guild: Option<u64>,
    pub channel: u64,
    pub user: u64,
    pub bot: bool,
    pub webhook: bool,
    pub content: String,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Text(String),
    File(String),
}
pub fn response_reply(text: String) -> Reply {
    if text.encode_utf16().count() <= 1900 {
        Reply::Text(text)
    } else {
        Reply::File(text)
    }
}
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn reply(&self, event: &Event, reply: Reply) -> bool;
    async fn talk(&self, text: &str, subject: &str) -> Result<String, ResidentError>;
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
    notify: Notify,
    capacity: usize,
}
impl<B: Backend> Bridge<B> {
    pub fn new(config: Config, backend: Arc<B>, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            config,
            backend,
            state: Mutex::new(State::default()),
            notify: Notify::new(),
            capacity,
        })
    }
    pub async fn handle(&self, event: Event, bot_id: u64) {
        if !self.config.allows(&event) {
            return;
        }
        let mention = format!("<@{bot_id}>");
        let nickname = format!("<@!{bot_id}>");
        if !event.content.contains(&mention) && !event.content.contains(&nickname) {
            return;
        }
        let text = event
            .content
            .replace(&mention, "")
            .replace(&nickname, "")
            .trim()
            .to_owned();
        let mut state = self.state.lock().await;
        if state.seen.contains(&event.id) {
            return;
        }
        if text.is_empty() || text.len() > 32768 {
            drop(state);
            self.backend
                .reply(
                    &event,
                    Reply::Text("メンションに続けて、32KiB以内の文章を送ってください。".into()),
                )
                .await;
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
                )
                .await;
            return;
        }
        let (tx, rx) = oneshot::channel();
        state.pending.push_back(Item {
            subject: self.config.subject(&event),
            event: event.clone(),
            text,
            ack: rx,
        });
        state.seen.insert(event.id);
        state.seen_order.push_back(event.id);
        if state.seen_order.len() > 4096
            && let Some(old) = state.seen_order.pop_front()
        {
            state.seen.remove(&old);
        }
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
            )
            .await;
        let _ = tx.send(accepted);
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
                active.insert(item.subject.clone());
                let backend = self.backend.clone();
                running.spawn(async move {
                    if item.ack.await.unwrap_or(false) {
                        let reply = match backend.talk(&item.text, &item.subject).await {
                            Ok(response) => response_reply(response),
                            Err(ResidentError) => Reply::Text(FAILURE.into()),
                        };
                        backend.reply(&item.event, reply).await;
                    }
                    item.subject
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
}

/// Client has no proxy inheritance, redirect following, or automatic retries.
#[derive(Debug, PartialEq, Eq)]
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
    pub async fn talk(&self, text: &str, subject: &str) -> Result<String, ResidentError> {
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
        payload
            .get("response")
            .and_then(|r| r.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or(ResidentError)
    }
}
/// Explicit empty mention parsing applies to acknowledgments, errors and files alike.
pub fn message_builder(reply: &Reply, event: &Event) -> CreateMessage<'static> {
    let mut builder = CreateMessage::default();
    builder
        .reference_message((
            ChannelId(event.channel),
            serenity::model::id::MessageId(event.id),
        ))
        .allowed_mentions(|m| m.empty_parse().replied_user(false))
        .flags(serenity::model::channel::MessageFlags::SUPPRESS_EMBEDS);
    match reply {
        Reply::Text(text) => {
            builder.content(text);
        }
        Reply::File(text) => {
            builder
                .content("長い応答をファイルにまとめました。")
                .add_file(serenity::model::channel::AttachmentType::Bytes {
                    data: std::borrow::Cow::Owned(text.as_bytes().to_vec()),
                    filename: "mio-response.txt".into(),
                });
        }
    }
    builder
}
pub struct LiveBackend {
    pub http: Arc<Http>,
    pub resident: Resident,
}
#[async_trait]
impl Backend for LiveBackend {
    async fn reply(&self, event: &Event, reply: Reply) -> bool {
        let result = ChannelId(event.channel)
            .send_message(&self.http, |m| {
                *m = message_builder(&reply, event);
                m
            })
            .await;
        if result.is_err() {
            eprintln!("Discord reply failed");
        }
        result.is_ok()
    }
    async fn talk(&self, text: &str, subject: &str) -> Result<String, ResidentError> {
        self.resident.talk(text, subject).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    fn event(id: u64, user: u64, content: &str) -> Event {
        Event {
            id,
            guild: Some(10),
            channel: 30,
            user,
            bot: false,
            webhook: false,
            content: content.into(),
        }
    }
    #[derive(Default)]
    struct Mock {
        replies: Mutex<Vec<(u64, Reply)>>,
        calls: Mutex<Vec<(String, String)>>,
        ack_fail: AtomicBool,
        reply_fail: AtomicBool,
        talk_fail: AtomicBool,
        gate: AtomicBool,
        release: Notify,
        active: AtomicUsize,
        maximum: AtomicUsize,
    }
    #[async_trait]
    impl Backend for Mock {
        async fn reply(&self, event: &Event, reply: Reply) -> bool {
            let ack = matches!(&reply, Reply::Text(t) if t.starts_with("受け付け"));
            self.replies.lock().await.push((event.id, reply));
            !(if ack {
                self.ack_fail.load(Ordering::SeqCst)
            } else {
                self.reply_fail.load(Ordering::SeqCst)
            })
        }
        async fn talk(&self, text: &str, subject: &str) -> Result<String, ResidentError> {
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
                Ok(text.into())
            }
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
        let reply = response_reply("😀".repeat(1800) + "\n日本語");
        assert!(matches!(reply, Reply::File(_)));
        let b = message_builder(&reply, &m);
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
            mock.replies
                .lock()
                .await
                .iter()
                .any(|(_, r)| matches!(r,Reply::Text(t) if t.contains("自動再送はしていません")))
        );
        assert!(!worker.is_finished());
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
                .await,
            Ok(text)
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
    }
}
