//! Process self-metrics for `/status`: RSS and average CPU.
//!
//! Linux reads `/proc`; other platforms report `null` fields rather than
//! failing. This is observability for the Pi's `RAM < 300MB / CPU < 5%`
//! idle budget — cheap, lock-free, and absent elsewhere.

use serde_json::{Value, json};

/// Parse `VmRSS` from `/proc/self/status` content. Returns kibibytes.
#[cfg(any(target_os = "linux", test))]
fn rss_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

/// Parse `utime + stime` jiffies from `/proc/self/stat` content.
/// The comm field is parenthesized and may contain spaces, so fields are
/// counted from the last `)`.
#[cfg(any(target_os = "linux", test))]
fn cpu_jiffies(stat: &str) -> Option<u64> {
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm: fields[0]=state, so utime is index 11, stime index 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// `{"rss_bytes", "cpu_seconds", "cpu_pct_avg"}` — `cpu_pct_avg` is CPU
/// over the process's measured lifetime, not instantaneous load.
pub fn read() -> Value {
    self_metrics()
}

#[cfg(target_os = "linux")]
fn self_metrics() -> Value {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rss = rss_kib(&status).map(|k| k * 1024);
    // Linux clock tick is almost universally 100Hz; a lifetime-average
    // figure for a health document does not need sysconf precision.
    let hz = 100.0;
    let cpu_seconds = cpu_jiffies(&stat).map(|j| j as f64 / hz);
    let cpu_pct = cpu_seconds.map(|s| {
        if elapsed < 1.0 {
            0.0
        } else {
            s / elapsed * 100.0
        }
    });
    json!({
        "rss_bytes": rss,
        "cpu_seconds": cpu_seconds,
        "cpu_pct_avg": cpu_pct,
    })
}

#[cfg(not(target_os = "linux"))]
fn self_metrics() -> Value {
    json!({"rss_bytes": null, "cpu_seconds": null, "cpu_pct_avg": null})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_status() {
        let status = "Name:\tkamimusuhi\nVmRSS:\t   12345 kB\nVmSize:\t999 kB\n";
        assert_eq!(rss_kib(status), Some(12345));
        assert_eq!(rss_kib("Name:\tx"), None);
    }

    #[test]
    fn parses_proc_stat_with_spaced_comm() {
        // utime=100, stime=50 (fields 14,15; after comm close: idx 11,12).
        let stat = "123 (weird name) S 1 2 3 4 5 6 7 8 9 10 100 50 0 0 20";
        assert_eq!(cpu_jiffies(stat), Some(150));
    }
}
