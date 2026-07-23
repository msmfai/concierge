//! Concierge download daemon — the **server** half of the client/server split.
//!
//! A single background process owns the process-global [`DownloadManager`] and
//! serves it over a local socket (Unix domain socket on unix, named pipe on
//! Windows — the platform is chosen by the transport crate). Clients (the GUI,
//! or the headless view) connect and drive downloads; the GUI can quit entirely
//! while this process keeps downloading.
//!
//! ## Protocol — the closed action vocabulary on the wire
//!
//! Control messages are the SAME closed action ids the two views already share
//! (`dl_pause_all`, `dl_pause:<id>`, …), validated here against
//! [`concierge_ui::is_action_id`]. There is deliberately **no parallel RPC
//! surface**: the only request types beyond an action id are the data-carrying
//! [`Request::Enqueue`] (a new download needs a url + dest, which no bare action
//! id can express) and the read-only [`Request::Snapshot`] / [`Request::Ping`].
//! So the daemon is the model in another process, driven by the same vocabulary.
//!
//! Framing is a little-endian `u32` length prefix followed by JSON — one request
//! and one response per connection; clients reconnect per call.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use concierge::download_manager::{DownloadManager, JobView};
use interprocess::local_socket::prelude::*;
#[cfg(not(windows))]
use interprocess::local_socket::ToFsName;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, Stream};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The system-tray icon + its host event loop (macOS/Windows only).
#[cfg(any(target_os = "macos", windows))]
pub mod tray;

/// This build's version, returned by [`Request::Ping`] so a client can tell a
/// stale daemon from a fresh one before handing work to it.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cap on a single framed message (guards against a corrupt/huge length prefix).
const MAX_MSG: usize = 8 * 1024 * 1024;

/// A request from a client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Liveness check — the reply carries the daemon's version.
    Ping,
    /// The current job table + global-pause flag (feeds `UiFacts.downloads`).
    Snapshot,
    /// Start a new download. Carries the data a bare action id cannot.
    Enqueue {
        /// Human-facing name shown in the queue.
        name: String,
        /// Source URL (already a resolved, ToS-safe one-click link).
        url: String,
        /// Destination path in the content-addressed cache.
        dest: PathBuf,
    },
    /// A closed-vocabulary action id (`dl_pause_all` / `dl_pause:<id>` / …).
    /// Validated with [`concierge_ui::is_action_id`]; unknown ids are refused.
    Action(String),
}

/// A reply from the daemon to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Reply to [`Request::Ping`].
    Pong {
        /// The daemon's `CARGO_PKG_VERSION`.
        version: String,
    },
    /// Reply to [`Request::Snapshot`].
    Snapshot(Snapshot),
    /// Reply to [`Request::Enqueue`] — the id of the newly-registered job, so
    /// the client can poll it in [`Snapshot`](Response::Snapshot) until terminal.
    Enqueued {
        /// The download manager's job id.
        id: u64,
    },
    /// The action was accepted.
    Ok,
    /// The request was rejected (e.g. an unknown action id).
    Err(String),
}

/// The download state a client polls: the job rows plus the global-pause flag.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Every job (active + finished this session), newest last.
    pub jobs: Vec<JobView>,
    /// Whether every active download is globally paused.
    pub paused_all: bool,
}

// --- transport / framing ----------------------------------------------------

/// Write a length-prefixed JSON frame.
fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message too large"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read a length-prefixed JSON frame.
fn read_msg<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = usize::try_from(u32::from_le_bytes(len_buf))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "length overflow"))?;
    if len > MAX_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(io::Error::other)
}

// --- socket name (platform-specialised) -------------------------------------

/// The filesystem path of the Unix domain socket (unix only).
#[cfg(not(windows))]
#[must_use]
pub fn socket_path() -> PathBuf {
    concierge_platform::config_dir().join("concierge-daemon.sock")
}

/// Build the platform socket name: a filesystem path (unix) or a namespaced
/// pipe name (Windows).
///
/// # Errors
/// Returns an error if the path/name is not a valid local-socket name.
#[cfg(not(windows))]
fn make_name() -> io::Result<Name<'static>> {
    socket_path().to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
