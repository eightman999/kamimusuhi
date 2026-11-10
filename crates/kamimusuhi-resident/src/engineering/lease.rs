//! Exclusive ownership of an engineering checkpoint directory.
//!
//! Lock a dedicated, stable file before reading or recovering the checkpoint.
//! Checkpoints are replaced atomically, so locking the checkpoint itself would
//! lock an obsolete inode after a save. Never unlink or replace the lockfile:
//! Dropping the owner explicitly unlocks it. After an abrupt process exit,
//! the OS releases it when the last inherited descriptor closes.
//!
//! This is a cooperative OS file lock, not distributed fencing. Network
//! filesystem semantics are not established by a successful local test.
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::path::Path;

#[derive(Debug)]
pub struct Lease {
    // Keep ownership for the containing Engineering's complete lifetime.
    // Do not expose cloning or explicit unlocking to callers.
    _file: File,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Concurrent fork/exec can briefly retain the open file description
        // even with CLOEXEC. Closing only our descriptor would leave a stale
        // lock until that child execs. Release the owned lock explicitly.
        let _ = self._file.unlock();
    }
}

fn validate_metadata(metadata: &Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink() {
        return Err("engineering state lock must not be a symlink".to_owned());
    }
    if !metadata.is_file() {
        return Err("engineering state lock must be a regular file".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("engineering state lock must not be hard-linked".to_owned());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    Err("engineering state lock identity checks are unavailable on this platform".to_owned())
}

fn inspect(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_metadata(&metadata)?;
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "engineering state lock cannot be inspected ({:?})",
            error.kind()
        )),
    }
}

#[cfg(unix)]
fn same_file(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &Metadata, _right: &Metadata) -> bool {
    false
}

fn verify_identity(path: &Path, opened: &Metadata) -> Result<(), String> {
    match inspect(path)? {
        Some(current) if same_file(opened, &current) => Ok(()),
        _ => Err("engineering state lock identity changed during acquisition".to_owned()),
    }
}

/// Acquire the dedicated lockfile, normally `engineering.lock` beside the
/// checkpoint. `path` must not name the checkpoint or a temporary file.
///
/// Contention is nonblocking. Any inspection, creation or locking failure
/// denies ownership; callers must not load or save a checkpoint without it.
/// The lockfile persists after this value is dropped.
pub fn acquire(path: &Path) -> Result<Lease, String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|_| "engineering state lock directory is unavailable".to_owned())?;
    }
    let before = inspect(path)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "engineering state lock cannot be opened ({:?})",
            error.kind()
        )
    })?;
    let opened = file
        .metadata()
        .map_err(|_| "engineering state lock handle cannot be inspected".to_owned())?;
    validate_metadata(&opened)?;
    if before.as_ref().is_some_and(|old| !same_file(old, &opened)) {
        return Err("engineering state lock identity changed during acquisition".to_owned());
    }
    verify_identity(path, &opened)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err("engineering state is already owned by another resident".to_owned());
        }
        Err(TryLockError::Error(error)) => {
            return Err(format!(
                "engineering state lock is unavailable ({:?})",
                error.kind()
            ));
        }
    }
    // Catch replacement during acquisition too. Cooperating owners never
    // replace this pathname; active filesystem tampering is not a sandbox.
    let owned = Lease { _file: file };
    verify_identity(path, &opened)?;
    Ok(owned)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            for _ in 0..64 {
                let path = std::env::temp_dir().join(format!(
                    "kamimusuhi-lease-{}-{}",
                    std::process::id(),
                    SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("temporary fixture directory: {error}"),
                }
            }
            panic!("temporary fixture directory names exhausted");
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn independent_opens_contend_until_drop_without_removing_or_truncating_lockfile() {
        use std::os::unix::fs::MetadataExt;
        let directory = Directory::new();
        let path = directory.0.join("engineering.lock");
        fs::write(&path, "preserved lock metadata").unwrap();
        let inode = fs::metadata(&path).unwrap().ino();
        let first = acquire(&path).unwrap();
        assert_eq!(
            acquire(&path).unwrap_err(),
            "engineering state is already owned by another resident"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "preserved lock metadata"
        );
        drop(first);
        assert!(
            path.exists(),
            "dropping ownership must not unlink the lockfile"
        );
        let second = acquire(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        drop(second);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "preserved lock metadata"
        );
    }

    #[test]
    fn different_state_directories_have_independent_owners() {
        let directory = Directory::new();
        let a = directory.0.join("a/engineering.lock");
        let b = directory.0.join("b/engineering.lock");
        let _first = acquire(&a).unwrap();
        let _second = acquire(&b).unwrap();
        assert!(a.is_file());
        assert!(b.is_file());
    }

    #[test]
    fn a_duplicate_descriptor_does_not_retain_released_ownership() {
        let directory = Directory::new();
        let path = directory.0.join("engineering.lock");
        let first = acquire(&path).unwrap();
        // A fork before exec can retain the same open file description.
        // Model that lifetime deterministically without a timing race.
        let inherited = first._file.try_clone().unwrap();
        drop(first);
        let second = acquire(&path).expect("owner drop must explicitly release its lock");
        drop(inherited);
        assert!(
            acquire(&path).is_err(),
            "old descriptor cannot release the new owner"
        );
        drop(second);
        assert!(acquire(&path).is_ok());
    }

    #[test]
    fn symlinks_hard_links_directories_and_special_files_are_refused() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        let original = directory.0.join("original");
        fs::write(&original, "untouched").unwrap();
        let symlink_path = directory.0.join("symlink");
        symlink(&original, &symlink_path).unwrap();
        assert!(acquire(&symlink_path).unwrap_err().contains("symlink"));
        let hard_link = directory.0.join("hard-link");
        fs::hard_link(&original, &hard_link).unwrap();
        assert!(acquire(&hard_link).unwrap_err().contains("hard-linked"));
        assert!(acquire(&directory.0).unwrap_err().contains("regular file"));
        // This must be rejected by metadata inspection, before opening it.
        assert!(
            acquire(Path::new("/dev/null"))
                .unwrap_err()
                .contains("regular file")
        );
        assert_eq!(fs::read_to_string(&original).unwrap(), "untouched");
    }
}
