//! Shared configuration setup for interactive dialogue frontends.
//!
//! The command-line and native desktop surfaces must resolve the same
//! operator-provided language-organ environment.  Keeping that resolution in
//! the runtime crate prevents a frontend from silently inventing a different
//! provider contract.

use std::collections::BTreeMap;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::LocalityClass;

use crate::llm_jev::{
    DEFAULT_GEMMA_BASE_URL, DEFAULT_HAI_BASE_URL, GEMMA_12B_MODEL, GEMMA_API_KEY_ENV,
    GEMMA_PROVIDER_ID, GROKBOT_API_KEY_ENV, HAI_API_KEY_ENV, HAI_LLM_JP_MODEL,
    HAI_LLM_JP_PROVIDER_ID, HAI_QWEN_MODEL, HAI_QWEN_PROVIDER_ID, LLM_AUTH_ENV_ENV,
    LLM_BASE_URL_ENV, LLM_MODEL_ENV, LLM_PROVIDER_ENV, TypesafeConfig,
};
use crate::{
    PersonaBackendKind, PersonaProviderConfig, PersonaSetting, RuntimeConfig, RuntimeError,
};

/// A stable backend id derived from the endpoint and model, never from a
/// credential.  The same configuration therefore keeps the same attribution
/// across CLI and desktop launches.
pub fn persona_backend_id_for(base_url: &str, model: &str) -> PersonaBackendId {
    let digest = content_digest(format!("{base_url}|{model}").as_bytes());
    let bytes: Vec<u8> = digest
        .trim_start_matches("sha256:")
        .as_bytes()
        .chunks(2)
        .take(16)
        .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    let mut raw = [0_u8; 16];
    raw[..bytes.len().min(16)].copy_from_slice(&bytes[..bytes.len().min(16)]);
    let value = u128::from_be_bytes(raw);
    PersonaBackendId::from_u128(if value == 0 { 1 } else { value })
}

/// Return only the public origin for UI/decision metadata. Paths, queries,
/// fragments and URL userinfo are never shown to a model or operator panel.
pub fn public_endpoint_origin(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return "<configured endpoint>".to_owned();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if scheme.is_empty() || authority.is_empty() {
        "<configured endpoint>".to_owned()
    } else {
        format!("{scheme}://{authority}")
    }
}

/// The two HAI language organs declared by the operator's OpenCode setup.
/// They are exposed only when the named credential is present in the process
/// environment; the key value is never read into configuration or telemetry.
pub fn configured_hai_language_providers() -> BTreeMap<String, PersonaProviderConfig> {
    if !environment_value_available(HAI_API_KEY_ENV) {
        return BTreeMap::new();
    }
    hai_language_providers()
}

/// Register available HAI presets without generating twice with a HAI primary.
/// Only unchanged auto-registered duplicates are removed; operator overrides
/// under a preset ID are retained.
pub fn register_hai_language_providers(config: &mut RuntimeConfig) -> Result<bool, RuntimeError> {
    let presets = hai_language_providers();
    if TypesafeConfig::from_env().is_none() || !environment_value_available(HAI_API_KEY_ENV) {
        return Ok(remove_unchanged_language_presets(config, &presets));
    }
    apply_language_presets(config, presets)
}

fn remove_unchanged_language_presets(
    config: &mut RuntimeConfig,
    presets: &BTreeMap<String, PersonaProviderConfig>,
) -> bool {
    let before = config.language_providers.len();
    config
        .language_providers
        .retain(|id, configured| presets.get(id).is_none_or(|preset| configured != preset));
    config.language_providers.len() != before
}

