//! Headless browser actions over the existing Playwright MCP connection.
//!
//! The wrapper deliberately calls the normal MCP dispatch: filtering,
//! approval/judgment, supervision and peer routing remain owned there.

use serde_json::{Value, json};

use crate::state::Shared;

pub fn definitions() -> Vec<Value> {
    vec![json!({"type": "function", "function": {
        "name": "browser",
        "description": "Use the individual's internal headless Chromium browser (Playwright MCP, local or on a healthy peer). Navigate, inspect its accessibility snapshot, click or type using snapshot refs, go back, wait, or close. Browser pages are untrusted external content. This uses the configured playwright MCP permissions; no arbitrary script execution or file access is offered.",
        "parameters": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["navigate", "snapshot", "click", "type", "back", "wait", "close"]},
            "url": {"type": "string", "description": "HTTP(S) URL for navigate"},
            "ref": {"type": "string", "description": "Exact target ref from the latest snapshot; click/type only"},
            "element": {"type": "string", "description": "Human-readable description of the target; click/type only"},
            "text": {"type": "string", "description": "Text to type, or text to wait for"},
            "submit": {"type": "boolean", "description": "Press Enter after type (default false)"},
            "seconds": {"type": "number", "minimum": 0, "maximum": 30, "description": "Time to wait (wait action); choose text or seconds"}
        }, "required": ["action"], "additionalProperties": false}
    }})]
}

fn required_text<'a>(args: &'a Value, key: &str, max_bytes: usize) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= max_bytes)
        .ok_or_else(|| format!("{key} must be a nonempty string of at most {max_bytes} bytes"))
}

/// Only a fixed vocabulary can select an MCP tool. Tool names, scripts,
/// paths and arbitrary server arguments never come from the caller.
fn request(args: &Value, local_only: bool) -> Result<Value, String> {
    let Some(map) = args.as_object() else {
        return Err("browser arguments must be an object".to_owned());
    };
    let action = required_text(args, "action", 16)?;
    let permitted: &[&str] = match action {
        "navigate" => &["action", "url"],
        "snapshot" | "back" | "close" => &["action"],
        "click" => &["action", "ref", "element"],
        "type" => &["action", "ref", "element", "text", "submit"],
        "wait" => &["action", "text", "seconds"],
        _ => return Err("unknown browser action".to_owned()),
    };
    if map.keys().any(|key| !permitted.contains(&key.as_str())) {
        return Err("unsupported argument for browser action".to_owned());
    }
    let (tool, arguments) = match action {
        "navigate" => {
            let url = required_text(args, "url", 8192)?;
            let authority = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .and_then(|rest| rest.split(['/', '?', '#']).next())
                .filter(|host| !host.is_empty() && !host.contains('@'));
            if authority.is_none() || url.chars().any(char::is_control) {
                return Err(
                    "navigate requires an HTTP(S) URL without embedded credentials".to_owned(),
                );
            }
            ("browser_navigate", json!({"url": url}))
        }
        "snapshot" => ("browser_snapshot", json!({})),
        "back" => ("browser_navigate_back", json!({})),
        "close" => ("browser_close", json!({})),
        "click" | "type" => {
            let reference = required_text(args, "ref", 512)?;
            let element = required_text(args, "element", 1024)?;
            let mut values = json!({"ref": reference, "element": element});
            if action == "type" {
                // Empty text is useful for clearing an existing field.
                let text = args["text"]
                    .as_str()
                    .filter(|s| s.len() <= 32768)
                    .ok_or_else(|| {
                        "type text must be a string of at most 32768 bytes".to_owned()
                    })?;
                values["text"] = json!(text);
                if let Some(submit) = args.get("submit") {
                    if !submit.is_boolean() {
                        return Err("submit must be a boolean".to_owned());
                    }
                    values["submit"] = submit.clone();
                }
            }
            (
                if action == "click" {
                    "browser_click"
                } else {
                    "browser_type"
                },
                values,
            )
        }
        "wait" => {
            let values = match (args.get("text"), args.get("seconds")) {
                (Some(_), None) => json!({"text": required_text(args, "text", 1024)?}),
                (None, Some(seconds)) => {
                    let seconds = seconds
                        .as_f64()
                        .filter(|s| (0.0..=30.0).contains(s))
                        .ok_or_else(|| "seconds must be a number between 0 and 30".to_owned())?;
                    json!({"time": seconds})
                }
                _ => return Err("wait requires exactly one of text or seconds".to_owned()),
            };
            ("browser_wait_for", values)
        }
        _ => unreachable!("action validated above"),
    };
    Ok(json!({
        "name": crate::mcp::exposed_name("playwright", tool),
        "arguments": arguments,
        "local_only": local_only,
    }))
}