fn make_name() -> io::Result<Name<'static>> {
    "concierge-daemon.sock".to_ns_name::<GenericNamespaced>()
}

// --- server -----------------------------------------------------------------

/// Handle one connection: read a request, apply it, write the response.
fn handle_conn<S: Read + Write>(stream: &mut S) -> io::Result<()> {
    let req: Request = read_msg(stream)?;
    let resp = dispatch(req);
    write_msg(stream, &resp)
}

/// Apply a request to the process-global download manager, producing a response.
fn dispatch(req: Request) -> Response {
    match req {
        Request::Ping => Response::Pong {
            version: VERSION.to_owned(),
        },
        Request::Snapshot => Response::Snapshot(current_snapshot()),
        Request::Enqueue { name, url, dest } => {
            // Register + fetch on the manager's own thread; return the job id so
            // the client can poll it. Bandwidth stays globally capped by the
            // manager's RateLimiter. (A bounded worker pool is a later refinement
            // — for now the GUI's fetch pool already bounds concurrency.)
            let id = concierge::download_manager::global().spawn(&name, url, dest);
            Response::Enqueued { id }
        }
        Request::Action(id) => {
            if !concierge_ui::is_action_id(&id) {
                return Response::Err(format!("unknown action id: {id}"));
            }
            apply_action(concierge::download_manager::global(), &id);
            Response::Ok
        }
    }
}

/// Map a download-control action id onto the manager. Non-download action ids
/// (they share one vocabulary) are simply no-ops for the daemon.
fn apply_action(mgr: &DownloadManager, id: &str) {
    match id {
        "dl_pause_all" => mgr.pause_all(),
        "dl_resume_all" => mgr.resume_all(),
        "dl_clear" => mgr.clear_finished(),
        _ => {
            if let Some(rest) = id.strip_prefix("dl_pause:") {
                if let Ok(n) = rest.parse::<u64>() {
                    mgr.pause(n);
                }
            } else if let Some(rest) = id.strip_prefix("dl_resume:") {
                if let Ok(n) = rest.parse::<u64>() {
                    mgr.resume(n);
                }
            } else if let Some(rest) = id.strip_prefix("dl_cancel:") {
                if let Ok(n) = rest.parse::<u64>() {
                    mgr.cancel(n);
                }
            }
        }
    }
}

/// Snapshot the process-global manager.
fn current_snapshot() -> Snapshot {
    let mgr = concierge::download_manager::global();
    Snapshot {
        jobs: mgr.snapshot(),
        paused_all: mgr.is_paused_globally(),
    }
}

/// Serve on an already-built name until the listener errors terminally.
fn serve_on(name: Name<'_>) -> io::Result<()> {
    let listener = ListenerOptions::new().name(name).create_sync()?;
    concierge_platform::diag("daemon: listening");
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                if let Err(e) = handle_conn(&mut stream) {
                    concierge_platform::diag(&format!("daemon: connection error: {e}"));
                }
            }
            Err(e) => concierge_platform::diag(&format!("daemon: accept error: {e}")),
        }
    }
    Ok(())
}

/// Run the daemon: bind the platform socket and serve download requests forever.
///
/// # Errors
/// Returns an error if the socket cannot be bound (e.g. another daemon owns it).
pub fn serve() -> io::Result<()> {
    // A leftover socket FILE from a crashed daemon blocks bind on unix; the
    // client only spawns us after failing to reach a live daemon, so removing a
    // stale file here is safe. (Windows named pipes have no such artifact.)
    #[cfg(not(windows))]
    {
        let path = socket_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&path);
    }
    // File-based liveness marker: the daemon now owns the heartbeat the nxm
    // handoff checks (moved off the GUI), so a one-click always finds the
    // always-on service.
    concierge_platform::start_heartbeat(DAEMON_HEARTBEAT);
    serve_on(make_name()?)
}

// --- spawn-or-connect (client bootstrap) ------------------------------------

/// Locate a sibling binary (`stem`, `stem.exe` on Windows) next to the current
/// executable — the daemon, the GUI, and the browser-launched nxm handler all
/// live in the same directory (the `.app`'s `MacOS/`, or beside the exe).
#[must_use]
pub fn sibling_exe(stem: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    exe_in(&dir, stem)
}

