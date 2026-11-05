//! MCP-like tool surface over HTTP.
//!
//! * `GET  /v1/tools` — tool definitions in OpenAI function-calling form,
//!   plus the library catalog they operate on.
//! * `POST /v1/tools/call` — `{"name": "...", "arguments": {...}}`.
//!
//! Built-in tools are read-only, except `http_request`, which is *bounded*:
//! it can only reach the hosts in `tools.http_allow_hosts` with the methods
//! in `tools.http_allow_methods`, and both lists fail closed (an empty host
//! list, the default, disables the tool). That allowlist — not the model —
//! is the permission boundary, so no approval or judge step sits in front
//! of it; every call is journaled under `logs/http_request` and recorded
//! on the task board like every other tool. MCP tools are what their
//! servers offer, and any tool listed in `approval_required` is queued for
//! operator approval instead of executed (see `approvals.rs`). A tool
//! error is returned as a result (`ok: false`) so a model can see it and
//! recover; only an unknown tool name or a malformed request is an HTTP
//! error.
//!
//! Secrecy rules for `http_request`: a header value written as
//! `"$ENV:NAME"` is resolved from the process environment at call time.
//! The journal records the request's header *names* and sizes only —
//! never a header value, never a body — so neither `$ENV` references nor
//! literal secrets in arguments are persisted.

use kamimusuhi_resource_http::{Endpoint, Header, HttpError, TrustAnchors, http};
use serde_json::{Value, json};

use crate::state::Shared;

const DEFAULT_SEARCH_HITS: u64 = 8;
const MAX_SEARCH_HITS: u64 = 20;

/// `http_request` bounds: a call hangs at most this long, and each side's
/// body is clipped to this many bytes.
const HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_HTTP_BODY: usize = 32 * 1024;
/// `http_request` argument caps beyond the body: bounded, like the rest.
const MAX_HTTP_HEADERS: usize = 32;

/// The response headers worth showing to the model. A fixed list is the
/// answer to "headers-lite": an upstream service cannot put a
/// credential-shaped header into the result.
const LITE_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "retry-after",
    "location",
    "etag",
    "last-modified",
    "date",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
    "x-ratelimit-used",
    "x-request-id",
    "x-github-request-id",
];

fn function(name: &str, description: &str, parameters: Value) -> Value {
    json!({"type": "function", "function": {
        "name": name, "description": description, "parameters": parameters}})
}

pub fn definitions() -> Vec<Value> {
    let lib = json!({"type": "string", "description": "library name from library_list"});
    vec![
        function(
            "task_list",
            "List the task board: what you, the operator and the system are working on in parallel, with status and predecessor links (depends_on).",
            json!({"type": "object", "properties": {"all": {"type": "boolean"}}}),
        ),
        function(
            "task_create",
            "Add a task you plan to work on (shown to the operator on the task board). Use depends_on to link it after existing task ids. For a goal you want the engine to keep pursuing on its own — retries, replanning, verification, across restarts — use commit_create instead.",
            json!({"type": "object", "properties": {
                "title": {"type": "string"},
                "status": {"type": "string", "enum": ["in_progress", "waiting", "on_hold"]},
                "depends_on": {"type": "array", "items": {"type": "string"}},
                "note": {"type": "string"}},
                "required": ["title"]}),
        ),
        function(
            "task_update",
            "Update the status of, or add a note to, a task you or the operator created.",
            json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "status": {"type": "string", "enum": ["in_progress", "waiting", "on_hold", "done", "failed"]},
                "note": {"type": "string"}},
                "required": ["id"]}),
        ),
        function(
            "library_list",
            "List the read-only reference libraries (datasets, repositories) Kamimusuhi can consult.",
            json!({"type": "object", "properties": {}}),
        ),
        function(
            "library_tree",
            "List one directory inside a library.",
            json!({"type": "object", "properties": {
                "library": lib, "path": {"type": "string", "description": "directory, '' for root"}},
                "required": ["library"]}),
        ),
        function(
            "library_read",
            "Read a slice of a text file in a library (max 2MiB per call; use offset to continue).",
            json!({"type": "object", "properties": {
                "library": lib, "path": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 65536}},
                "required": ["library", "path"]}),
        ),
        function(
            "library_search",
            "Substring search over text files in a library. Returns path, line and a snippet per hit (default 8, max 20). Refine q or path when truncated is true.",
            json!({"type": "object", "properties": {
                "library": lib, "q": {"type": "string", "minLength": 2},
                "path": {"type": "string", "description": "limit to this directory"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SEARCH_HITS,
                          "default": DEFAULT_SEARCH_HITS}},
                "required": ["library", "q"]}),
        ),
        function(
            "json_get",
            "Get the value at a JSON pointer (RFC 6901, e.g. /companies/7203) inside a JSON file of a library. Large values are summarized.",
            json!({"type": "object", "properties": {
                "library": lib, "path": {"type": "string"},
                "pointer": {"type": "string"}, "max_bytes": {"type": "integer"}},
                "required": ["library", "path", "pointer"]}),
        ),
        function(
            "json_find",
            "Find elements of a JSON array/object (at `pointer`) whose fields match. field is a JSON pointer inside each element, or _key for object keys. op is eq or contains.",
            json!({"type": "object", "properties": {
                "library": lib, "path": {"type": "string"}, "pointer": {"type": "string"},
                "match": {"type": "array", "items": {"type": "object", "properties": {
                    "field": {"type": "string"}, "op": {"type": "string", "enum": ["eq", "contains"]},
                    "value": {"type": "string"}}, "required": ["field", "value"]}},
                "mode": {"type": "string", "enum": ["any", "all"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}},
                "required": ["library", "path", "pointer", "match"]}),
        ),
        function(
            "http_request",
            "Bounded HTTP request to a host on this node's allowlist (tools.http_allow_hosts — empty means every request is refused). Methods limited to tools.http_allow_methods (default GET/POST). The body (optional, ≤32KiB) is sent as application/json; the response body is clipped to 32KiB. Query strings, URL fragments and userinfo are not supported. A header value \"$ENV:NAME\" is filled from environment variable NAME at call time.",
            json!({"type": "object", "properties": {
                "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"],
                           "default": "GET"},
                "url": {"type": "string"},
                "headers": {"type": "object",
                            "additionalProperties": {"type": "string"}},
                "body": {"type": "string"}},
                "required": ["method", "url"]}),
        ),
        function(
            "self_note",
            "Write down a self-authored working note about yourself (at most a few per turn). kind is what the note claims; text is the note itself (≤1KiB); evidence_refs optionally names the ids it draws on. Notes are scratch state: they come back marked SELF_NOTES — self-authored and unverified — never canonical memory or evidence.",
            json!({"type": "object", "properties": {
                "kind": {"type": "string",
                         "enum": ["tendency", "lesson", "preference", "observation"]},
                "text": {"type": "string", "maxLength": 1024},
                "evidence_refs": {"type": "array", "maxItems": 8,
                                  "items": {"type": "string", "maxLength": 64}}},
                "required": ["kind", "text"]}),
        ),
        function(
            "improve",
            "Propose an operative-parameter change for operator review — this call never applies anything. param is a closed-vocabulary key (retrieval.<name>, conversation.max_response_chars, reflection.interval_turns; dialogue.<name> is accepted as an alias of conversation.<name>); value is the proposed numeric value; reason says why the change should help. The proposal lands pending on the derived lane and the operator's gate decides it.",
            json!({"type": "object", "properties": {
                "param": {"type": "string"},
                "value": {"type": "number"},
                "reason": {"type": "string", "maxLength": 600}},
                "required": ["param", "value"]}),
        ),
    ]
}

