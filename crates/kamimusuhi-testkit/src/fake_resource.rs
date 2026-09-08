//! Deterministic fake cognitive resources.
//!
//! Fake A and Fake B exist to prove one thing: swapping the resource that
//! does the thinking changes the *material* and its attribution, and changes
//! nothing about who the individual is (plan §9.2). They are not a quality
//! comparison, and nothing here stands in for a model.
//!
//! Each fake echoes the request in a shape that is obviously its own, so a
//! test can tell the two apart by `ResourceId` — never by reading the text.

use kamimusuhi_core::ids::ResourceId;
use kamimusuhi_core::resources::{
    CognitiveResource, ResourceDescriptor, ResourceError, ResourceKind, ResourceRequest,
    ResourceResult,
};

/// Fixed IDs so a stored `resource_calls` row is recognisable across runs.
pub const FAKE_A_RESOURCE_ID: ResourceId = ResourceId::from_u128(0x0FAA);
pub const FAKE_B_RESOURCE_ID: ResourceId = ResourceId::from_u128(0x0FAB);
pub const UNAVAILABLE_RESOURCE_ID: ResourceId = ResourceId::from_u128(0x0FAC);

/// One deterministic fake. Two instances with different `answer` values are
/// two distinct resources, not two configurations of one.
#[derive(Debug, Clone)]
pub struct FakeResource {
    resource_id: ResourceId,
    name: &'static str,
    answer: &'static str,
}

impl FakeResource {
    /// Fake A: answers `result-a`.
    pub const fn a() -> Self {
        Self {
            resource_id: FAKE_A_RESOURCE_ID,
            name: "fake-a",
            answer: "result-a",
        }
    }

    /// Fake B: same interface, different identity and different answer.
    pub const fn b() -> Self {
        Self {
            resource_id: FAKE_B_RESOURCE_ID,
            name: "fake-b",
            answer: "result-b",
        }
    }

    pub const fn resource_id(&self) -> ResourceId {
        self.resource_id
    }
}

impl CognitiveResource for FakeResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor {
            resource_id: self.resource_id,
            name: self.name.to_owned(),
            kind: ResourceKind::Generation,
            adapter: "fake".to_owned(),
            version: "1".to_owned(),
            read_only: true,
        }
    }

    fn invoke(&self, request: &ResourceRequest) -> Result<ResourceResult, ResourceError> {
        if request.individual_id.is_nil() {
            return Err(ResourceError::InvalidRequest {
                reason: "request has no individual".to_owned(),
            });
        }
        Ok(ResourceResult::new(
            self.resource_id,
            serde_json::json!({
                "answer": self.answer,
                "produced_by": self.name,
                "echo": request.input,
            }),
        ))
    }
}

/// A resource that always fails, for the controlled-error path.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableResource;

impl CognitiveResource for UnavailableResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor {
            resource_id: UNAVAILABLE_RESOURCE_ID,
            name: "fake-unavailable".to_owned(),
            kind: ResourceKind::Generation,
            adapter: "fake".to_owned(),
            version: "1".to_owned(),
            read_only: true,
        }
    }

    fn invoke(&self, _: &ResourceRequest) -> Result<ResourceResult, ResourceError> {
        Err(ResourceError::Unavailable(UNAVAILABLE_RESOURCE_ID))
    }
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::ids::IndividualId;

    use super::*;

    fn request() -> ResourceRequest {
        ResourceRequest::new(
            IndividualId::from_u128(1),
            "summarize-turn",
            serde_json::json!({ "text": "お茶の話" }),
        )
    }

    #[test]
    fn a_and_b_are_distinct_resources_with_distinct_answers() {
        let a = FakeResource::a().invoke(&request()).unwrap();
        let b = FakeResource::b().invoke(&request()).unwrap();
        assert_ne!(a.resource_id, b.resource_id);
        assert_ne!(a.content, b.content);
        assert_eq!(a.content["answer"], "result-a");
        assert_eq!(b.content["answer"], "result-b");
    }

    #[test]
    fn the_same_request_always_gets_the_same_answer() {
        let first = FakeResource::a().invoke(&request()).unwrap();
        let second = FakeResource::a().invoke(&request()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn both_fakes_declare_themselves_read_only() {
        for descriptor in [
            FakeResource::a().descriptor(),
            FakeResource::b().descriptor(),
            UnavailableResource.descriptor(),
        ] {
            assert!(descriptor.read_only);
            assert_eq!(descriptor.adapter, "fake");
        }
    }

    #[test]
    fn the_unavailable_fake_returns_a_controlled_error() {
        assert_eq!(
            UnavailableResource.invoke(&request()),
            Err(ResourceError::Unavailable(UNAVAILABLE_RESOURCE_ID))
        );
    }
}
