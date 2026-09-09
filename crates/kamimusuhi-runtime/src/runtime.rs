//! Opening a runtime directory.
//!
//! The rule this module exists to enforce: **identity comes back from the
//! canonical database, or the command fails.** There is no path here that
//! reacts to a missing, corrupt or unreadable runtime by creating a fresh
//! individual. `init` creates one only when the database holds none; every
//! other entry point resumes what is already there or refuses.
//!
//! A runtime directory holds three separable things, and only the first is
//! canonical:
//!
//! ```text
//! kamimusuhi.sqlite   canonical state: identity, lineage, evidence, memory, Library
//! runtime.json        infrastructure: which fake fills which cognitive slot
//! trace.jsonl         observability: append-only, owns nothing
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kamimusuhi_core::continuity::{
    ContinuityHead, ContinuityKernel, ContinuityStore, Individual, NewIndividual, WriterIdentity,
};
use kamimusuhi_core::ids::{
    BootId, CommitId, IdGenerator, IndividualId, NodeId, RandomIdGenerator, SchemaVersion,
};
use kamimusuhi_core::mutation::MutationPolicyV0;
use kamimusuhi_core::time::{Clock, SystemClock};
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_store_sqlite::{SqliteStore, StoreConfig};
use kamimusuhi_testkit::{FixedClock, FixedIdGenerator};

use crate::config::RuntimeConfig;
use crate::error::RuntimeError;
use crate::trace::{JsonlTraceSink, TraceRecorder};

/// The kernel the runtime drives. Owns the store; everything else reaches
/// storage through [`Runtime::store`].
pub type RuntimeKernel = ContinuityKernel<SqliteStore, MutationPolicyV0, Arc<dyn Clock>>;

/// Determinism knobs.
///
/// A seed makes IDs and the clock reproducible so the W4 fixture is the same
/// on every machine. Without one the runtime uses real time and random IDs.
/// Two processes sharing a runtime directory must use different seeds, or
/// they would mint colliding IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeOptions {
    pub seed: Option<u64>,
}

impl RuntimeOptions {
    fn clock(self) -> Arc<dyn Clock> {
        match self.seed {
            Some(_) => Arc::new(FixedClock::baseline()),
            None => Arc::new(SystemClock::new()),
        }
    }

    fn ids(self) -> Arc<dyn IdGenerator> {
        match self.seed {
            Some(seed) => Arc::new(FixedIdGenerator::new(seed)),
            None => Arc::new(RandomIdGenerator),
        }
    }
}

/// Paths inside a runtime directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub dir: PathBuf,
}

impl RuntimePaths {
    pub const DB_FILE_NAME: &'static str = "kamimusuhi.sqlite";

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn database(&self) -> PathBuf {
        self.dir.join(Self::DB_FILE_NAME)
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join(RuntimeConfig::FILE_NAME)
    }

    pub fn trace(&self) -> PathBuf {
        self.dir.join(JsonlTraceSink::FILE_NAME)
    }

    fn display(&self) -> String {
        self.dir.display().to_string()
    }
}

/// An opened runtime: canonical store, configuration and trace, plus the
/// identity restored from disk.
pub struct Runtime {
    paths: RuntimePaths,
    config: RuntimeConfig,
    kernel: RuntimeKernel,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    trace: TraceRecorder,
    boot_id: BootId,
    individual: Individual,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("dir", &self.paths.dir)
            .field("individual_id", &self.individual.individual_id)
            .field("boot_id", &self.boot_id)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Create a runtime directory and, only if the database holds no
    /// individual, one individual.
    ///
    /// An existing database is adopted, never re-initialized: if it holds an
    /// individual, that individual is the one this runtime has. If it cannot
    /// be restored, this fails — a corrupt runtime is a problem to fix, not a
    /// reason to become someone else.
    pub fn init(
        dir: impl AsRef<Path>,
        options: RuntimeOptions,
        general: crate::config::FakeImplementation,
    ) -> Result<Self, RuntimeError> {
        let paths = RuntimePaths::new(dir.as_ref());
        std::fs::create_dir_all(&paths.dir).map_err(|source| RuntimeError::DirectoryIo {
            path: paths.display(),
            message: source.to_string(),
        })?;
        if paths.config().exists() {
            return Err(RuntimeError::AlreadyInitialized {
                path: paths.display(),
            });
        }

        let clock = options.clock();
        let ids = options.ids();
        let store = open_store(&paths, Arc::clone(&clock), Arc::clone(&ids))?;

        let node_id = NodeId::generate(ids.as_ref());
        let boot_id = BootId::generate(ids.as_ref());

        // Adopt an existing individual; create one only for an empty store.
        let existing = store.individuals()?;
        let individual = match existing.len() {
            0 => {
                let bootstrap = store.create_individual(NewIndividual {
                    individual_id: IndividualId::generate(ids.as_ref()),
                    root_commit_id: CommitId::generate(ids.as_ref()),
                    node_id,
                    boot_id,
                })?;
                bootstrap.individual
            }
            1 => existing[0],
            count => {
                return Err(RuntimeError::AmbiguousIndividual {
                    path: paths.display(),
                    count,
                });
            }
        };

        // Fails closed on a database that cannot account for its own head.
        store.load_head(individual.individual_id)?;

        let config = RuntimeConfig::new(node_id, general);
        config.save(&paths.config())?;

        Self::assemble(paths, config, store, clock, ids, boot_id, individual)
    }