fn action_for(name: &str) -> Option<&'static str> {
    Some(match name {
        "library_list" => "list",
        "library_tree" => "tree",
        "library_read" => "file",
        "library_search" => "search",
        "json_get" => "json_get",
        "json_find" => "json_find",
        _ => return None,
    })
}

/// Listing a peer's tools is turn metadata, not the work itself; a peer
/// that accepts a connection but never answers must not hold it open.
const PEER_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// A forwarded call is real work (browser driving, builds): give it room.
const PEER_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub(crate) fn post_peer(
    url: &str,
    suffix: &str,
    body: &Value,
    token: Option<&str>,
    timeout: std::time::Duration,
) -> Option<(u16, Value)> {
    let endpoint = Endpoint::parse(url, suffix).ok()?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &crate::client::auth(token),
        timeout,
        &TrustAnchors::Webpki,
    )
    .ok()?;
    let value = serde_json::from_str(&response.body).ok()?;
    Some((response.status, value))
}

/// This node's MCP tool definitions.
fn local_mcp(shared: &Shared) -> Vec<Value> {
    shared
        .mcp
        .iter()
        .flat_map(|server| server.tools().into_iter().map(|t| t.definition))
        .collect()
}

/// Built-in tools, this node's MCP tools and (unless `local_only`) the MCP
/// tools of peers, which are called through them.
pub fn list_with(shared: &Shared, local_only: bool) -> Value {
    let mut tools = definitions();
    if shared.task_plane.is_some() {
        tools.extend(crate::task_orchestrator::definitions());
    }
    if shared.config.commitments.enabled {
        tools.extend(crate::commitments::definitions());
    }
    if shared.config.intercom.enabled {
        tools.extend(crate::intercom::definitions());
    }
    tools.extend(local_mcp(shared));
    let mut seen: std::collections::HashSet<String> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
        .collect();
    let mut mcp_servers: Vec<Value> = shared
        .mcp
        .iter()
        .map(|m| {
            let mut s = m.status();
            s["node"] = json!(shared.config.node.id);
            s["description"] = json!(m.config.description);
            s
        })
        .collect();
    if !local_only {
        for peer in &shared.config.peers {
            // A peer that fails health probes is unreachable or wedged;
            // contacting it would only stall the listing past callers'
            // timeouts.
            if !shared.peer_healthy(&peer.id) {
                continue;
            }
            if let Some((200, v)) = post_peer(
                &shared.peer_url(peer),
                "/v1/tools/list",
                &json!({"local_only": true}),
                shared.token.as_deref(),
                PEER_LIST_TIMEOUT,
            ) {
                for tool in v["tools"].as_array().into_iter().flatten() {
                    let name = tool["function"]["name"].as_str().unwrap_or("").to_owned();
                    // A peer's task plane is used through that peer.
                    let remote = name.starts_with("mcp__")
                        || crate::task_orchestrator::TOOL_NAMES.contains(&name.as_str());
                    if remote && seen.insert(name) {
                        tools.push(tool.clone());
                    }
                }
                mcp_servers.extend(v["mcp_servers"].as_array().into_iter().flatten().cloned());
            }
        }
    }
    let (_, catalog) =
        crate::library::handle(shared, &json!({"action": "list", "local_only": local_only}));
    json!({"tools": tools, "libraries": catalog["libraries"], "mcp_servers": mcp_servers})
}

pub fn list(shared: &Shared) -> Value {
    list_with(shared, false)
}

/// Returns (HTTP status, body).
/// Where a tool runs: this node, or the peer that offers it (an MCP tool
/// or a library held elsewhere).
fn executing_node(shared: &Shared, name: &str, arguments: &Value) -> String {
    let peer = || {
        shared
            .config
            .peers
            .iter()
            .find(|p| shared.peer_healthy(&p.id))
            .map_or_else(|| shared.config.node.id.clone(), |p| p.id.clone())
    };
    if action_for(name).is_some() {
        let args = match arguments {
            Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
            other => other.clone(),
        };
        return match args["library"].as_str() {
            Some(lib) if !shared.config.libraries.iter().any(|l| l.name == lib) => peer(),
            _ => shared.config.node.id.clone(),
        };
    }
    if !name.starts_with("mcp__")
        || shared
            .mcp
            .iter()
            .any(|m| m.tools().iter().any(|t| t.exposed == name))
    {
        return shared.config.node.id.clone();
    }
    shared
        .config
        .peers
        .iter()
        .find(|p| shared.peer_healthy(&p.id))
        .map_or_else(|| shared.config.node.id.clone(), |p| p.id.clone())
}

/// One-line description of a call for the task board.
fn call_title(name: &str, arguments: &Value) -> String {
    let args = match arguments {
        Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        other => other.clone(),
    };
    let target = [
        "path",
        "pointer",
        "q",
        "url",
        "query",
        "libraryName",
        "owner",
        "repo",
        "file",
    ]
    .iter()
    .find_map(|k| args.get(*k).and_then(Value::as_str))
    .unwrap_or("");
    let short = name.trim_start_matches("mcp__").replace("__", "/");
    if target.is_empty() {
        short
    } else {
        format!("{short} {target}")
    }
}

/// Task-board tools for the individual: plan and track its own work.
fn task_tool(shared: &Shared, name: &str, request: &Value, turn: Option<&str>) -> (u16, Value) {
    let mut args = match &request["arguments"] {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other.clone(),
    };
    // A task the individual plans mid-conversation follows that turn.
    if name == "task_create"
        && args["depends_on"].as_array().is_none_or(Vec::is_empty)
        && let Some(turn) = turn
    {
        args["depends_on"] = json!([turn]);
    }
    args["action"] = json!(match name {
        "task_create" => "create",
        "task_update" => "update",
        _ => "list",
    });
    let (status, reply) = crate::tasks::handle(shared, &args, "mio");
    if status == 200 {
        (200, json!({"ok": true, "result": reply}))
    } else {
        (
            200,
            json!({"ok": false, "error": reply.get("error").cloned().unwrap_or(reply)}),
        )
    }
}

/// Task-plane tools: served here when this node has executors, otherwise
/// by the first peer that does. The delegated task is recorded on the
/// board of the node that runs it.
fn task_plane_tool(
    shared: &Shared,
    name: &str,
    request: &Value,
    turn: Option<&str>,
) -> (u16, Value) {
    if shared.task_plane.is_some() {
        let args = match &request["arguments"] {
            Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
            Value::Null => json!({}),
            other => other.clone(),
        };
        return crate::task_orchestrator::tool(shared, name, &args, turn);
    }
    if !request["local_only"].as_bool().unwrap_or(false) {
        let mut forwarded = request.clone();
        forwarded["local_only"] = Value::Bool(true);
        for peer in &shared.config.peers {
            if !shared.peer_healthy(&peer.id) {
                continue;
            }
            if let Some((200, v)) = post_peer(
                &shared.peer_url(peer),
                "/v1/tools/call",
                &forwarded,
                shared.token.as_deref(),
                PEER_CALL_TIMEOUT,
            ) && v["error"] != json!("no task plane")
            {
                return (200, v);
            }
        }
    }
    (200, json!({"ok": false, "error": "no task plane"}))
}

