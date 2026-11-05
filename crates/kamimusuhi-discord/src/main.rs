use kamimusuhi_discord::{Bridge, Config, Event, INTENTS, LiveBackend, Resident};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    http::Http,
    model::{channel::Message, gateway::Ready},
};
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
        eprintln!("Discord mention gateway ready");
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
    let resident = Resident::new("http://127.0.0.1:7860").unwrap_or_else(|_| {
        eprintln!("resident client setup failed");
        std::process::exit(1);
    });
    let bridge = Bridge::new(
        config,
        Arc::new(LiveBackend {
            http: Arc::new(Http::new(&token)),
            resident,
        }),
        16,
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
    let worker = tokio::spawn(bridge.run());
    tokio::select! {
        result = client.start() => { if result.is_err() { eprintln!("Discord client stopped"); worker.abort(); std::process::exit(1); } },
        _ = tokio::signal::ctrl_c() => { client.shard_manager.lock().await.shutdown_all().await; }
    }
    worker.abort();
}
