//! Downloads: the Settings section (folder + every knob) and the background
//! download-manager panel (live queue with per-file progress/speed and
//! pause/resume/cancel). Operates on `concierge::settings::Settings` held by the
//! App and the process-global `download_manager`.

use eframe::egui;

use concierge::settings::{Settings, Theme, UpdateChannel};

/// Format a byte count as a compact human string (B / KiB / MiB / GiB).
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = bytes;
    let mut u = 0usize;
    while v >= 1024 && u + 1 < UNITS.len() {
        v /= 1024;
        u += 1;
    }
    format!("{v} {}", UNITS.get(u).unwrap_or(&"B"))
}

/// Integer percent (0..=100) of done/total, avoiding float `as` conversions.
pub fn percent(done: u64, total: Option<u64>) -> u16 {
    total
        .and_then(|t| (t > 0).then(|| u16::try_from(done.saturating_mul(100) / t).unwrap_or(100)))
        .unwrap_or(0)
        .min(100)
}

/// Render the Downloads + Behaviour + Interface settings. Returns `true` if any
/// value changed (the caller persists + applies side effects).
pub fn settings_section(ui: &mut egui::Ui, s: &mut Settings) -> bool {
    let mut changed = false;
    ui.strong("Downloads");

    ui.horizontal(|ui| {
        ui.label("Download folder:");
        let mut path = s
            .download_dir
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string());
        if ui
            .add(
                egui::TextEdit::singleline(&mut path)
                    .desired_width(260.0)
                    .hint_text("default: each modpack's own cache"),
            )
            .on_hover_text("shared, content-addressed archive cache (dedupes across modpacks)")
            .changed()
        {
            let t = path.trim();
            s.download_dir = if t.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(t))
            };
            changed = true;
        }
        if ui
            .button("Reset")
            .on_hover_text("use the default per-modpack cache")
            .clicked()
        {
            s.download_dir = None;
            changed = true;
        }
    });

    changed |= ui
        .add(
            egui::Slider::new(&mut s.max_parallel_downloads, 1..=16).text("Max parallel downloads"),
        )
        .changed();

    ui.horizontal(|ui| {
        ui.label("Max bandwidth:");
        changed |= ui
            .add(
                egui::DragValue::new(&mut s.max_bandwidth_kib)
                    .speed(64)
                    .range(0..=1_048_576)
                    .suffix(" KiB/s"),
            )
            .on_hover_text("global download speed cap — 0 means unlimited")
            .changed();
        if s.max_bandwidth_kib == 0 {
            ui.weak("(unlimited)");
        }
    });

    changed |= ui
        .add(egui::Slider::new(&mut s.download_retries, 0..=10).text("Retries on failure"))
        .changed();
    changed |= ui
        .checkbox(&mut s.resume_downloads, "Resume interrupted downloads")
        .changed();
    changed |= ui
        .checkbox(&mut s.verify_checksums, "Verify checksums")
        .changed();
    changed |= ui
        .checkbox(&mut s.keep_archives, "Keep archives after install")
        .changed();

    ui.separator();
    ui.strong("Behaviour");
    ui.horizontal(|ui| {
        ui.label("Update channel:");
        changed |= ui
            .selectable_value(&mut s.update_channel, UpdateChannel::Stable, "Stable")
            .changed();
        changed |= ui
            .selectable_value(&mut s.update_channel, UpdateChannel::Beta, "Beta")
            .changed();
    });
    changed |= ui
        .checkbox(
            &mut s.check_updates_on_startup,
            "Check for updates on startup",
        )
        .changed();
    changed |= ui
        .checkbox(
            &mut s.open_pages_one_at_a_time,
            "Open Nexus pages one at a time (guided) instead of all at once",
        )
        .changed();
    changed |= ui
        .checkbox(
            &mut s.auto_apply_after_download,
            "Apply (deploy) automatically after Download",
        )
        .changed();
    changed |= ui
        .checkbox(&mut s.confirm_before_uninstall, "Confirm before uninstall")
        .changed();
    changed |= ui
        .checkbox(&mut s.desktop_notifications, "Desktop notifications")
        .changed();

    ui.separator();
    ui.strong("Interface");
    ui.horizontal(|ui| {
        ui.label("Theme:");
        changed |= ui
            .selectable_value(&mut s.theme, Theme::System, "System")
            .changed();
        changed |= ui
            .selectable_value(&mut s.theme, Theme::Dark, "Dark")
            .changed();
        changed |= ui
            .selectable_value(&mut s.theme, Theme::Light, "Light")
            .changed();
    });
    changed |= ui
        .checkbox(&mut s.minimize_to_tray, "Minimize to tray on close")
        .changed();
    changed |= ui
        .checkbox(&mut s.show_advanced, "Show advanced options")
        .changed();

    changed
}