/// Execute a tool call and record it on the task board as a successor of
/// `turn`, the dialogue turn that made it (see [`Shared::turn_for`]).
/// Forwarded (`local_only`) calls are recorded by the node that forwarded
/// them, not twice.
pub fn call(shared: &Shared, request: &Value, turn: Option<&str>) -> (u16, Value) {
    let name = request["name"].as_str().unwrap_or("");
    if crate::task_orchestrator::TOOL_NAMES.contains(&name) {
        return task_plane_tool(shared, name, request, turn);
    }
    // Commitments live in this node's durable store; they are never
    // forwarded to a peer.
    if crate::commitments::TOOL_NAMES.contains(&name) {
        return crate::commitments::tool(shared, name, request);
    }
    // Intercom sends mail between the individuals on peer nodes; it is
    // local bookkeeping on this node, not work to forward.
    if crate::intercom::TOOL_NAMES.contains(&name) {
        return crate::intercom::tool(shared, name, request);
    }
    if name.starts_with("task_") {
        return task_tool(shared, name, request, turn);
    }
    if request["local_only"].as_bool().unwrap_or(false) {
        return call_inner(shared, request, turn);
    }
    let turn = turn.map(str::to_owned);
    let node = executing_node(shared, name, &request["arguments"]);
    let task = shared.tasks.create(
        &shared.spool,
        crate::tasks::NewTask {
            title: &call_title(name, &request["arguments"]),
            kind: "tool",
            node: &node,
            owner: "mio",
            status: crate::tasks::TaskStatus::InProgress,
            depends_on: turn.clone().into_iter().collect(),
            detail: json!({"tool": name}),
        },
    );
    let started = std::time::Instant::now();
    let (status, reply) = call_inner(shared, request, turn.as_deref());
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if reply["pending_approval"].as_bool().unwrap_or(false) {
        shared.tasks.update(
            &shared.spool,
            &task,
            Some(crate::tasks::TaskStatus::Done),
            Some(("system", "承認待ちとして登録")),
            None,
        );
        let approval_id = reply["approval_id"].as_str().unwrap_or("").to_owned();
        let approval_task = shared.tasks.create(
            &shared.spool,
            crate::tasks::NewTask {
                title: &format!("承認待ち: {}", call_title(name, &request["arguments"])),
                kind: "approval",
                node: &shared.config.node.id,
                owner: "operator",
                status: crate::tasks::TaskStatus::AwaitingOperator,
                depends_on: vec![task],
                detail: json!({"approval": approval_id, "tool": name}),
            },
        );
        shared.approvals.set_task(&approval_id, &approval_task);
    } else {
        let ok = reply["ok"].as_bool().unwrap_or(false);
        let note = if ok {
            format!("{ms}ms")
        } else {
            format!(
                "{ms}ms error: {}",
                reply["error"]
                    .to_string()
                    .chars()
                    .take(200)
                    .collect::<String>()
            )
        };
        shared.tasks.update(
            &shared.spool,
            &task,
            Some(if ok {
                crate::tasks::TaskStatus::Done
            } else {
                crate::tasks::TaskStatus::Failed
            }),
            Some(("system", &note)),
            None,
        );
    }
    (status, reply)
}

fn call_inner(shared: &Shared, request: &Value, turn: Option<&str>) -> (u16, Value) {
    let name = request["name"].as_str().unwrap_or("");
    if name.starts_with("mcp__") {
        return call_mcp(shared, name, request);
    }
    if name == "http_request" {
        // OpenAI tool calls carry arguments as a JSON string; accept both.
        let arguments = match &request["arguments"] {
            Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
            other => other.clone(),
        };
        return http_request_tool(shared, &arguments, HTTP_REQUEST_TIMEOUT);
    }
    if name == "self_note" || name == "improve" {
        // OpenAI tool calls carry arguments as a JSON string; accept both.
        let arguments = match &request["arguments"] {
            Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
            other => other.clone(),
        };
        return if name == "self_note" {
            self_note_tool(shared, &arguments, turn)
        } else {
            improve_tool(shared, &arguments, turn)
        };
    }
    let Some(action) = action_for(name) else {
        return (
            400,
            json!({"ok": false, "error": format!("unknown tool {name:?}")}),
        );
    };
    // OpenAI tool calls carry arguments as a JSON string; accept both.
    let arguments = match &request["arguments"] {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(v) => v,
            Err(_) => {
                return (
                    200,
                    json!({"ok": false, "error": "arguments are not valid JSON"}),
                );
            }
        },
        Value::Null => json!({}),
        other => other.clone(),
    };
    let Value::Object(mut body) = arguments else {
        return (
            200,
            json!({"ok": false, "error": "arguments must be an object"}),
        );
    };
    body.remove("local_only");
    body.insert("action".into(), Value::String(action.to_owned()));
    if action == "file" {
        // Keep tool results small enough for a model context.
        let limit = body
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(16_384)
            .min(65_536);
        body.insert("limit".into(), json!(limit));
    } else if action == "search" {
        // Apply the dialogue budget before local execution or peer forwarding.
        // Missing/non-integer limits use the default; zero still returns one hit.
        let limit = body
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_SEARCH_HITS)
            .clamp(1, MAX_SEARCH_HITS);
        body.insert("limit".into(), json!(limit));
    }
    let (status, result) = crate::library::handle(shared, &Value::Object(body));
    if status == 200 {
        (200, json!({"ok": true, "result": result}))
    } else {
        (
            200,
            json!({"ok": false, "error": result.get("error").cloned().unwrap_or(result)}),
        )
    }
}

/// `url` split into the `scheme://authority` + `path` shape
/// [`Endpoint::parse`] wants. The endpoint parser then refuses queries,
/// fragments, credentials and control characters — that refusal is the
/// bound, not a convenience.
fn http_endpoint(url: &str) -> Result<Endpoint, String> {
    let (scheme, rest) = url
        .split_once("://")
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| "url must start with http:// or https://".to_owned())?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    Endpoint::parse(&format!("{scheme}://{authority}"), path)
}

/// `host` matches an allowlist entry exactly or as a subdomain of it
/// (`github.com` admits `api.github.com`, not `evilgithub.com`).
fn http_host_allowed(allow: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    allow.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        host == entry
            || host
                .strip_suffix(&entry)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// A `"$ENV:NAME"` header value is read from the process environment at
/// call time; anything else is literal. Errors name the variable, never
/// a value.
fn resolve_header_value(value: &str) -> Result<String, String> {
    let Some(var) = value.strip_prefix("$ENV:") else {
        return Ok(value.to_owned());
    };
    if var.is_empty() || !var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("$ENV needs a variable name of [A-Za-z0-9_]".to_owned());
    }
    std::env::var(var).map_err(|_| format!("environment variable {var} is not set"))
}

fn http_error_kind(error: &HttpError) -> &'static str {
    match error {
        HttpError::InvalidRequest(_) => "invalid_request",
        HttpError::Timeout { .. } => "timeout",
        HttpError::Transport(_) => "transport",
        HttpError::Malformed(_) => "malformed",
        HttpError::Tls { .. } => "tls",
    }
}