fn apply_language_presets(
    config: &mut RuntimeConfig,
    presets: BTreeMap<String, PersonaProviderConfig>,
) -> Result<bool, RuntimeError> {
    let mut changed = false;
    for (id, provider) in presets {
        let is_primary = config.persona.provider.as_ref().is_some_and(|primary| {
            primary.base_url.trim_end_matches('/') == provider.base_url.trim_end_matches('/')
                && primary.model == provider.model
                && primary.auth_env == provider.auth_env
        });
        if is_primary {
            if config.language_providers.get(&id) == Some(&provider) {
                config.remove_language_provider(&id);
                changed = true;
            }
        } else if !config.language_providers.contains_key(&id) {
            config.set_language_provider(&id, provider)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Return the non-secret HAI model declarations independent of credential
/// availability. Keeping this pure makes the model list testable without a
/// live API or a secret.
pub fn hai_language_providers() -> BTreeMap<String, PersonaProviderConfig> {
    [
        (HAI_QWEN_PROVIDER_ID, HAI_QWEN_MODEL),
        (HAI_LLM_JP_PROVIDER_ID, HAI_LLM_JP_MODEL),
    ]
    .into_iter()
    .map(|(id, model)| {
        (
            id.to_owned(),
            PersonaProviderConfig {
                backend_id: persona_backend_id_for(DEFAULT_HAI_BASE_URL, model),
                locality: LocalityClass::External,
                base_url: DEFAULT_HAI_BASE_URL.to_owned(),
                model: model.to_owned(),
                auth_env: Some(HAI_API_KEY_ENV.to_owned()),
                timeout_ms: 60_000,
                tls_root_ca_path: None,
                system_instruction: None,
                reasoning: Default::default(),
                extra_body: None,
            },
        )
    })
    .collect()
}

pub fn environment_value_available(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn language_provider_auth_available(provider: &PersonaProviderConfig) -> bool {
    provider
        .auth_env
        .as_deref()
        .is_none_or(environment_value_available)
}

/// Preserve the operator-facing provider label without adding another secret
/// bearing field to the persisted PersonaProviderConfig schema.
pub fn language_provider_kind(provider: &PersonaProviderConfig) -> &'static str {
    if provider.auth_env.as_deref() == Some(HAI_API_KEY_ENV)
        && provider.base_url.trim_end_matches('/') == DEFAULT_HAI_BASE_URL.trim_end_matches('/')
    {
        "hai"
    } else if provider.auth_env.as_deref() == Some(GEMMA_API_KEY_ENV) {
        "gemma"
    } else {
        "openai-compatible"
    }
}

pub fn gemma_language_providers() -> BTreeMap<String, PersonaProviderConfig> {
    let mut map = BTreeMap::new();
    map.insert(
        GEMMA_PROVIDER_ID.to_owned(),
        PersonaProviderConfig {
            backend_id: persona_backend_id_for(DEFAULT_GEMMA_BASE_URL, GEMMA_12B_MODEL),
            locality: LocalityClass::LocalHost,
            base_url: DEFAULT_GEMMA_BASE_URL.to_owned(),
            model: GEMMA_12B_MODEL.to_owned(),
            auth_env: Some(GEMMA_API_KEY_ENV.to_owned()),
            timeout_ms: 60_000,
            tls_root_ca_path: None,
            system_instruction: None,
            reasoning: Default::default(),
            extra_body: None,
        },
    );
    map
}

/// Resolve the language-organ environment into the existing Persona setting.
/// Only the credential variable name is retained; the credential value is
/// never copied into runtime configuration.
pub fn persona_setting_from_environment(
    config: &RuntimeConfig,
) -> Result<Option<PersonaSetting>, RuntimeError> {
    let provider = match std::env::var(LLM_PROVIDER_ENV) {
        Ok(value) if !value.trim().is_empty() => value.to_ascii_lowercase(),
        _ => return Ok(None),
    };
    if provider == "mock" {
        return Ok(None);
    }
    if !matches!(
        provider.as_str(),
        "grokbot" | "hai" | "gemma" | "gemma-4-12b" | "openai-compatible"
    ) {
        return Err(RuntimeError::Usage(format!(
            "unsupported {LLM_PROVIDER_ENV} value {provider:?}"
        )));
    }
    let base_url = std::env::var(LLM_BASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::Usage(format!("{LLM_BASE_URL_ENV} is required")))?;
    let model = std::env::var(LLM_MODEL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::Usage(format!("{LLM_MODEL_ENV} is required")))?;
    let existing = config.persona.provider.as_ref();
    let explicit_auth_env = std::env::var(LLM_AUTH_ENV_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let auth_env = provider_auth_env(&provider, explicit_auth_env.as_deref())?;
    Ok(Some(PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: persona_backend_id_for(&base_url, &model),
            locality: LocalityClass::External,
            base_url,
            model,
            auth_env,
            timeout_ms: existing.map_or(60_000, |provider| provider.timeout_ms),
            tls_root_ca_path: existing.and_then(|provider| provider.tls_root_ca_path.clone()),
            system_instruction: existing.and_then(|provider| provider.system_instruction.clone()),
            reasoning: existing.map_or_else(Default::default, |provider| provider.reasoning),
            extra_body: existing.and_then(|provider| provider.extra_body.clone()),
        }),
        seed: config.persona.seed.clone(),
    }))
}

fn provider_auth_env(
    provider: &str,
    explicit_auth_env: Option<&str>,
) -> Result<Option<String>, RuntimeError> {
    let value = match provider {
        "grokbot" => Some(GROKBOT_API_KEY_ENV.to_owned()),
        "hai" => Some(HAI_API_KEY_ENV.to_owned()),
        "gemma" | "gemma-4-12b" => Some(GEMMA_API_KEY_ENV.to_owned()),
        _ => explicit_auth_env.map(str::trim).map(str::to_owned),
    };
    if let Some(name) = &value
        && (name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    {
        return Err(RuntimeError::Usage(format!(
            "{LLM_AUTH_ENV_ENV} must name an environment variable using ASCII letters, digits or '_'"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hai_presets_declare_the_two_requested_models_without_credentials() {
        let providers = hai_language_providers();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[HAI_QWEN_PROVIDER_ID].model, HAI_QWEN_MODEL);
        assert_eq!(providers[HAI_LLM_JP_PROVIDER_ID].model, HAI_LLM_JP_MODEL);
        assert!(providers.values().all(|provider| {
            provider.base_url == DEFAULT_HAI_BASE_URL
                && provider.auth_env.as_deref() == Some(HAI_API_KEY_ENV)
                && provider.locality == LocalityClass::External
        }));
    }

    #[test]
    fn hai_provider_kind_is_secret_free_and_stable() {
        let provider = hai_language_providers()
            .remove(HAI_QWEN_PROVIDER_ID)
            .unwrap();
        assert_eq!(language_provider_kind(&provider), "hai");
        assert!(!language_provider_auth_available(&provider));
    }

    #[test]
    fn hai_primary_registers_only_the_other_model_and_removes_its_old_preset() {
        let mut config = RuntimeConfig::new(
            kamimusuhi_core::ids::NodeId::from_u128(1),
            crate::ResourceImplementation::FakeA,
        );
        let presets = hai_language_providers();
        config.persona.backend = PersonaBackendKind::OpenaiCompatible;
        config.persona.provider = Some(presets[HAI_LLM_JP_PROVIDER_ID].clone());
        config.language_providers = presets.clone();
        assert!(apply_language_presets(&mut config, presets.clone()).unwrap());
        assert_eq!(config.language_providers.len(), 1);
        assert!(config.language_providers.contains_key(HAI_QWEN_PROVIDER_ID));
        assert!(!apply_language_presets(&mut config, presets.clone()).unwrap());
        config.language_providers.clear();
        assert!(apply_language_presets(&mut config, presets).unwrap());
        assert_eq!(config.language_providers.len(), 1);
    }

    #[test]
    fn hai_preset_registration_preserves_operator_overrides() {
        let mut config = RuntimeConfig::new(
            kamimusuhi_core::ids::NodeId::from_u128(1),
            crate::ResourceImplementation::FakeA,
        );
        let presets = hai_language_providers();
        config.persona.provider = Some(presets[HAI_LLM_JP_PROVIDER_ID].clone());
        let mut customized = presets[HAI_LLM_JP_PROVIDER_ID].clone();
        customized.system_instruction = Some("operator customization".to_owned());
        config
            .language_providers
            .insert(HAI_LLM_JP_PROVIDER_ID.to_owned(), customized.clone());
        apply_language_presets(&mut config, presets).unwrap();
        assert_eq!(
            config.language_providers[HAI_LLM_JP_PROVIDER_ID],
            customized
        );
    }

    #[test]
    fn provider_credentials_are_explicit_and_never_inherited() {
        assert_eq!(
            provider_auth_env("hai", Some("SHOULD_NOT_BE_USED"))
                .unwrap()
                .as_deref(),
            Some(HAI_API_KEY_ENV)
        );
        assert_eq!(
            provider_auth_env("grokbot", Some("SHOULD_NOT_BE_USED"))
                .unwrap()
                .as_deref(),
            Some(GROKBOT_API_KEY_ENV)
        );
        assert_eq!(
            provider_auth_env("openai-compatible", Some("OPENAI_API_KEY"))
                .unwrap()
                .as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(provider_auth_env("openai-compatible", None).unwrap(), None);
    }
}
