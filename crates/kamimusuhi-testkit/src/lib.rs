//! Deterministic fakes and fixtures for Kamimusuhi tests.
//!
//! Nothing here is a stand-in for model quality. The fakes reproduce runtime
//! contracts deterministically so continuity bugs can be separated from model
//! behaviour. Fixture strings are test data, not Kamimusuhi's persona.

pub mod fake_persona;
pub mod fake_resource;
pub mod fixed_clock;
pub mod fixed_ids;

pub use fake_persona::FakePersonaCore;
pub use fake_resource::{FakeResource, UnavailableResource};
pub use fixed_clock::FixedClock;
pub use fixed_ids::FixedIdGenerator;
