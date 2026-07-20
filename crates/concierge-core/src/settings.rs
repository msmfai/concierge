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

impl Settings {
    /// Every setting as `(key, label, value-as-string)` — projected so both views
    /// SHOW the same settings and an agent can name them to change them.
    #[must_use]
    pub fn as_rows(&self) -> Vec<(&'static str, &'static str, String)> {
        vec![
            (
                "download_dir",
                "Download folder",
                self.download_dir
                    .as_ref()
                    .map_or_else(String::new, |p| p.display().to_string()),
            ),
            (
                "max_parallel_downloads",
                "Max parallel downloads",
                self.max_parallel_downloads.to_string(),
            ),
            (
                "max_bandwidth_kib",
                "Max bandwidth (KiB/s, 0=unlimited)",
                self.max_bandwidth_kib.to_string(),
            ),
            (
                "download_retries",
                "Retries on failure",
                self.download_retries.to_string(),
            ),
            (
                "verify_checksums",
                "Verify checksums",
                self.verify_checksums.to_string(),
            ),
            (
                "keep_archives",
                "Keep archives after install",
                self.keep_archives.to_string(),
            ),
            (
                "resume_downloads",
                "Resume interrupted downloads",
                self.resume_downloads.to_string(),
            ),
            (
                "check_updates_on_startup",
                "Check for updates on startup",
                self.check_updates_on_startup.to_string(),
            ),
            (
                "update_channel",
                "Update channel",
                if self.update_channel == UpdateChannel::Beta {
                    "beta"
                } else {
                    "stable"
                }
                .to_owned(),
            ),
            (
                "open_pages_one_at_a_time",
                "Open Nexus pages one at a time",
                self.open_pages_one_at_a_time.to_string(),
            ),
            (
                "confirm_before_uninstall",
                "Confirm before uninstall",
                self.confirm_before_uninstall.to_string(),
            ),
            (
                "desktop_notifications",
                "Desktop notifications",
                self.desktop_notifications.to_string(),
            ),
            (
                "auto_apply_after_download",
                "Apply automatically after Download",
                self.auto_apply_after_download.to_string(),
            ),
            (
                "theme",
                "Theme",
                match self.theme {
                    Theme::Dark => "dark",
                    Theme::Light => "light",
                    Theme::System => "system",
                }
                .to_owned(),
            ),
            ("language", "Language", self.language.clone()),
            (
                "show_advanced",
                "Show advanced options",
                self.show_advanced.to_string(),
            ),
            (
                "minimize_to_tray",
                "Minimize to tray on close",
                self.minimize_to_tray.to_string(),
            ),
        ]
    }

    /// Set a field by its `key` from a string `value` (the `set:<key>=<value>`
    /// action). Returns whether the key was recognised. Bools accept
    /// `true`/`false`; a blank `download_dir` clears the override.
    #[must_use]
    pub fn set_by_key(&mut self, key: &str, value: &str) -> bool {
        let b = || value.eq_ignore_ascii_case("true") || value == "1";
        match key {
            "download_dir" => {
                self.download_dir = (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()));
            }
            "max_parallel_downloads" => {
                self.max_parallel_downloads = value.parse().unwrap_or(self.max_parallel_downloads);
            }
            "max_bandwidth_kib" => {
                self.max_bandwidth_kib = value.parse().unwrap_or(self.max_bandwidth_kib);
            }
            "download_retries" => {
                self.download_retries = value.parse().unwrap_or(self.download_retries);
            }
            "verify_checksums" => self.verify_checksums = b(),
            "keep_archives" => self.keep_archives = b(),
            "resume_downloads" => self.resume_downloads = b(),
            "check_updates_on_startup" => self.check_updates_on_startup = b(),
            "update_channel" => {
                self.update_channel = if value.eq_ignore_ascii_case("beta") {
                    UpdateChannel::Beta
                } else {
                    UpdateChannel::Stable
                }
            }
            "open_pages_one_at_a_time" => self.open_pages_one_at_a_time = b(),
            "confirm_before_uninstall" => self.confirm_before_uninstall = b(),
            "desktop_notifications" => self.desktop_notifications = b(),
            "auto_apply_after_download" => self.auto_apply_after_download = b(),
            "theme" => {
                self.theme = match value.to_ascii_lowercase().as_str() {
                    "dark" => Theme::Dark,
                    "light" => Theme::Light,
                    _ => Theme::System,
                }
            }
            "language" => value.clone_into(&mut self.language),
            "show_advanced" => self.show_advanced = b(),
            "minimize_to_tray" => self.minimize_to_tray = b(),
            _ => return false,
        }
        true
    }
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
    fn set_by_key_roundtrips_every_row() {
        let mut s = Settings::default();
        // Every projected row's key must be settable (no orphan keys).
        for (key, _, _) in Settings::default().as_rows() {
            assert!(s.set_by_key(key, "0"), "row key '{key}' not settable");
        }
        assert!(s.set_by_key("max_parallel_downloads", "9"));
        assert_eq!(s.max_parallel_downloads, 9);
        assert!(s.set_by_key("verify_checksums", "false"));
        assert!(!s.verify_checksums);
        assert!(s.set_by_key("theme", "dark"));
        assert_eq!(s.theme, Theme::Dark);
        assert!(s.set_by_key("update_channel", "beta"));
        assert_eq!(s.update_channel, UpdateChannel::Beta);
        assert!(!s.set_by_key("nonexistent_key", "x"));
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