fn http_error_detail(error: &HttpError) -> String {
    match error {
        HttpError::InvalidRequest(detail)
        | HttpError::Transport(detail)
        | HttpError::Malformed(detail)
        | HttpError::Tls { detail, .. } => detail.clone(),
        HttpError::Timeout { elapsed_ms, phase } => {
            format!("timed out after {elapsed_ms}ms in {phase}")
        }
    }
}

fn clip_utf8(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// `http_request`: one bounded call to an allowlisted host. Every policy
/// failure refuses *before* a socket opens; what the transport answers —
/// success or failure — becomes a structured result the model can read.
/// The journal records the request's header names and sizes, never a
/// value or a body, so secrets in arguments are not persisted.
fn http_request_tool(
    shared: &Shared,
    arguments: &Value,
    timeout: std::time::Duration,
) -> (u16, Value) {
    let fail = |kind: &str, detail: String| {
        (
            200,
            json!({"ok": false, "error": {"kind": kind, "detail": detail}}),
        )
    };
    let Value::Object(args) = arguments else {
        return fail("invalid", "arguments must be an object".to_owned());
    };
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let Some(url) = args.get("url").and_then(Value::as_str) else {
        return fail("invalid", "url is required".to_owned());
    };
    if !shared
        .config
        .tools
        .http_allow_methods
        .iter()
        .any(|m| m.eq_ignore_ascii_case(&method))
    {
        return fail(
            "denied",
            format!("method {method} is not in tools.http_allow_methods"),
        );
    }
    let endpoint = match http_endpoint(url) {
        Ok(endpoint) => endpoint,
        Err(detail) => return fail("invalid", detail),
    };
    if !http_host_allowed(&shared.config.tools.http_allow_hosts, &endpoint.host) {
        return fail(
            "denied",
            format!(
                "host {} is not in tools.http_allow_hosts (an empty list refuses all)",
                endpoint.host
            ),
        );
    }
    let body = args.get("body").and_then(Value::as_str).unwrap_or("");
    if body.len() > MAX_HTTP_BODY {
        return fail(
            "invalid",
            format!("body exceeds the {MAX_HTTP_BODY}-byte cap"),
        );
    }
    let mut headers = Vec::new();
    let mut header_names = Vec::new();
    if let Some(raw_headers) = args.get("headers") {
        let Some(map) = raw_headers.as_object() else {
            return fail(
                "invalid",
                "headers must be an object of name: value".to_owned(),
            );
        };
        if map.len() > MAX_HTTP_HEADERS {
            return fail("invalid", format!("over {MAX_HTTP_HEADERS} headers"));
        }
        for (name, value) in map {
            let Some(value) = value.as_str() else {
                return fail("invalid", format!("header {name} needs a string value"));
            };
            // Framing and connection fields are owned by the client; a
            // caller-supplied one would be a smuggling attempt at worst,
            // a silently doubled header at best. Content-Type is fixed to
            // application/json, so the body cannot be mislabeled.
            if name.eq_ignore_ascii_case("content-type") {
                if !value.trim().eq_ignore_ascii_case("application/json") {
                    return fail(
                        "invalid",
                        "content-type is fixed to application/json".to_owned(),
                    );
                }
                continue;
            }
            if ["host", "content-length", "transfer-encoding", "connection"]
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved))
            {
                return fail("invalid", format!("header {name} is managed by the client"));
            }
            let value = match resolve_header_value(value) {
                Ok(value) => value,
                Err(detail) => return fail("denied", detail),
            };
            header_names.push(name.clone());
            headers.push(Header {
                name: name.clone(),
                value,
            });
        }
    }
    let started = std::time::Instant::now();
    let outcome = http::request(
        &method,
        &endpoint,
        body,
        &headers,
        timeout,
        &TrustAnchors::Webpki,
    );
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let _ = shared.spool.append(
        "logs/http_request",
        json!({
            "method": method,
            "host": endpoint.host,
            "port": endpoint.port,
            "tls": endpoint.tls,
            "request_headers": header_names,
            "request_bytes": body.len(),
            "status": outcome.as_ref().ok().map(|r| r.status),
            "latency_ms": latency_ms,
            "error": outcome.as_ref().err().map(http_error_kind),
        }),
    );
    match outcome {
        Ok(response) => {
            let (body, truncated) = clip_utf8(&response.body, MAX_HTTP_BODY);
            let lite: serde_json::Map<String, Value> = response
                .headers
                .iter()
                .filter(|(name, _)| {
                    LITE_RESPONSE_HEADERS
                        .iter()
                        .any(|lite| name.eq_ignore_ascii_case(lite))
                })
                .take(16)
                .map(|(name, value)| (name.to_ascii_lowercase(), json!(value)))
                .collect();
            (
                200,
                json!({"ok": true, "result": {
                    "status": response.status,
                    "headers": lite,
                    "body": body,
                    "body_truncated": truncated,
                }}),
            )
        }
        Err(error) => fail(http_error_kind(&error), http_error_detail(&error)),
    }
}

/// Self-improvement budget: how many `self_note`/`improve` calls one
/// dialogue turn may make, counted on the task board (the journal of what
/// the turn already did). Calls unattributed to a turn — a direct
/// `POST /v1/tools/call` — are uncapped, like every other tool.
const SELF_TOOL_MAX_PER_TURN: usize = 4;

/// `improve` spawns `kamimusuhi-runtime propose` to queue the proposal; a
/// wedged store open must not pin the call.
const IMPROVE_SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `self_note`: append one bounded, self-authored note to
/// `<dialogue.dir>/self_notes.jsonl`. The file is scratch state the resident
/// owns — never canonical memory — and the workspace marks it as such.
fn self_note_tool(shared: &Shared, arguments: &Value, turn: Option<&str>) -> (u16, Value) {
    let fail = |detail: String| (200, json!({"ok": false, "error": detail}));
    let Some(config) = &shared.config.dialogue else {
        return fail("self_note is only served on the node hosting the individual".to_owned());
    };
    if !config.dir.join("kamimusuhi.sqlite").is_file() {
        return fail("individual is not initialized on this node".to_owned());
    }
    // The current call's board entry is already counted; > MAX means the
    // earlier calls exhausted the turn's budget.
    let used = turn.map_or(0, |t| shared.tasks.tool_calls_in(t, "self_note"));
    if used > SELF_TOOL_MAX_PER_TURN {
        return fail(format!(
            "self_note is limited to {SELF_TOOL_MAX_PER_TURN} calls per turn"
        ));
    }
    let Some(raw_kind) = arguments.get("kind").and_then(Value::as_str) else {
        return fail("kind is required (tendency|lesson|preference|observation)".to_owned());
    };
    let kind = match raw_kind.parse::<kamimusuhi_core::self_note::SelfNoteKind>() {
        Ok(kind) => kind,
        Err(error) => return fail(error.to_string()),
    };
    let text = arguments
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let evidence_refs: Vec<String> = match arguments.get("evidence_refs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(refs)) => {
            if let Some(bad) = refs.iter().position(|r| !r.is_string()) {
                return fail(format!("evidence_refs[{bad}] must be a string"));
            }
            refs.iter()
                .filter_map(|r| r.as_str().map(str::to_owned))
                .collect()
        }
        Some(_) => return fail("evidence_refs must be an array of strings".to_owned()),
    };
    let at = kamimusuhi_core::time::UtcTimestamp::from_unix_millis(
        i64::try_from(crate::util::unix_now_ms()).unwrap_or(i64::MAX),
    );
    match kamimusuhi_runtime::self_notes::append(&config.dir, kind, &text, evidence_refs, at) {
        Ok(note) => {
            let _ = shared.spool.append(
                "logs/self_note",
                json!({
                    "note_id": note.note_id(),
                    "kind": note.kind,
                    "text": note.text,
                    "evidence_refs": note.evidence_refs,
                }),
            );
            let kept = kamimusuhi_runtime::self_notes::load(&config.dir)
                .map(|notes| notes.len())
                .unwrap_or(0);
            (
                200,
                json!({"ok": true, "result": {
                    "note_id": note.note_id(),
                    "seq": note.seq,
                    "kind": note.kind,
                    "kept": kept,
                }}),
            )
        }
        Err(error) => fail(error.to_string()),
    }
}

