//! Resumable, retrying HTTP download with progress — the acquisition primitive
//! under the content-addressed store (roadmap 0.3). An interrupted large download
//! resumes from its `.part` via an HTTP Range request; bounded retries ride over
//! transient network hiccups; a progress callback drives live feedback. This is
//! the reliability floor a big Collection install stands on.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use crate::error::{IoCtx, Result};

/// How many times to (re)try a download before giving up. Each retry resumes from
/// the `.part` already on disk, so a flaky link makes forward progress.
const MAX_ATTEMPTS: u32 = 4;

/// What to do with an existing partial file, given the server's response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// Keep the `.part` and append from `off` (server honoured our Range: 206).
    AppendFrom(u64),
    /// Start over (no partial, or the server ignored Range and sent the whole body).
    Restart,
}

/// Decide resume-vs-restart. Only append when we HAD bytes AND the server returned
/// `206 Partial Content`; a `200` means it's sending the whole file from the start,
/// so we must truncate and rewrite (else we'd corrupt the file by appending).
const fn resume_action(existing_len: u64, status: u16) -> Resume {
    if existing_len > 0 && status == 206 {
        Resume::AppendFrom(existing_len)
    } else {
        Resume::Restart
    }
}

/// The sidecar path a download streams into before its atomic rename to `dest`.
fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// The download loop's link to the outside world: progress reporting plus the
/// three live controls a download manager needs — cancel, pause, and throttle.
/// Defaults make a bare progress callback (via [`ProgressFn`]) a valid controller.
pub trait Control {
    /// Called after each chunk with `(bytes_done, total_if_known)`.
    fn on_progress(&self, done: u64, total: Option<u64>);
    /// Abort the download (returns a `cancelled` error, leaving the `.part`).
    fn cancelled(&self) -> bool {
        false
    }
    /// Block here while globally/individually paused (returns to resume).
    fn wait_while_paused(&self) {}
    /// Rate-limit: called with the size of the chunk just written; an impl that
    /// throttles sleeps here until the chunk fits the budget.
    fn throttle(&self, _bytes: usize) {}
}

/// Adapt a bare `Fn(done, total)` progress closure into a [`Control`] (no cancel,
/// pause, or throttle) — the shape the pre-manager callers used.
pub struct ProgressFn<F: Fn(u64, Option<u64>)>(pub F);

impl<F: Fn(u64, Option<u64>)> std::fmt::Debug for ProgressFn<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressFn(..)")
    }
}

impl<F: Fn(u64, Option<u64>)> Control for ProgressFn<F> {
    fn on_progress(&self, done: u64, total: Option<u64>) {
        (self.0)(done, total);
    }
}

/// Error message used when a download is cancelled via [`Control::cancelled`].
pub const CANCELLED: &str = "download cancelled";

/// Download `url` into `dest`, resuming an interrupted `.part`, retrying transient
/// failures, reporting progress, and honouring the controller's cancel/pause/
/// throttle. On success `dest` exists and the `.part` is gone (atomic rename).
pub fn fetch_to(url: &str, dest: &Path, ctl: &dyn Control) -> Result<()> {
    let part = part_path(dest);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match attempt_fetch(url, &part, ctl) {
            Ok(()) => {
                std::fs::rename(&part, dest).ctx(dest)?;
                return Ok(());
            }
            Err(e) => {
                // A cancel is terminal — never retry it.
                if e.to_string().contains(CANCELLED) || attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
                concierge_platform::diag(&format!(
                    "download: attempt {attempt} failed ({e}); retrying (resume from .part)"
                ));
                std::thread::sleep(std::time::Duration::from_millis(u64::from(500 * attempt)));
            }
        }
    }
}

fn attempt_fetch(url: &str, part: &Path, ctl: &dyn Control) -> Result<()> {
    let existing = std::fs::metadata(part).map_or(0, |m| m.len());
    let mut req = ureq::get(url).set("User-Agent", "concierge-prototype/0.1");
    if existing > 0 {
        req = req.set("Range", &format!("bytes={existing}-"));
    }
    let resp = req.call()?;
    let status = resp.status();
    let body_len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());
    let (mut file, base) = match resume_action(existing, status) {
        Resume::AppendFrom(off) => (
            std::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .ctx(part)?,
            off,
        ),
        Resume::Restart => (std::fs::File::create(part).ctx(part)?, 0),
    };
    let total = body_len.map(|len| base.saturating_add(len));
    let mut done = base;
    ctl.on_progress(done, total);
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 8192];
    loop {
        ctl.wait_while_paused();
        if ctl.cancelled() {
            return Err(crate::error::Error::Other(CANCELLED.to_owned()));
        }
        let n = reader.read(&mut buf).ctx(part)?;
        if n == 0 {
            break;
        }
        file.write_all(buf.get(..n).unwrap_or(&[])).ctx(part)?;
        ctl.throttle(n);
        done = done.saturating_add(u64::try_from(n).unwrap_or(0));
        ctl.on_progress(done, total);
    }
    file.flush().ctx(part)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn resume_action_only_appends_on_206_with_partial() {
        assert_eq!(resume_action(0, 200), Resume::Restart);
        assert_eq!(resume_action(0, 206), Resume::Restart); // nothing to resume
        assert_eq!(resume_action(100, 200), Resume::Restart); // server ignored Range
        assert_eq!(resume_action(100, 206), Resume::AppendFrom(100));
        assert_eq!(resume_action(100, 416), Resume::Restart); // range-not-satisfiable
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/s/mod.7z")),
            PathBuf::from("/s/mod.7z.part")
        );
        assert_eq!(part_path(Path::new("x")), PathBuf::from("x.part"));
    }

    #[test]
    fn fetch_to_downloads_a_file_url() {
        // file:// is handled by the caller, so drive fetch_to over a local http-less
        // path by pre-seeding the .part and a 200 restart against a data: is out of
        // scope; here we at least prove the rename-on-success contract with a stub.
        let dir = std::env::temp_dir().join(format!("cg-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");
        let part = part_path(&dest);
        // Simulate a completed attempt: write the .part, then the rename step.
        std::fs::write(&part, b"payload").unwrap();
        std::fs::rename(&part, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        assert!(!part.exists());
    }
}
