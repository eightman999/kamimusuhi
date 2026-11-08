//! Behavioral declarations for resident aliases and their registered peers.
//!
//! Declarations contain hashes, never credentials, prompts or model output.
//! Failed health checks retain the last declaration; a pinned request cannot
//! use a changed peer while its replacement declaration has not been reviewed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};

use kamimusuhi_core::digest::{content_digest, json_digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::state::Shared;

pub const HEADER: &str = "X-Kamimusuhi-Identity-Expected";
pub const CONTEXT_ENV: &str = "KAMIMUSUHI_IDENTITY_DECLARATIONS";
const MAX_MODELS: usize = 128;
const MAX_PEERS: usize = 32;
const MAX_CACHE_BYTES: u64 = 256 * 1024;

fn hash(value: &str) -> String {
    content_digest(value.as_bytes())
}

fn valid_hash(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Canonical resident base, shared with runtime's pin lookup: no `/v1` suffix.
/// Credentials and query/fragment values are refused rather than persisted.
pub fn canonical_endpoint(url: &str) -> Result<String, String> {
    if url.chars().any(char::is_whitespace)
        || url.chars().any(char::is_control)
        || url.contains(['?', '#', '@'])
    {
        return Err("IDENTITY_UNSAFE_ENDPOINT".to_owned());
    }
    let (scheme, rest) = url
        .split_once("://")
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| "IDENTITY_UNSAFE_ENDPOINT".to_owned())?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err("IDENTITY_UNSAFE_ENDPOINT".to_owned());
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix("/v1").unwrap_or(path);
    let path = if path == "v1" { "" } else { path };
    Ok(format!(
        "{scheme}://{}{}{}",
        authority.to_ascii_lowercase(),
        if path.is_empty() { "" } else { "/" },
        path
    ))
}

fn body_hash(extra: &Option<serde_json::Map<String, Value>>) -> Result<String, String> {
    const ALLOWED: &[&str] = &[
        "max_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "top_k",
        "seed",
        "frequency_penalty",
        "presence_penalty",
        "min_p",
        "repetition_penalty",
        "stop",
        "reasoning_effort",
        "reasoning",
        "thinking",
        "chat_template_kwargs",
        "include_reasoning",
        "response_format",
        "json_schema",
    ];
    if extra
        .as_ref()
        .is_some_and(|map| map.keys().any(|key| !ALLOWED.contains(&key.as_str())))
    {
        return Err("UNCLASSIFIED_ROUTING_FIELD".to_owned());
    }
    fn safe(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().all(|(key, value)| {
                let key = key.to_ascii_lowercase();
                ![
                    "api_key",
                    "token",
                    "secret",
                    "password",
                    "authorization",
                    "credential",
                    "access_token",
                    "refresh_token",
                ]
                .contains(&key.as_str())
                    && safe(value)
            }),
            Value::Array(items) => items.iter().all(safe),
            _ => true,
        }
    }
    let value = serde_json::to_value(extra).map_err(|_| "IDENTITY_SERIALIZATION".to_owned())?;
    if !safe(&value) {
        return Err("UNCLASSIFIED_ROUTING_FIELD".to_owned());
    }
    Ok(json_digest(&value))
}