/// `stem` (`stem.exe` on Windows) inside `dir`, if present.
fn exe_in(dir: &Path, stem: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    };
    let path = dir.join(name);
    path.exists().then_some(path)
}

/// The daemon binary next to the current executable (for spawn / handler
/// registration).
#[must_use]
pub fn daemon_exe() -> Option<PathBuf> {
    sibling_exe("concierge-daemon")
}

/// Append a timestamped line to the daemon's own log file. A plain `fn(&str)` (no
/// captures) so it can back [`concierge_platform::set_diag_logger`], giving the
/// background service the same firehose the GUI has — so an nxm handoff or a
/// download decision is reconstructable from disk.
fn log_line(msg: &str) {
    use std::io::Write as _;
    let path = concierge_platform::config_dir().join("concierge-daemon.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// Wire the daemon's diagnostics to its log file. Call once at startup.
pub fn install_logger() {
    concierge_platform::set_diag_logger(log_line);
    concierge_platform::diag(&format!(
        "daemon: v{VERSION} starting (pid {})",
        std::process::id()
    ));
}

/// The name a GUI process refreshes so the daemon can tell one is running.
const GUI_HEARTBEAT: &str = "concierge-gui";
/// The name the daemon service refreshes (its file-based liveness marker).
const DAEMON_HEARTBEAT: &str = "concierge-daemon";

/// Handle an `nxm://` launch (the daemon is the registered handler): queue the
/// url(s) in the inbox, ensure the download **service** and a **GUI** (which
/// holds the active modpack, so it does the actual pin/download) are running,
/// then return. At most one GUI is spawned, and only if none is live — so a
/// one-click lands in the daemon and its running GUI, never a second window.
pub fn handoff_nxm(urls: &[String]) {
    for url in urls {
        let _ = concierge::nexus::append_nxm_inbox(url);
        concierge_platform::diag(&format!("daemon(nxm): queued {url}"));
    }
    // Ensure the always-on download service is up (a detached daemon, no args).
    if !Client.is_alive() {
        if let Some(exe) = daemon_exe() {
            let _ = std::process::Command::new(exe).spawn();
            concierge_platform::diag("daemon(nxm): started the background service");
        }
    }
    // Ensure a GUI is live to drain + process the inbox (it holds the active
    // modpack). Spawns one ONLY if none is live — the no-second-window guard.
    ensure_gui();
}

/// Ensure a GUI window is running — spawn one next to us only if none is live
/// (the `concierge-gui` heartbeat is fresh). Used by the tray's "Open Concierge"
/// and the nxm handoff. Focusing an already-open window cross-process isn't
/// attempted; the common case is "none open → launch one".
pub fn ensure_gui() {
    if concierge_platform::heartbeat_age(GUI_HEARTBEAT).is_some_and(|age| age < 6) {
        concierge_platform::diag("daemon: a GUI is already live");
    } else if let Some(exe) = gui_exe() {
        let _ = std::process::Command::new(exe).spawn();
        concierge_platform::diag("daemon: launched a GUI");
    }
}

/// The GUI binary next to the current executable. In the current layout the
/// GUI ships as `concierge` (with `concierge-cli` beside it — that sibling is
/// the layout discriminator), except inside the macOS .app where it keeps the
/// `concierge-gui` name (the launcher script owns `Concierge`, which on a
/// case-insensitive filesystem is the same name). In the pre-rename layout
/// `concierge` was the CLI and the GUI was `concierge-gui`.
fn gui_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    gui_exe_in(&dir)
}

/// [`gui_exe`] on an explicit directory (unit-testable).
fn gui_exe_in(dir: &Path) -> Option<PathBuf> {
    if exe_in(dir, "concierge-cli").is_some() {
        exe_in(dir, "concierge").or_else(|| exe_in(dir, "concierge-gui"))
    } else {
        exe_in(dir, "concierge-gui")
    }
}

/// Where the daemon leaves a "quit" marker for the GUI. Tray → Quit stops the
/// daemon AND asks any open GUI to exit (the client can't be pushed to over the
/// per-call socket, so a file is the simplest cross-process signal).
fn quit_marker_path() -> PathBuf {
    concierge_platform::config_dir().join("concierge.quit")
}

/// Ask any running GUI to quit (called by the tray's Quit before the daemon
/// exits). Only drops the marker when a GUI is actually live, so a stale marker
/// can't linger and kill the *next* GUI that starts.
pub fn request_gui_quit() {
    if concierge_platform::heartbeat_age(GUI_HEARTBEAT).is_some_and(|age| age < 6) {
        let _ = std::fs::write(quit_marker_path(), "1");
    }
}

/// Clear any leftover quit marker — the GUI calls this once at startup so a
/// marker from a previous session can't immediately close a fresh window.
pub fn clear_quit_request() {
    let _ = std::fs::remove_file(quit_marker_path());
}

/// The GUI calls this each frame: if a quit was requested, consume the marker and
/// return `true` (the GUI then closes its window).
#[must_use]
pub fn take_quit_request() -> bool {
    let path = quit_marker_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
        true
    } else {
        false
    }
}

