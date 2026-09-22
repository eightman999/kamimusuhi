//! Read-only reference libraries (external repositories, datasets) that
//! Kamimusuhi may consult.
//!
//! A library is a directory on the node that holds it (e.g. a git clone on
//! llm_master's HDD). The resident serves it read-only:
//!
//! `POST /v1/library` with a JSON body:
//!
//! * `{"action":"list"}` — libraries on this node and, via peers, elsewhere
//! * `{"action":"tree","library":"<name>","path":"<dir>"}` — one listing
//! * `{"action":"file","library":"<name>","path":"<file>","offset":0,"limit":262144}`
//! * `{"action":"search","library":"<name>","q":"<text>","path":"<dir>","limit":100}`
//! * `{"action":"json_get","library":"<name>","path":"<file>","pointer":"/a/b"}`
//! * `{"action":"json_find","library":"<name>","path":"<file>","pointer":"/items",
//!   "match":[{"field":"/k","op":"eq|contains","value":"x"}],"mode":"any|all","limit":20}`
//!
//! A request for a library this node does not hold is forwarded to the peer
//! that does, so the Pi can reference data kept on llm_master. Paths are
//! resolved inside the library root only; `.git` is never served.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::client::auth;
use crate::config::LibraryConfig;
use crate::state::Shared;

const MAX_SLICE: u64 = 2 * 1024 * 1024;
const DEFAULT_SLICE: u64 = 256 * 1024;
const MAX_HITS: usize = 100;
const MAX_SEARCH_FILE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 2000;

/// Resolve `relative` strictly inside `root` (no `..`, no absolute paths,
/// no `.git`, no symlink escape).
fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative.trim_start_matches('/'));
    for component in rel.components() {
        match component {
            Component::Normal(part) if part != ".git" => {}
            Component::CurDir => {}
            _ => return Err("path must stay inside the library".to_owned()),
        }
    }
    let joined = root.join(rel);
    let canonical = joined
        .canonicalize()
        .map_err(|_| "no such path in library".to_owned())?;
    let root = root
        .canonicalize()
        .map_err(|_| "library root is unavailable".to_owned())?;
    if !canonical.starts_with(&root) {
        return Err("path escapes the library".to_owned());
    }
    Ok(canonical)
}

fn tree(lib: &LibraryConfig, path: &str) -> Result<Value, String> {
    let dir = resolve(&lib.path, path)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        entries.push(json!({
            "name": name,
            "type": if meta.is_dir() { "dir" } else { "file" },
            "bytes": meta.is_file().then_some(meta.len()),
        }));
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({"library": lib.name, "path": path, "entries": entries}))
}

fn file(lib: &LibraryConfig, path: &str, offset: u64, limit: u64) -> Result<Value, String> {
    let target = resolve(&lib.path, path)?;
    let mut handle = fs::File::open(&target).map_err(|e| e.to_string())?;
    let size = handle.metadata().map_err(|e| e.to_string())?.len();
    let limit = limit.clamp(1, MAX_SLICE);
    handle
        .seek(SeekFrom::Start(offset.min(size)))
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    handle
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let end = offset.min(size) + buf.len() as u64;
    Ok(json!({
        "library": lib.name,
        "path": path,
        "size": size,
        "offset": offset.min(size),
        "next_offset": (end < size).then_some(end),
        "binary": buf.contains(&0),
        "text": String::from_utf8_lossy(&buf),
    }))
}

