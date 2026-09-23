//! OpenAI-compatible chat routing across tiers.
//!
//! Order comes from configuration (`local` → `llm_master` → `hai`). Healthy
//! tiers are tried first in that order; a tier that fails a live request is
//! marked unhealthy and the next one is tried. Health probes restore a tier
//! as soon as it answers again, so the primary is used again automatically
//! after recovery.

use std::time::{Duration, Instant};

use kamimusuhi_resource_http::{Endpoint, Header, TrustAnchors, http};
use kamimusuhi_runtime::route_gate::{BillingClass, RouteGate, RouteLane, RouteProvider};
use serde_json::{Map, Value, json};

use crate::config::{TierBilling, TierCondition, TierConfig};
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

impl RouteProvider for TierConfig {
    fn route_id(&self) -> &str {
        &self.name
    }

    fn billing(&self) -> BillingClass {
        match self.billing.unwrap_or(TierBilling::Metered) {
            TierBilling::Local => BillingClass::Local,
            TierBilling::Subscription => BillingClass::Subscription,
            TierBilling::FreeTier => BillingClass::FreeTier,
            TierBilling::Metered => BillingClass::Metered,
        }
    }

    fn privacy_ok_for_private_memory(&self) -> bool {
        self.privacy_ok_for_private_memory.unwrap_or(matches!(
            self.billing,
            Some(TierBilling::Local) | Some(TierBilling::Subscription)
        ))
    }

    fn context_limit_tokens(&self) -> Option<u64> {
        self.context_limit_tokens
    }

    fn approx_tpm_limit(&self) -> Option<u64> {
        self.approx_tpm_limit
    }

    fn reasoning_suppression(&self) -> bool {
        self.reasoning_suppression.unwrap_or(false)
    }

    fn route_available(&self) -> bool {
        self.billing.is_some() && !self.base_url.is_empty()
    }
}

fn request_private(shared: &Shared, request: &RouteRequest) -> bool {
    request
        .body
        .get("kamimusuhi_private")
        .and_then(Value::as_bool)
        .unwrap_or(shared.config.routing.private_by_default)
}

fn route_lane(request: &RouteRequest) -> RouteLane {
    if request.local_only || request.body.get("model").and_then(Value::as_str) == Some("k0") {
        return RouteLane::LocalChat;
    }
    match request
        .body
        .get("kamimusuhi_route_lane")
        .and_then(Value::as_str)
    {
        Some("LOCAL_CHAT") => RouteLane::LocalChat,
        Some("DEEP_REASONING") => RouteLane::DeepReasoning,
        Some("TOOL_TASK") => RouteLane::ToolTask,
        Some("MEMORY_HEAVY") => RouteLane::MemoryHeavy,
        _ => RouteLane::FastChat,
    }
}

fn prompt_tokens_est(body: &Value) -> u64 {
    // Same conservative approximation used by provider-bench.  The resident
    // sees the fully assembled OpenAI-compatible body, so this includes the
    // persona prefix/history rather than only the latest user message.
    let bytes = u64::try_from(body.to_string().len()).unwrap_or(u64::MAX);
    bytes.saturating_add(2) / 3
}

