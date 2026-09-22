use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use kamimusuhi_resident::config::{Config, DEFAULT_CONFIG_PATH, NodeRole};
use kamimusuhi_resident::spool::Spool;
use kamimusuhi_resident::state::Shared;
use kamimusuhi_resident::util::{atomic_write, unix_now};
use kamimusuhi_resident::{kcore, probes, server, status};
use serde_json::{Value, json};

const USAGE: &str = "\
kamimusuhi serve  [--config <path>]
kamimusuhi status [--config <path>] [--url <node-url>] [--json]
kamimusuhi ask    [--config <path>] [--url <node-url>] [--model <m>] <text>
kamimusuhi chat   [--url <node-url>] [--subject <id>]     talk with the individual (REPL)
kamimusuhi library list | tree <lib> [dir] | file <lib> <path> [offset] | search <lib> <text> [dir]
kamimusuhi approvals | approve <id> | reject <id>   [--url <node-url>]
kamimusuhi check-config [--config <path>]

Default config: /srv/kamimusuhi/config/resident.json (or $KAMIMUSUHI_CONFIG).
Remote --url needs the node token ($KAMIMUSUHI_NODE_TOKEN or ~/.config/kamimusuhi/node_token).
chat without --url tries $KAMIMUSUHI_URL (comma list), else ~/.config/kamimusuhi/nodes.
status exits 0 when healthy, 1 when degraded, 2 when the node is unreachable.";

struct Args {
    command: String,
    config: PathBuf,
    url: Option<String>,
    model: String,
    json: bool,
    subject: Option<String>,
    text: Vec<String>,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let command = it.next().ok_or(USAGE)?;
    let mut args = Args {
        command,
        config: std::env::var_os("KAMIMUSUHI_CONFIG")
            .map_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH), PathBuf::from),
        url: None,
        model: "kamimusuhi".to_owned(),
        json: false,
        subject: None,
        text: Vec::new(),
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => args.config = it.next().ok_or("--config needs a value")?.into(),
            "--url" => args.url = Some(it.next().ok_or("--url needs a value")?),
            "--model" => args.model = it.next().ok_or("--model needs a value")?,
            "--json" => args.json = true,
            "--subject" => args.subject = Some(it.next().ok_or("--subject needs a value")?),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            _ => args.text.push(arg),
        }
    }
    Ok(args)
}

fn local_url(config_path: &std::path::Path) -> Result<String, String> {
    let config = Config::load(config_path)?;
    let port = config
        .node
        .listen
        .rsplit_once(':')
        .map(|(_, p)| p.to_owned())
        .ok_or("node.listen has no port")?;
    Ok(format!("http://127.0.0.1:{port}"))
}

/// Monotonic per-node start counter, persisted in current_state/.
fn next_boot_epoch(config: &Config) -> u64 {
    let path = config.paths.current_state().join("boot_epoch");
    let previous = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let next = previous + 1;
    let _ = atomic_write(&path, next.to_string().as_bytes());
    next
}

fn serve(args: &Args) -> Result<(), String> {
    let config = Config::load(&args.config)?;
    for dir in [
        config.paths.spool(),
        config.paths.current_state(),
        config.paths.cache(),
        config.paths.runtime(),
    ] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let token = std::env::var(&config.node.token_env)
        .ok()
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_owned());
    if token.is_none() {
        eprintln!(
            "warning: {} unset; only loopback clients can use /status and /v1",
            config.node.token_env
        );
    }
    let epoch = next_boot_epoch(&config);
    let spool =
        Spool::new(config.paths.spool(), &config.node.id).map_err(|e| format!("spool: {e}"))?;
    let kcore_config = config.kcore.clone();
    let role = config.node.role;
    let shared = Arc::new(Shared::new(config, spool, epoch, token));
    let _ = shared.spool.append(
        "logs/lifecycle",
        json!({"event": "start", "boot_epoch": epoch, "version": env!("CARGO_PKG_VERSION"),
               "role": role.as_str(), "unix": unix_now()}),
    );
    probes::spawn_all(&shared);
    for server in &shared.mcp {
        server.supervise();
    }
    kamimusuhi_resident::jobs::spawn_all(&shared);
    if role == NodeRole::Continuity
        && let Some(k) = kcore_config
    {
        kcore::spawn(&shared, k);
    }
    eprintln!(
        "kamimusuhi-resident {} node={} role={} listen={} epoch={epoch}",
        env!("CARGO_PKG_VERSION"),
        shared.config.node.id,
        role.as_str(),
        shared.config.node.listen
    );
    server::serve(&shared).map_err(|e| format!("listen {}: {e}", shared.config.node.listen))
}

