//! Per-turn reference material from operator-registered, read-only sources.
//!
//! On every dialogue turn the host (not the model) consults the configured
//! tool server: it lists the library catalog, so the individual knows what
//! it *can* consult, and runs key lookups for identifiers found in the input
//! (e.g. Japanese securities codes → a company record). The result goes to
//! the Persona as `REFERENCE_MATERIAL`: external data with provenance, never
//! belief, memory or instruction.
//!
//! Consultation is best-effort and bounded: a slow or absent tool server
//! yields no material (and a recorded error), never a failed turn.

use std::time::Duration;

use kamimusuhi_resource_http::http::{Endpoint, Header, get_json, post_json};
use kamimusuhi_resource_http::tls::TrustAnchors;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceSetting {
    /// Tool server base URL, e.g. `http://127.0.0.1:7860`.
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_env: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Include the library catalog in every turn.
    #[serde(default = "default_true")]
    pub catalog: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_lookups: Vec<KeyLookup>,
}

const fn default_timeout_ms() -> u64 {
    5_000
}

const fn default_true() -> bool {
    true
}

/// Which identifiers in the input trigger a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPattern {
    /// Four characters: a digit, then digits or capital letters (`7203`,
    /// `156A`), not part of a longer alphanumeric run. Full-width forms are
    /// normalized first.
    JpSecuritiesCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyLookup {
    pub pattern: KeyPattern,
    pub library: String,
    /// JSON file inside the library.
    pub path: String,
    /// JSON pointer prefix; the key is appended (`/companies/` + `7203`).
    pub pointer_prefix: String,
    /// JSON pointers to keep from each record. Empty keeps the (bounded) record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    #[serde(default = "default_max_keys")]
    pub max_keys: usize,
}

const fn default_max_keys() -> usize {
    3
}

/// Normalize full-width digits/letters to ASCII.
fn halfwidth(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '０'..='９' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ' => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            other => other,
        })
        .collect()
}

pub fn find_keys(pattern: KeyPattern, text: &str, max: usize) -> Vec<String> {
    match pattern {
        KeyPattern::JpSecuritiesCode => {
            let chars: Vec<char> = halfwidth(text).chars().collect();
            let mut keys = Vec::new();
            let mut i = 0;
            while i < chars.len() {
                if !chars[i].is_ascii_alphanumeric() {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let run = &chars[start..i];
                let is_code = run.len() == 4
                    && run[0].is_ascii_digit()
                    && run[1..]
                        .iter()
                        .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
                let key: String = run.iter().collect();
                if is_code && !keys.contains(&key) {
                    keys.push(key);
                    if keys.len() >= max {
                        break;
                    }
                }
            }
            keys
        }
    }
}

pub struct ReferenceClient {
    setting: ReferenceSetting,
    catalog: Option<Value>,
}

impl ReferenceClient {
    pub const fn new(setting: ReferenceSetting) -> Self {
        Self {
            setting,
            catalog: None,
        }
    }

    fn headers(&self) -> Result<Vec<Header>, String> {
        let Some(name) = &self.setting.auth_env else {
            return Ok(Vec::new());
        };
        match std::env::var(name) {
            Ok(t) if !t.trim().is_empty() => Ok(vec![Header {
                name: "Authorization".to_owned(),
                value: format!("Bearer {}", t.trim()),
            }]),
            _ => Err(format!("environment variable {name} is not set")),
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.setting.timeout_ms.max(1))
    }

    fn fetch_catalog(&self) -> Result<Value, String> {
        let endpoint = Endpoint::parse(&self.setting.base_url, "/v1/tools")?;
        let response = get_json(
            &endpoint,
            &self.headers()?,
            self.timeout(),
            &TrustAnchors::Webpki,
        )
        .map_err(|e| format!("{e:?}"))?;
        if !response.is_success() {
            return Err(format!("tool server HTTP {}", response.status));
        }
        let parsed: Value =
            serde_json::from_str(&response.body).map_err(|_| "catalog is not JSON".to_owned())?;
        Ok(json!({
            "libraries": parsed["libraries"],
            "tools": parsed["tools"].as_array().map(|t| t.iter()
                .filter_map(|d| d["function"]["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()),
        }))
    }

    fn call(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let endpoint = Endpoint::parse(&self.setting.base_url, "/v1/tools/call")?;
        let body = json!({"name": name, "arguments": arguments});
        let response = post_json(
            &endpoint,
            &body.to_string(),
            &self.headers()?,
            self.timeout(),
            &TrustAnchors::Webpki,
        )
        .map_err(|e| format!("{e:?}"))?;
        let reply: Value = serde_json::from_str(&response.body)
            .map_err(|_| format!("tool server HTTP {} (non-JSON)", response.status))?;
        if reply["ok"].as_bool() == Some(true) {
            Ok(reply["result"].clone())
        } else {
            Err(reply["error"].to_string())
        }
    }

    /// Material for this input, or `None` when there is nothing to show.
    pub fn consult(&mut self, text: &str) -> Option<Value> {
        let mut errors = Vec::new();
        if self.setting.catalog && self.catalog.is_none() {
            match self.fetch_catalog() {
                Ok(catalog) => self.catalog = Some(catalog),
                Err(e) => errors.push(json!({"step": "catalog", "error": e})),
            }
        }
        let mut lookups = Vec::new();
        for lookup in &self.setting.key_lookups {
            for key in find_keys(lookup.pattern, text, lookup.max_keys) {
                let pointer = format!("{}{}", lookup.pointer_prefix, key);
                match self.call(
                    "json_get",
                    json!({"library": lookup.library, "path": lookup.path,
                           "pointer": pointer, "max_bytes": 6000}),
                ) {
                    Ok(result) => {
                        let record = &result["value"];
                        let value = if lookup.fields.is_empty() {
                            record.clone()
                        } else {
                            let mut kept = serde_json::Map::new();
                            for field in &lookup.fields {
                                if let Some(v) = record.pointer(field) {
                                    kept.insert(field.clone(), v.clone());
                                }
                            }
                            Value::Object(kept)
                        };
                        lookups.push(json!({"key": key, "library": lookup.library,
                            "path": lookup.path, "pointer": pointer, "value": value}));
                    }
                    // A key that is not in the dataset is normal, not an error.
                    Err(e) if e.contains("no value at") => {}
                    Err(e) => errors.push(json!({"step": "lookup", "key": key, "error": e})),
                }
            }
        }
        if self.catalog.is_none() && lookups.is_empty() && errors.is_empty() {
            return None;
        }
        let mut material = json!({
            "kind": "operator_registered_reference",
            "authority": "external_material_not_belief",
            "catalog": self.catalog,
            "lookups": lookups,
        });
        if !errors.is_empty() {
            material["errors"] = Value::Array(errors);
            // Retry the catalog next turn.
            if self.catalog.is_none() {
                material["catalog"] = Value::Null;
            }
        }
        Some(material)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_securities_codes() {
        let keys = find_keys(
            KeyPattern::JpSecuritiesCode,
            "トヨタ(7203)と１５６Ａ、それに12345やABCDやa7203",
            5,
        );
        assert_eq!(keys, ["7203", "156A"]);
    }

    #[test]
    fn unreachable_server_is_reported_not_fatal() {
        let mut client = ReferenceClient::new(ReferenceSetting {
            base_url: "http://127.0.0.1:9".to_owned(),
            auth_env: None,
            timeout_ms: 500,
            catalog: true,
            key_lookups: Vec::new(),
        });
        let material = client.consult("hello").expect("errors are material");
        assert!(material["errors"].is_array());
        assert!(material["catalog"].is_null());
    }
}
