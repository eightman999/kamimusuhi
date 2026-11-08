//! FI3: a configuration identity gate, separate from canonical individual identity.
//!
//! A manifest attests declarations and locally compiled prompt implementation,
//! not the weights behind a remote model alias. Credentials are never captured.
//! Read-only status never enrolls an existing individual. A rejected dispatch
//! may persist a pending review; only explicit, compare-and-swap acceptance can
//! advance the baseline. The caller must also compare any warm session snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::ids::{CommitId, IndividualId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{PersonaProviderConfig, RuntimeConfig};

pub const MANIFEST_FILE: &str = "identity-manifest.json";
pub const PENDING_FILE: &str = "identity-pending.json";
const LOCK_FILE: &str = "identity.lock";
const FORMAT_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 1024 * 1024;
const ATTESTATION: &str = "declared_configuration_only";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub individual_id: IndividualId,
    pub root_commit_id: CommitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub binding: Binding,
    /// Values are content digests, never prompt text or credentials.
    pub components: BTreeMap<String, String>,
    /// Digests of accepted configured (endpoint, model) declarations.
    pub declared_models: BTreeSet<String>,
    /// Configured model names, including host-attested proxy targets. Names
    /// remain private; only raw-byte model-name digests are persisted.
    pub allowed_model_hashes: BTreeSet<String>,
    /// Explicitly refuses to imply verification of remote model weights.
    pub model_attestation: String,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Continue,
    AcceptRequired,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateStatus {
    pub state: GateState,
    pub candidate_digest: String,
    pub accepted_digest: Option<String>,
    /// Binds both candidate and baseline. Passing only a candidate digest is
    /// insufficient when another operator accepted something in the meantime.
    pub review_token: String,
    pub changed_components: Vec<String>,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error("identity manifest is invalid: {0}")]
    Invalid(&'static str),
    #[error("identity manifest storage operation failed")]
    Storage,
    #[error("identity change requires explicit acceptance (review token {review_token})")]
    AcceptanceRequired { review_token: String },
    #[error("identity continuation is blocked: {0}")]
    Blocked(&'static str),
    #[error("identity review is stale; inspect the current manifest again")]
    StaleReview,
}

impl Manifest {
    pub fn capture(config: &RuntimeConfig, binding: Binding) -> Result<Self, GateError> {
        Self::capture_with_declarations(config, binding, Value::Null)
    }

    /// The host supplies a secret-free declaration of proxy routing targets.
    /// An opaque digest detects changes; the host also validates actual model
    /// observations against that declaration before accepting a response.
    pub fn capture_with_declarations(
        config: &RuntimeConfig,
        binding: Binding,
        mut declarations: Value,
    ) -> Result<Self, GateError> {
        reject_secret_keys(&declarations)?;
        let mut allowed_model_hashes = BTreeSet::new();
        if let Some(version) = declarations.get("version")
            && version != &json!(1)
        {
            return Err(GateError::Invalid("UNSUPPORTED_ROUTE_DECLARATION_VERSION"));
        }
        if let Some(values) = declarations.get_mut("model_hashes") {
            let array = values
                .as_array_mut()
                .ok_or(GateError::Invalid("INVALID_ROUTE_MODEL_HASHES"))?;
            for value in array.iter() {
                let digest = value
                    .as_str()
                    .filter(|s| valid_digest(s))
                    .ok_or(GateError::Invalid("INVALID_ROUTE_MODEL_HASHES"))?;
                allowed_model_hashes.insert(digest.to_owned());
            }
            array.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            array.dedup();
        }
        if let Some(pins) = declarations.get("route_pins") {
            for (endpoint, digest) in pins
                .as_object()
                .ok_or(GateError::Invalid("INVALID_ROUTE_PINS"))?
            {
                endpoint_identity(endpoint)?;
                if !digest.as_str().is_some_and(valid_digest) {
                    return Err(GateError::Invalid("INVALID_ROUTE_PINS"));
                }
            }
        }
        let mut components = BTreeMap::new();
        let mut models = BTreeSet::new();
        insert(&mut components, "proxy_routing_declarations", &declarations);
        let typesafe_url = std::env::var(crate::llm_jev::TYPESAFE_BASE_URL_ENV)
            .unwrap_or_else(|_| crate::llm_jev::DEFAULT_TYPESAFE_BASE_URL.to_owned());
        let typesafe_model = std::env::var(crate::llm_jev::TYPESAFE_MODEL_ENV)
            .unwrap_or_else(|_| crate::llm_jev::DEFAULT_TYPESAFE_MODEL.to_owned());
        insert(
            &mut components,
            "decision_declaration",
            &json!({
                "allowed_kinds":["rule_based","typesafe","persona_api"],
                "selection": config.decision,
                "api_provider": config.persona.provider.as_ref().map(provider_value).transpose()?,
                "endpoint":endpoint_identity(&typesafe_url)?,"model":typesafe_model,
            }),
        );
        models.insert(model_declaration(&typesafe_url, &typesafe_model)?);
        allowed_model_hashes.insert(content_digest(typesafe_model.as_bytes()));
        insert(
            &mut components,
            "language_adapter_kind",
            &json!(
                std::env::var(crate::llm_jev::LLM_PROVIDER_ENV)
                    .unwrap_or_else(|_| "openai-compatible".to_owned())
            ),
        );
        insert(
            &mut components,
            "runtime_schema",
            &json!(config.config_version),
        );
        insert(
            &mut components,
            "persona_backend",
            &json!(config.persona.backend),
        );
        let seed = config
            .persona
            .seed
            .resolve()
            .map_err(|_| GateError::Invalid("INVALID_PERSONA_SEED"))?;
        insert(
            &mut components,
            "persona_seed",
            &serde_json::to_value(seed).map_err(|_| GateError::Invalid("SEED_ENCODING"))?,
        );
        insert(
            &mut components,
            "avatar_framing",
            &json!({
                "name": config.avatar_name, "aliases": config.avatar_aliases,
                "self_description": config.avatar_self_description,
            }),
        );
        if let Some(provider) = &config.persona.provider {
            let value = provider_value(provider)?;
            models.insert(model_declaration(&provider.base_url, &provider.model)?);
            allowed_model_hashes.insert(content_digest(provider.model.as_bytes()));
            insert(&mut components, "language.primary", &value);
        }
        for (id, provider) in &config.language_providers {
            let value = provider_value(provider)?;
            models.insert(model_declaration(&provider.base_url, &provider.model)?);
            allowed_model_hashes.insert(content_digest(provider.model.as_bytes()));
            insert(&mut components, &format!("language.{id}"), &value);
        }
        insert(
            &mut components,
            "resource_implementations",
            &json!(config.resources),
        );
        for (slot, provider) in &config.providers {
            insert(
                &mut components,
                &format!("resource.{slot}"),
                &json!({
                    "endpoint":endpoint_identity(&provider.base_url)?, "model":provider.model,
                    "capabilities":provider.capabilities,
                }),
            );
            models.insert(model_declaration(&provider.base_url, &provider.model)?);
            allowed_model_hashes.insert(content_digest(provider.model.as_bytes()));
        }
        insert(
            &mut components,
            "reflector_backend",
            &json!(config.c0.reflector.backend),
        );
        insert(
            &mut components,
            "idle_cognition_policy",
            &json!(config.c0.idle),
        );
        components.insert(
            "compiled_idle_cognition_implementation".to_owned(),
            content_digest(include_bytes!("idle_cognition.rs")),
        );
        components.insert(
            "compiled_reflection_implementation".to_owned(),
            content_digest(include_bytes!("c0/reflection.rs")),
        );
        components.insert(
            "compiled_replay_implementation".to_owned(),
            content_digest(include_bytes!("c0/replay.rs")),
        );
        if let Some(provider) = &config.c0.reflector.provider {
            insert(
                &mut components,
                "reflector_provider",
                &provider_value(provider)?,
            );
            models.insert(model_declaration(&provider.base_url, &provider.model)?);
            allowed_model_hashes.insert(content_digest(provider.model.as_bytes()));
        }
        insert(
            &mut components,
            "tools",
            &match &config.tools {
                Some(tools) => {
                    let mut allowed = tools.allowed.clone();
                    allowed.sort();
                    allowed.dedup();
                    let core = tools.core.as_ref().map(|v| {
                        let mut v = v.clone();
                        v.sort();
                        v.dedup();
                        v
                    });
                    json!({"endpoint": endpoint_identity(&tools.base_url)?, "allowed":allowed,"core":core,"max_rounds":tools.max_rounds})
                }
                None => Value::Null,
            },
        );
        insert(
            &mut components,
            "genome_exploration",
            &json!(config.genome_exploration),
        );
        // These sources contain conditional prompt additions/rendering as well
        // as constants. Conservative implementation fingerprinting also marks
        // non-prompt edits for review; it cannot silently miss a template edit.
        components.insert(
            "compiled_persona_prompt_implementation".to_owned(),
            content_digest(include_bytes!("../../kamimusuhi-persona-http/src/lib.rs")),
        );
        components.insert(
            "compiled_tool_prompt_implementation".to_owned(),
            content_digest(include_bytes!("../../kamimusuhi-persona-http/src/tools.rs")),
        );
        components.insert(
            "compiled_decision_prompt_implementation".to_owned(),
            content_digest(include_bytes!("llm_jev/assessment.rs")),
        );
        components.insert(
            "compiled_decision_provider_implementation".to_owned(),
            content_digest(include_bytes!("llm_jev.rs")),
        );
        components.insert(
            "compiled_api_decision_implementation".to_owned(),
            content_digest(include_bytes!("llm_jev/api.rs")),
        );
        components.insert(
            "compiled_prompt_schema".to_owned(),
            content_digest(include_bytes!("../../kamimusuhi-core/src/persona.rs")),
        );
        let mut result = Self {
            format_version: FORMAT_VERSION,
            binding,
            components,
            declared_models: models,
            allowed_model_hashes,
            model_attestation: ATTESTATION.to_owned(),
            digest: String::new(),
        };
        result.digest = result.calculated_digest();
        result.validate()?;
        Ok(result)
    }

    pub fn allows_model_name(&self, model: &str) -> bool {
        self.allowed_model_hashes
            .contains(&content_digest(model.as_bytes()))
    }

    pub fn declares_model(&self, endpoint: &str, model: &str) -> Result<bool, GateError> {
        Ok(self
            .declared_models
            .contains(&model_declaration(endpoint, model)?))
    }

    pub fn validate(&self) -> Result<(), GateError> {
        if self.format_version != FORMAT_VERSION || self.model_attestation != ATTESTATION {
            return Err(GateError::Invalid("UNSUPPORTED_MANIFEST_VERSION"));
        }
        if self.binding.individual_id.is_nil() || self.binding.root_commit_id.is_nil() {
            return Err(GateError::Invalid("INVALID_INDIVIDUAL_BINDING"));
        }
        if self.components.is_empty()
            || self.components.len() > 128
            || self.declared_models.len() > 128
            || self.allowed_model_hashes.len() > 128
            || self
                .components
                .iter()
                .any(|(k, v)| k.len() > 160 || !valid_digest(v))
            || self.declared_models.iter().any(|v| !valid_digest(v))
            || self.allowed_model_hashes.iter().any(|v| !valid_digest(v))
            || !valid_digest(&self.digest)
            || self.digest != self.calculated_digest()
        {
            return Err(GateError::Invalid("MANIFEST_DIGEST_MISMATCH"));
        }
        Ok(())
    }

    fn calculated_digest(&self) -> String {
        json_digest(
            &json!({"format_version":self.format_version,"binding":self.binding,
            "components":self.components,"declared_models":self.declared_models,
            "allowed_model_hashes":self.allowed_model_hashes,"model_attestation":self.model_attestation}),
        )
    }
}

fn insert(map: &mut BTreeMap<String, String>, name: &str, value: &Value) {
    map.insert(name.to_owned(), json_digest(value));
}

fn provider_value(p: &PersonaProviderConfig) -> Result<Value, GateError> {
    if let Some(extra) = &p.extra_body {
        validate_extra(&Value::Object(extra.clone()))?;
    }
    Ok(
        json!({"endpoint":endpoint_identity(&p.base_url)?,"model":p.model,
        "locality":p.locality,"system_instruction":p.system_instruction.as_deref().unwrap_or(kamimusuhi_persona_http::DEFAULT_SYSTEM_INSTRUCTION),
        "reasoning":p.reasoning,"extra_body":p.extra_body}),
    )
}

fn model_declaration(endpoint: &str, model: &str) -> Result<String, GateError> {
    Ok(json_digest(
        &json!({"endpoint":endpoint_identity(endpoint)?,"model":model}),
    ))
}

fn endpoint_identity(endpoint: &str) -> Result<&str, GateError> {
    let (_, rest) = endpoint
        .split_once("://")
        .ok_or(GateError::Invalid("INVALID_PROVIDER_ENDPOINT"))?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || endpoint.contains('?')
        || endpoint.contains('#')
    {
        return Err(GateError::Invalid("CREDENTIAL_BEARING_PROVIDER_ENDPOINT"));
    }
    Ok(endpoint.trim_end_matches('/'))
}

fn validate_extra(value: &Value) -> Result<(), GateError> {
    const ALLOWED: &[&str] = &[
        "temperature",
        "top_p",
        "top_k",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "max_tokens",
        "max_completion_tokens",
        "reasoning_effort",
        "chat_template_kwargs",
        "stop",
        "min_p",
        "repeat_penalty",
        "enable_thinking",
        "thinking_budget",
        "response_format",
        "reasoning",
        "thinking",
    ];
    let Some(map) = value.as_object() else {
        return Err(GateError::Invalid("INVALID_PROVIDER_EXTRA_BODY"));
    };
    for (key, value) in map {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(GateError::Invalid("UNCLASSIFIED_PROVIDER_FIELD"));
        }
        reject_secret_keys(value)?;
    }
    Ok(())
}
fn reject_secret_keys(value: &Value) -> Result<(), GateError> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let k = key.to_ascii_lowercase();
                if [
                    "authorization",
                    "api_key",
                    "apikey",
                    "access_token",
                    "auth_token",
                    "secret",
                    "password",
                    "credential",
                ]
                .iter()
                .any(|s| k.contains(s))
                {
                    return Err(GateError::Invalid("CREDENTIAL_BEARING_PROVIDER_FIELD"));
                }
                reject_secret_keys(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_secret_keys(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

pub struct Gate;
impl Gate {
    /// Inspect only. A legacy runtime with no manifest must be accepted by an
    /// operator, not silently enrolled by opening/inspecting it.
    pub fn status(dir: &Path, current: &Manifest) -> Result<GateStatus, GateError> {
        current.validate()?;
        match load(dir.join(MANIFEST_FILE).as_path()) {
            Ok(baseline) => Ok(compare(baseline.as_ref(), current)),
            Err(GateError::Invalid(code)) => Ok(blocked(current, code)),
            Err(error) => Err(error),
        }
    }

    pub fn ensure(dir: &Path, current: &Manifest) -> Result<(), GateError> {
        let _lock = lock(dir)?;
        let status = Self::status(dir, current)?;
        match status.state {
            GateState::Continue => Ok(()),
            GateState::Blocked => Err(GateError::Blocked("INVALID_STORED_MANIFEST_OR_BINDING")),
            GateState::AcceptRequired => {
                atomic_write(
                    dir,
                    PENDING_FILE,
                    &json!({"format_version":FORMAT_VERSION,"candidate":current,
                    "accepted_digest":status.accepted_digest,"review_token":status.review_token}),
                )?;
                Err(GateError::AcceptanceRequired {
                    review_token: status.review_token,
                })
            }
        }
    }

    /// Only call immediately after creating a genuinely new individual/DB.
    pub fn enroll_new(dir: &Path, current: &Manifest) -> Result<(), GateError> {
        current.validate()?;
        let _lock = lock(dir)?;
        if let Some(existing) = load(&dir.join(MANIFEST_FILE))? {
            if existing == *current {
                return Ok(());
            }
            return Err(GateError::Blocked("BASELINE_ALREADY_EXISTS"));
        }
        atomic_write(dir, MANIFEST_FILE, current)
    }

    /// Explicit operator action; the review token binds candidate AND old
    /// baseline. Re-read the actual config and recapture before calling this.
    pub fn accept(
        dir: &Path,
        current: &Manifest,
        expected_review_token: &str,
    ) -> Result<(), GateError> {
        current.validate()?;
        let _lock = lock(dir)?;
        let status = Self::status(dir, current)?;
        if status.state == GateState::Blocked {
            return Err(GateError::Blocked("INVALID_STORED_MANIFEST_OR_BINDING"));
        }
        if status.review_token != expected_review_token {
            return Err(GateError::StaleReview);
        }
        atomic_write(dir, MANIFEST_FILE, current)?;
        match fs::remove_file(dir.join(PENDING_FILE)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(GateError::Storage),
        }
        Ok(())
    }
}

fn compare(baseline: Option<&Manifest>, current: &Manifest) -> GateStatus {
    let accepted_digest = baseline.map(|m| m.digest.clone());
    let token = json_digest(
        &json!({"binding":current.binding,"candidate_digest":current.digest,"accepted_digest":accepted_digest}),
    );
    let mut result = GateStatus {
        state: GateState::AcceptRequired,
        candidate_digest: current.digest.clone(),
        accepted_digest,
        review_token: token,
        changed_components: Vec::new(),
        reason_codes: Vec::new(),
    };
    let Some(baseline) = baseline else {
        result.reason_codes.push("BASELINE_MISSING".to_owned());
        return result;
    };
    if baseline.binding != current.binding {
        result.state = GateState::Blocked;
        result
            .reason_codes
            .push("INDIVIDUAL_BINDING_MISMATCH".to_owned());
        return result;
    }
    if baseline.digest == current.digest {
        result.state = GateState::Continue;
        return result;
    }
    let keys: BTreeSet<&String> = baseline
        .components
        .keys()
        .chain(current.components.keys())
        .collect();
    result.changed_components = keys
        .into_iter()
        .filter(|k| baseline.components.get(*k) != current.components.get(*k))
        .cloned()
        .collect();
    if baseline.allowed_model_hashes != current.allowed_model_hashes {
        result
            .changed_components
            .push("allowed_model_hashes".to_owned());
    }
    if baseline.declared_models != current.declared_models {
        result.changed_components.push("declared_models".to_owned());
    }
    result
        .reason_codes
        .push("BEHAVIOR_DECLARATION_CHANGED".to_owned());
    result
}
fn blocked(current: &Manifest, code: &str) -> GateStatus {
    GateStatus {
        state: GateState::Blocked,
        candidate_digest: current.digest.clone(),
        accepted_digest: None,
        review_token: String::new(),
        changed_components: Vec::new(),
        reason_codes: vec![code.to_owned()],
    }
}

fn load(path: &Path) -> Result<Option<Manifest>, GateError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(meta) if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_STATE_BYTES => {
            return Err(GateError::Invalid("INVALID_MANIFEST_FILE"));
        }
        Ok(_) => {}
        Err(_) => return Err(GateError::Storage),
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| GateError::Storage)?
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GateError::Storage)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(GateError::Invalid("MANIFEST_TOO_LARGE"));
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| GateError::Invalid("MALFORMED_MANIFEST"))?;
    manifest.validate()?;
    Ok(Some(manifest))
}
fn lock(dir: &Path) -> Result<File, GateError> {
    let p = dir.join(LOCK_FILE);
    if fs::symlink_metadata(&p).is_ok_and(|m| m.is_symlink()) {
        return Err(GateError::Invalid("SYMLINK_LOCK"));
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(p)
        .map_err(|_| GateError::Storage)?;
    file.lock().map_err(|_| GateError::Storage)?;
    Ok(file)
}
fn atomic_write<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<(), GateError> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| GateError::Invalid("MANIFEST_ENCODING"))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(GateError::Invalid("MANIFEST_TOO_LARGE"));
    }
    let temp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|_| GateError::Storage)?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| GateError::Storage)?;
        fs::rename(&temp, dir.join(name)).map_err(|_| GateError::Storage)?;
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|_| GateError::Storage)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PersonaBackendKind, ReasoningSetting, ResourceImplementation};
    use kamimusuhi_core::ids::{NodeId, PersonaBackendId};
    use kamimusuhi_core::routing::LocalityClass;

    fn binding() -> Binding {
        Binding {
            individual_id: IndividualId::from_u128(71),
            root_commit_id: CommitId::from_u128(72),
        }
    }
    fn provider(model: &str) -> PersonaProviderConfig {
        PersonaProviderConfig {
            backend_id: PersonaBackendId::from_u128(73),
            locality: LocalityClass::External,
            base_url: "https://provider.example/v1".to_owned(),
            model: model.to_owned(),
            auth_env: Some("TEST_API_KEY".to_owned()),
            timeout_ms: 1000,
            tls_root_ca_path: None,
            system_instruction: None,
            reasoning: ReasoningSetting::Off,
            extra_body: None,
        }
    }
    fn config() -> RuntimeConfig {
        let mut c = RuntimeConfig::new(NodeId::from_u128(74), ResourceImplementation::FakeA);
        c.persona.backend = PersonaBackendKind::OpenaiCompatible;
        c.persona.provider = Some(provider("private-model-label"));
        c
    }
    fn snapshot(c: &RuntimeConfig) -> Manifest {
        Manifest::capture(c, binding()).unwrap()
    }

    #[test]
    fn legacy_manifest_requires_exact_explicit_acceptance_without_auto_adoption() {
        let dir = tempfile::tempdir().unwrap();
        let m = snapshot(&config());
        let s = Gate::status(dir.path(), &m).unwrap();
        assert_eq!(s.state, GateState::AcceptRequired);
        assert!(!dir.path().join(MANIFEST_FILE).exists());
        assert!(!dir.path().join(PENDING_FILE).exists());
        assert!(matches!(
            Gate::ensure(dir.path(), &m),
            Err(GateError::AcceptanceRequired { .. })
        ));
        assert!(dir.path().join(PENDING_FILE).exists());
        assert!(!dir.path().join(MANIFEST_FILE).exists());
        assert!(matches!(
            Gate::accept(dir.path(), &m, "wrong"),
            Err(GateError::StaleReview)
        ));
        Gate::accept(dir.path(), &m, &s.review_token).unwrap();
        assert_eq!(
            Gate::status(dir.path(), &m).unwrap().state,
            GateState::Continue
        );
        assert!(!dir.path().join(PENDING_FILE).exists());
    }

    #[test]
    fn model_and_prompt_changes_are_detected_and_revert_to_baseline_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config();
        let old = snapshot(&c);
        Gate::enroll_new(dir.path(), &old).unwrap();
        c.persona.provider.as_mut().unwrap().model = "replacement".to_owned();
        let changed = snapshot(&c);
        assert_eq!(
            Gate::status(dir.path(), &changed).unwrap().state,
            GateState::AcceptRequired
        );
        assert!(Gate::ensure(dir.path(), &changed).is_err());
        assert_eq!(
            Gate::status(dir.path(), &old).unwrap().state,
            GateState::Continue
        );
        c = config();
        c.persona.provider.as_mut().unwrap().system_instruction =
            Some("Changed framing".to_owned());
        let s = Gate::status(dir.path(), &snapshot(&c)).unwrap();
        assert_eq!(s.state, GateState::AcceptRequired);
        assert!(
            s.changed_components
                .contains(&"language.primary".to_owned())
        );
    }

    #[test]
    fn prompt_content_change_is_detected_even_when_file_mtime_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let mut c = config();
        c.save(&path).unwrap();
        let original = fs::metadata(&path).unwrap().modified().unwrap();
        let old = snapshot(&RuntimeConfig::load(&path).unwrap());
        c.persona.provider.as_mut().unwrap().system_instruction = Some("new prompt".to_owned());
        c.save(&path).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original))
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), original);
        let new = snapshot(&RuntimeConfig::load(&path).unwrap());
        assert_ne!(old.digest, new.digest);
    }

    #[test]
    fn operational_rotation_ids_and_provider_order_do_not_change_manifest() {
        let mut c = config();
        c.language_providers
            .insert("b".to_owned(), provider("model-b"));
        c.language_providers
            .insert("a".to_owned(), provider("model-a"));
        let old = snapshot(&c);
        c.node_id = NodeId::from_u128(99);
        c.persona.provider.as_mut().unwrap().backend_id = PersonaBackendId::from_u128(100);
        c.persona.provider.as_mut().unwrap().auth_env = Some("ROTATED_KEY_NAME".to_owned());
        c.provider_timeout_ms = 500;
        c.persona.provider.as_mut().unwrap().timeout_ms = 2000;
        let map = c.language_providers.clone();
        c.language_providers.clear();
        for (k, v) in map.into_iter().rev() {
            c.language_providers.insert(k, v);
        }
        assert_eq!(old, snapshot(&c));
        assert!(
            old.declares_model("https://provider.example/v1", "model-a")
                .unwrap()
        );
        assert!(
            old.declares_model("https://provider.example/v1/", "model-b")
                .unwrap()
        );
        assert!(
            !old.declares_model("https://provider.example/v1", "new-model")
                .unwrap()
        );
    }

    #[test]
    fn provider_set_sampling_and_safe_proxy_declarations_require_review() {
        let c = config();
        let old = snapshot(&c);
        let dir = tempfile::tempdir().unwrap();
        Gate::enroll_new(dir.path(), &old).unwrap();
        let mut added = c.clone();
        added
            .language_providers
            .insert("fallback".to_owned(), provider("fallback"));
        assert_eq!(
            Gate::status(dir.path(), &snapshot(&added)).unwrap().state,
            GateState::AcceptRequired
        );
        let mut sampled = c.clone();
        sampled.persona.provider.as_mut().unwrap().extra_body =
            Some(serde_json::from_value(json!({"temperature":0.5,"top_p":0.8})).unwrap());
        assert_eq!(
            Gate::status(dir.path(), &snapshot(&sampled)).unwrap().state,
            GateState::AcceptRequired
        );
        let declared = Manifest::capture_with_declarations(
            &c,
            binding(),
            json!({"tiers":[{"model":"declared-a"},{"model":"declared-b"}]}),
        )
        .unwrap();
        assert_ne!(declared.digest, old.digest);
    }

    #[test]
    fn accepted_proxy_set_allows_fallback_but_refuses_unknown_models_and_versions() {
        let c = config();
        let a = content_digest(b"actual-a");
        let b = content_digest(b"actual-b");
        let context = json!({"version":1,"model_hashes":[a,b],"route_pins":{"http://node":content_digest(b"routes")}});
        let manifest = Manifest::capture_with_declarations(&c, binding(), context.clone()).unwrap();
        assert!(manifest.allows_model_name("actual-a"));
        assert!(manifest.allows_model_name("actual-b"));
        assert!(!manifest.allows_model_name("silent-replacement"));
        let mut reversed = context;
        reversed["model_hashes"].as_array_mut().unwrap().reverse();
        assert_eq!(
            manifest.digest,
            Manifest::capture_with_declarations(&c, binding(), reversed)
                .unwrap()
                .digest
        );
        assert!(Manifest::capture_with_declarations(&c, binding(), json!({"version":2})).is_err());
        assert!(
            Manifest::capture_with_declarations(
                &c,
                binding(),
                json!({"version":1,"model_hashes":["not-a-digest"]})
            )
            .is_err()
        );
    }

    #[test]
    fn stale_acceptance_cannot_override_newer_baseline_or_changed_candidate() {
        let c = config();
        let a = snapshot(&c);
        let dir = tempfile::tempdir().unwrap();
        Gate::enroll_new(dir.path(), &a).unwrap();
        let mut b = c.clone();
        b.persona.provider.as_mut().unwrap().model = "b".to_owned();
        let b = snapshot(&b);
        let old_review = Gate::status(dir.path(), &b).unwrap().review_token;
        let mut d = c.clone();
        d.persona.provider.as_mut().unwrap().model = "d".to_owned();
        let d = snapshot(&d);
        let review = Gate::status(dir.path(), &d).unwrap();
        Gate::accept(dir.path(), &d, &review.review_token).unwrap();
        assert!(matches!(
            Gate::accept(dir.path(), &b, &old_review),
            Err(GateError::StaleReview)
        ));
        assert_eq!(
            Gate::status(dir.path(), &d).unwrap().state,
            GateState::Continue
        );
        assert!(matches!(
            Gate::accept(dir.path(), &d, &old_review),
            Err(GateError::StaleReview)
        ));
    }

    #[test]
    fn malformed_tampered_or_different_individual_baselines_block_without_reset() {
        let dir = tempfile::tempdir().unwrap();
        let m = snapshot(&config());
        Gate::enroll_new(dir.path(), &m).unwrap();
        let mut other = m.clone();
        other.binding.individual_id = IndividualId::from_u128(900);
        other.digest = other.calculated_digest();
        assert_eq!(
            Gate::status(dir.path(), &other).unwrap().state,
            GateState::Blocked
        );
        assert!(matches!(
            Gate::accept(dir.path(), &other, "anything"),
            Err(GateError::Blocked(_))
        ));
        let path = dir.path().join(MANIFEST_FILE);
        let mut tampered = m.clone();
        tampered
            .components
            .insert("language.primary".to_owned(), content_digest(b"tampered"));
        fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert_eq!(
            Gate::status(dir.path(), &m).unwrap().state,
            GateState::Blocked
        );
        let bytes = fs::read(&path).unwrap();
        assert!(Gate::enroll_new(dir.path(), &m).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, b"broken json").unwrap();
        assert_eq!(
            Gate::status(dir.path(), &m).unwrap().state,
            GateState::Blocked
        );
        assert_eq!(fs::read(&path).unwrap(), b"broken json");
    }

    #[test]
    fn manifest_stores_no_prompt_or_credentials_and_rejects_credential_like_fields() {
        let mut c = config();
        c.persona.provider.as_mut().unwrap().system_instruction =
            Some("PRIVATE_PROMPT_DO_NOT_STORE".to_owned());
        let bytes = serde_json::to_string(&snapshot(&c)).unwrap();
        for forbidden in [
            "PRIVATE_PROMPT_DO_NOT_STORE",
            "private-model-label",
            "TEST_API_KEY",
            "provider.example",
        ] {
            assert!(!bytes.contains(forbidden));
        }
        c.persona.provider.as_mut().unwrap().extra_body =
            Some(serde_json::from_value(json!({"api_key":"do-not-hash"})).unwrap());
        assert!(Manifest::capture(&c, binding()).is_err());
        c.persona.provider.as_mut().unwrap().extra_body = None;
        c.persona.provider.as_mut().unwrap().base_url =
            "https://user:secret@provider.example/v1".to_owned();
        assert!(Manifest::capture(&c, binding()).is_err());
    }

    #[test]
    fn unsupported_manifest_version_and_enroll_overwrite_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let m = snapshot(&config());
        Gate::enroll_new(dir.path(), &m).unwrap();
        let mut changed = m.clone();
        changed.format_version = 99;
        changed.digest = changed.calculated_digest();
        fs::write(
            dir.path().join(MANIFEST_FILE),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert_eq!(
            Gate::status(dir.path(), &m).unwrap().state,
            GateState::Blocked
        );
        assert!(Gate::enroll_new(dir.path(), &m).is_err());
    }
}
