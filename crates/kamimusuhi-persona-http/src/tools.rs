//! Tool calling against an MCP-like tool server (the resident's
//! `/v1/tools` surface).
//!
//! The model may request tools; this module executes them and returns the
//! results as `tool` messages. What a tool returns is external material: it
//! is recorded in [`ToolCallRecord`]s for the host to audit and is never the
//! expression itself, never evidence of the individual's experience, and
//! never mutation authority. Tool permissions and approvals belong to the
//! resident; the client additionally applies its configured allowlist.

use std::time::{Duration, Instant};

use kamimusuhi_core::persona::ToolCallRecord;
use kamimusuhi_resource_http::http::{Endpoint, Header, get_json, post_json};
use kamimusuhi_resource_http::tls::TrustAnchors;
use serde_json::{Value, json};

/// Where tools come from and how far a turn may use them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolServerConfig {
    /// Base URL of the tool server, e.g. `http://127.0.0.1:7860` (the
    /// server exposes `/v1/tools` and `/v1/tools/call`).
    pub base_url: String,
    /// Environment variable holding the bearer token, never the token.
    pub auth_env: Option<String>,
    /// Model round-trips that may request tools before a final answer.
    pub max_rounds: u32,
    /// Per tool-server request.
    pub timeout_ms: u64,
    /// Tool names offered to the model. Empty offers every tool. An entry
    /// ending in `*` matches by prefix (`mcp__context7__*`).
    pub allowed: Vec<String>,
}

/// Whether `name` is admitted by `allowed` (empty admits everything).
pub fn is_allowed(allowed: &[String], name: &str) -> bool {
    allowed.is_empty()
        || allowed.iter().any(|a| match a.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => a == name,
        })
}

