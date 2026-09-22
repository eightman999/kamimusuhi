//! OpenAI-compatible chat routing across tiers.
//!
//! Order comes from configuration (`local` → `llm_master` → `hai`). Healthy
//! tiers are tried first in that order; a tier that fails a live request is
//! marked unhealthy and the next one is tried. Health probes restore a tier
//! as soon as it answers again, so the primary is used again automatically
//! after recovery.

use std::time::{Duration, Instant};

use kamimusuhi_resource_http::{Endpoint, Header, TrustAnchors, http};
use serde_json::{Map, Value, json};

use crate::config::{TierCondition, TierConfig};
use crate::probes::{ROUTE_HEADER, bearer, describe};
use crate::state::{RouteEvent, Shared};
use crate::util::unix_now;

/// Client-visible model name meaning "let Kamimusuhi route".
pub const AUTO_MODELS: [&str; 3] = ["kamimusuhi", "auto", "k0"];

#[derive(Debug)]
pub struct RouteRequest {
    pub body: Value,
    /// Peer asked to be served from node-local tiers only.
    pub local_only: bool,
}

#[derive(Debug)]
pub struct Routed {
    pub status: u16,
    pub body: Value,
    pub tier: Option<String>,
    pub attempts: Vec<String>,
}

fn prompt_chars(body: &Value) -> usize {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|m| match m.get("content") {
                    Some(Value::String(s)) => s.chars().count(),
                    Some(other) => other.to_string().len(),
                    None => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Tiers to try, in order, with the backend model for each.
pub fn plan<'a>(
    shared: &'a Shared,
    request: &RouteRequest,
) -> Result<Vec<(&'a TierConfig, String)>, String> {
    let tiers = &shared.config.tiers;
    let requested = request
        .body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("kamimusuhi");

    // Forced tier: `hai` or `hai/glm-5.3`.
    if !AUTO_MODELS.contains(&requested) {
        let (name, model) = requested.split_once('/').unwrap_or((requested, ""));
        let tier = tiers
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("unknown model or tier {requested:?}"))?;
        if request.local_only && !tier.node_local {
            return Err(format!("tier {name} is not local to this node"));
        }
        let model = if model.is_empty() {
            tier.model.clone()
        } else {
            model.to_owned()
        };
        return Ok(vec![(tier, model)]);
    }

    let small = requested == "k0"
        || prompt_chars(&request.body) <= shared.config.routing.small_request_chars;
    let eligible: Vec<&TierConfig> = tiers
        .iter()
        .filter(|t| !request.local_only || t.node_local)
        .filter(|t| t.condition == TierCondition::Always || small)
        .collect();
    let (healthy, unhealthy): (Vec<&TierConfig>, Vec<&TierConfig>) = eligible
        .into_iter()
        .partition(|t| shared.tier_healthy(&t.name));
    // Known-down tiers are only tried when nothing is known-healthy, so an
    // outage does not add a timeout to every request.
    let chosen = if healthy.is_empty() {
        unhealthy
    } else {
        healthy
    };
    if chosen.is_empty() {
        return Err("no eligible tier".to_owned());
    }
    Ok(chosen.into_iter().map(|t| (t, t.model.clone())).collect())
}

fn call_tier(tier: &TierConfig, model: &str, body: &Value) -> Result<Value, String> {
    let mut upstream = body.clone();
    if let Value::Object(map) = &mut upstream {
        map.insert("model".into(), Value::String(model.to_owned()));
        // Streaming is re-synthesized by the server from the full reply.
        map.insert("stream".into(), Value::Bool(false));
        map.remove("stream_options");
    }
    let mut headers: Vec<Header> = bearer(tier.auth_env.as_deref())?;
    if tier.peer_local_only {
        headers.push(Header {
            name: ROUTE_HEADER.to_owned(),
            value: "local".to_owned(),
        });
    }
    let endpoint = Endpoint::parse(&tier.base_url, "/chat/completions")?;
    let response = http::post_json(
        &endpoint,
        &upstream.to_string(),
        &headers,
        Duration::from_secs(tier.timeout_secs),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| describe(&e))?;
    if !response.is_success() {
        return Err(format!("HTTP {}", response.status));
    }
    let value: Value =
        serde_json::from_str(&response.body).map_err(|_| "reply is not JSON".to_owned())?;
    let has_choice = value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|c| !c.is_empty());
    if !has_choice {
        return Err("reply has no choices".to_owned());
    }
    if response_kind(&value).is_none() {
        // Reasoning is not a final expression or an executable tool call.
        // Do not accept a reasoning-only 200 and prevent a usable fallback.
        // Only a closed metadata vocabulary can enter diagnostic records.
        return Err(format!(
            "reply has no usable content, tool calls or refusal (finish_reason={})",
            completion_finish_reason(&value)
        ));
    }
    Ok(value)
}