fn completion_tokens_for_cost(shared: &Shared, body: &Value) -> u32 {
    ["max_completion_tokens", "max_tokens"]
        .into_iter()
        .find_map(|key| body.get(key).and_then(Value::as_u64))
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(shared.config.routing.max_completion_tokens_for_cost)
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
    let private = request_private(shared, request);

    // Forced tier: `hai` or `hai/glm-5.3`. Explicit routing still obeys
    // the privacy wall and local-only boundary; cost limits are enforced at
    // call time below.
    if !AUTO_MODELS.contains(&requested) {
        let (name, model) = requested.split_once('/').unwrap_or((requested, ""));
        let tier = tiers
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("unknown model or tier {requested:?}"))?;
        if request.local_only && !tier.node_local {
            return Err(format!("tier {name} is not local to this node"));
        }
        if tier.billing.is_none() {
            return Err(format!("tier {name} has no declared billing class"));
        }
        if private && !tier.privacy_ok_for_private_memory() {
            return Err(format!("tier {name} is not eligible for private memory"));
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
    let lane = route_lane(request);
    let gate = RouteGate {
        fast_chat_ttft_ceiling_ms: Some(shared.config.routing.fast_chat_latency_ceiling_ms),
        ..Default::default()
    };
    let health = shared
        .route_health
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let candidates = gate.select(
        lane,
        tiers,
        prompt_tokens_est(&request.body),
        private,
        &health,
        Instant::now(),
    );
    drop(health);

    let eligible: Vec<&TierConfig> = candidates
        .into_iter()
        .map(|candidate| candidate.spec)
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
fn call_tier(
    tier: &TierConfig,
    model: &str,
    body: &Value,
    default_max_tokens: Option<u32>,
) -> Result<Value, String> {
    let mut upstream = body.clone();
    if let Value::Object(map) = &mut upstream {
        map.insert("model".into(), Value::String(model.to_owned()));
        // Streaming is re-synthesized by the server from the full reply.
        map.insert("stream".into(), Value::Bool(false));
        map.remove("stream_options");
        // Host-only policy hints must never be forwarded to providers.
        map.remove("kamimusuhi_private");
        map.remove("kamimusuhi_route_lane");
        if let Some(max_tokens) = default_max_tokens
            && !map.contains_key("max_tokens")
            && !map.contains_key("max_completion_tokens")
        {
            // OpenAI-compatible providers universally used by this router
            // accept max_tokens; bounding output makes the cost pre-flight
            // estimate an enforceable upper bound for metered fallbacks.
            map.insert("max_tokens".into(), Value::from(max_tokens));
        }
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

struct GuardedReply {
    body: Value,
    cost_usd: Option<f64>,
    cost_kind: Option<&'static str>,
}

fn call_tier_guarded(
    shared: &Shared,
    tier: &TierConfig,
    model: &str,
    body: &Value,
) -> Result<GuardedReply, String> {
    if tier.billing != Some(TierBilling::Metered) {
        let reply = call_tier(tier, model, body, None)?;
        let usage = usage_of(&reply);
        let (cost_usd, cost_kind) = cost_of(tier, &usage);
        return Ok(GuardedReply {
            body: reply,
            cost_usd,
            cost_kind,
        });
    }
    let price = tier
        .input_usd_per_mtok
        .zip(tier.output_usd_per_mtok)
        .ok_or_else(|| "cost guard: metered tier has no declared price".to_owned())?;
    let prompt_tokens = prompt_tokens_est(body);
    let max_completion_tokens = completion_tokens_for_cost(shared, body);
    // Keep the guard locked through the paid call. Metered fallbacks are
    // rare, and serializing only those calls prevents concurrent requests
    // from both reserving the same remaining daily/monthly budget.
    let mut guard = shared.cost_guard.lock().unwrap_or_else(|p| p.into_inner());
    let estimate = guard.estimate(Some(price), prompt_tokens, max_completion_tokens);
    guard
        .permit(estimate)
        .map_err(|error| format!("cost guard: {error}"))?;
    match call_tier(tier, model, body, Some(max_completion_tokens)) {
        Ok(reply) => {
            let usage = usage_of(&reply);
            let (reported_cost, reported_kind) = cost_of(tier, &usage);
            let cost_usd = reported_cost.or(estimate);
            let cost_kind = reported_kind.or_else(|| estimate.map(|_| "estimate"));
            if let Some(cost) = cost_usd {
                guard.record_cost(cost);
            } else {
                guard.record_unpriced();
            }
            Ok(GuardedReply {
                body: reply,
                cost_usd,
                cost_kind,
            })
        }
        Err(error) => {
            // A provider may bill work even when the reply times out, is
            // malformed, or is rejected by our final-response checks. Charge
            // the pre-flight upper-bound estimate to the local budget rather
            // than assuming a failed request was free.
            if let Some(estimate) = estimate {
                guard.record_cost(estimate);
            } else {
                guard.record_unpriced();
            }
            Err(error)
        }
    }
}

pub fn route(shared: &Shared, request: &RouteRequest) -> Routed {
    let started = Instant::now();
    let plan = match plan(shared, request) {
        Ok(plan) => plan,
        Err(e) => {
            let event = RouteEvent {
                at: unix_now(),
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                ..RouteEvent::default()
            };
            shared.usage.record_request(&event, true);
            let _ = shared.spool.append(
                "logs/routing",
                serde_json::to_value(&event).unwrap_or(Value::Null),
            );
            *shared.last_route.write().unwrap_or_else(|p| p.into_inner()) = Some(event);
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
        match call_tier_guarded(shared, tier, &model, &request.body) {
            Ok(GuardedReply {
                mut body,
                cost_usd,
                cost_kind,
            }) => {
                let ms = u64::try_from(tier_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let usage = usage_of(&body);
                let actual_model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|model| !model.trim().is_empty())
                    .unwrap_or(&model)
                    .to_owned();
                shared.usage.record_attempt(&RouteEvent {
                    at: unix_now(),
                    tier: Some(tier.name.clone()),
                    model: Some(actual_model.clone()),
                    billing: tier.billing.map(|b| b.as_str().to_owned()),
                    latency_ms: ms,
                    ok: true,
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    cached_tokens: usage.cached_tokens,
                    cost_usd,
                    cost_kind: cost_kind.map(str::to_owned),
                    ..RouteEvent::default()
                });
                shared.set_tier(&tier.name, Ok(Value::Null), ms);
                shared
                    .route_health
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get_mut(&tier.name)
                    .observe(
                        true,
                        Some(ms),
                        false,
                        usage.cached_tokens,
                        Duration::from_secs(60),
                        Instant::now(),
                    );
                if let Value::Object(map) = &mut body {
                    map.insert(
                        "kamimusuhi_route".into(),
                        json!({
                            "tier": tier.name,
                            "model": actual_model,
                            "billing": tier.billing.map(|b| b.as_str()),
                            "cost_usd": cost_usd,
                            "cost_kind": cost_kind,
                            "prompt_tokens": usage.prompt_tokens,
                            "completion_tokens": usage.completion_tokens,
                            "cached_tokens": usage.cached_tokens,
                            "attempts": attempts.clone(),
                        }),
                    );
                }
                attempts.push(format!("{}:ok", tier.name));
                result = Some((tier, body, actual_model, usage, cost_usd, cost_kind));
                break;
            }
            Err(e) => {
                let ms = u64::try_from(tier_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                if !e.starts_with("cost guard:") {
                    shared.usage.record_attempt(&RouteEvent {
                        at: unix_now(),
                        tier: Some(tier.name.clone()),
                        model: Some(model.clone()),
                        billing: tier.billing.map(|b| b.as_str().to_owned()),
                        latency_ms: ms,
                        ..RouteEvent::default()
                    });
                    shared.set_tier(&tier.name, Err(format!("request failed: {e}")), ms);
                    shared
                        .route_health
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get_mut(&tier.name)
                        .observe(
                            false,
                            None,
                            e == "HTTP 429",
                            None,
                            Duration::from_secs(60),
                            Instant::now(),
                        );
                }
                attempts.push(format!("{}:{e}", tier.name));
            }
        }
    }
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let event = RouteEvent {
        at: unix_now(),
        tier: result.as_ref().map(|(t, ..)| t.name.clone()),
        model: result.as_ref().map(|(_, _, m, ..)| m.clone()),
        billing: result
            .as_ref()
            .and_then(|(t, ..)| t.billing.map(|b| b.as_str().to_owned())),
        attempts: attempts.clone(),
        latency_ms,
        ok: result.is_some(),
        prompt_tokens: result.as_ref().and_then(|(.., u, _, _)| u.prompt_tokens),
        completion_tokens: result
            .as_ref()
            .and_then(|(.., u, _, _)| u.completion_tokens),
        cached_tokens: result.as_ref().and_then(|(.., u, _, _)| u.cached_tokens),
        cost_usd: result.as_ref().and_then(|(.., c, _)| *c),
        cost_kind: result.as_ref().and_then(|(.., k)| k.map(str::to_owned)),
    };
    let _ = shared.spool.append(
        "logs/routing",
        serde_json::to_value(&event).unwrap_or(Value::Null),
    );
    shared.usage.record_request(&event, false);
    *shared.last_route.write().unwrap_or_else(|p| p.into_inner()) = Some(event);

    match result {
        Some((tier, body, _model, ..)) => {
            if shared.config.routing.log_conversations && !request.local_only {
                let _ = shared.spool.append(
                    "conversations",
                    json!({
                        "tier": tier.name,
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
                tier: Some(tier.name.clone()),
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

/// Token accounting from a completion's `usage` block — the same shapes
/// the provider bench records, plus upstream-reported billed cost
/// (`usage.cost_usd`, OrcaRouter's `X-OrcaRouter-Include-Cost` field, or
/// OpenRouter's `usage.cost`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TurnUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Prompt tokens served from the provider's prefix cache.
    pub cached_tokens: Option<u64>,
    /// Billed cost the upstream reported on the response, when present.
    pub actual_cost_usd: Option<f64>,
}

fn usage_of(body: &Value) -> TurnUsage {
    let usage = &body["usage"];
    let num = |key: &str| usage.get(key).and_then(Value::as_u64);
    let cost = usage
        .get("cost_usd")
        .or_else(|| usage.get("cost"))
        .and_then(Value::as_f64);
    TurnUsage {
        prompt_tokens: num("prompt_tokens"),
        completion_tokens: num("completion_tokens"),
        cached_tokens: usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .or_else(|| num("prompt_cache_hit_tokens")),
        actual_cost_usd: cost,
    }
}

/// Cost basis for one served request. Actual billed cost wins; a tier with
/// configured prices yields an estimate; otherwise `None` — an undeclared
/// plan's cost is unknown, not free.
fn cost_of(tier: &TierConfig, usage: &TurnUsage) -> (Option<f64>, Option<&'static str>) {
    if let Some(cost) = usage.actual_cost_usd {
        return (Some(cost), Some("actual"));
    }
    if let (Some(input), Some(output), Some(prompt_tokens), Some(completion_tokens)) = (
        tier.input_usd_per_mtok,
        tier.output_usd_per_mtok,
        usage.prompt_tokens,
        usage.completion_tokens,
    ) {
        let tokens = prompt_tokens as f64 * input + completion_tokens as f64 * output;
        return (Some(tokens / 1e6), Some("estimate"));
    }
    // Missing usage cannot safely be interpreted as zero tokens. The
    // production CostGuard still books its pre-flight upper-bound estimate,
    // while the UI reports the amount as unknown rather than "$0.0000".
    (None, None)
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
                {"name": "local", "billing": "local", "base_url": "http://127.0.0.1:9/v1", "model": "tiny",
                 "condition": "small_request", "node_local": true,
                 "privacy_ok_for_private_memory": true, "reasoning_suppression": true},
                {"name": "llm_master", "billing": "local", "base_url": "http://127.0.0.1:9/v1", "model": "big",
                 "privacy_ok_for_private_memory": true, "reasoning_suppression": true},
                {"name": "hai", "billing": "subscription", "base_url": "https://example.invalid/v1", "model": "q",
                 "privacy_ok_for_private_memory": true, "reasoning_suppression": true}
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
    fn usage_and_cost_are_extracted_for_the_route_event() {
        let body = json!({
            "usage": {
                "prompt_tokens": 1200,
                "completion_tokens": 40,
                "prompt_tokens_details": {"cached_tokens": 512}
            }
        });
        let usage = usage_of(&body);
        assert_eq!(usage.prompt_tokens, Some(1200));
        assert_eq!(usage.completion_tokens, Some(40));
        assert_eq!(usage.cached_tokens, Some(512));
        assert_eq!(usage.actual_cost_usd, None);

        // Upstream-reported billed cost wins over the configured estimate.
        let billed = json!({"usage": {"prompt_tokens": 1, "cost_usd": 0.007}});
        let usage = usage_of(&billed);
        let mut tier = TierConfig {
            name: "t".into(),
            base_url: "http://x/v1".into(),
            model: "m".into(),
            auth_env: None,
            timeout_secs: 1,
            condition: TierCondition::Always,
            node_local: false,
            peer_local_only: false,
            probe_interval_secs: 1,
            billing: Some(crate::config::TierBilling::Metered),
            input_usd_per_mtok: Some(1.0),
            output_usd_per_mtok: Some(2.0),
            privacy_ok_for_private_memory: Some(true),
            context_limit_tokens: None,
            approx_tpm_limit: None,
            reasoning_suppression: Some(true),
        };
        let (cost, kind) = cost_of(&tier, &usage);
        assert_eq!(cost, Some(0.007));
        assert_eq!(kind, Some("actual"));

        // No actual cost → estimate from configured prices.
        let usage = usage_of(&json!({"usage": {"prompt_tokens": 1_000_000,
                                              "completion_tokens": 500_000}}));
        let (cost, kind) = cost_of(&tier, &usage);
        assert_eq!(cost, Some(2.0));
        assert_eq!(kind, Some("estimate"));

        // Missing usage is unknown, never a fabricated zero-dollar turn.
        let missing = usage_of(&json!({"choices": []}));
        let (cost, kind) = cost_of(&tier, &missing);
        assert_eq!(cost, None);
        assert_eq!(kind, None);

        // An undeclared plan reports no cost — never zero.
        tier.input_usd_per_mtok = None;
        let (cost, kind) = cost_of(&tier, &usage);
        assert_eq!(cost, None);
        assert_eq!(kind, None);
    }

    #[test]
    fn usage_records_invalid_routes_without_upstream_attempts_or_request_body() {
        let (s, _dir) = shared();
        let routed = route(
            &s,
            &RouteRequest {
                body: json!({"model": "nonexistent", "messages": [{"role": "user", "content": "private content"}]}),
                local_only: false,
            },
        );
        assert_eq!(routed.status, 400);
        let history = s.usage.history(10, None);
        let records = history["records"].as_array().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["kind"], "request");
        assert_eq!(records[0]["outcome"], "invalid");
        assert!(!history.to_string().contains("private content"));
    }

    #[test]
    fn usage_records_failed_tier_attempt_and_failed_request_separately() {
        let (s, _dir) = shared();
        let routed = route(
            &s,
            &RouteRequest {
                body: json!({"model": "local", "messages": [{"role": "user", "content": "hi"}]}),
                local_only: true,
            },
        );
        assert_eq!(routed.status, 503);
        let history = s.usage.history(10, None);
        let records = history["records"].as_array().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["kind"], "request");
        assert_eq!(records[0]["outcome"], "error");
        assert_eq!(records[1]["kind"], "attempt");
        assert_eq!(records[1]["model"], "tiny");
        assert_eq!(records[1]["outcome"], "error");
        assert!(records[1]["cost_usd"].is_null());
        assert!(!history.to_string().contains("127.0.0.1"));
    }

    #[test]
    fn usage_records_reported_model_and_excludes_cost_guard_rejections_from_attempts() {
        let (url, server) = completion_server(json!({
            "model": "tiny-version-2026",
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
        }));
        let (mut s, _dir) = shared();
        s.config.tiers[0].base_url = url;
        let routed = route(
            &s,
            &RouteRequest {
                body: json!({"model": "local"}),
                local_only: true,
            },
        );
        assert_eq!(routed.status, 200);
        assert_eq!(
            routed.body["kamimusuhi_route"]["model"],
            "tiny-version-2026"
        );
        assert_eq!(
            s.usage.history(10, None)["records"][0]["model"],
            "tiny-version-2026"
        );
        server.join().expect("fixture finished");

        let (mut s, _dir) = shared();
        s.config.tiers[0].billing = Some(TierBilling::Metered);
        let routed = route(
            &s,
            &RouteRequest {
                body: json!({"model": "local"}),
                local_only: true,
            },
        );
        assert_eq!(routed.status, 503);
        let history = s.usage.history(10, None);
        let records = history["records"].as_array().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["kind"], "request");
    }

    #[test]
    fn route_metadata_reaches_the_response_body() {
        let (url, server) = completion_server(json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5, "cost_usd": 0.001}
        }));
        let (mut s, _dir) = shared();
        let mut tier = s.config.tiers[2].clone();
        tier.base_url = url;
        tier.billing = Some(crate::config::TierBilling::Metered);
        tier.input_usd_per_mtok = Some(1.0);
        tier.output_usd_per_mtok = Some(1.0);
        tier.privacy_ok_for_private_memory = Some(true);
        s.config.tiers = vec![tier];
        let req = RouteRequest {
            body: json!({"model": "kamimusuhi",
                         "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        let routed = route(&s, &req);
        assert_eq!(routed.status, 200);
        let meta = &routed.body["kamimusuhi_route"];
        assert_eq!(meta["tier"], "hai");
        assert_eq!(meta["billing"], "metered");
        assert_eq!(meta["cost_usd"], 0.001);
        assert_eq!(meta["cost_kind"], "actual");
        assert_eq!(meta["prompt_tokens"], 100);
        let event = s
            .last_route
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .expect("route event");
        assert_eq!(event.model.as_deref(), Some("q"));
        assert_eq!(event.billing.as_deref(), Some("metered"));
        assert_eq!(event.cost_usd, Some(0.001));
        let history = s.usage.history(10, None);
        let records = history["records"].as_array().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["kind"], "request");
        assert_eq!(records[1]["kind"], "attempt");
        assert_eq!(records[1]["cost_kind"], "actual");
        server.join().expect("fixture finished");
    }

    #[test]
    fn metered_missing_usage_reports_the_preflight_estimate() {
        let (url, server) = completion_server(json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
        }));
        let (mut s, _dir) = shared();
        let mut tier = s.config.tiers[2].clone();
        tier.base_url = url;
        tier.auth_env = None;
        tier.billing = Some(crate::config::TierBilling::Metered);
        tier.input_usd_per_mtok = Some(1.0);
        tier.output_usd_per_mtok = Some(1.0);
        tier.privacy_ok_for_private_memory = Some(true);
        s.config.tiers = vec![tier];
        let req = RouteRequest {
            body: json!({"model": "kamimusuhi",
                         "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        let routed = route(&s, &req);
        assert_eq!(routed.status, 200);
        let meta = &routed.body["kamimusuhi_route"];
        assert_eq!(meta["cost_kind"], "estimate");
        assert!(meta["cost_usd"].as_f64().is_some_and(|cost| cost > 0.0));
        assert!(
            s.cost_guard
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .spent_usd()
                > 0.0
        );
        server.join().expect("fixture finished");
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
    fn production_plan_keeps_private_turns_off_untrusted_free_tiers() {
        let (mut s, _dir) = shared();
        let mut free = s.config.tiers[2].clone();
        free.name = "free".to_owned();
        free.billing = Some(crate::config::TierBilling::FreeTier);
        free.privacy_ok_for_private_memory = Some(false);
        free.auth_env = None;
        s.config.tiers.insert(0, free);

        let long = "x".repeat(5_000);
        let private = RouteRequest {
            body: json!({"model": "kamimusuhi",
                         "messages": [{"role": "user", "content": long.clone()}]}),
            local_only: false,
        };
        let private_names = names(&plan(&s, &private).expect("private plan"));
        assert!(!private_names.iter().any(|name| name == "free"));

        let public = RouteRequest {
            body: json!({"model": "kamimusuhi", "kamimusuhi_private": false,
                         "messages": [{"role": "user", "content": long}]}),
            local_only: false,
        };
        let public_names = names(&plan(&s, &public).expect("public plan"));
        assert_eq!(public_names.first().map(String::as_str), Some("free"));
    }

    #[test]
    fn production_cost_guard_blocks_expensive_metered_call_before_network() {
        let (mut s, _dir) = shared();
        let mut paid = s.config.tiers[2].clone();
        paid.name = "paid".to_owned();
        paid.base_url = "http://127.0.0.1:9/v1".to_owned();
        paid.auth_env = None;
        paid.billing = Some(crate::config::TierBilling::Metered);
        paid.input_usd_per_mtok = Some(1_000.0);
        paid.output_usd_per_mtok = Some(1_000.0);
        paid.privacy_ok_for_private_memory = Some(true);
        s.config.tiers = vec![paid];
        s.set_tier("paid", Ok(Value::Null), 1);

        let req = RouteRequest {
            body: json!({"model": "kamimusuhi",
                         "messages": [{"role": "user", "content": "hi"}]}),
            local_only: false,
        };
        let routed = route(&s, &req);
        assert_eq!(routed.status, 503);
        assert!(
            routed
                .attempts
                .iter()
                .any(|attempt| attempt.contains("cost guard:"))
        );
        assert!(
            s.tier_healthy("paid"),
            "budget rejection is policy, not provider health failure"
        );
        assert_eq!(
            s.cost_guard
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .spent_usd(),
            0.0,
            "blocked call never reaches the network and spends nothing"
        );
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