impl ToolServerConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_env: None,
            max_rounds: 4,
            timeout_ms: 30_000,
            allowed: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.max_rounds == 0 || self.max_rounds > 16 {
            return Err("tools.max_rounds must be 1..=16".to_owned());
        }
        if self.timeout_ms == 0 {
            return Err("tools.timeout_ms is zero".to_owned());
        }
        Endpoint::parse(&self.base_url, "/v1/tools").map(|_| ())
    }

    fn headers(&self) -> Result<Vec<Header>, String> {
        let Some(name) = &self.auth_env else {
            return Ok(Vec::new());
        };
        match std::env::var(name) {
            Ok(token) if !token.trim().is_empty() => Ok(vec![Header {
                name: "Authorization".to_owned(),
                value: format!("Bearer {}", token.trim()),
            }]),
            _ => Err(format!("environment variable {name} is not set")),
        }
    }

    /// Tool definitions to offer, filtered by `allowed`. `Err` means tools
    /// are unavailable this turn; the caller answers without them.
    pub fn definitions(&self, anchors: &TrustAnchors) -> Result<Vec<Value>, String> {
        let endpoint = Endpoint::parse(&self.base_url, "/v1/tools")?;
        let response = get_json(
            &endpoint,
            &self.headers()?,
            Duration::from_millis(self.timeout_ms),
            anchors,
        )
        .map_err(|e| format!("{e:?}"))?;
        if !response.is_success() {
            return Err(format!("tool server HTTP {}", response.status));
        }
        let parsed: Value =
            serde_json::from_str(&response.body).map_err(|_| "tool list is not JSON")?;
        let tools = parsed["tools"]
            .as_array()
            .ok_or("tool list has no tools[]")?
            .iter()
            .filter(|t| {
                let name = t["function"]["name"].as_str().unwrap_or("");
                !name.is_empty() && is_allowed(&self.allowed, name)
            })
            .cloned()
            .collect();
        Ok(tools)
    }

    /// Execute one call. Never fails: failures become an error result the
    /// model can read.
    pub fn call(
        &self,
        anchors: &TrustAnchors,
        call_id: Option<String>,
        name: &str,
        raw_arguments: &Value,
    ) -> ToolCallRecord {
        let started = Instant::now();
        let arguments = match raw_arguments {
            Value::String(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
            }
            other => other.clone(),
        };
        let allowed = is_allowed(&self.allowed, name);
        let outcome: Result<Value, String> = if allowed {
            (|| {
                let endpoint = Endpoint::parse(&self.base_url, "/v1/tools/call")?;
                let body = json!({"name": name, "arguments": arguments});
                let response = post_json(
                    &endpoint,
                    &body.to_string(),
                    &self.headers()?,
                    Duration::from_millis(self.timeout_ms),
                    anchors,
                )
                .map_err(|e| format!("{e:?}"))?;
                serde_json::from_str::<Value>(&response.body)
                    .map_err(|_| format!("tool server HTTP {} (non-JSON)", response.status))
            })()
        } else {
            Err(format!("tool {name} is not allowed"))
        };
        let (ok, result) = match outcome {
            Ok(reply) if reply["ok"].as_bool() == Some(true) => (true, reply["result"].clone()),
            Ok(reply) => (
                false,
                json!({"error": reply.get("error").cloned().unwrap_or(reply)}),
            ),
            Err(e) => (false, json!({"error": e})),
        };
        ToolCallRecord {
            call_id,
            name: name.to_owned(),
            arguments,
            ok,
            result: truncate(result, MAX_TOOL_RESULT_CHARS),
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// Bound what one tool result may add to the model context.
pub const MAX_TOOL_RESULT_CHARS: usize = 12_000;

fn truncate(value: Value, max: usize) -> Value {
    let text = value.to_string();
    if text.chars().count() <= max {
        return value;
    }
    json!({"truncated": true, "partial_json": text.chars().take(max).collect::<String>()})
}

/// Parse `tool_calls` out of an assistant message: (id, name, arguments).
pub fn requested_calls(message: &Value) -> Vec<(Option<String>, String, Value)> {
    message["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .filter_map(|c| {
                    let name = c["function"]["name"].as_str()?.to_owned();
                    Some((
                        c["id"].as_str().map(str::to_owned),
                        name,
                        c["function"]["arguments"].clone(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `tool` message that carries a record back to the model.
pub fn tool_message(record: &ToolCallRecord) -> Value {
    let content = json!({"ok": record.ok, "result": record.result}).to_string();
    let mut message = json!({"role": "tool", "name": record.name, "content": content});
    if let Some(id) = &record.call_id {
        message["tool_call_id"] = Value::String(id.clone());
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_tool_calls() {
        let message = json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function",
             "function": {"name": "json_get", "arguments": "{\"library\":\"x\"}"}}]});
        let calls = requested_calls(&message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_deref(), Some("c1"));
        assert_eq!(calls[0].1, "json_get");
    }

    #[test]
    fn disallowed_tool_is_an_error_result_not_a_request() {
        let mut config = ToolServerConfig::new("http://127.0.0.1:9");
        config.allowed = vec!["library_list".to_owned()];
        let record = config.call(&TrustAnchors::Webpki, None, "json_get", &json!("{}"));
        assert!(!record.ok);
        assert!(
            record.result["error"]
                .as_str()
                .unwrap_or("")
                .contains("not allowed")
        );
    }

    #[test]
    fn allow_list_supports_prefixes() {
        let allowed = vec!["json_get".to_owned(), "mcp__context7__*".to_owned()];
        assert!(is_allowed(&allowed, "json_get"));
        assert!(is_allowed(&allowed, "mcp__context7__query-docs"));
        assert!(!is_allowed(&allowed, "mcp__github__create_issue"));
        assert!(is_allowed(&[], "anything"));
    }

    #[test]
    fn offered_definitions_use_the_same_prefix_allowlist_as_calls() {
        use kamimusuhi_testkit::{FixtureResponse, FixtureServer};
        let body = json!({"tools": [
            {"type": "function", "function": {"name": "json_get"}},
            {"type": "function", "function": {"name": "mcp__chrome_web__google_search"}},
            {"type": "function", "function": {"name": "mcp__chrome_web__fetch_url"}},
            {"type": "function", "function": {"name": "mcp__other__write"}},
            {"type": "function", "function": {"name": ""}}
        ]})
        .to_string();
        let server = FixtureServer::always(FixtureResponse::RawHttp {
            response: format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
        }).expect("fixture");
        let mut config = ToolServerConfig::new(server.base_url().trim_end_matches("/v1"));
        config.allowed = vec!["json_get".into(), "mcp__chrome_web__*".into()];
        let definitions = config
            .definitions(&TrustAnchors::Webpki)
            .expect("definitions");
        let names: Vec<_> = definitions
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "json_get",
                "mcp__chrome_web__google_search",
                "mcp__chrome_web__fetch_url"
            ]
        );
        assert_eq!(server.requests()[0].path, "/v1/tools");
    }

    #[test]
    fn large_results_are_truncated() {
        let big = json!({"x": "a".repeat(50_000)});
        let t = truncate(big, 100);
        assert_eq!(t["truncated"], true);
    }
}