fn nonblank_string(value: &Value) -> bool {
    value.as_str().is_some_and(|text| !text.trim().is_empty())
}

/// The first choice is the one consumed by Persona and the SSE adapter.
/// Preserve refusals and standard function calls without promoting reasoning.
fn response_kind(value: &Value) -> Option<&'static str> {
    let message = value.pointer("/choices/0/message")?;
    if nonblank_string(&message["refusal"]) {
        return Some("refusal");
    }
    if nonblank_string(&message["content"]) {
        return Some("content");
    }
    let calls = message["tool_calls"].as_array()?;
    (!calls.is_empty()
        && calls.iter().all(|call| {
            call["type"] == "function"
                && nonblank_string(&call["id"])
                && nonblank_string(&call["function"]["name"])
                && call["function"]["arguments"].is_string()
        }))
    .then_some("tool_calls")
}

/// Upstream metadata is untrusted too; never copy arbitrary provider text
/// into failure diagnostics or the metadata-only completion fields.
fn completion_finish_reason(value: &Value) -> &'static str {
    match value
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
    {
        Some("stop") => "stop",
        Some("length") => "length",
        Some("tool_calls") => "tool_calls",
        Some("function_call") => "function_call",
        Some("content_filter") => "content_filter",
        Some(_) => "unknown",
        None => "missing",
    }
}

