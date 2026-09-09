//! Cognitive resources: replaceable thinking capacity.
//!
//! A cognitive resource is something Kamimusuhi *uses*, not something it *is*.
//! A resource can be swapped for another mid-life without the individual
//! becoming a different individual, which is why nothing here touches
//! `IndividualId` or the continuity head: a read-only resource call is
//! cognition, not a canonical mutation (plan §7, §9.2).
//!
//! The registry addresses resources by [`ResourceSlot`] — the *role* being
//! filled — so replacing the backing implementation is an ordinary
//! substitution rather than an identity migration. Every invocation leaves a
//! [`ResourceCall`] naming the resource that actually answered, so a result
//! can never be mistaken for one from a different resource.
//!
//! W3 has no real provider. There is deliberately no field anywhere in this
//! module for a token, key or credential: a request is correlated by digest,
//! never stored.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::digest::json_digest;
use crate::ids::{IndividualId, ResourceCallId, ResourceId};
use crate::mutation::UnknownVocabulary;
use crate::time::{Clock, UtcTimestamp};

/// The role a resource fills. Stable across replacement of the backing
/// implementation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceSlot(pub String);

impl ResourceSlot {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a resource is for. W3 only needs read-only cognition; acting on the
/// world is a later wave and has no variant here on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// Produces text or structured material from a prompt. Read-only.
    Generation,
    /// Condenses supplied material. Read-only.
    Summarization,
}

impl ResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generation => "generation",
            Self::Summarization => "summarization",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResourceKind {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "generation" => Self::Generation,
            "summarization" => Self::Summarization,
            other => return Err(UnknownVocabulary::new("resource_kind", other)),
        })
    }
}

/// Provider-neutral identity of one resource implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDescriptor {
    pub resource_id: ResourceId,
    /// Human-readable name for operators. Not an identity.
    pub name: String,
    pub kind: ResourceKind,
    /// Adapter family, e.g. `fake`. Never a provider credential or endpoint.
    pub adapter: String,
    pub version: String,
    /// W3 resources are read-only cognition. A resource that acted on the
    /// world would need an Executor and an authorization path that phase 1
    /// does not have.
    pub read_only: bool,
}

/// One read-only request to a resource.
///
/// `input` is cognitive material, so it must not carry secrets: the call
/// record digests it, and digesting is not redaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub individual_id: IndividualId,
    /// Why the runtime is asking. Used for attribution, not for routing.
    pub purpose: String,
    pub input: serde_json::Value,
}

impl ResourceRequest {
    pub fn new(
        individual_id: IndividualId,
        purpose: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            individual_id,
            purpose: purpose.into(),
            input,
        }
    }

    /// Non-secret correlation digest of the request, stable across retries of
    /// the same request and independent of JSON key order.
    pub fn digest(&self) -> String {
        json_digest(&serde_json::json!({
            "individual_id": self.individual_id,
            "purpose": self.purpose,
            "input": self.input,
        }))
    }
}

/// What a resource returned, tagged with the resource that produced it.
///
/// The `resource_id` is set by the registry from the descriptor of the
/// resource that actually ran, so a result cannot claim to come from a
/// resource that did not produce it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceResult {
    pub resource_id: ResourceId,
    pub content: serde_json::Value,
}

impl ResourceResult {
    pub fn new(resource_id: ResourceId, content: serde_json::Value) -> Self {
        Self {
            resource_id,
            content,
        }
    }

    pub fn digest(&self) -> String {
        json_digest(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("resource request is invalid: {reason}")]
    InvalidRequest { reason: String },
    #[error("resource {0} is unavailable")]
    Unavailable(ResourceId),
    #[error("resource {resource_id} timed out after {elapsed_ms}ms")]
    Timeout {
        resource_id: ResourceId,
        elapsed_ms: u64,
    },
    #[error("resource {resource_id} failed: {message}")]
    Backend {
        resource_id: ResourceId,
        message: String,
    },
}

impl ResourceError {
    /// Stable code for the call record and for tests.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "INVALID_REQUEST",
            Self::Unavailable(_) => "UNAVAILABLE",
            Self::Timeout { .. } => "TIMEOUT",
            Self::Backend { .. } => "BACKEND",
        }
    }
}

