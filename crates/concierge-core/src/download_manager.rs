//! Background download manager (roadmap 0.3, "whole hog"). A long-lived,
//! process-global job table every download flows through, so the GUI can show a
//! live queue with per-file progress + speed and offer **pause / resume / cancel**
//! (per-download and globally) plus a **global bandwidth cap**. Downloads run on
//! the caller's worker thread (the `store::fetch_all` pool bounds concurrency);
//! the manager owns visibility and control, not its own thread pool.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::download::Control;
use crate::error::Result;

/// The lifecycle of one download.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JobState {
    Downloading,
    Paused,
    Done,
    Cancelled,
    Failed(String),
}

/// A snapshot of one job for the UI (no control handles).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JobView {
    pub id: u64,
    pub name: String,
    pub state: JobState,
    pub done: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
}

#[derive(Debug)]
struct Job {
    id: u64,
    name: String,
    state: JobState,
    done: u64,
    total: Option<u64>,
    bytes_per_sec: u64,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    /// (instant, bytes) of the last speed sample.
    sample: (Instant, u64),
}

#[derive(Debug)]
struct Inner {
    jobs: Mutex<Vec<Job>>,
    next_id: AtomicU64,
    global_pause: AtomicBool,
    limiter: RateLimiter,
}

/// The process-wide download manager.
#[derive(Debug)]
pub struct DownloadManager {
    inner: Arc<Inner>,
}

/// The global manager. Concurrency is bounded by the fetch pool; bandwidth by
/// [`Settings::max_bandwidth_kib`](crate::settings::Settings).
#[must_use]
pub fn global() -> &'static DownloadManager {
    static MANAGER: OnceLock<DownloadManager> = OnceLock::new();
    MANAGER.get_or_init(DownloadManager::new)
}

impl DownloadManager {
    fn new() -> Self {
        let kib = crate::settings::get().max_bandwidth_kib;
        Self {
            inner: Arc::new(Inner {
                jobs: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                global_pause: AtomicBool::new(false),
                limiter: RateLimiter::new(kib.saturating_mul(1024)),
            }),
        }
    }

    /// Run a download through the manager: register a job, fetch (resumable,
    /// retrying, cancel/pause/throttle-aware), and record the final state. Blocks
    /// on the calling thread; returns the fetch result.
    pub fn download(&self, name: &str, url: &str, dest: &Path) -> Result<()> {
        // Re-apply the current bandwidth setting each run (cheap; picks up changes).
        self.inner.limiter.set_limit(
            crate::settings::get()
                .max_bandwidth_kib
                .saturating_mul(1024),
        );
        let handle = self.register(name);
        let result = crate::download::fetch_to(url, dest, &handle);
        self.finish(handle.id, result.as_ref().err().map(ToString::to_string));
        result
    }

    /// Register a job and run its fetch on a NEW thread, returning the job id
    /// immediately (vs [`download`](Self::download), which blocks the caller).
    /// The daemon uses this so a client can poll the job by id while it runs on
    /// the daemon's own thread — reusing the same `register`/`fetch_to`/`finish`
    /// path, no duplicated download logic. Requires a `'static` manager (the
    /// process-global) so the worker thread can record the final state.
    pub fn spawn(&'static self, name: &str, url: String, dest: PathBuf) -> u64 {
        self.inner.limiter.set_limit(
            crate::settings::get()
                .max_bandwidth_kib
                .saturating_mul(1024),
        );
        let handle = self.register(name);
        let id = handle.id;
        std::thread::spawn(move || {
            let result = crate::download::fetch_to(&url, &dest, &handle);
            self.finish(id, result.as_ref().err().map(ToString::to_string));
        });
        id
    }

