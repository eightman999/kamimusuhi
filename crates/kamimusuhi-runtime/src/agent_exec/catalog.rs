//! Model catalog: what each executor can run right now, discovered from
//! the harness itself and cached with a TTL. Nothing here is hard-coded,
//! so a model a harness adds tomorrow is usable without a code change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::{DiscoverFormat, ExecutorSpec, glob_match};
use super::types::{AgentBilling, ModelCapabilities, ModelDescriptor, ModelRef};

/// A model as a listing described it, before billing is decided.
#[derive(Debug, Clone, PartialEq)]
pub struct RawModel {
    pub id: String,
    pub aliases: Vec<String>,
    pub label: Option<String>,
    pub context_tokens: Option<u64>,
    pub price_note: Option<String>,
    /// USD per million (input, output) tokens, when the listing states
    /// what *this account* pays through the harness.
    pub discovered_usd_per_mtok: Option<(f64, f64)>,
    pub capabilities: Option<ModelCapabilities>,
    /// Variants folded into this entry (effort levels and the like).
    pub variants: usize,
    pub available: bool,
}

impl RawModel {
    pub fn new(id: String) -> Self {
        Self {
            id,
            aliases: Vec::new(),
            label: None,
            context_tokens: None,
            price_note: None,
            discovered_usd_per_mtok: None,
            capabilities: None,
            variants: 0,
            available: true,
        }
    }
}

/// Name-based speed hint.
pub fn sounds_fast(lower_id: &str) -> bool {
    [
        "flash",
        "fast",
        "mini",
        "lite",
        "haiku",
        "lightning",
        "highspeed",
        "small",
    ]
    .iter()
    .any(|w| lower_id.contains(w))
}

fn parse_lines(text: &str) -> Result<Vec<RawModel>, String> {
    let models: Vec<RawModel> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|token| {
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./:@".contains(&b))
        })
        .map(|id| RawModel::new(id.to_owned()))
        .collect();
    if models.is_empty() {
        return Err("model listing is empty".to_owned());
    }
    Ok(models)
}

fn parse_json(text: &str) -> Result<Vec<RawModel>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("model listing is not JSON: {e}"))?;
    let items = value
        .as_array()
        .or_else(|| value["data"].as_array())
        .or_else(|| value["models"].as_array())
        .ok_or("model listing has no array (top level, data[] or models[])")?;
    let models: Vec<RawModel> = items
        .iter()
        .filter_map(|item| {
            let id = item.as_str().map(str::to_owned).or_else(|| {
                ["id", "model", "name"]
                    .iter()
                    .find_map(|k| item[*k].as_str().map(str::to_owned))
            })?;
            Some(RawModel {
                label: item["name"]
                    .as_str()
                    .filter(|n| *n != id)
                    .map(str::to_owned),
                context_tokens: item["context_length"]
                    .as_u64()
                    .or_else(|| item["context_window"].as_u64()),
                ..RawModel::new(id)
            })
        })
        .collect();
    if models.is_empty() {
        return Err("model listing is empty".to_owned());
    }
    Ok(models)
}

pub fn parse(format: DiscoverFormat, text: &str) -> Result<Vec<RawModel>, String> {
    match format {
        DiscoverFormat::Lines => parse_lines(text),
        DiscoverFormat::Json => parse_json(text),
        DiscoverFormat::DevinJson => super::devin::parse_models(text),
        DiscoverFormat::OpencodeVerbose => super::opencode::parse_models(text),
        DiscoverFormat::CommandCodeText => super::commandcode::parse_models(text),
    }
}

/// Billing for `model` on `spec`: configured rule, then a price the
/// listing stated, then the executor default (itself `unknown` unless
/// declared). A zero price alone does not make a model free — local,
/// plan-bundled and free-tier models all list as zero — so it only counts
/// as a hint that the model is not metered.
pub fn classify(spec: &ExecutorSpec, raw: &RawModel) -> (AgentBilling, String) {
    let names = std::iter::once(&raw.id).chain(&raw.aliases);
    for rule in &spec.billing {
        if names.clone().any(|n| glob_match(&rule.models, n)) {
            return (rule.billing, format!("rule:{}", rule.models));
        }
    }
    if let Some((input, output)) = raw.discovered_usd_per_mtok
        && (input > 0.0 || output > 0.0)
    {
        return (AgentBilling::Metered, "discovered_price".to_owned());
    }
    (spec.default_billing, "executor_default".to_owned())
}

pub fn describe(spec: &ExecutorSpec, raw: RawModel, now: u64) -> ModelDescriptor {
    let (billing, billing_source) = classify(spec, &raw);
    let lower = raw.id.to_lowercase();
    let capabilities = raw.capabilities.unwrap_or(ModelCapabilities {
        // Every executor is a coding/tool-using agent harness.
        coding: true,
        tools: true,
        reasoning: false,
        vision: false,
        long_context: raw.context_tokens.is_some_and(|c| c >= 200_000),
        fast: sounds_fast(&lower),
    });
    ModelDescriptor {
        id: ModelRef::new(&spec.name, &raw.id),
        aliases: raw.aliases,
        label: raw.label,
        capabilities,
        billing,
        billing_source,
        context_tokens: raw.context_tokens,
        price_note: raw.price_note,
        usd_per_mtok: raw.discovered_usd_per_mtok,
        available: raw.available,
        discovered_at: now,
    }
}