fn run() -> Result<ExitCode, String> {
    let args = parse()?;
    let token = kamimusuhi_resident::client::token();
    match args.command.as_str() {
        "serve" => serve(&args).map(|()| ExitCode::SUCCESS),
        "check-config" => {
            Config::load(&args.config)?;
            println!("config ok: {}", args.config.display());
            Ok(ExitCode::SUCCESS)
        }
        "status" => {
            let url = match &args.url {
                Some(u) => u.clone(),
                None => local_url(&args.config)?,
            };
            match status::fetch(&url, token.as_deref()) {
                Ok(doc) => {
                    let (text, ok) = status::render(&doc);
                    if args.json {
                        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
                    } else {
                        print!("{text}");
                    }
                    Ok(ExitCode::from(if ok { 0 } else { 1 }))
                }
                Err(e) => {
                    println!("resident at {url}: DOWN ({e})");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "chat" => {
            use kamimusuhi_resident::client;
            let url = client::discover(&client::candidates(args.url.as_deref()))?;
            let subject = args
                .subject
                .clone()
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "local-user".to_owned());
            client::repl(&url, token.as_deref(), &subject)?;
            Ok(ExitCode::SUCCESS)
        }
        "approvals" | "approve" | "reject" => {
            use kamimusuhi_resident::client;
            let url = match &args.url {
                Some(u) => u.clone(),
                None if args.config.is_file() => local_url(&args.config)?,
                None => client::discover(&client::candidates(None))?,
            };
            if args.command == "approvals" {
                for a in client::approvals(&url, token.as_deref())? {
                    println!(
                        "{:<9} {}",
                        a["state"].as_str().unwrap_or("?"),
                        kamimusuhi_resident::status::describe_approval(&a)
                    );
                }
            } else {
                let id = args.text.first().ok_or("approval id required")?;
                let reply = client::decide(&url, token.as_deref(), id, args.command == "approve")?;
                client::print_decision(&reply);
            }
            Ok(ExitCode::SUCCESS)
        }
        "library" => {
            use kamimusuhi_resident::client;
            let url = match &args.url {
                Some(u) => u.clone(),
                None if args.config.is_file() => local_url(&args.config)?,
                None => client::discover(&client::candidates(None))?,
            };
            let t = |i: usize| args.text.get(i).cloned().unwrap_or_default();
            let action = if args.text.is_empty() {
                "list".to_owned()
            } else {
                t(0)
            };
            let body = match action.as_str() {
                "list" => json!({"action": "list"}),
                "tree" => json!({"action": "tree", "library": t(1), "path": t(2)}),
                "file" => json!({"action": "file", "library": t(1), "path": t(2),
                                 "offset": t(3).parse::<u64>().unwrap_or(0)}),
                "search" => json!({"action": "search", "library": t(1), "q": t(2), "path": t(3)}),
                _ => return Err(USAGE.to_owned()),
            };
            let reply = client::library(&url, token.as_deref(), &body)?;
            if action == "file" && !args.json {
                print!("{}", reply["text"].as_str().unwrap_or_default());
                if let Some(next) = reply["next_offset"].as_u64() {
                    eprintln!("\n[truncated: size={} next_offset={next}]", reply["size"]);
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&reply).unwrap_or_default()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        "ask" => {
            let url = match &args.url {
                Some(u) => u.clone(),
                None => local_url(&args.config)?,
            };
            let text = args.text.join(" ");
            if text.is_empty() {
                return Err("ask needs text".to_owned());
            }
            let body =
                json!({"model": args.model, "messages": [{"role": "user", "content": text}]});
            let mut headers = Vec::new();
            if let Some(t) = &token {
                headers.push(kamimusuhi_resource_http::Header {
                    name: "Authorization".to_owned(),
                    value: format!("Bearer {t}"),
                });
            }
            let endpoint = kamimusuhi_resource_http::Endpoint::parse(&url, "/v1/chat/completions")?;
            let response = kamimusuhi_resource_http::http::post_json(
                &endpoint,
                &body.to_string(),
                &headers,
                Duration::from_secs(600),
                &kamimusuhi_resource_http::TrustAnchors::Webpki,
            )
            .map_err(|e| probes::describe(&e))?;
            let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
            if !response.is_success() {
                return Err(format!("HTTP {}: {value}", response.status));
            }
            eprintln!("[route] {}", value["kamimusuhi_route"]);
            println!(
                "{}",
                value
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(64)
        }
    }
}