    fn register(&self, name: &str) -> JobHandle {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            jobs.push(Job {
                id,
                name: name.to_owned(),
                state: JobState::Downloading,
                done: 0,
                total: None,
                bytes_per_sec: 0,
                cancel: Arc::clone(&cancel),
                pause: Arc::clone(&pause),
                sample: (Instant::now(), 0),
            });
        }
        JobHandle {
            inner: Arc::clone(&self.inner),
            id,
            cancel,
            pause,
        }
    }

    fn finish(&self, id: u64, error: Option<String>) {
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                job.bytes_per_sec = 0;
                job.state = match error {
                    None => JobState::Done,
                    Some(e) if e.contains(crate::download::CANCELLED) => JobState::Cancelled,
                    Some(e) => JobState::Failed(e),
                };
            }
        }
    }

    /// A UI snapshot of all jobs (active + finished this session), newest last.
    #[must_use]
    pub fn snapshot(&self) -> Vec<JobView> {
        self.inner.jobs.lock().map_or_else(
            |_| Vec::new(),
            |jobs| {
                jobs.iter()
                    .map(|j| JobView {
                        id: j.id,
                        name: j.name.clone(),
                        state: j.state.clone(),
                        done: j.done,
                        total: j.total,
                        bytes_per_sec: j.bytes_per_sec,
                    })
                    .collect()
            },
        )
    }

    /// Pause / resume / cancel a single job by id.
    pub fn pause(&self, id: u64) {
        self.set_pause(id, true);
    }
    pub fn resume(&self, id: u64) {
        self.set_pause(id, false);
    }
    fn set_pause(&self, id: u64, paused: bool) {
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                job.pause.store(paused, Ordering::Relaxed);
                job.state = if paused {
                    JobState::Paused
                } else {
                    JobState::Downloading
                };
            }
        }
    }
    pub fn cancel(&self, id: u64) {
        if let Ok(jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.iter().find(|j| j.id == id) {
                job.cancel.store(true, Ordering::Relaxed);
                // unpause so a paused job can observe the cancel and unwind
                job.pause.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Global pause / resume — halts every active download.
    pub fn pause_all(&self) {
        self.inner.global_pause.store(true, Ordering::Relaxed);
    }
    pub fn resume_all(&self) {
        self.inner.global_pause.store(false, Ordering::Relaxed);
    }
    #[must_use]
    pub fn is_paused_globally(&self) -> bool {
        self.inner.global_pause.load(Ordering::Relaxed)
    }

    /// Drop finished/cancelled/failed jobs from the list (keep active ones).
    pub fn clear_finished(&self) {
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            jobs.retain(|j| matches!(j.state, JobState::Downloading | JobState::Paused));
        }
    }
}

/// The per-job link handed to the download loop.
#[derive(Debug)]
struct JobHandle {
    inner: Arc<Inner>,
    id: u64,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl Control for JobHandle {
    fn on_progress(&self, done: u64, total: Option<u64>) {
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == self.id) {
                // Speed: bytes since the last sample over the elapsed time, refreshed
                // about twice a second so the number is readable. Integer math
                // (bytes*1000/ms) — no float/`as` conversions.
                let now = Instant::now();
                let dt_ms = now.duration_since(job.sample.0).as_millis();
                if dt_ms >= 500 {
                    let delta = done.saturating_sub(job.sample.1);
                    let ms = u64::try_from(dt_ms).unwrap_or(1).max(1);
                    job.bytes_per_sec = delta.saturating_mul(1000) / ms;
                    job.sample = (now, done);
                }
                job.done = done;
                job.total = total;
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn wait_while_paused(&self) {
        while (self.pause.load(Ordering::Relaxed)
            || self.inner.global_pause.load(Ordering::Relaxed))
            && !self.cancel.load(Ordering::Relaxed)
        {
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    }

    fn throttle(&self, bytes: usize) {
        self.inner.limiter.take(bytes);
    }
}

/// Global bandwidth limiter: a simple fixed-window (1s) budget shared across all
/// downloads. `0` bytes/s means unlimited. Not a precise token bucket, but keeps
/// aggregate throughput near the cap without busy-waiting.
#[derive(Debug)]
struct RateLimiter {
    max_per_sec: AtomicU64,
    window: Mutex<(Instant, u64)>, // (window_start, bytes_used_this_window)
}

impl RateLimiter {
    fn new(max_per_sec: u64) -> Self {
        Self {
            max_per_sec: AtomicU64::new(max_per_sec),
            window: Mutex::new((Instant::now(), 0)),
        }
    }

    fn set_limit(&self, max_per_sec: u64) {
        self.max_per_sec.store(max_per_sec, Ordering::Relaxed);
    }

    fn take(&self, bytes: usize) {
        let max = self.max_per_sec.load(Ordering::Relaxed);
        if max == 0 {
            return; // unlimited
        }
        let sleep = self.reserve(bytes, max);
        if let Some(d) = sleep {
            std::thread::sleep(d);
        }
    }

    /// Reserve `bytes` against the window; return how long to sleep (if over budget).
    /// Pure-ish (state under the lock) so the accounting is testable.
    fn reserve(&self, bytes: usize, max: u64) -> Option<std::time::Duration> {
        let n = u64::try_from(bytes).unwrap_or(0);
        let mut g = self.window.lock().ok()?;
        let (start, used) = *g;
        let elapsed = start.elapsed();
        if elapsed.as_secs() >= 1 {
            // new window
            *g = (Instant::now(), n);
            return None;
        }
        let new_used = used.saturating_add(n);
        *g = (start, new_used);
        let over = new_used > max;
        drop(g); // release the lock before the (lock-independent) sleep calc
                 // over budget: sleep out the rest of this 1s window
        over.then(|| std::time::Duration::from_secs(1).saturating_sub(elapsed))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_throttles() {
        let rl = RateLimiter::new(0);
        // max==0 short-circuits in take(); reserve isn't consulted.
        rl.take(1_000_000);
        assert_eq!(rl.max_per_sec.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reserve_sleeps_only_when_over_budget() {
        let rl = RateLimiter::new(1000);
        // under budget → no sleep
        assert!(rl.reserve(500, 1000).is_none());
        // pushes over 1000 → sleep suggested
        assert!(rl.reserve(600, 1000).is_some());
    }

    #[test]
    fn manager_registers_and_finishes_jobs() {
        let mgr = DownloadManager::new();
        let h = mgr.register("TestMod");
        assert_eq!(mgr.snapshot().len(), 1);
        assert_eq!(mgr.snapshot()[0].state, JobState::Downloading);
        h.on_progress(50, Some(100));
        assert_eq!(mgr.snapshot()[0].done, 50);
        let id = h.id;
        drop(h);
        mgr.finish(id, None);
        assert_eq!(mgr.snapshot()[0].state, JobState::Done);
        mgr.clear_finished();
        assert!(mgr.snapshot().is_empty());
    }

    #[test]
    fn cancel_flag_is_observed() {
        let mgr = DownloadManager::new();
        let h = mgr.register("M");
        assert!(!h.cancelled());
        mgr.cancel(h.id);
        assert!(h.cancelled());
    }
}
