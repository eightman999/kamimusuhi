//! Per-job execution deduplication: a stable OS lock plus a durable
//! reservation ledger beside the checkpoint.
//!
//! Layout under the canonicalized checkpoint directory:
//!
//! ```text
//! current_state/engineering.json.executions/
//!   <job_id>.lock   # held from admission through the workspace effect
//!   <job_id>.json   # reservation ledger, written atomically
//! ```
//!
//! The ledger answers "was this sequence already reserved for this job and
//! candidate?" even after every participant has exited. A sequence below or
//! equal to `high_water` is never executed a second time, whatever the
//! outcome of the earlier attempt was. `high_water` is never rewound.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use kamimusuhi_core::digest::json_digest;

use super::Job;
use super::lease::{self, AcquireError, Lease, Mode};
use super::worker::Request;
use super::workspace::Candidate;

/// The only ledger schema this implementation reads or writes.
pub(super) const LEDGER_SCHEMA_VERSION: u32 = 1;
/// Checkpoint marker value written when the ledger contract is introduced.
pub(super) const LEDGER_MARKER_VERSION: u32 = 1;
/// Ledgers are small fixed-shape records; anything larger is corrupt.
const MAX_LEDGER_BYTES: u64 = 4096;
/// The per-claim projection a worker verifies instead of the whole
/// checkpoint; large enough for any pending action yet firmly bounded.
const MAX_CLAIM_BYTES: u64 = 1024 * 1024;
/// Unique suffixes for our own temporary publish files.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Directory that owns one job's lockfile and ledger.
pub(super) struct Namespace {
    pub(super) dir: PathBuf,
}

/// Derive the namespace from the checkpoint path. Canonicalizing the parent
/// collapses symlink/relative aliases so the parent and a detached worker
/// land on the same directory; a vanished parent is an error, not something
/// to recreate silently.
pub(super) fn namespace(state_path: &Path) -> Result<Namespace, String> {
    let name = state_path
        .file_name()
        .ok_or_else(|| "engineering checkpoint name is invalid".to_owned())?;
    let mut dir_name = name.to_os_string();
    dir_name.push(".executions");
    let parent = state_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "engineering checkpoint directory is invalid".to_owned())?;
    let canonical = parent
        .canonicalize()
        .map_err(|_| "engineering checkpoint directory cannot be resolved".to_owned())?;
    Ok(Namespace {
        dir: canonical.join(dir_name),
    })
}

/// Candidate identity recorded in the ledger so a reservation cannot be
/// replayed onto a different worktree.
pub(super) fn candidate_digest(candidate: &Candidate) -> String {
    json_digest(&json!({ "schema_version": 1, "candidate": candidate }))
}

/// Wire-request identity recorded in the ledger when a sequence is reserved.
pub(super) fn request_digest(request: &Request) -> String {
    let request = serde_json::to_value(request).unwrap_or(Value::Null);
    json_digest(&json!({ "schema_version": 1, "request": request }))
}

/// Durable per-job reservation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ledger {
    pub schema_version: u32,
    pub job_id: String,
    pub candidate_digest: String,
    pub high_water: u64,
    pub request_digest: Option<String>,
}

impl Ledger {
    fn genesis(job_id: &str, candidate_digest: &str) -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            job_id: job_id.to_owned(),
            candidate_digest: candidate_digest.to_owned(),
            high_water: 0,
            request_digest: None,
        }
    }

    fn validate(&self, job_id: &str, expected_candidate: &str) -> Result<(), String> {
        if self.schema_version != LEDGER_SCHEMA_VERSION
            || self.job_id != job_id
            || self.candidate_digest != expected_candidate
            || !digest_valid(&self.candidate_digest)
        {
            return Err("engineering execution ledger identity mismatch".to_owned());
        }
        match (&self.request_digest, self.high_water) {
            (None, 0) => Ok(()),
            (Some(digest), n) if n > 0 && digest_valid(digest) => Ok(()),
            _ => Err("engineering execution ledger reservation is inconsistent".to_owned()),
        }
    }
}

