//! Agora's public JSON API. Account credentials never leave this module.
//!
//! Registration is deliberately not retried after an ambiguous response:
//! the service returns its key once, so a timeout is not proof of failure.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::Shared;

const HOST: &str = "agora.grimoire-fio.workers.dev";
const BASE: &str = "https://agora.grimoire-fio.workers.dev";
const WRITE_INTERVAL_SECS: u64 = 10;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    name: String,
    #[serde(default)]
    api_key: Option<String>,
    /// Persisted before registration goes on the wire. A crash retains it.
    #[serde(default)]
    registration_pending: bool,
    #[serde(default)]
    last_write_at: Option<u64>,
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn fail(message: &str) -> Value {
    json!({"ok": false, "error": message})
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let tmp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(&tmp)
        .map_err(|_| "cannot create private Agora state")?;
    // Never include an I/O error's path or the serialized state in output.
    let outcome = (|| {
        file.write_all(bytes)
            .map_err(|_| "cannot write private Agora state")?;
        file.sync_all()
            .map_err(|_| "cannot sync private Agora state")?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect private Agora state")?;
        fs::rename(&tmp, path).map_err(|_| "cannot replace private Agora state")?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(tmp);
    }
    outcome
}

fn save(path: &Path, account: &Account) -> Result<(), String> {
    let bytes = serde_json::to_vec(account).map_err(|_| "cannot encode private Agora state")?;
    private_write(path, &bytes)
}

fn text<'a>(args: &'a Value, name: &str, max: usize) -> Result<&'a str, String> {
    let value = args[name]
        .as_str()
        .ok_or_else(|| format!("{name} is required"))?;
    if value.trim().is_empty() || value.len() > max {
        return Err(format!("{name} must be nonempty and at most {max} bytes"));
    }
    Ok(value)
}

pub fn definitions() -> Vec<Value> {
    vec![json!({"type": "function", "function": {
        "name": "agora",
        "description": "Agora public agent forum: status, register (once; key stored privately), post a thread, reply to a thread, or read listings. Posts must describe measured observations with date/machine context and distinguish inference. Writes are limited to one per 10 seconds, including across restarts. Requires the Agora host in tools.http_allow_hosts. No registration or posting retry after uncertain transport failure.",
        "parameters": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["status", "register", "post", "reply", "listings"]},
            "name": {"type": "string", "description": "agent name for registration"},
            "description": {"type": "string", "description": "agent description for registration"},
            "title": {"type": "string"},
            "body": {"type": "string"},
            "thread_id": {"type": "integer", "minimum": 1}
        }, "required": ["action"]}
    }})]
}

pub fn call(shared: &Shared, args: &Value) -> (u16, Value) {
    let action = args["action"].as_str().unwrap_or("");
    if action != "status" {
        let host_allowed = shared.config.tools.http_allow_hosts.iter().any(|entry| {
            let entry = entry.to_ascii_lowercase();
            HOST == entry || HOST.strip_suffix(&entry).is_some_and(|p| p.ends_with('.'))
        });
        let method = if action == "listings" { "GET" } else { "POST" };
        if !host_allowed
            || !shared
                .config
                .tools
                .http_allow_methods
                .iter()
                .any(|m| m == method)
        {
            return (
                200,
                fail("Agora host/method must be enabled in tools HTTP allowlists"),
            );
        }
    }
    let root = shared.config.paths.root.join("private/agora");
    let reply = execute(&root, args, crate::util::unix_now(), |request| {
        crate::tools::http_request_tool(shared, request, Duration::from_secs(30)).1
    });
    (200, reply)
}

fn execute(root: &Path, args: &Value, now: u64, mut send: impl FnMut(&Value) -> Value) -> Value {
    match execute_inner(root, args, now, &mut send) {
        Ok(reply) => reply,
        Err(error) => fail(&error),
    }
}

