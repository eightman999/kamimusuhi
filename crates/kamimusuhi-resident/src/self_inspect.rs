//! Read-only self-inspection tool enabled when `tools.access_mode == "full"`.
//!
//! Allows the individual to inspect its own runtime configuration
//! (`resident.json`, `runtime.json`, `identity-manifest.json`), `current_state/`,
//! logs (`trace.jsonl`, routing/dialogue journals), and mounted NAS storage —
//! while strictly refusing credential/secret files and any path outside the
//! node's own roots.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::AccessMode;
use crate::state::Shared;

const DEFAULT_READ_BYTES: usize = 16 * 1024;
const MAX_READ_BYTES: usize = 64 * 1024;
const DEFAULT_TAIL_LINES: usize = 40;
const MAX_TAIL_LINES: usize = 200;
const MAX_LIST_ENTRIES: usize = 200;
const DEFAULT_SEARCH_HITS: usize = 15;
const MAX_SEARCH_HITS: usize = 30;
const MAX_SEARCH_FILES: usize = 400;
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub fn definitions() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "self_inspect",
            "description": "Read-only inspection of this Kamimusuhi node's own configuration (resident.json, runtime.json, identity-manifest.json), current_state, individual files, logs (trace.jsonl, routing/dialogue logs) and mounted NAS. Only available when tools.access_mode is full. Secret/credential files are always excluded.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["overview", "list", "read", "tail", "search"],
                        "description": "overview: show available roots and paths; list: list a directory; read: read text slice; tail: read last N lines; search: substring search"
                    },
                    "root": {
                        "type": "string",
                        "enum": ["node_root", "config", "individual", "current_state", "logs", "nas"],
                        "description": "Named root to inspect (optional when path is an absolute path inside one of these roots)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Relative path within root, or an absolute path inside an allowed self root (e.g. /srv/kamimusuhi/config/resident.json)"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Byte offset for read"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 65536,
                        "description": "Max bytes for read (default 16384, max 65536), max lines for tail (default 40, max 200), or max hits for search (default 15, max 30)"
                    },
                    "q": {
                        "type": "string",
                        "minLength": 2,
                        "description": "Substring query for search"
                    }
                },
                "required": ["action"]
            }
        }
    })]
}

pub fn call(shared: &Shared, args: &Value) -> (u16, Value) {
    if shared.config.tools.access_mode != AccessMode::Full {
        return fail("self_inspect requires tools.access_mode = \"full\"");
    }
    let action = args["action"].as_str().unwrap_or("");
    let roots = inspectable_roots(shared);
    match action {
        "overview" => ok(overview(shared, &roots)),
        "list" => match resolve_target(&roots, args, true) {
            Ok((root_name, path)) => match list_dir(&root_name, &path) {
                Ok(v) => ok(v),
                Err(e) => fail(&e),
            },
            Err(e) => fail(&e),
        },
        "read" => match resolve_target(&roots, args, false) {
            Ok((root_name, path)) => {
                let offset = args["offset"].as_u64().unwrap_or(0);
                let limit = args["limit"]
                    .as_u64()
                    .map_or(DEFAULT_READ_BYTES, |n| (n as usize).clamp(1, MAX_READ_BYTES));
                match read_slice(&root_name, &path, offset, limit) {
                    Ok(v) => ok(v),
                    Err(e) => fail(&e),
                }
            }
            Err(e) => fail(&e),
        },
        "tail" => match resolve_target(&roots, args, false) {
            Ok((root_name, path)) => {
                let lines = args["limit"]
                    .as_u64()
                    .map_or(DEFAULT_TAIL_LINES, |n| (n as usize).clamp(1, MAX_TAIL_LINES));
                match tail_file(&root_name, &path, lines) {
                    Ok(v) => ok(v),
                    Err(e) => fail(&e),
                }
            }
            Err(e) => fail(&e),
        },
        "search" => {
            let Some(query) = args["q"].as_str().filter(|s| s.len() >= 2) else {
                return fail("search requires q (at least 2 characters)");
            };
            let limit = args["limit"].as_u64().map_or(DEFAULT_SEARCH_HITS, |n| {
                (n as usize).clamp(1, MAX_SEARCH_HITS)
            });
            match resolve_target(&roots, args, true) {
                Ok((root_name, path)) => match search_path(&root_name, &path, query, limit) {
                    Ok(v) => ok(v),
                    Err(e) => fail(&e),
                },
                Err(e) => fail(&e),
            }
        }
        _ => fail("action must be one of: overview, list, read, tail, search"),
    }
}

