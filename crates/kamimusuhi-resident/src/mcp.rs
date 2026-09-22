//! MCP client: runs configured MCP servers (stdio transport) as supervised
//! child processes and exposes their tools through the resident's tool
//! surface as `mcp__<server>__<tool>`.
//!
//! Protocol: newline-delimited JSON-RPC 2.0 — `initialize`, then
//! `notifications/initialized`, `tools/list` (paginated) and `tools/call`.
//! Requests the server sends to us are answered "method not found": this
//! client declares no capabilities (no roots, sampling or elicitation), so
//! a server's reach is exactly what its command line grants.
//!
//! A server that dies, hangs past a call deadline or fails to start is
//! restarted with backoff; its tools disappear from the listing meanwhile.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::{iso8601, unix_now};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_TOOL_NAME: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Short id; tools appear as `mcp__<name>__<tool>`.
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment. A value starting with `$` is copied from the
    /// resident's own environment (e.g. `"$GITHUB_TOKEN"`), so secrets stay
    /// in secrets.env rather than in this file.
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,
    /// Offer only tools annotated `readOnlyHint: true`.
    #[serde(default)]
    pub read_only: bool,
    /// If non-empty, only these tool names (server-side names) are offered.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default = "default_call_timeout")]
    pub call_timeout_secs: u64,
    /// Operator note shown in the tool catalog and each function definition.
    #[serde(default)]
    pub description: Option<String>,
    /// Tools offered to the model but executed only after the operator
    /// approves each call (see `approvals.rs`).
    #[serde(default)]
    pub approval_required: Vec<String>,
    /// Command + args run after an approved call succeeds; `{id}` and
    /// `{tool}` are substituted (e.g. commit and push the change).
    #[serde(default)]
    pub after_approved: Option<Vec<String>>,
}

const fn default_call_timeout() -> u64 {
    120
}

#[derive(Debug, Clone)]
pub struct McpTool {
    /// Exposed name, `mcp__<server>__<tool>` (sanitized, ≤ 64 chars).
    pub exposed: String,
    pub original: String,
    pub definition: Value,
}

/// OpenAI function names: `[A-Za-z0-9_-]{1,64}`.
pub fn exposed_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let full = format!("mcp__{}__{}", clean(server), clean(tool));
    if full.len() <= MAX_TOOL_NAME {
        return full;
    }
    // Keep it unique when truncating: short FNV-1a hash of the full name.
    let hash = full.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    format!("{}_{:08x}", &full[..MAX_TOOL_NAME - 9], hash & 0xffff_ffff)
}

struct Session {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
}

