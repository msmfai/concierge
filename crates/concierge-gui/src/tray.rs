//! The in-app system-tray icon (Vortex model): ONE process owns the window,
//! the tray, and the download queue — quitting it stops everything. Clicking
//! the icon toggles the window; the menu offers Open, a live "Downloads: N"
//! status line, and Quit. The daemon's tray did this job when the daemon was
//! the always-on process; here eframe's winit loop is the OS message pump, so
//! the icon is built lazily on the first frame (the loop is live by then) and
//! its event channels are drained every frame.
//!
//! macOS/Windows only, like the daemon's legacy tray (the same OS constraint:
//! Linux notification areas need a desktop-specific stack we don't carry). On
//! other platforms [`Tray`] is an inert stub, so callers stay cfg-free.

#[cfg(any(target_os = "macos", windows))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", windows))]
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
#[cfg(any(target_os = "macos", windows))]
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[cfg(any(target_os = "macos", windows))]
use crate::diag;

/// Status refresh cadence — a socket snapshot per second, not per frame.
#[cfg(any(target_os = "macos", windows))]
const STATUS_TICK: Duration = Duration::from_secs(1);

/// Inert stand-in where no tray is supported: never yields an action.
#[cfg(not(any(target_os = "macos", windows)))]
#[derive(Default)]
pub struct Tray;

#[cfg(not(any(target_os = "macos", windows)))]
impl Tray {
    pub fn ensure_built(&mut self) {}

    #[must_use]
    pub fn poll(&mut self) -> Vec<Action> {
        Vec::new()
    }
}

/// What a drained tray interaction asks the app to do.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub enum Action {
    /// Menu "Open Concierge": un-hide/restore/focus the window.
    Show,
    /// Plain left-click on the icon: hide the window if visible, show if not.
    Toggle,
    /// Menu "Quit Concierge": close the window — the whole app exits with it.
    Quit,
}

#[cfg(any(target_os = "macos", windows))]
#[derive(Default)]
pub struct Tray {
    icon: Option<TrayIcon>,
    status: Option<MenuItem>,
    open_id: Option<MenuId>,
    quit_id: Option<MenuId>,
    last_status: String,
    last_refresh: Option<Instant>,
    attempted: bool,
}

#[cfg(any(target_os = "macos", windows))]
impl Tray {
    /// Build the icon + menu, once, on the first frame. A failure (e.g. no
    /// tray support under some Wine setups) is logged and final — the app
    /// still works, the window close button just quits it like any app.
    pub fn ensure_built(&mut self) {
        if self.attempted {
            return;
        }
        self.attempted = true;
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
            diag::log("tray: menu assembly failed");
            return;
        }
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Concierge");
        if let Some(icon) = concierge_daemon::tray::make_icon() {
            builder = builder.with_icon(icon);
        }
        match builder.build() {
            Ok(tray) => {
                self.open_id = Some(open.id().clone());
                self.quit_id = Some(quit.id().clone());
                self.status = Some(status);
                self.icon = Some(tray);
                diag::log("tray: icon installed");
            }
            Err(e) => diag::log(&format!("tray: build failed: {e}")),
        }
    }

    /// Drain pending tray interactions and refresh the status line.
    pub fn poll(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.icon.is_none() {
            return out;
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if self.open_id.as_ref() == Some(&ev.id) {
                out.push(Action::Show);
            } else if self.quit_id.as_ref() == Some(&ev.id) {
                out.push(Action::Quit);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            // Vortex behavior: a plain click on the icon toggles the window.
            // (On macOS a menu-bearing status item opens its menu instead —
            // the click never reaches us there; the menu's Open covers it.)
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                out.push(Action::Toggle);
            }
        }
        if self.last_refresh.is_none_or(|t| t.elapsed() >= STATUS_TICK) {
            self.last_refresh = Some(Instant::now());
            self.refresh_status();
        }
        out
    }

    /// Refresh "Downloads: N" from whoever serves the queue over the socket —
    /// this process normally; an old standalone daemon on the compat path.
    fn refresh_status(&mut self) {
        let Some(status) = &self.status else { return };
        let active = concierge_daemon::Client.snapshot().map_or(0, |s| {
            s.jobs
                .iter()
                .filter(|j| {
                    matches!(
                        j.state,
                        concierge::download_manager::JobState::Downloading
                            | concierge::download_manager::JobState::Paused
                    )
                })
                .count()
        });
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
}
