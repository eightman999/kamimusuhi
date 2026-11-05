use kamimusuhi_discord::{Bridge, Config, Event, INTENTS, LiveBackend, NoticeState, Resident};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    http::Http,
    model::{channel::Message, gateway::Ready},
};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
struct Handler {
    bridge: Arc<Bridge<LiveBackend>>,
    bot: AtomicU64,
}
#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        self.bot.store(ready.user.id.0, Ordering::Relaxed);
        eprintln!("Discord reply gateway ready");
    }
    async fn message(&self, _: Context, message: Message) {
        let bot = self.bot.load(Ordering::Relaxed);
        if bot != 0 {
            self.bridge.handle(Event::from(message), bot).await;
        }
    }
}
#[tokio::main]
async fn main() {
    let config = Config::from_env().unwrap_or_else(|reason| {
        eprintln!("{reason}");
        std::process::exit(1);
    });
    let token = config.token.clone();
    // The notice (channel, operator) pair; both resolve or notices stay off.
    let notify = match (config.notify_channel, config.notify_user) {
        (Some(channel), Some(user)) => Some((channel, user)),
        _ => None,
    };
    let resident = Resident::new("http://127.0.0.1:7860").unwrap_or_else(|_| {
        eprintln!("resident client setup failed");
        std::process::exit(1);
    });
    // Which task notices were already posted (survives restarts).
    let state_dir = std::env::var("STATE_DIRECTORY")
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_owned();
    let notices = NoticeState::load(if state_dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(state_dir).join("notices.json"))
    });
    let bridge = Bridge::with_notices(
        config,
        Arc::new(LiveBackend {
            http: Arc::new(Http::new(&token)),
            resident,
            notify,
        }),
        16,
        notices,
    );
    let mut client = Client::builder(&token, INTENTS)
        .event_handler(Handler {
            bridge: bridge.clone(),
            bot: AtomicU64::new(0),
        })
        .await
        .unwrap_or_else(|_| {
            eprintln!("Discord client setup failed");
            std::process::exit(1);
        });
    let worker = tokio::spawn(bridge.clone().run());
    let notifier = notify.map(|_| tokio::spawn(bridge.notice_loop()));
    tokio::select! {
        result = client.start() => { if result.is_err() { eprintln!("Discord client stopped"); worker.abort(); if let Some(n) = &notifier { n.abort(); } std::process::exit(1); } },
        _ = tokio::signal::ctrl_c() => { client.shard_manager.lock().await.shutdown_all().await; }
    }
    worker.abort();
    if let Some(n) = notifier {
        n.abort();
    }
}
