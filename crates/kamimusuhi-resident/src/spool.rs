//! Local spool with deferred delivery to the NAS.
//!
//! Every durable write goes to the local spool first; nothing on a request or
//! heartbeat path ever touches the NAS. A separate syncer thread delivers the
//! spool into the NAS tree when the NAS is healthy, so a NAS outage (or a hung
//! NFS server) only delays delivery.
//!
//! The spool mirrors NAS-relative paths:
//!
//! * `*.jsonl` journals are rotated to `*.jsonl.<stamp>-<n>.ready` under the
//!   journal lock and then **appended** to the NAS file of the same name.
//!   Delivery is at-least-once: a crash between append and unlink can repeat
//!   a batch, so every record carries a unique `id`.
//! * Any other file is copied whole (temp name + rename) and then removed.
//!
//! Single-writer rule: journal paths embed the node id
//! (`logs/<category>/<node>/<date>.jsonl`), so the two nodes never append to
//! the same NAS file.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::util::{file_stamp, unix_now, utc_date};

const READY: &str = ".ready";
const TMP_MARK: &str = ".tmp-";

pub struct Spool {
    root: PathBuf,
    node: String,
    /// Serializes appends against rotation so no line is split or lost.
    lock: Mutex<()>,
    seq: AtomicU64,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncReport {
    pub delivered_files: u64,
    pub delivered_bytes: u64,
    pub error: Option<String>,
}

impl Spool {
    pub fn new(root: PathBuf, node: &str) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            node: node.to_owned(),
            lock: Mutex::new(()),
            seq: AtomicU64::new(0),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Append one record to `<category>/<node>/<date>.jsonl`.
    ///
    /// `category` is a relative path such as `logs/heartbeat` or
    /// `conversations`. The record gets `id`, `ts` and `node` if absent.
    pub fn append(&self, category: &str, mut record: Value) -> io::Result<()> {
        let now = unix_now();
        if let Value::Object(map) = &mut record {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            map.entry("id").or_insert_with(|| {
                Value::String(format!(
                    "{}-{}-{seq}",
                    self.node,
                    crate::util::unix_now_ms()
                ))
            });
            map.entry("ts")
                .or_insert_with(|| Value::String(crate::util::iso8601(now)));
            map.entry("node")
                .or_insert_with(|| Value::String(self.node.clone()));
        }
        let mut line = serde_json::to_string(&record).map_err(io::Error::other)?;
        line.push('\n');
        let path = self
            .root
            .join(safe_relative(category)?)
            .join(&self.node)
            .join(format!("{}.jsonl", utc_date(now)));
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(line.as_bytes())
    }

    /// Place a whole file for delivery at NAS-relative `relative`.
    pub fn stage_file(&self, relative: &str) -> io::Result<PathBuf> {
        let path = self.root.join(safe_relative(relative)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    /// Bytes and files currently waiting in the spool.
    pub fn backlog(&self) -> (u64, u64) {
        let mut bytes = 0;
        let mut files = 0;
        walk(&self.root, &mut |path, len| {
            if !is_tmp(path) {
                bytes += len;
                files += 1;
            }
        });
        (bytes, files)
    }

    /// Rotate live journals so they can be delivered without racing appends.
    fn rotate(&self) -> io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut live = Vec::new();
        walk(&self.root, &mut |path, _| {
            if path.extension().is_some_and(|e| e == "jsonl") {
                live.push(path.to_path_buf());
            }
        });
        let stamp = file_stamp(unix_now());
        for path in live {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let mut name = path.as_os_str().to_owned();
            name.push(format!(".{stamp}-{seq:06}{READY}"));
            fs::rename(&path, PathBuf::from(name))?;
        }
        Ok(())
    }

    /// Deliver everything to `nas_root`. Runs on the syncer thread only.
    pub fn deliver(&self, nas_root: &Path) -> SyncReport {
        let mut report = SyncReport::default();
        if let Err(e) = self.rotate() {
            report.error = Some(format!("rotate: {e}"));
            return report;
        }
        // Journals first, oldest batch first, grouped by target file.
        let mut ready: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut whole = Vec::new();
        walk(&self.root, &mut |path, _| {
            let name = path.to_string_lossy();
            if is_tmp(path) {
                return;
            }
            if let Some(stripped) = name.strip_suffix(READY)
                && let Some(pos) = stripped.rfind(".jsonl.")
            {
                ready
                    .entry(PathBuf::from(&stripped[..pos + ".jsonl".len()]))
                    .or_default()
                    .push(path.to_path_buf());
            } else if path.extension().is_none_or(|e| e != "jsonl") {
                whole.push(path.to_path_buf());
            }
        });
        for (target, mut batches) in ready {
            batches.sort();
            for batch in batches {
                match self.append_to_nas(&batch, &target, nas_root) {
                    Ok(bytes) => {
                        report.delivered_files += 1;
                        report.delivered_bytes += bytes;
                    }
                    Err(e) => {
                        report.error = Some(format!("append {}: {e}", self.rel(&target)));
                        return report;
                    }
                }
            }
        }
        whole.sort();
        for path in whole {
            match self.copy_to_nas(&path, nas_root) {
                Ok(bytes) => {
                    report.delivered_files += 1;
                    report.delivered_bytes += bytes;
                }
                Err(e) => {
                    report.error = Some(format!("copy {}: {e}", self.rel(&path)));
                    return report;
                }
            }
        }
        prune_empty_dirs(&self.root);
        report
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn append_to_nas(&self, batch: &Path, target: &Path, nas_root: &Path) -> io::Result<u64> {
        let rel = target.strip_prefix(&self.root).map_err(io::Error::other)?;
        let dest = nas_root.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = fs::read(batch)?;
        let mut out = OpenOptions::new().create(true).append(true).open(&dest)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        fs::remove_file(batch)?;
        Ok(bytes.len() as u64)
    }

    fn copy_to_nas(&self, path: &Path, nas_root: &Path) -> io::Result<u64> {
        let rel = path.strip_prefix(&self.root).map_err(io::Error::other)?;
        let dest = nas_root.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}{TMP_MARK}{}", dest.display(), self.node));
        let bytes = fs::copy(path, &tmp)?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, &dest)?;
        fs::remove_file(path)?;
        Ok(bytes)
    }

