//! Platform tray / menu-bar icon (macOS status item, Windows notification area).
//!
//! The icon is the app's persistent presence while the window is hidden to the
//! tray. Its menu items don't call anything directly — a click is forwarded, as
//! a view-model action id (`show_window` / `quit`), into the App's single
//! dispatch path, so the tray drives the SAME closed vocabulary both views share
//! (and can't drift from the GUI's own controls or the agent view).
//!
//! Only built on macOS/Windows; elsewhere `Tray` is a no-op stub so the rest of
//! the GUI compiles and runs unchanged.

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use std::sync::mpsc::Sender;

    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use crate::diag;

    /// A live tray icon. Dropping it removes the icon, so the App holds it.
    pub struct Tray {
        _icon: TrayIcon,
        /// The disabled "Downloads: …" line, refreshed from the queue snapshot.
        status: MenuItem,
    }

    impl Tray {
        /// Build the tray and start forwarding its menu clicks into `actions`,
        /// waking the UI via `ctx`. Returns `None` if the platform refused the
        /// icon (the app then simply runs without a tray).
        pub fn new(actions: Sender<String>, ctx: eframe::egui::Context) -> Option<Self> {
            let menu = Menu::new();
            let open = MenuItem::new("Open Concierge", true, None);
            let status = MenuItem::new("Downloads: idle", false, None);
            let quit = MenuItem::new("Quit Concierge", true, None);
            menu.append(&open).ok()?;
            menu.append(&PredefinedMenuItem::separator()).ok()?;
            menu.append(&status).ok()?;
            menu.append(&PredefinedMenuItem::separator()).ok()?;
            menu.append(&quit).ok()?;

            let mut builder = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Concierge");
            if let Some(icon) = make_icon() {
                builder = builder.with_icon(icon);
            }
            let icon = builder.build().ok()?;

            let open_id = open.id().clone();
            let quit_id = quit.id().clone();
            // Menu events arrive on a global channel. Forward them off the UI
            // thread so the tray stays responsive even while the window is hidden;
            // request_repaint wakes the (idle) event loop to run the dispatch.
            std::thread::spawn(move || {
                let rx = MenuEvent::receiver();
                while let Ok(ev) = rx.recv() {
                    let id = if ev.id == open_id {
                        "show_window"
                    } else if ev.id == quit_id {
                        "quit"
                    } else {
                        continue;
                    };
                    if actions.send(id.to_owned()).is_err() {
                        break; // App is gone
                    }
                    ctx.request_repaint();
                }
            });
            diag::log("tray: menu-bar icon installed");
            Some(Self {
                _icon: icon,
                status,
            })
        }

        /// Update the "Downloads: …" line (called on the UI thread).
        pub fn set_status(&self, text: &str) {
            self.status.set_text(text);
        }
    }

    /// A simple 32×32 filled-circle icon in the brand accent — recognisable in the
    /// tray without shipping an image asset. (No `as` casts / indexing: strict
    /// lints. Coords stay in `u16` so `f32::from` is available.)
    fn make_icon() -> Option<Icon> {
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
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::sync::mpsc::Sender;

    /// No-op tray on platforms without a supported tray backend. The methods
    /// mirror the real impl's signatures (so callers are platform-agnostic), so
    /// the unused `self`/args and const-ness are expected here.
    pub struct Tray;

    #[allow(
        clippy::unused_self,
        clippy::missing_const_for_fn,
        clippy::needless_pass_by_value
    )]
    impl Tray {
        pub fn new(_actions: Sender<String>, _ctx: eframe::egui::Context) -> Option<Self> {
            None
        }
        pub fn set_status(&self, _text: &str) {}
    }
}

pub use imp::Tray;
