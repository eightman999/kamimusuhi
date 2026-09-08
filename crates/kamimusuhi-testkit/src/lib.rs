//! `kamimusuhi-testkit`: deterministic fixtures shared by tests across the
//! workspace. Depends only on `kamimusuhi-core`, never on
//! `kamimusuhi-store-sqlite` (a store's tests may depend on this crate as
//! a dev-dependency, but not the other way around).

pub mod fake_persona;
pub mod fake_resource;
pub mod fixed_clock;
pub mod fixed_ids;
pub mod fixtures;
