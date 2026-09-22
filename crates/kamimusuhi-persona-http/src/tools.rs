//! Tool calling against an MCP-like tool server (the resident's
//! `/v1/tools` surface).
//!
//! The model may request tools; this module executes them and returns the
//! results as `tool` messages. What a tool returns is external material: it
//! is recorded in [`ToolCallRecord`]s for the host to audit and is never the
//! expression itself, never evidence of the individual's experience, and
//! never mutation authority. Every tool on the server is read-only.

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
    /// Which allowed tools are offered from the first round (the rest are
    /// discoverable through [`TOOL_CATALOG`] / [`TOOL_ENABLE`]). `None`
    /// offers the server's built-in tools upfront and makes MCP tools
    /// (`mcp__*`) discoverable; patterns follow the `allowed` syntax, and
    /// `["*"]` offers everything upfront.
    pub core: Option<Vec<String>>,
}

/// Whether `name` is admitted by `allowed` (empty admits everything).
pub fn is_allowed(allowed: &[String], name: &str) -> bool {
    allowed.is_empty()
        || allowed.iter().any(|a| match a.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => a == name,
        })
}

/// Synthetic tool: list the discoverable tools (name + short description).
pub const TOOL_CATALOG: &str = "tool_catalog";
/// Synthetic tool: add discoverable tools' full definitions to this turn.
pub const TOOL_ENABLE: &str = "tool_enable";

/// Longest tool description sent to the model, in characters.
pub const TOOL_DESCRIPTION_CHARS: usize = 200;
/// Longest parameter description sent to the model, in characters.
pub const PARAMETER_DESCRIPTION_CHARS: usize = 120;
/// Longest description in a catalog entry.
const CATALOG_DESCRIPTION_CHARS: usize = 120;

/// Schema keys that carry no meaning for calling the tool.
const DROPPED_SCHEMA_KEYS: [&str; 6] = [
    "title",
    "examples",
    "additionalProperties",
    "$schema",
    "$id",
    "deprecated",
];

/// Maps whose keys are property names, not schema keywords.
const SCHEMA_NAME_MAPS: [&str; 5] = [
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn compact_schema(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            for key in DROPPED_SCHEMA_KEYS {
                map.remove(key);
            }
            for (key, value) in map.iter_mut() {
                if key == "description" {
                    if let Value::String(text) = value {
                        *text = clip(text, PARAMETER_DESCRIPTION_CHARS);
                    }
                } else if SCHEMA_NAME_MAPS.contains(&key.as_str()) {
                    if let Value::Object(named) = value {
                        for (_, inner) in named.iter_mut() {
                            compact_schema(inner);
                        }
                    }
                } else if key != "enum" && key != "required" && key != "type" {
                    compact_schema(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                compact_schema(item);
            }
        }
        _ => {}
    }
}

/// A definition with descriptions clipped and schema decoration removed.
/// Names, types, `required`, `enum` and `properties` are untouched.
pub fn compact_definition(definition: &Value) -> Value {
    let mut compact = definition.clone();
    if let Some(function) = compact.get_mut("function").and_then(Value::as_object_mut) {
        if let Some(Value::String(text)) = function.get_mut("description") {
            *text = clip(text, TOOL_DESCRIPTION_CHARS);
        }
        if let Some(parameters) = function.get_mut("parameters") {
            compact_schema(parameters);
        }
    }
    compact
}

fn definition_name(definition: &Value) -> &str {
    definition["function"]["name"].as_str().unwrap_or("")
}

/// What one turn may offer: the upfront `core` set and the `discoverable`
/// rest, both compacted, deduplicated and sorted by name so an unchanged
/// server yields a byte-identical prompt prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOffer {
    pub core: Vec<Value>,
    pub discoverable: Vec<Value>,
}

impl ToolOffer {
    pub fn is_empty(&self) -> bool {
        self.core.is_empty() && self.discoverable.is_empty()
    }

    /// The synthetic discovery tools, present only when there is something
    /// to discover.
    pub fn discovery_definitions(&self) -> Vec<Value> {
        if self.discoverable.is_empty() {
            return Vec::new();
        }
        vec![
            json!({"type": "function", "function": {
                "name": TOOL_CATALOG,
                "description": format!(
                    "ここに出ていない外部ツール（{}件）の名前と説明を一覧する。queryで絞り込める。",
                    self.discoverable.len()),
                "parameters": {"type": "object", "properties": {
                    "query": {"type": "string", "description": "名前・説明に含まれる語（省略可）"}}}}}),
            json!({"type": "function", "function": {
                "name": TOOL_ENABLE,
                "description": "tool_catalogで見つけた外部ツールを有効化し、次の応答から呼べるようにする。",
                "parameters": {"type": "object", "required": ["names"], "properties": {
                    "names": {"type": "array", "items": {"type": "string"},
                              "description": "有効化するツール名"}}}}}),
        ]
    }