impl Session {
    fn send(&self, message: &Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().unwrap_or_else(|p| p.into_inner());
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("write to server: {e}"))
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let reply = rx.recv_timeout(timeout).map_err(|_| {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            if self.alive.load(Ordering::SeqCst) {
                format!("{method} timed out after {}s", timeout.as_secs())
            } else {
                "server exited".to_owned()
            }
        })?;
        if let Some(error) = reply.get("error") {
            return Err(format!(
                "server error {}: {}",
                error["code"],
                error["message"].as_str().unwrap_or("")
            ));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct McpServer {
    pub config: McpServerConfig,
    log_dir: std::path::PathBuf,
    session: Mutex<Option<Arc<Session>>>,
    tools: RwLock<Vec<McpTool>>,
    status: RwLock<Value>,
    refresh: Arc<AtomicBool>,
}

impl McpServer {
    pub fn new(config: McpServerConfig, log_dir: std::path::PathBuf) -> Self {
        Self {
            config,
            log_dir,
            session: Mutex::new(None),
            tools: RwLock::new(Vec::new()),
            status: RwLock::new(json!({"state": "not_started"})),
            refresh: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_status(&self, value: Value) {
        *self.status.write().unwrap_or_else(|p| p.into_inner()) = value;
    }

    pub fn status(&self) -> Value {
        let mut status = self
            .status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        status["name"] = json!(self.config.name);
        status["tools"] = json!(self.tools.read().unwrap_or_else(|p| p.into_inner()).len());
        status
    }

    pub fn tools(&self) -> Vec<McpTool> {
        self.tools.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn spawn(&self) -> Result<Arc<Session>, String> {
        let mut command = Command::new(&self.config.command);
        command.args(&self.config.args);
        for (key, value) in &self.config.env {
            let resolved = match value.strip_prefix('$') {
                Some(var) => std::env::var(var)
                    .map_err(|_| format!("environment variable {var} for {key} is not set"))?,
                None => value.clone(),
            };
            command.env(key, resolved);
        }
        if let Some(cwd) = &self.config.cwd {
            command.current_dir(cwd);
        }
        let _ = std::fs::create_dir_all(&self.log_dir);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_dir.join(format!("mcp-{}.log", self.config.name)))
            .map_err(|e| format!("log file: {e}"))?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.config.command))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let pending: Arc<Mutex<HashMap<u64, Sender<Value>>>> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        {
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            let stdin = Arc::clone(&stdin);
            let refresh = Arc::clone(&self.refresh);
            let name = self.config.name.clone();
            let _ = thread::Builder::new()
                .name(format!("mcp-{name}"))
                .spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        let Ok(message) = serde_json::from_str::<Value>(&line) else {
                            continue; // servers sometimes print banners
                        };
                        let method = message.get("method").and_then(Value::as_str);
                        match (message.get("id"), method) {
                            // Response to one of our requests.
                            (Some(id), None) => {
                                if let Some(tx) = id.as_u64().and_then(|id| {
                                    pending
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .remove(&id)
                                }) {
                                    let _ = tx.send(message);
                                }
                            }
                            // Request from the server: we offer no client features.
                            (Some(id), Some(method)) => {
                                let reply = json!({"jsonrpc": "2.0", "id": id, "error": {
                                    "code": -32601, "message": format!("{method} not supported")}});
                                let mut out = stdin.lock().unwrap_or_else(|p| p.into_inner());
                                let _ = writeln!(out, "{reply}");
                                let _ = out.flush();
                            }
                            (None, Some("notifications/tools/list_changed")) => {
                                refresh.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                    // Fail every waiter now rather than at its deadline.
                    pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
                });
        }
        let session = Arc::new(Session {
            child,
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            alive,
        });
        session.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                   "clientInfo": {"name": "kamimusuhi-resident",
                                  "version": env!("CARGO_PKG_VERSION")}}),
            Duration::from_secs(60),
        )?;
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(session)
    }

    fn list_tools(&self, session: &Session) -> Result<Vec<McpTool>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _page in 0..50 {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |c| json!({"cursor": c}));
            let result = session.request("tools/list", params, Duration::from_secs(60))?;
            for tool in result["tools"].as_array().into_iter().flatten() {
                let Some(original) = tool["name"].as_str() else {
                    continue;
                };
                if !self.config.allow.is_empty() && !self.config.allow.iter().any(|a| a == original)
                {
                    continue;
                }
                if self.config.deny.iter().any(|d| d == original) {
                    continue;
                }
                if self.config.read_only
                    && tool
                        .pointer("/annotations/readOnlyHint")
                        .and_then(Value::as_bool)
                        != Some(true)
                {
                    continue;
                }
                let exposed = exposed_name(&self.config.name, original);
                let gated = self.config.approval_required.iter().any(|a| a == original);
                let mut description = format!(
                    "[MCP {}]{} {}",
                    self.config.name,
                    if gated {
                        " [要承認: 呼ぶと操作者の承認待ちになり、承認されるまで実行されない]"
                    } else {
                        ""
                    },
                    tool["description"].as_str().unwrap_or("")
                );
                if let Some(note) = self
                    .config
                    .description
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    description.push_str("\n[Operator note] ");
                    description.push_str(note);
                }
                let parameters = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                tools.push(McpTool {
                    exposed: exposed.clone(),
                    original: original.to_owned(),
                    definition: json!({"type": "function", "function": {
                        "name": exposed, "description": description, "parameters": parameters}}),
                });
            }
            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Live session, starting one (and listing tools) when needed.
    fn ensure(&self) -> Result<Arc<Session>, String> {
        let mut slot = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(session) = slot.as_ref()
            && session.alive.load(Ordering::SeqCst)
        {
            if self.refresh.swap(false, Ordering::SeqCst) {
                let tools = self.list_tools(session)?;
                *self.tools.write().unwrap_or_else(|p| p.into_inner()) = tools;
            }
            return Ok(Arc::clone(session));
        }
        *slot = None;
        self.tools
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        let session = self.spawn()?;
        let tools = self.list_tools(&session)?;
        self.set_status(json!({"state": "running", "since": iso8601(unix_now())}));
        *self.tools.write().unwrap_or_else(|p| p.into_inner()) = tools;
        *slot = Some(Arc::clone(&session));
        Ok(session)
    }

    fn reset(&self, reason: &str) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.tools
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.set_status(json!({"state": "restarting", "reason": reason,
                               "at": iso8601(unix_now())}));
    }

    /// Call one tool by its server-side name.
    pub fn call(&self, original: &str, arguments: &Value) -> Result<Value, String> {
        let session = self.ensure()?;
        let timeout = Duration::from_secs(self.config.call_timeout_secs.max(1));
        match session.request(
            "tools/call",
            json!({"name": original, "arguments": arguments}),
            timeout,
        ) {
            Ok(result) => Ok(result),
            Err(e) => {
                // A hung or dead server is replaced rather than left wedged.
                if e.contains("timed out") || e.contains("exited") || e.contains("write to") {
                    self.reset(&e);
                }
                Err(e)
            }
        }
    }

    /// Whether calls to `original` must wait for operator approval.
    pub fn requires_approval(&self, original: &str) -> bool {
        self.config.approval_required.iter().any(|a| a == original)
    }

    /// Execute a call the operator has approved.
    pub fn call_approved(&self, original: &str, arguments: &Value) -> Result<Value, String> {
        self.call(original, arguments)
    }

    /// Keep the server up; refresh tools periodically. Runs on its own thread.
    pub fn supervise(self: &Arc<Self>) {
        let server = Arc::clone(self);
        let _ = thread::Builder::new()
            .name(format!("mcp-sup-{}", self.config.name))
            .spawn(move || {
                let mut backoff = Duration::from_secs(5);
                let mut last_refresh = Instant::now();
                loop {
                    match server.ensure() {
                        Ok(_) => {
                            backoff = Duration::from_secs(5);
                            if last_refresh.elapsed() > Duration::from_secs(600) {
                                server.refresh.store(true, Ordering::SeqCst);
                                last_refresh = Instant::now();
                            }
                            thread::sleep(Duration::from_secs(15));
                        }
                        Err(e) => {
                            server.reset(&e);
                            server.set_status(json!({"state": "failed", "error": e,
                                "retry_in_secs": backoff.as_secs(), "at": iso8601(unix_now())}));
                            thread::sleep(backoff);
                            backoff = (backoff * 2).min(Duration::from_secs(600));
                        }
                    }
                }
            });
    }
}