fn digest_valid(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Read a job's ledger. `Ok(None)` means the ledger is genuinely absent;
/// corruption, identity mismatches, foreign shapes and unknown versions are
/// `Err`, never rewritten into a fresh empty ledger.
pub(super) fn read(
    namespace: &Namespace,
    job_id: &str,
    expected_candidate: &str,
) -> Result<Option<Ledger>, String> {
    let path = namespace.dir.join(format!("{job_id}.json"));
    let file = match open_regular(&path) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut contents = Vec::new();
    file.take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| "engineering execution ledger cannot be read".to_owned())?;
    if contents.len() as u64 > MAX_LEDGER_BYTES {
        return Err("engineering execution ledger exceeds its bounded size".to_owned());
    }
    let ledger: Ledger = serde_json::from_slice(&contents)
        .map_err(|_| "engineering execution ledger is corrupt".to_owned())?;
    ledger.validate(job_id, expected_candidate)?;
    Ok(Some(ledger))
}

/// Open the ledger only when it is a plain regular file carried by the
/// filesystem — symlink entries, directories, sockets and hardlinked files
/// are corruption evidence, not ledgers.
fn open_regular(path: &Path) -> Result<Option<File>, String> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("engineering execution ledger cannot be inspected".to_owned()),
    };
    if before.file_type().is_symlink() || !before.is_file() || before.len() > MAX_LEDGER_BYTES {
        return Err("engineering execution ledger is corrupt".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1 {
            return Err("engineering execution ledger is corrupt".to_owned());
        }
    }
    let file =
        File::open(path).map_err(|_| "engineering execution ledger cannot be opened".to_owned())?;
    let opened = file
        .metadata()
        .map_err(|_| "engineering execution ledger cannot be inspected".to_owned())?;
    if !opened.is_file() || opened.len() > MAX_LEDGER_BYTES || !same_file(&before, &opened) {
        return Err("engineering execution ledger is corrupt".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.nlink() != 1 {
            return Err("engineering execution ledger is corrupt".to_owned());
        }
    }
    Ok(Some(file))
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    true
}

/// Atomically publish `bytes` as `target` inside the namespace. The caller
/// must hold the job lock. A unique sibling temp file is created with
/// `create_new`, synced, renamed over the target and the namespace directory
/// synced. Failure after the write may leave the rename outcome uncertain —
/// callers report that as unknown rather than assuming either state.
fn publish_file(
    namespace: &Namespace,
    target: PathBuf,
    bytes: &[u8],
    max_bytes: u64,
) -> Result<(), String> {
    if bytes.len() as u64 > max_bytes {
        return Err("engineering execution record exceeds its bounded size".to_owned());
    }
    let job_id = target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("job");
    for _ in 0..8 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = namespace
            .dir
            .join(format!("{job_id}.tmp-{}-{sequence}", std::process::id()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err("engineering execution ledger cannot be published".to_owned());
            }
        };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "engineering execution record cannot be published".to_owned());
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        // Never overwrite a foreign or linked entry.
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                let _ = fs::remove_file(&temporary);
                return Err("engineering execution record target is corrupt".to_owned());
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                let _ = fs::remove_file(&temporary);
                return Err("engineering execution ledger cannot be published".to_owned());
            }
        }
        fs::rename(&temporary, &target).map_err(|_| {
            let _ = fs::remove_file(&temporary);
            "engineering execution record cannot be published".to_owned()
        })?;
        sync_directory(&namespace.dir)?;
        return Ok(());
    }
    Err("engineering execution record cannot be published".to_owned())
}