/// Where a second bare `concierge` launch leaves a "show yourself" marker for
/// the live GUI (Vortex-style single instance: relaunching raises the existing
/// window instead of opening a second one).
fn show_marker_path() -> PathBuf {
    concierge_platform::config_dir().join("concierge.show")
}

/// Ask the live GUI to un-hide/restore/focus its window. Only drops the marker
/// when a GUI is actually live, so a stale marker can't pop the *next* GUI.
pub fn request_gui_show() {
    if concierge_platform::heartbeat_age(GUI_HEARTBEAT).is_some_and(|age| age < 6) {
        let _ = std::fs::write(show_marker_path(), "1");
    }
}

/// The GUI polls this each frame (and clears it once at startup): consume a
/// pending show request, if any.
#[must_use]
pub fn take_show_request() -> bool {
    let path = show_marker_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
        true
    } else {
        false
    }
}

/// Remove the download-service liveness marker. The GUI — which hosts the
/// service in-process — calls this as it exits, so nothing briefly mistakes
/// the gone process for a live service.
pub fn clear_service_liveness() {
    let _ = std::fs::remove_file(concierge_platform::heartbeat_path(DAEMON_HEARTBEAT));
}

/// Run the daemon **service**: the socket server + heartbeat, plus — on
/// macOS/Windows — the system-tray icon, whose OS event loop owns the main
/// thread. Blocks until the tray's Quit (or, on Linux, a fatal listener error).
///
/// # Errors
/// Propagates a fatal listener error on platforms without a tray.
pub fn run_service() -> io::Result<()> {
    #[cfg(any(target_os = "macos", windows))]
    {
        // The socket server + heartbeat run on a background thread so the tray can
        // own the main thread (its event loop must run there).
        std::thread::spawn(|| {
            if let Err(e) = serve() {
                concierge_platform::diag(&format!("daemon: serve error: {e}"));
            }
        });
        tray::run();
        // Tray Quit fell through: clear our own liveness so nothing thinks the
        // service is still up, then let the process exit.
        let _ = std::fs::remove_file(concierge_platform::heartbeat_path(DAEMON_HEARTBEAT));
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        serve()
    }
}

