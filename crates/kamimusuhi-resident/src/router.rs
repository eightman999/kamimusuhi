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
    Ok(value)
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
    for key in ["content", "reasoning_content", "tool_calls"] {
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
}