/// `improve`: validate an operative-param change against the closed
/// vocabulary *before* proposing it — a bad target gets the bound error
/// back, not a stored rejection — then queue the proposal on the C0 lane
/// through `kamimusuhi-runtime propose`. The operator's gate applies it.
fn improve_tool(shared: &Shared, arguments: &Value, turn: Option<&str>) -> (u16, Value) {
    let fail = |detail: Value| (200, json!({"ok": false, "error": detail}));
    let Some(config) = &shared.config.dialogue else {
        return fail(json!(
            "improve is only served on the node hosting the individual"
        ));
    };
    if !config.dir.join("kamimusuhi.sqlite").is_file() {
        return fail(json!("individual is not initialized on this node"));
    }
    let used = turn.map_or(0, |t| shared.tasks.tool_calls_in(t, "improve"));
    if used > SELF_TOOL_MAX_PER_TURN {
        return fail(json!(format!(
            "improve is limited to {SELF_TOOL_MAX_PER_TURN} calls per turn"
        )));
    }
    let Some(raw_param) = arguments.get("param").and_then(Value::as_str) else {
        return fail(json!(
            "param is required — a closed-vocabulary operative key"
        ));
    };
    // `dialogue.*` is the model-facing alias of the `conversation.*`
    // namespace; the stored target is always the canonical key.
    let param = raw_param.strip_prefix("dialogue.").map_or_else(
        || raw_param.to_owned(),
        |rest| format!("conversation.{rest}"),
    );
    let value = arguments.get("value").cloned().unwrap_or(Value::Null);
    if let Err(error) = kamimusuhi_core::c0::OperativeParams::check(&param, &value) {
        return fail(json!({
            "kind": "closed_vocabulary",
            "detail": error.to_string(),
            "param": param,
            "known_params": kamimusuhi_core::c0::param_keys().collect::<Vec<_>>(),
        }));
    }
    let reason: String = arguments
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(600)
        .collect();
    let dir = config.dir.to_string_lossy().into_owned();
    let binary = config.runtime_binary.to_string_lossy().into_owned();
    let value_text = value.to_string();
    let args = vec![
        "propose",
        "--dir",
        dir.as_str(),
        "--param",
        param.as_str(),
        "--value",
        value_text.as_str(),
        "--reason",
        reason.as_str(),
    ];
    let Some(output) =
        crate::util::run_with_timeout_env(&binary, &args, &[], IMPROVE_SPAWN_TIMEOUT)
    else {
        return fail(json!("propose timed out or could not start"));
    };
    // `propose` prints the queued proposal as JSON; take the last JSON line
    // in case anything else ever prints first (the memory report does the
    // same for spawned output).
    let parsed = serde_json::from_str::<Value>(&output.stdout)
        .ok()
        .or_else(|| {
            output
                .stdout
                .lines()
                .rev()
                .find_map(|line| serde_json::from_str::<Value>(line).ok())
        });
    let _ = shared.spool.append(
        "logs/improve",
        json!({
            "param": param,
            "value": value,
            "reason": reason,
            "ok": output.success,
            "proposal": parsed.clone(),
        }),
    );
    match parsed {
        Some(report) if output.success => (
            200,
            json!({"ok": true, "result": {
                "proposal": report,
                "status": "pending",
                "message": "queued for operator review; nothing was applied",
            }}),
        ),
        _ => {
            let tail: Vec<&str> = output.stderr.lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            fail(json!({
                "kind": "propose_failed",
                "detail": tail.join("\n"),
            }))
        }
    }
}

/// Queue a gated call for the operator instead of running it now —
/// `approval_required` calls always, `judge_required` calls when the judge
/// model escalates or cannot answer (per `tool_judge.on_unavailable`).
fn enqueue_for_approval(
    shared: &Shared,
    server: &crate::mcp::McpServer,
    tool: &crate::mcp::McpTool,
    arguments: &Value,
    note: &str,
) -> (u16, Value) {
    match shared.approvals.enqueue(
        &server.config.name,
        &tool.original,
        &tool.exposed,
        arguments.clone(),
        None,
    ) {
        Ok(a) => {
            let _ = shared.spool.append_sync(
                "logs/approvals",
                json!({"event": "requested", "note": note, "approval": a.view()}),
            );
            (
                200,
                json!({"ok": false, "pending_approval": true, "approval_id": a.id,
                       "message": "操作者の承認待ちです。まだ実行されていません。承認IDを操作者に伝えてください。"}),
            )
        }
        Err(e) => (200, json!({"ok": false, "error": e})),
    }
}

