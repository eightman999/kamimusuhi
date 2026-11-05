//! Read-only live probe: authenticate, receive READY, and disconnect.
//! Uses the normal DISCORD_* environment; never runs the dialogue bridge.
use kamimusuhi_discord::{Config, INTENTS};
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    model::gateway::Ready,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

struct Probe {
    guild: u64,
    ready: Arc<AtomicBool>,
    received: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl EventHandler for Probe {
    async fn ready(&self, _: Context, event: Ready) {
        self.ready.store(
            event.guilds.iter().any(|guild| guild.id.0 == self.guild),
            Ordering::SeqCst,
        );
        self.received.notify_one();
    }
}

#[tokio::main]
async fn main() {
    let config = Config::from_env().unwrap_or_else(|reason| {
        eprintln!("{reason}");
        std::process::exit(1);
    });
    let ready = Arc::new(AtomicBool::new(false));
    let received = Arc::new(tokio::sync::Notify::new());
    let mut client = Client::builder(&config.token, INTENTS)
        .event_handler(Probe {
            guild: config.guild,
            ready: ready.clone(),
            received: received.clone(),
        })
        .await
        .unwrap_or_else(|_| {
            eprintln!("gateway probe setup failed");
            std::process::exit(1);
        });
    let shards = client.shard_manager.clone();
    tokio::select! {
        _ = client.start() => {},
        _ = received.notified() => {},
        _ = tokio::time::sleep(Duration::from_secs(30)) => {},
    }
    shards.lock().await.shutdown_all().await;
    if !ready.load(Ordering::SeqCst) {
        eprintln!("gateway probe failed: READY for the configured guild was not confirmed");
        std::process::exit(1);
    }
    println!("Gateway READY and configured guild membership: PASS (no messages sent)");
}