/// MCP `tools/call` result → the resident's tool-result shape.
pub fn normalize(result: &Value) -> (bool, Value) {
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let mut texts = Vec::new();
    let mut other = Vec::new();
    for part in result["content"].as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => texts.push(part["text"].as_str().unwrap_or("").to_owned()),
            Some(kind) => other.push(json!({"type": kind, "omitted": true,
                "mimeType": part.get("mimeType"), "uri": part.pointer("/resource/uri")})),
            None => {}
        }
    }
    let mut out = json!({"text": texts.join("\n")});
    if let Some(structured) = result.get("structuredContent") {
        out["structured"] = structured.clone();
    }
    if !other.is_empty() {
        out["non_text_content"] = Value::Array(other);
    }
    (!is_error, out)
}

/// Decode only observed Bing href targets; omit navigation and policy links.
fn bing_follow_up_url(url: &str) -> Option<String> {
    use base64::Engine as _;
    let target = if url.starts_with("https://www.bing.com/ck/a?") {
        let encoded = url
            .split_once('?')?
            .1
            .split('&')
            .find_map(|part| part.strip_prefix("u="))?
            .strip_prefix("a1")?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded.trim_end_matches('='))
            .ok()?;
        String::from_utf8(bytes).ok()?
    } else {
        url.to_owned()
    };
    if !(target.starts_with("https://") || target.starts_with("http://"))
        || target.starts_with("https://www.bing.com/")
        || target.starts_with("https://bing.com/")
        || target.starts_with("https://go.microsoft.com/fwlink/")
        || target.starts_with("https://aka.ms/3rdpartycookies")
    {
        return None;
    }
    Some(target)
}

