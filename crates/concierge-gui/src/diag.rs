//! Lightweight file diagnostics. A GUI app on Windows has no console, so
//! `env_logger`'s stderr is lost and a failed terminal spawn is otherwise
//! invisible. We write logs NEXT TO THE EXECUTABLE (falling back to the temp
//! dir when that isn't writable) so they can be read after the fact — e.g.
//! synced back off the user's machine via a shared folder. Std-only.

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

/// Append a line to the diagnostics log (best-effort; never panics).
pub fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())
    {
        let _ = writeln!(f, "[{}] {msg}", stamp());
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
    log(&format!("logs: {}", log_file().display()));
}