fn search(lib: &LibraryConfig, needle: &str, path: &str, limit: u64) -> Result<Value, String> {
    if needle.chars().count() < 2 {
        return Err("q must be at least 2 characters".to_owned());
    }
    let start = resolve(&lib.path, path)?;
    let root = lib.path.canonicalize().map_err(|e| e.to_string())?;
    let limit = usize::try_from(limit.clamp(1, MAX_HITS as u64)).unwrap_or(MAX_HITS);
    let mut hits = Vec::new();
    let mut stack = vec![start];
    let mut scanned = 0u64;
    let mut truncated = false;
    'search: while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if meta.len() > MAX_SEARCH_FILE {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            scanned += 1;
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            // Every occurrence, not just the first per line: data files are
            // often a single minified line.
            for (lineno, line) in text.lines().enumerate() {
                for (pos, _) in line.match_indices(needle) {
                    // Only report truncation when a real additional hit exists.
                    if hits.len() == limit {
                        truncated = true;
                        break 'search;
                    }
                    let from = line[..pos]
                        .char_indices()
                        .rev()
                        .nth(80)
                        .map_or(0, |(i, _)| i);
                    let snippet: String = line[from..].chars().take(240).collect();
                    hits.push(
                        json!({"path": rel, "line": lineno + 1, "byte": pos, "snippet": snippet}),
                    );
                }
            }
        }
    }
    Ok(
        json!({"library": lib.name, "q": needle, "files_scanned": scanned,
              "truncated": truncated, "hits": hits}),
    )
}

const MAX_JSON_FILE: u64 = 128 * 1024 * 1024;
const DEFAULT_JSON_BYTES: usize = 20_000;
const MAX_JSON_BYTES: usize = 200_000;

fn load_json(lib: &LibraryConfig, path: &str) -> Result<Value, String> {
    let target = resolve(&lib.path, path)?;
    let size = fs::metadata(&target).map_err(|e| e.to_string())?.len();
    if size > MAX_JSON_FILE {
        return Err(format!("{path} is too large for JSON tools ({size} bytes)"));
    }
    let bytes = fs::read(&target).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{path} is not JSON: {e}"))
}

/// Shrink a value to at most `max` serialized bytes, keeping its shape.
fn bounded(value: &Value, max: usize) -> (Value, bool) {
    let text = value.to_string();
    if text.len() <= max {
        return (value.clone(), false);
    }
    let summary = match value {
        Value::Object(map) => json!({
            "type": "object",
            "key_count": map.len(),
            "keys": map.keys().take(200).collect::<Vec<_>>(),
        }),
        Value::Array(items) => {
            let mut head = Vec::new();
            let mut used = 0;
            for item in items {
                used += item.to_string().len();
                if used > max / 2 {
                    break;
                }
                head.push(item.clone());
            }
            json!({"type": "array", "length": items.len(), "head": head})
        }
        Value::String(s) => Value::String(s.chars().take(max / 4).collect()),
        other => other.clone(),
    };
    (summary, true)
}

fn json_get(lib: &LibraryConfig, path: &str, pointer: &str, max: usize) -> Result<Value, String> {
    let doc = load_json(lib, path)?;
    let value = doc
        .pointer(pointer)
        .ok_or_else(|| format!("no value at {pointer:?} in {path}"))?;
    let (value, truncated) = bounded(value, max.clamp(1000, MAX_JSON_BYTES));
    Ok(
        json!({"library": lib.name, "path": path, "pointer": pointer,
              "truncated": truncated, "value": value}),
    )
}

/// One condition: `field` is a JSON pointer inside each element (`_key` is
/// the element's key when the collection is an object).
fn matches(key: Option<&str>, item: &Value, cond: &Value) -> bool {
    let field = cond["field"].as_str().unwrap_or("");
    let want = match &cond["value"] {
        Value::String(s) => s.clone(),
        Value::Null => return false,
        other => other.to_string(),
    };
    let got = if field == "_key" {
        key.map(str::to_owned)
    } else {
        item.pointer(field).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    };
    let Some(got) = got else { return false };
    match cond["op"].as_str().unwrap_or("eq") {
        "contains" => got.contains(&want),
        _ => got == want,
    }
}

