//! Devin CLI preset and model-list parser.
//!
//! First stage is a subprocess per task (`devin -p`). A long-lived
//! `devin acp` session is the intended successor; it will be another
//! output format, not another executor type.

use serde_json::Value;

use super::catalog::RawModel;
use super::config::{
    AcpSpec, DiscoverFormat, DiscoverSpec, OutputFormat, PermissionArgs, PermissionModes, Preset,
    RunSpec, strings,
};
use super::types::{ModelCapabilities, TaskKind};

pub fn preset() -> Preset {
    Preset {
        command: "devin",
        discover: Some(DiscoverSpec {
            args: strings(&["models", "list", "--format", "json"]),
            format: DiscoverFormat::DevinJson,
        }),
        run: Some(RunSpec {
            prefix: Vec::new(),
            args: strings(&["-p", "{prompt}"]),
            model_args: strings(&["--model", "{model}"]),
            // Print mode reports no session id, so continuing a task needs
            // the ACP protocol.
            resume_args: Vec::new(),
            output: OutputFormat::Text,
        }),
        // `auto` approves read-only tools only; anything else would prompt,
        // and print mode cannot prompt, so it is refused.
        permission_args: PermissionArgs {
            read_only: Some(strings(&["--permission-mode", "auto"])),
            workspace_write: Some(strings(&["--permission-mode", "accept-edits"])),
            autonomous_workspace: Some(strings(&["--permission-mode", "dangerous"])),
        },
        prefer_for: vec![TaskKind::Coding, TaskKind::Debug, TaskKind::Review],
        // `devin acp`: the model is fixed per server (`--model`); sessions
        // start in `accept-edits`, so the mode is always set explicitly.
        acp: Some(AcpSpec {
            args: strings(&["acp"]),
            model_args: strings(&["--model", "{model}"]),
            model_option: None,
            mode_option: "mode".to_owned(),
            modes: PermissionModes {
                read_only: Some("ask".to_owned()),
                workspace_write: Some("accept-edits".to_owned()),
                autonomous_workspace: Some("bypass".to_owned()),
            },
            idle_secs: 900,
        }),
        server: None,
    }
}

/// `{"families": [{"slug", "family_label", "aliases", "variants": [...]}]}`.
/// One model per family, addressed by its slug; variants are the same
/// model at other effort levels and are summarised, not listed.
pub fn parse_models(text: &str) -> Result<Vec<RawModel>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("devin models: not JSON: {e}"))?;
    let families = value["families"]
        .as_array()
        .ok_or("devin models: no families[]")?;
    Ok(families
        .iter()
        .filter_map(|family| {
            let slug = family["slug"].as_str()?.to_owned();
            let variants = family["variants"].as_array().cloned().unwrap_or_default();
            let context = variants
                .iter()
                .filter_map(|v| v["max_context_tokens"].as_u64())
                .max();
            let first = variants.first();
            let price_note = first.and_then(|v| {
                let tier = v["cost_tier"].as_str().unwrap_or("");
                let summary = v["cost_summary"].as_str().unwrap_or("");
                let note = [tier, summary]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · ");
                (!note.is_empty()).then_some(note)
            });
            let mut aliases: Vec<String> = family["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect();
            if let Some(uid) = family["family_uid"].as_str()
                && uid != slug
            {
                aliases.push(uid.to_owned());
            }
            Some(RawModel {
                id: slug,
                aliases,
                label: family["family_label"].as_str().map(str::to_owned),
                context_tokens: context,
                price_note,
                // Devin bills against the account plan; per-token list
                // prices describe the model, not what this account pays.
                discovered_usd_per_mtok: None,
                capabilities: None::<ModelCapabilities>,
                variants: variants.len(),
                available: true,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"families": [
      {"family_label": "Claude Opus 5.5", "family_uid": "claude-opus-5-5",
       "slug": "claude-opus-5.5", "aliases": ["opus"],
       "variants": [
         {"model_uid": "claude-opus-5-5-medium", "max_context_tokens": 1000000,
          "cost_tier": "High cost", "cost_summary": "$4 / MTok In · $20 / MTok Out"},
         {"model_uid": "claude-opus-5-5-low", "max_context_tokens": 1000000}]},
      {"family_label": "SWE-2", "family_uid": "swe-2", "slug": "swe-2", "aliases": ["swe"],
       "variants": [{"model_uid": "swe-2", "max_context_tokens": 200000}]}
    ]}"#;

    #[test]
    fn families_become_models_with_aliases() {
        let models = parse_models(SAMPLE).expect("parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-opus-5.5");
        assert_eq!(models[0].aliases, vec!["opus", "claude-opus-5-5"]);
        assert_eq!(models[0].context_tokens, Some(1_000_000));
        assert_eq!(models[0].variants, 2);
        assert!(
            models[0]
                .price_note
                .as_deref()
                .unwrap_or("")
                .contains("High cost")
        );
        assert!(models[0].discovered_usd_per_mtok.is_none());
        assert_eq!(models[1].aliases, vec!["swe"]);
        assert!(parse_models("not json").is_err());
    }
}
