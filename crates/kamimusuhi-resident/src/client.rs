//! Remote client side: node discovery, token lookup and the `chat` REPL.

use std::io::{BufRead, Write};
use std::time::Duration;

use kamimusuhi_resource_http::{Endpoint, Header, TrustAnchors, http};
use serde_json::{Value, json};

use crate::probes::{describe, get_json};

/// Site-local list of resident URLs, one per line, tried in order when
/// neither `--url` nor `$KAMIMUSUHI_URL` is given. Kept outside the
/// repository so host addresses are never committed.
pub fn nodes_file() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config/kamimusuhi/nodes"))
}

/// `$KAMIMUSUHI_NODE_TOKEN`, else `~/.config/kamimusuhi/node_token`.
pub fn token() -> Option<String> {
    if let Ok(t) = std::env::var("KAMIMUSUHI_NODE_TOKEN")
        && !t.trim().is_empty()
    {
        return Some(t.trim().to_owned());
    }
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join(".config/kamimusuhi/node_token");
        if let Some(t) = std::fs::read_to_string(path)
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        {
            return Some(t);
        }
    }
    // On a node, the service user can read its own secrets file.
    std::fs::read_to_string("/srv/kamimusuhi/config/secrets.env")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("KAMIMUSUHI_NODE_TOKEN="))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

pub fn history(
    url: &str,
    token: Option<&str>,
    subject: &str,
    limit: u64,
) -> Result<Vec<Value>, String> {
    let endpoint = Endpoint::parse(url, "/v1/kamimusuhi/history")?;
    let body = json!({"subject": subject, "limit": limit});
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(15),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if !response.is_success() {
        return Err(format!("HTTP {}", response.status));
    }
    Ok(value["turns"].as_array().cloned().unwrap_or_default())
}

pub fn tools(url: &str, token: Option<&str>) -> Result<Value, String> {
    get_json(url, "/v1/tools", &auth(token), Duration::from_secs(20))
}

pub fn tasks(url: &str, token: Option<&str>) -> Result<Vec<Value>, String> {
    let v = get_json(url, "/v1/tasks", &auth(token), Duration::from_secs(10))?;
    Ok(v["tasks"].as_array().cloned().unwrap_or_default())
}

pub fn task_action(url: &str, token: Option<&str>, body: &Value) -> Result<Value, String> {
    let endpoint = Endpoint::parse(url, "/v1/tasks")?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(15),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if response.is_success() {
        Ok(value)
    } else {
        Err(format!("HTTP {}: {value}", response.status))
    }
}

pub fn approvals(url: &str, token: Option<&str>) -> Result<Vec<Value>, String> {
    let v = get_json(url, "/v1/approvals", &auth(token), Duration::from_secs(10))?;
    Ok(v["approvals"].as_array().cloned().unwrap_or_default())
}

pub fn decide(url: &str, token: Option<&str>, id: &str, approve: bool) -> Result<Value, String> {
    let endpoint = Endpoint::parse(url, "/v1/approvals/decide")?;
    let body = json!({"id": id, "decision": if approve { "approve" } else { "reject" }});
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(300),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if response.is_success() {
        Ok(value)
    } else {
        Err(format!("HTTP {}: {value}", response.status))
    }
}

/// Print an approval decision result for a human.
pub fn print_decision(reply: &Value) {
    let ok = reply["ok"].as_bool().unwrap_or(false);
    println!(
        "{} state={}",
        if ok { "✔" } else { "✘" },
        reply["state"]
            .as_str()
            .or_else(|| reply["approval"]["state"].as_str())
            .unwrap_or("?")
    );
    if let Some(text) = reply["result"]["text"].as_str().filter(|t| !t.is_empty()) {
        println!("  {}", text.chars().take(400).collect::<String>());
    }
    if let Some(err) = reply["result"].get("error") {
        println!("  error: {err}");
    }
    if let Some(hook) = reply["result"].get("after_approved") {
        println!(
            "  after_approved: ok={} {}",
            hook["ok"],
            hook["stdout"]
                .as_array()
                .and_then(|l| l.first())
                .and_then(Value::as_str)
                .unwrap_or("")
        );
    }
}

fn pending(url: &str, token: Option<&str>) -> Vec<Value> {
    approvals(url, token)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a["state"] == "pending")
        .collect()
}