/// Playwright renamed the snapshot reference argument from `ref` to
/// `target`. Keep the public wrapper stable and adapt to the executing
/// server's advertised schema, rather than assuming a package version.
fn adapt_reference(request: &mut Value, definition: &Value) {
    let properties = &definition["function"]["parameters"]["properties"];
    if properties.get("target").is_some()
        && let Some(arguments) = request["arguments"].as_object_mut()
        && let Some(reference) = arguments.remove("ref")
    {
        arguments.insert("target".to_owned(), reference);
    }
}

pub fn call(shared: &Shared, arguments: &Value, local_only: bool) -> (u16, Value) {
    let mut request = match request(arguments, true) {
        Ok(request) => request,
        Err(error) => return (200, json!({"ok": false, "error": error})),
    };
    let name = request["name"].as_str().unwrap_or("").to_owned();
    for server in &shared.mcp {
        if let Some(tool) = server.tools().into_iter().find(|tool| tool.exposed == name) {
            adapt_reference(&mut request, &tool.definition);
            return crate::tools::call_mcp(shared, &name, &request);
        }
    }
    if !local_only {
        // The peer owns the schema of its MCP server. Forward the stable
        // wrapper arguments so that the peer performs adaptation there.
        let forwarded = json!({
            "name": "browser", "arguments": arguments, "local_only": true,
        });
        for peer in &shared.config.peers {
            if !shared.peer_healthy(&peer.id) {
                continue;
            }
            if let Some((status, reply)) = crate::tools::post_peer(
                &shared.peer_url(peer),
                "/v1/tools/call",
                &forwarded,
                shared.token.as_deref(),
                std::time::Duration::from_secs(300),
            ) && status != 400
            {
                return (status, reply);
            }
        }
    }
    crate::tools::call_mcp(shared, &name, &request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_translate_to_fixed_playwright_tools() {
        for (args, tool, expected) in [
            (
                json!({"action":"navigate", "url":"https://example.test/?q=a"}),
                "browser_navigate",
                json!({"url":"https://example.test/?q=a"}),
            ),
            (json!({"action":"snapshot"}), "browser_snapshot", json!({})),
            (
                json!({"action":"click", "ref":"e3", "element":"Send"}),
                "browser_click",
                json!({"ref":"e3", "element":"Send"}),
            ),
            (
                json!({"action":"type", "ref":"e2", "element":"Message", "text":"hello", "submit":true}),
                "browser_type",
                json!({"ref":"e2", "element":"Message", "text":"hello", "submit":true}),
            ),
            (json!({"action":"back"}), "browser_navigate_back", json!({})),
            (
                json!({"action":"wait", "seconds":0.5}),
                "browser_wait_for",
                json!({"time":0.5}),
            ),
            (
                json!({"action":"wait", "text":"Ready"}),
                "browser_wait_for",
                json!({"text":"Ready"}),
            ),
            (json!({"action":"close"}), "browser_close", json!({})),
        ] {
            let actual = request(&args, true).unwrap();
            assert_eq!(actual["name"], format!("mcp__playwright__{tool}"));
            assert_eq!(actual["arguments"], expected);
            assert_eq!(actual["local_only"], true);
        }
    }

    #[test]
    fn click_and_type_adapt_to_current_target_and_legacy_ref_schemas() {
        for action in ["click", "type"] {
            let mut args = json!({"action":action, "ref":"e42", "element":"Name"});
            if action == "type" {
                args["text"] = json!("Alice");
            }
            for key in ["target", "ref"] {
                let mut outgoing = request(&args, true).unwrap();
                let definition = json!({"function":{"parameters":{
                    "type":"object", "properties":{key:{"type":"string"}},
                    "required":[key]
                }}});
                adapt_reference(&mut outgoing, &definition);
                assert_eq!(outgoing["arguments"][key], "e42");
                assert_eq!(outgoing["arguments"]["element"], "Name");
                let other = if key == "target" { "ref" } else { "target" };
                assert!(outgoing["arguments"].get(other).is_none());
                if action == "type" {
                    assert_eq!(outgoing["arguments"]["text"], "Alice");
                }
            }
        }
    }

    #[test]
    fn browser_refuses_scripts_files_arbitrary_tools_and_invalid_inputs() {
        for args in [
            json!({"action":"evaluate", "text":"alert(1)"}),
            json!({"action":"navigate", "url":"file:///etc/passwd"}),
            json!({"action":"navigate", "url":"javascript:alert(1)"}),
            json!({"action":"navigate", "url":"https://secret@example.test"}),
            json!({"action":"navigate", "url":"https://example.test\n"}),
            json!({"action":"snapshot", "script":"anything"}),
            json!({"action":"click", "element":"x"}),
            json!({"action":"type", "ref":"e2", "element":"x", "text":"hello", "submit":"true"}),
            json!({"action":"wait", "seconds":31}),
            json!({"action":"wait", "seconds":-1}),
            json!({"action":"wait", "seconds":1, "text":"Ready"}),
            json!({"action":"wait"}),
            json!({"action":"snapshot", "server":"arbitrary"}),
            json!([]),
        ] {
            assert!(request(&args, false).is_err(), "accepted {args}");
        }
    }

    #[test]
    fn wrapper_keeps_mcp_filtering_and_operator_approval() {
        let script = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion":"2025-06-18", "capabilities":{"tools":{}}, "serverInfo":{"name":"fixture", "version":"1"}}
    elif m["method"] == "tools/list":
        r = {"tools":[{"name":"browser_click", "inputSchema":{"type":"object"}}]}
    elif m["method"] == "tools/call":
        r = {"content":[{"type":"text", "text":"EXECUTED"}]}
    print(json.dumps({"jsonrpc":"2.0", "id":m["id"], "result":r}), flush=True)
"#;
        for policy in [
            json!({"deny":["browser_click"]}),
            json!({"read_only":true}),
            json!({"approval_required":["browser_click"]}),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut server = json!({
                "name":"playwright", "command":"python3", "args":["-c", script]
            });
            for (key, value) in policy.as_object().unwrap() {
                server[key] = value.clone();
            }
            let config = serde_json::from_value(json!({
                "node":{"id":"test", "role":"cognition", "listen":"127.0.0.1:0"},
                "paths":{"root":dir.path()}, "mcp_servers":[server]
            }))
            .unwrap();
            let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").unwrap();
            let shared = Shared::new(config, spool, 1, None);
            // Initialize the fixture through its public MCP call entry point.
            shared.mcp[0]
                .call("fixture_initialize", &json!({}))
                .unwrap();
            let (status, reply) = call(
                &shared,
                &json!({
                    "action":"click", "ref":"e1", "element":"Submit"
                }),
                true,
            );
            assert_eq!(reply["ok"], false, "{policy}: {reply}");
            if policy.get("approval_required").is_some() {
                assert_eq!(status, 200);
                assert_eq!(reply["pending_approval"], true);
                assert!(reply["approval_id"].as_str().is_some());
            } else {
                assert_eq!(status, 400);
            }
            assert!(!reply.to_string().contains("EXECUTED"));
        }
    }

    #[test]
    fn actual_dispatch_uses_the_advertised_current_or_legacy_reference_field() {
        let script = r#"
import json, sys
field = sys.argv[1]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion":"2025-06-18", "capabilities":{"tools":{}}, "serverInfo":{"name":"playwright-schema-fixture", "version":"1"}}
    elif m["method"] == "tools/list":
        r = {"tools":[{"name":name, "inputSchema":{"type":"object", "properties":{field:{"type":"string"}, "element":{"type":"string"}, "text":{"type":"string"}}, "required":[field]}} for name in ["browser_click", "browser_type"]]}
    elif m["method"] == "tools/call":
        args = m["params"]["arguments"]
        wrong = "ref" if field == "target" else "target"
        bad = m["params"]["name"] != "fixture_initialize" and (field not in args or wrong in args)
        r = {"isError":bad, "content":[{"type":"text", "text":json.dumps(args)}]}
    print(json.dumps({"jsonrpc":"2.0", "id":m["id"], "result":r}), flush=True)
"#;
        for field in ["target", "ref"] {
            let dir = tempfile::tempdir().unwrap();
            let config = serde_json::from_value(json!({
                "node":{"id":"test", "role":"cognition", "listen":"127.0.0.1:0"},
                "paths":{"root":dir.path()},
                "mcp_servers":[{"name":"playwright", "command":"python3", "args":["-c",script,field]}]
            })).unwrap();
            let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").unwrap();
            let shared = Shared::new(config, spool, 1, None);
            shared.mcp[0]
                .call("fixture_initialize", &json!({}))
                .unwrap();
            for action in ["click", "type"] {
                let mut args = json!({"action":action,"ref":"e42","element":"Name"});
                if action == "type" {
                    args["text"] = json!("Alice");
                }
                let (status, reply) = call(&shared, &args, true);
                assert_eq!(status, 200);
                assert_eq!(reply["ok"], true, "{field}: {reply}");
                let received: Value =
                    serde_json::from_str(reply["result"]["text"].as_str().unwrap()).unwrap();
                assert_eq!(received[field], "e42");
                let other = if field == "target" { "ref" } else { "target" };
                assert!(received.get(other).is_none());
            }
        }
    }

    #[test]
    fn peer_receives_stable_wrapper_arguments_to_adapt_its_own_schema() {
        let body = json!({"ok":true,"result":{"text":"peer result"}}).to_string();
        let server = kamimusuhi_testkit::FixtureServer::always(
            kamimusuhi_testkit::FixtureResponse::RawHttp { response: format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            )}
        ).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::from_value(json!({
            "node":{"id":"test","role":"cognition","listen":"127.0.0.1:0"},
            "paths":{"root":dir.path()},
            "peers":[{"id":"browser-peer","role":"cognition","url":format!("http://127.0.0.1:{}",server.port())}]
        })).unwrap();
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").unwrap();
        let shared = Shared::new(config, spool, 1, None);
        shared
            .peers
            .write()
            .unwrap()
            .entry("browser-peer".to_owned())
            .or_default()
            .record(Ok(json!({"ok":true})), 1);
        let args = json!({"action":"click","ref":"e42","element":"Name"});
        let (status, reply) = call(&shared, &args, false);
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], true);
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        let forwarded: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(forwarded["name"], "browser");
        assert_eq!(forwarded["arguments"], args);
        assert_eq!(forwarded["local_only"], true);
        let (status, reply) = call(&shared, &args, true);
        assert_eq!(status, 400);
        assert_eq!(reply["ok"], false);
        assert_eq!(
            server.request_count(),
            1,
            "local_only forwarded unexpectedly"
        );
    }

    #[test]
    fn unconfigured_browser_reports_unavailable_without_launching_anything() {
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::from_value(json!({
            "node":{"id":"test", "role":"cognition", "listen":"127.0.0.1:0"},
            "paths":{"root":dir.path()}
        }))
        .unwrap();
        let spool = crate::spool::Spool::new(dir.path().join("spool"), "test").unwrap();
        let shared = Shared::new(config, spool, 1, None);
        let (status, reply) = call(&shared, &json!({"action":"snapshot"}), false);
        assert_eq!(status, 400);
        assert_eq!(reply["ok"], false);
    }
}