fn json_find(lib: &LibraryConfig, request: &Value) -> Result<Value, String> {
    let path = request["path"].as_str().unwrap_or("");
    let pointer = request["pointer"].as_str().unwrap_or("");
    let conditions = request["match"]
        .as_array()
        .filter(|c| !c.is_empty())
        .ok_or("match must be a non-empty array of {field, op, value}")?;
    let all = request["mode"].as_str() == Some("all");
    let limit = usize::try_from(request["limit"].as_u64().unwrap_or(20))
        .unwrap_or(20)
        .clamp(1, 200);
    let doc = load_json(lib, path)?;
    let collection = doc
        .pointer(pointer)
        .ok_or_else(|| format!("no collection at {pointer:?} in {path}"))?;
    let elements: Vec<(Option<&str>, &Value)> = match collection {
        Value::Array(items) => items.iter().map(|v| (None, v)).collect(),
        Value::Object(map) => map.iter().map(|(k, v)| (Some(k.as_str()), v)).collect(),
        _ => return Err("pointer must name an array or object".to_owned()),
    };
    let mut found = Vec::new();
    let mut total = 0usize;
    for (key, item) in elements {
        let hit = if all {
            conditions.iter().all(|c| matches(key, item, c))
        } else {
            conditions.iter().any(|c| matches(key, item, c))
        };
        if hit {
            total += 1;
            if found.len() < limit {
                let (value, _) = bounded(item, 8000);
                found.push(match key {
                    Some(k) => json!({"_key": k, "value": value}),
                    None => value,
                });
            }
        }
    }
    Ok(
        json!({"library": lib.name, "path": path, "pointer": pointer,
              "total_matches": total, "returned": found.len(), "items": found}),
    )
}

fn describe(lib: &LibraryConfig, node: &str) -> Value {
    json!({
        "name": lib.name,
        "node": node,
        "description": lib.description,
        "source": lib.source,
        "available": lib.path.is_dir(),
    })
}

fn post(url: &str, body: &Value, token: Option<&str>) -> Result<(u16, Value), String> {
    use kamimusuhi_resource_http::{Endpoint, TrustAnchors, http};
    let endpoint = Endpoint::parse(url, "/v1/library")?;
    let response = http::post_json(
        &endpoint,
        &body.to_string(),
        &auth(token),
        Duration::from_secs(60),
        &TrustAnchors::Webpki,
    )
    .map_err(|e| crate::probes::describe(&e))?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    Ok((response.status, value))
}