/// Atomically publish `ledger` as the job's durable record.
pub(super) fn publish(namespace: &Namespace, job_id: &str, ledger: &Ledger) -> Result<(), String> {
    let bytes = serde_json::to_vec(ledger)
        .map_err(|_| "engineering execution ledger cannot be encoded".to_owned())?;
    publish_file(
        namespace,
        namespace.dir.join(format!("{job_id}.json")),
        &bytes,
        MAX_LEDGER_BYTES,
    )
}

/// Publish the bounded per-claim projection the worker verifies in place of
/// the whole checkpoint. Written under the job lock after the pending claim
/// is committed; a crash leaves an absent or stale projection that the
/// worker refuses rather than guesses.
pub(super) fn publish_claim(
    namespace: &Namespace,
    job_id: &str,
    bytes: &[u8],
) -> Result<(), String> {
    publish_file(
        namespace,
        namespace.dir.join(format!("{job_id}.claim.json")),
        bytes,
        MAX_CLAIM_BYTES,
    )
}

/// Remove a spent claim projection after its dispatch completes. Best
/// effort: a stale or absent file is refused by the worker anyway, so a
/// removal failure only defers cleanup.
pub(super) fn remove_claim(namespace: &Namespace, job_id: &str) -> Result<(), String> {
    let path = namespace.dir.join(format!("{job_id}.claim.json"));
    match fs::remove_file(&path) {
        Ok(()) => sync_directory(&namespace.dir),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err("engineering claim projection cannot be removed".to_owned()),
    }
}

/// Read the bounded per-claim projection. `Ok(None)` means it is genuinely
/// absent; foreign shapes are `Err`.
pub(super) fn read_claim(namespace: &Namespace, job_id: &str) -> Result<Option<Vec<u8>>, String> {
    let path = namespace.dir.join(format!("{job_id}.claim.json"));
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("engineering claim projection cannot be inspected".to_owned()),
    };
    if before.file_type().is_symlink() || !before.is_file() || before.len() > MAX_CLAIM_BYTES {
        return Err("engineering claim projection is corrupt".to_owned());
    }
    let file = File::open(&path)
        .map_err(|_| "engineering claim projection cannot be opened".to_owned())?;
    let mut contents = Vec::new();
    file.take(MAX_CLAIM_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| "engineering claim projection cannot be read".to_owned())?;
    if contents.len() as u64 > MAX_CLAIM_BYTES {
        return Err("engineering claim projection exceeds its bounded size".to_owned());
    }
    Ok(Some(contents))
}

fn sync_directory(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|entry| entry.sync_all())
        .map_err(|_| "engineering state directory cannot be synced".to_owned())
}

/// Ensure the namespace directory is usable. `create` is allowed only to the
/// parent's initialization path; an absent directory under `create` is made
/// and its parent synced. Under `Existing` an absent or non-directory
/// namespace is unavailable.
fn ensure(namespace: &Namespace, create: bool) -> Result<(), AcquireError> {
    let unavailable = |error: &str| AcquireError::Unavailable(error.to_owned());
    match fs::symlink_metadata(&namespace.dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(
            unavailable("engineering execution namespace is not a directory"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound && create => {
            match fs::create_dir(&namespace.dir) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    return ensure(namespace, false);
                }
                Err(_) => {
                    return Err(unavailable(
                        "engineering execution namespace is unavailable",
                    ));
                }
            }
            if let Some(parent) = namespace.dir.parent() {
                sync_directory(parent).map_err(AcquireError::Unavailable)?;
            }
            Ok(())
        }
        Err(_) => Err(unavailable(
            "engineering execution namespace is unavailable",
        )),
    }
}

/// A held per-job lock. Dropping it releases the OS lock; the lockfile is
/// never deleted or replaced.
pub(super) struct JobLock {
    _lease: Lease,
}