/// One replaceable unit of external cognition.
pub trait CognitiveResource: Send + Sync {
    fn descriptor(&self) -> ResourceDescriptor;

    fn invoke(&self, request: &ResourceRequest) -> Result<ResourceResult, ResourceError>;
}

/// How an invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOutcome {
    Ok,
    Error,
}

impl ResourceOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for ResourceOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResourceOutcome {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ok" => Self::Ok,
            "error" => Self::Error,
            other => return Err(UnknownVocabulary::new("resource_outcome", other)),
        })
    }
}

/// Durable attribution for one invocation.
///
/// Records identities, digests and timing — never the request or the result
/// text, and never a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCall {
    pub resource_call_id: ResourceCallId,
    /// The resource that actually answered, not the slot that was asked.
    pub resource_id: ResourceId,
    pub slot: ResourceSlot,
    pub individual_id: IndividualId,
    pub adapter: String,
    pub purpose: String,
    pub request_digest: String,
    pub outcome: ResourceOutcome,
    /// Digest of the result, or the error code when the call failed.
    pub result_digest: Option<String>,
    pub error_code: Option<String>,
    pub started_at: UtcTimestamp,
    pub completed_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResourceCall {
    pub resource_call_id: ResourceCallId,
    pub resource_id: ResourceId,
    pub slot: ResourceSlot,
    pub individual_id: IndividualId,
    pub adapter: String,
    pub purpose: String,
    pub request_digest: String,
    pub outcome: ResourceOutcome,
    pub result_digest: Option<String>,
    pub error_code: Option<String>,
    pub started_at: UtcTimestamp,
    pub completed_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceCallLogError {
    #[error("resource call record is invalid: {reason}")]
    Invalid { reason: String },
    #[error("individual {0} is not present in this store")]
    IndividualNotFound(IndividualId),
    #[error("resource call log is inconsistent: {detail}")]
    Corrupt { detail: String },
    #[error("resource call log is busy: {detail}")]
    Contended { detail: String },
    #[error("resource call log backend error: {message}")]
    Backend { message: String },
}

/// Durable record of resource invocations.
///
/// Separate from `audit_events`: an audit event explains a canonical state
/// transition, while a resource call explains where cognitive material came
/// from. A call never advances the continuity head.
pub trait ResourceCallLog: Send + Sync {
    fn record(&self, call: NewResourceCall) -> Result<ResourceCall, ResourceCallLogError>;

    fn get_call(
        &self,
        resource_call_id: ResourceCallId,
    ) -> Result<Option<ResourceCall>, ResourceCallLogError>;

    /// Calls made for an individual, oldest first.
    fn calls(&self, individual_id: IndividualId)
    -> Result<Vec<ResourceCall>, ResourceCallLogError>;
}

/// Result of an invocation together with its durable attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedResult {
    pub result: ResourceResult,
    pub call: ResourceCall,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("no resource is registered for slot {0}")]
    SlotEmpty(ResourceSlot),
    #[error("slot {slot} is already filled by {resource_id}")]
    SlotOccupied {
        slot: ResourceSlot,
        resource_id: ResourceId,
    },
    #[error("resource descriptor is invalid: {reason}")]
    InvalidDescriptor { reason: String },
    #[error("resource call failed: {0}")]
    Call(#[from] ResourceError),
    #[error("resource call could not be recorded: {0}")]
    Log(#[from] ResourceCallLogError),
}

/// Provider-neutral registry of replaceable resources.
///
/// Runtime infrastructure, not canonical state: the registry is rebuilt on
/// every start, is never persisted as part of the individual, and replacing an
/// entry is not a lineage event.
#[derive(Default)]
pub struct ResourceRegistry {
    slots: BTreeMap<ResourceSlot, Arc<dyn CognitiveResource>>,
}

impl fmt::Debug for ResourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceRegistry")
            .field("slots", &self.descriptors())
            .finish()
    }
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fill an empty slot. Filling an occupied slot is an error: replacing a
    /// resource is [`Self::replace`], which is explicit about what it does.
    pub fn register(
        &mut self,
        slot: ResourceSlot,
        resource: Arc<dyn CognitiveResource>,
    ) -> Result<ResourceDescriptor, RegistryError> {
        let descriptor = validate(resource.as_ref())?;
        if let Some(existing) = self.slots.get(&slot) {
            return Err(RegistryError::SlotOccupied {
                slot,
                resource_id: existing.descriptor().resource_id,
            });
        }
        self.slots.insert(slot, resource);
        Ok(descriptor)
    }

    /// Swap the resource filling a slot. Returns the descriptor of what was
    /// there before, if anything.
    ///
    /// This is a substitution of thinking capacity. It does not create,
    /// migrate or retire an individual, and it writes nothing canonical.
    pub fn replace(
        &mut self,
        slot: ResourceSlot,
        resource: Arc<dyn CognitiveResource>,
    ) -> Result<Option<ResourceDescriptor>, RegistryError> {
        validate(resource.as_ref())?;
        let previous = self.slots.insert(slot, resource);
        Ok(previous.map(|p| p.descriptor()))
    }

    pub fn resolve(&self, slot: &ResourceSlot) -> Option<Arc<dyn CognitiveResource>> {
        self.slots.get(slot).cloned()
    }

    pub fn descriptor(&self, slot: &ResourceSlot) -> Option<ResourceDescriptor> {
        self.slots.get(slot).map(|r| r.descriptor())
    }

    /// Every filled slot with its current descriptor, in slot order.
    pub fn descriptors(&self) -> Vec<(ResourceSlot, ResourceDescriptor)> {
        self.slots
            .iter()
            .map(|(slot, resource)| (slot.clone(), resource.descriptor()))
            .collect()
    }

    /// Invoke through `clock`, timing the call rather than being told how long
    /// it took.
    ///
    /// Timestamps a caller invents cannot disagree with reality; timestamps
    /// read around the call can, which is the point. W4 only needs honest
    /// durations for the trace — timeout, retry and latency policy are a
    /// later wave and deliberately live nowhere in this module.
    pub fn invoke_with_clock(
        &self,
        slot: &ResourceSlot,
        request: &ResourceRequest,
        log: &dyn ResourceCallLog,
        call_id: ResourceCallId,
        clock: &dyn Clock,
    ) -> Result<AttributedResult, RegistryError> {
        let started_at = clock.now_utc();
        self.invoke_at(slot, request, log, call_id, started_at, || clock.now_utc())
    }

    /// Invoke the resource currently filling `slot` and record the call.
    ///
    /// The returned result is re-tagged with the descriptor of the resource
    /// that actually ran, so attribution cannot be spoofed by the resource
    /// itself. Failures are recorded too: a call that errored still happened.
    pub fn invoke(
        &self,
        slot: &ResourceSlot,
        request: &ResourceRequest,
        log: &dyn ResourceCallLog,
        call_id: ResourceCallId,
        started_at: UtcTimestamp,
        completed_at: UtcTimestamp,
    ) -> Result<AttributedResult, RegistryError> {
        self.invoke_at(slot, request, log, call_id, started_at, || completed_at)
    }

    fn invoke_at(
        &self,
        slot: &ResourceSlot,
        request: &ResourceRequest,
        log: &dyn ResourceCallLog,
        call_id: ResourceCallId,
        started_at: UtcTimestamp,
        completed_at: impl FnOnce() -> UtcTimestamp,
    ) -> Result<AttributedResult, RegistryError> {
        let resource = self
            .resolve(slot)
            .ok_or_else(|| RegistryError::SlotEmpty(slot.clone()))?;
        let descriptor = resource.descriptor();
        let outcome = resource.invoke(request);
        let completed_at = completed_at();

        let mut record = NewResourceCall {
            resource_call_id: call_id,
            resource_id: descriptor.resource_id,
            slot: slot.clone(),
            individual_id: request.individual_id,
            adapter: descriptor.adapter.clone(),
            purpose: request.purpose.clone(),
            request_digest: request.digest(),
            outcome: ResourceOutcome::Error,
            result_digest: None,
            error_code: None,
            started_at,
            completed_at,
        };

        match outcome {
            Ok(result) => {
                let result = ResourceResult {
                    resource_id: descriptor.resource_id,
                    content: result.content,
                };
                record.outcome = ResourceOutcome::Ok;
                record.result_digest = Some(result.digest());
                let call = log.record(record)?;
                Ok(AttributedResult { result, call })
            }
            Err(error) => {
                record.error_code = Some(error.code().to_owned());
                // The failed call is durable before the error is returned: a
                // call that happened must remain visible in attribution.
                log.record(record)?;
                Err(RegistryError::Call(error))
            }
        }
    }
}