/// Interpret the pinned chrome-web-mcp envelope only for its registered
/// server. A filesystem tool may legitimately read a file containing
/// {"success": false}; file contents never decide whether that read succeeded.
pub fn normalize_for_server(server: &str, result: &Value) -> (bool, Value) {
    let (mut ok, mut out) = normalize(result);
    if server == "chrome_web" {
        if result
            .get("structuredContent")
            .and_then(|v| v.get("success"))
            .and_then(Value::as_bool)
            == Some(false)
        {
            ok = false;
        }
        if let Some(content) = result["content"].as_array()
            && content.len() == 1
            && content[0]["type"] == "text"
            && let Some(text) = content[0]["text"].as_str()
            && let Ok(mut payload) = serde_json::from_str::<Value>(text)
        {
            if payload.get("success").and_then(Value::as_bool) == Some(false) {
                ok = false;
            }
            // The pinned server includes up to 200 links even for a bounded
            // markdown fetch. Markdown already carries inline links; avoid
            // re-expanding the prompt with an unbounded duplicate link index.
            // Explicit format=links responses have `text`, and keep their index.
            if let Some(data) = payload.get_mut("data").and_then(Value::as_object_mut)
                && let Some(markdown) = data.get("markdown").and_then(Value::as_str)
            {
                let bounded: String = markdown.chars().take(6000).collect();
                if bounded.len() < markdown.len() {
                    data.insert("markdown".into(), json!(bounded));
                    data.insert("truncated".into(), json!(true));
                    data.insert("resident_char_limit".into(), json!(6000));
                }
                if let Some(Value::Array(links)) = data.remove("links") {
                    // Some search pages lose every href during extraction.
                    // Retain a small set of follow-up targets absent from the
                    // markdown, rather than discarding the only source URLs.
                    let markdown = data["markdown"].as_str().unwrap_or("");
                    let is_bing = data
                        .get("final_url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| url.starts_with("https://www.bing.com/search?"));
                    let mut follow_up = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    let mut chars = 0;
                    for link in &links {
                        let Some(url) = link["url"].as_str() else {
                            continue;
                        };
                        let url = if is_bing {
                            let Some(target) = bing_follow_up_url(url) else {
                                continue;
                            };
                            target
                        } else {
                            url.to_owned()
                        };
                        if !(url.starts_with("https://") || url.starts_with("http://"))
                            || markdown.contains(&url)
                            || !seen.insert(url.clone())
                        {
                            continue;
                        }
                        let entry = json!({"url": url, "text": link["text"].as_str().unwrap_or("")
                            .chars().take(160).collect::<String>()});
                        let size = entry.to_string().chars().count();
                        if chars + size > 3000 {
                            continue;
                        }
                        chars += size;
                        follow_up.push(entry);
                        if follow_up.len() == 8 {
                            break;
                        }
                    }
                    data.insert(
                        "omitted_link_index_count".into(),
                        json!(links.len() - follow_up.len()),
                    );
                    if !follow_up.is_empty() {
                        data.insert("follow_up_links".into(), json!(follow_up));
                    }
                }
            }
            // Keep the known JSON envelope as JSON, rather than re-escaping
            // it inside a text string and cutting off its source metadata.
            out.as_object_mut()
                .expect("normalized object")
                .remove("text");
            out["structured"] = payload;
            // JSON escaping also consumes the persona's 12,000-char budget.
            // Shorten only markdown so source metadata stays valid JSON.
            if out.to_string().chars().count() > 11_000
                && let Some(markdown) = out
                    .pointer("/structured/data/markdown")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            {
                out["structured"]["data"]["truncated"] = json!(true);
                out["structured"]["data"]["resident_json_limit"] = json!(11_000);
                let mut boundaries: Vec<usize> = markdown.char_indices().map(|(i, _)| i).collect();
                boundaries.push(markdown.len());
                let (mut low, mut high) = (0, boundaries.len() - 1);
                while low < high {
                    let mid = (low + high).div_ceil(2);
                    out["structured"]["data"]["markdown"] = json!(&markdown[..boundaries[mid]]);
                    if out.to_string().chars().count() <= 11_000 {
                        low = mid;
                    } else {
                        high = mid - 1;
                    }
                }
                out["structured"]["data"]["markdown"] = json!(&markdown[..boundaries[low]]);
            }
        }
    }
    (ok, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposed_names_are_openai_safe() {
        assert_eq!(
            exposed_name("github", "get_file_contents"),
            "mcp__github__get_file_contents"
        );
        assert_eq!(exposed_name("a.b", "x/y"), "mcp__a_b__x_y");
        let long = exposed_name("playwright", &"t".repeat(80));
        assert!(long.len() <= 64);
        assert_ne!(long, exposed_name("playwright", &"t".repeat(81)));
    }

    #[test]
    fn normalizes_text_and_errors() {
        let (ok, out) = normalize(&json!({"content": [{"type": "text", "text": "a"},
            {"type": "image", "data": "xx", "mimeType": "image/png"}], "isError": false}));
        assert!(ok);
        assert_eq!(out["text"], "a");
        assert_eq!(out["non_text_content"][0]["type"], "image");
        let (ok, _) = normalize(&json!({"content": [], "isError": true}));
        assert!(!ok);
    }

    #[test]
    fn application_failure_in_json_text_is_not_successful_evidence() {
        let payload = json!({"success": false, "captcha_required": true, "error": "challenge"});
        let (ok, out) = normalize_for_server(
            "chrome_web",
            &json!({"content": [{"type": "text", "text": payload.to_string()}]}),
        );
        assert!(!ok);
        let file = json!({"content": [{"type": "text", "text": payload.to_string()}]});
        assert!(normalize_for_server("fs", &file).0);
        assert!(normalize_for_server("nas", &file).0);
        assert_eq!(out["structured"], payload);
        let (ok, _) = normalize_for_server(
            "chrome_web",
            &json!({"structuredContent": {"success": false}, "content": []}),
        );
        assert!(!ok);
        let payload = json!({"success": true, "data": {"web": [{"url": "https://example.org"}]}});
        let (ok, out) = normalize_for_server(
            "chrome_web",
            &json!({"content": [{"type": "text", "text": payload.to_string()}]}),
        );
        assert!(ok);
        assert_eq!(
            out["structured"]["data"]["web"][0]["url"],
            "https://example.org"
        );
    }

    #[test]
    fn chrome_markdown_keeps_sources_without_duplicate_link_index() {
        let url = "https://doc.rust-lang.org/";
        let payload = json!({"success": true, "data": {
            "final_url": url, "markdown": format!("[Rust documentation]({url})"),
            "links": vec![json!({"url": url}); 200]
        }});
        let raw = json!({"content": [{"type": "text", "text": payload.to_string()}]});
        let (ok, out) = normalize_for_server("chrome_web", &raw);
        assert!(ok);
        let parsed = &out["structured"];
        assert_eq!(parsed["data"]["final_url"], url);
        assert!(parsed["data"]["markdown"].as_str().unwrap().contains(url));
        assert_eq!(parsed["data"]["omitted_link_index_count"], 200);
        assert!(parsed["data"].get("links").is_none());
        assert_eq!(
            normalize_for_server("fs", &raw).1["text"],
            payload.to_string()
        );
        let payload =
            json!({"success": true, "data": {"text": "links mode", "links": [{"url": url}]}});
        let (_, out) = normalize_for_server(
            "chrome_web",
            &json!({"content": [{"type": "text", "text": payload.to_string()}]}),
        );
        assert_eq!(out["structured"], payload);
    }

    #[test]
    fn chrome_long_markdown_is_bounded_with_source_metadata_intact() {
        let url = "https://doc.rust-lang.org/book/";
        let raw = json!({"content": [{"type": "text", "text": json!({
            "success": true, "data": {"markdown": "あ".repeat(12000), "final_url": url,
            "title": "The Rust Book", "truncated": false}
        }).to_string()}]});
        let (ok, out) = normalize_for_server("chrome_web", &raw);
        assert!(ok);
        let data = &out["structured"]["data"];
        assert_eq!(data["markdown"].as_str().unwrap().chars().count(), 6000);
        assert_eq!(data["final_url"], url);
        assert_eq!(data["title"], "The Rust Book");
        assert_eq!(data["truncated"], true);
        assert!(out.get("text").is_none());
        assert!(out.to_string().chars().count() < 12000);
    }

    #[test]
    fn chrome_markdown_budget_accounts_for_json_escaping() {
        let url = "https://example.org/docs";
        let raw = json!({"content": [{"type": "text", "text": json!({
            "success": true, "data": {"markdown": "\"\n\\\u{0001}あ".repeat(1200),
            "final_url": url, "title": "A quoted document", "truncated": false}
        }).to_string()}]});
        let (ok, out) = normalize_for_server("chrome_web", &raw);
        assert!(ok);
        assert!(out.to_string().chars().count() <= 11000);
        let data = &out["structured"]["data"];
        assert_eq!(data["final_url"], url);
        assert_eq!(data["title"], "A quoted document");
        assert_eq!(data["truncated"], true);
        assert_eq!(out["structured"]["success"], true);
        assert!(!data["markdown"].as_str().unwrap().is_empty());
    }

    #[test]
    fn chrome_search_without_inline_urls_keeps_bounded_follow_up_targets() {
        use base64::Engine as _;
        let mut links = vec![json!({"url": "https://www.bing.com/images", "text": "Images"})];
        links.extend((0..100).map(|i| json!({
            "url": format!("https://www.bing.com/ck/a?u=a1{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("https://example.org/result{i}"))), "text": format!("Result {i}")
        })));
        let raw = json!({"content": [{"type": "text", "text": json!({
            "success": true, "data": {"markdown": "Search snippets without links",
            "final_url": "https://www.bing.com/search?q=test", "links": links}
        }).to_string()}]});
        let (ok, out) = normalize_for_server("chrome_web", &raw);
        assert!(ok);
        let data = &out["structured"]["data"];
        assert_eq!(data["follow_up_links"].as_array().unwrap().len(), 8);
        assert_eq!(
            data["follow_up_links"][0]["url"],
            "https://example.org/result0"
        );
        assert_eq!(data["omitted_link_index_count"], 93);
        assert!(out.to_string().chars().count() < 11000);
        for target in [
            "/images/search?q=test",
            "https://aka.ms/3rdpartycookies",
            "https://go.microsoft.com/fwlink/?linkid=521839",
        ] {
            let url = format!(
                "https://www.bing.com/ck/a?u=a1{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(target)
            );
            assert_eq!(bing_follow_up_url(&url), None);
        }
        assert_eq!(
            bing_follow_up_url("https://www.bing.com/ck/a?u=a1!invalid"),
            None
        );
    }

    /// A tiny MCP server in Python: initialize, tools/list, tools/call(echo).
    #[test]
    fn talks_to_a_stdio_server() {
        let script = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": True}},
                       {"name": "write", "description": "Write", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": json.dumps(m["params"]["arguments"])}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let config = McpServerConfig {
            name: "t".to_owned(),
            command: "python3".to_owned(),
            args: vec!["-c".to_owned(), script.to_owned()],
            env: HashMap::new(),
            cwd: None,
            read_only: true,
            allow: Vec::new(),
            deny: Vec::new(),
            call_timeout_secs: 10,
            description: Some("Use the documented public search fallback.".to_owned()),
            approval_required: Vec::new(),
            after_approved: None,
        };
        let server = McpServer::new(config, dir.path().to_path_buf());
        server.ensure().expect("start");
        let tools = server.tools();
        assert_eq!(tools.len(), 1, "read_only hides the unannotated tool");
        assert_eq!(tools[0].exposed, "mcp__t__echo");
        assert!(
            tools[0].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("Use the documented public search fallback.")
        );
        let result = server.call("echo", &json!({"x": 1})).expect("call");
        let (ok, out) = normalize(&result);
        assert!(ok);
        assert_eq!(out["text"], r#"{"x": 1}"#);
    }
}