pub fn route(shared: &Shared, request: &RouteRequest) -> Routed {
    let started = Instant::now();
    let plan = match plan(shared, request) {
        Ok(plan) => plan,
        Err(e) => {
            return Routed {
                status: 400,
                body: error_body(&e, "invalid_request_error"),
                tier: None,
                attempts: Vec::new(),
            };
        }
    };
    let mut attempts = Vec::new();
    let mut result = None;
    for (tier, model) in plan {
        let tier_started = Instant::now();
        match call_tier(tier, &model, &request.body) {
            Ok(mut body) => {
                if let Value::Object(map) = &mut body {
                    map.insert(
                        "kamimusuhi_route".into(),
                        json!({"tier": tier.name, "model": model, "attempts": attempts.clone()}),
                    );
                }
                attempts.push(format!("{}:ok", tier.name));
                result = Some((tier.name.clone(), body));
                break;
            }
            Err(e) => {
                let ms = u64::try_from(tier_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                shared.set_tier(&tier.name, Err(format!("request failed: {e}")), ms);
                attempts.push(format!("{}:{e}", tier.name));
            }
        }
    }
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let event = RouteEvent {
        at: unix_now(),
        tier: result.as_ref().map(|(t, _)| t.clone()),
        attempts: attempts.clone(),
        latency_ms,
        ok: result.is_some(),
    };
    let _ = shared.spool.append(
        "logs/routing",
        serde_json::to_value(&event).unwrap_or(Value::Null),
    );
    *shared.last_route.write().unwrap_or_else(|p| p.into_inner()) = Some(event);

    match result {
        Some((tier, body)) => {
            if shared.config.routing.log_conversations && !request.local_only {
                let _ = shared.spool.append(
                    "conversations",
                    json!({
                        "tier": tier,
                        "latency_ms": latency_ms,
                        "request": {"model": request.body.get("model"), "messages": request.body.get("messages")},
                        "response": body.pointer("/choices/0/message"),
                        "response_kind": response_kind(&body),
                        "finish_reason": completion_finish_reason(&body),
                        "usage": body.get("usage"),
                    }),
                );
            }
            Routed {
                status: 200,
                body,
                tier: Some(tier),
                attempts,
            }
        }
        None => Routed {
            status: 503,
            body: error_body(
                &format!("all tiers failed: {}", attempts.join("; ")),
                "service_unavailable",
            ),
            tier: None,
            attempts,
        },
    }
}

fn error_body(message: &str, kind: &str) -> Value {
    json!({"error": {"message": message, "type": kind}})
}

/// Re-express a complete chat completion as an SSE stream.
pub fn to_sse(body: &Value) -> String {
    let id = body
        .get("id")
        .cloned()
        .unwrap_or_else(|| json!("kamimusuhi"));
    let model = body.get("model").cloned().unwrap_or(Value::Null);
    let created = body
        .get("created")
        .cloned()
        .unwrap_or_else(|| json!(unix_now()));
    let message = body
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let finish = body
        .pointer("/choices/0/finish_reason")
        .cloned()
        .unwrap_or_else(|| json!("stop"));
    let mut delta = Map::new();
    delta.insert("role".into(), json!("assistant"));
    for key in ["content", "refusal", "reasoning_content", "tool_calls"] {
        if let Some(v) = message.get(key).filter(|v| !v.is_null()) {
            delta.insert(key.into(), v.clone());
        }
    }
    let chunk = |delta: Value, finish: Value| {
        json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model,
               "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    };
    let mut out = String::new();
    for event in [
        chunk(Value::Object(delta), Value::Null),
        chunk(json!({}), finish),
    ] {
        out.push_str("data: ");
        out.push_str(&event.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::spool::Spool;

    fn shared() -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config: Config = serde_json::from_value(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "tiers": [
                {"name": "local", "base_url": "http://127.0.0.1:9/v1", "model": "tiny",
                 "condition": "small_request", "node_local": true},
                {"name": "llm_master", "base_url": "http://127.0.0.1:9/v1", "model": "big"},
                {"name": "hai", "base_url": "https://example.invalid/v1", "model": "q"}
            ]
        }))
        .expect("config");
        let spool = Spool::new(dir.path().join("spool"), "pi").expect("spool");
        (Shared::new(config, spool, 1, None), dir)
    }

    fn names(plan: &[(&TierConfig, String)]) -> Vec<String> {
        plan.iter().map(|(t, _)| t.name.clone()).collect()
    }

    fn completion_server(reply: Value) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let url = format!("http://{}/v1", listener.local_addr().expect("address"));
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).expect("request header") > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse::<usize>().expect("length");
                }
            }
            reader
                .read_exact(&mut vec![0; length])
                .expect("request body");
            let body = reply.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            )
            .expect("reply");
        });
        (url, handle)
    }

    #[test]
    fn response_shapes_keep_content_refusals_and_standard_tool_calls() {
        let completion = |message| json!({"choices": [{"message": message}]});
        assert_eq!(
            response_kind(&completion(json!({"content": "こんにちは"}))),
            Some("content")
        );
        assert_eq!(
            response_kind(&completion(
                json!({"content": null, "refusal": "対応できません"})
            )),
            Some("refusal")
        );
        assert_eq!(
            response_kind(&completion(json!({"content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {
                    "name": "library_search", "arguments": "{\"query\":\"Rust\"}"
                }}
            ]}))),
            Some("tool_calls")
        );
        for message in [
            json!({"content": "", "reasoning_content": "internal reasoning"}),
            json!({"content": " \n\t", "refusal": " ", "tool_calls": []}),
            json!({"content": null, "tool_calls": [{}]}),
            json!({"tool_calls": [{"id": "c", "type": "function", "function": {
                "name": " ", "arguments": "{}"
            }}]}),
        ] {
            assert_eq!(response_kind(&completion(message)), None);
        }
    }

    #[test]
    fn reasoning_only_http_200_falls_back_without_leaking_provider_text() {
        let (bad_url, bad_server) = completion_server(json!({"choices": [{
            "message": {"role": "assistant", "content": " \n",
                        "reasoning_content": "PRIVATE_REASONING"},
            "finish_reason": "PRIVATE_PROVIDER_METADATA"
        }]}));
        let (good_url, good_server) = completion_server(json!({"choices": [{
            "message": {"role": "assistant", "content": "確認できました"},
            "finish_reason": "stop"
        }]}));
        let (mut s, _dir) = shared();
        let mut bad = s.config.tiers[2].clone();
        bad.base_url = bad_url;
        let mut good = s.config.tiers[1].clone();
        good.base_url = good_url;
        s.config.tiers = vec![bad, good];
        let req = RouteRequest {
            body: json!({"model": "kamimusuhi", "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        let routed = route(&s, &req);
        assert_eq!(routed.status, 200);
        assert_eq!(routed.tier.as_deref(), Some("llm_master"));
        assert_eq!(
            routed.body["choices"][0]["message"]["content"],
            "確認できました"
        );
        assert_eq!(routed.attempts.len(), 2);
        assert!(routed.attempts[0].contains("finish_reason=unknown"));
        assert!(!routed.attempts.join(" ").contains("PRIVATE_"));
        assert!(!s.tier_healthy("hai"));
        bad_server.join().expect("bad fixture finished");
        good_server.join().expect("good fixture finished");

        let directory = s.spool.root().join("conversations/pi");
        let log = std::fs::read_dir(directory)
            .expect("conversation log directory")
            .next()
            .expect("conversation log")
            .expect("entry")
            .path();
        let record: Value =
            serde_json::from_str(std::fs::read_to_string(log).expect("read log").trim())
                .expect("record");
        assert_eq!(record["finish_reason"], "stop");
        assert_eq!(record["response_kind"], "content");
    }

    #[test]
    fn finish_reason_metadata_uses_only_known_values() {
        for reason in [
            "stop",
            "length",
            "tool_calls",
            "function_call",
            "content_filter",
        ] {
            assert_eq!(
                completion_finish_reason(&json!({"choices": [{"finish_reason": reason}]})),
                reason
            );
        }
        assert_eq!(completion_finish_reason(&json!({})), "missing");
        assert_eq!(
            completion_finish_reason(&json!({"choices": [{"finish_reason": "private text"}]})),
            "unknown"
        );
    }

    #[test]
    fn prefers_healthy_tiers_in_order_and_recovers() {
        let (s, _dir) = shared();
        let long = "x".repeat(5000);
        let req = RouteRequest {
            body: json!({"model": "kamimusuhi", "messages": [{"role": "user", "content": long}]}),
            local_only: false,
        };
        // Nothing probed yet: try everything eligible (local is not: too long).
        assert_eq!(names(&plan(&s, &req).expect("plan")), ["llm_master", "hai"]);

        s.set_tier("hai", Ok(Value::Null), 1);
        assert_eq!(
            names(&plan(&s, &req).expect("plan")),
            ["hai"],
            "llm_master down → HAI"
        );

        s.set_tier("llm_master", Ok(Value::Null), 1);
        assert_eq!(
            names(&plan(&s, &req).expect("plan")),
            ["llm_master", "hai"],
            "recovered primary is preferred again"
        );
    }

    #[test]
    fn small_requests_may_use_local() {
        let (s, _dir) = shared();
        for t in ["local", "llm_master", "hai"] {
            s.set_tier(t, Ok(Value::Null), 1);
        }
        let req = RouteRequest {
            body: json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        assert_eq!(
            names(&plan(&s, &req).expect("plan")),
            ["local", "llm_master", "hai"]
        );
    }

    #[test]
    fn forced_and_local_only() {
        let (s, _dir) = shared();
        let forced = RouteRequest {
            body: json!({"model": "hai/glm-5.3", "messages": []}),
            local_only: false,
        };
        let p = plan(&s, &forced).expect("plan");
        assert_eq!(p[0].0.name, "hai");
        assert_eq!(p[0].1, "glm-5.3");

        let local = RouteRequest {
            body: json!({"model": "kamimusuhi", "messages": [{"role": "user", "content": "hi"}]}),
            local_only: true,
        };
        assert_eq!(names(&plan(&s, &local).expect("plan")), ["local"]);
        let bad = RouteRequest {
            body: json!({"model": "hai", "messages": []}),
            local_only: true,
        };
        assert!(plan(&s, &bad).is_err());
    }

    #[test]
    fn failing_tiers_fall_through_to_503() {
        let (s, _dir) = shared();
        let req = RouteRequest {
            body: json!({"model": "llm_master", "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        let routed = route(&s, &req);
        assert_eq!(routed.status, 503);
        assert!(!s.tier_healthy("llm_master"));
    }

    #[test]
    fn sse_carries_content() {
        let sse = to_sse(
            &json!({"id": "x", "choices": [{"message": {"role": "assistant", "content": "やあ"}, "finish_reason": "stop"}]}),
        );
        assert!(sse.contains("やあ"));
        assert!(sse.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn sse_preserves_refusal_without_promoting_reasoning() {
        let sse = to_sse(&json!({"choices": [{"message": {
            "role": "assistant", "content": null, "refusal": "対応できません",
            "reasoning_content": "internal reasoning"
        }, "finish_reason": "stop"}]}));
        let chunk: Value = serde_json::from_str(
            sse.lines()
                .next()
                .expect("chunk")
                .strip_prefix("data: ")
                .expect("SSE data"),
        )
        .expect("JSON chunk");
        assert_eq!(chunk["choices"][0]["delta"]["refusal"], "対応できません");
        assert!(chunk["choices"][0]["delta"].get("content").is_none());
    }
}