/// Own declaration only. It never includes a peer's recursive declaration.
pub fn declaration(config: &Config) -> Result<Value, String> {
    let mut models = BTreeSet::new();
    let mut tiers = Vec::new();
    for tier in &config.tiers {
        tier.validate_identity_model_aliases()?;
        let aliases: BTreeSet<String> = tier
            .identity_model_aliases
            .iter()
            .map(|name| hash(name))
            .collect();
        models.extend(aliases.iter().cloned());
        models.insert(hash(&tier.model));
        tiers.push(json!({
            "id": hash(&tier.name),
            "endpoint": hash(&canonical_endpoint(&tier.base_url)?),
            "model": hash(&tier.model),
            "model_aliases": aliases,
            "condition": tier.condition,
            "node_local": tier.node_local,
            "peer_local_only": tier.peer_local_only,
            "privacy": tier.privacy_ok_for_private_memory,
            "billing_class": tier.billing,
            "input_usd_per_mtok": tier.input_usd_per_mtok,
            "output_usd_per_mtok": tier.output_usd_per_mtok,
            "context_limit_tokens": tier.context_limit_tokens,
            "approx_tpm_limit": tier.approx_tpm_limit,
            "extra_body": body_hash(&tier.extra_body)?,
            "strip_fields": tier.strip_fields,
            "prefer_lanes": tier.prefer_lanes,
            "reasoning_suppression": tier.reasoning_suppression,
        }));
    }
    if models.len() > MAX_MODELS {
        return Err("IDENTITY_MODEL_LIMIT".to_owned());
    }
    let components = json!({
        "node": hash(&config.node.id),
        "tiers": tiers,
        "routing": {
            "small_request_chars": config.routing.small_request_chars,
            "fast_chat_latency_ceiling_ms": config.routing.fast_chat_latency_ceiling_ms,
            "max_request_cost_usd": config.routing.max_request_cost_usd,
            "max_session_cost_usd": config.routing.max_session_cost_usd,
            "max_daily_cost_usd": config.routing.max_daily_cost_usd,
            "max_monthly_cost_usd": config.routing.max_monthly_cost_usd,
            "memory_heavy_prompt_tokens": config.routing.memory_heavy_prompt_tokens,
            "fast_chat_max_input_chars": config.routing.fast_chat_max_input_chars,
            "private_by_default": config.routing.private_by_default,
            "max_completion_tokens_for_cost": config.routing.max_completion_tokens_for_cost,
        },
        "router_implementation": hash(include_str!("router.rs")),
    });
    let mut value = json!({"version":1, "node_hash":hash(&config.node.id),
        "model_hashes":models, "components":components});
    value["digest"] = json!(json_digest(&value));
    Ok(value)
}

