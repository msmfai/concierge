//! Concierge download daemon binary — a windowless background process that owns
//! the download manager and serves it over a local socket. Launched on demand by
//! the GUI (spawn-or-connect); keeps running after the GUI quits.

// No console/window on Windows: this is a background service, not a terminal
// app. Keep the console in debug so `cargo run -p concierge-daemon` shows stderr.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::io::Result<()> {
    // Load persisted settings into the process-global so the manager reads the
    // user's bandwidth cap (the GUI writes the same file).
    concierge::settings::load();
    concierge_daemon::serve()
}