/// Handle `POST /v1/library`.
pub fn handle(shared: &Shared, request: &Value) -> (u16, Value) {
    let local = &shared.config.libraries;
    let node = &shared.config.node.id;
    let token = shared.token.as_deref();
    // A forwarded request is answered from this node only (one hop).
    let local_only = request["local_only"].as_bool().unwrap_or(false);
    let action = request["action"].as_str().unwrap_or("list");
    let text = |key: &str| request[key].as_str().unwrap_or("").to_owned();

    if action == "list" {
        let mut all: Vec<Value> = local.iter().map(|l| describe(l, node)).collect();
        if !local_only {
            let ask = json!({"action": "list", "local_only": true});
            for peer in &shared.config.peers {
                if let Ok((200, v)) = post(&peer.url, &ask, token)
                    && let Some(list) = v["libraries"].as_array()
                {
                    all.extend(list.iter().cloned());
                }
            }
        }
        return (200, json!({"libraries": all}));
    }

    let name = text("library");
    let Some(lib) = local.iter().find(|l| l.name == name) else {
        if !local_only {
            let mut forwarded = request.clone();
            forwarded["local_only"] = Value::Bool(true);
            for peer in &shared.config.peers {
                // The holder's answer (including its errors) is the answer.
                if let Ok((status, v)) = post(&peer.url, &forwarded, token)
                    && status != 404
                {
                    return (status, v);
                }
            }
        }
        return (404, json!({"error": format!("library {name:?} not found")}));
    };
    let sub = text("path");
    let result = match action {
        "tree" => tree(lib, &sub),
        "file" => file(
            lib,
            &sub,
            request["offset"].as_u64().unwrap_or(0),
            request["limit"].as_u64().unwrap_or(DEFAULT_SLICE),
        ),
        "search" => search(
            lib,
            &text("q"),
            &sub,
            request["limit"].as_u64().unwrap_or(MAX_HITS as u64),
        ),
        "json_get" => json_get(
            lib,
            &sub,
            &text("pointer"),
            usize::try_from(request["max_bytes"].as_u64().unwrap_or(0))
                .ok()
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_JSON_BYTES),
        ),
        "json_find" => json_find(lib, request),
        _ => Err("action must be list, tree, file, search, json_get or json_find".to_owned()),
    };
    match result {
        Ok(v) => (200, v),
        Err(e) => (400, json!({"error": e})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(dir: &Path) -> LibraryConfig {
        LibraryConfig {
            name: "demo".to_owned(),
            path: dir.to_path_buf(),
            description: None,
            source: None,
        }
    }

    #[test]
    fn serves_inside_root_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
        fs::create_dir_all(dir.path().join(".git")).expect("mkdir");
        fs::write(dir.path().join("sub/a.txt"), "トヨタ自動車 7203\nother\n").expect("write");
        let l = lib(dir.path());

        let t = tree(&l, "").expect("tree");
        let names: Vec<&str> = t["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert_eq!(names, ["sub"], ".git hidden");

        let f = file(&l, "sub/a.txt", 0, 10).expect("file");
        assert_eq!(f["size"], "トヨタ自動車 7203\nother\n".len());
        assert_eq!(f["next_offset"], 10);

        let s = search(&l, "7203", "", MAX_HITS as u64).expect("search");
        assert_eq!(s["hits"][0]["path"], "sub/a.txt");
        assert_eq!(s["hits"][0]["line"], 1);

        assert!(file(&l, "../etc/passwd", 0, 10).is_err());
        assert!(
            file(&l, "/etc/passwd", 0, 10).is_err(),
            "absolute is re-rooted, then missing"
        );
        assert!(tree(&l, ".git").is_err());
    }

    #[test]
    fn search_marks_truncated_only_when_more_hits_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.json");
        let l = lib(dir.path());

        // Multiple matches on one minified line must each count toward the cap.
        fs::write(&path, r#"["トヨタ", "トヨタ"]"#).expect("write");
        for limit in [2, 8] {
            let found = search(&l, "トヨタ", "", limit).expect("search");
            assert_eq!(found["hits"].as_array().expect("hits").len(), 2);
            assert_eq!(found["truncated"], false);
            assert_eq!(found["hits"][0]["path"], "data.json");
            assert_eq!(found["hits"][0]["line"], 1);
            assert!(
                found["hits"][0]["snippet"]
                    .as_str()
                    .expect("snippet")
                    .contains("トヨタ")
            );
            let decoded: Value = serde_json::from_str(&found.to_string()).expect("valid JSON");
            assert_eq!(decoded, found);
        }

        let found = search(&l, "トヨタ", "", 1).expect("search");
        assert_eq!(found["hits"].as_array().expect("hits").len(), 1);
        assert_eq!(found["truncated"], true);

        // An additional match in a different directory also signals truncation.
        fs::create_dir(dir.path().join("sub")).expect("mkdir");
        fs::write(dir.path().join("sub/more.txt"), "トヨタ").expect("write");
        let found = search(&l, "トヨタ", "", 2).expect("search");
        assert_eq!(found["hits"].as_array().expect("hits").len(), 2);
        assert_eq!(found["truncated"], true);

        let found = search(&l, "該当なし", "", 1).expect("search");
        assert!(found["hits"].as_array().expect("hits").is_empty());
        assert_eq!(found["truncated"], false);
    }

    #[test]
    fn json_tools_find_structured_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("d.json"),
            r#"{"companies":{"7203":{"name":"トヨタ自動車"},"8015":{"name":"豊田通商"}},
               "relations":[{"source":{"key":"7203"},"target":{"key":"8015"}},
                            {"source":{"key":"9999"},"target":{"key":"1111"}}]}"#,
        )
        .expect("write");
        let l = lib(dir.path());
        let got = json_get(&l, "d.json", "/companies/7203", 20_000).expect("get");
        assert_eq!(got["value"]["name"], "トヨタ自動車");

        let rel = json_find(
            &l,
            &json!({"path": "d.json", "pointer": "/relations",
                    "match": [{"field": "/source/key", "value": "8015"},
                              {"field": "/target/key", "value": "8015"}]}),
        )
        .expect("find");
        assert_eq!(rel["total_matches"], 1);

        let by_name = json_find(
            &l,
            &json!({"path": "d.json", "pointer": "/companies",
                    "match": [{"field": "/name", "op": "contains", "value": "豊田"}]}),
        )
        .expect("find");
        assert_eq!(by_name["items"][0]["_key"], "8015");
    }
}
