//! Bounded admission for synchronous, potentially non-cooperative providers.
//!
//! Rust cannot safely terminate arbitrary in-process code. A timed-out call
//! therefore keeps its provider's single-flight slot and its process-wide
//! permit until the actual code returns. There is no pending-job queue, and
//! replacing a session/provider cannot create more than the global worker
//! limit. This bounds host-created threads and retained requests; it is not an
//! OS memory sandbox for code that allocates indefinitely inside one call.
//!
//! The first admission installs a forwarding panic hook. It suppresses raw
//! panic diagnostics only while executing a provider job on its worker thread,
//! preserving the previous hook for host code and unrelated threads. Embedders
//! that replace the global hook afterwards must preserve this redaction policy;
//! provider-created threads are outside this in-process boundary.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Once, mpsc};
use std::time::Duration;

/// Shared by every session and provider in this process, including replacements.
pub const MAX_PROVIDER_WORKERS: usize = 64;
const WORKER_STACK_BYTES: usize = 2 * 1024 * 1024;
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
static INSTALL_PROVIDER_PANIC_HOOK: Once = Once::new();

thread_local! {
    // Only the provider job is redacted. Host completion sinks and panics on
    // every unrelated thread continue through the previously installed hook.
    static IN_PROVIDER_JOB: Cell<bool> = const { Cell::new(false) };
}

fn install_provider_panic_hook() {
    INSTALL_PROVIDER_PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !IN_PROVIDER_JOB.try_with(Cell::get).unwrap_or(false) {
                previous(info);
            }
        }));
    });
}

struct ProviderPanicScope {
    previous: bool,
}

impl ProviderPanicScope {
    fn enter() -> Self {
        Self {
            previous: IN_PROVIDER_JOB.with(|active| active.replace(true)),
        }
    }
}

impl Drop for ProviderPanicScope {
    fn drop(&mut self) {
        IN_PROVIDER_JOB.with(|active| active.set(self.previous));
    }
}

/// Fixed, secret-free boundary failures. Provider panic/error text is not copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryError {
    Timeout,
    Busy,
    Capacity,
    Panicked,
    Spawn,
}

impl BoundaryError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Timeout => "PROVIDER_TIMEOUT",
            Self::Busy => "PROVIDER_BUSY",
            Self::Capacity => "PROVIDER_CAPACITY",
            Self::Panicked => "PROVIDER_PANICKED",
            Self::Spawn => "PROVIDER_SPAWN",
        }
    }
}

impl std::fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for BoundaryError {}

/// One provider's admission slot. Clone this together with the provider Arc.
#[derive(Clone, Debug, Default)]
pub struct Boundary {
    busy: Arc<AtomicBool>,
}

struct Permit {
    busy: Arc<AtomicBool>,
    provider_released: bool,
}

impl Permit {
    fn release_provider(&mut self) {
        self.busy.store(false, Ordering::Release);
        self.provider_released = true;
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
        if !self.provider_released {
            self.busy.store(false, Ordering::Release);
        }
    }
}

/// Includes timed-out calls that are still running; never exceeds the limit.
pub fn active_provider_workers() -> usize {
    ACTIVE_WORKERS.load(Ordering::Acquire)
}

