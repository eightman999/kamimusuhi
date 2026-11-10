use futures_util::{SinkExt, StreamExt};
use kamimusuhi_slack::{Api, Config, Seen, Turn, enqueue, reply_bodies, talk};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

type Result<T> = std::result::Result<T, &'static str>;
async fn session(
    config: &Config,
    api: &Api,
    bot: &str,
    queue: &mpsc::Sender<Turn>,
    seen: &mut Seen,
) -> Result<()> {
    let opened = api
        .call("apps.connections.open", &config.app_token, json!({}))
        .await?;
    let url = opened["url"]
        .as_str()
        .filter(|url| url.starts_with("wss://"))
        .ok_or("invalid Slack socket URL")?;
    // Never log URL/errors: the URL includes a temporary authentication ticket.
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(30),
        connect_async_with_config(
            url,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(1024 * 1024))
                    .max_frame_size(Some(1024 * 1024)),
            ),
            false,
        ),
    )
    .await
    .map_err(|_| "Slack socket connect timeout")?
    .map_err(|_| "Slack socket connect failed")?;
    eprintln!("Slack Socket Mode connected");
    receive(&mut socket, config, bot, queue, seen).await
}
async fn receive<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    config: &Config,
    bot: &str,
    queue: &mpsc::Sender<Turn>,
    seen: &mut Seen,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(120), socket.next())
            .await
            .map_err(|_| "Slack socket idle timeout")?
            .ok_or("Slack socket closed")?
            .map_err(|_| "Slack socket read failed")?;
        match message {
            Message::Ping(data) => {
                tokio::time::timeout(Duration::from_secs(10), socket.send(Message::Pong(data)))
                    .await
                    .map_err(|_| "Slack pong timeout")?
                    .map_err(|_| "Slack pong failed")?;
            }
            Message::Text(text) => {
                let envelope: Value =
                    serde_json::from_str(&text).map_err(|_| "invalid Slack envelope")?;
                if envelope["type"] == "disconnect" {
                    return Ok(());
                }
                if let Some(id) = enqueue(config, bot, &envelope, queue, seen)? {
                    tokio::time::timeout(
                        Duration::from_secs(2),
                        socket.send(Message::Text(json!({"envelope_id":id}).to_string().into())),
                    )
                    .await
                    .map_err(|_| "Slack ACK timeout")?
                    .map_err(|_| "Slack ACK failed")?;
                }
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
}
async fn run(config: Config) -> Result<()> {
    let config = Arc::new(config);
    let api = Api::new("https://slack.com/api")?;
    let auth = api.call("auth.test", &config.bot_token, json!({})).await?;
    if auth["team_id"] != config.team {
        return Err("Slack bot belongs to a different workspace");
    }
    let bot = auth["user_id"]
        .as_str()
        .ok_or("Slack bot identity missing")?
        .to_owned();
    // Local resident only: credentials and user input must never be redirected.
    let resident = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(660))
        .build()
        .map_err(|_| "resident client failed")?;
    let (tx, mut rx) = mpsc::channel::<Turn>(16);
    let worker_api = api.clone();
    let worker_config = config.clone();
    let mut worker = tokio::spawn(async move {
        // A single consumer preserves per-user ordering, including across reconnects.
        while let Some(turn) = rx.recv().await {
            let response = match talk(&resident, "http://127.0.0.1:7860", &turn).await {
                Ok(text) => text,
                Err(reason) => {
                    eprintln!("{reason}");
                    "会話処理を完了できませんでした。処理が継続している場合があるため、自動再実行はしません。".into()
                }
            };
            for (index, body) in reply_bodies(&turn, &response).into_iter().enumerate() {
                if index > 0 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                // A timeout may mean the message was accepted: no automatic resend.
                if let Err(reason) = worker_api
                    .call("chat.postMessage", &worker_config.bot_token, body)
                    .await
                {
                    eprintln!("{reason}; reply delivery incomplete (no automatic retry)");
                    break;
                }
            }
        }
    });
    let mut seen = Seen::default();
    let gateway = async {
        loop {
            if let Err(reason) = session(&config, &api, &bot, &tx, &mut seen).await {
                eprintln!("{reason}");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    };
    tokio::select! {
        _ = gateway => {},
        _ = tokio::signal::ctrl_c() => {},
        _ = &mut worker => { return Err("Slack dialogue worker stopped"); }
    }
    worker.abort();
    Ok(())
}
#[tokio::main]
async fn main() {
    let result = match Config::from_env() {
        Ok(config) => run(config).await,
        Err(reason) => Err(reason),
    };
    if let Err(reason) = result {
        eprintln!("{reason}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn websocket_tls_provider_is_available() {
        // Socket Mode uses rustls 0.23; reqwest's 0.21 provider does not configure it.
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }
    #[tokio::test]
    async fn socket_ack_ping_and_reconnect_signal() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let config = Config::parse(|k| {
                match k {
                    "SLACK_APP_TOKEN" => "xapp-fake",
                    "SLACK_BOT_TOKEN" => "xoxb-fake",
                    "SLACK_TEAM_ID" => "T1",
                    "SLACK_ALLOWED_USER_IDS" => "U1",
                    "SLACK_ALLOWED_CHANNEL_IDS" => "C1",
                    _ => "",
                }
                .into()
            })
            .unwrap();
            receive(&mut socket, &config, "UBOT", &tx, &mut Seen::default()).await
        });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        socket.send(Message::Ping(vec![1, 2].into())).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Pong(vec![1, 2].into())
        );
        let envelope = json!({"envelope_id":"en1", "type":"events_api", "payload":{
            "type":"event_callback", "team_id":"T1", "event_id":"Ev1",
            "event":{"type":"app_mention","user":"U1","channel":"C1","ts":"1.1","text":"<@UBOT> hello"}}});
        for _ in 0..2 {
            socket
                .send(Message::Text(envelope.to_string().into()))
                .await
                .unwrap();
            let ack = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&ack).unwrap(),
                json!({"envelope_id":"en1"})
            );
        }
        assert_eq!(rx.recv().await.unwrap().text, "hello");
        assert!(rx.try_recv().is_err());
        socket
            .send(Message::Text(
                json!({"type":"disconnect","reason":"refresh_requested"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert!(server.await.unwrap().is_ok());
    }
}