/// Nonblocking acquisition of a job's execution lock. `Mode::Create` is for
/// the parent's first worker-targeted claim on a job that has no ledger
/// marker yet; `Mode::Existing` is required of workers and of every
/// participant once the marker is published.
pub(super) fn try_lock(
    namespace: &Namespace,
    job_id: &str,
    mode: Mode,
) -> Result<JobLock, AcquireError> {
    ensure(namespace, mode == Mode::Create)?;
    lease::acquire_mode(&namespace.dir.join(format!("{job_id}.lock")), mode)
        .map(|lease| JobLock { _lease: lease })
}

/// The parent's admission decision for a job about to claim a
/// worker-dispatched action.
pub(super) enum ParentAdmission {
    /// Claim creation may proceed; the caller writes marker and pending in
    /// one checkpoint transaction while holding the lock.
    Proceed,
    /// Stop execution durably; never recreate or reinterpret the ledger.
    Stop(String),
}

/// Evaluate marker/ledger/cursor consistency while holding the job lock.
///
/// | marker | ledger | decision |
/// |---|---|---|
/// | absent | absent | durable genesis, then claim |
/// | absent | genesis (`high_water=0`, no request) | adopt — crash left an unpublished marker |
/// | absent | positive reservation | stop |
/// | `1` | present, `high_water < next_action_sequence` | proceed |
/// | `1` | `high_water >= next_action_sequence` | stop — ledger ran ahead of the cursor |
/// | `1` | missing/corrupt | stop — never repaired into empty |
pub(super) fn parent_admission(namespace: &Namespace, job: &Job) -> ParentAdmission {
    let expected = candidate_digest(&job.candidate);
    let ledger = match read(namespace, &job.id, &expected) {
        Ok(ledger) => ledger,
        Err(error) => return ParentAdmission::Stop(error),
    };
    match (job.execution_ledger_version, ledger) {
        (None, None) => {
            let genesis = Ledger::genesis(&job.id, &expected);
            match publish(namespace, &job.id, &genesis) {
                Ok(()) => ParentAdmission::Proceed,
                Err(error) => ParentAdmission::Stop(error),
            }
        }
        (None, Some(ledger)) if ledger.high_water == 0 && ledger.request_digest.is_none() => {
            ParentAdmission::Proceed
        }
        (None, Some(_)) => ParentAdmission::Stop(
            "engineering execution ledger exists without its checkpoint marker".to_owned(),
        ),
        (Some(LEDGER_MARKER_VERSION), Some(ledger))
            if ledger.high_water < job.next_action_sequence =>
        {
            ParentAdmission::Proceed
        }
        (Some(LEDGER_MARKER_VERSION), Some(_)) => ParentAdmission::Stop(
            "engineering execution ledger exceeds the pending action cursor".to_owned(),
        ),
        (Some(LEDGER_MARKER_VERSION), None) => {
            ParentAdmission::Stop("engineering execution ledger is missing".to_owned())
        }
        (Some(_), _) => {
            ParentAdmission::Stop("engineering execution ledger marker is unsupported".to_owned())
        }
    }
}

