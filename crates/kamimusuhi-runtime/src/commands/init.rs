//! `kamimusuhi-runtime init`: create/migrate the canonical database and
//! ensure a root individual exists.

use std::path::Path;

use kamimusuhi_core::ids::RandomIdGenerator;
use kamimusuhi_core::time::SystemWallClock;
use kamimusuhi_store_sqlite::SqliteContinuityStore;

pub fn run(db_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let store = SqliteContinuityStore::open(
        db_path,
        Box::new(SystemWallClock),
        Box::new(RandomIdGenerator),
    )?;

    let individuals = store.list_individuals()?;
    if individuals.is_empty() {
        let individual = store.create_root_individual()?;
        println!(
            "created root individual {} (root commit {}) at {}",
            individual.individual_id,
            individual.root_commit_id,
            db_path.display()
        );
    } else {
        println!(
            "database already initialized at {} with {} individual(s)",
            db_path.display(),
            individuals.len()
        );
    }

    Ok(())
}
