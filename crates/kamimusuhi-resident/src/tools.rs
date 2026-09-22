//! MCP-like tool surface over HTTP.
//!
//! * `GET  /v1/tools` — tool definitions in OpenAI function-calling form,
//!   plus the library catalog they operate on.
//! * `POST /v1/tools/call` — `{"name": "...", "arguments": {...}}`.
//!
//! Built-in tools are read-only; MCP tools are what their servers offer,
//! and any tool listed in `approval_required` is queued for operator
//! approval instead of executed (see `approvals.rs`). A tool error is returned as a result
//! (`ok: false`) so a model can see it and recover; only an unknown tool
//! name or a malformed request is an HTTP error.

use serde_json::{Value, json};

use crate::state::Shared;

const DEFAULT_SEARCH_HITS: u64 = 8;
const MAX_SEARCH_HITS: u64 = 20;

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
            "Add a task you plan to work on (shown to the operator on the task board). Use depends_on to link it after existing task ids.",
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

fn post_peer(url: &str, suffix: &str, body: &Value, token: Option<&str>) -> Option<(u16, Value)> {
    use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
    let endpoint = Endpoint::parse(url, suffix).ok()?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &crate::client::auth(token),
        std::time::Duration::from_secs(300),
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
            if let Some((200, v)) = post_peer(
                &peer.url,
                "/v1/tools/list",
                &json!({"local_only": true}),
                shared.token.as_deref(),
            ) {
                for tool in v["tools"].as_array().into_iter().flatten() {
                    let name = tool["function"]["name"].as_str().unwrap_or("").to_owned();
                    if name.starts_with("mcp__") && seen.insert(name) {
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
            .first()
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
        .first()
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
fn task_tool(shared: &Shared, name: &str, request: &Value) -> (u16, Value) {
    let mut args = match &request["arguments"] {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other.clone(),
    };
    // A task the individual plans mid-conversation follows that turn.
    if name == "task_create"
        && args["depends_on"].as_array().is_none_or(Vec::is_empty)
        && let Some(turn) = shared
            .current_turn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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

/// Execute a tool call and record it on the task board as a successor of
/// the dialogue turn in progress. Forwarded (`local_only`) calls are
/// recorded by the node that forwarded them, not twice.
pub fn call(shared: &Shared, request: &Value) -> (u16, Value) {
    let name = request["name"].as_str().unwrap_or("");
    if name.starts_with("task_") {
        return task_tool(shared, name, request);
    }
    if request["local_only"].as_bool().unwrap_or(false) {
        return call_inner(shared, request);
    }
    let turn = shared
        .current_turn
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let node = executing_node(shared, name, &request["arguments"]);
    let task = shared.tasks.create(
        &shared.spool,
        crate::tasks::NewTask {
            title: &call_title(name, &request["arguments"]),
            kind: "tool",
            node: &node,
            owner: "mio",
            status: crate::tasks::TaskStatus::InProgress,
            depends_on: turn.into_iter().collect(),
            detail: json!({"tool": name}),
        },
    );
    let started = std::time::Instant::now();
    let (status, reply) = call_inner(shared, request);
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

fn call_inner(shared: &Shared, request: &Value) -> (u16, Value) {
    let name = request["name"].as_str().unwrap_or("");
    if name.starts_with("mcp__") {
        return call_mcp(shared, name, request);
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

fn call_mcp(shared: &Shared, name: &str, request: &Value) -> (u16, Value) {
    let arguments = match &request["arguments"] {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other.clone(),
    };
    for server in &shared.mcp {
        if let Some(tool) = server.tools().into_iter().find(|t| t.exposed == name) {
            if server.requires_approval(&tool.original) {
                return match shared.approvals.enqueue(
                    &server.config.name,
                    &tool.original,
                    &tool.exposed,
                    arguments.clone(),
                    None,
                ) {
                    Ok(a) => {
                        let _ = shared.spool.append(
                            "logs/approvals",
                            json!({"event": "requested", "approval": a.view()}),
                        );
                        (
                            200,
                            json!({"ok": false, "pending_approval": true, "approval_id": a.id,
                                   "message": "操作者の承認待ちです。まだ実行されていません。承認IDを操作者に伝えてください。"}),
                        )
                    }
                    Err(e) => (200, json!({"ok": false, "error": e})),
                };
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
            if let Some((status, v)) = post_peer(
                &peer.url,
                "/v1/tools/call",
                &forwarded,
                shared.token.as_deref(),
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
                name.starts_with("task_") || action_for(name).is_some(),
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
}
