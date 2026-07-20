//! In-app auto-updater UI + orchestration (roadmap 0.2). Checks GitHub Releases
//! for a newer Concierge, and on request downloads the verified platform asset
//! and swaps it in — retiring the hand-delivery (Bear Share) loop.
//!
//! All network/IO runs off the UI thread; the render reads a shared `Status`.

use std::sync::{Arc, Mutex};

use concierge_update::{Channel, Decision};

use crate::diag;

const REPO: &str = "msmfai/concierge";

/// What the updater is doing right now (shared with the render).
#[derive(Debug, Clone)]
enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(String), // release tag
    Working(String),   // human message (downloading / installing)
    Failed(String),
}

pub struct Updates {
    status: Arc<Mutex<Status>>,
    channel: Channel,
    /// Whether a check has been kicked off this session (auto on first frame).
    started: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(Status::Idle)),
            channel: Channel::Stable,
            started: false,
        }
    }
}

fn set(status: &Arc<Mutex<Status>>, s: Status) {
    if let Ok(mut g) = status.lock() {
        *g = s;
    }
}

fn read(status: &Arc<Mutex<Status>>) -> Status {
    status.lock().map_or(Status::Idle, |g| g.clone())
}

impl Updates {
    /// Kick off a background check (idempotent per intent — safe to call again).
    pub fn check(&self) {
        let status = Arc::clone(&self.status);
        let channel = self.channel;
        std::thread::spawn(move || {
            set(&status, Status::Checking);
            let current = concierge_update::parse_current(env!("CARGO_PKG_VERSION"));
            match concierge_update::fetch_releases(REPO) {
                Ok(releases) => match concierge_update::select_update(&current, channel, &releases)
                {
                    Decision::UpToDate => set(&status, Status::UpToDate),
                    Decision::Update { tag, .. } => {
                        diag::log(&format!(
                            "update: {tag} available (on v{})",
                            env!("CARGO_PKG_VERSION")
                        ));
                        set(&status, Status::Available(tag));
                    }
                },
                Err(e) => {
                    diag::log(&format!("update: check failed: {e}"));
                    set(&status, Status::Failed(format!("check failed: {e}")));
                }
            }
        });
    }

    /// Download the verified asset for `tag`, swap it in, and relaunch. On success
    /// this process exits from inside the worker; on failure the status reflects it.
    fn install(&self, tag: String) {
        let status = Arc::clone(&self.status);
        std::thread::spawn(move || {
            set(&status, Status::Working(format!("downloading {tag}…")));
            match do_install(&tag) {
                Ok(()) => {} // unreachable: process exits inside on success
                Err(e) => {
                    diag::log(&format!("update: install failed: {e}"));
                    set(&status, Status::Failed(format!("install failed: {e}")));
                }
            }
        });
    }

    /// Sweep any binary swapped-aside by a previous update. Call once at startup.
    pub fn startup_cleanup() {
        if let Some(dir) = install_dir() {
            concierge_update::cleanup_old(&dir);
        }
    }

    /// Render the Updates section of Settings.
    pub fn render(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        // Auto-check once, the first time Settings is drawn this session.
        if !self.started {
            self.started = true;
            self.check();
        }
        ui.strong("Updates");
        ui.label(format!("Current version: v{}", env!("CARGO_PKG_VERSION")));
        ui.horizontal(|ui| {
            ui.label("Channel:");
            let mut changed = false;
            changed |= ui
                .selectable_value(&mut self.channel, Channel::Stable, "Stable")
                .changed();
            changed |= ui
                .selectable_value(&mut self.channel, Channel::Beta, "Beta (pre-releases)")
                .changed();
            if changed {
                self.check();
            }
        });
        match read(&self.status) {
            Status::Idle | Status::UpToDate => {
                if matches!(read(&self.status), Status::UpToDate) {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 190, 120),
                        "\u{2713} Up to date.",
                    );
                }
                if ui.button("Check for updates").clicked() {
                    self.check();
                }
            }
            Status::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking\u{2026}");
                });
            }
            Status::Available(tag) => {
                ui.colored_label(
                    egui::Color32::from_rgb(240, 200, 120),
                    format!("{tag} is available."),
                );
                if ui
                    .button(format!("\u{2b07} Download & install {tag}"))
                    .on_hover_text("downloads the verified build and relaunches into it")
                    .clicked()
                {
                    self.install(tag);
                }
            }
            Status::Working(msg) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(msg);
                });
                ui.small("Concierge will relaunch into the new version when it's ready.");
            }
            Status::Failed(e) => {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 120), e);
                if ui.button("Retry").clicked() {
                    self.check();
                }
            }
        }
    }
}

/// The directory holding the running executables (where the swap happens).
fn install_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

fn do_install(tag: &str) -> concierge_update::Result<()> {
    let releases = concierge_update::fetch_releases(REPO)?;
    let release = releases
        .iter()
        .find(|r| r.tag_name == tag)
        .ok_or_else(|| concierge_update::Error::Http(format!("release {tag} vanished")))?;
    let asset = concierge_update::pick_asset(release, concierge_update::platform_key())
        .ok_or_else(|| {
            concierge_update::Error::NoAsset(concierge_update::platform_key().to_owned())
        })?;
    let dir =
        install_dir().ok_or_else(|| concierge_update::Error::Io("no install dir".to_owned()))?;
    let staging = concierge_platform::config_dir().join("updates");
    let staged = concierge_update::stage_update(&asset.browser_download_url, tag, &staging)?;
    let new_gui = concierge_update::apply_staged(&staged, &dir)?;
    diag::log(&format!(
        "update: applied {tag}; relaunching {}",
        new_gui.display()
    ));
    concierge_update::relaunch(&new_gui);
    // Release our image so the (already-relaunched) new build owns the window.
    std::process::exit(0);
}