fn execute_inner(
    root: &Path,
    args: &Value,
    now: u64,
    send: &mut impl FnMut(&Value) -> Value,
) -> Result<Value, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let action = args["action"].as_str().unwrap_or("");
    if !matches!(
        action,
        "status" | "register" | "post" | "reply" | "listings"
    ) {
        return Err("unknown Agora action".to_owned());
    }
    fs::create_dir_all(root).map_err(|_| "cannot create private Agora directory")?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))
        .map_err(|_| "cannot protect private Agora directory")?;
    let lock_path = root.join("account.lock");
    let _file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|_| "Agora account is busy (or a stale lock needs operator recovery)")?;
    let _lock = Lock(lock_path);
    let path = root.join("account.json");
    let mut account = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Account>(&bytes)
            .map_err(|_| "invalid private Agora account state")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Account::default(),
        Err(_) => return Err("cannot read private Agora account state".to_owned()),
    };
    if action == "status" {
        return Ok(json!({"ok": true, "result": {
            "registered": account.api_key.is_some(),
            "name": account.name,
            "registration_pending": account.registration_pending,
            "retry_after_seconds": account.last_write_at.map_or(0, |at| WRITE_INTERVAL_SECS.saturating_sub(now.saturating_sub(at)))
        }}));
    }
    if action == "listings" {
        return Ok(safe_response(
            send(
                &json!({"method": "GET", "url": format!("{BASE}/api/listings"), "headers": {"User-Agent": "Kamimusuhi/0.1"}}),
            ),
            account.api_key.as_deref(),
        ));
    }
    if action == "register" && account.api_key.is_some() {
        return Ok(
            json!({"ok": true, "result": {"registered": true, "name": account.name, "already_registered": true}}),
        );
    }
    if account.registration_pending {
        return Err("registration outcome is uncertain; operator must recover the existing account before retrying".to_owned());
    }
    if let Some(at) = account.last_write_at
        && now.saturating_sub(at) < WRITE_INTERVAL_SECS
    {
        return Ok(
            json!({"ok": false, "error": "Agora write rate limit", "retry_after_seconds": WRITE_INTERVAL_SECS - now.saturating_sub(at)}),
        );
    }
    let mut headers = json!({"User-Agent": "Kamimusuhi/0.1"});
    let (url, body) = match action {
        "register" => {
            let name = text(args, "name", 120)?;
            let description = text(args, "description", 2000)?;
            account.name = name.to_owned();
            account.registration_pending = true;
            (
                format!("{BASE}/api/register"),
                json!({"name": name, "description": description}),
            )
        }
        "post" | "reply" => {
            let key = account
                .api_key
                .as_deref()
                .ok_or("register with Agora first")?;
            headers["X-Agent-Key"] = json!(key);
            let body = text(args, "body", 24 * 1024)?;
            if action == "post" {
                let title = text(args, "title", 400)?;
                (
                    format!("{BASE}/api/threads"),
                    json!({"title": title, "body": body}),
                )
            } else {
                let id = args["thread_id"]
                    .as_u64()
                    .filter(|v| *v > 0)
                    .ok_or("thread_id must be a positive integer")?;
                (
                    format!("{BASE}/api/threads/{id}/posts"),
                    json!({"body": body}),
                )
            }
        }
        _ => unreachable!(),
    };
    // Reserve before sending: timeouts and crashes cannot bypass the rate cap.
    account.last_write_at = Some(now);
    save(&path, &account)?;
    let response =
        send(&json!({"method": "POST", "url": url, "headers": headers, "body": body.to_string()}));
    if action == "register" {
        let status = response["result"]["status"].as_u64();
        // Neither a server body nor transport details are returned here: the
        // server might echo a one-time credential even in an error response.
        if response["ok"] != true || !status.is_some_and(|s| (200..300).contains(&s)) {
            return Err("Agora registration failed or is uncertain; account state retained for operator recovery".to_owned());
        }
        let parsed: Value = serde_json::from_str(response["result"]["body"].as_str().unwrap_or(""))
            .map_err(|_| "invalid registration response; operator recovery required")?;
        let key = parsed["api_key"]
            .as_str()
            .filter(|k| !k.is_empty() && k.len() <= 4096 && !k.contains(['\r', '\n']))
            .ok_or("registration response has no usable api_key; operator recovery required")?;
        account.api_key = Some(key.to_owned());
        account.registration_pending = false;
        save(&path, &account)?;
        return Ok(
            json!({"ok": true, "result": {"registered": true, "name": account.name, "credential_saved": true}}),
        );
    }
    Ok(safe_response(response, account.api_key.as_deref()))
}

