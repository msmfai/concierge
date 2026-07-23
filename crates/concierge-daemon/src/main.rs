//! Concierge download daemon binary — LEGACY/COMPAT. The GUI now hosts the
//! download service and tray in-process (the Vortex model: one process owns
//! window, tray, queue, and socket; quitting it stops everything). This binary
//! remains for old nxm:// registrations that still point at it (its handoff
//! queues the url and launches/raises the app) and as a headless service if
//! run by hand.

// No console/window on Windows: this is a background service, not a terminal
// app. Keep the console in debug so `cargo run -p concierge-daemon` shows stderr.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::io::Result<()> {
    // Firehose: capture the daemon's own diagnostics (handoff, downloads) to a
    // log file, the same discipline the GUI follows.
    concierge_daemon::install_logger();
    // Load persisted settings into the process-global so the manager reads the
    // user's bandwidth cap (the GUI writes the same file).
    concierge::settings::load();
    // The browser registers THIS binary as the nxm:// handler (`concierge-daemon
    // nxm "%1"`). In that mode we're a short-lived handoff: queue the url, make
    // sure the service + a GUI are up, and exit — never a second window.
    let nxm: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a.starts_with("nxm://"))
        .collect();
    if !nxm.is_empty() {
        concierge_daemon::handoff_nxm(&nxm);
        return Ok(());
    }
    // Otherwise run as the long-lived download service — with the tray icon on
    // the main thread (macOS/Windows) and the socket server behind it.
    concierge_daemon::run_service()
}