    /// Open an initialized runtime and restore its individual from disk.
    ///
    /// This does not claim the writer epoch, so it is safe for read-only
    /// commands. A command that intends to mutate calls
    /// [`Runtime::claim_writer`] explicitly.
    pub fn open(dir: impl AsRef<Path>, options: RuntimeOptions) -> Result<Self, RuntimeError> {
        let paths = RuntimePaths::new(dir.as_ref());
        if !paths.config().exists() || !paths.database().exists() {
            return Err(RuntimeError::NotInitialized {
                path: paths.display(),
            });
        }
        let config = RuntimeConfig::load(&paths.config())?;
        let clock = options.clock();
        let ids = options.ids();
        let store = open_store(&paths, Arc::clone(&clock), Arc::clone(&ids))?;

        let existing = store.individuals()?;
        let individual = match existing.len() {
            0 => {
                // The one thing this runtime must never do is "recover" by
                // becoming a new individual.
                return Err(RuntimeError::NoIndividual {
                    path: paths.display(),
                });
            }
            1 => existing[0],
            count => {
                return Err(RuntimeError::AmbiguousIndividual {
                    path: paths.display(),
                    count,
                });
            }
        };
        store
            .load_head(individual.individual_id)
            .map_err(|_| RuntimeError::UnrestorableIndividual(individual.individual_id))?;

        let boot_id = BootId::generate(ids.as_ref());
        Self::assemble(paths, config, store, clock, ids, boot_id, individual)
    }

    fn assemble(
        paths: RuntimePaths,
        config: RuntimeConfig,
        store: SqliteStore,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        boot_id: BootId,
        individual: Individual,
    ) -> Result<Self, RuntimeError> {
        let sink = JsonlTraceSink::open(paths.trace(), ids.as_ref()).map_err(|source| {
            RuntimeError::DirectoryIo {
                path: paths.trace().display().to_string(),
                message: source.to_string(),
            }
        })?;
        let trace = TraceRecorder::new(
            sink,
            Arc::clone(&clock),
            TraceCorrelation {
                individual_id: Some(individual.individual_id),
                node_id: Some(config.node_id),
                boot_id: Some(boot_id),
                process_id: Some(std::process::id()),
                ..TraceCorrelation::default()
            },
        );
        let kernel = ContinuityKernel::new(store, MutationPolicyV0, Arc::clone(&clock));

        let runtime = Self {
            paths,
            config,
            kernel,
            clock,
            ids,
            trace,
            boot_id,
            individual,
        };
        runtime.trace.record_with(
            TraceEventKind::RuntimeBoot,
            TraceCorrelation::default(),
            serde_json::json!({
                "schema_version": runtime.schema_version(),
                "resources": runtime.config.resources,
            }),
        );
        Ok(runtime)
    }

    /// Take writer authority for this process, fencing out older epochs.
    ///
    /// This writes to the canonical store (a new epoch row and its audit
    /// event) but does not advance the continuity head. Read-only commands
    /// must not call it.
    pub fn claim_writer(&self) -> Result<WriterIdentity, RuntimeError> {
        Ok(self.kernel.store().claim_writer_epoch(
            self.individual.individual_id,
            self.config.node_id,
            self.boot_id,
        )?)
    }

    pub fn store(&self) -> &SqliteStore {
        self.kernel.store()
    }

    pub const fn kernel(&self) -> &RuntimeKernel {
        &self.kernel
    }

    pub const fn paths(&self) -> &RuntimePaths {
        &self.paths
    }

    pub const fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Rewrite the configuration, e.g. to point a slot at another fake.
    /// Infrastructure only: nothing canonical changes.
    pub fn save_config(&mut self, config: RuntimeConfig) -> Result<(), RuntimeError> {
        config.save(&self.paths.config())?;
        self.config = config;
        Ok(())
    }

