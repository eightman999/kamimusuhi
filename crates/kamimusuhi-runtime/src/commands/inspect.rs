//! `kamimusuhi-runtime inspect`: read-only summary of a canonical
//! database. Opening the store already performs full recovery (PRAGMA
//! re-verification, migration-is-a-no-op if already current).

use std::path::Path;

use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::ids::RandomIdGenerator;
use kamimusuhi_core::time::SystemWallClock;
use kamimusuhi_store_sqlite::SqliteContinuityStore;

pub fn run(db_path: &Path) -> anyhow::Result<()> {
    let store = SqliteContinuityStore::open(
        db_path,
        Box::new(SystemWallClock),
        Box::new(RandomIdGenerator),
    )?;

    let individuals = store.list_individuals()?;
    println!("database: {}", db_path.display());
    println!("individuals: {}", individuals.len());
    for individual in individuals {
        let head = store.load_head(individual.individual_id)?;
        let commit_count = store.count_commits_for_individual(individual.individual_id)?;
        println!(
            "  - individual_id={} root_commit_id={} created_at={} head_generation={} head_commit_id={} commit_count={}",
            individual.individual_id,
            individual.root_commit_id,
            individual.created_at.to_rfc3339(),
            head.generation,
            head.commit_id,
            commit_count,
        );
    }

    Ok(())
}