fn call_mcp(shared: &Shared, name: &str, request: &Value) -> (u16, Value) {
    let arguments = match &request["arguments"] {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other.clone(),
    };
    for server in &shared.mcp {
        if let Some(tool) = server.tools().into_iter().find(|t| t.exposed == name) {
            if server.requires_approval(&tool.original) {
                return enqueue_for_approval(shared, server, &tool, &arguments, "operator gate");
            }
            if server.requires_judgment(&tool.original) {
                let description = tool.definition["function"]["description"]
                    .as_str()
                    .unwrap_or("");
                match crate::judge::check(
                    shared,
                    &server.config.name,
                    &tool.original,
                    description,
                    &arguments,
                ) {
                    crate::judge::Verdict::Allow => {}
                    crate::judge::Verdict::Deny(reason) => {
                        return (
                            200,
                            json!({"ok": false, "error":
                                   format!("判定モデルが拒否しました: {reason}")}),
                        );
                    }
                    crate::judge::Verdict::Escalate(reason) => {
                        return enqueue_for_approval(shared, server, &tool, &arguments, &reason);
                    }
                }
            }
            let started = std::time::Instant::now();
            let outcome = server
                .call(&tool.original, &arguments)
                .map(|raw| crate::mcp::normalize_for_server(&server.config.name, &raw));
            let _ = shared.spool.append(
                "logs/mcp",
                json!({"server": server.config.name, "tool": tool.original,
                       "ok": outcome.as_ref().is_ok_and(|(ok, _)| *ok),
                       "latency_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                       "arguments": arguments}),
            );
            return match outcome {
                Ok((ok, out)) => {
                    if ok {
                        (200, json!({"ok": true, "result": out}))
                    } else {
                        (200, json!({"ok": false, "error": out}))
                    }
                }
                Err(e) => (200, json!({"ok": false, "error": e})),
            };
        }
    }
    if !request["local_only"].as_bool().unwrap_or(false) {
        let mut forwarded = request.clone();
        forwarded["local_only"] = Value::Bool(true);
        for peer in &shared.config.peers {
            if !shared.peer_healthy(&peer.id) {
                continue;
            }
            if let Some((status, v)) = post_peer(
                &shared.peer_url(peer),
                "/v1/tools/call",
                &forwarded,
                shared.token.as_deref(),
                PEER_CALL_TIMEOUT,
            ) && status != 400
            {
                return (status, v);
            }
        }
    }
    (
        400,
        json!({"ok": false, "error": format!("unknown or unavailable tool {name:?}")}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_fixture() -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let library = dir.path().join("library");
        std::fs::create_dir(&library).expect("library directory");
        std::fs::write(library.join("data.txt"), "トヨタ \"7203\"\n".repeat(101)).expect("write");
        let config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "libraries": [{"name": "demo", "path": library}]
        }))
        .expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").expect("spool");
        (Shared::new(config, spool, 1, None), dir)
    }

    #[test]
    fn every_definition_maps_to_an_action() {
        for def in definitions() {
            let name = def["function"]["name"].as_str().expect("name");
            assert!(
                name.starts_with("task_")
                    || matches!(name, "http_request" | "self_note" | "improve")
                    || action_for(name).is_some(),
                "{name}"
            );
        }
        assert!(action_for("rm_rf").is_none());
    }

    #[test]
    fn library_search_schema_advertises_the_dialogue_budget() {
        let definition = definitions()
            .into_iter()
            .find(|def| def["function"]["name"] == "library_search")
            .expect("search definition");
        let limit = &definition["function"]["parameters"]["properties"]["limit"];
        assert_eq!(limit["type"], "integer");
        assert_eq!(limit["minimum"], 1);
        assert_eq!(limit["maximum"], 20);
        assert_eq!(limit["default"], 8);
    }

    #[test]
    fn library_search_caps_tool_results_and_preserves_direct_default() {
        let (shared, _dir) = search_fixture();
        for (limit, expected) in [
            (None, 8),
            (Some(json!(3)), 3),
            (Some(json!(0)), 1),
            (Some(json!(20)), 20),
            (Some(json!(100)), 20),
            (Some(json!(u64::MAX)), 20),
            (Some(json!(-1)), 8),
            (Some(json!(1.5)), 8),
            (Some(json!("3")), 8),
            (Some(Value::Null), 8),
        ] {
            let mut arguments = json!({"library": "demo", "q": "トヨタ"});
            if let Some(limit) = limit {
                arguments["limit"] = limit;
            }
            // Exercise both OpenAI's string arguments and direct object calls.
            for args in [arguments.clone(), json!(arguments.to_string())] {
                let (status, reply) = call_inner(
                    &shared,
                    &json!({"name": "library_search", "arguments": args}),
                    None,
                );
                assert_eq!(status, 200);
                assert_eq!(reply["ok"], true);
                let result = &reply["result"];
                assert_eq!(result["hits"].as_array().expect("hits").len(), expected);
                assert_eq!(result["truncated"], true);
                assert_eq!(result["hits"][0]["path"], "data.txt");
                assert_eq!(result["hits"][0]["line"], 1);
                assert!(
                    result["hits"][0]["snippet"]
                        .as_str()
                        .expect("snippet")
                        .contains("トヨタ")
                );
            }
        }

        // The direct HTTP/CLI entry point retains its existing 100-hit budget.
        let request = json!({"action": "search", "library": "demo", "q": "トヨタ"});
        let (status, result) = crate::library::handle(&shared, &request);
        assert_eq!(status, 200);
        assert_eq!(result["hits"].as_array().expect("hits").len(), 100);
        assert_eq!(result["truncated"], true);
    }

    /// A node whose `tools` section is `tools`; `Value::Null` omits it
    /// entirely, exercising the fail-closed defaults.
    fn http_shared(tools: Value) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
        });
        if !tools.is_null() {
            cfg["tools"] = tools;
        }
        let config = serde_json::from_value(cfg).expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").expect("spool");
        (Shared::new(config, spool, 1, None), dir)
    }

    fn http_call(shared: &Shared, arguments: Value) -> (u16, Value) {
        call_inner(
            shared,
            &json!({"name": "http_request", "arguments": arguments}),
            None,
        )
    }

    fn http_fixture() -> (kamimusuhi_testkit::FixtureServer, String) {
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::RawHttp {
                response: "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                           Content-Length: 2\r\nConnection: close\r\n\r\n{}"
                    .to_owned(),
            },
        )
        .expect("fixture");
        let url = format!("http://127.0.0.1:{}/items", server.port());
        (server, url)
    }

    /// Every file under `dir`, concatenated — for "the secret is nowhere
    /// on disk" assertions.
    fn everything_on_disk(dir: &std::path::Path) -> String {
        let mut out = String::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).expect("read_dir") {
                let entry = entry.expect("entry").path();
                if entry.is_dir() {
                    stack.push(entry);
                } else {
                    out.push_str(&String::from_utf8_lossy(
                        &std::fs::read(&entry).expect("read"),
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn an_empty_host_allowlist_refuses_everything() {
        let (server, url) = http_fixture();
        // No `tools` section at all: the defaults must still fail closed.
        let (shared, _dir) = http_shared(Value::Null);
        let (status, reply) = http_call(&shared, json!({"method": "GET", "url": url}));
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "denied");
        assert_eq!(server.request_count(), 0, "no socket may open");
    }

    #[test]
    fn host_allowlist_is_exact_or_parent_domain() {
        let allow = vec!["github.com".to_owned(), "127.0.0.1".to_owned()];
        assert!(http_host_allowed(&allow, "github.com"));
        assert!(http_host_allowed(&allow, "api.github.com"));
        assert!(http_host_allowed(&allow, "API.GitHub.COM"));
        assert!(http_host_allowed(&allow, "127.0.0.1"));
        assert!(!http_host_allowed(&allow, "evilgithub.com"));
        assert!(!http_host_allowed(&allow, "github.com.evil.test"));
        assert!(!http_host_allowed(&allow, "127.0.0.10"));
        assert!(!http_host_allowed(&[], "anything"));
    }

    #[test]
    fn a_host_off_the_allowlist_never_gets_a_socket() {
        let (server, url) = http_fixture();
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["example.com"]}));
        let (status, reply) = http_call(&shared, json!({"method": "GET", "url": url}));
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "denied");
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn a_disallowed_method_is_refused_before_the_socket() {
        let (server, url) = http_fixture();
        // Default methods are GET and POST only.
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        for method in ["DELETE", "PUT", "PATCH"] {
            let (status, reply) = http_call(&shared, json!({"method": method, "url": url}));
            assert_eq!(status, 200, "{method}");
            assert_eq!(reply["ok"], false, "{method}");
            assert_eq!(reply["error"]["kind"], "denied", "{method}");
        }
        assert_eq!(server.request_count(), 0);

        // Opting in works: PUT runs when the config lists it.
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"],
                   "http_allow_methods": ["GET", "POST", "PUT"]}));
        let (_status, reply) = http_call(&shared, json!({"method": "PUT", "url": url}));
        assert_eq!(reply["ok"], true);
        assert_eq!(server.request_count(), 1);
    }

    #[test]
    fn an_allowed_request_round_trips_clipped_and_header_lite() {
        let big = "x".repeat(40 * 1024);
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             X-RateLimit-Remaining: 41\r\nSet-Cookie: session=SESS\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{big}",
            big.len()
        );
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::RawHttp { response: raw },
        )
        .expect("fixture");
        let url = format!("http://127.0.0.1:{}/items", server.port());
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));

        let (status, reply) = http_call(
            &shared,
            json!({"method": "GET", "url": url, "headers": {"Accept": "application/json"}}),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], true, "{reply}");
        let result = &reply["result"];
        assert_eq!(result["status"], 200);
        assert_eq!(result["headers"]["x-ratelimit-remaining"], "41");
        // set-cookie is not a lite header: it cannot reach the result.
        assert!(
            !result.to_string().contains("SESS"),
            "non-lite headers must be dropped"
        );
        assert_eq!(result["body"].as_str().expect("body").len(), 32 * 1024);
        assert_eq!(result["body_truncated"], true);
        assert_eq!(server.request_count(), 1);
        assert_eq!(server.requests()[0].path, "/items");
    }

    #[test]
    fn an_oversized_body_never_leaves() {
        let (server, url) = http_fixture();
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        let (status, reply) = http_call(
            &shared,
            json!({"method": "POST", "url": url, "body": "y".repeat(33 * 1024)}),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "invalid");
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn env_headers_resolve_at_call_time_and_never_reach_disk() {
        let (server, url) = http_fixture();
        let (shared, dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        // The crate forbids `unsafe` (set_var needs it in edition 2024), so
        // the reference points at a variable that is certainly set.
        let secret = std::env::var("PATH").expect("PATH");
        let (status, reply) = http_call(
            &shared,
            json!({"method": "GET", "url": url, "headers": {
                "Authorization": "$ENV:PATH",
                "X-Token": "literal-secret-abc123",
            }}),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(
            server.requests()[0].authorization.as_deref(),
            Some(secret.as_str()),
            "the $ENV value was sent"
        );

        let disk = everything_on_disk(dir.path());
        assert!(!disk.contains(&secret), "resolved secret persisted");
        assert!(
            !disk.contains("literal-secret-abc123"),
            "literal secret persisted"
        );
        // The journal kept the header *names* though — enough to audit
        // what was sent without storing what it was.
        assert!(disk.contains("Authorization"), "{disk}");
        assert!(disk.contains("X-Token"), "{disk}");
        // The http_request journal itself exists under the spool.
        assert!(dir.path().join("spool/logs/http_request/test").is_dir());
    }

    #[test]
    fn a_missing_env_variable_refuses_the_call() {
        let (server, url) = http_fixture();
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        let (status, reply) = http_call(
            &shared,
            json!({"method": "GET", "url": url, "headers": {
                "Authorization": "$ENV:KAMIMUSUHI_DEFINITELY_UNSET_VAR_9F3B"}}),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "denied");
        assert!(
            reply["error"]["detail"]
                .as_str()
                .expect("detail")
                .contains("KAMIMUSUHI_DEFINITELY_UNSET_VAR_9F3B")
        );
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn a_stalled_server_is_a_structured_timeout_not_a_hang() {
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::Silence { ms: 2_000 },
        )
        .expect("fixture");
        let url = format!("http://127.0.0.1:{}/slow", server.port());
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        let (status, reply) = http_request_tool(
            &shared,
            &json!({"method": "GET", "url": url}),
            std::time::Duration::from_millis(300),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "timeout", "{reply}");
    }

    #[test]
    fn call_records_http_request_on_the_task_board() {
        let (server, url) = http_fixture();
        let (shared, _dir) = http_shared(json!({"http_allow_hosts": ["127.0.0.1"]}));
        let listed = list(&shared);
        let names: Vec<&str> = listed["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(names.contains(&"http_request"));

        let (status, reply) = call(
            &shared,
            &json!({"name": "http_request",
                    "arguments": {"method": "GET", "url": url}}),
            None,
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(server.request_count(), 1);
    }

    /// A node with one peer (`peerbox`) at `url`.
    fn peer_shared(url: &str) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "peers": [{"id": "peerbox", "role": "cognition", "url": url}]
        }))
        .expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").expect("spool");
        (Shared::new(config, spool, 1, None), dir)
    }

    fn raw_json(body: &Value) -> kamimusuhi_testkit::FixtureResponse {
        let body = body.to_string();
        kamimusuhi_testkit::FixtureResponse::RawHttp {
            response: format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        }
    }

    fn mark_healthy(shared: &Shared, peer: &str) {
        shared
            .peers
            .write()
            .expect("peers")
            .entry(peer.to_owned())
            .or_default()
            .record(Ok(json!({"ok": true})), 1);
    }

    #[test]
    fn an_unhealthy_peer_is_neither_listed_nor_called() {
        // A peer failing health probes is not contacted: its socket may
        // accept a connection and still never answer, which is what used to
        // stall every /v1/tools listing for minutes while llm_master was
        // down.
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::ok("unused"),
        )
        .expect("fixture");
        let url = format!("http://127.0.0.1:{}", server.port());
        let (shared, _dir) = peer_shared(&url);

        let listed = list_with(&shared, false);
        assert_eq!(server.request_count(), 0, "unhealthy peer was contacted");
        let names: Vec<&str> = listed["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(names.contains(&"json_get"));
        assert!(!names.iter().any(|n| n.starts_with("mcp__")));

        let (status, reply) = call(
            &shared,
            &json!({"name": "mcp__peerbox__echo", "arguments": {}}),
            None,
        );
        assert_eq!(status, 400);
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or("")
                .contains("unavailable")
        );
        assert_eq!(server.request_count(), 0, "unhealthy peer was called");
    }

    #[test]
    fn a_healthy_peers_tools_are_listed_and_calls_forward() {
        let listing = raw_json(&json!({
            "tools": [{"type": "function", "function": {
                "name": "mcp__peerbox__echo", "description": "echo",
                "parameters": {"type": "object"}}}],
            "mcp_servers": [{"name": "peerbox", "node": "peerbox",
                             "state": "running", "tools": 1}]}));
        let server = kamimusuhi_testkit::FixtureServer::always(listing).expect("fixture");
        let url = format!("http://127.0.0.1:{}", server.port());
        let (shared, _dir) = peer_shared(&url);
        mark_healthy(&shared, "peerbox");

        let listed = list_with(&shared, false);
        let names: Vec<&str> = listed["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(names.contains(&"mcp__peerbox__echo"));
        assert!(
            listed["mcp_servers"]
                .as_array()
                .expect("mcp_servers")
                .iter()
                .any(|s| s["name"] == "peerbox")
        );

        let (status, _reply) = call(
            &shared,
            &json!({"name": "mcp__peerbox__echo", "arguments": {}}),
            None,
        );
        assert_eq!(status, 200);
        let requests = server.requests();
        // tools/list, the library catalog inside the same listing, then the
        // forwarded call itself.
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].path, "/v1/tools/call");
        assert!(requests[2].body.contains("\"local_only\":true"));
    }

    // ------------------------------------------------------------------
    // self_note / improve
    // ------------------------------------------------------------------

    /// A node hosting an individual: `dir` is the runtime directory with a
    /// marker `kamimusuhi.sqlite`, `binary` a script pretending to be the
    /// runtime CLI.
    fn self_improve_fixture(script_body: &str) -> (Shared, tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let rt = dir.path().join("rt");
        std::fs::create_dir_all(&rt).expect("rt dir");
        std::fs::write(rt.join("kamimusuhi.sqlite"), b"").expect("sqlite marker");
        let bin = dir.path().join("fake-runtime");
        std::fs::write(&bin, format!("#!/bin/sh\n{script_body}\n")).expect("fake runtime");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 5},
        }))
        .expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").expect("spool");
        (Shared::new(config, spool, 1, None), dir, rt)
    }

    fn self_note_call(shared: &Shared, args: Value, turn: Option<&str>) -> (u16, Value) {
        call(
            shared,
            &json!({"name": "self_note", "arguments": args}),
            turn,
        )
    }

    #[test]
    fn self_note_appends_whitelisted_bounded_notes() {
        let (shared, _dir, rt) = self_improve_fixture("exit 0");
        let file = kamimusuhi_runtime::self_notes::path(&rt);

        let (status, reply) = self_note_call(
            &shared,
            json!({"kind": "lesson", "text": "長くなる前に確認する",
                   "evidence_refs": ["ev-1", "rec-2"]}),
            Some("t-turn-1"),
        );
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["note_id"], "sn-1");

        // The note file carries the lane's own shape — and the call is
        // journaled like every other tool.
        let notes = kamimusuhi_runtime::self_notes::load(&rt).expect("load");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind.as_str(), "lesson");
        assert_eq!(notes[0].text, "長くなる前に確認する");
        assert_eq!(notes[0].evidence_refs, vec!["ev-1", "rec-2"]);
        let journal_dir = shared.spool.root().join("logs/self_note/test");
        let journal = std::fs::read_to_string(
            std::fs::read_dir(&journal_dir)
                .expect("journal dir")
                .next()
                .expect("one journal file")
                .expect("entry")
                .path(),
        )
        .expect("journal");
        assert!(journal.contains("sn-1"), "{journal}");

        // Unknown kind: refused before anything is written.
        let (_, reply) = self_note_call(
            &shared,
            json!({"kind": "fact", "text": "x"}),
            Some("t-turn-2"),
        );
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("tendency"));
        assert_eq!(kamimusuhi_runtime::self_notes::load(&rt).unwrap().len(), 1);
        drop(file);
    }

    #[test]
    fn self_note_bounds_and_the_per_turn_cap_hold() {
        let (shared, _dir, rt) = self_improve_fixture("exit 0");
        // Text over 1KiB is refused, not clipped.
        let (_, reply) = self_note_call(
            &shared,
            json!({"kind": "observation", "text": "x".repeat(1025)}),
            None,
        );
        assert_eq!(reply["ok"], false);
        // Too many refs is refused too.
        let (_, reply) = self_note_call(
            &shared,
            json!({"kind": "observation", "text": "t",
                   "evidence_refs": (0..9).map(|i| i.to_string()).collect::<Vec<_>>()}),
            None,
        );
        assert_eq!(reply["ok"], false);
        assert!(
            kamimusuhi_runtime::self_notes::load(&rt)
                .unwrap()
                .is_empty()
        );

        // The turn's budget is 4; the fifth call in one turn is refused
        // while calls in other turns still pass.
        let turn = "t-turn-cap";
        for i in 0..4 {
            let (_, reply) = self_note_call(
                &shared,
                json!({"kind": "tendency", "text": format!("n{i}")}),
                Some(turn),
            );
            assert_eq!(reply["ok"], true, "call {i}: {reply}");
        }
        let (_, reply) = self_note_call(
            &shared,
            json!({"kind": "tendency", "text": "over"}),
            Some(turn),
        );
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("per turn"));
        let (_, reply) = self_note_call(
            &shared,
            json!({"kind": "tendency", "text": "other turn"}),
            Some("t-other"),
        );
        assert_eq!(reply["ok"], true);
        assert_eq!(kamimusuhi_runtime::self_notes::load(&rt).unwrap().len(), 5);
    }

    #[test]
    fn improve_validates_before_proposing_and_queues_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rt = dir.path().join("rt");
        std::fs::create_dir_all(&rt).expect("rt dir");
        std::fs::write(rt.join("kamimusuhi.sqlite"), b"").expect("sqlite marker");
        let marker = dir.path().join("invoked.txt");
        let bin = dir.path().join("fake-runtime");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\necho '{{\"proposal_id\":\"p1\",\"kind\":\"retrieval_update\",\"target\":\"{}\",\"proposed_value\":6,\"status\":\"pending\"}}'\n",
                marker.display(),
                "retrieval.self_notes_max"
            ),
        )
        .expect("fake runtime");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 5},
        }))
        .expect("config");
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").expect("spool");
        let shared = Shared::new(config, spool, 1, None);

        // A param outside the closed vocabulary never reaches the runtime:
        // the bound error comes back first and the binary is not invoked.
        let (_, reply) = call(
            &shared,
            &json!({"name": "improve", "arguments": {
                "param": "retrieval.made_up_knob", "value": 4, "reason": "why"}}),
            None,
        );
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["kind"], "closed_vocabulary");
        assert!(
            reply["error"]["detail"]
                .as_str()
                .unwrap()
                .contains("unknown param target")
        );
        assert!(!marker.exists(), "invalid param spawned the runtime");

        // Out-of-bounds and fractional values for integer params are refused
        // the same way.
        for bad in [json!(10_000), json!(1.5), json!("6")] {
            let (_, reply) = call(
                &shared,
                &json!({"name": "improve", "arguments": {
                    "param": "retrieval.self_notes_max", "value": bad}}),
                None,
            );
            assert_eq!(reply["ok"], false, "{reply}");
        }
        assert!(!marker.exists());

        // A valid proposal goes through the proposal machinery and comes
        // back pending — never applied by the tool itself.
        let (_, reply) = call(
            &shared,
            &json!({"name": "improve", "arguments": {
                "param": "dialogue.max_response_chars", "value": 2000,
                "reason": "shorter replies work better"}}),
            None,
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["status"], "pending");
        assert_eq!(reply["result"]["proposal"]["proposal_id"], "p1");
        // `dialogue.*` was normalized to the canonical conversation.* key
        // before validation — and before the spawn.
        let invoked = std::fs::read_to_string(&marker).expect("marker");
        assert!(
            invoked.contains("--param conversation.max_response_chars"),
            "{invoked}"
        );
    }

    #[test]
    fn the_self_notes_endpoint_reads_the_tail() {
        let (shared, _dir, rt) = self_improve_fixture("exit 0");
        for i in 0..12 {
            kamimusuhi_runtime::self_notes::append(
                &rt,
                kamimusuhi_core::self_note::SelfNoteKind::Observation,
                &format!("n{i}"),
                vec![],
                kamimusuhi_core::time::UtcTimestamp::from_unix_millis(i),
            )
            .expect("append");
        }
        let (status, reply) = crate::self_notes::self_notes(&shared, &json!({"limit": 5}));
        assert_eq!(status, 200);
        let notes = reply["self_notes"].as_array().expect("notes");
        assert_eq!(notes.len(), 5);
        assert_eq!(notes[0]["text"], "n7");
        assert_eq!(notes[4]["text"], "n11");
        assert_eq!(reply["file_present"], true);
    }
}