fn ok(result: Value) -> (u16, Value) {
    (200, json!({"ok": true, "result": result}))
}

fn fail(message: &str) -> (u16, Value) {
    (200, json!({"ok": false, "error": message}))
}

fn inspectable_roots(shared: &Shared) -> BTreeMap<&'static str, PathBuf> {
    let mut map = BTreeMap::new();
    let root = &shared.config.paths.root;
    if let Ok(canon) = fs::canonicalize(root) {
        map.insert("node_root", canon);
    }
    for (name, candidate) in [
        ("config", root.join("config")),
        ("current_state", shared.config.paths.current_state()),
        ("logs", root.join("logs")),
    ] {
        if let Ok(canon) = fs::canonicalize(&candidate) {
            map.insert(name, canon);
        }
    }
    if let Some(dialogue) = &shared.config.dialogue
        && let Ok(canon) = fs::canonicalize(&dialogue.dir)
    {
        map.insert("individual", canon);
    }
    if let Some(nas) = &shared.config.nas
        && nas.root.join(&nas.marker).exists()
        && let Ok(canon) = fs::canonicalize(&nas.root)
    {
        map.insert("nas", canon);
    }
    map
}

fn overview(shared: &Shared, roots: &BTreeMap<&'static str, PathBuf>) -> Value {
    let roots_json: BTreeMap<String, String> = roots
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.display().to_string()))
        .collect();
    json!({
        "access_mode": shared.config.tools.access_mode.as_str(),
        "node_id": shared.config.node.id,
        "role": shared.config.node.role.as_str(),
        "roots": roots_json,
        "secret_policy": "credential and secret files (secrets.env, *.env, *token*, *secret*, *.key, *.pem, auth.json, etc.) are always refused",
    })
}

fn is_secret_component(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".env")
        || lower == "auth.json"
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
    {
        return true;
    }
    ["secret", "token", "credential", "passwd", "password"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn contains_secret_path(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(is_secret_component)
    })
}

fn is_binary_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "sqlite" | "sqlite-wal" | "sqlite-shm" | "db" | "png" | "jpg" | "jpeg" | "gif" | "webp"
            | "pdf" | "zip" | "gz" | "tar" | "zst" | "wasm" | "so" | "dylib" | "o"
    )
}

fn resolve_target(
    roots: &BTreeMap<&'static str, PathBuf>,
    args: &Value,
    allow_empty_path: bool,
) -> Result<(String, PathBuf), String> {
    let raw_path = args["path"].as_str().unwrap_or("").trim();
    let root_arg = args["root"].as_str().map(str::trim).filter(|s| !s.is_empty());

    let (root_name, root_dir, candidate) = if !raw_path.is_empty() && Path::new(raw_path).is_absolute()
    {
        let canon = fs::canonicalize(raw_path)
            .map_err(|_| format!("path not found or unreadable: {raw_path}"))?;
        let matched = roots
            .iter()
            .filter(|(_, r)| canon.starts_with(r))
            .max_by_key(|(_, r)| r.components().count())
            .ok_or_else(|| format!("path is outside allowed self-inspection roots: {raw_path}"))?;
        ((*matched.0).to_owned(), matched.1.clone(), canon)
    } else {
        let rname = root_arg.unwrap_or("node_root");
        let root_dir = roots
            .get(rname)
            .ok_or_else(|| format!("root {rname:?} is not available on this node"))?
            .clone();
        if raw_path.is_empty() {
            if !allow_empty_path {
                return Err("path is required".to_owned());
            }
            (rname.to_owned(), root_dir.clone(), root_dir)
        } else {
            let joined = root_dir.join(raw_path);
            let canon = fs::canonicalize(&joined)
                .map_err(|_| format!("path not found or unreadable: {raw_path}"))?;
            if !canon.starts_with(&root_dir) {
                return Err("path escapes allowed root".to_owned());
            }
            (rname.to_owned(), root_dir, canon)
        }
    };

    if contains_secret_path(&candidate, &root_dir) {
        return Err("access to secret or credential files is refused".to_owned());
    }
    Ok((root_name, candidate))
}

