//! Lightweight file diagnostics. A GUI app on Windows has no console, so
//! `env_logger`'s stderr is lost and a failed terminal spawn is otherwise
//! invisible. We write logs NEXT TO THE EXECUTABLE (falling back to the temp
//! dir when that isn't writable) so they can be read after the fact — e.g.
//! synced back off the user's machine via a shared folder.
//!
//! Every line is ALSO mirrored to a STABLE per-user location
//! (`<config_dir>/concierge-gui.log`) so there is always ONE canonical log to
//! grab regardless of where the exe was launched from — running a copy from
//! `Downloads` used to scatter its log there, out of the synced folder, so a
//! "nothing happened" report had no on-disk trace to read. Std-only.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The directory logs are written to: beside the exe if writable (so it travels
/// with the app), else the system temp dir.
fn log_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .filter(|d| is_writable(d))
            .unwrap_or_else(std::env::temp_dir)
    })
}

/// A stable, per-user log location that does NOT move with the exe — so no
/// matter which copy of Concierge was launched (Bear Share, `Downloads`, a
/// re-install), its log lands in the same findable place. `None` if the
/// directory can't be created.
fn stable_log_file() -> Option<&'static PathBuf> {
    static F: OnceLock<Option<PathBuf>> = OnceLock::new();
    F.get_or_init(|| {
        let dir = concierge_platform::config_dir();
        std::fs::create_dir_all(&dir).ok()?;
        // Only keep it if it's a DIFFERENT file from the exe-adjacent one, so we
        // don't double-write the same path.
        let f = dir.join("concierge-gui.log");
        (f != log_dir().join("concierge-gui.log")).then_some(f)
    })
    .as_ref()
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".concierge-write-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The general diagnostics log (startup, actions, terminal spawn results).
#[must_use]
pub fn log_file() -> PathBuf {
    log_dir().join("concierge-gui.log")
}

/// Create a fresh per-open session directory (`<logdir>/shell-<unixsecs>/`) and
/// return it. Everything for one "open shell" — the trace from every layer, the
/// terminal transcript, the exact sandbox script — lands inside it, so a failure
/// is a single self-contained, greppable folder that syncs back with the app.
#[must_use]
pub fn new_session() -> PathBuf {
    let dir = log_dir().join(format!("shell-{}", stamp()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Append a line to the diagnostics log (best-effort; never panics). Written to
/// BOTH the exe-adjacent log (travels with the app / syncs back via Bear Share)
/// and the stable per-user log (always the same findable place).
pub fn log(msg: &str) {
    let line = format!("[{}] {msg}\n", stamp());
    append(&log_file(), &line);
    if let Some(stable) = stable_log_file() {
        append(stable, &line);
    }
}

fn append(path: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Write the session banner: version, OS, exe path, and where logs go — so the
/// very top of a synced-back log identifies exactly which build produced it.
pub fn start_session() {
    log("======================================================================");
    log(&format!(
        "concierge-gui {} · {}/{} · pid {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id(),
    ));
    if let Ok(exe) = std::env::current_exe() {
        log(&format!("exe: {}", exe.display()));
    }
    log(&format!(
        "args: {:?}",
        std::env::args().skip(1).collect::<Vec<_>>()
    ));
    log(&format!("log (beside exe): {}", log_file().display()));
    if let Some(stable) = stable_log_file() {
        log(&format!("log (stable):     {}", stable.display()));
    }
    log(&format!(
        "nxm inbox: {}",
        concierge::nexus::nxm_inbox_path().display()
    ));
    log(&format!(
        "nexus api key configured: {}",
        concierge::nexus::api_key().is_ok()
    ));
}
