//! Declarative runtime configuration.
//!
//! The config says which *implementation* currently fills each cognitive
//! *slot*, and how to reach it. It is runtime infrastructure and nothing else:
//! it holds no `IndividualId`, so losing or rewriting it cannot lose, fork or
//! migrate the individual. Swapping `fake-a` for `openai-compatible` is a
//! substitution of thinking capacity, never a migration of who is thinking.
//!
//! **No credential is stored here.** A provider entry names the *environment
//! variable* its bearer token lives in; the token itself is read at call time
//! and never written to this file, to the database, or to the trace. That is
//! why swapping providers cannot leak one provider's secret into another's
//! records, and why this file is safe to keep beside a runtime directory.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use kamimusuhi_core::ids::{NodeId, ResourceId};
use kamimusuhi_core::mutation::UnknownVocabulary;
use kamimusuhi_core::resources::{CognitiveResource, ResourceRegistry, ResourceSlot};
use kamimusuhi_core::routing::{
    CostClass, HealthState, LatencyClass, LocalityClass, Modality, QualityTier,
    ResourceCapabilities,
};
use kamimusuhi_resource_http::{OpenAiCompatibleConfig, OpenAiCompatibleResource, TrustAnchors};
use kamimusuhi_testkit::{FakeResource, UnavailableResource};
use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

/// The cognitive role the scenario fills. One slot is enough to prove that a
/// role outlives the implementation behind it.
pub const GENERAL_SLOT: &str = "general";

/// Which implementation fills a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceImplementation {
    FakeA,
    FakeB,
    /// Always fails. Present so the error path is configurable rather than
    /// only reachable from unit tests.
    FakeUnavailable,
    /// A real HTTP provider speaking the OpenAI chat-completions shape.
    /// Needs a matching [`ProviderConfig`] for the same slot.
    OpenaiCompatible,
}

impl ResourceImplementation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FakeA => "fake-a",
            Self::FakeB => "fake-b",
            Self::FakeUnavailable => "fake-unavailable",
            Self::OpenaiCompatible => "openai-compatible",
        }
    }

    /// Whether this implementation talks to something outside the process.
    pub const fn is_networked(self) -> bool {
        matches!(self, Self::OpenaiCompatible)
    }
}

impl fmt::Display for ResourceImplementation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResourceImplementation {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "fake-a" => Self::FakeA,
            "fake-b" => Self::FakeB,
            "fake-unavailable" => Self::FakeUnavailable,
            "openai-compatible" => Self::OpenaiCompatible,
            other => return Err(UnknownVocabulary::new("resource_implementation", other)),
        })
    }
}

/// How to reach one HTTP provider.
///
/// Everything here is non-secret configuration. `auth_env` is the *name* of an
/// environment variable, not its value — that distinction is the whole reason
/// this struct can be serialized at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    pub model: String,
    /// Name of the environment variable holding the bearer token. `None` for a
    /// local endpoint that needs no credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_env: Option<String>,
    /// Deadline for one logical call, retries included.
    pub timeout_ms: u64,
    /// Physical attempts the adapter may make. 1 disables retry.
    pub max_attempts: u32,
    /// Fixed pause between attempts.
    pub retry_backoff_ms: u64,
    /// Distinguishes two configured providers from each other in
    /// `resource_calls`. Stable per configured provider, not per request.
    pub resource_id: ResourceId,
    /// A PEM file of root certificates, for an endpoint served by a private
    /// CA. Absent means the bundled public root set. There is deliberately no
    /// setting that disables verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_root_ca_path: Option<std::path::PathBuf>,
    /// What the operator declares this provider to be, for routing. Asserted,
    /// not measured: a provider does not get to describe itself.
    #[serde(default = "default_provider_capabilities")]
    pub capabilities: ResourceCapabilities,
}

/// An external, cheap-but-not-free, interactive-speed, standard-quality
/// provider: the conservative reading of an endpoint an operator pointed us at.
fn default_provider_capabilities() -> ResourceCapabilities {
    ResourceCapabilities {
        locality: LocalityClass::External,
        modalities: [Modality::Text].into_iter().collect(),
        context_capacity: 8_192,
        latency: LatencyClass::Fast,
        cost: CostClass::Low,
        quality: QualityTier::Standard,
        health: HealthState::Healthy,
    }
}