fn list_dir(root_name: &str, dir: &Path) -> Result<Value, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(dir).map_err(|e| format!("failed to read directory: {e}"))?;
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_secret_component(&name) {
            continue;
        }
        let meta = entry.metadata().ok();
        let kind = match meta.as_ref() {
            Some(m) if m.is_dir() => "dir",
            Some(m) if m.is_file() => "file",
            _ => "other",
        };
        let bytes = meta.as_ref().map_or(0, fs::Metadata::len);
        entries.push((name, kind, bytes));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let truncated = entries.len() > MAX_LIST_ENTRIES;
    entries.truncate(MAX_LIST_ENTRIES);
    let items: Vec<Value> = entries
        .into_iter()
        .map(|(name, kind, bytes)| json!({"name": name, "kind": kind, "bytes": bytes}))
        .collect();
    Ok(json!({
        "root": root_name,
        "path": dir.display().to_string(),
        "entries": items,
        "truncated": truncated,
    }))
}

fn read_slice(root_name: &str, path: &Path, offset: u64, limit: usize) -> Result<Value, String> {
    if !path.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if is_binary_extension(path) {
        return Err("binary file cannot be read as text".to_owned());
    }
    let mut file = File::open(path).map_err(|e| format!("failed to open file: {e}"))?;
    let total_bytes = file.metadata().map_err(|e| e.to_string())?.len();
    let start = offset.min(total_bytes);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek failed: {e}"))?;
    let mut buf = vec![0u8; limit];
    let n = file.read(&mut buf).map_err(|e| format!("read failed: {e}"))?;
    buf.truncate(n);
    if buf.contains(&0) {
        return Err("binary content detected; refusing text read".to_owned());
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let next_offset = start + n as u64;
    Ok(json!({
        "root": root_name,
        "path": path.display().to_string(),
        "offset": start,
        "bytes_read": n,
        "total_bytes": total_bytes,
        "eof": next_offset >= total_bytes,
        "content": text,
    }))
}

fn tail_file(root_name: &str, path: &Path, max_lines: usize) -> Result<Value, String> {
    if !path.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if is_binary_extension(path) {
        return Err("binary file cannot be tailed as text".to_owned());
    }
    let mut file = File::open(path).map_err(|e| format!("failed to open file: {e}"))?;
    let total_bytes = file.metadata().map_err(|e| e.to_string())?.len();
    let window = (MAX_READ_BYTES as u64).min(total_bytes);
    let start = total_bytes.saturating_sub(window);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek failed: {e}"))?;
    let mut buf = Vec::with_capacity(window as usize);
    file.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if buf.contains(&0) {
        return Err("binary content detected; refusing text tail".to_owned());
    }
    let text = String::from_utf8_lossy(&buf);
    let all_lines: Vec<&str> = text.lines().collect();
    let keep = all_lines.len().min(max_lines);
    let tail = all_lines[all_lines.len() - keep..].join("\n");
    Ok(json!({
        "root": root_name,
        "path": path.display().to_string(),
        "total_bytes": total_bytes,
        "lines_returned": keep,
        "content": tail,
    }))
}

fn search_path(root_name: &str, target: &Path, query: &str, limit: usize) -> Result<Value, String> {
    let mut hits = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = false;
    let mut stack = vec![target.to_path_buf()];
    while let Some(current) = stack.pop() {
        if hits.len() >= limit || files_scanned >= MAX_SEARCH_FILES {
            truncated = true;
            break;
        }
        if current.is_dir() {
            if let Ok(rd) = fs::read_dir(&current) {
                let mut children: Vec<PathBuf> = rd
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        !is_secret_component(&name) && name != "bin" && name != "backups"
                    })
                    .map(|e| e.path())
                    .collect();
                children.sort();
                for child in children.into_iter().rev() {
                    stack.push(child);
                }
            }
            continue;
        }
        if !current.is_file() || is_binary_extension(&current) {
            continue;
        }
        let Ok(meta) = fs::metadata(&current) else {
            continue;
        };
        if meta.len() > MAX_SEARCH_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&current) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        files_scanned += 1;
        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.lines().enumerate() {
            if line.contains(query) {
                let snippet: String = line.chars().take(240).collect();
                hits.push(json!({
                    "path": current.display().to_string(),
                    "line": idx + 1,
                    "snippet": snippet,
                }));
                if hits.len() >= limit {
                    truncated = true;
                    break;
                }
            }
        }
    }
    Ok(json!({
        "root": root_name,
        "path": target.display().to_string(),
        "query": query,
        "files_scanned": files_scanned,
        "hits": hits,
        "truncated": truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DialogueConfig, NodeConfig, NodeRole, PathsConfig};
    use crate::spool::Spool;

    fn test_shared(root: &Path, access_mode: AccessMode) -> Shared {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("current_state")).unwrap();
        fs::create_dir_all(root.join("runtime/individual")).unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        let mut config = Config {
            node: NodeConfig {
                id: "test-node".to_owned(),
                role: NodeRole::Continuity,
                listen: "127.0.0.1:0".to_owned(),
                token_env: String::new(),
                capabilities: Vec::new(),
            },
            paths: PathsConfig {
                root: root.to_path_buf(),
                spool_warn_bytes: 1024 * 1024,
            },
            nas: None,
            peers: Vec::new(),
            tiers: Vec::new(),
            routing: Default::default(),
            hai_account: None,
            kcore: None,
            snapshot: None,
            dialogue: Some(DialogueConfig {
                runtime_binary: PathBuf::from("/bin/true"),
                dir: root.join("runtime/individual"),
                privacy: "unconstrained".to_owned(),
                timeout_secs: 30,
                max_concurrent: 1,
                session_idle_secs: 60,
                host_max: 2,
            }),
            cognition: None,
            memory_stall: Default::default(),
            libraries: Vec::new(),
            mcp_servers: Vec::new(),
            jobs: Vec::new(),
            intervals: Default::default(),
            task_plane: None,
            commitments: Default::default(),
            intercom: Default::default(),
            tool_judge: Default::default(),
            tools: Default::default(),
            self_improvement: None,
            delegation: Default::default(),
            social: None,
        };
        config.tools.access_mode = access_mode;
        let spool = Spool::new(config.paths.spool(), "test-node").unwrap();
        Shared::new(config, spool, 1, None)
    }

    #[test]
    fn standard_mode_refuses_self_inspect_and_omits_definition() {
        let dir = tempfile::tempdir().unwrap();
        let shared = test_shared(dir.path(), AccessMode::Standard);
        let listed = crate::tools::list_with(&shared, true);
        let has_tool = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "self_inspect");
        assert!(!has_tool);
        let (status, reply) = call(&shared, &json!({"action": "overview"}));
        assert_eq!(status, 200);
        assert_eq!(reply["ok"], false);
    }

    #[test]
    fn full_mode_reads_configs_and_refuses_secrets_and_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.txt"), "nope").unwrap();
        let shared = test_shared(dir.path(), AccessMode::Full);
        fs::write(
            dir.path().join("config/resident.json"),
            r#"{"tiers":[{"name":"hai_qwen27b"}]}"#,
        )
        .unwrap();
        fs::write(dir.path().join("config/secrets.env"), "API_KEY=supersecret").unwrap();
        fs::write(dir.path().join("config/node_token"), "tok-123").unwrap();
        fs::write(
            dir.path().join("runtime/individual/trace.jsonl"),
            "line1\nline2\nline3\n",
        )
        .unwrap();

        let listed = crate::tools::list_with(&shared, true);
        let has_tool = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "self_inspect");
        assert!(has_tool);

        // Overview + read resident.json by absolute path
        let (_, ov) = call(&shared, &json!({"action": "overview"}));
        assert_eq!(ov["ok"], true);
        assert_eq!(ov["result"]["access_mode"], "full");

        let res_path = dir.path().join("config/resident.json");
        let (_, rd) = call(
            &shared,
            &json!({"action": "read", "path": res_path.display().to_string()}),
        );
        assert_eq!(rd["ok"], true);
        assert!(rd["result"]["content"].as_str().unwrap().contains("hai_qwen27b"));

        // Tail trace.jsonl
        let (_, tl) = call(
            &shared,
            &json!({"action": "tail", "root": "individual", "path": "trace.jsonl", "limit": 2}),
        );
        assert_eq!(tl["ok"], true);
        assert_eq!(tl["result"]["content"], "line2\nline3");

        // List hides secrets.env and node_token
        let (_, ls) = call(&shared, &json!({"action": "list", "root": "config"}));
        let names: Vec<&str> = ls["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["resident.json"]);

        // Direct read of secrets.env, node_token, and outside path must fail
        let (_, sec) = call(
            &shared,
            &json!({"action": "read", "root": "config", "path": "secrets.env"}),
        );
        assert_eq!(sec["ok"], false);
        let (_, tok) = call(
            &shared,
            &json!({"action": "read", "root": "config", "path": "node_token"}),
        );
        assert_eq!(tok["ok"], false);
        let (_, esc) = call(
            &shared,
            &json!({"action": "read", "path": outside.path().join("outside.txt").display().to_string()}),
        );
        assert_eq!(esc["ok"], false);
    }
}