impl Boundary {
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Start immediately or refuse immediately. No request is queued.
    ///
    /// Dropping the receiver (on timeout/cancellation) does not release the
    /// permit. Only actual job completion/unwind releases it. The bounded
    /// result channel retains at most one response and late sends are dropped.
    pub fn try_spawn<T, F>(
        &self,
        job: F,
    ) -> Result<mpsc::Receiver<Result<T, BoundaryError>>, BoundaryError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.try_spawn_into(job, move |result| {
            let _ = tx.send(result);
        })?;
        Ok(rx)
    }

    /// Publish completion through a host-owned, nonblocking sink after the
    /// provider's single-flight slot is released. This lets a race use one
    /// shared result channel without a forwarding thread per provider.
    ///
    /// The global permit stays held until the sink returns, so even a broken
    /// sink cannot create unbounded worker threads. Sink panics unwind the
    /// worker and release the permit; no provider panic text reaches the sink.
    pub fn try_spawn_into<T, F, S>(&self, job: F, sink: S) -> Result<(), BoundaryError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        S: FnOnce(Result<T, BoundaryError>) + Send + 'static,
    {
        self.try_spawn_joinable_into(job, sink).map(|_| ())
    }

    /// Joinable host supervisor for process-isolated providers. Joining is
    /// safe only when the job has its own kill/reap deadline.
    pub fn try_spawn_joinable_into<T, F, S>(
        &self,
        job: F,
        sink: S,
    ) -> Result<std::thread::JoinHandle<()>, BoundaryError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        S: FnOnce(Result<T, BoundaryError>) + Send + 'static,
    {
        install_provider_panic_hook();
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| BoundaryError::Busy)?;
        if ACTIVE_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_PROVIDER_WORKERS).then_some(active + 1)
            })
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            return Err(BoundaryError::Capacity);
        }
        let mut permit = Permit {
            busy: Arc::clone(&self.busy),
            provider_released: false,
        };
        std::thread::Builder::new()
            .name("provider-call".to_owned())
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || {
                let result = {
                    let _panic_scope = ProviderPanicScope::enter();
                    catch_unwind(AssertUnwindSafe(job)).map_err(|_| BoundaryError::Panicked)
                };
                // The result proves completion. Release before publishing it
                // so an immediate bounded repair can reuse this provider.
                permit.release_provider();
                sink(result);
                drop(permit);
            })
            .map_err(|_| BoundaryError::Spawn)
    }

    /// Caller-side hard deadline, independent of provider cooperation.
    pub fn call<T, F>(&self, timeout: Duration, job: F) -> Result<T, BoundaryError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        if timeout.is_zero() {
            return Err(BoundaryError::Timeout);
        }
        let started = std::time::Instant::now();
        let rx = self.try_spawn(job)?;
        rx.recv_timeout(timeout.saturating_sub(started.elapsed()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => BoundaryError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => BoundaryError::Panicked,
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_quarantines_until_actual_completion_without_queueing() {
        let boundary = Boundary::default();
        let (release_tx, release_rx) = mpsc::channel();
        let result = boundary.call(Duration::from_millis(20), move || {
            release_rx.recv().unwrap();
            7
        });
        assert_eq!(result, Err(BoundaryError::Timeout));
        assert!(boundary.is_busy());
        for _ in 0..1_000 {
            assert!(matches!(boundary.try_spawn(|| 8), Err(BoundaryError::Busy)));
        }
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while boundary.is_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(boundary.call(Duration::from_secs(2), || 9), Ok(9));
    }

    #[test]
    fn clone_shares_quarantine_and_late_completion_is_discarded() {
        let boundary = Boundary::default();
        let clone = boundary.clone();
        let (release_tx, release_rx) = mpsc::channel();
        let rx = boundary
            .try_spawn(move || {
                release_rx.recv().unwrap();
                "late"
            })
            .unwrap();
        drop(rx);
        assert!(matches!(
            clone.try_spawn(|| "new"),
            Err(BoundaryError::Busy)
        ));
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while clone.is_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(clone.call(Duration::from_secs(2), || "new"), Ok("new"));
    }

    #[test]
    fn zero_deadline_never_invokes_job() {
        let boundary = Boundary::default();
        assert_eq!(
            boundary.call(Duration::ZERO, || panic!("must not run")),
            Err(BoundaryError::Timeout)
        );
        assert!(!boundary.is_busy());
    }

    #[test]
    fn panic_is_fixed_error_and_releases_slot() {
        let boundary = Boundary::default();
        assert_eq!(
            boundary.call(Duration::from_secs(2), || panic!("fixture")),
            Err(BoundaryError::Panicked)
        );
        assert_eq!(boundary.call(Duration::from_secs(2), || 1), Ok(1));
    }

    #[test]
    fn completion_sink_observes_released_provider_slot() {
        let boundary = Boundary::default();
        let clone = boundary.clone();
        let (tx, rx) = mpsc::channel();
        boundary
            .try_spawn_into(
                || 7,
                move |result| {
                    assert_eq!(result, Ok(7));
                    assert!(!clone.is_busy());
                    tx.send(clone.call(Duration::from_secs(2), || 8)).unwrap();
                },
            )
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), Ok(8));
    }

    #[test]
    fn panic_hook_redacts_provider_panics_without_hiding_unrelated_panics() {
        const CHILD: &str = "KAMIMUSUHI_PROVIDER_PANIC_HOOK_TEST_CHILD";
        const SECRET: &str = "FIXTURE_PROVIDER_SECRET_MUST_NOT_REACH_STDERR";
        if std::env::var_os(CHILD).is_some() {
            // A subprocess isolates the process-global hook from the rest of
            // the test harness, including concurrently running boundary tests.
            let forwarded = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&forwarded);
            std::panic::set_hook(Box::new(move |info| {
                observed.fetch_add(1, Ordering::AcqRel);
                eprintln!("FORWARDED_HOOK:{info}");
            }));
            let boundary = Boundary::default();
            assert_eq!(
                boundary.call(Duration::from_secs(2), || panic!("{SECRET}")),
                Err(BoundaryError::Panicked)
            );
            assert_eq!(forwarded.load(Ordering::Acquire), 0);

            // Check that the TLS scope is restored on the same worker before
            // invoking host code, not just on a separate unrelated thread.
            let (tx, rx) = mpsc::channel();
            boundary
                .try_spawn_into(
                    || 1,
                    move |result| {
                        assert_eq!(result, Ok(1));
                        let _ = catch_unwind(|| panic!("VISIBLE_SINK_PANIC"));
                        tx.send(()).unwrap();
                    },
                )
                .unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(forwarded.load(Ordering::Acquire), 1);
            let _ = catch_unwind(|| panic!("VISIBLE_UNRELATED_PANIC"));
            assert_eq!(forwarded.load(Ordering::Acquire), 2);
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "panic_hook_redacts_provider_panics_without_hiding_unrelated_panics",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains(SECRET));
        assert!(stderr.contains("VISIBLE_SINK_PANIC"));
        assert!(stderr.contains("VISIBLE_UNRELATED_PANIC"));
    }
}