/// Connect to a running daemon, or spawn one and wait for it to answer.
///
/// Returns a live [`Client`], or `None` if no daemon could be reached or started
/// — in which case the caller runs downloads in-process (the safe fallback), so
/// a missing/broken daemon never breaks downloading.
#[must_use]
pub fn spawn_or_connect() -> Option<Client> {
    let client = Client;
    if client.is_alive() {
        concierge_platform::diag("daemon: connected to a running instance");
        return Some(client);
    }
    let exe = daemon_exe()?;
    if let Err(e) = std::process::Command::new(&exe).spawn() {
        concierge_platform::diag(&format!("daemon: spawn failed: {e}"));
        return None;
    }
    concierge_platform::diag(&format!("daemon: spawned {}", exe.display()));
    for _ in 0..50 {
        if client.is_alive() {
            concierge_platform::diag("daemon: up and answering");
            return Some(client);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    concierge_platform::diag("daemon: spawned but never answered — using in-process fallback");
    None
}

// --- client -----------------------------------------------------------------

/// A thin client to a running daemon. Stateless: each call opens a fresh
/// connection (one request / one response), so a dropped daemon just surfaces as
/// a connection error on the next call.
#[derive(Debug, Clone, Copy, Default)]
pub struct Client;

impl Client {
    /// Send one request and read the response.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or the exchange fails.
    pub fn call(self, req: &Request) -> io::Result<Response> {
        let mut stream = Stream::connect(make_name()?)?;
        write_msg(&mut stream, req)?;
        read_msg(&mut stream)
    }

    /// Whether a daemon is answering (a successful [`Request::Ping`]).
    #[must_use]
    pub fn is_alive(self) -> bool {
        matches!(self.call(&Request::Ping), Ok(Response::Pong { .. }))
    }

    /// The daemon's version, or an error if it is unreachable.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or replies unexpectedly.
    pub fn version(self) -> io::Result<String> {
        match self.call(&Request::Ping)? {
            Response::Pong { version } => Ok(version),
            other => Err(unexpected(&other)),
        }
    }

    /// Poll the current download state.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or replies unexpectedly.
    pub fn snapshot(self) -> io::Result<Snapshot> {
        match self.call(&Request::Snapshot)? {
            Response::Snapshot(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    /// Enqueue a new download; returns the daemon's job id so the caller can poll
    /// it in [`Client::snapshot`] until it reaches a terminal state.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or rejects the request.
    pub fn enqueue(self, name: &str, url: &str, dest: PathBuf) -> io::Result<u64> {
        match self.call(&Request::Enqueue {
            name: name.to_owned(),
            url: url.to_owned(),
            dest,
        })? {
            Response::Enqueued { id } => Ok(id),
            Response::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidInput, e)),
            other => Err(unexpected(&other)),
        }
    }

    /// Send a closed-vocabulary action id (pause/resume/cancel/clear).
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or rejects the id.
    pub fn action(self, id: &str) -> io::Result<()> {
        self.expect_ok(&Request::Action(id.to_owned()))
    }

    /// Send a request expecting a bare `Ok`.
    fn expect_ok(self, req: &Request) -> io::Result<()> {
        match self.call(req)? {
            Response::Ok => Ok(()),
            Response::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidInput, e)),
            other => Err(unexpected(&other)),
        }
    }
}

/// An unexpected response variant, as an IO error.
fn unexpected(resp: &Response) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected daemon response: {resp:?}"),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// `gui_exe_in` must pick the right binary in every install layout the
    /// transition can produce: current flat (concierge = GUI, concierge-cli
    /// beside it), macOS .app internals (concierge-cli + concierge-gui, no
    /// `concierge` — the launcher script owns that name case-insensitively),
    /// and pre-rename (concierge-gui = GUI, `concierge` = the old CLI).
    #[test]
    fn gui_lookup_matches_every_install_layout() {
        let exe = |stem: &str| {
            if cfg!(windows) {
                format!("{stem}.exe")
            } else {
                stem.to_owned()
            }
        };
        let fresh = |tag: &str, names: &[&str]| {
            let dir = std::env::temp_dir().join(format!("cg-guiexe-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for n in names {
                std::fs::write(dir.join(exe(n)), b"x").unwrap();
            }
            dir
        };

        let flat = fresh("flat", &["concierge", "concierge-cli", "concierge-daemon"]);
        assert_eq!(gui_exe_in(&flat).unwrap(), flat.join(exe("concierge")));

        let app = fresh(
            "app",
            &["concierge-gui", "concierge-cli", "concierge-daemon"],
        );
        assert_eq!(gui_exe_in(&app).unwrap(), app.join(exe("concierge-gui")));

        let legacy = fresh(
            "legacy",
            &["concierge-gui", "concierge", "concierge-daemon"],
        );
        assert_eq!(
            gui_exe_in(&legacy).unwrap(),
            legacy.join(exe("concierge-gui"))
        );

        let empty = fresh("empty", &[]);
        assert!(gui_exe_in(&empty).is_none());

        for d in [flat, app, legacy, empty] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn frame_round_trips() {
        let msg = Request::Enqueue {
            name: "Test Mod".to_owned(),
            url: "https://example.invalid/f.zip".to_owned(),
            dest: PathBuf::from("/tmp/f.zip"),
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &msg).unwrap();
        let mut cur = Cursor::new(buf);
        let back: Request = read_msg(&mut cur).unwrap();
        match back {
            Request::Enqueue { name, url, .. } => {
                assert_eq!(name, "Test Mod");
                assert_eq!(url, "https://example.invalid/f.zip");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn ping_and_snapshot_dispatch() {
        match dispatch(Request::Ping) {
            Response::Pong { version } => assert_eq!(version, VERSION),
            other => panic!("ping: {other:?}"),
        }
        match dispatch(Request::Snapshot) {
            Response::Snapshot(_) => {}
            other => panic!("snapshot: {other:?}"),
        }
    }

    #[test]
    fn unknown_action_is_refused_known_action_ok() {
        match dispatch(Request::Action("not_a_real_action".to_owned())) {
            Response::Err(e) => assert!(e.contains("unknown action id")),
            other => panic!("expected refusal: {other:?}"),
        }
        // A real download-control id from the shared vocabulary is accepted.
        // Use `dl_clear` (a harmless no-op with no jobs) rather than a
        // global-pause id, so this test doesn't race the observability test
        // below on the shared process-global manager.
        assert!(concierge_ui::is_action_id("dl_clear"));
        match dispatch(Request::Action("dl_clear".to_owned())) {
            Response::Ok => {}
            other => panic!("expected ok: {other:?}"),
        }
    }

    #[test]
    fn global_pause_action_is_observable_in_snapshot() {
        // Drive the manager purely through the action vocabulary + snapshot,
        // proving the daemon owns and exposes the global-pause control.
        dispatch(Request::Action("dl_pause_all".to_owned()));
        let paused = match dispatch(Request::Snapshot) {
            Response::Snapshot(s) => s.paused_all,
            other => panic!("snapshot: {other:?}"),
        };
        assert!(paused, "pause_all should be reflected in the snapshot");
        dispatch(Request::Action("dl_resume_all".to_owned()));
        let resumed = match dispatch(Request::Snapshot) {
            Response::Snapshot(s) => s.paused_all,
            other => panic!("snapshot: {other:?}"),
        };
        assert!(!resumed, "resume_all should clear the global pause");
    }

    /// End-to-end over a REAL local socket, proving the transport (not just the
    /// framing) works. Bound to a UNIQUE, test-only socket path so it never
    /// collides with a live daemon (which owns the production socket) — that
    /// isolation keeps it deterministic in CI and alongside a running app. Unix
    /// only: Windows relies on the framing + dispatch unit tests, since its
    /// named-pipe transport can't take an arbitrary filesystem path here.
    #[cfg(unix)]
    #[test]
    fn transport_round_trips_over_a_socket() {
        let path = concierge_platform::config_dir()
            .join(format!("test-daemon-{}.sock", std::process::id()));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _ = std::fs::remove_file(&path);
        let server_path = path.clone();
        std::thread::spawn(move || {
            if let Ok(name) = server_path.as_path().to_fs_name::<GenericFilePath>() {
                let _ = serve_on(name);
            }
        });
        // A direct connect + framed exchange (the Client wraps exactly this, but
        // against the production socket, which a running daemon may own).
        let exchange = |req: &Request| -> io::Result<Response> {
            let name = path.as_path().to_fs_name::<GenericFilePath>()?;
            let mut stream = Stream::connect(name)?;
            write_msg(&mut stream, req)?;
            read_msg(&mut stream)
        };
        // Retry: the listener may not be bound the instant the thread starts.
        let mut pong = None;
        for _ in 0..50 {
            if let Ok(Response::Pong { version }) = exchange(&Request::Ping) {
                pong = Some(version);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(pong.as_deref(), Some(VERSION), "daemon should answer ping");
        // Snapshot round-trips a real reply; enqueue returns a real job id.
        assert!(matches!(
            exchange(&Request::Snapshot),
            Ok(Response::Snapshot(_))
        ));
        let enq = exchange(&Request::Enqueue {
            name: "Bogus".to_owned(),
            url: "https://example.invalid/x.zip".to_owned(),
            dest: PathBuf::from("/dev/null"),
        });
        assert!(
            matches!(enq, Ok(Response::Enqueued { id }) if id > 0),
            "enqueue should return a real job id, got {enq:?}"
        );
        let _ = std::fs::remove_file(&path);
    }
}