/// One executor's last discovery.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Unix seconds of the last attempt.
    pub checked_at: u64,
    /// Unix seconds of the last successful discovery.
    pub discovered_at: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// Harness version string, when it reports one.
    #[serde(default)]
    pub harness_version: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
}

impl CatalogEntry {
    pub fn is_stale(&self, ttl_secs: u64, now: u64) -> bool {
        self.checked_at == 0 || now.saturating_sub(self.checked_at) >= ttl_secs
    }

    /// Models by id or alias.
    pub fn find(&self, name: &str) -> Option<&ModelDescriptor> {
        self.models.iter().find(|m| m.answers_to(name))
    }
}

/// All executors' catalogs, persisted as one JSON file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCatalog {
    #[serde(default)]
    pub executors: BTreeMap<String, CatalogEntry>,
}

impl ModelCatalog {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    /// Drop executors that are no longer configured.
    pub fn retain(&mut self, names: &[&str]) {
        self.executors.retain(|k, _| names.contains(&k.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_exec::config::ExecutorConfig;

    fn spec(json: serde_json::Value) -> ExecutorSpec {
        serde_json::from_value::<ExecutorConfig>(json)
            .expect("config")
            .resolve()
            .expect("resolves")
    }

    #[test]
    fn billing_prefers_rules_then_prices_and_never_assumes_free() {
        let s = spec(serde_json::json!({
            "name": "opencode", "adapter": "opencode",
            "billing": [{"models": "opencode/*-free", "billing": "free_tier"},
                        {"models": "lmstudio/*", "billing": "local"}]
        }));
        let with_price = |id: &str, price: Option<(f64, f64)>| RawModel {
            discovered_usd_per_mtok: price,
            ..RawModel::new(id.to_owned())
        };
        assert_eq!(
            classify(&s, &with_price("opencode/mimo-v2.5-free", Some((0.0, 0.0)))).0,
            AgentBilling::FreeTier
        );
        assert_eq!(
            classify(&s, &with_price("lmstudio/qwen3", None)).0,
            AgentBilling::Local
        );
        assert_eq!(
            classify(
                &s,
                &with_price("anthropic/claude-opus-5-5", Some((4.0, 20.0)))
            )
            .0,
            AgentBilling::Metered
        );
        // A zero price with no rule is not evidence of "free".
        let (billing, source) = classify(&s, &with_price("opencode/big-pickle", Some((0.0, 0.0))));
        assert_eq!(billing, AgentBilling::Unknown);
        assert_eq!(source, "executor_default");
    }

    #[test]
    fn rules_match_aliases_too() {
        let s = spec(serde_json::json!({
            "name": "devin", "adapter": "devin_cli", "default_billing": "subscription",
            "billing": [{"models": "opus", "billing": "included_credit"}]
        }));
        let raw = RawModel {
            aliases: vec!["opus".to_owned()],
            ..RawModel::new("claude-opus-5.5".to_owned())
        };
        assert_eq!(classify(&s, &raw).0, AgentBilling::IncludedCredit);
        assert_eq!(
            classify(&s, &RawModel::new("swe-2".to_owned())).0,
            AgentBilling::Subscription
        );
    }

    #[test]
    fn generic_formats() {
        let lines = parse(DiscoverFormat::Lines, "model-a  desc\n\nmodel-b\n").expect("lines");
        assert_eq!(lines.len(), 2);
        let json = parse(
            DiscoverFormat::Json,
            r#"{"data": [{"id": "x", "context_length": 8192}, "y"]}"#,
        )
        .expect("json");
        assert_eq!(json[0].context_tokens, Some(8192));
        assert_eq!(json[1].id, "y");
        assert!(parse(DiscoverFormat::Json, "{}").is_err());
    }

    #[test]
    fn catalog_round_trips_and_forgets_removed_executors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("catalog.json");
        let s = spec(serde_json::json!({"name": "oc", "adapter": "opencode"}));
        let mut catalog = ModelCatalog::default();
        catalog.executors.insert(
            "oc".to_owned(),
            CatalogEntry {
                checked_at: 10,
                discovered_at: 10,
                models: vec![describe(&s, RawModel::new("a/b".to_owned()), 10)],
                ..CatalogEntry::default()
            },
        );
        catalog
            .executors
            .insert("gone".to_owned(), CatalogEntry::default());
        catalog.save(&path).expect("save");
        let mut loaded = ModelCatalog::load(&path);
        assert_eq!(loaded, catalog);
        loaded.retain(&["oc"]);
        assert_eq!(loaded.executors.len(), 1);
        let entry = &loaded.executors["oc"];
        assert!(entry.find("a/b").is_some());
        assert!(!entry.is_stale(100, 50));
        assert!(entry.is_stale(100, 200));
    }
}
