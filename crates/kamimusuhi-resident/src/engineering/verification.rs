//! Pure presentation of historical validation evidence.
//!
//! A matching digest compares recorded operator settings. It does not inspect
//! today's candidate, external executable contents, PATH, or service state.
//! Status reads must never turn into git operations or another validation.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::workspace::{Limits, Validation, ValidationContract, WorkspaceConfig};
use super::{EngineeringConfig, Job};

pub(super) fn policy_digest(policy: &WorkspaceConfig, limits: &Limits) -> String {
    kamimusuhi_core::digest::json_digest(&json!({
        "schema_version":1,
        "workspace":policy.name,
        "path":policy.path,
        "checks":policy.checks,
        "validation_timeout_seconds":limits.validation_timeout.as_secs(),
        "validation_timeout_subsecond_nanos":limits.validation_timeout.subsec_nanos(),
        "output_bytes":limits.output_bytes,
    }))
}

pub(super) fn contract(policy: &WorkspaceConfig, limits: &Limits) -> ValidationContract {
    ValidationContract {
        schema_version: 1,
        policy_digest: policy_digest(policy, limits),
        completed_at: crate::util::unix_now(),
    }
}

pub(super) fn valid(contract: &ValidationContract) -> bool {
    contract.schema_version == 1
        && contract
            .policy_digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

pub(super) fn view(validation: &Validation, current_digest: Option<String>) -> Value {
    let recorded = validation.contract.as_ref();
    let status = match (recorded, current_digest.as_deref()) {
        (None, _) => "unknown",
        (Some(_), None) => "unavailable",
        (Some(contract), Some(current)) if contract.policy_digest == current => "matching",
        (Some(_), Some(_)) => "changed",
    };
    json!({
        "scope":"recorded_commit",
        "head":validation.head,
        "passed":validation.passed,
        "completed_at":recorded.map(|c| c.completed_at),
        "policy":{
            "status":status,
            "recorded_digest":recorded.map(|c| &c.policy_digest),
            "current_digest":current_digest,
        },
        "candidate":{"status":"not_observed","observed_at":null},
    })
}

/// Compute each workspace's configuration digest once per response, before
/// locking the store. Many historical jobs can share the same workspace.
pub(super) struct Views {
    digests: BTreeMap<String, String>,
}

impl Views {
    pub(super) fn new(config: Option<&EngineeringConfig>) -> Self {
        let digests = config
            .into_iter()
            .flat_map(|config| {
                let limits = config.limits();
                config
                    .workspaces
                    .iter()
                    .map(move |policy| (policy.name.clone(), policy_digest(policy, &limits)))
            })
            .collect();
        Self { digests }
    }

    fn verification(&self, job: &Job) -> Option<Value> {
        let validation = job.validation.as_ref()?;
        Some(view(validation, self.digests.get(&job.workspace).cloned()))
    }

    pub(super) fn job(&self, job: &Job) -> Value {
        let mut result = json!(job);
        if let Some(verification) = self.verification(job) {
            result["verification"] = verification;
        }
        result
    }

    pub(super) fn summary(&self, job: &Job) -> Value {
        let mut result = json!({
            "id":job.id,"workspace":job.workspace,"phase":job.phase,"metrics":job.metrics,
        });
        if let Some(verification) = self.verification(job) {
            result["verification"] = verification;
        }
        result
    }
}