fn validate_declaration(value: &Value, peer: &str) -> Result<(), String> {
    if value["version"] != 1 || value["node_hash"] != hash(peer) {
        return Err("IDENTITY_PEER_BINDING".to_owned());
    }
    let digest = value["digest"]
        .as_str()
        .filter(|digest| valid_hash(digest))
        .ok_or_else(|| "IDENTITY_PEER_DECLARATION".to_owned())?;
    let models = value["model_hashes"]
        .as_array()
        .filter(|models| models.len() <= MAX_MODELS)
        .ok_or_else(|| "IDENTITY_PEER_DECLARATION".to_owned())?;
    if models
        .iter()
        .any(|model| !model.as_str().is_some_and(valid_hash))
    {
        return Err("IDENTITY_PEER_DECLARATION".to_owned());
    }
    let mut unsigned = value.clone();
    unsigned
        .as_object_mut()
        .ok_or("IDENTITY_PEER_DECLARATION")?
        .remove("digest");
    if json_digest(&unsigned) != digest {
        return Err("IDENTITY_PEER_DECLARATION".to_owned());
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedPeer {
    endpoint_hash: String,
    declaration: Value,
}

fn endpoint_hash(config: &crate::config::PeerConfig) -> Result<String, String> {
    let endpoints: BTreeSet<_> = config
        .urls()
        .map(canonical_endpoint)
        .collect::<Result<_, _>>()?;
    Ok(json_digest(&json!(endpoints)))
}

/// Bounded, last-known declarations survive process restart and peer downtime.
/// Authentication availability and probe health never enter the manifest.
pub fn context(shared: &Shared) -> Result<Value, String> {
    let own = declaration(&shared.config)?;
    if shared.config.peers.len() > MAX_PEERS {
        return Err("IDENTITY_PEER_LIMIT".to_owned());
    }
    let path = shared
        .config
        .paths
        .current_state()
        .join("identity-peers.json");
    fs::create_dir_all(shared.config.paths.current_state())
        .map_err(|_| "IDENTITY_CACHE_IO".to_owned())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|_| "IDENTITY_CACHE_IO".to_owned())?;
    lock.lock().map_err(|_| "IDENTITY_CACHE_IO".to_owned())?;
    let mut cached: BTreeMap<String, CachedPeer> = if path.exists() {
        if path.metadata().map_err(|_| "IDENTITY_CACHE_IO")?.len() > MAX_CACHE_BYTES {
            return Err("IDENTITY_CACHE_INVALID".to_owned());
        }
        serde_json::from_slice(&fs::read(&path).map_err(|_| "IDENTITY_CACHE_IO")?)
            .map_err(|_| "IDENTITY_CACHE_INVALID".to_owned())?
    } else {
        BTreeMap::new()
    };
    let before = serde_json::to_vec(&cached).map_err(|_| "IDENTITY_SERIALIZATION")?;
    cached.retain(|id, _| shared.config.peers.iter().any(|peer| &peer.id == id));
    let probes = shared
        .peers
        .read()
        .unwrap_or_else(|poison| poison.into_inner());
    for peer in &shared.config.peers {
        let fingerprint = endpoint_hash(peer)?;
        if cached
            .get(&peer.id)
            .is_some_and(|cached| cached.endpoint_hash != fingerprint)
        {
            cached.remove(&peer.id);
        }
        if let Some(declaration) = probes
            .get(&peer.id)
            .map(|probe| &probe.detail["identity_declaration"])
            .filter(|value| !value.is_null())
        {
            validate_declaration(declaration, &peer.id)?;
            cached.insert(
                peer.id.clone(),
                CachedPeer {
                    endpoint_hash: fingerprint,
                    declaration: declaration.clone(),
                },
            );
        }
        if let Some(entry) = cached.get(&peer.id) {
            validate_declaration(&entry.declaration, &peer.id)?;
        }
    }
    drop(probes);
    let encoded = serde_json::to_vec(&cached).map_err(|_| "IDENTITY_SERIALIZATION")?;
    if encoded.len() as u64 > MAX_CACHE_BYTES {
        return Err("IDENTITY_CACHE_INVALID".to_owned());
    }
    if before != encoded {
        crate::util::atomic_write(&path, &encoded).map_err(|_| "IDENTITY_CACHE_IO")?;
    }
    let mut models: BTreeSet<String> = own["model_hashes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    let mut pins = BTreeMap::new();
    let own_digest = own["digest"].as_str().unwrap().to_owned();
    for endpoint in local_endpoints(&shared.config) {
        pins.insert(endpoint, own_digest.clone());
    }
    let mut peers = BTreeMap::new();
    for peer in &shared.config.peers {
        match cached.get(&peer.id) {
            Some(entry) => {
                let digest = entry.declaration["digest"].as_str().unwrap().to_owned();
                peers.insert(peer.id.clone(), json!(digest));
                for endpoint in peer.urls() {
                    pins.insert(canonical_endpoint(endpoint)?, digest.clone());
                }
                for model in entry.declaration["model_hashes"].as_array().unwrap() {
                    models.insert(model.as_str().unwrap().to_owned());
                }
            }
            None => {
                peers.insert(peer.id.clone(), Value::Null);
            }
        }
    }
    Ok(json!({"version":1,"model_hashes":models,"route_pins":pins,
        "declarations":{"self":own_digest,"peers":peers}}))
}

fn local_endpoints(config: &Config) -> BTreeSet<String> {
    let mut urls = BTreeSet::new();
    let port = config.node.listen.rsplit(':').next().unwrap_or("7860");
    for address in [
        config.node.listen.clone(),
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ] {
        if let Ok(url) = canonical_endpoint(&format!("http://{address}")) {
            urls.insert(url);
        }
    }
    urls
}

pub fn matches_pin(shared: &Shared, expected: Option<&str>) -> bool {
    match expected {
        None => true,
        Some(expected) => {
            declaration(&shared.config).is_ok_and(|value| value["digest"] == expected)
        }
    }
}

pub fn is_peer(shared: &Shared, url: &str) -> Result<bool, String> {
    let endpoint = canonical_endpoint(url)?;
    Ok(shared.config.peers.iter().any(|peer| {
        peer.urls()
            .filter_map(|url| canonical_endpoint(url).ok())
            .any(|url| url == endpoint)
    }))
}

pub fn supplied_pins(value: &Value) -> Result<(), String> {
    let map = value
        .as_object()
        .filter(|map| map.len() <= 128)
        .ok_or_else(|| "IDENTITY_ROUTE_CONTEXT_INVALID".to_owned())?;
    for (url, digest) in map {
        if canonical_endpoint(url)? != *url || !digest.as_str().is_some_and(valid_hash) {
            return Err("IDENTITY_ROUTE_CONTEXT_INVALID".to_owned());
        }
    }
    Ok(())
}

/// A configured peer never receives an unpinned request from this router.
pub fn peer_pin(shared: &Shared, url: &str) -> Result<Option<String>, String> {
    let endpoint = canonical_endpoint(url)?;
    let peer = shared.config.peers.iter().find(|peer| {
        peer.urls()
            .filter_map(|url| canonical_endpoint(url).ok())
            .any(|url| url == endpoint)
    });
    let Some(_peer) = peer else { return Ok(None) };
    let context = context(shared)?;
    context["route_pins"][&endpoint]
        .as_str()
        .map(|pin| Some(pin.to_owned()))
        .ok_or_else(|| "IDENTITY_PEER_UNVERIFIED".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spool::Spool;
    use crate::state::ProbeResult;

    fn config(root: &std::path::Path, id: &str) -> Config {
        serde_json::from_value(json!({
            "node":{"id":id,"role":"continuity","listen":"127.0.0.1:7860"},
            "paths":{"root":root},
            "tiers":[{"name":"cloud","model":"model-a","base_url":"https://models.invalid/v1","billing":"subscription","extra_body":{"temperature":0.1}}]
        })).unwrap()
    }

    fn shared(config: Config) -> Shared {
        let spool = Spool::new(config.paths.spool(), &config.node.id).unwrap();
        Shared::new(config, spool, 1, None)
    }

    #[test]
    fn model_and_prompt_shaping_change_but_auth_rotation_and_timeouts_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "pi");
        let initial = declaration(&c).unwrap();
        c.tiers[0].auth_env = Some("ROTATED_CREDENTIAL_ENV".to_owned());
        c.tiers[0].timeout_secs = 7;
        c.tiers[0].probe_interval_secs = 19;
        assert_eq!(initial, declaration(&c).unwrap());
        c.tiers[0].model = "model-b".to_owned();
        assert_ne!(initial["digest"], declaration(&c).unwrap()["digest"]);
        c.tiers[0].model = "model-a".to_owned();
        c.tiers[0]
            .extra_body
            .as_mut()
            .unwrap()
            .insert("temperature".to_owned(), json!(0.9));
        assert_ne!(initial["digest"], declaration(&c).unwrap()["digest"]);
    }

    #[test]
    fn credential_fields_and_unsafe_urls_are_refused_without_echoing_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "pi");
        c.tiers[0]
            .extra_body
            .as_mut()
            .unwrap()
            .insert("api_key".to_owned(), json!("private-value"));
        assert_eq!(declaration(&c).unwrap_err(), "UNCLASSIFIED_ROUTING_FIELD");
        assert!(canonical_endpoint("https://u:private-value@example.invalid/v1").is_err());
        assert_eq!(
            canonical_endpoint("http://EXAMPLE.invalid:7860/v1/").unwrap(),
            "http://example.invalid:7860"
        );
    }

    #[test]
    fn cache_retains_peer_identity_across_failure_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "mac");
        c.peers=serde_json::from_value(json!([{"id":"pi","role":"continuity","url":"http://pi.invalid:7860","alt_urls":["http://100.64.0.2:7860"]}])).unwrap();
        let s = shared(c.clone());
        let peer = declaration(&config(remote.path(), "pi")).unwrap();
        let mut probe = ProbeResult::default();
        probe.record(Ok(json!({"identity_declaration":peer})), 1);
        s.peers.write().unwrap().insert("pi".to_owned(), probe);
        let initial = context(&s).unwrap();
        s.peers
            .write()
            .unwrap()
            .get_mut("pi")
            .unwrap()
            .record(Err("temporarily offline".to_owned()), 20);
        assert_eq!(initial, context(&s).unwrap());
        let restored = shared(c);
        assert_eq!(initial, context(&restored).unwrap());
        assert!(
            initial["model_hashes"]
                .as_array()
                .unwrap()
                .contains(&json!(hash("model-a")))
        );
        assert_eq!(
            peer_pin(&restored, "http://pi.invalid:7860/v1").unwrap(),
            initial["route_pins"]["http://pi.invalid:7860"]
                .as_str()
                .map(str::to_owned)
        );
    }

    #[test]
    fn changed_peer_declaration_changes_context_without_health_driven_drift() {
        let dir = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "mac");
        c.peers = serde_json::from_value(
            json!([{"id":"pi","role":"continuity","url":"http://pi.invalid:7860"}]),
        )
        .unwrap();
        let s = shared(c);
        let mut peer = config(remote.path(), "pi");
        let mut probe = ProbeResult::default();
        probe.record(
            Ok(json!({"identity_declaration":declaration(&peer).unwrap()})),
            1,
        );
        s.peers.write().unwrap().insert("pi".to_owned(), probe);
        let first = context(&s).unwrap();
        peer.tiers[0].model = "new-peer-model".to_owned();
        s.peers.write().unwrap().get_mut("pi").unwrap().record(
            Ok(json!({"identity_declaration":declaration(&peer).unwrap()})),
            3,
        );
        assert_ne!(first, context(&s).unwrap());
    }

    #[test]
    fn altered_cache_and_unknown_peer_do_not_authorize_unpinned_use() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "mac");
        c.peers = serde_json::from_value(
            json!([{"id":"pi","role":"continuity","url":"http://pi.invalid:7860"}]),
        )
        .unwrap();
        let s = shared(c);
        assert_eq!(
            peer_pin(&s, "http://pi.invalid:7860/v1").unwrap_err(),
            "IDENTITY_PEER_UNVERIFIED"
        );
        let cache = s.config.paths.current_state().join("identity-peers.json");
        fs::write(cache, b"{broken").unwrap();
        assert_eq!(context(&s).unwrap_err(), "IDENTITY_CACHE_INVALID");
    }

    #[test]
    fn model_aliases_are_explicit_bounded_order_independent_and_require_new_review() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config(dir.path(), "pi");
        let before = declaration(&c).unwrap();
        assert!(
            !before["model_hashes"]
                .as_array()
                .unwrap()
                .contains(&json!(hash("response-model")))
        );
        c.tiers[0].identity_model_aliases =
            vec!["response-model".into(), "response-model-v2".into()];
        c.validate().unwrap();
        let declared = declaration(&c).unwrap();
        assert_ne!(
            before["digest"], declared["digest"],
            "alias changes invalidate the reviewed declaration"
        );
        assert!(
            declared["model_hashes"]
                .as_array()
                .unwrap()
                .contains(&json!(hash("response-model")))
        );
        assert!(!matches_pin(&shared(c.clone()), before["digest"].as_str()));
        c.tiers[0].identity_model_aliases.reverse();
        assert_eq!(declaration(&c).unwrap(), declared);
        for invalid in [
            vec![String::new()],
            vec!["x".repeat(257)],
            vec!["bad\nmodel".into()],
            vec!["model".into(); 17],
        ] {
            c.tiers[0].identity_model_aliases = invalid;
            assert_eq!(c.validate().unwrap_err(), "IDENTITY_MODEL_ALIASES_INVALID");
            assert_eq!(
                declaration(&c).unwrap_err(),
                "IDENTITY_MODEL_ALIASES_INVALID"
            );
        }
    }

    #[test]
    fn expected_pin_is_compared_before_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let s = shared(config(dir.path(), "pi"));
        let declaration = declaration(&s.config).unwrap();
        assert!(matches_pin(&s, declaration["digest"].as_str()));
        assert!(!matches_pin(&s, Some(&"0".repeat(64))));
        assert!(matches_pin(&s, None));
    }
}
