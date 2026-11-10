//! Allowlisted Socket Mode conversation surface; the resident owns all dialogue state.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::Duration;

type Result<T> = std::result::Result<T, &'static str>;

// Deliberately no Debug: this configuration contains credentials.
pub struct Config {
    pub app_token: String,
    pub bot_token: String,
    pub team: String,
    pub users: Vec<String>,
    pub channels: Vec<String>,
    pub operator_subject: Option<String>,
}
fn identifier(s: &str, prefixes: &str) -> bool {
    s.len() >= 2
        && s.len() <= 64
        && prefixes.contains(s.as_bytes()[0] as char)
        && s.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}
impl Config {
    pub fn from_env() -> Result<Self> {
        Self::parse(|key| std::env::var(key).unwrap_or_default())
    }
    pub fn parse(get: impl Fn(&str) -> String) -> Result<Self> {
        let list = |key: &str, prefixes| -> Result<Vec<String>> {
            let raw = get(key);
            let values: Vec<_> = raw.split(',').map(str::trim).map(str::to_owned).collect();
            if values.iter().any(|s| !identifier(s, prefixes)) {
                return Err("invalid or empty Slack allowlist");
            }
            Ok(values)
        };
        let app_token = get("SLACK_APP_TOKEN").trim().to_owned();
        let bot_token = get("SLACK_BOT_TOKEN").trim().to_owned();
        if !app_token.starts_with("xapp-") || !bot_token.starts_with("xoxb-") {
            return Err("SLACK_APP_TOKEN and SLACK_BOT_TOKEN are required");
        }
        let team = get("SLACK_TEAM_ID").trim().to_owned();
        if !identifier(&team, "T") {
            return Err("invalid SLACK_TEAM_ID");
        }
        let users = list("SLACK_ALLOWED_USER_IDS", "UW")?;
        let channels = list("SLACK_ALLOWED_CHANNEL_IDS", "CG")?;
        let subject = get("SLACK_OPERATOR_SUBJECT").trim().to_owned();
        if !subject.is_empty()
            && (users.len() != 1
                || !subject.as_bytes()[0].is_ascii_alphanumeric()
                || subject.len() > 64
                || !subject
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)))
        {
            return Err("SLACK_OPERATOR_SUBJECT requires one allowed user and a valid subject");
        }
        Ok(Self {
            app_token,
            bot_token,
            team,
            users,
            channels,
            operator_subject: (!subject.is_empty()).then_some(subject),
        })
    }
}
#[derive(Clone)]
pub struct Turn {
    pub event_id: String,
    pub channel: String,
    pub thread: String,
    pub subject: String,
    pub text: String,
}
/// Accept only original human mentions in the configured workspace/channel/user tuple.
pub fn turn(config: &Config, bot: &str, payload: &Value) -> Option<Turn> {
    let event = &payload["event"];
    if payload["type"] != "event_callback"
        || payload["team_id"] != config.team
        || event["type"] != "app_mention"
        || event.get("subtype").is_some()
        || event.get("bot_id").is_some()
        || event.get("bot_profile").is_some()
    {
        return None;
    }
    let user = event["user"].as_str()?;
    let channel = event["channel"].as_str()?;
    if user == bot
        || !config.users.iter().any(|u| u == user)
        || !config.channels.iter().any(|c| c == channel)
    {
        return None;
    }
    let mention = format!("<@{bot}>");
    let raw = event["text"].as_str()?;
    if !raw.contains(&mention) {
        return None;
    }
    let text = raw.replace(&mention, "").trim().to_owned();
    let event_id = payload["event_id"].as_str()?;
    let ts = event["ts"].as_str()?;
    let thread = event["thread_ts"].as_str().unwrap_or(ts);
    if text.is_empty()
        || text.len() > 32_000
        || event_id.is_empty()
        || ts.is_empty()
        || thread.is_empty()
    {
        return None;
    }
    Some(Turn {
        event_id: event_id.into(),
        channel: channel.into(),
        thread: thread.into(),
        subject: config.operator_subject.clone().unwrap_or_else(|| {
            let digest = Sha256::digest(format!("{}:{user}", config.team));
            let hex: String = digest[..24]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            format!("slack-{hex}")
        }),
        text,
    })
}
#[derive(Default)]
pub struct Seen(VecDeque<String>);
impl Seen {
    pub fn contains(&self, id: &str) -> bool {
        self.0.iter().any(|s| s == id)
    }
    pub fn insert(&mut self, id: String) {
        self.0.push_back(id);
        if self.0.len() > 4096 {
            self.0.pop_front();
        }
    }
}
/// Plain text blocks disable Slack mention/link interpretation, even for model output.
pub fn reply_bodies(turn: &Turn, text: &str) -> Vec<Value> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(2500)
        .map(|chunk| {
            let text: String = chunk.iter().collect();
            json!({"channel":turn.channel,"thread_ts":turn.thread,"text":text,
            "parse":"none", "mrkdwn":false, "link_names":false, "unfurl_links":false,"unfurl_media":false,
            "blocks":[{"type":"section","text":{"type":"plain_text","text":text,"emoji":false}}]})
        })
        .collect()
}
#[derive(Clone)]
pub struct Api {
    client: reqwest::Client,
    base: String,
}
impl Api {
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| "HTTP client failed")?,
            base: base.into(),
        })
    }
    pub async fn call(&self, method: &str, token: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{}/{method}", self.base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| "Slack request failed")?;
        let value = bounded_json(response).await?;
        if value["ok"] != true {
            return Err("Slack API rejected request");
        }
        Ok(value)
    }
}
async fn bounded_json(mut response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err("HTTP request rejected");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "HTTP body failed")? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("HTTP response too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid HTTP JSON")
}
pub async fn talk(client: &reqwest::Client, url: &str, turn: &Turn) -> Result<String> {
    let response = client
        .post(format!("{url}/v1/kamimusuhi/talk"))
        .json(&json!({"message":turn.text,"subject":turn.subject}))
        .send()
        .await
        .map_err(|_| "resident request failed")?;
    let value = bounded_json(response).await?;
    value["response"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .ok_or("invalid resident response")
}

/// Queue before acknowledging; on saturation return an error without claiming delivery.
pub fn enqueue(
    config: &Config,
    bot: &str,
    envelope: &Value,
    queue: &tokio::sync::mpsc::Sender<Turn>,
    seen: &mut Seen,
) -> Result<Option<String>> {
    let Some(id) = envelope["envelope_id"].as_str() else {
        return Ok(None);
    };
    if envelope["type"] == "events_api"
        && let Some(turn) = turn(config, bot, &envelope["payload"])
        && !seen.contains(&turn.event_id)
    {
        let permit = queue
            .try_reserve()
            .map_err(|_| "Slack queue full or stopped")?;
        seen.insert(turn.event_id.clone());
        permit.send(turn);
    }
    Ok(Some(id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config::parse(|key| {
            match key {
                "SLACK_APP_TOKEN" => "xapp-fake",
                "SLACK_BOT_TOKEN" => "xoxb-fake",
                "SLACK_TEAM_ID" => "T123",
                "SLACK_ALLOWED_USER_IDS" => "U123",
                "SLACK_ALLOWED_CHANNEL_IDS" => "C123",
                _ => "",
            }
            .into()
        })
        .unwrap()
    }
    fn payload() -> Value {
        json!({"type":"event_callback","team_id":"T123","event_id":"Ev1",
            "event":{"type":"app_mention","user":"U123","channel":"C123","ts":"1.2","text":"<@UBOT> hello"}})
    }
    #[test]
    fn configuration_fails_closed() {
        for key in [
            "SLACK_APP_TOKEN",
            "SLACK_BOT_TOKEN",
            "SLACK_TEAM_ID",
            "SLACK_ALLOWED_USER_IDS",
            "SLACK_ALLOWED_CHANNEL_IDS",
        ] {
            assert!(
                Config::parse(|k| if k == key {
                    String::new()
                } else {
                    match k {
                        "SLACK_APP_TOKEN" => "xapp-fake",
                        "SLACK_BOT_TOKEN" => "xoxb-fake",
                        "SLACK_TEAM_ID" => "T123",
                        "SLACK_ALLOWED_USER_IDS" => "U123",
                        "SLACK_ALLOWED_CHANNEL_IDS" => "C123",
                        _ => "",
                    }
                    .into()
                })
                .is_err()
            );
        }
        assert!(
            Config::parse(|k| match k {
                "SLACK_APP_TOKEN" => "xapp-fake",
                "SLACK_BOT_TOKEN" => "xoxb-fake",
                "SLACK_TEAM_ID" => "T123",
                "SLACK_ALLOWED_USER_IDS" => "U123,U456",
                "SLACK_ALLOWED_CHANNEL_IDS" => "C123",
                "SLACK_OPERATOR_SUBJECT" => "eightman",
                _ => "",
            }
            .into())
            .is_err()
        );
    }
    #[test]
    fn routes_and_isolates_human_mentions() {
        let config = config();
        let mut p = payload();
        let t = turn(&config, "UBOT", &p).unwrap();
        assert_eq!(t.text, "hello");
        assert!(t.subject.starts_with("slack-") && t.subject.len() <= 64);
        let mut other = Config {
            app_token: String::new(),
            bot_token: String::new(),
            team: config.team.clone(),
            users: config.users.clone(),
            channels: config.channels.clone(),
            operator_subject: None,
        };
        other.users.push("U456".into());
        let mut second = payload();
        second["event"]["user"] = json!("U456");
        assert_ne!(t.subject, turn(&other, "UBOT", &second).unwrap().subject);
        assert_eq!(t.thread, "1.2");
        p["event"]["thread_ts"] = json!("0.1");
        assert_eq!(turn(&config, "UBOT", &p).unwrap().thread, "0.1");
        let mut operator = config;
        operator.operator_subject = Some("eightman".into());
        assert_eq!(turn(&operator, "UBOT", &p).unwrap().subject, "eightman");
        for (key, value) in [
            ("user", "U999"),
            ("channel", "C999"),
            ("user", "UBOT"),
            ("type", "message"),
            ("subtype", "message_changed"),
            ("bot_id", "B1"),
            ("text", "no mention"),
            ("text", "<@UBOT>"),
        ] {
            let mut p = payload();
            p["event"][key] = json!(value);
            assert!(turn(&operator, "UBOT", &p).is_none(), "{key}");
        }
        p["team_id"] = json!("T999");
        assert!(turn(&operator, "UBOT", &p).is_none());
    }
    #[tokio::test]
    async fn ack_dedup_and_saturation() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut seen = Seen::default();
        let mut e = json!({"type":"events_api","envelope_id":"e1","payload":payload()});
        assert_eq!(
            enqueue(&config(), "UBOT", &e, &tx, &mut seen).unwrap(),
            Some("e1".into())
        );
        // Duplicate is ACKed even when queue is full, but never delivered twice.
        assert!(enqueue(&config(), "UBOT", &e, &tx, &mut seen).is_ok());
        e["payload"]["event_id"] = json!("Ev2");
        assert!(enqueue(&config(), "UBOT", &e, &tx, &mut seen).is_err());
        assert!(!seen.contains("Ev2"));
        assert_eq!(rx.recv().await.unwrap().event_id, "Ev1");
        assert!(rx.try_recv().is_err());
        assert!(enqueue(&config(), "UBOT", &e, &tx, &mut seen).is_ok());
        assert_eq!(rx.recv().await.unwrap().event_id, "Ev2");
    }
    #[test]
    fn unicode_chunks_are_plain_text() {
        let t = turn(&config(), "UBOT", &payload()).unwrap();
        let text = format!("{}<@U123><!channel>", "澪".repeat(3000));
        let bodies = reply_bodies(&t, &text);
        assert_eq!(bodies.len(), 2);
        let joined: String = bodies
            .iter()
            .map(|b| b["blocks"][0]["text"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(joined, text);
        for b in bodies {
            assert_eq!(b["blocks"][0]["text"]["type"], "plain_text");
            assert_eq!(b["thread_ts"], "1.2");
            assert_eq!(b["link_names"], false);
        }
    }
    // Real local HTTP transport, no Slack credentials or external requests.
    async fn mock(status: &str, body: Value) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let len: usize = header
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap_or("0")
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(bytes).unwrap()
        });
        (format!("http://{addr}"), task)
    }
    #[tokio::test]
    async fn resident_contract_and_reply_transport() {
        let t = turn(&config(), "UBOT", &payload()).unwrap();
        let (url, server) = mock(
            "200 OK",
            json!({"response":"こんにちは", "internal":"private"}),
        )
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(talk(&client, &url, &t).await.unwrap(), "こんにちは");
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/kamimusuhi/talk "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"message":"hello","subject":t.subject}));
        let (url, server) = mock("200 OK", json!({"ok":true})).await;
        let api = Api::new(&url).unwrap();
        api.call(
            "chat.postMessage",
            "fake",
            reply_bodies(&t, "hello").remove(0),
        )
        .await
        .unwrap();
        assert!(server.await.unwrap().starts_with("POST /chat.postMessage "));
    }
    #[tokio::test]
    async fn failures_never_return_private_error_bodies() {
        let t = turn(&config(), "UBOT", &payload()).unwrap();
        for (status, body) in [
            ("500 Error", json!({"response":"private"})),
            ("200 OK", json!({"error":"private"})),
            ("200 OK", json!({"response":" "})),
        ] {
            let (url, server) = mock(status, body).await;
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            assert!(talk(&client, &url, &t).await.is_err());
            server.await.unwrap();
        }
        let (url, server) = mock("200 OK", json!({"ok":false,"error":"private"})).await;
        assert_eq!(
            Api::new(&url)
                .unwrap()
                .call("chat.postMessage", "fake", json!({}))
                .await
                .unwrap_err(),
            "Slack API rejected request"
        );
        server.await.unwrap();
    }
}
