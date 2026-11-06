//! A curl-shaped argument language, never a shell or a curl subprocess.
//!
//! Only inline requests are translated. The existing HTTP tool remains the
//! single owner of host/method policy, secret resolution and request logging.

use serde_json::{Map, Value, json};

pub(crate) struct Request {
    pub arguments: Value,
    pub fail_on_http_error: bool,
}

/// Parse without expanding variables, command substitutions or glob patterns.
/// Errors intentionally do not reflect arguments, which may contain secrets.
pub(crate) fn parse(command: &str) -> Result<Request, String> {
    if command.len() > 65_536 || command.contains('\0') {
        return Err("command exceeds bounds".to_owned());
    }
    let words = shlex::split(command).ok_or("invalid quoting in curl command")?;
    let mut words = words.into_iter();
    if words.next().as_deref() != Some("curl") {
        return Err("command must start with curl".to_owned());
    }
    let mut url = None;
    let mut method = None;
    let mut body: Option<String> = None;
    let mut headers = Map::new();
    let mut fail_on_http_error = false;
    while let Some(word) = words.next() {
        let (option, inline) = match word.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option, Some(value)),
            _ => (word.as_str(), None),
        };
        let kind = if matches!(option, "-X" | "--request") || word.starts_with("-X") {
            Some("method")
        } else if matches!(option, "-H" | "--header") || word.starts_with("-H") {
            Some("header")
        } else if matches!(option, "-d" | "--data" | "--data-raw" | "--json")
            || word.starts_with("-d")
        {
            Some("body")
        } else if option == "--url" {
            Some("url")
        } else {
            None
        };
        if let Some(kind) = kind {
            let attached = if matches!(word.get(..2), Some("-X" | "-H" | "-d")) {
                word.get(2..).filter(|s| !s.is_empty())
            } else {
                None
            };
            let value = inline
                .or(attached)
                .map(str::to_owned)
                .or_else(|| words.next())
                .ok_or("curl option needs a value")?;
            match kind {
                "method" => {
                    if method.replace(value.to_ascii_uppercase()).is_some() {
                        return Err("only one request method is supported".to_owned());
                    }
                }
                "header" => {
                    let (name, value) = value.split_once(':').ok_or("header needs name: value")?;
                    let name = name.trim().to_ascii_lowercase();
                    if name.is_empty() || headers.insert(name, json!(value.trim())).is_some() {
                        return Err("empty or repeated header is unsupported".to_owned());
                    }
                }
                "body" => {
                    if value.starts_with('@') {
                        return Err("file-backed request bodies are unsupported".to_owned());
                    }
                    // The shared transport labels every body application/json.
                    // Reject other formats rather than silently relabel them.
                    serde_json::from_str::<Value>(&value)
                        .map_err(|_| "request body must be inline JSON")?;
                    if body.replace(value).is_some() {
                        return Err("only one inline request body is supported".to_owned());
                    }
                    if option == "--json" {
                        headers
                            .entry("accept".to_owned())
                            .or_insert(json!("application/json"));
                    }
                }
                _ => set_url(&mut url, value)?,
            }
        } else if word.len() > 1
            && word.starts_with('-')
            && !word.starts_with("--")
            && word[1..].chars().all(|c| matches!(c, 's' | 'S' | 'f'))
        {
            fail_on_http_error |= word.contains('f');
        } else if matches!(
            word.as_str(),
            "--silent" | "--show-error" | "--fail" | "--fail-with-body"
        ) {
            fail_on_http_error |= matches!(word.as_str(), "--fail" | "--fail-with-body");
        } else if word.starts_with("http://") || word.starts_with("https://") {
            set_url(&mut url, word)?;
        } else {
            return Err(
                "unsupported curl argument; only inline HTTP requests are allowed".to_owned(),
            );
        }
    }
    let url = url.ok_or("one HTTP(S) URL is required")?;
    let method = method.unwrap_or_else(|| if body.is_some() { "POST" } else { "GET" }.to_owned());
    Ok(Request {
        arguments: json!({"method": method, "url": url, "headers": headers,
            "body": body.unwrap_or_default()}),
        fail_on_http_error,
    })
}

fn set_url(url: &mut Option<String>, value: String) -> Result<(), String> {
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err("URL must use HTTP(S)".to_owned());
    }
    if url.replace(value).is_some() {
        return Err("only one URL is supported".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_parses_quoted_json_and_keeps_env_reference_literal() {
        let r = parse(r#"curl -sSf -XPOST 'https://example.test/api' -H 'X-Agent-Key: $ENV:AGORA_KEY' --json '{"body":"日本語 and spaces"}'"#).unwrap();
        assert_eq!(r.arguments["method"], "POST");
        assert_eq!(r.arguments["body"], r#"{"body":"日本語 and spaces"}"#);
        assert_eq!(r.arguments["headers"]["x-agent-key"], "$ENV:AGORA_KEY");
        assert_eq!(r.arguments["headers"]["accept"], "application/json");
        assert!(r.fail_on_http_error);
    }

    #[test]
    fn curl_defaults_and_attached_arguments() {
        let r = parse("curl --url=https://example.test/ -HAccept:application/json").unwrap();
        assert_eq!(r.arguments["method"], "GET");
        let r = parse("curl https://example.test/ -d'{}'").unwrap();
        assert_eq!(r.arguments["method"], "POST");
        assert!(!r.fail_on_http_error);
    }

    #[test]
    fn curl_treats_fail_with_body_like_fail() {
        let fail = parse("curl --fail https://example.test/").unwrap();
        let fail_with_body = parse("curl --fail-with-body https://example.test/").unwrap();
        assert_eq!(fail_with_body.arguments, fail.arguments);
        assert!(fail.fail_on_http_error);
        assert_eq!(fail_with_body.fail_on_http_error, fail.fail_on_http_error);
    }

    #[test]
    fn curl_rejects_files_shell_operators_and_unsupported_options_without_echoing_secrets() {
        for command in [
            "sh -c curl",
            "curl https://example.test/ ; secret-command",
            "curl https://example.test/ -d@secret-file",
            "curl https://example.test/ --data-raw @secret-file",
            "curl https://example.test/ -o secret-file",
            "curl -L https://example.test/",
            "curl https://example.test/ --proxy secret-proxy",
            "curl https://example.test/ -d 'secret-body'",
            "curl https://example.test/ -H 'secret-header'",
            "curl https://example.test/ 'unterminated",
            "curl https://example.test/ https://secret.test/",
            "curl -X",
            "curl",
        ] {
            let error = parse(command).err().expect("must refuse");
            assert!(!error.contains("secret"), "{error}");
        }
        let r =
            parse(r#"curl https://example.test/ --json '{"body":"$(secret-command)"}'"#).unwrap();
        assert!(
            r.arguments["body"]
                .as_str()
                .unwrap()
                .contains("$(secret-command)")
        );
    }
}