pub fn auth(token: Option<&str>) -> Vec<Header> {
    token
        .map(|t| Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {t}"),
        })
        .into_iter()
        .collect()
}

/// First candidate whose `/health` answers.
pub fn discover(candidates: &[String]) -> Result<String, String> {
    if candidates.is_empty() {
        return Err(
            "no resident URL: pass --url, set KAMIMUSUHI_URL or list URLs in ~/.config/kamimusuhi/nodes"
                .to_owned(),
        );
    }
    let mut errors = Vec::new();
    for url in candidates {
        match get_json(url, "/health", &[], Duration::from_secs(3)) {
            Ok(_) => return Ok(url.clone()),
            Err(e) => errors.push(format!("{url}: {e}")),
        }
    }
    Err(format!("no resident reachable ({})", errors.join("; ")))
}

pub fn candidates(explicit: Option<&str>) -> Vec<String> {
    if let Some(url) = explicit {
        return vec![url.to_owned()];
    }
    if let Ok(list) = std::env::var("KAMIMUSUHI_URL") {
        let urls: Vec<String> = list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !urls.is_empty() {
            return urls;
        }
    }
    nodes_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn talk(url: &str, token: Option<&str>, subject: &str, message: &str) -> Result<Value, String> {
    let endpoint = Endpoint::parse(url, "/v1/kamimusuhi/talk")?;
    let body = json!({"subject": subject, "message": message});
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(900),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if response.is_success() {
        Ok(value)
    } else {
        Err(format!("HTTP {}: {value}", response.status))
    }
}

pub fn library(url: &str, token: Option<&str>, body: &Value) -> Result<Value, String> {
    let endpoint = Endpoint::parse(url, "/v1/library")?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(120),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if response.is_success() {
        Ok(value)
    } else {
        Err(format!("HTTP {}: {value}", response.status))
    }
}

/// Interactive loop: one line in, one reply out. `/quit` or EOF ends it.
pub fn repl(url: &str, token: Option<&str>, subject: &str) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    eprintln!(
        "Kamimusuhi @ {url} / subject={subject}  (/quit 終了, /status 状態, /approvals 承認待ち, /approve <id>, /reject <id>)"
    );
    let mut announced: std::collections::HashSet<String> = pending(url, token)
        .iter()
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect();
    loop {
        let _ = write!(out, "\nあなた> ");
        let _ = out.flush();
        let mut line = String::new();
        if stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            break;
        }
        let line = line.trim();
        match line {
            "" => continue,
            "/quit" | "/exit" => break,
            "/approvals" => {
                let list = pending(url, token);
                if list.is_empty() {
                    println!("承認待ちはありません。");
                }
                for a in &list {
                    println!("  {}", crate::status::describe_approval(a));
                }
                continue;
            }
            cmd if cmd.starts_with("/approve ") || cmd.starts_with("/reject ") => {
                let approve = cmd.starts_with("/approve ");
                let id = cmd.split_whitespace().nth(1).unwrap_or("");
                match decide(url, token, id, approve) {
                    Ok(reply) => print_decision(&reply),
                    Err(e) => eprintln!("エラー: {e}"),
                }
                continue;
            }
            "/status" => {
                match crate::status::fetch(url, token) {
                    Ok(doc) => print!("{}", crate::status::render(&doc).0),
                    Err(e) => eprintln!("status: {e}"),
                }
                continue;
            }
            _ => {}
        }
        match talk(url, token, subject, line) {
            Ok(reply) => {
                println!(
                    "澪> {}",
                    reply.get("response").and_then(Value::as_str).unwrap_or("")
                );
                eprintln!(
                    "  [{}ms via {}]",
                    reply.get("latency_ms").map_or(Value::Null, Clone::clone),
                    reply.get("tier").and_then(Value::as_str).unwrap_or("?")
                );
            }
            Err(e) => eprintln!("エラー: {e}"),
        }
        // Surface approval requests this turn created.
        for a in pending(url, token) {
            let id = a["id"].as_str().unwrap_or("").to_owned();
            if announced.insert(id.clone()) {
                println!("  ⚠ 承認待ち: {}", crate::status::describe_approval(&a));
                println!("    → /approve {id}  または  /reject {id}");
            }
        }
    }
    Ok(())
}
