//! Declarative runtime configuration.
//!
//! The config says which *implementation* currently fills each cognitive
//! *slot*. It is runtime infrastructure and nothing else: it holds no
//! `IndividualId`, so losing or rewriting it cannot lose, fork or migrate the
//! individual. Identity comes back from the canonical database, which is why
//! swapping `fake-a` for `fake-b` here is a substitution and not a migration.
//!
//! W4 has no real provider, and this file has no field for an endpoint, a
//! token or a credential. That is the schema, not a convention.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use kamimusuhi_core::ids::NodeId;
use kamimusuhi_core::mutation::UnknownVocabulary;
use kamimusuhi_core::resources::{CognitiveResource, ResourceRegistry, ResourceSlot};
use kamimusuhi_testkit::{FakeResource, UnavailableResource};
use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

/// The cognitive role W4's scenario fills. One slot is enough to prove that a
/// role outlives the implementation behind it.
pub const GENERAL_SLOT: &str = "general";

/// Which deterministic fake fills a slot.
///
/// W4 is fake-only by design: the wave proves replacement does not disturb
/// identity, and a real provider would add failure modes that belong to W5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FakeImplementation {
    FakeA,
    FakeB,
    /// Always fails. Present so the error path is configurable rather than
    /// only reachable from unit tests.
    FakeUnavailable,
}

impl FakeImplementation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FakeA => "fake-a",
            Self::FakeB => "fake-b",
            Self::FakeUnavailable => "fake-unavailable",
        }
    }

    fn build(self) -> Arc<dyn CognitiveResource> {
        match self {
            Self::FakeA => Arc::new(FakeResource::a()),
            Self::FakeB => Arc::new(FakeResource::b()),
            Self::FakeUnavailable => Arc::new(UnavailableResource),
        }
    }
}

impl fmt::Display for FakeImplementation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FakeImplementation {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "fake-a" => Self::FakeA,
            "fake-b" => Self::FakeB,
            "fake-unavailable" => Self::FakeUnavailable,
            other => return Err(UnknownVocabulary::new("resource_implementation", other)),
        })
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
    pub resources: BTreeMap<String, FakeImplementation>,
}

impl RuntimeConfig {
    pub const VERSION: u32 = 1;
    pub const FILE_NAME: &'static str = "runtime.json";

    pub fn new(node_id: NodeId, general: FakeImplementation) -> Self {
        let mut resources = BTreeMap::new();
        resources.insert(GENERAL_SLOT.to_owned(), general);
        Self {
            config_version: Self::VERSION,
            node_id,
            resources,
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

    pub fn implementation(&self, slot: &str) -> Option<FakeImplementation> {
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
        implementation: FakeImplementation,
    ) -> Option<FakeImplementation> {
        self.resources
            .insert(slot.to_owned(), implementation)
            .filter(|previous| *previous != implementation)
    }

    /// Build the registry this config describes.
    ///
    /// The registry is rebuilt from the file on every start and is never
    /// persisted as part of the individual.
    pub fn build_registry(&self) -> Result<ResourceRegistry, RuntimeError> {
        let mut registry = ResourceRegistry::new();
        for (slot, implementation) in &self.resources {
            registry
                .register(ResourceSlot::new(slot.clone()), implementation.build())
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
        RuntimeConfig::new(NodeId::from_u128(0x0E), FakeImplementation::FakeA)
    }

    #[test]
    fn config_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RuntimeConfig::FILE_NAME);
        let written = config();
        written.save(&path).unwrap();
        assert_eq!(RuntimeConfig::load(&path).unwrap(), written);
    }

    #[test]
    fn config_holds_no_individual_and_no_credential() {
        let text = serde_json::to_string(&config()).unwrap();
        assert!(!text.contains("individual"));
        for forbidden in ["token", "key", "secret", "password", "endpoint", "url"] {
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
            config.set_implementation(GENERAL_SLOT, FakeImplementation::FakeB),
            Some(FakeImplementation::FakeA)
        );
        assert_eq!(
            config.implementation(GENERAL_SLOT),
            Some(FakeImplementation::FakeB)
        );
        // Setting the same implementation again displaces nothing.
        assert_eq!(
            config.set_implementation(GENERAL_SLOT, FakeImplementation::FakeB),
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
    fn implementation_vocabulary_round_trips() {
        for implementation in [
            FakeImplementation::FakeA,
            FakeImplementation::FakeB,
            FakeImplementation::FakeUnavailable,
        ] {
            assert_eq!(
                implementation
                    .as_str()
                    .parse::<FakeImplementation>()
                    .unwrap(),
                implementation
            );
        }
        assert!("gpt-4".parse::<FakeImplementation>().is_err());
    }
}