    /// Keep at most `keep` staged files in `relative_dir` (oldest dropped).
    /// Bounds local disk use for snapshots during a long NAS outage.
    pub fn trim_staged(&self, relative_dir: &str, keep: usize) -> io::Result<()> {
        let dir = self.root.join(safe_relative(relative_dir)?);
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(());
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && !is_tmp(p))
            .collect();
        files.sort();
        let excess = files.len().saturating_sub(keep);
        for path in files.into_iter().take(excess) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn is_tmp(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().contains(TMP_MARK))
}

/// Reject absolute paths and `..` so a category can never escape the spool.
fn safe_relative(relative: &str) -> io::Result<&Path> {
    let path = Path::new(relative);
    let ok = !relative.is_empty()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if ok {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "spool path must be relative without '..'",
        ))
    }
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path, u64)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk(&path, f);
        } else if meta.is_file() {
            f(&path, meta.len());
        }
    }
}

fn prune_empty_dirs(root: &Path) {
    fn inner(dir: &Path, is_root: bool) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    inner(&entry.path(), false);
                }
            }
        }
        if !is_root {
            let _ = fs::remove_dir(dir);
        }
    }
    inner(root, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn journals_append_and_deliver_at_least_once() {
        let spool_dir = tempfile::tempdir().expect("tempdir");
        let nas = tempfile::tempdir().expect("tempdir");
        let spool = Spool::new(spool_dir.path().to_path_buf(), "pi").expect("spool");

        spool
            .append("logs/heartbeat", json!({"n": 1}))
            .expect("append");
        spool
            .append("logs/heartbeat", json!({"n": 2}))
            .expect("append");
        let report = spool.deliver(nas.path());
        assert!(report.error.is_none(), "{report:?}");

        // A second batch appends to the same NAS file.
        spool
            .append("logs/heartbeat", json!({"n": 3}))
            .expect("append");
        spool.deliver(nas.path());

        let date = utc_date(unix_now());
        let delivered = fs::read_to_string(
            nas.path()
                .join("logs/heartbeat/pi")
                .join(format!("{date}.jsonl")),
        )
        .expect("delivered");
        let lines: Vec<Value> = delivered
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2]["n"], 3);
        assert_eq!(lines[0]["node"], "pi");
        assert!(lines[0]["id"].is_string());
        assert_eq!(spool.backlog(), (0, 0));
    }

    #[test]
    fn failed_delivery_keeps_spool() {
        let spool_dir = tempfile::tempdir().expect("tempdir");
        let spool = Spool::new(spool_dir.path().to_path_buf(), "pi").expect("spool");
        spool
            .append("conversations", json!({"q": "hi"}))
            .expect("append");
        // A regular file where the NAS directory should be: every write fails.
        let blocker = tempfile::NamedTempFile::new().expect("file");
        let report = spool.deliver(blocker.path());
        assert!(report.error.is_some());
        assert_eq!(spool.backlog().1, 1, "ready batch still waiting");
    }

    #[test]
    fn whole_files_are_copied_and_trimmed() {
        let spool_dir = tempfile::tempdir().expect("tempdir");
        let nas = tempfile::tempdir().expect("tempdir");
        let spool = Spool::new(spool_dir.path().to_path_buf(), "pi").expect("spool");
        for i in 0..5 {
            let path = spool
                .stage_file(&format!("memory/snapshots/pi/{i}.sqlite"))
                .expect("stage");
            fs::write(path, [i]).expect("write");
        }
        spool.trim_staged("memory/snapshots/pi", 2).expect("trim");
        assert_eq!(spool.backlog().1, 2);
        spool.deliver(nas.path());
        assert!(nas.path().join("memory/snapshots/pi/4.sqlite").exists());
        assert!(!nas.path().join("memory/snapshots/pi/0.sqlite").exists());
        assert_eq!(spool.backlog(), (0, 0));
    }

    #[test]
    fn rejects_escaping_paths() {
        let spool_dir = tempfile::tempdir().expect("tempdir");
        let spool = Spool::new(spool_dir.path().to_path_buf(), "pi").expect("spool");
        assert!(spool.append("../x", json!({})).is_err());
        assert!(spool.stage_file("/etc/passwd").is_err());
    }
}