    pub const fn trace(&self) -> &TraceRecorder {
        &self.trace
    }

    pub fn set_trace_base(&mut self, base: TraceCorrelation) {
        self.trace.set_base(base);
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    pub fn ids(&self) -> &Arc<dyn IdGenerator> {
        &self.ids
    }

    pub const fn boot_id(&self) -> BootId {
        self.boot_id
    }

    pub const fn individual(&self) -> Individual {
        self.individual
    }

    pub const fn individual_id(&self) -> IndividualId {
        self.individual.individual_id
    }

    pub fn head(&self) -> Result<ContinuityHead, RuntimeError> {
        Ok(self
            .kernel
            .store()
            .load_head(self.individual.individual_id)?)
    }

    pub fn schema_version(&self) -> SchemaVersion {
        self.kernel.store().schema_version()
    }

    pub fn stopping(&self) {
        self.trace
            .record(TraceEventKind::RuntimeStopping, TraceCorrelation::default());
    }
}

fn open_store(
    paths: &RuntimePaths,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
) -> Result<SqliteStore, RuntimeError> {
    Ok(SqliteStore::open(
        paths.database(),
        &StoreConfig::default(),
        clock,
        ids,
    )?)
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::continuity::Generation;

    use super::*;
    use crate::config::FakeImplementation;

    fn options(seed: u64) -> RuntimeOptions {
        RuntimeOptions { seed: Some(seed) }
    }

    #[test]
    fn init_creates_one_individual_at_generation_zero() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        let head = runtime.head().unwrap();
        assert_eq!(head.generation, Generation::ROOT);
        assert_eq!(head.individual_id, runtime.individual_id());
        assert!(runtime.paths().config().exists());
        assert!(runtime.paths().database().exists());
        assert!(runtime.paths().trace().exists());
    }

    #[test]
    fn reopening_restores_the_same_individual_without_minting_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        let individual = first.individual_id();
        drop(first);

        let second = Runtime::open(dir.path(), options(2)).unwrap();
        assert_eq!(second.individual_id(), individual);
        assert_eq!(second.store().individuals().unwrap().len(), 1);
        // A fresh process lifetime, but the same individual.
        assert_eq!(
            second.store().individuals().unwrap()[0].individual_id,
            individual
        );
    }

    #[test]
    fn init_refuses_an_already_initialized_directory() {
        let dir = tempfile::tempdir().unwrap();
        Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        assert!(matches!(
            Runtime::init(dir.path(), options(2), FakeImplementation::FakeA),
            Err(RuntimeError::AlreadyInitialized { .. })
        ));
    }

    #[test]
    fn opening_an_uninitialized_directory_does_not_create_an_individual() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Runtime::open(dir.path(), options(1)),
            Err(RuntimeError::NotInitialized { .. })
        ));
        assert!(!RuntimePaths::new(dir.path()).database().exists());
    }

    #[test]
    fn a_database_with_no_individual_is_refused_rather_than_repopulated() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        let db = runtime.paths().database();
        drop(runtime);

        // Simulate a runtime whose identity is gone: the store is intact, the
        // individual is not. Recovering by minting a new one would silently
        // replace the individual, so it must fail instead.
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DELETE FROM continuity_heads;
             DELETE FROM audit_events;
             DELETE FROM canonical_commits;
             DELETE FROM writer_epochs;
             DELETE FROM individuals;",
        )
        .unwrap();
        drop(conn);

        assert!(matches!(
            Runtime::open(dir.path(), options(2)),
            Err(RuntimeError::NoIndividual { .. })
        ));
    }

    #[test]
    fn an_individual_whose_head_is_gone_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        let db = runtime.paths().database();
        let individual = runtime.individual_id();
        drop(runtime);

        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF; DELETE FROM continuity_heads;")
            .unwrap();
        drop(conn);

        match Runtime::open(dir.path(), options(2)) {
            Err(RuntimeError::UnrestorableIndividual(id)) => assert_eq!(id, individual),
            other => panic!("expected a fail-closed restore, got {other:?}"),
        }
    }

    #[test]
    fn claiming_the_writer_epoch_advances_the_epoch_but_not_the_head() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::init(dir.path(), options(1), FakeImplementation::FakeA).unwrap();
        let before = runtime.head().unwrap();
        let writer = runtime.claim_writer().unwrap();
        let after = runtime.head().unwrap();
        assert!(writer.writer_epoch > before.writer_epoch);
        assert_eq!(after.commit_id, before.commit_id);
        assert_eq!(after.generation, before.generation);
    }
}