    /// Tools offered from the first round.
    pub fn initial_definitions(&self) -> Vec<Value> {
        let mut offered = self.core.clone();
        offered.extend(self.discovery_definitions());
        offered
    }

    /// Catalog entries, optionally filtered by a case-insensitive substring.
    pub fn catalog(&self, query: Option<&str>) -> Value {
        let query = query
            .map(str::to_lowercase)
            .filter(|q| !q.trim().is_empty());
        let entries: Vec<Value> = self
            .discoverable
            .iter()
            .filter(|definition| {
                query.as_ref().is_none_or(|q| {
                    definition_name(definition).to_lowercase().contains(q)
                        || definition["function"]["description"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(q)
                })
            })
            .map(|definition| {
                json!({
                    "name": definition_name(definition),
                    "description": clip(
                        definition["function"]["description"].as_str().unwrap_or(""),
                        CATALOG_DESCRIPTION_CHARS),
                })
            })
            .collect();
        json!({"tools": entries, "hint": format!("使うツールは {TOOL_ENABLE} で有効化してから呼ぶ")})
    }

    /// Definitions for `names`, and the names that are not discoverable.
    pub fn enable(&self, names: &[String]) -> (Vec<Value>, Vec<String>) {
        let mut found = Vec::new();
        let mut unknown = Vec::new();
        for name in names {
            match self
                .discoverable
                .iter()
                .find(|definition| definition_name(definition) == name)
            {
                Some(definition) => found.push(definition.clone()),
                None => unknown.push(name.clone()),
            }
        }
        (found, unknown)
    }
}

/// The record for a `tool_catalog` call. Handled by the host, never sent to
/// the tool server.
pub fn catalog_record(
    offer: &ToolOffer,
    call_id: Option<String>,
    arguments: &Value,
) -> ToolCallRecord {
    let started = Instant::now();
    let arguments = parse_arguments(arguments);
    let result = offer.catalog(arguments["query"].as_str());
    ToolCallRecord {
        call_id,
        name: TOOL_CATALOG.to_owned(),
        arguments,
        ok: true,
        result,
        latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The record for a `tool_enable` call plus the definitions to add to the
/// offered set.
pub fn enable_record(
    offer: &ToolOffer,
    call_id: Option<String>,
    arguments: &Value,
) -> (ToolCallRecord, Vec<Value>) {
    let started = Instant::now();
    let arguments = parse_arguments(arguments);
    let names: Vec<String> = arguments["names"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let (definitions, unknown) = offer.enable(&names);
    let enabled: Vec<&str> = definitions.iter().map(definition_name).collect();
    let ok = !enabled.is_empty();
    let result = json!({"enabled": enabled, "unknown": unknown});
    (
        ToolCallRecord {
            call_id,
            name: TOOL_ENABLE.to_owned(),
            arguments,
            ok,
            result,
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
        definitions,
    )
}

/// Arguments as the model supplied them: a JSON string is decoded when it
/// parses, anything else is kept as is.
fn parse_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(text) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        other => other.clone(),
    }
}

impl ToolServerConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_env: None,
            max_rounds: 4,
            timeout_ms: 30_000,
            allowed: Vec::new(),
            core: None,
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

    /// Whether `name` is offered from the first round.
    pub fn is_core(&self, name: &str) -> bool {
        match &self.core {
            Some(patterns) => is_allowed(patterns, name) && !patterns.is_empty(),
            None => !name.starts_with("mcp__"),
        }
    }

    /// Tool definitions to offer, filtered by `allowed` (same prefix rules
    /// as [`is_allowed`]), compacted and split into core and discoverable.
    /// `Err` means tools are unavailable this turn; the caller answers
    /// without them.
    pub fn definitions(&self, anchors: &TrustAnchors) -> Result<ToolOffer, String> {
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
        let listed = parsed["tools"]
            .as_array()
            .ok_or("tool list has no tools[]")?;
        Ok(self.offer(listed))
    }

    /// Split, filter, compact, deduplicate and sort a listed tool set.
    pub fn offer(&self, listed: &[Value]) -> ToolOffer {
        let mut seen = std::collections::BTreeSet::new();
        let mut offer = ToolOffer::default();
        for definition in listed {
            let name = definition_name(definition);
            if name.is_empty()
                || name == TOOL_CATALOG
                || name == TOOL_ENABLE
                || !is_allowed(&self.allowed, name)
                || !seen.insert(name.to_owned())
            {
                continue;
            }
            let compact = compact_definition(definition);
            if self.is_core(name) {
                offer.core.push(compact);
            } else {
                offer.discoverable.push(compact);
            }
        }
        offer
            .core
            .sort_by(|a, b| definition_name(a).cmp(definition_name(b)));
        offer
            .discoverable
            .sort_by(|a, b| definition_name(a).cmp(definition_name(b)));
        offer
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
        let arguments = parse_arguments(raw_arguments);
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
    fn large_results_are_truncated() {
        let big = json!({"x": "a".repeat(50_000)});
        let t = truncate(big, 100);
        assert_eq!(t["truncated"], true);
    }
}