/// Consistency gate used by `resume` and any parent-side mutation that must
/// not silently repair a lost or divergent ledger. The caller holds the job
/// lock. Returns the ledger when execution may continue.
pub(super) fn parent_consistency(
    namespace: &Namespace,
    job: &Job,
) -> Result<Option<Ledger>, String> {
    let expected = candidate_digest(&job.candidate);
    let ledger = read(namespace, &job.id, &expected)?;
    match (job.execution_ledger_version, &ledger) {
        (None, None) => Ok(ledger),
        (None, Some(existing)) if existing.high_water == 0 && existing.request_digest.is_none() => {
            Ok(ledger)
        }
        (None, Some(_)) => {
            Err("engineering execution ledger exists without its checkpoint marker".to_owned())
        }
        (Some(LEDGER_MARKER_VERSION), Some(existing))
            if existing.high_water < job.next_action_sequence =>
        {
            Ok(ledger)
        }
        (Some(LEDGER_MARKER_VERSION), Some(_)) => {
            Err("engineering execution ledger exceeds the pending action cursor".to_owned())
        }
        (Some(LEDGER_MARKER_VERSION), None) => {
            Err("engineering execution ledger is missing".to_owned())
        }
        (Some(_), _) => Err("engineering execution ledger marker is unsupported".to_owned()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn namespace_at(dir: &Path) -> Namespace {
        Namespace {
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn ledger_roundtrips_through_publish_and_read() {
        let temp = tempdir();
        let ns = namespace_at(&temp.path().join("executions"));
        fs::create_dir(&ns.dir).unwrap();
        let ledger = Ledger::genesis("eng-1", &format!("sha256:{}", "a".repeat(64)));
        publish(&ns, "eng-1", &ledger).unwrap();
        let read_back = read(&ns, "eng-1", &ledger.candidate_digest)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.high_water, 0);
        assert!(read_back.request_digest.is_none());
    }

    #[test]
    fn read_rejects_corrupt_and_foreign_ledger_entries() {
        let temp = tempdir();
        let ns = namespace_at(temp.path());
        let digest = format!("sha256:{}", "a".repeat(64));
        assert!(read(&ns, "eng-1", &digest).unwrap().is_none());
        fs::write(ns.dir.join("eng-1.json"), b"not json").unwrap();
        assert!(read(&ns, "eng-1", &digest).is_err());
        fs::remove_file(ns.dir.join("eng-1.json")).unwrap();
        fs::write(ns.dir.join("eng-1.json"), b"{}").unwrap();
        assert!(read(&ns, "eng-1", &digest).is_err());
        fs::remove_file(ns.dir.join("eng-1.json")).unwrap();
        let path = ns.dir.join("eng-1.json");
        fs::write(
            &path,
            serde_json::to_vec(&Ledger::genesis("eng-1", &digest)).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink("/dev/null", ns.dir.join("eng-2.json")).unwrap();
        assert!(read(&ns, "eng-2", &digest).is_err());
        // A hardlinked ledger is refused.
        fs::hard_link(&path, ns.dir.join("eng-3.json")).unwrap();
        assert!(
            fs::symlink_metadata(ns.dir.join("eng-3.json"))
                .unwrap()
                .nlink()
                == 2
        );
        assert!(read(&ns, "eng-3", &digest).is_err());
    }

    #[test]
    fn publish_never_leaves_its_temp_as_a_ledger() {
        let temp = tempdir();
        let ns = namespace_at(temp.path());
        let digest = format!("sha256:{}", "b".repeat(64));
        let mut ledger = Ledger::genesis("eng-1", &digest);
        ledger.high_water = 3;
        ledger.request_digest = Some(format!("sha256:{}", "c".repeat(64)));
        publish(&ns, "eng-1", &ledger).unwrap();
        let entries: Vec<_> = fs::read_dir(&ns.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1);
        let read_back = read(&ns, "eng-1", &digest).unwrap().unwrap();
        assert_eq!(read_back.high_water, 3);
        // Publishing again (a later sequence) replaces atomically.
        ledger.high_water = 7;
        publish(&ns, "eng-1", &ledger).unwrap();
        assert_eq!(read(&ns, "eng-1", &digest).unwrap().unwrap().high_water, 7);
    }

    #[test]
    fn job_lock_reports_busy_and_existing_modes() {
        let temp = tempdir();
        let ns = namespace_at(&temp.path().join("executions"));
        assert!(matches!(
            try_lock(&ns, "eng-1", Mode::Existing),
            Err(AcquireError::Unavailable(_))
        ));
        let first = try_lock(&ns, "eng-1", Mode::Create).unwrap();
        assert!(matches!(
            try_lock(&ns, "eng-1", Mode::Existing),
            Err(AcquireError::Busy)
        ));
        drop(first);
        try_lock(&ns, "eng-1", Mode::Existing).unwrap();
        // The lockfile persists after release.
        assert!(ns.dir.join("eng-1.lock").exists());
    }
}
