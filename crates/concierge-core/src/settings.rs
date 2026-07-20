//! Persistent user settings — one `settings.toml` under the config dir, matching
//! (and extending) the knobs Vortex exposes. Loaded once at startup into a
//! process-global so any crate (the store's download location, the download
//! manager's concurrency/throttle) can read it without threading it through.
//!
//! Missing fields fall back to defaults (`#[serde(default)]`), so an old or
//! hand-edited file never fails to load and new settings appear with sane values.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The update channel a user opts into (mirrors `concierge-update::Channel` but
/// lives here so core doesn't depend on the updater crate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Stable,
    Beta,
}

/// The GUI theme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    System,
}

/// All persisted settings. Grouped by area in comments; flat on disk (TOML).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // a settings record is mostly toggles
pub struct Settings {
    // --- Downloads ---------------------------------------------------------
    /// Where downloaded archives live (the shared, content-addressed cache).
    /// `None` = the default `<workspace>/store`. Vortex: "Downloads Folder".
    pub download_dir: Option<PathBuf>,
    /// How many files download at once. Vortex: "Max parallel downloads".
    pub max_parallel_downloads: usize,
    /// Global download speed cap in KiB/s; `0` = unlimited. Vortex: "Max bandwidth".
    pub max_bandwidth_kib: u64,
    /// Retry a failed download this many times (each retry resumes).
    pub download_retries: u32,
    /// Verify every downloaded file against its pinned checksum.
    pub verify_checksums: bool,
    /// Keep archives in the cache after install (vs delete to save space).
    pub keep_archives: bool,
    /// Resume interrupted downloads from their `.part` (vs restart).
    pub resume_downloads: bool,

    // --- Behaviour ---------------------------------------------------------
    /// Check GitHub for a newer Concierge on startup.
    pub check_updates_on_startup: bool,
    /// Which releases the updater offers.
    pub update_channel: UpdateChannel,
    /// Open Nexus mod pages one-at-a-time (the guided panel) vs a tab per mod.
    pub open_pages_one_at_a_time: bool,
    /// Ask before uninstalling / destructive actions.
    pub confirm_before_uninstall: bool,
    /// Show desktop notifications for long-running actions.
    pub desktop_notifications: bool,
    /// Automatically Apply (deploy) after a successful Download.
    pub auto_apply_after_download: bool,

    // --- Interface ---------------------------------------------------------
    pub theme: Theme,
    /// UI language code (e.g. `"en"`). i18n is a roadmap follow-up; stored now.
    pub language: String,
    /// Reveal advanced/expert controls.
    pub show_advanced: bool,
    /// Minimise to the system tray instead of quitting on window close.
    pub minimize_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: None,
            max_parallel_downloads: 4,
            max_bandwidth_kib: 0,
            download_retries: 4,
            verify_checksums: true,
            keep_archives: true,
            resume_downloads: true,
            check_updates_on_startup: true,
            update_channel: UpdateChannel::Stable,
            open_pages_one_at_a_time: true,
            confirm_before_uninstall: true,
            desktop_notifications: true,
            auto_apply_after_download: false,
            theme: Theme::System,
            language: "en".to_owned(),
            show_advanced: false,
            minimize_to_tray: false,
        }
    }
}

fn settings_path() -> PathBuf {
    concierge_platform::config_dir().join("settings.toml")
}

/// The process-global, loaded once at startup and updated on save.
static CURRENT: RwLock<Option<Settings>> = RwLock::new(None);

/// Load settings from disk (or defaults), install them as the global, return them.
/// Call once at startup. A malformed file falls back to defaults rather than
/// failing the app (and is logged).
pub fn load() -> Settings {
    let s = match std::fs::read_to_string(settings_path()) {
        Ok(text) => toml::from_str::<Settings>(&text).unwrap_or_else(|e| {
            concierge_platform::diag(&format!("settings: parse failed ({e}); using defaults"));
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(s.clone());
    }
    s
}

/// The current settings (defaults if [`load`] was never called).
#[must_use]
pub fn get() -> Settings {
    CURRENT
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

/// Persist `settings` to disk and update the global. Errors if the file can't be
/// written (the in-memory global is still updated so the session reflects it).
pub fn save(settings: &Settings) -> Result<()> {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(settings.clone());
    }
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Other(e.to_string()))?;
    }
    let text = toml::to_string_pretty(settings).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| Error::Other(e.to_string()))?;
    Ok(())
}

/// The configured download-cache directory override, if any (else `None` so the
/// caller uses the default `<workspace>/store`). Read by `Repo::store`.
#[must_use]
pub fn download_dir_override() -> Option<PathBuf> {
    get().download_dir
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let d = Settings::default();
        assert_eq!(d.max_parallel_downloads, 4);
        assert!(d.verify_checksums);
        assert!(d.resume_downloads);
        assert_eq!(d.update_channel, UpdateChannel::Stable);
        assert!(d.download_dir.is_none());
    }

    #[test]
    fn toml_roundtrips() {
        let mut s = Settings::default();
        s.max_parallel_downloads = 8;
        s.max_bandwidth_kib = 1024;
        s.download_dir = Some(PathBuf::from("/mods/downloads"));
        s.theme = Theme::Dark;
        s.update_channel = UpdateChannel::Beta;
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.max_parallel_downloads, 8);
        assert_eq!(back.max_bandwidth_kib, 1024);
        assert_eq!(back.download_dir, Some(PathBuf::from("/mods/downloads")));
        assert_eq!(back.theme, Theme::Dark);
        assert_eq!(back.update_channel, UpdateChannel::Beta);
    }

    #[test]
    fn partial_file_uses_defaults_for_missing() {
        // Only one field present — everything else must fall back to defaults.
        let text = "max_parallel_downloads = 2\n";
        let s: Settings = toml::from_str(text).unwrap();
        assert_eq!(s.max_parallel_downloads, 2);
        assert!(s.verify_checksums); // default
        assert_eq!(s.language, "en"); // default
    }

    #[test]
    fn empty_file_is_all_defaults() {
        let s: Settings = toml::from_str("").unwrap();
        assert_eq!(
            s.max_parallel_downloads,
            Settings::default().max_parallel_downloads
        );
    }
}
