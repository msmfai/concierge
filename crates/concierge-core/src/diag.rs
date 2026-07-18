//! Structured session diagnostics. When `$CONCIERGE_LOG_DIR` is set (the GUI
//! sets it per "open shell"), every component — this library, the CLI, and the
//! Windows sandbox bootstrap — appends tagged one-line events to
//! `<dir>/trace.log`. A failure anywhere in the chain is then greppable by
//! `comp`/`tag` after the fact, even off a machine we can't see (synced back).
//!
//! Format per line: `<unix_secs> <comp> <tag> <msg>` — deliberately flat so
//! `grep bootstrap`, `grep error`, etc. Just Work; no parser needed.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The active session log directory, if the environment names one.
#[must_use]
pub fn session_dir() -> Option<PathBuf> {
    std::env::var_os("CONCIERGE_LOG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Append one tagged event to the session trace (best-effort; a no-op when no
/// session dir is set). `comp` = component (cli/gui/bootstrap/sandbox), `tag` =
/// short category, `msg` = free text (newlines flattened so it stays one line).
pub fn event(comp: &str, tag: &str, msg: &str) {
    let Some(dir) = session_dir() else {
        return;
    };
    let flat: String = msg
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("trace.log"))
    {
        let _ = writeln!(f, "{} {comp:<9} {tag:<12} {flat}", secs());
    }
}

/// Write `content` to `<session_dir>/<name>` (best-effort) — for dropping a
/// verbatim artifact next to the trace, e.g. the exact sandbox script used.
pub fn artifact(name: &str, content: &str) {
    if let Some(dir) = session_dir() {
        let _ = std::fs::write(dir.join(name), content);
    }
}