fn validate(resource: &dyn CognitiveResource) -> Result<ResourceDescriptor, RegistryError> {
    let descriptor = resource.descriptor();
    if descriptor.resource_id.is_nil() {
        return Err(RegistryError::InvalidDescriptor {
            reason: "resource_id is nil".to_owned(),
        });
    }
    if !descriptor.read_only {
        return Err(RegistryError::InvalidDescriptor {
            reason: "phase 1 registers read-only cognition only".to_owned(),
        });
    }
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct StubResource {
        descriptor: ResourceDescriptor,
        answer: &'static str,
        fail: bool,
    }

    impl StubResource {
        fn new(id: u128, name: &str, answer: &'static str) -> Self {
            Self {
                descriptor: ResourceDescriptor {
                    resource_id: ResourceId::from_u128(id),
                    name: name.to_owned(),
                    kind: ResourceKind::Generation,
                    adapter: "stub".to_owned(),
                    version: "1".to_owned(),
                    read_only: true,
                },
                answer,
                fail: false,
            }
        }
    }

    impl CognitiveResource for StubResource {
        fn descriptor(&self) -> ResourceDescriptor {
            self.descriptor.clone()
        }

        fn invoke(&self, _: &ResourceRequest) -> Result<ResourceResult, ResourceError> {
            if self.fail {
                return Err(ResourceError::Unavailable(self.descriptor.resource_id));
            }
            Ok(ResourceResult::new(
                self.descriptor.resource_id,
                serde_json::json!({ "text": self.answer }),
            ))
        }
    }

    /// A resource that lies about which resource produced the result.
    struct LyingResource;

    impl CognitiveResource for LyingResource {
        fn descriptor(&self) -> ResourceDescriptor {
            ResourceDescriptor {
                resource_id: ResourceId::from_u128(0x9),
                name: "liar".to_owned(),
                kind: ResourceKind::Generation,
                adapter: "stub".to_owned(),
                version: "1".to_owned(),
                read_only: true,
            }
        }

        fn invoke(&self, _: &ResourceRequest) -> Result<ResourceResult, ResourceError> {
            Ok(ResourceResult::new(
                ResourceId::from_u128(0xFFFF),
                serde_json::json!({ "text": "not mine" }),
            ))
        }
    }

    #[derive(Default)]
    struct MemoryLog(Mutex<Vec<ResourceCall>>);

    impl ResourceCallLog for MemoryLog {
        fn record(&self, call: NewResourceCall) -> Result<ResourceCall, ResourceCallLogError> {
            let stored = ResourceCall {
                resource_call_id: call.resource_call_id,
                resource_id: call.resource_id,
                slot: call.slot,
                individual_id: call.individual_id,
                adapter: call.adapter,
                purpose: call.purpose,
                request_digest: call.request_digest,
                outcome: call.outcome,
                result_digest: call.result_digest,
                error_code: call.error_code,
                started_at: call.started_at,
                completed_at: call.completed_at,
            };
            self.0.lock().unwrap().push(stored.clone());
            Ok(stored)
        }

        fn get_call(
            &self,
            resource_call_id: ResourceCallId,
        ) -> Result<Option<ResourceCall>, ResourceCallLogError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.resource_call_id == resource_call_id)
                .cloned())
        }

        fn calls(&self, _: IndividualId) -> Result<Vec<ResourceCall>, ResourceCallLogError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn request() -> ResourceRequest {
        ResourceRequest::new(
            IndividualId::from_u128(1),
            "summarize-turn",
            serde_json::json!({ "text": "お茶の話" }),
        )
    }

    fn invoke(
        registry: &ResourceRegistry,
        log: &MemoryLog,
        call_id: u128,
    ) -> Result<AttributedResult, RegistryError> {
        registry.invoke(
            &ResourceSlot::new("reasoning"),
            &request(),
            log,
            ResourceCallId::from_u128(call_id),
            UtcTimestamp::from_unix_millis(10),
            UtcTimestamp::from_unix_millis(20),
        )
    }

    #[test]
    fn replacement_changes_the_answer_and_the_attribution() {
        let mut registry = ResourceRegistry::new();
        let slot = ResourceSlot::new("reasoning");
        registry
            .register(
                slot.clone(),
                Arc::new(StubResource::new(0xA, "a", "answer-a")),
            )
            .unwrap();
        let log = MemoryLog::default();

        let first = invoke(&registry, &log, 1).unwrap();
        assert_eq!(first.result.resource_id, ResourceId::from_u128(0xA));
        assert_eq!(first.call.resource_id, ResourceId::from_u128(0xA));

        let previous = registry
            .replace(slot, Arc::new(StubResource::new(0xB, "b", "answer-b")))
            .unwrap()
            .expect("the slot was filled");
        assert_eq!(previous.resource_id, ResourceId::from_u128(0xA));

        let second = invoke(&registry, &log, 2).unwrap();
        assert_eq!(second.result.resource_id, ResourceId::from_u128(0xB));
        assert_eq!(second.call.resource_id, ResourceId::from_u128(0xB));
        assert_ne!(first.result.content, second.result.content);
        // Same request either way: the material changed, the question did not.
        assert_eq!(first.call.request_digest, second.call.request_digest);
        assert_ne!(first.call.result_digest, second.call.result_digest);
    }

    #[test]
    fn a_result_cannot_claim_a_resource_that_did_not_produce_it() {
        let mut registry = ResourceRegistry::new();
        registry
            .register(ResourceSlot::new("reasoning"), Arc::new(LyingResource))
            .unwrap();
        let log = MemoryLog::default();
        let attributed = invoke(&registry, &log, 1).unwrap();
        assert_eq!(attributed.result.resource_id, ResourceId::from_u128(0x9));
        assert_eq!(attributed.call.resource_id, ResourceId::from_u128(0x9));
    }

    #[test]
    fn a_failed_call_is_still_recorded() {
        let mut registry = ResourceRegistry::new();
        let mut failing = StubResource::new(0xA, "a", "answer-a");
        failing.fail = true;
        registry
            .register(ResourceSlot::new("reasoning"), Arc::new(failing))
            .unwrap();
        let log = MemoryLog::default();

        let error = invoke(&registry, &log, 1).expect_err("must surface the failure");
        assert!(matches!(
            error,
            RegistryError::Call(ResourceError::Unavailable(_))
        ));
        let calls = log.calls(IndividualId::from_u128(1)).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].outcome, ResourceOutcome::Error);
        assert_eq!(calls[0].error_code.as_deref(), Some("UNAVAILABLE"));
        assert_eq!(calls[0].result_digest, None);
    }

    #[test]
    fn an_empty_slot_is_an_error_not_a_silent_skip() {
        let registry = ResourceRegistry::new();
        let log = MemoryLog::default();
        assert!(matches!(
            invoke(&registry, &log, 1),
            Err(RegistryError::SlotEmpty(_))
        ));
        assert!(log.calls(IndividualId::from_u128(1)).unwrap().is_empty());
    }

    #[test]
    fn registering_over_an_occupied_slot_is_refused() {
        let mut registry = ResourceRegistry::new();
        let slot = ResourceSlot::new("reasoning");
        registry
            .register(
                slot.clone(),
                Arc::new(StubResource::new(0xA, "a", "answer-a")),
            )
            .unwrap();
        assert!(matches!(
            registry.register(slot, Arc::new(StubResource::new(0xB, "b", "answer-b"))),
            Err(RegistryError::SlotOccupied { .. })
        ));
    }

    #[test]
    fn request_digest_is_stable_and_carries_no_payload() {
        let digest = request().digest();
        assert_eq!(digest, request().digest());
        assert!(digest.starts_with("sha256:"));
        assert!(!digest.contains("お茶"));
    }
}
