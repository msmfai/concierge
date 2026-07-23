//! The daemon's system-tray / menu-bar icon and the OS event loop that hosts it.
//!
//! LEGACY PATH: the GUI now owns the tray in-process (the Vortex model — one
//! process holds window, tray, queue, and socket; see concierge-gui's tray
//! module, which reuses [`make_icon`]). This tray only appears when the daemon
//! binary runs standalone — e.g. an old nxm:// registration started it before
//! the GUI was up. Hosting a status item (macOS) / notification-area icon
//! (Windows) needs an OS message pump on the main thread, which winit provides
//! safely; the socket server runs on a background thread (see `run_service`).
//!
//! Menu: **Open Concierge** (launch a GUI if none is open), a disabled
//! **Downloads: N** status line refreshed from the daemon's own queue, and
//! **Quit** (ask any GUI to exit, then stop the daemon → the whole app stops).

use std::time::{Duration, Instant};

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use concierge::download_manager::JobState;

/// Poll cadence for menu clicks + the download-status refresh.
const TICK: Duration = Duration::from_millis(500);

struct TrayApp {
    tray: Option<TrayIcon>,
    status: Option<MenuItem>,
    open_id: Option<MenuId>,
    quit_id: Option<MenuId>,
    last_status: String,
}

impl TrayApp {
    const fn new() -> Self {
        Self {
            tray: None,
            status: None,
            open_id: None,
            quit_id: None,
            last_status: String::new(),
        }
    }

    /// Build the tray icon + menu (once the event loop is active).
    fn build(&mut self) {
        let menu = Menu::new();
        let open = MenuItem::new("Open Concierge", true, None);
        let status = MenuItem::new("Downloads: idle", false, None);
        let quit = MenuItem::new("Quit Concierge", true, None);
        let ok = menu.append(&open).is_ok()
            && menu.append(&PredefinedMenuItem::separator()).is_ok()
            && menu.append(&status).is_ok()
            && menu.append(&PredefinedMenuItem::separator()).is_ok()
            && menu.append(&quit).is_ok();
        if !ok {
            concierge_platform::diag("daemon: tray menu assembly failed");
            return;
        }
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Concierge");
        if let Some(icon) = make_icon() {
            builder = builder.with_icon(icon);
        }
        match builder.build() {
            Ok(tray) => {
                self.open_id = Some(open.id().clone());
                self.quit_id = Some(quit.id().clone());
                self.status = Some(status);
                self.tray = Some(tray);
                concierge_platform::diag("daemon: tray icon installed");
            }
            Err(e) => concierge_platform::diag(&format!("daemon: tray build failed: {e}")),
        }
    }

    /// Refresh the "Downloads: N" line from the daemon's own queue snapshot.
    fn refresh_status(&mut self) {
        let Some(status) = &self.status else { return };
        let snap = crate::current_snapshot();
        let active = snap
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Downloading | JobState::Paused))
            .count();
        let text = if active == 0 {
            "Downloads: idle".to_owned()
        } else {
            format!("Downloads: {active} active")
        };
        if text != self.last_status {
            status.set_text(&text);
            self.last_status = text;
        }
    }

    /// Drain tray-menu clicks: Open launches a GUI, Quit stops everything.
    fn pump_menu(&self, event_loop: &ActiveEventLoop) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if self.open_id.as_ref() == Some(&ev.id) {
                concierge_platform::diag("daemon(tray): Open Concierge");
                crate::ensure_gui();
            } else if self.quit_id.as_ref() == Some(&ev.id) {
                concierge_platform::diag("daemon(tray): Quit — stopping everything");
                crate::request_gui_quit();
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler for TrayApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            self.build();
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pump_menu(event_loop);
        self.refresh_status();
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + TICK));
    }
}

/// Run the tray's event loop on the current (main) thread until Quit.
pub fn run() {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    {
        // Menu-bar agent: no Dock icon, no app-switcher entry.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS as _};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    match builder.build() {
        Ok(event_loop) => {
            let mut app = TrayApp::new();
            if let Err(e) = event_loop.run_app(&mut app) {
                concierge_platform::diag(&format!("daemon: tray loop ended: {e}"));
            }
        }
        Err(e) => concierge_platform::diag(&format!("daemon: tray event loop init failed: {e}")),
    }
}

/// A simple 32×32 filled-circle icon in the brand accent (no image asset; strict
/// lints ⇒ no `as`/indexing, `u16` coords so `f32::from` applies).
/// The Concierge tray dot — shared with the GUI's in-process tray.
#[must_use]
pub fn make_icon() -> Option<Icon> {
    const S: u16 = 32;
    let (cx, cy, r) = (15.5_f32, 15.5_f32, 14.0_f32);
    let mut rgba: Vec<u8> = Vec::with_capacity(4096);
    for y in 0..S {
        for x in 0..S {
            let dx = f32::from(x) - cx;
            let dy = f32::from(y) - cy;
            if dx.mul_add(dx, dy * dy) <= r * r {
                rgba.extend_from_slice(&[0x4d, 0x9e, 0xff, 0xff]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Icon::from_rgba(rgba, u32::from(S), u32::from(S)).ok()
}
