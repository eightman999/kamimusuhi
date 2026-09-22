//! `kamimusuhi status`: one screen covering Pi / llm_master / NAS / HAI.

use std::fmt::Write as _;
use std::time::Duration;

use kamimusuhi_resource_http::Header;
use serde_json::Value;

use crate::probes::get_json;

pub fn fetch(url: &str, token: Option<&str>) -> Result<Value, String> {
    let headers: Vec<Header> = token
        .map(|t| Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {t}"),
        })
        .into_iter()
        .collect();
    get_json(url, "/status", &headers, Duration::from_secs(8))
}

fn mark(ok: bool) -> &'static str {
    if ok { "OK  " } else { "DOWN" }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "-".to_owned(),
        Some(other) => other.to_string(),
    }
}

/// Render the status document. Returns the text and whether everything
/// required for normal operation is healthy.
pub fn render(status: &Value) -> (String, bool) {
    let mut out = String::new();
    let mut all_ok = true;
    let node = &status["node"];
    let _ = writeln!(
        out,
        "Kamimusuhi status @ {}  (queried node: {} / {})",
        s(status.get("generated_at")),
        s(node.get("node")),
        s(node.get("role"))
    );
    let _ = writeln!(out);

    // Nodes: self + peers.
    let _ = writeln!(out, "NODES");
    let self_line = format!(
        "  [{}] {:<11} role={:<10} up={}s epoch={} kcore={} local_llm={}",
        mark(true),
        s(node.get("node")),
        s(node.get("role")),
        s(node.get("uptime_secs")),
        s(node.get("boot_epoch")),
        s(node.get("kcore")),
        s(node.get("local_llm")),
    );
    let _ = writeln!(out, "{self_line}");
    if let Some(peers) = status["peers"].as_object() {
        for (id, peer) in peers {
            let ok = peer["healthy"].as_bool().unwrap_or(false);
            all_ok &= ok;
            let d = &peer["detail"];
            if ok {
                let gpu = d["gpu"]
                    .as_array()
                    .map(|g| {
                        g.iter()
                            .map(|g| {
                                format!(
                                    "{}:{}/{}MiB",
                                    s(g.get("name")),
                                    s(g.get("memory_used_mib")),
                                    s(g.get("memory_total_mib"))
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_else(|| "-".to_owned());
                let _ = writeln!(
                    out,
                    "  [{}] {:<11} role={:<10} up={}s local_llm={} nas={} gpu={}",
                    mark(true),
                    id,
                    s(d.get("role")),
                    s(d.get("uptime_secs")),
                    s(d.get("local_llm")),
                    s(d.get("nas")),
                    gpu
                );
            } else {
                let _ = writeln!(
                    out,
                    "  [{}] {:<11} last_ok={} error={}",
                    mark(false),
                    id,
                    s(peer.get("last_ok")),
                    s(peer.get("error"))
                );
            }
        }
    }

    // Routing.
    let routing = &status["routing"];
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "ROUTING  order={}  active_primary={}",
        routing["order"]
            .as_array()
            .map(|o| o.iter().map(|x| s(Some(x))).collect::<Vec<_>>().join(" → "))
            .unwrap_or_default(),
        s(routing.get("active_primary"))
    );
    if let Some(tiers) = routing["tiers"].as_object() {
        for (name, tier) in tiers {
            let ok = tier["healthy"].as_bool().unwrap_or(false);
            let _ = writeln!(
                out,
                "  [{}] {:<11} latency={}ms checked={} {}",
                mark(ok),
                name,
                s(tier.get("latency_ms")),
                s(tier.get("checked_at")),
                if ok {
                    String::new()
                } else {
                    format!("error={}", s(tier.get("error")))
                }
            );
        }
    }
    if routing["active_primary"].is_null() {
        all_ok = false;
    }
    if let Some(last) = routing.get("last_route").filter(|v| !v.is_null()) {
        let _ = writeln!(
            out,
            "  last request: tier={} ok={} {}ms attempts={}",
            s(last.get("tier")),
            s(last.get("ok")),
            s(last.get("latency_ms")),
            s(last.get("attempts"))
        );
    }

    // NAS.
    let nas = &status["nas"];
    let _ = writeln!(out);
    if nas["configured"].as_bool().unwrap_or(false) {
        let ok = nas["probe"]["healthy"].as_bool().unwrap_or(false);
        all_ok &= ok;
        let _ = writeln!(
            out,
            "NAS  [{}] {}  {}",
            mark(ok),
            s(nas.get("root")),
            if ok {
                String::new()
            } else {
                format!("error={}", s(nas["probe"].get("error")))
            }
        );
        let _ = writeln!(
            out,
            "  spool backlog: {} files / {} bytes{}   last sync ok: {}",
            s(nas.get("spool_backlog_files")),
            s(nas.get("spool_backlog_bytes")),
            if nas["spool_warn"].as_bool().unwrap_or(false) {
                " (WARN: large)"
            } else {
                ""
            },
            s(nas["sync"].get("last_success"))
        );
    } else {
        let _ = writeln!(out, "NAS  [----] not configured (spool only)");
    }

    // HAI.
    let _ = writeln!(out);
    let hai_tier = &routing["tiers"]["hai"];
    let hai_ok = hai_tier["healthy"].as_bool().unwrap_or(false);
    let account = &status["hai_account"];
    let _ = writeln!(
        out,
        "HAI  [{}] api latency={}ms  balance={} JPY{}",
        mark(hai_ok),
        s(hai_tier.get("latency_ms")),
        s(account["detail"].get("balance_jpy")),
        if account["detail"]["low_balance"].as_bool().unwrap_or(false) {
            "  (LOW)"
        } else {
            ""
        }
    );
    all_ok &= hai_ok;

    let kcore = &status["kcore"];
    if !kcore.is_null() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "K-CORE  state={} restarts={} last_output={}",
            s(kcore.get("state")),
            s(kcore.get("restarts")),
            s(kcore["last_output"].get("at"))
        );
    }
    if let Some(servers) = status["mcp"].as_array().filter(|s| !s.is_empty()) {
        let _ = writeln!(out);
        let _ = writeln!(out, "MCP");
        for server in servers {
            let running = server["state"] == "running";
            all_ok &= running;
            let _ = writeln!(
                out,
                "  [{}] {:<11} tools={} {}",
                mark(running),
                s(server.get("name")),
                s(server.get("tools")),
                if running {
                    String::new()
                } else {
                    format!("error={}", s(server.get("error")))
                }
            );
        }
    }
    if let Some(jobs) = status["jobs"].as_object().filter(|j| !j.is_empty()) {
        let _ = writeln!(out);
        let _ = writeln!(out, "JOBS");
        for (name, job) in jobs {
            let ok = job["ok"].as_bool().unwrap_or(false);
            all_ok &= ok;
            let detail = if ok {
                String::new()
            } else {
                format!(
                    "error={} {}",
                    s(job.get("error")),
                    job["stderr_tail"]
                        .as_array()
                        .and_then(|t| t.last())
                        .and_then(Value::as_str)
                        .unwrap_or("")
                )
            };
            let _ = writeln!(
                out,
                "  [{}] {:<11} last={} {}",
                mark(ok),
                name,
                s(job.get("last_run")),
                detail
            );
        }
    }
    if let Some(pending) = status["approvals_pending"]
        .as_array()
        .filter(|p| !p.is_empty())
    {
        let _ = writeln!(out);
        let _ = writeln!(out, "APPROVALS PENDING ({})", pending.len());
        for a in pending {
            let _ = writeln!(out, "  {}", describe_approval(a));
        }
    }
    let snapshot = &status["snapshot"];
    if !snapshot.is_null() {
        let _ = writeln!(
            out,
            "SNAPSHOT  ok={} at={} {}",
            s(snapshot.get("ok")),
            s(snapshot.get("at")),
            s(snapshot.get("error").or(snapshot.get("staged")))
        );
    }
    (out, all_ok)
}

/// One line for an approval request: id, tool and the most telling argument.
pub fn describe_approval(a: &Value) -> String {
    let args = &a["arguments"];
    let target = ["path", "file", "note", "oldPath", "url"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .unwrap_or("");
    let preview: String = ["content", "newString", "text"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .chars()
        .take(80)
        .collect::<String>()
        .replace('\n', "⏎");
    format!(
        "{}  {}  {}  {}{}",
        s(a.get("id")),
        s(a.get("exposed")),
        target,
        if preview.is_empty() { "" } else { "« " },
        if preview.is_empty() {
            String::new()
        } else {
            format!("{preview} »")
        }
    )
}