impl ProviderConfig {
    fn build(&self, slot: &str) -> Result<Arc<dyn CognitiveResource>, RuntimeError> {
        let config = OpenAiCompatibleConfig::new(
            self.resource_id,
            self.base_url.clone(),
            self.model.clone(),
        )
        .with_timeout_ms(self.timeout_ms)
        .with_max_attempts(self.max_attempts)
        .with_retry_backoff_ms(self.retry_backoff_ms)
        .with_auth_env(self.auth_env.clone())
        .with_capabilities(self.capabilities.clone())
        .with_trust_anchors(match &self.tls_root_ca_path {
            Some(path) => TrustAnchors::PemFile(path.clone()),
            None => TrustAnchors::Webpki,
        });
        config
            .validate()
            .map_err(|message| RuntimeError::ProviderConfig {
                slot: slot.to_owned(),
                message,
            })?;
        Ok(Arc::new(OpenAiCompatibleResource::new(config)))
    }
}

/// On-disk runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Config format version. Refuse what this build does not understand
    /// rather than guessing.
    pub config_version: u32,
    /// This host. Stable across restarts on the same machine, and part of the
    /// writer identity — but not the identity of the individual.
    pub node_id: NodeId,
    /// slot → implementation. The whole point of the file.
    pub resources: BTreeMap<String, ResourceImplementation>,
    /// slot → how to reach its provider, when the implementation is networked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl RuntimeConfig {
    pub const VERSION: u32 = 1;
    pub const FILE_NAME: &'static str = "runtime.json";

    pub fn new(node_id: NodeId, general: ResourceImplementation) -> Self {
        let mut resources = BTreeMap::new();
        resources.insert(GENERAL_SLOT.to_owned(), general);
        Self {
            config_version: Self::VERSION,
            node_id,
            resources,
            providers: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, RuntimeError> {
        let text = std::fs::read_to_string(path).map_err(|source| RuntimeError::ConfigIo {
            path: path.display().to_string(),
            message: source.to_string(),
        })?;
        let config: Self =
            serde_json::from_str(&text).map_err(|source| RuntimeError::ConfigMalformed {
                path: path.display().to_string(),
                message: source.to_string(),
            })?;
        if config.config_version != Self::VERSION {
            return Err(RuntimeError::ConfigVersion {
                found: config.config_version,
                supported: Self::VERSION,
            });
        }
        if config.node_id.is_nil() {
            return Err(RuntimeError::ConfigMalformed {
                path: path.display().to_string(),
                message: "node_id is nil".to_owned(),
            });
        }
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), RuntimeError> {
        let mut text =
            serde_json::to_string_pretty(self).map_err(|source| RuntimeError::ConfigMalformed {
                path: path.display().to_string(),
                message: source.to_string(),
            })?;
        text.push('\n');
        std::fs::write(path, text).map_err(|source| RuntimeError::ConfigIo {
            path: path.display().to_string(),
            message: source.to_string(),
        })
    }

    pub fn implementation(&self, slot: &str) -> Option<ResourceImplementation> {
        self.resources.get(slot).copied()
    }

    /// Point a slot at a different implementation.
    ///
    /// Returns what was there before. This is the whole of "replacing the
    /// resource": no canonical write happens, and the caller still has the
    /// same individual afterwards.
    pub fn set_implementation(
        &mut self,
        slot: &str,
        implementation: ResourceImplementation,
    ) -> Option<ResourceImplementation> {
        self.resources
            .insert(slot.to_owned(), implementation)
            .filter(|previous| *previous != implementation)
    }

    pub fn set_provider(&mut self, slot: &str, provider: ProviderConfig) {
        self.providers.insert(slot.to_owned(), provider);
    }

    pub fn provider(&self, slot: &str) -> Option<&ProviderConfig> {
        self.providers.get(slot)
    }

    /// Build the registry this config describes.
    ///
    /// The registry is rebuilt from the file on every start and is never
    /// persisted as part of the individual.
    pub fn build_registry(&self) -> Result<ResourceRegistry, RuntimeError> {
        let mut registry = ResourceRegistry::new();
        for (slot, implementation) in &self.resources {
            let resource: Arc<dyn CognitiveResource> = match implementation {
                ResourceImplementation::FakeA => Arc::new(FakeResource::a()),
                ResourceImplementation::FakeB => Arc::new(FakeResource::b()),
                ResourceImplementation::FakeUnavailable => Arc::new(UnavailableResource),
                ResourceImplementation::OpenaiCompatible => self
                    .providers
                    .get(slot)
                    .ok_or_else(|| RuntimeError::ProviderConfig {
                        slot: slot.clone(),
                        message: "openai-compatible needs a provider entry for this slot"
                            .to_owned(),
                    })?
                    .build(slot)?,
            };
            registry
                .register(ResourceSlot::new(slot.clone()), resource)
                .map_err(|source| RuntimeError::ResourceRegistry {
                    message: source.to_string(),
                })?;
        }
        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RuntimeConfig {
        RuntimeConfig::new(NodeId::from_u128(0x0E), ResourceImplementation::FakeA)
    }

    fn provider() -> ProviderConfig {
        ProviderConfig {
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            model: "test-model".to_owned(),
            auth_env: Some("KAMIMUSUHI_TEST_TOKEN".to_owned()),
            timeout_ms: 250,
            max_attempts: 2,
            retry_backoff_ms: 1,
            resource_id: ResourceId::from_u128(0x0B01),
            tls_root_ca_path: None,
            capabilities: default_provider_capabilities(),
        }
    }

    #[test]
    fn config_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RuntimeConfig::FILE_NAME);
        let mut written = config();
        written.set_implementation(GENERAL_SLOT, ResourceImplementation::OpenaiCompatible);
        written.set_provider(GENERAL_SLOT, provider());
        written.save(&path).unwrap();
        assert_eq!(RuntimeConfig::load(&path).unwrap(), written);
    }

    #[test]
    fn config_holds_no_individual_and_no_credential() {
        let mut with_provider = config();
        with_provider.set_provider(GENERAL_SLOT, provider());
        let text = serde_json::to_string(&with_provider).unwrap();
        assert!(!text.contains("individual"));
        // The variable's *name* is configuration; its value never lands here.
        assert!(text.contains("KAMIMUSUHI_TEST_TOKEN"));
        for forbidden in ["Bearer", "sk-", "password", "secret_value"] {
            assert!(!text.contains(forbidden), "config mentions {forbidden}");
        }
    }

    #[test]
    fn a_future_config_version_is_refused_rather_than_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RuntimeConfig::FILE_NAME);
        let mut future = config();
        future.config_version = 99;
        future.save(&path).unwrap();
        assert!(matches!(
            RuntimeConfig::load(&path),
            Err(RuntimeError::ConfigVersion {
                found: 99,
                supported: 1
            })
        ));
    }

    #[test]
    fn replacing_a_slot_reports_what_it_displaced() {
        let mut config = config();
        assert_eq!(
            config.set_implementation(GENERAL_SLOT, ResourceImplementation::FakeB),
            Some(ResourceImplementation::FakeA)
        );
        assert_eq!(
            config.implementation(GENERAL_SLOT),
            Some(ResourceImplementation::FakeB)
        );
        // Setting the same implementation again displaces nothing.
        assert_eq!(
            config.set_implementation(GENERAL_SLOT, ResourceImplementation::FakeB),
            None
        );
    }

    #[test]
    fn the_registry_is_built_from_the_file() {
        let registry = config().build_registry().unwrap();
        let descriptor = registry
            .descriptor(&ResourceSlot::new(GENERAL_SLOT))
            .expect("the general slot is filled");
        assert_eq!(descriptor.name, "fake-a");
        assert_eq!(descriptor.adapter, "fake");
        assert!(descriptor.read_only);
    }

    #[test]
    fn a_networked_slot_without_a_provider_entry_is_refused() {
        let mut config = config();
        config.set_implementation(GENERAL_SLOT, ResourceImplementation::OpenaiCompatible);
        assert!(matches!(
            config.build_registry(),
            Err(RuntimeError::ProviderConfig { .. })
        ));
    }

    #[test]
    fn a_configured_provider_becomes_a_registered_resource() {
        let mut config = config();
        config.set_implementation(GENERAL_SLOT, ResourceImplementation::OpenaiCompatible);
        config.set_provider(GENERAL_SLOT, provider());
        let registry = config.build_registry().unwrap();
        let descriptor = registry
            .descriptor(&ResourceSlot::new(GENERAL_SLOT))
            .unwrap();
        assert_eq!(descriptor.adapter, "openai-compatible");
        assert_eq!(descriptor.resource_id, ResourceId::from_u128(0x0B01));
        assert!(descriptor.read_only);
    }

    #[test]
    fn implementation_vocabulary_round_trips() {
        for implementation in [
            ResourceImplementation::FakeA,
            ResourceImplementation::FakeB,
            ResourceImplementation::FakeUnavailable,
            ResourceImplementation::OpenaiCompatible,
        ] {
            assert_eq!(
                implementation
                    .as_str()
                    .parse::<ResourceImplementation>()
                    .unwrap(),
                implementation
            );
        }
        assert!("gpt-4".parse::<ResourceImplementation>().is_err());
        assert!(ResourceImplementation::OpenaiCompatible.is_networked());
        assert!(!ResourceImplementation::FakeA.is_networked());
    }
}