/// Never pass the credential through even if an upstream echoes it.
fn safe_response(response: Value, key: Option<&str>) -> Value {
    fn scrub(value: Value, key: Option<&str>) -> Value {
        match value {
            Value::String(text) => Value::String(
                key.filter(|k| !k.is_empty())
                    .map_or(text.clone(), |k| text.replace(k, "[REDACTED]")),
            ),
            Value::Array(values) => {
                Value::Array(values.into_iter().map(|v| scrub(v, key)).collect())
            }
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(k, v)| {
                        let secret = k.eq_ignore_ascii_case("api_key")
                            || k.eq_ignore_ascii_case("x-agent-key");
                        (
                            k,
                            if secret {
                                json!("[REDACTED]")
                            } else {
                                scrub(v, key)
                            },
                        )
                    })
                    .collect(),
            ),
            other => other,
        }
    }
    let response = scrub(response, key);
    if response["ok"] == true
        && response["result"]["status"]
            .as_u64()
            .is_some_and(|s| !(200..300).contains(&s))
    {
        json!({"ok": false, "error": "Agora returned an HTTP error", "result": response["result"]})
    } else {
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered(root: &Path) {
        let reply = execute(
            root,
            &json!({"action": "register", "name": "fixture", "description": "test only"}),
            100,
            |req| {
                assert_eq!(req["url"], format!("{BASE}/api/register"));
                json!({"ok": true, "result": {"status": 201, "body": "{\"api_key\":\"fixture-secret\"}"}})
            },
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(!reply.to_string().contains("fixture-secret"));
    }

    #[test]
    fn registration_saves_private_key_and_retries_do_not_register_again() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        registered(dir.path());
        assert_eq!(
            fs::metadata(dir.path().join("account.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let result = execute(dir.path(), &json!({"action": "register"}), 101, |_| {
            panic!("duplicate registration")
        });
        assert_eq!(result["result"]["already_registered"], true);
    }

    #[test]
    fn post_and_reply_use_private_key_and_durable_rate_limit() {
        let dir = tempfile::tempdir().unwrap();
        registered(dir.path());
        let args = json!({"action": "post", "title": "Measured", "body": "2026-11-06 fixture observation"});
        let limited = execute(dir.path(), &args, 109, |_| panic!("rate limited"));
        assert_eq!(limited["retry_after_seconds"], 1);
        let posted = execute(dir.path(), &args, 110, |req| {
            assert_eq!(req["headers"]["X-Agent-Key"], "fixture-secret");
            assert_eq!(req["url"], format!("{BASE}/api/threads"));
            json!({"ok": true, "result": {"status": 201, "body": "fixture-secret"}})
        });
        assert_eq!(posted["ok"], true);
        assert!(!posted.to_string().contains("fixture-secret"));
        let reply = execute(
            dir.path(),
            &json!({"action": "reply", "thread_id": 3, "body": "measured"}),
            120,
            |req| {
                assert_eq!(req["url"], format!("{BASE}/api/threads/3/posts"));
                json!({"ok": true, "result": {"status": 201, "body": "{}"}})
            },
        );
        assert_eq!(reply["ok"], true);
    }

    #[test]
    fn uncertain_registration_is_not_retried_and_never_echoes_response() {
        let dir = tempfile::tempdir().unwrap();
        let args = json!({"action": "register", "name": "fixture", "description": "test"});
        let result = execute(
            dir.path(),
            &args,
            100,
            |_| json!({"ok": false, "error": "might contain secret"}),
        );
        assert_eq!(result["ok"], false);
        assert!(!result.to_string().contains("might contain secret"));
        let result = execute(dir.path(), &args, 200, |_| {
            panic!("uncertain registration retried")
        });
        assert_eq!(result["ok"], false);
        assert_eq!(
            execute(dir.path(), &json!({"action": "status"}), 200, |_| panic!())["result"]["registration_pending"],
            true
        );
    }

    #[test]
    fn invalid_reply_id_never_reaches_transport_and_http_errors_are_failures() {
        let dir = tempfile::tempdir().unwrap();
        registered(dir.path());
        let result = execute(
            dir.path(),
            &json!({"action": "reply", "thread_id": "1/../../register", "body": "x"}),
            110,
            |_| panic!(),
        );
        assert_eq!(result["ok"], false);
        let result = execute(
            dir.path(),
            &json!({"action": "post", "title": "t", "body": "b"}),
            110,
            |_| json!({"ok": true, "result": {"status": 429, "body": "wait"}}),
        );
        assert_eq!(result["ok"], false);
    }
}
