//! Small std-only helpers: wall-clock formatting, bounded child processes,
//! atomic file replacement and calls that must not block their caller.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn unix_now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Proleptic Gregorian civil date from days since 1970-01-01 (Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `YYYY-MM-DDTHH:MM:SSZ` in UTC.
pub fn iso8601(secs: u64) -> String {
    let secs = i64::try_from(secs).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `YYYY-MM-DD` in UTC, used to partition journals.
pub fn utc_date(secs: u64) -> String {
    iso8601(secs)[..10].to_owned()
}

/// Compact sortable stamp for file names: `YYYYMMDDTHHMMSSZ`.
pub fn file_stamp(secs: u64) -> String {
    iso8601(secs).replace(['-', ':'], "")
}

/// Replace `path` with `bytes` via a sibling temp file and rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Outcome of a bounded child process.
#[derive(Debug)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run a command with a hard deadline. The child is killed when the deadline
/// passes, so a wedged tool (e.g. `nvidia-smi` on a sick driver) cannot pin a
/// probe thread forever.
pub fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Option<CommandOutput> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    status.map(|status| CommandOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

/// Run `f` on a helper thread and give up waiting after `timeout`.
///
/// Used for filesystem calls against network mounts: a stuck NFS request
/// strands only the helper thread, never the caller. `None` means the call
/// did not finish in time (it may still finish later, unobserved).
pub fn call_with_timeout<T, F>(timeout: Duration, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("bounded-call".to_owned())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(timeout).ok()
}

/// Constant-time comparison for bearer tokens.
pub fn secret_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_790_000_000), "2026-09-21T14:13:20Z");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(file_stamp(0), "19700101T000000Z");
    }

    #[test]
    fn bounded_call_gives_up() {
        let slow = call_with_timeout(Duration::from_millis(20), || {
            thread::sleep(Duration::from_millis(500));
            1
        });
        assert_eq!(slow, None);
        assert_eq!(call_with_timeout(Duration::from_secs(1), || 2), Some(2));
    }

    #[test]
    fn command_timeout_kills_child() {
        let started = Instant::now();
        let out = run_with_timeout("sleep", &["5"], Duration::from_millis(200));
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
        let ok = run_with_timeout("echo", &["hi"], Duration::from_secs(5)).expect("echo runs");
        assert!(ok.success);
        assert_eq!(ok.stdout.trim(), "hi");
    }

    #[test]
    fn secret_comparison() {
        assert!(secret_eq(b"abc", b"abc"));
        assert!(!secret_eq(b"abc", b"abd"));
        assert!(!secret_eq(b"abc", b"ab"));
    }
}
