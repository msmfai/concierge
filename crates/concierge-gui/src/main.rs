//! Concierge GUI — game-agnostic dashboard over the shared-cache + profiles
//! model. Pick a game and profile; see and edit its mods (toggle, reorder,
//! add, remove), the load order, conflicts and invariant lints; then
//! realize/check/launch with a live log. Every mutation goes through the pure,
//! format-preserving `concierge::manifest_edit` functions — the manifest stays
//! the single source of truth; the UI writes it and re-evals. All real work
//! lives in the core + accelerator crates; this is a thin shell.

#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod diag;
mod terminal;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use concierge::manifest::{Manifest, Mod};
use concierge::manifest_edit::{self, NewMod, NewSource};
use concierge::plan::{eval, Plan};
use concierge::profiles::{self, GameEntry, ProfileEntry};
use concierge::repo::Repo;
use concierge::state::Realized;
use concierge_ai::tools::{catalog_search, CatalogFilter, CatalogHit};
use concierge_lint::{Severity, Violation};

/// The two halves of the Nix-style model: the editable declaration (manifest)
/// vs the read-only realised generation (what is deployed on disk).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum CentralTab {
    Declaration,
    Realised,
}

/// The last action the user triggered — recorded so a panic log has context.
static LAST_ACTION: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Where crash diagnostics are written (portable per-OS data dir).
fn crash_log_path() -> std::path::PathBuf {
    concierge_platform::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("concierge")
        .join("crash.log")
}

/// The durable action log — the operational log stream (fetch/add/apply/…)
/// persisted so a failed session stays diagnosable after the GUI exits.
fn action_log_path() -> std::path::PathBuf {
    concierge_platform::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("concierge")
        .join("actions.log")
}

/// Append log lines to the durable action log with a monotonic frame stamp
/// (`SystemTime` seconds). Best-effort — a logging failure never disrupts the UI.
fn append_action_log(lines: &[String]) {
    use std::io::Write as _;
    if lines.is_empty() {
        return;
    }
    let path = action_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        for l in lines {
            let _ = writeln!(f, "{ts}\t{l}");
        }
    }
}

/// Install a panic hook that appends the panic, the last action, and a backtrace
/// to the crash log (then runs the default hook so stderr still shows it). The
/// unexplained interaction-triggered crash is finally diagnosable.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write as _;
        let path = crash_log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let bt = std::backtrace::Backtrace::force_capture();
        let last = LAST_ACTION
            .lock()
            .map_or_else(|_| String::new(), |s| s.clone());
        let body = format!(
            "==== Concierge panic ====\nlast action: {last}\n{info}\n\nbacktrace:\n{bt}\n\n"
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(body.as_bytes());
        }
        default(info);
    }));
}

/// Choose which enumerated wgpu adapter to use, by backend preference:
/// Vulkan, then Metal, then DX12, then GL, then whatever is first. Vulkan leads
/// because it is the one backend that composites correctly through the Wine and
/// Metal translation layers, and it is equally fine on native Windows and Linux;
/// macOS exposes no Vulkan adapter, so it falls through to Metal. Returns the
/// index into the `backends` slice, or `None` if it is empty.
fn preferred_adapter(backends: &[eframe::wgpu::Backend]) -> Option<usize> {
    use eframe::wgpu::Backend;
    for want in [Backend::Vulkan, Backend::Metal, Backend::Dx12, Backend::Gl] {
        if let Some(i) = backends.iter().position(|b| *b == want) {
            return Some(i);
        }
    }
    (!backends.is_empty()).then_some(0)
}

/// The wgpu backend set to enable, per platform. This fixes the surface type,
/// so the choice must be the backend that actually composites on screen.
/// - macOS: Metal.
/// - Windows: Vulkan when a Vulkan adapter exists (it composites under
///   CrossOver/Wine, where DX12 draws a blank window, and is present on
///   essentially all modern Windows GPUs), otherwise DX12. Override with
///   `WGPU_BACKEND=dx12`.
/// - Other (Linux): Vulkan plus GL as a fallback.
fn graphics_backends() -> eframe::wgpu::Backends {
    use eframe::wgpu::Backends;
    if cfg!(target_os = "macos") {
        Backends::METAL
    } else if cfg!(target_os = "windows") {
        // Use Vulkan when its loader is installed, else DX12. Detection is a
        // file check, NOT a wgpu probe: creating a throwaway Vulkan instance
        // before eframe's corrupts the real one under winevulkan/MoltenVK and
        // blanks the window.
        if windows_has_vulkan_loader() {
            Backends::VULKAN
        } else {
            Backends::DX12
        }
    } else {
        Backends::VULKAN | Backends::GL
    }
}

/// Whether the Vulkan ICD loader (`vulkan-1.dll`) is installed. It ships with
/// any Vulkan-capable Windows GPU driver and with Wine's winevulkan, so its
/// presence is a reliable, side-effect-free signal that Vulkan is usable.
fn windows_has_vulkan_loader() -> bool {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&root)
        .join("System32")
        .join("vulkan-1.dll")
        .exists()
}

fn main() -> eframe::Result {
    // Surfaces wgpu/winit diagnostics when RUST_LOG is set (e.g.
    // RUST_LOG=wgpu_core=info,wgpu_hal=info); silent otherwise.
    env_logger::Builder::from_default_env().init();
    // A GUI app has no console on Windows, so also log to a file beside the exe
    // (see diag) — that's how a failed terminal spawn becomes readable.
    diag::start_session();
    // Wire every family/leaf adapter crate into core's resolver at startup.
    concierge_games::register();
    install_panic_hook();
    // A browser "Mod Manager Download" (nxm://) launches the app with the URL as
    // an arg; drop it in the inbox so this (or an already-running) instance pins
    // it. (The already-running-instance case for a running app is served by the
    // per-frame inbox poll in update(); browser->running-app without a relaunch
    // still needs macOS openURLs glue.)
    for arg in std::env::args().skip(1) {
        if arg.starts_with("nxm://") {
            let _ = concierge::nexus::append_nxm_inbox(&arg);
        }
    }
    // Render with wgpu, targeting each platform's native graphics API. The
    // default glow/OpenGL path fails on setups without a modern WGL/GL context
    // (older Intel drivers, VMs, remote desktop, Wine), so it isn't a reliable
    // default. The chosen backend fixes the *surface* type, which must match the
    // renderer that composites: under CrossOver/Wine only Vulkan (via MoltenVK)
    // composites — DX12 (via D3DMetal) draws to a window that stays blank.
    let backends = graphics_backends();
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = backends;
        // When more than one backend is enabled (Linux: Vulkan+GL), prefer the
        // Vulkan adapter among those the surface supports.
        setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _surface| {
            let order: Vec<eframe::wgpu::Backend> =
                adapters.iter().map(|a| a.get_info().backend).collect();
            let idx =
                preferred_adapter(&order).ok_or_else(|| "no compatible wgpu adapter".to_owned())?;
            adapters
                .get(idx)
                .cloned()
                .ok_or_else(|| "adapter index out of range".to_owned())
        }));
    }
    // Fifo (plain vsync) is the most broadly supported present mode; the default
    // AutoVsync can negotiate a swapchain that reports perpetually "suboptimal"
    // through Vulkan translation layers and never composites the rendered frame.
    wgpu_options.present_mode = eframe::wgpu::PresentMode::Fifo;
    // Under CrossOver/Wine the compositor shows a windowed swapchain's clear
    // color but not its drawn content; a borderless-fullscreen swapchain
    // composites correctly. Opt in with CONCIERGE_FULLSCREEN=1 (native platforms
    // keep a normal window).
    let mut viewport = eframe::egui::ViewportBuilder::default();
    if std::env::var_os("CONCIERGE_FULLSCREEN").is_some() {
        viewport = viewport.with_fullscreen(true);
    }
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        viewport,
        ..Default::default()
    };
    // Diagnostic: CONCIERGE_MINIMAL=1 runs a trivial one-label window with the
    // SAME wgpu setup, to distinguish a Concierge-specific rendering problem
    // from an eframe/wgpu/translation-layer stack problem.
    if std::env::var_os("CONCIERGE_MINIMAL").is_some() {
        return eframe::run_simple_native("Concierge", native_options, |ctx, _frame| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Concierge minimal render test");
                ui.label("If you can read this, wgpu is compositing.");
            });
        });
    }
    eframe::run_native(
        "Concierge",
        native_options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

/// egui's default text font lacks most symbols (arrows, dingbats, geometric
/// shapes, math) AND all emoji, so those render as missing-glyph boxes. Append
/// two fallbacks to both families: `DejaVu Sans` for the plain symbols and
/// `Noto Emoji` for the emoji — in that order, so a symbol that exists in both
/// (e.g. `▶`/`✔`) takes the clean outline over an emoji glyph.
fn install_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "dejavu-symbols".to_owned(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../assets/DejaVuSans.ttf"
        ))),
    );
    fonts.font_data.insert(
        "noto-emoji".to_owned(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoEmoji-Regular.ttf"
        ))),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let fam = fonts.families.entry(family).or_default();
        fam.push("dejavu-symbols".to_owned());
        fam.push("noto-emoji".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// A collected, deferred mutation — applied after the frame so the render pass
/// never holds a conflicting borrow of `self`.
enum Edit {
    Toggle(String, bool),
    Nudge(String, bool),
    Move(usize, usize),
    Remove(String),
    Add(NewMod),
    /// Point the profile at the game's install folder (`[game].pristine`).
    SetPristine(String),
    /// Set `[game].dlc` — the base+DLC masters detected in the install (only what
    /// the player owns, so the load order doesn't assume every DLC).
    SetDlc(Vec<String>),
    /// Set `[compat]` game version + loader (for Modrinth/Minecraft resolves).
    SetCompat(String, String),
}

/// A destructive action awaiting confirmation (Nielsen #5 / NN/g). The guard is
/// deliberately co-located with each destructive call site.
enum Confirm {
    RemoveMod(String),
    Undeploy,
    RestoreSave(String),
    Rollback(u64),
    DeleteProfile(String),
}

impl Confirm {
    fn prompt(&self) -> String {
        match self {
            Self::RemoveMod(n) => format!("Remove mod '{n}' from your setup?"),
            Self::Undeploy => "Uninstall — remove the installed mods from the game?".to_owned(),
            Self::RestoreSave(g) => format!("Restore saves from backup {g}, overwriting current saves?"),
            Self::Rollback(n) => format!("Restore your setup to version {n}?"),
            Self::DeleteProfile(n) => format!("Delete profile '{n}' and all its data (setup, versions, save backups)? This cannot be undone."),
        }
    }
}

/// The off-thread outcome of one catalog read: the hits, an optional refreshed
/// category list (only when it was recomputed), and a status message.
struct BrowseResult {
    hits: Vec<CatalogHit>,
    categories: Option<Vec<(String, u64)>>,
    msg: String,
    /// `(rows, synced_epoch)` for the sync status line — computed off-thread so
    /// the window never queries the catalog during a repaint.
    status: (u64, u64),
}

#[derive(Default)]
struct AddForm {
    open: bool,
    name: String,
    version: String,
    nexus_mod: String,
    nexus_file: String,
    url: String,
    md5: String,
    file: String,
    plugins: String,
}

/// How an action's outcome banner reads: green success, amber "you still
/// have a manual step", or red failure. Keeps a partial/manual result from
/// masquerading as a completed one (e.g. Download that only opened a web page).
#[derive(Clone, Copy)]
enum FlashKind {
    Ok,
    Warn,
    Err,
}

#[allow(clippy::struct_excessive_bools)]
struct App {
    workspace: Option<PathBuf>,
    games: Vec<GameEntry>,
    game_idx: usize,
    profiles: Vec<ProfileEntry>,
    profile_idx: usize,
    manifest: Option<Manifest>,
    plan: Option<Plan>,
    lint: Vec<Violation>,
    rel_issues: Vec<String>,
    error: Option<String>,
    /// A prominent green success banner from the last action (Apply/Check/…).
    notice: Option<String>,
    /// An amber "action succeeded but a manual step remains" banner — e.g. a
    /// Download that only opened the mod's web page and still needs a save.
    warn: Option<String>,
    /// In-app Nexus API-key entry (Settings), so users don't edit config files.
    nexus_key_input: String,
    /// In-app game-install-path entry (Settings) — writes `[game].pristine` so a
    /// fresh profile stops being "not realized" and can deploy.
    pristine_input: String,
    /// Minecraft pack's target version + loader (Settings) — `[compat]`, used to
    /// resolve the right Modrinth build.
    mc_version_input: String,
    mc_loader_input: String,
    /// Whether the `claude` CLI is on PATH — cached at startup (spawning a probe
    /// every frame is wasteful) and used to gate the AI features gracefully.
    agent_available: bool,
    /// Type-to-filter text for the (long) "+ add game" dropdown.
    add_game_filter: String,
    new_profile_name: String,
    search: String,
    selected: Option<String>,
    add: AddForm,
    cache: (usize, u64),
    log: Vec<String>,
    log_rx: Receiver<String>,
    log_tx: Sender<String>,
    /// Action outcome (ok?, message) surfaced as a banner, not just the log.
    flash_rx: Receiver<(FlashKind, String)>,
    flash_tx: Sender<(FlashKind, String)>,
    busy: Arc<AtomicBool>,
    /// Set by an off-thread action that edited the manifest; the update loop
    /// reloads the plan (on the UI thread) when it sees this.
    reload_pending: Arc<AtomicBool>,
    edit: Option<Edit>,
    /// The agent view IS a terminal: the user's real interactive agent,
    /// running in the concierge-shell sandbox for the active profile.
    term: Option<terminal::PtyTerminal>,
    term_epoch: u64,
    central_tab: CentralTab,
    dark: bool,
    ai_visible: bool,
    undo: Vec<String>,
    /// The first-run quick-start guide is showing (auto-hides once a pack is set
    /// up + applied; re-openable from the top bar).
    quickstart_open: bool,
    settings_open: bool,
    diff_open: bool,
    /// The enriched Preview text, computed once when the window opens (the
    /// per-file conflict scan touches disk, so it must stay off the render path).
    preview_lines: Vec<String>,
    browse_open: bool,
    /// The Wabbajack-style guided download queue window.
    download_session_open: bool,
    browse_query: String,
    nxm_input: String,
    nexus_account: Arc<std::sync::Mutex<String>>,
    /// Live catalog-sync progress (Some while a sync runs) — shown in Browse so
    /// it doesn't look frozen.
    sync_progress: Arc<std::sync::Mutex<Option<String>>>,
    /// When the current catalog sync started — drives the progress bar's ETA.
    sync_started: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    browse_hits: Vec<CatalogHit>,
    browse_msg: String,
    /// First-class category filter: None = all categories.
    browse_category: Option<String>,
    /// Sort order (default: most endorsed).
    browse_sort: concierge_ai::tools::SortBy,
    /// (category, count) for the active game's catalog — the filter's options.
    browse_categories: Vec<(String, u64)>,
    /// Catalog reads run off the UI thread; results arrive here tagged with the
    /// request seq so stale ones (from a superseded search) are dropped.
    browse_tx: Sender<(u64, BrowseResult)>,
    browse_rx: Receiver<(u64, BrowseResult)>,
    browse_seq: u64,
    browse_busy: bool,
    /// Cached `(rows, synced_epoch)` for the sync status line (refreshed by the
    /// worker), and a flag the sync thread sets to trigger a re-search.
    browse_status: (u64, u64),
    browse_refresh: Arc<AtomicBool>,
    mutable: bool,
    confirm: Option<Confirm>,
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Realize,
    Fetch,
    Check,
    Launch,
    Undeploy,
    Reconcile,
    Conflicts,
    SortLoot,
    ResolveDeps,
    SuggestPatches,
}

impl Action {
    /// Whether this action edits `manifest.toml` — the GUI reloads the plan
    /// after such an action completes off-thread.
    const fn mutates_manifest(self) -> bool {
        // Fetch auto-pins md5s; Realize auto-pins + resolves layout — both
        // edit manifest.toml, so the UI must reload the plan afterward.
        matches!(
            self,
            Self::SortLoot | Self::ResolveDeps | Self::Fetch | Self::Realize
        )
    }
}

/// True when the game has a rendered plugin load order with base masters — the
/// real capability behind the Bethesda-style load-order UI (sort/conflicts/plugin
/// panel). Asked of the game's adapter, not a hardcoded kind list: the old
/// `matches!(kind, "fallout4" | "skyrimse")` wrongly excluded Skyrim LE, Oblivion,
/// Fallout 3/NV and Starfield, which are the same family.
fn is_bethesda(kind: &str) -> bool {
    concierge::game::adapter_for(kind).is_ok_and(|a| a.plugin_bases().is_some())
}

const fn mod_source(m: &Mod) -> &'static str {
    if m.pipeline.is_some() {
        "pipeline"
    } else if m.nix.is_some() {
        "nix"
    } else if m.nexus_mod_id.is_some() {
        "nexus"
    } else if m.url.is_some() {
        "url"
    } else {
        "-"
    }
}

impl App {
    fn new() -> Self {
        let (log_tx, log_rx) = std::sync::mpsc::channel();
        let (flash_tx, flash_rx) = std::sync::mpsc::channel();
        let (browse_tx, browse_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            workspace: None,
            games: Vec::new(),
            game_idx: 0,
            profiles: Vec::new(),
            profile_idx: 0,
            manifest: None,
            plan: None,
            lint: Vec::new(),
            rel_issues: Vec::new(),
            error: None,
            notice: None,
            warn: None,
            nexus_key_input: String::new(),
            pristine_input: String::new(),
            mc_version_input: String::new(),
            mc_loader_input: String::new(),
            agent_available: which_agent_available(),
            add_game_filter: String::new(),
            new_profile_name: String::new(),
            search: String::new(),
            selected: None,
            add: AddForm::default(),
            cache: (0, 0),
            log: Vec::new(),
            log_rx,
            log_tx,
            flash_rx,
            flash_tx,
            busy: Arc::new(AtomicBool::new(false)),
            reload_pending: Arc::new(AtomicBool::new(false)),
            edit: None,
            term: None,
            term_epoch: 0,
            central_tab: CentralTab::Declaration,
            dark: true,
            ai_visible: true,
            undo: Vec::new(),
            quickstart_open: true,
            settings_open: false,
            diff_open: false,
            preview_lines: Vec::new(),
            browse_open: false,
            download_session_open: false,
            browse_query: String::new(),
            nxm_input: String::new(),
            nexus_account: Arc::new(std::sync::Mutex::new(String::new())),
            sync_progress: Arc::new(std::sync::Mutex::new(None)),
            sync_started: Arc::new(std::sync::Mutex::new(None)),
            browse_hits: Vec::new(),
            browse_msg: String::new(),
            browse_category: None,
            browse_sort: concierge_ai::tools::SortBy::Endorsements,
            browse_categories: Vec::new(),
            browse_tx,
            browse_rx,
            browse_seq: 0,
            browse_busy: false,
            browse_status: (0, 0),
            browse_refresh: Arc::new(AtomicBool::new(false)),
            mutable: true,
            confirm: None,
        };
        app.discover();
        // Surface a crash from a previous run (the panic hook wrote it), then
        // move it aside so it isn't re-reported.
        let crash = crash_log_path();
        if crash.is_file() {
            let prev = crash.with_extension("log.prev");
            let _ = std::fs::rename(&crash, &prev);
            app.error = Some(format!(
                "Concierge crashed on a previous run — diagnostics saved to {}",
                prev.display()
            ));
        }
        app
    }

    fn discover(&mut self) {
        self.error = None;
        match profiles::workspace() {
            Ok(ws) => {
                self.games = profiles::list_games(&ws);
                let _ = self.log_tx.send(format!(
                    "workspace: {} ({} games)",
                    ws.display(),
                    self.games.len()
                ));
                self.workspace = Some(ws);
                self.game_idx = self.game_idx.min(self.games.len().saturating_sub(1));
                self.reload_profiles();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn reload_profiles(&mut self) {
        self.profiles = self
            .games
            .get(self.game_idx)
            .map(|g| profiles::list_profiles(&g.dir))
            .unwrap_or_default();
        self.profile_idx = self.profile_idx.min(self.profiles.len().saturating_sub(1));
        // Locked lives on disk; render the active profile's truth.
        if let Some(p) = self.profiles.get(self.profile_idx) {
            self.mutable = !profiles::is_locked(&p.dir);
        }
        // The catalog + its categories are per-game — reset the browse filters.
        self.browse_categories.clear();
        self.browse_category = None;
        self.browse_hits.clear();
        self.reload_plan();
    }

    fn active_repo(&self) -> Option<Repo> {
        let profile = self.profiles.get(self.profile_idx)?;
        Some(Repo::at(&profile.dir))
    }

    fn manifest_path(&self) -> Option<PathBuf> {
        Some(self.active_repo()?.profile.join("manifest.toml"))
    }

    /// The active game's vocabulary — Bethesda says "load order", etc. Generic
    /// when no game is resolved. Lets game-specific terms override generic labels.
    fn active_lexicon(&self) -> concierge::game::Lexicon {
        self.plan
            .as_ref()
            .and_then(|p| concierge::game::adapter_for(&p.game.kind).ok())
            .map_or_else(
                concierge::game::Lexicon::default,
                concierge::game::GameAdapter::lexicon,
            )
    }

    /// Extract the pure [`concierge_ui::UiFacts`] snapshot from this frame's
    /// state — the SAME view-model the headless text/automaton renderer uses.
    fn ui_facts(&self) -> concierge_ui::UiFacts {
        let kind = self.plan.as_ref().map(|p| p.game.kind.clone());
        let lex = self.active_lexicon();
        let confirm = self.confirm.as_ref().map(|c| match c {
            Confirm::RemoveMod(_) => concierge_ui::ConfirmKind::RemoveMod,
            Confirm::Undeploy => concierge_ui::ConfirmKind::Uninstall,
            Confirm::RestoreSave(_) => concierge_ui::ConfirmKind::RestoreSave,
            Confirm::Rollback(_) => concierge_ui::ConfirmKind::Rollback,
            Confirm::DeleteProfile(_) => concierge_ui::ConfirmKind::DeleteProfile,
        });
        let tab = match self.central_tab {
            CentralTab::Declaration => concierge_ui::Tab::Setup,
            CentralTab::Realised => concierge_ui::Tab::Installed,
        };
        let mods = self.manifest.as_ref().map_or_else(Vec::new, |m| {
            m.mods
                .iter()
                .enumerate()
                .map(|(i, md)| concierge_ui::ModRow {
                    order: i + 1,
                    name: md.name.clone(),
                    enabled: md.enabled,
                })
                .collect()
        });
        let panels = self.build_panels();
        let (versions, saves, pin_status) = self.build_versions_saves();
        concierge_ui::UiFacts {
            has_workspace: self.workspace.is_some(),
            workspace_path: self.workspace.as_ref().map(|w| w.display().to_string()),
            game_count: self.games.len(),
            active_game: kind.clone(),
            active_profile: self.profiles.get(self.profile_idx).map(|p| p.name.clone()),
            is_bethesda: kind.as_deref().is_some_and(is_bethesda),
            has_catalog: self
                .plan
                .as_ref()
                .is_some_and(|p| p.game.nexus_domain.is_some() || p.game.modrinth_domain.is_some()),
            tab: concierge_ui::TabFacts(tab),
            mutable: self.mutable,
            has_undo: !self.undo.is_empty(),
            busy: self.busy.load(Ordering::SeqCst),
            ai_busy: self.term.as_ref().is_some_and(|t| !t.finished()),
            settings_open: self.settings_open,
            browse_open: self.browse_open,
            diff_open: self.diff_open,
            confirm,
            confirm_prompt: self.confirm.as_ref().map(Confirm::prompt),
            error: self.error.clone(),
            mods,
            log_tail: self.log.iter().rev().take(5).rev().cloned().collect(),
            order_word: lex.order.to_owned(),
            sort_label: lex.sort_action.to_owned(),
            nxm_input: self.nxm_input.clone(),
            search: self.search.clone(),
            browse_query: self.browse_query.clone(),
            browse_msg: self.browse_msg.clone(),
            browse_hits: {
                let declared: std::collections::BTreeSet<u64> = self
                    .manifest
                    .as_ref()
                    .map(|m| m.mods.iter().filter_map(|md| md.nexus_mod_id).collect())
                    .unwrap_or_default();
                self.browse_hits
                    .iter()
                    .map(|h| concierge_ui::BrowseHit {
                        mod_id: h.mod_id,
                        name: h.name.clone(),
                        endorsements: h.endorsements,
                        author: h.author.clone(),
                        summary: h.summary.clone(),
                        category: h.category.clone(),
                        downloads: h.downloads,
                        updated_at: h.updated_at.clone(),
                        added: declared.contains(&h.mod_id),
                    })
                    .collect()
            },
            add_open: self.add.open,
            add_fields: if self.add.open {
                self.add_form_fields()
            } else {
                Vec::new()
            },
            versions,
            saves,
            pin_status,
            ai_input: String::new(),
            ai_can_send: self.plan.is_some(),
            ai_quick: concierge_ai::agent::quick_actions()
                .iter()
                .map(|qa| qa.label.to_owned())
                .collect(),
            games: self.games.iter().map(|g| g.game.clone()).collect(),
            profiles: self.profiles.iter().map(|p| p.name.clone()).collect(),
            game_idx: self.game_idx,
            profile_idx: self.profile_idx,
            new_profile: self.new_profile_name.clone(),
            cache_summary: {
                let (n, bytes) = self.cache;
                format!("cache: {n}, {}", human_bytes(bytes))
            },
            panels,
        }
    }

    /// Setup versions (generations) + save backups + the pinned-versions status,
    /// as projected data. Reads the repo (same as the panels did inline).
    fn build_versions_saves(&self) -> (Vec<concierge_ui::VersionRow>, Vec<String>, Option<String>) {
        let Some(repo) = self.active_repo() else {
            return (Vec::new(), Vec::new(), None);
        };
        let versions = concierge::generations::list(&repo)
            .into_iter()
            .map(|g| concierge_ui::VersionRow {
                number: g.number,
                hash: g.plan_hash.chars().take(10).collect(),
            })
            .collect();
        let saves = concierge::saves::list(&repo);
        let pin_status = concierge::lockfile::read(&repo).map(|lock| {
            let synced = self
                .plan
                .as_ref()
                .and_then(|p| p.hash().ok())
                .is_some_and(|h| h == lock.plan_hash);
            format!(
                "pinned {} ({} mods) — {}",
                lock.plan_hash.chars().take(10).collect::<String>(),
                lock.mods.len(),
                if synced {
                    "matches your setup"
                } else {
                    "stale — Apply to re-lock"
                }
            )
        });
        (versions, saves, pin_status)
    }

    /// The add-mod form's text fields as projected [`concierge_ui::Field`]s.
    fn add_form_fields(&self) -> Vec<concierge_ui::Field> {
        let f = |id: &str, label: &str, v: &str| concierge_ui::Field {
            id: id.to_owned(),
            label: label.to_owned(),
            value: v.to_owned(),
        };
        vec![
            f("add_name", "name", &self.add.name),
            f("add_version", "version", &self.add.version),
            f("add_nexus_mod", "nexus mod id", &self.add.nexus_mod),
            f("add_nexus_file", "nexus file id", &self.add.nexus_file),
            f("add_url", "or url", &self.add.url),
            f("add_md5", "md5", &self.add.md5),
            f("add_file", "file", &self.add.file),
            f("add_plugins", "plugins (comma-sep)", &self.add.plugins),
        ]
    }

    /// Build the read-only info panels for the visible window(s). Settings for
    /// now (workspace, [game.paths], API-key presence, Nexus account status).
    fn build_panels(&self) -> Vec<concierge_ui::Panel> {
        let mut panels = Vec::new();
        if self.settings_open {
            let mut lines = Vec::new();
            match &self.workspace {
                Some(ws) => lines.push(format!(
                    "workspace: {} ({} games)",
                    ws.display(),
                    self.games.len()
                )),
                None => lines.push("no workspace resolved".to_owned()),
            }
            if let Some(m) = &self.manifest {
                lines.push("game paths [game.paths]:".to_owned());
                if m.game.paths.is_empty() {
                    lines.push("  (none declared)".to_owned());
                }
                for (k, v) in &m.game.paths {
                    lines.push(format!("  {k} = {}", v.display()));
                }
            }
            let key = |rel: &str| {
                if config_dir().join(rel).exists() {
                    "set"
                } else {
                    "not set"
                }
            };
            lines.push(format!(
                "API keys: Nexus={}, Anthropic={}",
                key("nexus-api-key"),
                key("anthropic-api-key")
            ));
            if let Ok(s) = self.nexus_account.lock() {
                if !s.is_empty() {
                    lines.push(format!("account: {s}"));
                }
            }
            panels.push(concierge_ui::Panel {
                id: "settings".into(),
                title: "Settings".into(),
                lines,
            });
        }
        if self.diff_open {
            panels.push(concierge_ui::Panel {
                id: "preview".into(),
                title: "Preview changes".into(),
                lines: self.preview_lines.clone(),
            });
        }
        panels
    }

    /// Compute the enriched Preview text once (on open): the mod add/remove
    /// summary, the per-plugin load-order delta, and per-file overwrite
    /// conflicts (which mod wins each shared path, MO2-style). Kept out of the
    /// per-frame render path because the conflict scan touches disk.
    fn compute_preview_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Preview of your Setup vs what is Installed. Nothing changes until you Apply."
                .to_owned(),
        ];
        let realized = self
            .active_repo()
            .and_then(|r| Realized::load(&r).ok())
            .unwrap_or_default();
        let mut per_mod: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for rec in realized.files.values() {
            *per_mod.entry(rec.mod_name.clone()).or_default() += 1;
        }
        let enabled: Vec<&str> = self.manifest.as_ref().map_or_else(Vec::new, |m| {
            m.mods
                .iter()
                .filter(|md| md.enabled)
                .map(|md| md.name.as_str())
                .collect()
        });
        let adds: Vec<&str> = enabled
            .iter()
            .copied()
            .filter(|n| !per_mod.contains_key(*n))
            .collect();
        let removes: Vec<&str> = per_mod
            .keys()
            .filter(|n| !enabled.contains(&n.as_str()))
            .map(String::as_str)
            .collect();
        lines.push(format!("+ deploy ({})", adds.len()));
        lines.extend(adds.iter().map(|a| format!("   + {a}")));
        lines.push(format!("- remove ({})", removes.len()));
        lines.extend(removes.iter().map(|r| format!("   - {r}")));

        let lex = self.active_lexicon();
        let order = self.plan.as_ref().map_or_else(Vec::new, load_order);
        let realised_order = self.plan.as_ref().map_or_else(Vec::new, deployed_order);
        if order == realised_order {
            lines.push(format!("{} unchanged", lex.order));
        } else {
            lines.push(format!("~ {} will change:", lex.order));
            lines.extend(order_delta_lines(&realised_order, &order));
        }

        // Per-file overwrites — which mod wins each shared path (MO2-style: the
        // last mod in load order wins). Only the real (non-benign) overwrites
        // are worth surfacing; identical bytes are a no-op.
        if let (Some(repo), Some(plan)) = (self.active_repo(), self.plan.as_ref()) {
            if let Ok(conflicts) = concierge_pluginorder::assets::asset_conflicts(&repo, plan) {
                let real: Vec<_> = conflicts.iter().filter(|c| !c.benign).collect();
                if !real.is_empty() {
                    lines.push(format!(
                        "~ {} file overwrite(s) — the later mod wins:",
                        real.len()
                    ));
                    for c in real.iter().take(PREVIEW_CONFLICT_CAP) {
                        let losers: Vec<&str> = c
                            .providers
                            .iter()
                            .filter(|p| **p != c.winner)
                            .map(String::as_str)
                            .collect();
                        lines.push(format!(
                            "   ~ {} — {} wins over {}",
                            c.path,
                            c.winner,
                            losers.join(", ")
                        ));
                    }
                    if real.len() > PREVIEW_CONFLICT_CAP {
                        lines.push(format!(
                            "   … and {} more",
                            real.len() - PREVIEW_CONFLICT_CAP
                        ));
                    }
                }
            }
        }

        if adds.is_empty() && removes.is_empty() && order == realised_order {
            lines.push("No pending changes — Installed matches your Setup.".to_owned());
        }
        lines
    }

    /// This frame's [`concierge_ui::Screen`] — the single source of truth the
    /// action bar renders from and the headless view mirrors.
    fn screen(&self) -> concierge_ui::Screen {
        concierge_ui::build_screen(&self.ui_facts())
    }

    /// Dispatch a widget intent (by its stable `concierge-ui` id) to the concrete
    /// behaviour. Every projected widget renders FROM the screen and calls this —
    /// so App mutates ONLY through here, never from a hand-coded widget.
    fn dispatch_intent(&mut self, id: &str) {
        match id {
            // action bar
            "download" => self.run_action("download", Action::Fetch),
            "apply" => self.run_action("apply", Action::Realize),
            "verify" => self.run_action("verify", Action::Check),
            "sort_load" => self.run_action("sort order", Action::SortLoot),
            "requirements" => self.run_action("requirements", Action::ResolveDeps),
            "find_patches" => self.run_action("find patches", Action::SuggestPatches),
            "merge_conflicts" => self.run_action("merge conflicts", Action::Reconcile),
            "conflicts" => self.run_action("conflicts", Action::Conflicts),
            "play" => self.run_action("play", Action::Launch),
            "uninstall" => self.confirm = Some(Confirm::Undeploy),
            // chrome
            // Locked is a fact ON DISK (manifest read-only + immutable flag),
            // not a GUI mood: toggle it there, then render from it.
            "toggle_lock" => {
                if let Some(repo) = self.active_repo() {
                    let want_locked = self.mutable; // toggling away from mutable
                    if let Err(e) = profiles::set_locked(&repo.profile, want_locked) {
                        let _ = self.log_tx.send(format!("lock toggle failed: {e}"));
                    }
                    self.mutable = !profiles::is_locked(&repo.profile);
                } else {
                    self.mutable = !self.mutable;
                }
            }
            "tab_setup" => self.central_tab = CentralTab::Declaration,
            "tab_installed" => self.central_tab = CentralTab::Realised,
            "open_settings" => self.settings_open = true,
            "close_settings" => self.settings_open = false,
            "check_account" => self.check_account(),
            "open_browse" => {
                self.browse_open = true;
                // Open pre-populated: everything, most-endorsed first.
                if self.browse_hits.is_empty() && !self.browse_busy {
                    self.do_browse_search();
                }
            }
            "close_browse" => self.browse_open = false,
            "browse_search" => self.do_browse_search(),
            "nxm_add" => {
                let url = std::mem::take(&mut self.nxm_input);
                self.handle_nxm(&url);
            }
            _ if id.starts_with("add_hit:") => {
                if let Ok(mod_id) = id.trim_start_matches("add_hit:").parse::<u64>() {
                    let name = self
                        .browse_hits
                        .iter()
                        .find(|h| h.mod_id == mod_id)
                        .map(|h| h.name.clone())
                        .unwrap_or_default();
                    self.add_catalog_hit(mod_id, &name);
                }
            }
            "open_preview" => {
                self.diff_open = true;
                self.preview_lines = self.compute_preview_lines();
            }
            "close_preview" => self.diff_open = false,
            "undo" => self.undo_edit(),
            // confirm dialog
            "confirm_yes" => {
                if let Some(c) = self.confirm.take() {
                    self.execute_confirm(c);
                }
            }
            "confirm_no" => self.confirm = None,
            // per-mod enable toggle: "mod_toggle:<name>"
            _ if id.starts_with("mod_toggle:") => {
                let name = id.trim_start_matches("mod_toggle:");
                let cur = self
                    .manifest
                    .as_ref()
                    .and_then(|m| m.mods.iter().find(|md| md.name == name))
                    .is_some_and(|md| md.enabled);
                self.edit = Some(Edit::Toggle(name.to_owned(), !cur));
            }
            _ if id.starts_with("mod_select:") => {
                self.selected = Some(id.trim_start_matches("mod_select:").to_owned());
            }
            _ if id.starts_with("mod_up:") => {
                self.edit = Some(Edit::Nudge(
                    id.trim_start_matches("mod_up:").to_owned(),
                    true,
                ));
            }
            _ if id.starts_with("mod_down:") => {
                self.edit = Some(Edit::Nudge(
                    id.trim_start_matches("mod_down:").to_owned(),
                    false,
                ));
            }
            _ if id.starts_with("mod_remove:") => {
                self.confirm = Some(Confirm::RemoveMod(
                    id.trim_start_matches("mod_remove:").to_owned(),
                ));
            }
            _ if id.starts_with("mod_move:") => {
                let rest = id.trim_start_matches("mod_move:");
                if let Some((f, t)) = rest.split_once(':') {
                    if let (Ok(from), Ok(to)) = (f.parse::<usize>(), t.parse::<usize>()) {
                        self.edit = Some(Edit::Move(from, to));
                    }
                }
            }
            "add_open" => self.add.open = !self.add.open,
            "add_confirm" => self.submit_add(),
            _ if id.starts_with("rollback:") => {
                if let Ok(n) = id.trim_start_matches("rollback:").parse::<u64>() {
                    self.confirm = Some(Confirm::Rollback(n));
                }
            }
            _ if id.starts_with("restore_save:") => {
                self.confirm = Some(Confirm::RestoreSave(
                    id.trim_start_matches("restore_save:").to_owned(),
                ));
            }
            "ai_interrupt" => {
                if let Some(t) = &self.term {
                    t.kill();
                }
            }
            "ai_work" => self.work_on_profile(),
            "rescan" => self.discover(),
            "delete_profile" => {
                if let Some(p) = self.profiles.get(self.profile_idx) {
                    self.confirm = Some(Confirm::DeleteProfile(p.name.clone()));
                }
            }
            "create_empty" => self.create_profile(false),
            "create_clone" => self.create_profile(true),
            "new_modpack_ai" => self.new_modpack_concierge(),
            _ if id.starts_with("add_game:") => {
                let kind = id.trim_start_matches("add_game:");
                if let Some(ws) = &self.workspace {
                    match profiles::create_game(ws, kind) {
                        Ok(_) => {
                            self.discover();
                            // Clear positive feedback — testers weren't sure the
                            // add worked and saw the stale startup "(0 games)" log
                            // line (i2c1, i5c1, i4c1).
                            self.notice = Some(format!(
                                "Added {} — {} game(s) in your workspace.",
                                concierge_games::display_name(kind),
                                self.games.len()
                            ));
                            // Focus the newly added game.
                            if let Some(i) = self.games.iter().position(|g| g.game == kind) {
                                self.game_idx = i;
                                self.profile_idx = 0;
                                self.reload_profiles();
                            }
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            }
            _ if id.starts_with("select_game:") => {
                if let Ok(i) = id.trim_start_matches("select_game:").parse::<usize>() {
                    if i != self.game_idx && i < self.games.len() {
                        self.game_idx = i;
                        self.profile_idx = 0;
                        self.selected = None;
                        self.reload_profiles();
                    }
                }
            }
            _ if id.starts_with("select_profile:") => {
                if let Ok(i) = id.trim_start_matches("select_profile:").parse::<usize>() {
                    if i != self.profile_idx && i < self.profiles.len() {
                        self.profile_idx = i;
                        self.selected = None;
                        self.reload_plan();
                    }
                }
            }
            _ => {}
        }
    }

    /// Render one transition as an egui button that dispatches its intent — the
    /// single projection primitive reused by every region (action bar, confirm,
    /// chrome). Styling (fill/hover) is the renderer's business; the label,
    /// enabled state, and behaviour all come from the [`concierge_ui::Transition`].
    fn transition_button(&mut self, ui: &mut eframe::egui::Ui, tr: &concierge_ui::Transition) {
        use concierge_ui::WidgetKind;
        use eframe::egui;
        let mut resp = match tr.kind {
            // Tabs and toggles show selection (selectable_label); a button doesn't.
            WidgetKind::Tab | WidgetKind::Toggle => {
                ui.selectable_label(tr.selected, tr.label.as_str())
            }
            WidgetKind::Button => {
                let mut btn = egui::Button::new(tr.label.as_str());
                if tr.id == "confirm_yes" {
                    btn = btn.fill(egui::Color32::from_rgb(170, 70, 70));
                }
                ui.add_enabled(tr.enabled, btn)
            }
        };
        if let Some(h) = &tr.hover {
            resp = resp.on_hover_text(h.as_str());
        }
        // A disabled button must say WHY. egui suppresses on_hover_text on a
        // disabled widget, so the guard (the reason it's blocked) was invisible
        // exactly when it mattered — a greyed button read as a silent dead end.
        // Route the guard to the disabled-hover path when disabled.
        if let Some(g) = &tr.guard {
            resp = if tr.enabled {
                resp.on_hover_text(g.as_str())
            } else {
                resp.on_disabled_hover_text(g.as_str())
            };
        } else if !tr.enabled {
            resp = resp.on_disabled_hover_text("unavailable right now");
        }
        if resp.clicked() {
            self.dispatch_intent(&tr.id);
        }
    }

    /// Render a per-row button (dynamic id) via the projection primitive — for
    /// grid cells whose enabled state is position-dependent (mod ↑/↓/remove).
    fn row_btn(
        &mut self,
        ui: &mut eframe::egui::Ui,
        id: &str,
        label: &str,
        hover: &str,
        enabled: bool,
    ) {
        let tr = concierge_ui::Transition {
            id: id.to_owned(),
            label: label.to_owned(),
            kind: concierge_ui::WidgetKind::Button,
            selected: false,
            enabled,
            guard: None,
            // Icon-only controls need a name on hover — the glyphs even render as
            // empty squares in some fonts (i3c5, i4c5).
            hover: Some(hover.to_owned()),
            target: None,
        };
        self.transition_button(ui, &tr);
    }

    /// Render every transition of a given widget kind from this frame's screen —
    /// the projection loop reused by the tab bar, etc.
    fn render_kind(&mut self, ui: &mut eframe::egui::Ui, kind: concierge_ui::WidgetKind) {
        let items: Vec<concierge_ui::Transition> = self
            .screen()
            .transitions
            .into_iter()
            .filter(|t| t.kind == kind)
            .collect();
        for tr in &items {
            self.transition_button(ui, tr);
        }
    }

    /// Render a text field from the screen by id, dispatching edits back through
    /// `dispatch_type` — so text input is projected + headless-drivable too.
    /// Returns whether Enter was pressed (for submit-on-Enter fields).
    fn render_field(&mut self, ui: &mut eframe::egui::Ui, id: &str) -> bool {
        use eframe::egui;
        let Some(field) = self.screen().fields.into_iter().find(|f| f.id == id) else {
            return false;
        };
        let mut val = field.value;
        let resp = ui.add(egui::TextEdit::singleline(&mut val).hint_text(field.label));
        if resp.changed() {
            self.dispatch_type(id, &val);
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !val.is_empty() && ui.small_button("x").clicked() {
            self.dispatch_type(id, "");
        }
        enter
    }

    /// Apply a text-field edit (the `type <field> <val>` intent) to app state.
    fn dispatch_type(&mut self, id: &str, value: &str) {
        match id {
            "search" => value.clone_into(&mut self.search),
            "nxm_input" => value.clone_into(&mut self.nxm_input),
            "browse_query" => value.clone_into(&mut self.browse_query),
            "new_profile" => value.clone_into(&mut self.new_profile_name),
            "add_name" => value.clone_into(&mut self.add.name),
            "add_version" => value.clone_into(&mut self.add.version),
            "add_nexus_mod" => value.clone_into(&mut self.add.nexus_mod),
            "add_nexus_file" => value.clone_into(&mut self.add.nexus_file),
            "add_url" => value.clone_into(&mut self.add.url),
            "add_md5" => value.clone_into(&mut self.add.md5),
            "add_file" => value.clone_into(&mut self.add.file),
            "add_plugins" => value.clone_into(&mut self.add.plugins),
            _ => {}
        }
    }

    fn reload_plan(&mut self) {
        self.error = None;
        self.plan = None;
        self.manifest = None;
        self.lint = Vec::new();
        self.rel_issues = Vec::new();
        let Some(repo) = self.active_repo() else {
            return;
        };
        self.cache = profiles::cache_stats(&repo);
        match Manifest::load(&repo.profile) {
            Ok(m) => {
                self.manifest = Some(m.clone());
                self.rel_issues = m.relation_issues();
                match eval(&m) {
                    Ok(plan) => {
                        self.lint = concierge_lint::validate(&plan).unwrap_or_default();
                        self.plan = Some(plan);
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Apply a collected edit: read the manifest, transform it, write it back,
    /// re-eval. Errors surface in the error banner.
    fn apply(&mut self, edit: Edit) {
        let Some(path) = self.manifest_path() else {
            self.error = Some("no manifest to edit".to_owned());
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("read manifest: {e}"));
                return;
            }
        };
        let edited = match edit {
            Edit::Toggle(name, en) => manifest_edit::set_mod_enabled(&text, &name, en),
            Edit::Nudge(name, up) => manifest_edit::nudge_mod(&text, &name, up),
            Edit::Move(from, to) => manifest_edit::move_mod(&text, from, to),
            Edit::Remove(name) => manifest_edit::remove_mod(&text, &name),
            Edit::Add(m) => manifest_edit::add_mod(&text, &m),
            Edit::SetPristine(p) => manifest_edit::set_pristine(&text, p.trim()),
            Edit::SetDlc(dlc) => manifest_edit::set_dlc(&text, &dlc),
            Edit::SetCompat(gv, loader) => {
                manifest_edit::set_compat(&text, gv.trim(), loader.trim())
            }
        };
        match edited {
            Ok(new_text) => match concierge::manifest_edit::write_manifest(&path, &new_text) {
                Ok(()) => {
                    // snapshot the pre-edit manifest so the edit can be undone
                    self.undo.push(text);
                    if self.undo.len() > 30 {
                        self.undo.remove(0);
                    }
                    self.reload_plan();
                }
                Err(e) => self.error = Some(format!("write manifest: {e}")),
            },
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Undo the last declaration edit by restoring the previous manifest.
    fn undo_edit(&mut self) {
        let Some(prev) = self.undo.pop() else {
            return;
        };
        let Some(path) = self.manifest_path() else {
            return;
        };
        match concierge::manifest_edit::write_manifest(&path, &prev) {
            Ok(()) => self.reload_plan(),
            Err(e) => self.error = Some(format!("undo: {e}")),
        }
    }

    fn submit_add(&mut self) {
        let a = &self.add;
        let name = a.name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let source = if let Ok(mod_id) = a.nexus_mod.trim().parse::<u32>() {
            NewSource::Nexus {
                mod_id,
                file_id: a.nexus_file.trim().parse::<u32>().unwrap_or(0),
            }
        } else {
            NewSource::Url(a.url.trim().to_owned())
        };
        let plugins = a
            .plugins
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let new = NewMod {
            name,
            version: a.version.trim().to_owned(),
            source,
            md5: a.md5.trim().to_owned(),
            file: a.file.trim().to_owned(),
            install_root: "data".to_owned(),
            plugins,
        };
        self.add = AddForm::default();
        self.apply(Edit::Add(new));
    }

    fn run_action(&self, name: &'static str, action: Action) {
        if let Ok(mut a) = LAST_ACTION.lock() {
            name.clone_into(&mut a);
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let (Some(repo), Some(plan)) = (self.active_repo(), self.plan.clone()) else {
            self.busy.store(false, Ordering::SeqCst);
            return;
        };
        let tx = self.log_tx.clone();
        let flash = self.flash_tx.clone();
        let busy = Arc::clone(&self.busy);
        let reload_pending = Arc::clone(&self.reload_pending);
        let mutates = action.mutates_manifest();
        std::thread::spawn(move || {
            let _ = tx.send(format!("> {name}"));
            match run_blocking(&repo, &plan, action, &tx) {
                Ok(lines) => {
                    // A "⏸ … still needed" line means the action did NOT finish the
                    // job — e.g. Download only opened the mod's web page and a manual
                    // save is still required. Surface that as amber, not a green
                    // "finished", so the outcome can't masquerade as complete.
                    let manual = lines
                        .iter()
                        .find(|l| l.contains('⏸') || l.contains("still needed"))
                        .cloned();
                    // A "⚠ …" line (e.g. conflicts found) is a real outcome to
                    // surface amber in the banner, not bury in the log.
                    let warned = lines.iter().find(|l| l.contains('⚠')).cloned();
                    let placed = lines
                        .iter()
                        .rev()
                        .find(|l| l.contains("placed") || l.contains('✅'))
                        .cloned();
                    for l in lines {
                        let _ = tx.send(l);
                    }
                    if mutates {
                        reload_pending.store(true, Ordering::SeqCst);
                    }
                    let _ = tx.send(format!("{name} done"));
                    let (kind, summary) = if let Some(m) = manual {
                        (FlashKind::Warn, m)
                    } else if let Some(w) = warned {
                        (FlashKind::Warn, w)
                    } else if let Some(p) = placed {
                        (FlashKind::Ok, p)
                    } else {
                        (FlashKind::Ok, format!("{name} finished"))
                    };
                    let _ = flash.send((kind, summary));
                }
                Err(e) => {
                    let msg = e.to_string();
                    let _ = tx.send(format!("{name}: {msg}"));
                    let _ = flash.send((FlashKind::Err, format!("{name} failed — {msg}")));
                }
            }
            busy.store(false, Ordering::SeqCst);
        });
    }

    fn create_profile(&mut self, clone_active: bool) {
        let Some(game) = self.games.get(self.game_idx) else {
            return;
        };
        let clone_from = if clone_active {
            self.profiles.get(self.profile_idx).map(|p| p.dir.clone())
        } else {
            None
        };
        let name = self.new_profile_name.trim().to_owned();
        match profiles::create_profile(&game.dir, &name, clone_from.as_deref()) {
            Ok(_) => {
                self.new_profile_name.clear();
                let target = self.profiles.len();
                self.reload_profiles();
                self.profile_idx = target.min(self.profiles.len().saturating_sub(1));
                self.reload_plan();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

/// The enabled plugin load order the game will read (DLC + mod plugins), pulled
/// from the plan's generated `plugins.txt` config.
/// Cap on how many per-file overwrite conflicts the Preview lists (the rest are
/// summarised as "… and N more") so a heavy modlist can't flood the window.
const PREVIEW_CONFLICT_CAP: usize = 12;

/// Per-plugin load-order delta between the deployed order and the planned order
/// — the plugins added to or dropped from the order, plus a note when the
/// surviving plugins are resequenced. This is the "what actually moves on Apply"
/// detail behind the one-line "load order will change".
fn order_delta_lines(deployed: &[String], planned: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in planned.iter().filter(|p| !deployed.iter().any(|d| d == *p)) {
        out.push(format!("   + {a}"));
    }
    for r in deployed.iter().filter(|d| !planned.iter().any(|p| p == *d)) {
        out.push(format!("   - {r}"));
    }
    let common_planned: Vec<&String> = planned
        .iter()
        .filter(|p| deployed.iter().any(|d| d == *p))
        .collect();
    let common_deployed: Vec<&String> = deployed
        .iter()
        .filter(|d| planned.iter().any(|p| p == *d))
        .collect();
    if common_planned != common_deployed {
        out.push("   ~ existing plugins resequenced".to_owned());
    }
    out
}

fn load_order(plan: &Plan) -> Vec<String> {
    // Identify the plugins.txt config by filename OR (when the path is a blank
    // placeholder, e.g. a fresh profile) by content — the load-order config is
    // the one with `*`-prefixed activation lines.
    plan.configs
        .iter()
        .find(|c| {
            std::path::Path::new(&c.path)
                .file_name()
                .is_some_and(|n| n == "plugins.txt")
                || c.content.lines().any(|l| l.starts_with('*'))
        })
        .map(|c| {
            c.content
                .lines()
                .filter_map(|l| l.strip_prefix('*'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The plugin order actually written to the deployed `plugins.txt` on disk (the
/// realised order), as opposed to `load_order`'s declared/planned order.
fn deployed_order(plan: &Plan) -> Vec<String> {
    plan.configs
        .iter()
        .find(|c| {
            std::path::Path::new(&c.path)
                .file_name()
                .is_some_and(|n| n == "plugins.txt")
        })
        .and_then(|c| std::fs::read_to_string(&c.path).ok())
        .map(|s| {
            s.lines()
                .filter_map(|l| l.strip_prefix('*'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Emit a named rebuild stage to the live log (so Realize shows progress, not
/// just a spinner — Vortex's "manifest → sort → merge → deploy" idea).
fn deploy_stage(tx: &Sender<String>, label: &str) {
    let _ = tx.send(format!("  ▸ {label}"));
}

/// Append a Nexus `[[mod]]` to the manifest from a worker thread, atomically
/// (write-temp + rename), and flag the UI to reload. Used by the off-thread
/// Browse-add and nxm-download paths so neither blocks the egui thread.
fn write_added_mod(repo: &Repo, entry: &NewMod, tx: &Sender<String>, reload: &Arc<AtomicBool>) {
    // Atomic read-modify-write under the core write lock — concurrent adds
    // queue and all complete (none silently clobbered).
    let path = repo.profile.join("manifest.toml");
    match concierge::manifest_edit::add_mod_to_file(&path, entry) {
        Ok(()) => {
            let _ = tx.send(format!("added '{}'", entry.name));
            reload.store(true, Ordering::SeqCst);
        }
        Err(e) => {
            let _ = tx.send(format!("add {}: {e}", entry.name));
        }
    }
}

/// Build a Nexus `[[mod]]` entry for the off-thread add paths.
fn nexus_new_mod(
    name: String,
    mod_id: u64,
    file_id: u64,
    md5: String,
    file: String,
    version: String,
) -> NewMod {
    NewMod {
        name,
        version: if version.is_empty() {
            "1".to_owned()
        } else {
            version
        },
        source: NewSource::Nexus {
            mod_id: u32::try_from(mod_id).unwrap_or(0),
            file_id: u32::try_from(file_id).unwrap_or(0),
        },
        md5,
        file,
        install_root: "data".to_owned(),
        plugins: Vec::new(),
    }
}

/// A URL-source mod (e.g. a resolved Modrinth CDN file). Unpinned — the url
/// download computes + pins the md5 on first fetch. `install_root` "data" is
/// omitted, so the game's default root applies (e.g. `mods/` for Minecraft).
fn url_new_mod(name: String, url: String, file: String, version: String) -> NewMod {
    NewMod {
        name,
        version: if version.is_empty() {
            "1".to_owned()
        } else {
            version
        },
        source: NewSource::Url(url),
        md5: String::new(),
        file,
        install_root: "data".to_owned(),
        plugins: Vec::new(),
    }
}

fn run_blocking(
    repo: &Repo,
    plan: &Plan,
    action: Action,
    tx: &Sender<String>,
) -> concierge::Result<Vec<String>> {
    match action {
        Action::Realize => {
            let mut out = Vec::new();
            // The most common Apply blocker in testing: no game install folder
            // set. Name the exact fix up front instead of failing opaquely deep
            // in deploy ("not realized" / raw path error).
            if plan.game.pristine.trim().is_empty() {
                return Err(concierge::Error::Other(
                    "No game install folder set — open Settings and set \"Game install \
                     folder\" to where your game is installed, then Apply. Concierge \
                     installs into a copy; your original is never touched."
                        .into(),
                ));
            }
            if !std::path::Path::new(&plan.game.pristine).exists() {
                return Err(concierge::Error::Other(format!(
                    "Game install folder not found: {} — fix it in Settings \u{2192} \
                     Game install folder, then Apply.",
                    plan.game.pristine
                )));
            }
            deploy_stage(tx, "back up saves");
            if let Some(b) = concierge::saves::backup(repo, plan)? {
                out.push(format!(
                    "backed up {} save file(s) -> generation {}",
                    b.files, b.generation
                ));
            }
            // Converge: fetch → auto-pin md5 → build → auto-resolve layout
            // (strip versioned roots, activate detected plugins) → deploy,
            // re-evaluating after each manifest edit. Replaces the old
            // fetch/build/realize that dead-ended on the first unpinned mod.
            deploy_stage(tx, "fetch → pin → build → resolve → deploy");
            let converge = concierge::realize::realize_converged(repo, false)?;
            for line in &converge.resolved {
                out.push(format!("resolved: {line}"));
            }
            let Some(r) = converge.report else {
                // Couldn't complete — run a preflight so EVERY unresolved item
                // is named, not just the first failure.
                let plan = concierge::manifest::Manifest::load(&repo.profile)
                    .and_then(|m| eval(&m))
                    .ok();
                for b in &converge.blocked {
                    out.push(format!("⏸ download needed — {b}"));
                }
                if let Some(p) = &plan {
                    for issue in concierge::realize::preflight(repo, p) {
                        out.push(format!(
                            "unresolved [{}]: {} — {}",
                            issue.kind.label(),
                            issue.mod_name,
                            issue.detail
                        ));
                    }
                }
                // Name the concrete next step, not "fix the items above".
                let dl = converge.blocked.len();
                let msg = if dl > 0 {
                    format!(
                        "Apply blocked: {dl} mod(s) still need downloading — click \
                         Download (or drop the files into ~/Downloads), then Apply again."
                    )
                } else {
                    "Apply blocked: resolve the unpinned/unbuilt items listed above \
                     (usually Download, then Apply again)."
                        .to_owned()
                };
                return Err(concierge::Error::Other(msg));
            };
            out.push(format!(
                "placed {} files ({} owned)",
                r.placed, r.total_owned
            ));
            // Post-deploy reporting runs against the CONVERGED plan.
            let manifest = concierge::manifest::Manifest::load(&repo.profile)?;
            let plan = &eval(&manifest)?;
            // Nix-style: record this declaration as an immutable generation.
            if let Ok(text) = std::fs::read_to_string(repo.profile.join("manifest.toml")) {
                let hash = plan.hash().unwrap_or_default();
                if let Ok(g) = concierge::generations::snapshot(repo, &text, &hash) {
                    out.push(format!("recorded generation {}", g.number));
                }
            }
            // Write the resolved-state lock (reproducible pin of this profile).
            if let Ok(lock) = concierge::lockfile::write(repo, plan) {
                out.push(format!("locked {} mods -> concierge.lock", lock.mods.len()));
            }
            deploy_stage(tx, "lint");
            // surface invariant lints post-deploy, like the CLI
            let issues = concierge_lint::validate(plan).unwrap_or_default();
            let (errors, warnings) = concierge_lint::partition(issues);
            for w in &warnings {
                out.push(format!("warn {} [{}]: {}", w.subject, w.rule, w.detail));
            }
            // Declared-relationship issues (missing requires, active incompatibles,
            // out-of-window game version) — the dependency/conflict health signal.
            let rel_issues = concierge::manifest::Manifest::load(&repo.profile)
                .map(|m| m.relation_issues())
                .unwrap_or_default();
            for r in &rel_issues {
                out.push(format!("relation: {r}"));
            }
            // Deterministic dependency check: plugins whose masters aren't present.
            let missing_masters = if is_bethesda(&plan.game.kind) {
                concierge_pluginorder::missing_masters(plan).unwrap_or_default()
            } else {
                Vec::new()
            };
            for mm in &missing_masters {
                out.push(format!(
                    "dependency: '{}' missing master(s): {}",
                    mm.plugin,
                    mm.missing.join(", ")
                ));
            }
            if errors.is_empty() {
                out.push(format!(
                    "✅ health check: GO — {} plugin(s), {} warning(s), {} relation issue(s), {} missing-master dep(s)",
                    plan.mods.len(),
                    warnings.len(),
                    rel_issues.len(),
                    missing_masters.len()
                ));
                Ok(out)
            } else {
                for e in &errors {
                    out.push(format!("ERROR {} [{}]: {}", e.subject, e.rule, e.detail));
                }
                Err(concierge::Error::Other(format!(
                    "❌ health check: NO-GO — {} invariant violation(s); fix and re-realize",
                    errors.len()
                )))
            }
        }
        Action::Fetch => {
            use concierge::store::FetchOutcome as F;
            let manifest_path = repo.profile.join("manifest.toml");
            let results = concierge::store::fetch_all(repo, plan)?;
            let total = results.len();
            let (mut cached, mut downloaded) = (0usize, 0usize);
            let mut needed: Vec<String> = Vec::new();
            let mut pins: Vec<String> = Vec::new();
            for (name, outcome) in results {
                match outcome {
                    F::Present(_) => cached += 1,
                    F::Stored(_) => downloaded += 1,
                    F::NeedsPin { md5, .. } => {
                        downloaded += 1;
                        // Write the computed md5 straight into manifest.toml —
                        // no more "pin it yourself" dead-end.
                        let doc = std::fs::read_to_string(&manifest_path)
                            .map_err(|e| concierge::Error::Other(e.to_string()))?;
                        match concierge::manifest_edit::pin_mod(&doc, &name, &md5, None, None, None)
                        {
                            Ok(updated) => {
                                concierge::manifest_edit::write_manifest(&manifest_path, &updated)?;
                                pins.push(format!("  pinned {name} -> md5 {md5}"));
                            }
                            Err(e) => pins.push(format!(
                                "  {name}: computed md5 {md5} but couldn't pin ({e})"
                            )),
                        }
                    }
                    F::Blocked { instructions } => {
                        needed.push(format!("  • {name}: {instructions}"));
                    }
                }
            }
            let present = total.saturating_sub(needed.len());
            let mut out = vec![format!(
                "download queue: {present}/{total} present  ({cached} cached, {downloaded} downloaded this run)"
            )];
            if needed.is_empty() {
                out.push("✅ all archives present + pinned — Apply can complete".to_owned());
            } else {
                out.push(format!(
                    "⏸ {} still needed — click 'Mod Manager Download' on each (one click each, free), or save to ~/Downloads, then re-run:",
                    needed.len()
                ));
                out.extend(needed);
            }
            out.extend(pins);
            Ok(out)
        }
        Action::Check => {
            let drift = concierge::check::check(repo, plan, false)?;
            let installed = concierge::state::Realized::load(repo)
                .ok()
                .is_some_and(|r| r.plan_hash.is_some());
            if drift.is_empty() {
                // "clean" alone read as "the pack is good" even when nothing was
                // ever installed (i1c6). Say what's clean, and the install state.
                let msg = if installed {
                    "Verify: clean — your installed setup matches and the original game \
                     files are untouched."
                } else {
                    "Verify: your original game files are untouched. Nothing is installed \
                     yet — Download, then Apply, to install your pack."
                };
                Ok(vec![msg.to_owned()])
            } else {
                let mut out = vec![format!(
                    "Verify: {} change(s) from the original game detected:",
                    drift.len()
                )];
                out.extend(drift.iter().map(|d| format!("  {d:?}")));
                Ok(out)
            }
        }
        Action::Launch => {
            // Translate the CLI-flavoured NoInstance ("run `concierge realize`")
            // into the GUI's own verb: Apply. i4c2 hit "Play fails by telling me
            // to run a CLI command".
            let info = concierge::launch::launch(plan).map_err(|e| {
                if matches!(e, concierge::Error::NoInstance) {
                    concierge::Error::Other(
                        "Nothing is installed yet — click Apply to install your setup into \
                         the game, then Play."
                            .to_owned(),
                    )
                } else {
                    e
                }
            })?;
            Ok(vec![format!("launched {} ({:?})", info.exe, info.runtime)])
        }
        Action::Undeploy => {
            let (removed, skipped) = concierge::realize::undeploy(repo, plan, false)?;
            Ok(vec![format!(
                "undeployed {removed} files ({skipped} skipped)"
            )])
        }
        Action::Conflicts => {
            let (parsed, matrix) = concierge_pluginorder::conflict_matrix(plan)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            let n = matrix.conflicts.len();
            // A self-explanatory summary as the FIRST line so it surfaces in the
            // outcome banner (amber when there are clashes), not just the log —
            // testers said "no clash was ever surfaced" (i2c6, i3c6).
            let head = if n == 0 {
                format!("✅ No conflicts — your {parsed} plugin(s) don't overwrite each other.")
            } else {
                format!(
                    "⚠ {n} conflicting record(s) across {parsed} plugins — the later mod wins \
                     (listed below). Use \"Merge conflicts\" to auto-resolve safely."
                )
            };
            let mut out = vec![head];
            for c in matrix.conflicts.iter().take(40) {
                let losers: Vec<&str> = c
                    .carriers
                    .iter()
                    .filter(|p| **p != c.winner)
                    .map(String::as_str)
                    .collect();
                out.push(format!(
                    "  {} {:08X}: {} wins over {}",
                    c.signature,
                    c.object_id,
                    c.winner,
                    losers.join(", ")
                ));
            }
            Ok(out)
        }
        Action::Reconcile => reconcile_and_deploy(repo, plan),
        Action::SortLoot => {
            let path = repo.profile.join("manifest.toml");
            let report = concierge_pluginorder::loadorder::sort(repo, plan)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            let text = std::fs::read_to_string(&path)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            let new = concierge::manifest_edit::set_load_order(&text, &report.suggested)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            concierge::manifest_edit::write_manifest(&path, &new)?;
            Ok(vec![format!(
                "LOOT: sorted {} plugins into [relations].load_order ({} dirty flagged)",
                report.suggested.len(),
                report.dirty.len()
            )])
        }
        Action::ResolveDeps => {
            let path = repo.profile.join("manifest.toml");
            let dep = concierge_pluginorder::resolve_dependencies(plan)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            let mut text = std::fs::read_to_string(&path)
                .map_err(|e| concierge::Error::Other(e.to_string()))?;
            let mut added = 0;
            for (m, needs) in &dep.requires {
                let new = concierge::manifest_edit::add_requires(&text, m, needs)
                    .map_err(|e| concierge::Error::Other(e.to_string()))?;
                if new != text {
                    added += 1;
                }
                text = new;
            }
            concierge::manifest_edit::write_manifest(&path, &text)?;
            let mut out = vec![format!(
                "dependencies: recorded {added} new requires fact(s); {} unresolved missing master(s)",
                dep.missing.len()
            )];
            for (m, mast) in dep.missing.iter().take(20) {
                out.push(format!(
                    "  ⚠ '{m}' needs master '{mast}' — no mod in the pack provides it"
                ));
            }
            Ok(out)
        }
        Action::SuggestPatches => {
            let game = plan.game.nexus_domain.clone().unwrap_or_default();
            let filter = concierge::manifest::Manifest::load(&repo.profile)
                .map(|m| CatalogFilter::from_curate(&m.curate))
                .unwrap_or_default();
            let mut pairs: std::collections::BTreeSet<(String, String)> =
                std::collections::BTreeSet::new();
            let norm = |mut a: String, mut b: String| {
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                (a, b)
            };
            if let Ok(land) = concierge_ai::tools::conflict_landscape(repo, plan) {
                for c in &land.asset_conflicts {
                    for (i, a) in c.providers.iter().enumerate() {
                        for b in c.providers.iter().skip(i + 1) {
                            pairs.insert(norm(a.clone(), b.clone()));
                        }
                    }
                }
            }
            if let Ok(m) = concierge::manifest::Manifest::load(&repo.profile) {
                for inc in &m.relations.incompatible {
                    pairs.insert(norm(inc.a.clone(), inc.b.clone()));
                }
            }
            if pairs.is_empty() {
                return Ok(vec![
                    "no conflicting mod pairs — no patches needed".to_owned()
                ]);
            }
            let mut out = Vec::new();
            let mut found = 0;
            for (a, b) in pairs.iter().take(15) {
                let query = if a.len() <= b.len() { a } else { b };
                let Ok(hits) = catalog_search(repo, &game, query, 6, &filter) else {
                    continue;
                };
                for h in hits {
                    let nl = h.name.to_lowercase();
                    if nl.contains("patch") || nl.contains("compat") {
                        found += 1;
                        out.push(format!(
                            "  patch for '{a}' + '{b}': {} (mod {})",
                            h.name, h.mod_id
                        ));
                        break;
                    }
                }
            }
            out.push(if found == 0 {
                format!(
                    "checked {} conflicting pair(s); no catalog patch matched",
                    pairs.len().min(15)
                )
            } else {
                format!("suggested {found} compat patch(es) — add via Browse")
            });
            Ok(out)
        }
    }
}

fn reconcile_and_deploy(repo: &Repo, plan: &Plan) -> concierge::Result<Vec<String>> {
    // Merge conflicts reconciles a plugin load order in the deployed game — it
    // only makes sense for plugin-based games with a set-up install. Without
    // these guards a fresh/non-plugin profile crashed with a raw filesystem
    // error (i1c6). Name the precondition instead.
    if !is_bethesda(&plan.game.kind) {
        return Err(concierge::Error::Other(format!(
            "Merge conflicts only applies to plugin-based games (Skyrim, Fallout 4) — \
             {} doesn't use a plugin load order.",
            plan.game.kind
        )));
    }
    if plan.game.pristine.trim().is_empty() {
        return Err(concierge::Error::Other(
            "Set the game install folder in Settings, then Download + Apply your mods \
             first — Merge conflicts works on an installed plugin load order."
                .into(),
        ));
    }
    concierge::saves::backup(repo, plan)?;
    concierge::store::fetch_all(repo, plan)?;
    concierge::build::build_all(repo, plan)?;
    concierge::realize::realize(repo, plan, false)?;
    let report = concierge_pluginorder::reconcile::reconcile(repo, plan)
        .map_err(|e| concierge::Error::Other(e.to_string()))?;
    let mut out = vec![
        format!(
            "{} plugins: {} conflicts, {} danger-class (left alone)",
            report.plugins, report.conflicts, report.danger
        ),
        format!(
            "merged {} leveled + {} form-lists + {} UDR fixes -> {} resolver records ({} masters)",
            report.leveled_merged,
            report.formlist_merged,
            report.udr_fixed,
            report.resolver_records,
            report.masters.len()
        ),
    ];
    let resolver_name = "ConciergeResolver.esp";
    let data = std::path::PathBuf::from(plan.game_dir()).join("Data");
    std::fs::create_dir_all(&data).map_err(|e| concierge::Error::Other(e.to_string()))?;
    std::fs::copy(&report.resolver, data.join(resolver_name))
        .map_err(|e| concierge::Error::Other(e.to_string()))?;
    for cfg in &plan.configs {
        let path = std::path::PathBuf::from(&cfg.path);
        if path.file_name().is_some_and(|n| n == "plugins.txt") {
            let mut content = std::fs::read_to_string(&path).unwrap_or_default();
            if !content.contains(resolver_name) {
                if !content.ends_with('\n') {
                    content.push('\n');
                }
                content.push('*');
                content.push_str(resolver_name);
                content.push('\n');
                std::fs::write(&path, content)
                    .map_err(|e| concierge::Error::Other(e.to_string()))?;
            }
        }
    }
    out.push(format!(
        "deployed {resolver_name}, activated last in plugins.txt"
    ));
    Ok(out)
}

/// Render one AI transcript line with light chat styling: user prompts
/// accented, stderr/status dimmed, everything else monospace (wraps to column).
/// Clamp a float to a `u16` window without a lint-flagged `as` cast on the
/// full float domain — the value is already bounded to `[lo, hi]` integers.
fn f32_to_u16(v: f32, lo: u16, hi: u16) -> u16 {
    if !v.is_finite() || v <= f32::from(lo) {
        return lo;
    }
    if v >= f32::from(hi) {
        return hi;
    }
    // v is now within [lo, hi] ⊂ u16 range; round to nearest.
    let r = v.round();
    (lo..=hi).find(|n| f32::from(*n) >= r).unwrap_or(hi)
}

/// Human "N ago" for a Unix-seconds timestamp (0 = never).
fn ago(epoch: u64) -> String {
    if epoch == 0 {
        return "never".to_owned();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let secs = now.saturating_sub(epoch);
    match secs {
        0..=3599 => format!("{}m ago", (secs / 60).max(1)),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Record an "ok" audit verdict for a browser-added mod (its id came from the
/// catalog, so it's verified by construction). Merges into state/audit.json —
/// the same record `concierge audit` writes and `eval`/`realize` read.
fn record_audit_ok(repo: &Repo, mod_id: u64, name: &str) {
    let path = repo.state_dir().join("audit.json");
    let mut obj = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    obj.insert(
        mod_id.to_string(),
        serde_json::json!({ "name": name, "verdict": "ok", "catalog_name": name }),
    );
    if std::fs::create_dir_all(repo.state_dir()).is_ok() {
        if let Ok(s) = serde_json::to_string_pretty(&serde_json::Value::Object(obj)) {
            let _ = std::fs::write(&path, s);
        }
    }
}

/// Is the `claude` CLI on PATH? Determines whether the agent terminal runs
/// the real agent or falls back to a plain shell.
/// Parse "<done>/<total>" out of a catalog-sync progress line.
fn parse_sync_counts(line: &str) -> Option<(u64, u64)> {
    line.split_whitespace().find_map(|tok| {
        let (done_str, total_str) = tok.split_once('/')?;
        let done = done_str.parse::<u64>().ok()?;
        let total = total_str.parse::<u64>().ok()?;
        (total > 0).then_some((done, total))
    })
}

/// Human sync progress from a raw line + elapsed seconds: `(fraction 0..=1,
/// label)`. `None` until the first "N/total" appears (the initial one-time
/// message), so the caller shows an indeterminate spinner then a real bar.
fn sync_bar(line: &str, elapsed_secs: u64) -> Option<(f32, String)> {
    let (done, total) = parse_sync_counts(line)?;
    let pct = done.saturating_mul(100) / total; // integer 0..=100, no float cast
    let frac = f32::from(u16::try_from(pct.min(100)).unwrap_or(100)) / 100.0;
    let eta = if done > 0 && done < total && elapsed_secs >= 2 {
        let remaining = elapsed_secs.saturating_mul(total - done) / done;
        format!(" · ~{} left", fmt_dur(remaining))
    } else {
        String::new()
    };
    Some((frac, format!("{done} / {total} mods · {pct}%{eta}")))
}

/// Compact duration for the ETA ("45 sec", "6 min", "1h 3m").
fn fmt_dur(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{} sec", secs.max(1))
    }
}

/// Is any supported AI coding agent on PATH? The sandboxed shell is agent-
/// agnostic — you run whichever of these you have.
fn which_agent_available() -> bool {
    ["opencode", "claude", "codex"]
        .iter()
        .any(|a| agent_on_path(a))
}

/// Whether `name` is a runnable CLI on PATH. On Windows, npm/scoop install these
/// as `.cmd`/`.ps1` shims that `Command::new(name)` (which only tries `.exe`)
/// never resolves — so go through `cmd /c` to honor PATHEXT, matching how the
/// sandbox shell itself finds them.
fn agent_on_path(name: &str) -> bool {
    #[cfg(windows)]
    let mut c = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", name, "--version"]);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = std::process::Command::new(name);
        c.arg("--version");
        c
    };
    c.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Translate this frame's egui keyboard/text events into the byte stream a
/// PTY expects. Covers text, Enter, Backspace, Tab, Esc, arrows,
/// and Ctrl-<letter> control codes.
fn keystrokes_to_bytes(ui: &eframe::egui::Ui, _ctx: &eframe::egui::Context) -> Vec<u8> {
    use eframe::egui::{Event, Key};
    let mut out = Vec::new();
    ui.input(|i| {
        for ev in &i.events {
            match ev {
                Event::Text(t) => out.extend_from_slice(t.as_bytes()),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.ctrl {
                        // Ctrl-A..Ctrl-Z -> 0x01..0x1a
                        if let Some(c) = key.name().bytes().next() {
                            let up = c.to_ascii_uppercase();
                            if up.is_ascii_uppercase() {
                                out.push(up - b'A' + 1);
                            }
                        }
                        continue;
                    }
                    match key {
                        Key::Enter => out.push(b'\r'),
                        Key::Backspace => out.push(0x7f),
                        Key::Tab => out.push(b'\t'),
                        Key::Escape => out.push(0x1b),
                        Key::ArrowUp => out.extend_from_slice(b"\x1b[A"),
                        Key::ArrowDown => out.extend_from_slice(b"\x1b[B"),
                        Key::ArrowRight => out.extend_from_slice(b"\x1b[C"),
                        Key::ArrowLeft => out.extend_from_slice(b"\x1b[D"),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    });
    out
}

/// `~/.config/concierge` — where the CLI keeps API keys.
fn config_dir() -> PathBuf {
    concierge_platform::config_dir()
}

/// Compact count for card stats: `9_000_000` becomes "9.0M", `12_000` "12.0k".
/// Integer-only (one decimal via /100 remainder) — no float cast.
fn human_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}.{}k", n / 1_000, (n % 1_000) / 100),
        _ => format!("{}.{}M", n / 1_000_000, (n % 1_000_000) / 100_000),
    }
}

/// A small pill/chip label (the category tag on a mod card).
fn chip(ui: &mut eframe::egui::Ui, text: &str) {
    use eframe::egui;
    egui::Frame::new()
        .fill(ui.visuals().widgets.inactive.bg_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).size(11.0)).selectable(false));
        });
}

fn human_bytes(n: u64) -> String {
    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    let f = n as f64;
    for (unit, size) in [("GB", 1e9), ("MB", 1e6), ("KB", 1e3)] {
        if f >= size {
            return format!("{:.1} {unit}", f / size);
        }
    }
    format!("{n} B")
}

impl eframe::App for App {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        self.update_ctx(ctx);
    }
}

impl App {
    /// The whole per-frame render, at `Context` level (eframe's `Frame` arg is
    /// unused). Split out so `egui_kittest` can drive the real render in a test
    /// without an `eframe::Frame`. (visual-testing dossier item 2)
    fn update_ctx(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        // nxm handoff: pin any links dropped in the inbox (browser download /
        // `concierge nxm <url>` reaching this running instance).
        for url in concierge::nexus::drain_nxm_inbox() {
            self.handle_nxm(&url);
        }
        let mut new_log = Vec::new();
        if self.sync_progress.lock().is_ok_and(|g| g.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        while let Ok((kind, msg)) = self.flash_rx.try_recv() {
            self.error = None;
            self.notice = None;
            self.warn = None;
            match kind {
                FlashKind::Ok => self.notice = Some(msg),
                FlashKind::Warn => {
                    // A Download that left mods still needing a manual fetch —
                    // pop the guided download session so the queue is right there.
                    if msg.contains("still needed") || msg.contains("Mod Manager Download") {
                        self.download_session_open = true;
                    }
                    self.warn = Some(msg);
                }
                FlashKind::Err => self.error = Some(msg),
            }
        }
        while let Ok(line) = self.log_rx.try_recv() {
            new_log.push(line);
        }
        if !new_log.is_empty() {
            append_action_log(&new_log); // durable: survives GUI exit
            self.log.extend(new_log);
        }
        // Off-thread catalog reads: apply only the latest (drop superseded).
        while let Ok((seq, res)) = self.browse_rx.try_recv() {
            if seq != self.browse_seq {
                continue;
            }
            self.browse_busy = false;
            self.browse_hits = res.hits;
            self.browse_msg = res.msg;
            self.browse_status = res.status;
            if let Some(cats) = res.categories {
                self.browse_categories = cats;
            }
        }
        // A finished catalog sync asks for a re-search (fresh counts + hits).
        if self
            .browse_refresh
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.browse_categories.clear();
            self.do_browse_search();
        }
        // An off-thread action edited the manifest — reload on the UI thread.
        if self
            .reload_pending
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.reload_plan();
        }
        // Repaint promptly while the agent terminal is producing output.
        if let Some(term) = &self.term {
            let tick = term.dirty_tick();
            if tick != self.term_epoch {
                self.term_epoch = tick;
                ctx.request_repaint();
            } else if !term.finished() {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        let busy = self.busy.load(Ordering::SeqCst);

        ctx.set_visuals(if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        self.top_panel(ctx);

        // Clone the data the central panel iterates so it can also mutate self.
        let plan = self.plan.clone();
        let mods: Vec<Mod> = self
            .manifest
            .as_ref()
            .map_or_else(Vec::new, |m| m.mods.clone());
        let order = plan.as_ref().map_or_else(Vec::new, load_order);
        let realised_order = plan.as_ref().map_or_else(Vec::new, deployed_order);
        let realized = self
            .active_repo()
            .and_then(|r| Realized::load(&r).ok())
            .unwrap_or_default();
        let mut per_mod: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for rec in realized.files.values() {
            *per_mod.entry(rec.mod_name.clone()).or_default() += 1;
        }
        let synced = self
            .plan
            .as_ref()
            .and_then(|p| p.hash().ok())
            .zip(realized.plan_hash.as_ref())
            .is_some_and(|(h, r)| &h == r);

        // AI assistant — a VSCode-style right-hand column (rightmost, collapsible).
        if self.ai_visible {
            self.ai_column(ctx);
        }

        egui::SidePanel::right("details")
            .default_width(240.0)
            .show(ctx, |ui| self.details_panel(ui, &mods, &per_mod));

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(140.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("log");
                    if ui.small_button("clear").clicked() {
                        self.log.clear();
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("log")
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(err) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
            }
            if let Some(n) = &self.notice {
                ui.colored_label(egui::Color32::from_rgb(90, 200, 120), n);
            }
            if let Some(w) = &self.warn {
                ui.colored_label(egui::Color32::from_rgb(220, 170, 90), w);
            }
            let Some(plan) = plan else {
                if self.games.is_empty() {
                    ui.heading("Welcome to Concierge");
                    ui.label(
                        "Add a game with the “+ add game” menu at the top, then create a \
                         profile to start building a modpack.",
                    );
                } else {
                    ui.label("Select a game and profile above, or create a new profile.");
                }
                if let Some(ws) = &self.workspace {
                    ui.add_space(4.0);
                    ui.weak(format!("workspace: {}", ws.display()));
                }
                ui.add_space(6.0);
                if ui.button("Open the quick-start guide").clicked() {
                    self.quickstart_open = true;
                }
                return;
            };
            // Onboarding: the one thing a fresh profile is missing is where the
            // game lives. Make it unmissable HERE (not just buried in Settings),
            // with one-click Steam detection — and setting it also detects which
            // DLC you own, so the load order matches your install.
            let pristine = plan.game.pristine.trim();
            let pristine_ok =
                !pristine.is_empty() && std::path::Path::new(pristine).exists();
            if !pristine_ok {
                ui.add_space(4.0);
                ui.group(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 180, 100),
                        "\u{26a0} Point Concierge at your game install",
                    );
                    ui.label(
                        "Concierge doesn't know where this game is installed yet. Set that \
                         and it detects which DLC you own. Your original install is never \
                         modified — mods deploy into a separate copy.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Detect via Steam").clicked() {
                            self.detect_steam_install();
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pristine_input)
                                .hint_text("…or paste the install folder"),
                        );
                        if ui.button("Set folder").clicked() {
                            let p = self.pristine_input.trim().to_owned();
                            if !p.is_empty() {
                                self.pristine_input.clear();
                                self.set_install_folder(p);
                            }
                        }
                    });
                });
                ui.add_space(6.0);
            }
            let bethesda = is_bethesda(&plan.game.kind);
            let lex = concierge::game::adapter_for(&plan.game.kind)
                .map_or_else(|_| concierge::game::Lexicon::default(), concierge::game::GameAdapter::lexicon);

            // Lifecycle status row: declaration → rebuild → realised generation.
            ui.horizontal(|ui| {
                ui.label(format!("{} · {:?}", plan.game.kind, plan.game.runtime));
                ui.separator();
                ui.colored_label(
                    if realized.plan_hash.is_none() {
                        egui::Color32::GRAY
                    } else if synced {
                        egui::Color32::from_rgb(120, 190, 120)
                    } else {
                        egui::Color32::from_rgb(220, 170, 90)
                    },
                    if realized.plan_hash.is_none() {
                        "not realized"
                    } else if synced {
                        "declaration realized"
                    } else {
                        "STALE — declaration changed since last rebuild"
                    },
                );
                if let Some(tr) = self.screen().transitions.iter().find(|t| t.id == "verify").cloned() {
                    self.transition_button(ui, &tr);
                }
            });

            self.lint_banner(ui);

            // Plain-language mode banner — what mode you're in and why, so a
            // non-technical user understands (and sees it applies to the AI too).
            ui.add_space(2.0);
            if self.mutable {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 200, 140),
                    "🔓 Edit mode — you can change this modpack. Changes save to your Setup; click Apply to install them into the game.",
                );
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 180, 100),
                    "🔒 Locked (view only) — nothing can change this modpack right now: not you, not the assistant. Switch to Edit (top-right) to make changes.",
                );
            }
            ui.add_space(2.0);

            ui.separator();
            ui.horizontal(|ui| {
                // The tab bar is projected from the screen's Tab-kind transitions.
                self.render_kind(ui, concierge_ui::WidgetKind::Tab);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(match self.central_tab {
                        CentralTab::Declaration => "your setup · editable",
                        CentralTab::Realised => "installed in the game · read-only",
                    });
                });
            });
            ui.separator();

            match self.central_tab {
                CentralTab::Declaration => {
                    ui.horizontal(|ui| {
                        ui.label("search:");
                        // the filter box is projected from the screen's search field
                        self.render_field(ui, "search");
                        ui.separator();
                        ui.add_enabled_ui(self.mutable, |ui| {
                            // "+ add mod" projected from the add_open transition.
                            if let Some(tr) = self.screen().transitions.iter().find(|t| t.id == "add_open").cloned() {
                                self.transition_button(ui, &tr);
                            }
                            if let Some(tr) = self.screen().transitions.iter().find(|t| t.id == "open_browse").cloned() {
                                self.transition_button(ui, &tr);
                            }
                        });
                        if !self.mutable {
                            ui.weak("🔒 locked — switch to Edit to change your setup");
                        }
                    });
                    if self.add.open {
                        self.add_form(ui);
                    }
                    // Promoted foundational tools (a script extender installs to
                    // a game root the adapter names) get their OWN slot, out of
                    // the ordinary mod list — they aren't mods like anything else.
                    let promoted_roots: Vec<String> = self
                        .plan
                        .as_ref()
                        .and_then(|p| concierge::game::adapter_for(&p.game.kind).ok())
                        .map(|a| {
                            a.promoted_tools()
                                .into_iter()
                                .map(|t| t.install_root.to_owned())
                                .collect()
                        })
                        .unwrap_or_default();
                    let is_tool = |m: &Mod| {
                        m.install_root
                            .as_deref()
                            .is_some_and(|r| promoted_roots.iter().any(|x| x == r))
                    };
                    let main_rows: Vec<(usize, Mod)> = mods
                        .iter()
                        .cloned()
                        .enumerate()
                        .filter(|(_, m)| !is_tool(m))
                        .collect();
                    let tool_rows: Vec<(usize, Mod)> = mods
                        .iter()
                        .cloned()
                        .enumerate()
                        .filter(|(_, m)| is_tool(m))
                        .collect();
                    ui.label("WHAT — the mods in this pack (identity + source):");
                    self.mod_list(ui, &main_rows, mods.len(), true);
                    if !tool_rows.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "\u{2699} Foundational tools — installed to the game and launched \
                                 through, kept out of the load order:",
                            )
                            .strong(),
                        );
                        self.mod_list(ui, &tool_rows, mods.len(), false);
                    }
                    // Surface the download reality by the buttons, not buried in
                    // the bottom log (T3.7: 14/30 only found it there).
                    if !mods.is_empty() {
                        ui.horizontal(|ui| {
                            ui.small(
                                "Downloads: Premium fetches automatically; free accounts click \
                                 \"Mod Manager Download\" per mod.",
                            );
                            if ui
                                .small_button("Download session…")
                                .on_hover_text(
                                    "a guided queue of the mods still to download (Wabbajack-style)",
                                )
                                .clicked()
                            {
                                self.download_session_open = true;
                            }
                        });
                    }
                    // Script-extender heads-up when the pack declares it needs one
                    // (i4c2: "no SKSE warning"). Concierge now installs it for you:
                    // add the archive and converge detects the loader and deploys
                    // it to the game root (see realize::resolve_layouts).
                    if let Some(se) = self
                        .manifest
                        .as_ref()
                        .and_then(|m| m.compat.script_extender.clone())
                    {
                        // The tool's NAME and HOME come from the active game's
                        // adapter — the crate that promotes it — so the UI never
                        // hardcodes which extender (SKSE/F4SE/…) or where to get it.
                        let tool = self
                            .plan
                            .as_ref()
                            .and_then(|p| concierge::game::adapter_for(&p.game.kind).ok())
                            .and_then(|a| a.promoted_tools().into_iter().next());
                        let name = tool.map_or("a script extender", |t| t.name);
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 170, 90),
                            format!(
                                "\u{2699} This pack needs {name} \u{2265} {se} — add the archive below \
                                 and Concierge installs it to your game folder and launches the game \
                                 through it."
                            ),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button(format!("\u{2795} Add {name}\u{2026}"))
                                .on_hover_text(
                                    "Opens the add-a-mod form. Paste the download URL; Concierge \
                                     detects the loader in the archive and installs it to the game \
                                     root automatically (no need to set install_root).",
                                )
                                .clicked()
                            {
                                self.add.open = true;
                            }
                            if let Some(t) = tool {
                                ui.hyperlink_to(format!("get {name}"), t.home);
                            }
                        });
                    }
                    // RELATIONAL — cross-mod concerns, kept distinct from the mods.
                    if bethesda && !order.is_empty() {
                        ui.separator();
                        ui.strong(format!("How mods relate · {} ({} {})", lex.order, order.len(), lex.plugins));
                        egui::ScrollArea::vertical()
                            .id_salt("order")
                            .max_height(120.0)
                            .show(ui, |ui| {
                                for (i, p) in order.iter().enumerate() {
                                    ui.monospace(format!("{i:>3}  {p}"));
                                }
                            });
                    }
                    if let Some(m) = &self.manifest {
                        let r = &m.relations;
                        if !r.requires.is_empty() || !r.incompatible.is_empty() || !r.provides.is_empty() {
                            ui.separator();
                            ui.strong("How mods relate · rules (needs / conflicts-with / provides)");
                            for req in &r.requires {
                                let v = req.min_version.as_deref().map_or_else(String::new, |v| format!(" >= {v}"));
                                ui.monospace(format!("requires: {} needs {}{v}", req.name, req.needs));
                            }
                            for inc in &r.incompatible {
                                ui.monospace(format!("incompatible: {} ✗ {}", inc.a, inc.b));
                            }
                            for p in &r.provides {
                                ui.monospace(format!("provides: {} → {}", p.name, p.capability));
                            }
                        }
                        if !r.patches.is_empty() || !r.rules.is_empty() {
                            ui.separator();
                            ui.strong("How mods relate · fixes (patches / overrides)");
                            for p in &r.patches {
                                ui.label(format!("patch '{}' bridges {}", p.name, p.bridges.join(" + ")));
                            }
                            for rule in &r.rules {
                                ui.label(format!("rule: '{}' wins {}", rule.winner, rule.path));
                            }
                        }
                        if !r.groups.is_empty() {
                            ui.separator();
                            ui.strong("How mods relate · load-order groups");
                            for g in &r.groups {
                                ui.label(if g.after.is_empty() {
                                    format!("group '{}'", g.name)
                                } else {
                                    format!("group '{}' after {}", g.name, g.after.join(", "))
                                });
                            }
                        }
                        let c = &m.compat;
                        let has_compat = c.game_version.is_some()
                            || c.game_version_max.is_some()
                            || !c.dlc.is_empty()
                            || c.script_extender.is_some()
                            || c.loader.is_some()
                            || c.side.is_some();
                        if has_compat {
                            ui.separator();
                            ui.strong("ENVIRONMENT · compat (what the pack needs)");
                            if let Some(v) = &c.game_version {
                                ui.monospace(format!("game >= {v}"));
                            }
                            if let Some(v) = &c.game_version_max {
                                ui.monospace(format!("game <= {v}"));
                            }
                            if !c.dlc.is_empty() {
                                ui.monospace(format!("DLC: {}", c.dlc.join(", ")));
                            }
                            if let Some(v) = &c.script_extender {
                                ui.monospace(format!("script extender >= {v}"));
                            }
                            if let Some(v) = &c.loader {
                                ui.monospace(format!("loader: {v} {}", c.loader_version.as_deref().unwrap_or("")));
                            }
                            if let Some(v) = &c.side {
                                ui.monospace(format!("side: {v}"));
                            }
                        }
                    }
                }
                CentralTab::Realised => {
                    Self::realised_view(ui, &realized, &per_mod, &mods, synced, &realised_order, lex);
                    ui.separator();
                    self.generations_panel(ui);
                    ui.separator();
                    self.saves_panel(ui, busy);
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                if let Some(tr) = self.screen().transitions.iter().find(|t| t.id == "open_preview").cloned() {
                    self.transition_button(ui, &tr);
                }
                // The action-bar row is rendered FROM the concierge-ui Screen —
                // the SAME view-model the headless text/automaton view uses — so
                // its labels, enabled/guard state, and hovers can never drift
                // from what an agent sees. Chrome (Preview/tabs/lock) stays put.
                let bar: Vec<concierge_ui::Transition> = self
                    .screen()
                    .transitions
                    .into_iter()
                    .filter(|t| concierge_ui::is_action_bar(&t.id))
                    .collect();
                for tr in &bar {
                    self.transition_button(ui, tr);
                }
                if busy {
                    ui.spinner();
                }
            });
        });

        self.settings_panel(ctx);
        self.diff_window(ctx);
        self.browse_window(ctx);
        self.download_window(ctx);
        self.quickstart_window(ctx);
        self.confirm_modal(ctx);

        if let Some(edit) = self.edit.take() {
            self.apply(edit);
        }

        if busy || self.browse_busy || self.term.as_ref().is_some_and(|t| !t.finished()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

impl App {
    /// The quick-start guide — a full "how to use Concierge" pop-up. The five
    /// steps check themselves off as the pack comes together; below them, the
    /// Setup-vs-Installed distinction and a reference to the key features and the
    /// agent shell. Opens on first run and from the top-bar "Quick start" toggle.
    fn quickstart_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if !self.quickstart_open {
            return;
        }
        let has_game = !self.games.is_empty();
        let has_profile = self.active_repo().is_some();
        let has_mods = self.manifest.as_ref().is_some_and(|m| !m.mods.is_empty());
        let is_realized = self
            .active_repo()
            .and_then(|r| Realized::load(&r).ok())
            .and_then(|rz| rz.plan_hash)
            .is_some();
        let done = [has_game, has_profile, has_mods, is_realized];
        let current = done.iter().position(|d| !d).unwrap_or(usize::MAX);

        // A SEPARATE OS window (its own egui viewport), not an in-app window —
        // it floats independently and gets its own title bar / close button.
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("concierge-quickstart"),
            egui::ViewportBuilder::default()
                .with_title("How to use Concierge")
                .with_inner_size([580.0, 660.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(
                    "Concierge builds a modded COPY of your game from a text modpack — your \
                     original install is never touched.",
                );
                        ui.add_space(8.0);

                        ui.heading("The flow");
                        let step =
                            |ui: &mut egui::Ui, i: usize, done: bool, title: &str, detail: &str| {
                                let (mark, col) = if done {
                                    ("\u{2714}", egui::Color32::from_rgb(120, 200, 140))
                                } else if current == i {
                                    ("\u{25B6}", egui::Color32::from_rgb(120, 180, 240))
                                } else {
                                    ("\u{2022}", ui.visuals().weak_text_color())
                                };
                                ui.horizontal_wrapped(|ui| {
                                    ui.colored_label(col, format!("{mark}  {}. {title}", i + 1));
                                    ui.weak(format!("— {detail}"));
                                });
                            };
                        step(
                            ui,
                            0,
                            has_game,
                            "Add your game",
                            "the game you want to mod — “+ add game” at the top",
                        );
                        step(
                            ui,
                            1,
                            has_profile,
                            "Create a modpack",
                            "a named profile of mods for that game — “new profile”",
                        );
                        step(
                    ui,
                    2,
                    has_mods,
                    "Add mods",
                    "“browse” the catalog or “+ add mod” with a link — they go into your Setup",
                );
                        step(
                            ui,
                            3,
                            is_realized,
                            "Apply",
                            "installs your Setup into a private copy of the game",
                        );
                        step(ui, 4, false, "Play", "launch the modded game");

                        ui.add_space(10.0);
                        ui.heading("Setup vs Installed");
                        ui.label("• Setup — the mods you WANT. Editable; this is your plan.");
                        ui.label(
                    "• Installed — what's actually DEPLOYED to the game right now. Read-only.",
                );
                        ui.weak(
                    "They match only after you Apply. Before that, Installed is empty — which is \
                     why the two tabs can look the same at first.",
                );

                        ui.add_space(10.0);
                        ui.heading("Key features");
                        ui.label("• Browse — search the mod catalog and add with one click.");
                        ui.label(
                    "• Preview — see exactly what an Apply would change (files, conflicts, load \
                     order) before it happens.",
                );
                        ui.label(
                    "• Sort load order / Conflicts — resolve mod ordering and file overwrites.",
                );
                        ui.label(
                    "• Foundational tools — a game's script extender (SKSE/F4SE/…) installs to the \
                     game root and launches through automatically; it sits in its own slot.",
                );
                        ui.label("• Verify — confirm your original game files are untouched.");
                        ui.label(
                    "• Undo / roll back — every change is reversible; Uninstall removes everything \
                     Concierge placed.",
                );

                        ui.add_space(10.0);
                        ui.heading("The agent terminal");
                        ui.label(
                    "The right-hand panel is a shell sandboxed to Concierge — it can only touch \
                     this modpack, never your machine. Run `claude` or `codex` in it to have an AI \
                     assistant build and maintain the pack; everything it does lands in the same \
                     modpack file you can see and edit.",
                );

                        ui.add_space(10.0);
                        ui.separator();
                        ui.weak(
                    "Nothing is installed until you Apply. Re-open this guide any time with \
                     “Quick start” up top.",
                );
                    });
                });
            },
        );
        if close {
            self.quickstart_open = false;
        }
    }

    /// Top bar — projected from the Screen: game/profile selectors (selection
    /// dispatches `select_game`/`select_profile`), rescan/delete/create-profile
    /// buttons + the new-profile field as transitions/fields; the ⚙ gear
    /// dispatches `open_settings`; theme + AI-column toggles stay hand-coded
    /// (layout/aesthetics).
    fn top_panel(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        let screen = self.screen();
        let games = screen.games.clone();
        let profiles = screen.profiles.clone();
        let (game_idx, profile_idx) = (screen.game_idx, screen.profile_idx);
        let find = |id: &str| screen.transitions.iter().find(|t| t.id == id).cloned();
        let (rescan_tr, delete_tr, undo_tr) =
            (find("rescan"), find("delete_profile"), find("undo"));
        let (empty_tr, clone_tr, ai_tr) = (
            find("create_empty"),
            find("create_clone"),
            find("new_modpack_ai"),
        );
        let cache = self.cache;
        let (settings_open, dark, ai_visible) = (self.settings_open, self.dark, self.ai_visible);
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Concierge");
                ui.separator();
                let mut gsel = game_idx;
                egui::ComboBox::from_label("game")
                    .selected_text(
                        games
                            .get(gsel)
                            .map_or_else(|| "-".to_owned(), |g| concierge_games::display_name(g)),
                    )
                    .show_ui(ui, |ui| {
                        for (i, g) in games.iter().enumerate() {
                            ui.selectable_value(&mut gsel, i, concierge_games::display_name(g));
                        }
                    });
                if gsel != game_idx {
                    self.dispatch_intent(&format!("select_game:{gsel}"));
                }
                // Add a game to the workspace: pick a supported kind that isn't
                // present yet; creating it makes the new-profile flow available.
                let mut to_add: Option<String> = None;
                // Local copy so the text field's &mut doesn't fight the outer
                // borrow of self; written back after the dropdown closes.
                let mut filter = std::mem::take(&mut self.add_game_filter);
                egui::ComboBox::from_id_salt("add_game")
                    .selected_text("+ add game")
                    .show_ui(ui, |ui| {
                        // Type-to-filter the ~45-game list (i5c1, i4c3).
                        ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter games…"));
                        let needle = filter.to_lowercase();
                        for kind in concierge_games::kinds() {
                            let name = concierge_games::display_name(kind);
                            // match the friendly name OR the raw slug
                            let shown = needle.is_empty()
                                || name.to_lowercase().contains(&needle)
                                || kind.to_lowercase().contains(&needle);
                            if shown && games.iter().all(|g| g != kind) {
                                ui.selectable_value(&mut to_add, Some(kind.to_owned()), name);
                            }
                        }
                    });
                self.add_game_filter = filter;
                if let Some(kind) = to_add {
                    self.add_game_filter.clear();
                    self.dispatch_intent(&format!("add_game:{kind}"));
                }
                let mut psel = profile_idx;
                egui::ComboBox::from_label("profile")
                    .selected_text(profiles.get(psel).map_or("-", String::as_str))
                    .show_ui(ui, |ui| {
                        for (i, p) in profiles.iter().enumerate() {
                            ui.selectable_value(&mut psel, i, p);
                        }
                    });
                if psel != profile_idx {
                    self.dispatch_intent(&format!("select_profile:{psel}"));
                }
                if let Some(tr) = &rescan_tr {
                    self.transition_button(ui, tr);
                }
                if let Some(tr) = &delete_tr {
                    self.transition_button(ui, tr);
                }
                let (n, bytes) = cache;
                ui.separator();
                ui.label(format!("cache: {n}, {}", human_bytes(bytes)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(settings_open, "Settings")
                        .on_hover_text("settings")
                        .clicked()
                    {
                        self.dispatch_intent("open_settings");
                    }
                    if ui
                        .selectable_label(self.quickstart_open, "Quick start")
                        .on_hover_text("show the first-run guide")
                        .clicked()
                    {
                        self.quickstart_open = !self.quickstart_open;
                    }
                    self.render_kind(ui, concierge_ui::WidgetKind::Toggle);
                    if ui
                        .selectable_label(dark, if dark { "Dark" } else { "Light" })
                        .on_hover_text("toggle theme")
                        .clicked()
                    {
                        self.dark = !self.dark;
                    }
                    if ui
                        .selectable_label(ai_visible, "AI")
                        .on_hover_text("show/hide the AI assistant column")
                        .clicked()
                    {
                        self.ai_visible = !self.ai_visible;
                    }
                    if let Some(tr) = &undo_tr {
                        self.transition_button(ui, tr);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("new profile:");
                self.render_field(ui, "new_profile");
                if let Some(tr) = &empty_tr {
                    self.transition_button(ui, tr);
                }
                if let Some(tr) = &clone_tr {
                    self.transition_button(ui, tr);
                }
                if let Some(tr) = &ai_tr {
                    if self.agent_available {
                        self.transition_button(ui, tr);
                    } else {
                        // Graceful degradation: without the claude CLI, "new
                        // modpack (AI)" used to make an empty pack + open a raw
                        // shell (15/30 confused testers). Disable it with a clear
                        // reason instead (the guard shows on disabled-hover).
                        let mut off = tr.clone();
                        off.enabled = false;
                        off.guard = Some(
                            "AI curation needs the `claude` CLI (not found on PATH) — \
                             install it, or use \"empty\"."
                                .to_owned(),
                        );
                        self.transition_button(ui, &off);
                    }
                }
            });
        });
    }

    /// Point the profile at its game install AND detect which base/DLC masters
    /// that install actually has — so the load order carries only DLC the player
    /// owns, instead of the adapter's assume-every-DLC default.
    fn set_install_folder(&mut self, path: String) {
        let path = path.trim().to_owned();
        if path.is_empty() {
            return;
        }
        let kind = self.manifest.as_ref().map(|m| m.game.kind.clone());
        self.apply(Edit::SetPristine(path.clone()));
        if let Some(dlc) = kind
            .and_then(|k| concierge::install::owned_base_plugins(&k, std::path::Path::new(&path)))
        {
            let owned = dlc.len();
            self.apply(Edit::SetDlc(dlc));
            self.notice = Some(format!(
                "Install folder set. Found {owned} base/DLC master(s) you own — the load \
                 order now matches your install (DLC you don't have aren't listed)."
            ));
        } else {
            self.notice = Some("Game install folder set.".to_owned());
        }
    }

    /// Auto-locate the active game through Steam (library folders + app manifest)
    /// and set it as the install folder. Falls back to asking for the path.
    fn detect_steam_install(&mut self) {
        let app_id = self
            .manifest
            .as_ref()
            .map(|m| m.game.kind.clone())
            .and_then(|k| {
                concierge::game::try_adapter(&k)
                    .and_then(concierge::game::GameAdapter::steam_app_id)
            });
        match app_id.and_then(concierge::install::find_steam_install) {
            Some(p) => self.set_install_folder(p.display().to_string()),
            None => {
                self.error = Some(
                    "Couldn't find the install via Steam. Enter the game's install folder \
                     manually below."
                        .to_owned(),
                );
            }
        }
    }

    /// Start (or focus) the sandboxed shell for the ACTIVE profile. The sandbox
    /// command is built IN-PROCESS (the GUI links concierge-core) and spawned
    /// DIRECTLY in the terminal — no `concierge.exe` subprocess. That matters on
    /// Windows: an unsigned helper exe spawned programmatically gets silently
    /// blocked by Defender/SmartScreen, whereas the direct child is the
    /// Microsoft-signed `powershell.exe` (or `sandbox-exec`/`bwrap` elsewhere).
    fn start_agent_terminal(&mut self) {
        let Some(dir) = self.profiles.get(self.profile_idx).map(|p| p.dir.clone()) else {
            self.log.push("no active profile for the agent".to_owned());
            return;
        };
        // One self-contained, greppable folder per open — trace.log (gui +
        // bootstrap), the terminal transcript, the exact sandbox script.
        let session = diag::new_session();
        std::env::set_var("CONCIERGE_LOG_DIR", &session);
        diag::log(&format!(
            "start_agent_terminal: profile={} · session={}",
            dir.display(),
            session.display()
        ));

        // Build the sandbox command from the plan, in-process.
        let (Some(repo), Some(plan)) = (self.active_repo(), self.plan.clone()) else {
            let msg = "no plan/repo for the active profile — set the game install folder first";
            diag::log(msg);
            self.error = Some(msg.to_owned());
            return;
        };
        let built = concierge::shell::shell_command(&repo, &plan, None, false, &[], &[]);
        let cmd_ref = match &built {
            Ok(c) => c,
            Err(e) => {
                concierge::diag::event("gui", "error", &format!("shell_command failed: {e}"));
                diag::log(&format!("shell_command failed: {e}"));
                self.error = Some(format!("Couldn't build the sandboxed shell: {e}"));
                return;
            }
        };
        // Extract argv / cwd / env from the built Command to feed the PTY.
        let program: Vec<String> = std::iter::once(cmd_ref.get_program())
            .chain(cmd_ref.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let cwd = cmd_ref
            .get_current_dir()
            .map_or_else(|| dir.clone(), std::path::Path::to_path_buf);
        let mut env: Vec<(String, String)> = cmd_ref
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        env.push((
            "CONCIERGE_LOG_DIR".to_owned(),
            session.display().to_string(),
        ));

        diag::log(&format!(
            "spawning directly: {program:?} · cwd={}",
            cwd.display()
        ));
        concierge::diag::event(
            "gui",
            "open",
            &format!(
                "direct spawn: {} · cwd={}",
                program.join(" "),
                cwd.display()
            ),
        );
        // Windows pre-flight: the interactive terminal loses PowerShell's own
        // startup errors (parse / AMSI-block / Add-Type failure) to the console
        // screen-clear. Run the SAME command once non-interactively with captured
        // stdout+stderr and write it to the (reliably-synced) top-level log — so a
        // silent failure is finally visible verbatim.
        if cfg!(windows) {
            let mut pf = std::process::Command::new(
                program.first().map_or("powershell.exe", String::as_str),
            );
            for a in program.iter().skip(1).filter(|a| a.as_str() != "-NoExit") {
                pf.arg(a);
            }
            pf.current_dir(&cwd);
            for (k, v) in &env {
                pf.env(k, v);
            }
            match pf.output() {
                Ok(out) => diag::log(&format!(
                    "PREFLIGHT exit={:?}\n---stdout---\n{}\n---stderr---\n{}\n---end preflight---",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                )),
                Err(e) => diag::log(&format!("PREFLIGHT spawn error: {e}")),
            }

            // Falsify the PTY-mechanism hypotheses INDEPENDENTLY of the sandbox:
            // does portable-pty's ConPTY render (1) any output at all, and (2) an
            // INTERACTIVE PowerShell that stays alive? If probe 2 shows nothing or
            // finished=true, the embedded terminal itself can't host an interactive
            // shell on this Windows — that's the bug, not the sandbox. Results go to
            // the top-level log (reliably synced). ~3s UI pause on the diag build.
            let probes: [(&str, Vec<String>); 3] = [
                (
                    "cmd-echo",
                    vec![
                        "cmd.exe".into(),
                        "/c".into(),
                        "echo CONCIERGE-CMD-ALIVE".into(),
                    ],
                ),
                (
                    "ps-write",
                    vec![
                        "powershell.exe".into(),
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-Command".into(),
                        "Write-Host CONCIERGE-PS-WRITE".into(),
                    ],
                ),
                (
                    "ps-interactive",
                    vec![
                        "powershell.exe".into(),
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-NoExit".into(),
                        "-Command".into(),
                        "Write-Host CONCIERGE-PS-INTERACTIVE".into(),
                    ],
                ),
            ];
            for (label, probe) in probes {
                match terminal::PtyTerminal::spawn(&probe, &cwd, &[], 40, 100, None) {
                    Ok(t) => {
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                        let finished = t.finished();
                        let grid: Vec<String> = t
                            .text_rows()
                            .into_iter()
                            .filter(|r| !r.trim().is_empty())
                            .collect();
                        diag::log(&format!(
                            "PTYPROBE[{label}] finished={finished} grid={grid:?}"
                        ));
                        t.kill();
                    }
                    Err(e) => diag::log(&format!("PTYPROBE[{label}] spawn error: {e}")),
                }
            }
        }
        let transcript = session.join("terminal.raw");
        match terminal::PtyTerminal::spawn(&program, &cwd, &env, 40, 100, Some(transcript)) {
            Ok(t) => {
                concierge::diag::event("gui", "spawned", "PTY spawn OK");
                diag::log("agent terminal spawned OK");
                // Capture the REAL sandbox terminal's state directly in the
                // top-level log (the session-folder transcript sometimes lags in
                // sync): did it stay alive, and what did it render?
                if cfg!(windows) {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let finished = t.finished();
                    let grid: Vec<String> = t
                        .text_rows()
                        .into_iter()
                        .filter(|r| !r.trim().is_empty())
                        .collect();
                    diag::log(&format!(
                        "REAL-TERMINAL after 1.5s: finished={finished} grid={grid:?}"
                    ));
                }
                self.term = Some(t);
                self.term_epoch = 0;
                self.ai_visible = true;
            }
            Err(e) => {
                concierge::diag::event("gui", "error", &format!("PTY spawn failed: {e}"));
                diag::log(&format!("agent terminal FAILED to start: {e}"));
                self.log
                    .push(format!("agent terminal failed to start: {e}"));
                self.error = Some(format!(
                    "The sandboxed shell failed to start: {e}\n(details in {})",
                    session.display()
                ));
            }
        }
    }

    /// New-modpack flow: create the empty profile, switch to it, then open the
    /// agent terminal there (the curator persona now lives in the provisioned
    /// /curate slash-command, not a bespoke system prompt).
    fn new_modpack_concierge(&mut self) {
        // Belt-and-suspenders for the disabled button: never create an empty
        // pack + open a raw shell when AI curation can't run.
        if !self.agent_available {
            self.error = Some(
                "AI curation needs the `claude` CLI (not found on PATH). Install it, or \
                 create an empty modpack instead."
                    .to_owned(),
            );
            return;
        }
        let name = self.new_profile_name.trim().to_owned();
        let Some(game) = self.games.get(self.game_idx).map(|g| g.dir.clone()) else {
            return;
        };
        match profiles::create_profile(&game, &name, None) {
            Ok(_) => {
                self.new_profile_name.clear();
                let target = self.profiles.len();
                self.reload_profiles();
                self.profile_idx = target.min(self.profiles.len().saturating_sub(1));
                self.reload_plan();
                self.start_agent_terminal();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Open the agent terminal on the active profile.
    fn work_on_profile(&mut self) {
        self.start_agent_terminal();
    }

    /// The AI assistant as a VSCode-style right-hand column: a header + quick
    /// actions on top, the input pinned at the bottom, the chat transcript
    /// filling the middle.
    /// The agent view: an embedded terminal running the user's real agent in
    /// the sandbox. When no session is running, an affordance to
    /// start one; otherwise the live PTY grid with keystroke forwarding.
    fn ai_column(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        egui::SidePanel::right("ai")
            .default_width(520.0)
            .width_range(360.0..=900.0)
            .show(ctx, |ui| {
                egui::TopBottomPanel::top("ai_head").show_inside(ui, |ui| {
                    ui.add_space(3.0);
                    ui.horizontal(|ui| {
                        ui.strong("AGENT TERMINAL");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let running = self.term.as_ref().is_some_and(|t| !t.finished());
                            if running && ui.small_button("stop").clicked() {
                                if let Some(t) = &self.term {
                                    t.kill();
                                }
                            }
                            if !running && self.term.is_some() && ui.small_button("close").clicked()
                            {
                                self.term = None;
                            }
                        });
                    });
                    ui.small(
                        "sandboxed to Concierge's write-set — the agent can touch this \
                         profile, never the pristine game or your machine",
                    );
                    if !self.mutable {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 180, 100),
                            "🔒 Locked — the manifest is read-only on disk; the agent can \
                             read and audit but cannot edit it",
                        );
                    }
                    ui.add_space(3.0);
                });

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let running = self.term.as_ref().is_some_and(|t| !t.finished());
                    if self.term.is_none() {
                        ui.add_space(8.0);
                        if ui.button("Open sandboxed shell").clicked() {
                            self.start_agent_terminal();
                        }
                        ui.add_space(6.0);
                        ui.weak(
                            "A terminal sandboxed to Concierge in this modpack. Start your AI \
                             coding agent in it — `opencode`, `codex`, or `claude` — and it \
                             builds the pack for you, confined to this modpack, never your \
                             machine. The profile carries CLAUDE.md + AGENTS.md and the \
                             slash-commands (/health, /curate, /sort, /conflicts, /audit-ids), \
                             so any agent already knows the tools.",
                        );
                        if !self.agent_available {
                            ui.add_space(6.0);
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 170, 90),
                                "Tip: install one of `opencode`, `codex`, or `claude` to run an \
                                 AI agent here — the terminal itself works without them.",
                            );
                        }
                        return;
                    }
                    if running {
                        self.forward_terminal_input(ui, ctx);
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(150, 190, 150),
                            "agent session ended — Close to dismiss, or start a new one",
                        );
                    }
                    self.render_terminal(ui);
                });
            });
    }

    /// Draw the PTY grid as monospaced rows. A styled cell-walk can
    /// come later; plain text is the honest, testable baseline.
    fn render_terminal(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        // Match the PTY size to the panel so the agent's TUI lays out right.
        let avail = ui.available_size();
        let char_w = ui
            .fonts(|f| f.glyph_width(&egui::FontId::monospace(12.0), 'M'))
            .max(4.0);
        let line_h = ui.text_style_height(&egui::TextStyle::Monospace).max(8.0);
        let cols = f32_to_u16(avail.x / char_w, 40, 400);
        let rows_n = f32_to_u16(avail.y / line_h, 10, 200);
        if let Some(term) = &mut self.term {
            term.resize(rows_n, cols);
        }
        let Some(term) = &self.term else { return };
        let rows = term.text_rows();
        egui::ScrollArea::vertical()
            .id_salt("agentterm")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let mut body = rows.join("\n");
                let text = egui::TextEdit::multiline(&mut body)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .interactive(false)
                    .frame(false);
                ui.add(text);
            });
    }

    /// Collect this frame's keystrokes and forward them to the PTY.
    fn forward_terminal_input(&mut self, ui: &eframe::egui::Ui, ctx: &eframe::egui::Context) {
        let bytes = keystrokes_to_bytes(ui, ctx);
        if !bytes.is_empty() {
            if let Some(term) = &mut self.term {
                term.send(&bytes);
            }
        }
    }

    fn lint_banner(&self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        // Declared relational-fact issues (topology): unmet requires, active
        // incompatibilities — distinct from resolver/invariant lints.
        for issue in &self.rel_issues {
            ui.colored_label(egui::Color32::from_rgb(220, 130, 110), format!("⚠ {issue}"));
        }
        let errors = self
            .lint
            .iter()
            .filter(|v| v.severity == Severity::Error)
            .count();
        let warns = self.lint.len() - errors;
        if errors == 0 && warns == 0 {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if errors > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 80, 80),
                    format!("✗ {errors} error(s)"),
                );
            }
            if warns > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 170, 90),
                    format!("⚠ {warns} warning(s)"),
                );
            }
        });
        for v in &self.lint {
            let c = if v.severity == Severity::Error {
                egui::Color32::from_rgb(220, 120, 120)
            } else {
                egui::Color32::from_rgb(200, 170, 110)
            };
            ui.colored_label(c, format!("  {} [{}]: {}", v.subject, v.rule, v.detail));
        }
    }

    /// Add-mod form — projected from the Screen: each text field via `render_field`
    /// (`dispatch_type`), the "add to manifest" button via the `add_confirm` transition.
    fn add_form(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        let fields: Vec<(String, String)> = self
            .screen()
            .fields
            .into_iter()
            .filter(|f| f.id.starts_with("add_"))
            .map(|f| (f.id, f.label))
            .collect();
        egui::Grid::new("addform").num_columns(2).show(ui, |ui| {
            for (id, label) in &fields {
                ui.label(label);
                self.render_field(ui, id);
                ui.end_row();
            }
        });
        if let Some(tr) = self
            .screen()
            .transitions
            .iter()
            .find(|t| t.id == "add_confirm")
            .cloned()
        {
            self.transition_button(ui, &tr);
        }
    }

    /// The declaration's mod list — editable config (toggle / reorder / remove /
    /// select). Reorder is gated off while a search filter is active, since the
    /// displayed order isn't the real order then (MO2's rule).
    /// Render a mod grid. `rows` carries each mod with its ORIGINAL manifest
    /// index, so the list can be split (main mods vs promoted foundational tools)
    /// without breaking drag-reorder — the index still points at the right
    /// manifest entry. `total` is the full manifest length (for the down-bound);
    /// `reorderable` is false for the tools slot (a game-root tool has no place
    /// in the load order).
    fn mod_list(
        &mut self,
        ui: &mut eframe::egui::Ui,
        rows: &[(usize, Mod)],
        total: usize,
        reorderable: bool,
    ) {
        use eframe::egui;
        let q = self.search.to_lowercase();
        let mutable = self.mutable;
        let can_reorder = q.is_empty() && mutable && reorderable;
        egui::ScrollArea::vertical()
            .id_salt("mods")
            .max_height(300.0)
            .show(ui, |ui| {
                egui::Grid::new("modsgrid")
                    .striped(true)
                    .num_columns(5)
                    .show(ui, |ui| {
                        ui.strong("on");
                        ui.strong("mod");
                        ui.strong("ver");
                        ui.strong("src");
                        ui.strong("");
                        ui.end_row();
                        for (i, m) in rows.iter().map(|(idx, m)| (*idx, m)) {
                            if !q.is_empty() && !m.name.to_lowercase().contains(&q) {
                                continue;
                            }
                            let mut en = m.enabled;
                            if ui
                                .add_enabled(mutable, egui::Checkbox::new(&mut en, ""))
                                .changed()
                            {
                                // route through dispatch: App mutates only via intents
                                self.dispatch_intent(&format!("mod_toggle:{}", m.name));
                            }
                            let sel = self.selected.as_deref() == Some(&m.name);
                            // The name is ALWAYS a plain clickable label, so a click
                            // always selects. Drag-to-reorder lives on a separate
                            // handle: a clickable widget inside a drag source has its
                            // clicks swallowed by the drag sense, which is why
                            // clicking the name did nothing before. The ↑/↓ buttons
                            // reorder as well, so the handle is a convenience.
                            ui.horizontal(|ui| {
                                if can_reorder {
                                    let dnd = ui.dnd_drag_source(
                                        egui::Id::new(("dragmod", i)),
                                        i,
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(egui::RichText::new(":::").weak())
                                                    .sense(egui::Sense::drag()),
                                            )
                                            .on_hover_text("drag to reorder");
                                        },
                                    );
                                    if let Some(from) = dnd.response.dnd_release_payload::<usize>()
                                    {
                                        if *from != i {
                                            self.dispatch_intent(&format!("mod_move:{from}:{i}"));
                                        }
                                    }
                                }
                                if ui.selectable_label(sel, &m.name).clicked() {
                                    self.dispatch_intent(&format!("mod_select:{}", m.name));
                                }
                            });
                            ui.label(&m.version);
                            ui.label(mod_source(m));
                            ui.horizontal(|ui| {
                                self.row_btn(
                                    ui,
                                    &format!("mod_up:{}", m.name),
                                    "↑",
                                    "move up in load order",
                                    can_reorder && i > 0,
                                );
                                self.row_btn(
                                    ui,
                                    &format!("mod_down:{}", m.name),
                                    "↓",
                                    "move down in load order",
                                    can_reorder && i + 1 < total,
                                );
                                self.row_btn(
                                    ui,
                                    &format!("mod_remove:{}", m.name),
                                    "🗑",
                                    "remove this mod from the pack",
                                    mutable,
                                );
                            });
                            ui.end_row();
                        }
                        if rows.is_empty() {
                            ui.label("(no mods — use + add mod)");
                            ui.end_row();
                        }
                    });
            });
    }

    /// The realised generation — READ-ONLY. What is actually deployed on disk:
    /// the generation hash, deployed file counts per mod, and whether each
    /// deployed mod is still in the declaration (else it's orphaned drift).
    fn realised_view(
        ui: &mut eframe::egui::Ui,
        realized: &Realized,
        per_mod: &std::collections::BTreeMap<String, usize>,
        mods: &[Mod],
        synced: bool,
        deployed_order: &[String],
        lex: concierge::game::Lexicon,
    ) {
        use eframe::egui;
        let Some(hash) = realized.plan_hash.as_deref() else {
            ui.add_space(10.0);
            ui.weak("Nothing installed yet.");
            ui.label("Set up your mods, then click Apply to install them.");
            return;
        };
        let short: String = hash.chars().take(12).collect();
        ui.horizontal(|ui| {
            ui.strong("version");
            ui.monospace(short);
            ui.separator();
            ui.label(format!("{} files deployed", realized.files.len()));
        });
        ui.colored_label(
            if synced {
                egui::Color32::from_rgb(120, 190, 120)
            } else {
                egui::Color32::from_rgb(220, 170, 90)
            },
            if synced {
                "matches the current declaration"
            } else {
                "the declaration has changed — Realize to update this generation"
            },
        );
        ui.weak("Read-only: this is what is deployed on disk. Change it by editing the Declaration and rebuilding.");
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("realised")
            .max_height(340.0)
            .show(ui, |ui| {
                egui::Grid::new("realisedgrid")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        ui.strong("mod");
                        ui.strong("files");
                        ui.strong("in setup");
                        ui.end_row();
                        for (name, count) in per_mod {
                            ui.label(name);
                            ui.label(count.to_string());
                            if mods.iter().any(|m| &m.name == name && m.enabled) {
                                ui.label("declared");
                            } else {
                                ui.colored_label(egui::Color32::from_rgb(220, 170, 90), "orphaned");
                            }
                            ui.end_row();
                        }
                        if per_mod.is_empty() {
                            ui.label("(no owned files)");
                            ui.end_row();
                        }
                    });
            });
        if !deployed_order.is_empty() {
            ui.separator();
            ui.strong(format!(
                "installed {} — on-disk plugins.txt ({})",
                lex.order,
                deployed_order.len()
            ));
            egui::ScrollArea::vertical()
                .id_salt("deployed_order")
                .max_height(120.0)
                .show(ui, |ui| {
                    for (i, p) in deployed_order.iter().enumerate() {
                        ui.monospace(format!("{i:>3}  {p}"));
                    }
                });
        }
    }

    /// Save-game backups (safety), with restore of a versioned generation. Not
    /// part of the read-only realised projection — this is a data-recovery
    /// action distinct from the deployed generation.
    /// Save-backup list — projected: each backup + its restore button (the
    /// `restore_save:<g>` transition).
    fn saves_panel(&mut self, ui: &mut eframe::egui::Ui, _busy: bool) {
        use eframe::egui;
        let screen = self.screen();
        let saves = screen.saves.clone();
        ui.horizontal(|ui| {
            ui.strong("save backups");
            ui.weak(format!("{} version(s)", saves.len()));
        });
        if saves.is_empty() {
            ui.weak("Saves are snapshotted automatically before every rebuild.");
            return;
        }
        let rows: Vec<(String, Option<concierge_ui::Transition>)> = saves
            .iter()
            .map(|g| {
                (
                    g.clone(),
                    screen
                        .transitions
                        .iter()
                        .find(|t| t.id == format!("restore_save:{g}"))
                        .cloned(),
                )
            })
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("saves")
            .max_height(110.0)
            .show(ui, |ui| {
                for (g, tr) in &rows {
                    ui.horizontal(|ui| {
                        ui.monospace(g);
                        if let Some(tr) = tr {
                            self.transition_button(ui, tr);
                        }
                    });
                }
            });
    }

    /// Restore a save generation on a worker thread, logging the outcome.
    fn restore_save(&self, generation: String) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let (Some(repo), Some(plan)) = (self.active_repo(), self.plan.clone()) else {
            self.busy.store(false, Ordering::SeqCst);
            return;
        };
        let tx = self.log_tx.clone();
        let busy = Arc::clone(&self.busy);
        std::thread::spawn(move || {
            let _ = tx.send(format!("> restore saves from generation {generation}"));
            match concierge::saves::restore(&repo, &plan, &generation) {
                Ok(n) => {
                    let _ = tx.send(format!("restored {n} save file(s)"));
                }
                Err(e) => {
                    let _ = tx.send(format!("restore failed: {e}"));
                }
            }
            busy.store(false, Ordering::SeqCst);
        });
    }

    /// Settings editor — game paths (from the manifest), API-key status, and
    /// app preferences.
    /// Kick off the Nexus `whoami` off the UI thread (dispatched by `check_account`).
    fn check_account(&self) {
        if let Ok(mut s) = self.nexus_account.lock() {
            "checking…".clone_into(&mut s);
        }
        let slot = Arc::clone(&self.nexus_account);
        std::thread::spawn(move || {
            let status = match concierge::nexus::api_key() {
                Ok(k) => match concierge::nexus::validate(&k) {
                    Ok(u) if u.is_premium => {
                        format!("Premium ({}) — downloads are automatic.", u.name)
                    }
                    Ok(u) => format!(
                        "Free ({}) — Nexus requires one click per uncached mod ('Mod Manager \
                         Download'); the API key is only for metadata + the click token, not bulk \
                         download.",
                        u.name
                    ),
                    Err(e) => format!("key set but validate failed: {e}"),
                },
                Err(_) => "No API key — optional (catalog/tracked/updates). Downloads still work \
                           via click or ~/Downloads."
                    .to_owned(),
            };
            if let Ok(mut s) = slot.lock() {
                *s = status;
            }
        });
    }

    /// Settings window — projected from the Screen: the info is the `settings`
    /// panel, the Check-account button is the Settings state's `check_account`
    /// transition; only the theme/layout preferences stay hand-coded (aesthetics).
    fn settings_panel(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        let mut open = self.settings_open;
        let screen = self.screen();
        let panel = screen.panels.iter().find(|p| p.id == "settings").cloned();
        let check = screen
            .transitions
            .iter()
            .find(|t| t.id == "check_account")
            .cloned();
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                if let Some(p) = &panel {
                    for line in &p.lines {
                        if line.starts_with("  ") {
                            ui.monospace(line.trim_start());
                        } else {
                            ui.label(line);
                        }
                    }
                }
                ui.separator();
                if let Some(tr) = &check {
                    self.transition_button(ui, tr);
                }
                ui.separator();
                ui.strong("Nexus Mods");
                ui.label(
                    "Automatic in-app downloads from Nexus need Nexus Premium (paid). A \
                     personal API key — free to create at nexusmods.com \u{2192} Account \
                     Settings \u{2192} API Keys — lets Concierge identify you and look mods \
                     up; paste it here. Without Premium you can still mod: on a mod's Nexus \
                     page use \"Mod Manager Download\" (or download the file), drop it in \
                     your Downloads folder, then Apply \u{2014} no key or Premium needed.",
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.nexus_key_input)
                            .password(true)
                            .hint_text("paste Nexus API key"),
                    );
                    if ui.button("Save key").clicked() {
                        let k = self.nexus_key_input.trim().to_owned();
                        if !k.is_empty() {
                            let dir = concierge_platform::config_dir();
                            let _ = std::fs::create_dir_all(&dir);
                            match std::fs::write(dir.join("nexus-api-key"), &k) {
                                Ok(()) => {
                                    self.notice = Some(
                                        "Nexus API key saved — you can download mods now."
                                            .to_owned(),
                                    );
                                    self.nexus_key_input.clear();
                                }
                                Err(e) => self.error = Some(format!("couldn't save key: {e}")),
                            }
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    "No Premium? Enable 1-click downloads: register Concierge as the handler \
                     for Nexus's \"Mod Manager Download\" button, then one click per mod on \
                     its Nexus page sends the file straight here — free, verified, no browser \
                     round-trip. (The same handoff MO2/Vortex/Wabbajack use; nothing is ever \
                     downloaded without your click.)",
                );
                if ui
                    .button("Enable 1-click downloads")
                    .on_hover_text("register the nxm:// protocol handler for this user")
                    .clicked()
                {
                    match std::env::current_exe() {
                        Ok(exe) => match concierge_platform::register_nxm_handler(&exe) {
                            Ok(msg) => self.notice = Some(msg),
                            Err(e) => self.error = Some(e),
                        },
                        Err(e) => self.error = Some(format!("couldn't find the app path: {e}")),
                    }
                }
                ui.separator();
                ui.strong("Game install folder");
                // Owned snapshot so the manifest borrow is released before the
                // Save button calls self.apply(...).
                let pristine = self.manifest.as_ref().map(|m| {
                    let p = &m.game.pristine;
                    (p.display().to_string(), p.as_os_str().is_empty(), p.exists())
                });
                match pristine {
                    None => {
                        ui.weak("add a game and create a modpack first");
                    }
                    Some((cur, is_empty, exists)) => {
                        if is_empty {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 170, 90),
                                "not set — Apply/deploy stays blocked until this points at your game.",
                            );
                        } else {
                            ui.label(format!("current: {cur}"));
                            if !exists {
                                ui.colored_label(
                                    egui::Color32::from_rgb(220, 130, 110),
                                    "\u{26a0} that folder isn't on this machine",
                                );
                            }
                        }
                        ui.label(
                            "The folder your game is installed in. Concierge installs a copy — \
                             your original is never modified.",
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button("Detect via Steam")
                                .on_hover_text(
                                    "look up this game in your Steam libraries and set the folder",
                                )
                                .clicked()
                            {
                                self.detect_steam_install();
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut self.pristine_input)
                                    .hint_text("…or paste the install folder"),
                            );
                            if ui.button("Save path").clicked() {
                                let p = self.pristine_input.trim().to_owned();
                                if !p.is_empty() {
                                    self.pristine_input.clear();
                                    self.set_install_folder(p);
                                }
                            }
                        });
                    }
                }
                // Minecraft (Modrinth): the version + loader decide which build
                // "Add" resolves. Only shown for Modrinth-backed games.
                if self.catalog_target().is_some_and(|(m, _)| m) {
                    let (cur_gv, cur_loader) = self.manifest.as_ref().map_or_else(
                        || (String::new(), String::new()),
                        |m| {
                            (
                                m.compat.game_version.clone().unwrap_or_default(),
                                m.compat.loader.clone().unwrap_or_default(),
                            )
                        },
                    );
                    ui.separator();
                    ui.strong("Minecraft version + loader");
                    ui.small(
                        "Which build to install — \"Add\" resolves the newest Modrinth version \
                         matching these.",
                    );
                    if cur_gv.is_empty() && cur_loader.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 170, 90),
                            "not set — Add may pick the wrong loader/version until you set these.",
                        );
                    } else {
                        ui.weak(format!(
                            "current: {} / {}",
                            if cur_gv.is_empty() { "(any)" } else { &cur_gv },
                            if cur_loader.is_empty() { "(any)" } else { &cur_loader }
                        ));
                    }
                    ui.horizontal(|ui| {
                        ui.label("version");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mc_version_input)
                                .hint_text("e.g. 1.20.1")
                                .desired_width(90.0),
                        );
                        ui.label("loader");
                        egui::ComboBox::from_id_salt("mc_loader")
                            .selected_text(if self.mc_loader_input.is_empty() {
                                "choose"
                            } else {
                                &self.mc_loader_input
                            })
                            .show_ui(ui, |ui| {
                                for l in ["fabric", "forge", "quilt", "neoforge"] {
                                    ui.selectable_value(
                                        &mut self.mc_loader_input,
                                        l.to_owned(),
                                        l,
                                    );
                                }
                            });
                        if ui.button("Save").clicked() {
                            // empty field = keep the current value (don't clear).
                            let gv = if self.mc_version_input.trim().is_empty() {
                                cur_gv.clone()
                            } else {
                                self.mc_version_input.trim().to_owned()
                            };
                            let loader = if self.mc_loader_input.trim().is_empty() {
                                cur_loader.clone()
                            } else {
                                self.mc_loader_input.trim().to_owned()
                            };
                            self.apply(Edit::SetCompat(gv, loader));
                            self.notice = Some("Minecraft version + loader saved.".to_owned());
                        }
                    });
                }
                ui.separator();
                ui.strong("preferences");
                ui.checkbox(&mut self.dark, "dark theme");
                ui.checkbox(&mut self.ai_visible, "show AI assistant column");
            });
        self.settings_open = open;
    }

    /// `ApplyDiff` preview — a dry look at what Realize will change (declaration
    /// vs realised generation), before anything is written.
    /// Preview window — a read-only projection of the `preview` panel (the diff
    /// summary is computed in `build_panels`). Line colouring is the renderer's
    /// aesthetic call; the window's X projects the `close_preview` transition.
    fn diff_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        let mut open = self.diff_open;
        let panel = self.screen().panels.into_iter().find(|p| p.id == "preview");
        egui::Window::new("Preview changes")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                if let Some(p) = &panel {
                    for line in &p.lines {
                        let color = if line.starts_with('+') || line.trim_start().starts_with('+') {
                            Some(egui::Color32::from_rgb(120, 190, 120))
                        } else if line.starts_with('-') || line.trim_start().starts_with('-') {
                            Some(egui::Color32::from_rgb(220, 120, 120))
                        } else if line.starts_with('~') {
                            Some(egui::Color32::from_rgb(220, 170, 90))
                        } else {
                            None
                        };
                        match color {
                            Some(c) => {
                                ui.colored_label(c, line);
                            }
                            None if line.starts_with("   ") => {
                                ui.monospace(line.trim_start());
                            }
                            None => {
                                ui.label(line);
                            }
                        }
                    }
                }
            });
        self.diff_open = open;
    }

    /// Integrated mod browser — search the local Nexus catalog and add a hit to
    /// the declaration.
    /// Browse window — projected from the Screen: the query + nxm text fields,
    /// the search + add-nxm buttons, the browse message panel, and one add button
    /// per result row (the `add_hit:<id>` transitions).
    fn browse_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        let mut open = self.browse_open;
        let screen = self.screen();
        let find = |id: &str| screen.transitions.iter().find(|t| t.id == id).cloned();
        let search_tr = find("browse_search");
        let nxm_tr = find("nxm_add");
        let msg = screen
            .panels
            .iter()
            .find(|p| p.id == "browse")
            .map(|p| p.lines.join(" "));
        let rows: Vec<(concierge_ui::BrowseHit, Option<concierge_ui::Transition>)> = screen
            .browse_hits
            .iter()
            .map(|h| (h.clone(), find(&format!("add_hit:{}", h.mod_id))))
            .collect();
        // The status line reads a CACHED (rows, synced) — the worker refreshes
        // it; the window never touches the catalog during a repaint.
        let target = self.catalog_target();
        let is_modrinth = target.as_ref().is_some_and(|(m, _)| *m);
        let provider = if is_modrinth { "Modrinth" } else { "Nexus" };
        let status = target.map(|(_, d)| (d, self.browse_status));
        let syncing = self.sync_progress.lock().ok().and_then(|g| g.clone());
        egui::Window::new(format!("Browse mods ({provider} catalog)"))
            .open(&mut open)
            .default_width(560.0)
            .default_height(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                // Catalog sync affordance — fixes the dead-end "is the catalog
                // synced?" by saying so and offering the fix in place.
                if let Some(msg) = &syncing {
                    let elapsed = self
                        .sync_started
                        .lock()
                        .ok()
                        .and_then(|g| *g)
                        .map_or(0, |t| t.elapsed().as_secs());
                    if let Some((frac, label)) = sync_bar(msg, elapsed) {
                        ui.add(egui::ProgressBar::new(frac).text(label).animate(true));
                        ui.weak("showing mods as they download — search works already");
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(msg);
                        });
                    }
                    ui.separator();
                } else if let Some((d, (rows_n, synced))) = &status {
                    ui.horizontal(|ui| {
                        if *rows_n == 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 170, 90),
                                format!("catalog for {d}: not downloaded yet — click Sync"),
                            );
                        } else {
                            ui.weak(format!("catalog: {rows_n} mods, synced {}", ago(*synced)));
                        }
                        if ui
                            .button("⟳ Sync now")
                            .on_hover_text(format!(
                                "download the public {provider} mod list for this game"
                            ))
                            .clicked()
                        {
                            self.sync_catalog(d.clone(), is_modrinth);
                        }
                    });
                    ui.separator();
                }
                let submitted = ui
                    .horizontal(|ui| {
                        let enter = self.render_field(ui, "browse_query");
                        if let Some(tr) = &search_tr {
                            self.transition_button(ui, tr);
                        }
                        enter
                    })
                    .inner;
                // Enter in the search box runs the search — testers repeatedly
                // hit Enter and nothing happened (i3c5, i4c4, i5c4).
                if submitted {
                    self.do_browse_search();
                }
                // First-class category filter + sort — the normal Nexus controls.
                self.browse_filters(ui);
                ui.horizontal(|ui| {
                    ui.label("or paste an nxm:// link:");
                    self.render_field(ui, "nxm_input");
                    if let Some(tr) = &nxm_tr {
                        self.transition_button(ui, tr);
                    }
                });
                if let Some(m) = &msg {
                    if !m.is_empty() {
                        ui.weak(m);
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if self.browse_busy {
                        ui.spinner();
                        ui.weak("loading catalog…");
                    } else {
                        ui.weak(format!("{} result(s)", rows.len()));
                        if self.browse_category.is_some()
                            || self.browse_sort != concierge_ai::tools::SortBy::Endorsements
                        {
                            ui.weak("·");
                            if let Some(c) = &self.browse_category {
                                ui.weak(format!("category: {c}"));
                            }
                            ui.weak(self.browse_sort.label());
                        }
                    }
                });
                ui.add_space(4.0);
                // Nexus-style card grid: one mod page-like card per hit. The green
                // Nexus "download" is replaced by "+ Add to manifest" (declarative
                // curation — nothing is downloaded here).
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (hit, add) in &rows {
                            self.browse_card(ui, hit, add.as_ref());
                            ui.add_space(8.0);
                        }
                    });
            });
        self.browse_open = open;
    }

    /// The category filter + sort controls — first-class, above the results.
    /// Changing either re-runs the search immediately.
    /// The guided "download session" — the Wabbajack pattern for free accounts:
    /// list the mods still missing their archive, open each one's Nexus file page
    /// on request, and let the nxm:// handoff drop them off the list as they land
    /// and hash-verify. One click per mod, free, no automation.
    fn download_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if !self.download_session_open {
            return;
        }
        let mut open = true;
        let repo = self.active_repo();
        let domain = self.plan.as_ref().and_then(|p| p.game.nexus_domain.clone());
        let mods = self.plan.as_ref().map_or_else(Vec::new, |p| p.mods.clone());
        let total = mods.len();
        let needed: Vec<&concierge::plan::PlannedMod> = mods
            .iter()
            .filter(|m| {
                let present = repo.as_ref().is_some_and(|r| {
                    m.md5
                        .as_deref()
                        .is_some_and(|h| !h.is_empty() && r.store_path(h, &m.file).exists())
                });
                !present
            })
            .collect();
        let have = total - needed.len();
        let pct = have.saturating_mul(100).checked_div(total).unwrap_or(0);
        let frac = f32::from(u16::try_from(pct.min(100)).unwrap_or(0)) / 100.0;
        egui::Window::new("Download session")
            .open(&mut open)
            .resizable(true)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::ProgressBar::new(frac).text(format!("{have} of {total} mods downloaded")),
                );
                ui.small(
                    "Free account? Turn on \"Enable 1-click downloads\" in Settings first. \
                     Then click \"Mod Manager Download\" on each page below — the file lands \
                     here, is checksum-verified, and drops off this list.",
                );
                ui.separator();
                if needed.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 190, 120),
                        "\u{2713} Every mod is downloaded — you can Apply.",
                    );
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for m in &needed {
                            ui.horizontal(|ui| {
                                ui.label(&m.name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| match (&m.source, domain.as_deref()) {
                                        (
                                            concierge::plan::Source::Nexus { mod_id, file_id },
                                            Some(d),
                                        ) => {
                                            if ui
                                                .button("Open Nexus page")
                                                .on_hover_text(
                                                    "click \"Mod Manager Download\" there",
                                                )
                                                .clicked()
                                            {
                                                let _ = concierge_platform::open_url(
                                                    &concierge::nexus::file_page_url(
                                                        d, *mod_id, *file_id,
                                                    ),
                                                );
                                            }
                                        }
                                        (concierge::plan::Source::Url { .. }, _) => {
                                            ui.weak("direct URL — use Download");
                                        }
                                        _ => {
                                            ui.weak("downloads via Download");
                                        }
                                    },
                                );
                            });
                        }
                    });
            });
        // While a download is in flight, keep repainting so items drop off live.
        if self.busy.load(Ordering::SeqCst) {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        self.download_session_open = open;
    }

    fn browse_filters(&mut self, ui: &mut eframe::egui::Ui) {
        use concierge_ai::tools::SortBy;
        use eframe::egui;
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("category:");
            let current = self.browse_category.clone();
            let label = current.as_deref().unwrap_or("All categories");
            egui::ComboBox::from_id_salt("browse_cat")
                .selected_text(label)
                .width(190.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current.is_none(), "All categories")
                        .clicked()
                    {
                        self.browse_category = None;
                        changed = true;
                    }
                    for (cat, n) in &self.browse_categories {
                        let sel = current.as_deref() == Some(cat.as_str());
                        if ui.selectable_label(sel, format!("{cat}  ({n})")).clicked() {
                            self.browse_category = Some(cat.clone());
                            changed = true;
                        }
                    }
                });

            ui.add_space(8.0);
            ui.label("sort:");
            egui::ComboBox::from_id_salt("browse_sort")
                .selected_text(self.browse_sort.label())
                .width(150.0)
                .show_ui(ui, |ui| {
                    for opt in SortBy::all() {
                        if ui
                            .selectable_label(self.browse_sort == opt, opt.label())
                            .clicked()
                        {
                            self.browse_sort = opt;
                            changed = true;
                        }
                    }
                });

            if (self.browse_category.is_some() || self.browse_sort != SortBy::Endorsements)
                && ui.button("clear").clicked()
            {
                self.browse_category = None;
                self.browse_sort = SortBy::Endorsements;
                changed = true;
            }
        });
        if changed {
            self.do_browse_search();
        }
    }

    /// One Nexus-style mod card: a placeholder thumbnail tile (we don't scrape
    /// or redistribute Nexus images), the title, author, category chip, stats
    /// (endorsements / downloads / updated), a summary, and the add action.
    fn browse_card(
        &mut self,
        ui: &mut eframe::egui::Ui,
        hit: &concierge_ui::BrowseHit,
        add: Option<&concierge_ui::Transition>,
    ) {
        use eframe::egui;
        egui::Frame::group(ui.style())
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(6)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Thumbnail placeholder tile (initial letter) — image-free,
                    // AUP-safe.
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 4.0, egui::Color32::from_gray(60));
                    let letter = hit
                        .name
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string();
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        letter,
                        egui::FontId::proportional(28.0),
                        egui::Color32::from_gray(180),
                    );
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(egui::RichText::new(&hit.name).size(15.0));
                            if !hit.author.is_empty() {
                                ui.weak(format!("by {}", hit.author));
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            if !hit.category.is_empty() {
                                chip(ui, &hit.category);
                            }
                            ui.weak(format!("▲ {}", human_count(hit.endorsements)));
                            ui.weak(format!("⭳ {}", human_count(hit.downloads)));
                            if !hit.updated_at.is_empty() {
                                ui.weak(format!(
                                    "· {}",
                                    hit.updated_at.split('T').next().unwrap_or("")
                                ));
                            }
                        });
                        if !hit.summary.is_empty() {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&hit.summary).weak()).wrap(),
                            );
                        }
                        ui.add_space(4.0);
                        // The add-to-manifest action, styled green like Nexus'
                        // download button; "added" shows a disabled confirmation.
                        if hit.added {
                            ui.add_enabled(false, egui::Button::new("✓ In pack"));
                        } else if let Some(tr) = add {
                            let btn = egui::Button::new(
                                egui::RichText::new("+ Add to pack").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(78, 141, 74));
                            if ui
                                .add_enabled(tr.enabled, btn)
                                .on_hover_text("add this mod to your pack")
                                .clicked()
                            {
                                self.dispatch_intent(&tr.id);
                            }
                        }
                    });
                });
            });
    }

    /// Add a catalog hit to the declaration, auto-resolving its main Nexus file
    /// so it's pinned-ready (the resolve step of resolve → download → pin).
    /// Add a catalog hit — resolving its main Nexus file (network) OFF the UI
    /// thread, then appending the pinned-ready `[[mod]]` atomically.
    /// Add a catalog hit to the manifest. Each add runs on its OWN thread and
    /// serializes only the manifest append (via `ADD_LOCK`) — it does NOT take
    /// the shared busy flag, so rapid/concurrent adds all complete instead of
    /// the second being silently dropped.
    fn add_catalog_hit(&self, mod_id: u64, name: &str) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let Some((is_modrinth, domain)) = self.catalog_target() else {
            return;
        };
        let name = name.to_owned();
        let (tx, reload) = (self.log_tx.clone(), Arc::clone(&self.reload_pending));
        let _ = tx.send(format!("adding {name}…"));
        if is_modrinth {
            // Resolve the project (slug lives in the catalog's version column)
            // to a free Modrinth CDN file, filtered by the pack's game version +
            // loader; add it as a Url mod so `download` fetches it free.
            let (gv, loader) = self.manifest.as_ref().map_or((None, None), |m| {
                (m.compat.game_version.clone(), m.compat.loader.clone())
            });
            std::thread::spawn(move || {
                let slug = concierge_db::catalog::Catalog::open(&repo.catalog_path())
                    .ok()
                    .and_then(|c| c.version_of(&domain, mod_id).ok().flatten())
                    .unwrap_or_default();
                if slug.is_empty() {
                    let _ = tx.send(format!("add {name}: no Modrinth id cached — re-sync"));
                    return;
                }
                match concierge_db::modrinth::resolve(&slug, gv.as_deref(), loader.as_deref()) {
                    Ok(r) => {
                        let entry = url_new_mod(name.clone(), r.url, r.filename, r.version_number);
                        write_added_mod(&repo, &entry, &tx, &reload);
                        record_audit_ok(&repo, mod_id, &name);
                    }
                    Err(e) => {
                        let _ = tx.send(format!("add {name}: {e}"));
                    }
                }
            });
            return;
        }
        std::thread::spawn(move || {
            // Resolve the main file: id + filename + the actual version.
            let (file_id, file, version) = concierge::nexus::api_key()
                .and_then(|k| concierge::nexus::main_file(&k, &domain, mod_id))
                .map_or_else(
                    |e| {
                        let _ = tx.send(format!(
                            "add {name}: couldn't resolve file ({e}) — unpinned"
                        ));
                        (0, String::new(), String::new())
                    },
                    |f| {
                        (
                            u32::try_from(f.file_id).unwrap_or(0),
                            f.file_name,
                            f.version.unwrap_or_default(),
                        )
                    },
                );
            let entry = nexus_new_mod(
                name.clone(),
                mod_id,
                u64::from(file_id),
                String::new(),
                file,
                version,
            );
            write_added_mod(&repo, &entry, &tx, &reload);
            // The id came straight from the catalog, so the entry is audit-OK
            // by construction — record it so eval/realize don't flag it
            // unaudited (the browser feeds the audit gate).
            record_audit_ok(&repo, mod_id, &name);
        });
    }

    /// Sync the catalog for `domain` off-thread (the browser's "Sync now").
    /// The active catalog provider for the current game: `(is_modrinth, domain)`.
    /// Prefers Modrinth (Minecraft) then Nexus; `None` if the game has neither.
    fn catalog_target(&self) -> Option<(bool, String)> {
        let g = &self.plan.as_ref()?.game;
        g.modrinth_domain.clone().map_or_else(
            || g.nexus_domain.clone().map(|d| (false, d)),
            |d| Some((true, d)),
        )
    }

    fn sync_catalog(&self, domain: String, modrinth: bool) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(repo) = self.active_repo() else {
            self.busy.store(false, Ordering::SeqCst);
            return;
        };
        let (tx, busy, refresh) = (
            self.log_tx.clone(),
            Arc::clone(&self.busy),
            Arc::clone(&self.browse_refresh),
        );
        let prog = Arc::clone(&self.sync_progress);
        let started = Arc::clone(&self.sync_started);
        let set_prog = |g: &Arc<std::sync::Mutex<Option<String>>>, v: Option<String>| {
            if let Ok(mut lock) = g.lock() {
                *lock = v;
            }
        };
        set_prog(
            &prog,
            Some("Downloading the mod list… (one-time, can take a couple of minutes)".to_owned()),
        );
        if let Ok(mut s) = started.lock() {
            *s = Some(std::time::Instant::now());
        }
        std::thread::spawn(move || {
            let _ = tx.send(format!("> syncing {domain} catalog…"));
            match concierge_db::catalog::Catalog::open(&repo.catalog_path()) {
                Ok(mut cat) => {
                    let mut pages = 0u32;
                    // Stream what's downloaded so far into an open browser (WAL
                    // lets the search read mid-sync) — no waiting for the whole
                    // catalog before you can look at anything.
                    let mut on_line = |l: &str| {
                        let _ = tx.send(l.to_owned());
                        set_prog(
                            &prog,
                            Some(format!("Downloading the mod list… {}", l.trim())),
                        );
                        pages = pages.wrapping_add(1);
                        if pages.is_multiple_of(15) {
                            refresh.store(true, Ordering::SeqCst);
                        }
                    };
                    let result = if modrinth {
                        concierge_db::sync::sync_modrinth(&mut cat, &domain, &mut on_line)
                    } else {
                        concierge_db::sync::sync_game(&mut cat, &domain, &mut on_line)
                    };
                    match result {
                        Ok(r) => {
                            let _ = tx.send(format!("catalog synced: {} rows", r.rows_synced));
                            refresh.store(true, Ordering::SeqCst); // final re-search
                        }
                        Err(e) => {
                            let _ = tx.send(format!("catalog sync failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(format!("catalog open failed: {e}"));
                }
            }
            set_prog(&prog, None);
            if let Ok(mut s) = started.lock() {
                *s = None;
            }
            busy.store(false, Ordering::SeqCst);
        });
    }

    /// Handle an `nxm://` link (from the site's "Mod Manager Download" button).
    /// If it carries the click token, download the file with it now (free-user,
    /// one-click, TOS-sanctioned) and add the mod pinned to that exact hash;
    /// otherwise add it unpinned. The token download blocks briefly — it's a
    /// deliberate click, and it's the sanctioned free-download path.
    fn handle_nxm(&mut self, url: &str) {
        let Some(n) = concierge::nexus::parse_nxm(url) else {
            self.error = Some(format!("not a valid nxm:// link: {url}"));
            return;
        };
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(repo) = self.active_repo() else {
            self.busy.store(false, Ordering::SeqCst);
            return;
        };
        let (tx, busy, reload) = (
            self.log_tx.clone(),
            Arc::clone(&self.busy),
            Arc::clone(&self.reload_pending),
        );
        // Download (network) + manifest write happen OFF the UI thread.
        std::thread::spawn(move || {
            let _ = tx.send(format!("> nxm {} file {}", n.mod_id, n.file_id));
            let name = concierge_ai::tools::catalog_names(&repo, &n.domain, &[n.mod_id])
                .ok()
                .and_then(|v| v.into_iter().next().map(|(_, mn)| mn))
                .unwrap_or_else(|| format!("nexus-mod-{}", n.mod_id));
            let (mut md5, mut file) = (String::new(), String::new());
            if let (Some(k), Some(exp), Ok(api)) = (&n.key, &n.expires, concierge::nexus::api_key())
            {
                match concierge::store::acquire_nxm(
                    &repo, &n.domain, n.mod_id, n.file_id, &api, k, exp,
                ) {
                    Ok((got, fname)) => {
                        let _ = tx.send(format!("nxm: downloaded + pinned '{fname}' (md5 {got})"));
                        md5 = got;
                        file = fname;
                    }
                    Err(e) => {
                        let _ =
                            tx.send(format!("nxm: token download failed ({e}); adding unpinned"));
                    }
                }
            }
            // nxm links don't carry the mod version; leave it to default ("1")
            // until a fetch/resolve fills it in.
            let entry = nexus_new_mod(name, n.mod_id, n.file_id, md5, file, String::new());
            write_added_mod(&repo, &entry, &tx, &reload);
            busy.store(false, Ordering::SeqCst);
        });
    }

    /// Kick off a catalog read on a WORKER THREAD — the query + the categories
    /// GROUP BY over a 75k-row catalog must never block the UI. Results arrive
    /// via `browse_rx`, tagged with `browse_seq`; a later search supersedes an
    /// in-flight one (stale results are dropped in `update`).
    fn do_browse_search(&mut self) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let Some((_, domain)) = self.catalog_target() else {
            self.browse_hits.clear();
            "this game has no mod catalog (Nexus or Modrinth)".clone_into(&mut self.browse_msg);
            return;
        };
        self.browse_seq += 1;
        self.browse_busy = true;
        let seq = self.browse_seq;
        let filter = self
            .manifest
            .as_ref()
            .map_or_else(CatalogFilter::default, |m| {
                CatalogFilter::from_curate(&m.curate)
            });
        let query = self.browse_query.clone();
        let category = self.browse_category.clone();
        let sort = self.browse_sort;
        let need_cats = self.browse_categories.is_empty();
        let tx = self.browse_tx.clone();
        std::thread::spawn(move || {
            let categories =
                need_cats.then(|| concierge_ai::tools::catalog_categories(&repo, &domain));
            let status = concierge_ai::tools::catalog_status(&repo, &domain);
            let result = match concierge_ai::tools::catalog_search_sorted(
                &repo,
                &domain,
                &query,
                50,
                &filter,
                category.as_deref(),
                sort,
            ) {
                Ok(hits) => {
                    let msg = if hits.is_empty() {
                        if status.0 > 0 {
                            "no matches — try a different search, category, or clear filters"
                        } else {
                            "catalog not synced yet — click Sync now above"
                        }
                        .to_owned()
                    } else {
                        String::new()
                    };
                    BrowseResult {
                        hits,
                        categories,
                        msg,
                        status,
                    }
                }
                Err(e) => BrowseResult {
                    hits: Vec::new(),
                    categories,
                    msg: format!("search failed: {e}"),
                    status,
                },
            };
            let _ = tx.send((seq, result));
        });
    }

    /// Modal confirmation for destructive actions (co-located guard at each
    /// destructive call site; nothing destructive runs without passing here).
    fn confirm_modal(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if self.confirm.is_none() {
            return;
        }
        // Projected from the screen: the prompt is the banner, and the Cancel /
        // Confirm buttons ARE the Confirming state's transitions (confirm_no,
        // confirm_yes) — dispatched like every other widget.
        let screen = self.screen();
        let prompt = screen.banner.clone().unwrap_or_default();
        let buttons: Vec<concierge_ui::Transition> = ["confirm_no", "confirm_yes"]
            .iter()
            .filter_map(|id| screen.transitions.iter().find(|t| &t.id == id).cloned())
            .collect();
        egui::Window::new("Confirm")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(prompt);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for tr in &buttons {
                        self.transition_button(ui, tr);
                    }
                });
            });
    }

    fn execute_confirm(&mut self, c: Confirm) {
        match c {
            Confirm::RemoveMod(n) => self.apply(Edit::Remove(n)),
            Confirm::Undeploy => self.run_action("undeploy", Action::Undeploy),
            Confirm::RestoreSave(g) => self.restore_save(g),
            Confirm::Rollback(n) => self.rollback_generation(n),
            Confirm::DeleteProfile(_) => self.delete_active_profile(),
        }
    }

    /// Delete the active profile (whole directory) and reselect a sibling.
    fn delete_active_profile(&mut self) {
        let Some(dir) = self.profiles.get(self.profile_idx).map(|p| p.dir.clone()) else {
            return;
        };
        match profiles::delete_profile(&dir) {
            Ok(()) => {
                self.selected = None;
                self.profile_idx = self.profile_idx.saturating_sub(1);
                self.reload_profiles();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Roll the declaration back to a recorded generation (Nix-style).
    fn rollback_generation(&mut self, number: u64) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        match concierge::generations::rollback(&repo, number) {
            Ok(_) => {
                self.undo.clear();
                let _ = self
                    .log_tx
                    .send(format!("rolled back to generation {number}"));
                self.reload_plan();
            }
            Err(e) => self.error = Some(format!("rollback: {e}")),
        }
    }

    /// Declaration generations (Nix-style version control) with rollback.
    /// Setup-versions list — projected: the pinned-versions status + each version
    /// + its roll-back button (the `rollback:<n>` transition).
    fn generations_panel(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        let screen = self.screen();
        if let Some(p) = &screen.pin_status {
            ui.weak(p.as_str());
            ui.separator();
        }
        let versions = screen.versions.clone();
        ui.horizontal(|ui| {
            ui.strong("setup versions");
            ui.weak(format!("{}", versions.len()));
        });
        if versions.is_empty() {
            ui.weak("Each Apply saves a version of your Setup you can restore.");
            return;
        }
        let rows: Vec<(concierge_ui::VersionRow, Option<concierge_ui::Transition>)> = versions
            .iter()
            .map(|v| {
                (
                    v.clone(),
                    screen
                        .transitions
                        .iter()
                        .find(|t| t.id == format!("rollback:{}", v.number))
                        .cloned(),
                )
            })
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("gens")
            .max_height(120.0)
            .show(ui, |ui| {
                for (v, tr) in &rows {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("gen {}", v.number));
                        ui.weak(&v.hash);
                        if let Some(tr) = tr {
                            self.transition_button(ui, tr);
                        }
                    });
                }
            });
    }

    fn details_panel(
        &self,
        ui: &mut eframe::egui::Ui,
        mods: &[Mod],
        per_mod: &std::collections::BTreeMap<String, usize>,
    ) {
        use eframe::egui;
        ui.strong("details");
        ui.separator();
        let Some(name) = &self.selected else {
            ui.label("click a mod's name in the list to see its details");
            return;
        };
        let Some(m) = mods.iter().find(|m| &m.name == name) else {
            ui.label("(not found)");
            return;
        };
        egui::Grid::new("det").num_columns(2).show(ui, |ui| {
            ui.label("name");
            ui.label(&m.name);
            ui.end_row();
            ui.label("version");
            ui.label(&m.version);
            ui.end_row();
            ui.label("enabled");
            ui.label(if m.enabled { "yes" } else { "no" });
            ui.end_row();
            ui.label("source");
            ui.label(mod_source(m));
            ui.end_row();
            if let Some(id) = m.nexus_mod_id {
                ui.label("nexus");
                ui.label(format!("{id} / {}", m.nexus_file_id.unwrap_or(0)));
                ui.end_row();
            }
            if let Some(u) = &m.url {
                ui.label("url");
                ui.label(u);
                ui.end_row();
            }
            ui.label("md5");
            ui.label(if m.md5.is_empty() {
                "UNPINNED".to_owned()
            } else {
                m.md5.clone()
            });
            ui.end_row();
            ui.label("files");
            ui.label(
                per_mod
                    .get(&m.name)
                    .map_or_else(|| "not deployed".to_owned(), |c| format!("{c} deployed")),
            );
            ui.end_row();
        });
        // This mod's declared requirements (framework/script-extender/other) —
        // shown in context, not only in the separate relations section
        // (i4c2: "mod details show no requirements/dependencies").
        if let Some(man) = &self.manifest {
            let reqs: Vec<&concierge::manifest::Requirement> = man
                .relations
                .requires
                .iter()
                .filter(|r| r.name == *name)
                .collect();
            if !reqs.is_empty() {
                ui.separator();
                ui.strong("requires");
                for r in reqs {
                    let v = r
                        .min_version
                        .as_deref()
                        .map_or_else(String::new, |v| format!(" \u{2265} {v}"));
                    ui.label(format!("\u{2022} {}{v}", r.needs));
                }
            }
        }
        if !m.plugins.is_empty() {
            ui.separator();
            ui.strong("config · provides (plugins)");
            for p in &m.plugins {
                ui.monospace(p);
            }
        }
        if !m.choices.is_empty() {
            ui.separator();
            ui.strong("config · install choices");
            for ch in &m.choices {
                ui.label(format!("{} ({})", ch.name, ch.select));
                for opt in &ch.options {
                    ui.monospace(format!(
                        "  [{}] {}",
                        if opt.selected { "x" } else { " " },
                        opt.label
                    ));
                }
            }
        }
        if !m.options.is_empty() {
            ui.separator();
            ui.strong("config · options");
            for (k, v) in &m.options {
                ui.monospace(format!("{k} = {v}"));
            }
        }
        let meta = &m.meta;
        let has_meta = meta.author.is_some()
            || meta.category.is_some()
            || !meta.tags.is_empty()
            || meta.nsfw
            || meta.license.is_some()
            || meta.website.is_some();
        if has_meta || m.group.is_some() || !m.mirrors.is_empty() || m.sha512.is_some() {
            ui.separator();
            ui.strong("metadata & trust");
            if let Some(a) = &meta.author {
                ui.label(format!("author: {a}"));
            }
            if let Some(c) = &meta.category {
                ui.label(format!("category: {c}"));
            }
            if !meta.tags.is_empty() {
                ui.label(format!("tags: {}", meta.tags.join(", ")));
            }
            if meta.nsfw {
                ui.colored_label(egui::Color32::from_rgb(220, 130, 130), "NSFW");
            }
            if let Some(l) = &meta.license {
                ui.label(format!("license: {l}"));
            }
            if let Some(g) = &m.group {
                ui.label(format!("group: {g}"));
            }
            if !m.mirrors.is_empty() {
                ui.label(format!("mirrors: {}", m.mirrors.len()));
            }
            if m.sha512.is_some() {
                ui.label("sha512: pinned");
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
mod tests {
    use super::{order_delta_lines, preferred_adapter, sync_bar};

    // A hermetic App showing a Fallout 4 pack: two data mods + F4SE (a promoted
    // tool, install_root = "game"). No disk/network — manifest + plan seeded
    // directly, so egui_kittest can drive the real render. (dossier item 2)
    fn fallout4_app() -> super::App {
        concierge_games::register();
        let toml = concat!(
            "[game]\n",
            "kind = \"fallout4\"\n",
            "pristine = \"/tmp/cg-kittest\"\n",
            "instance = \"/tmp/cg-kittest/inst\"\n",
            "version = \"1.10.163\"\n",
            "[game.paths]\n",
            "plugins_txt = \"/tmp/cg-kittest/plugins.txt\"\n",
            "my_games = \"/tmp/cg-kittest/mg\"\n\n",
            "[[mod]]\nname = \"alpha-textures\"\nversion = \"1.0\"\nnexus_mod_id = 111\nfile = \"alpha.zip\"\n\n",
            "[[mod]]\nname = \"F4SE\"\nversion = \"0.6.23\"\ninstall_root = \"game\"\nurl = \"https://example.invalid/f4se.7z\"\nfile = \"f4se.7z\"\n\n",
            "[[mod]]\nname = \"bravo-textures\"\nversion = \"1.0\"\nnexus_mod_id = 222\nfile = \"bravo.zip\"\n",
        );
        let manifest = concierge::manifest::Manifest::parse(toml).expect("fixture manifest parses");
        let plan = concierge::plan::eval(&manifest).expect("fixture plan evals");
        let mut app = super::App::new();
        app.manifest = Some(manifest);
        app.plan = Some(plan);
        app.quickstart_open = false; // a configured pack, not first-run
        app
    }

    // The agent terminal now builds the sandbox command in-process and spawns it
    // DIRECTLY (no concierge.exe subprocess). Verify the argv/env extraction that
    // feeds the PTY yields a runnable sandbox launcher carrying the MOTD — the
    // exact path start_agent_terminal takes.
    #[test]
    fn direct_spawn_extracts_a_runnable_sandbox_command() {
        concierge_games::register();
        let base = std::env::temp_dir().join(format!("cg-gui-shell-{}", std::process::id()));
        let profile = base.join("games/fallout4/profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(base.join(".concierge-workspace"), "").unwrap();
        let toml = concat!(
            "[game]\nkind = \"fallout4\"\npristine = \"/tmp/cg-x\"\nversion = \"1\"\n",
            "[game.paths]\nplugins_txt = \"/tmp/cg-x/p.txt\"\nmy_games = \"/tmp/cg-x/mg\"\n",
        );
        let manifest = concierge::manifest::Manifest::parse(toml).unwrap();
        let plan = concierge::plan::eval(&manifest).unwrap();
        let repo = concierge::repo::Repo::at(&profile);
        let c = concierge::shell::shell_command(&repo, &plan, None, false, &[], &[]).unwrap();
        let program: Vec<String> = std::iter::once(c.get_program())
            .chain(c.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let env: Vec<(String, String)> = c
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert!(
            program.first().is_some_and(|p| p.contains("sandbox-exec")
                || p.contains("bwrap")
                || p.to_lowercase().contains("powershell")),
            "leads with the OS sandbox launcher: {program:?}"
        );
        assert!(
            env.iter().any(|(k, _)| k == "CONCIERGE_MOTD"),
            "the MOTD env is carried to the PTY: {env:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn kittest_renders_the_pack_through_the_real_egui_tree() {
        use egui_kittest::kittest::Queryable as _;
        let app = fallout4_app();
        let mut h = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1280.0, 800.0))
            .build_state(|ctx, a: &mut super::App| a.update_ctx(ctx), app);
        h.run_steps(6);
        // egui actually built each mod as a widget — semantic proof the render
        // happened. A blank/black frame or a dropped row fails HERE, unlike the
        // text golden which never touches egui.
        h.get_by_label("alpha-textures");
        h.get_by_label("bravo-textures");
        h.get_by_label("F4SE");
        // …and the Foundational-tools slot heading rendered — proof the promoted
        // tool is segregated into its own section, not left in the mod list.
        h.get_by_label_contains("Foundational tools");
    }

    #[test]
    fn kittest_selecting_a_mod_updates_the_details_panel() {
        // Interaction regression guard: a real click on a mod's name drives the
        // selection and re-renders the details panel — click → state change →
        // re-render, all through the actual egui tree. (The pointer-driven ComboBox
        // "filter click closes the popup" bug needs real pointer events, so it's
        // verified in the live box — dossier item 3 — not synthetic clicks.)
        use egui_kittest::kittest::Queryable as _;
        let app = fallout4_app();
        let mut h = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1280.0, 800.0))
            .build_state(|ctx, a: &mut super::App| a.update_ctx(ctx), app);
        h.run_steps(4);
        // the details panel shows its placeholder until a mod is selected
        h.get_by_label_contains("click a mod");
        // click a mod's name → selection changes → the details panel re-renders
        h.get_by_label("alpha-textures").click();
        h.run_steps(4);
        assert!(
            h.query_by_label_contains("click a mod").is_none(),
            "details placeholder gone after selecting a mod (click drove a re-render)"
        );
    }

    #[test]
    fn kittest_quickstart_shows_on_first_run() {
        // An empty App (no workspace) lands on the Welcome screen and renders the
        // quick-start guide: the steps + the Setup-vs-Installed explainer.
        use egui_kittest::kittest::Queryable as _;
        concierge_games::register();
        let app = super::App::new();
        let mut h = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1280.0, 800.0))
            .build_state(|ctx, a: &mut super::App| a.update_ctx(ctx), app);
        h.run_steps(4);
        // ("Quick start" is intentionally not asserted — it matches both the
        // panel heading and the top-bar toggle; the step text below is unique.)
        h.get_by_label_contains("Add your game");
        h.get_by_label_contains("Create a modpack");
        h.get_by_label_contains("Setup vs Installed");
    }

    #[test]
    fn kittest_wgpu_frame_actually_paints() {
        // Render the real frame through wgpu and assert it is NOT blank — the
        // in-process black-frame regression test, and a check that the wgpu path
        // renders (Metal locally, llvmpipe/lavapipe in CI). No committed pixel
        // baseline: cross-platform pixel diffs are brittle, so we assert "it
        // painted" via grayscale variance instead of an exact image.
        //
        // Opt-in via CONCIERGE_WGPU_TESTS so a machine with no usable wgpu adapter
        // (bare headless CI without lavapipe) never fails on it — CI sets the var
        // after installing Mesa/lavapipe; it runs there and locally on demand.
        if std::env::var_os("CONCIERGE_WGPU_TESTS").is_none() {
            eprintln!("skipping wgpu frame test (set CONCIERGE_WGPU_TESTS=1 to run)");
            return;
        }
        let app = fallout4_app();
        let mut h = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1280.0, 800.0))
            .wgpu()
            .build_state(|ctx, a: &mut super::App| a.update_ctx(ctx), app);
        h.run_steps(6);
        let img = h.render().expect("wgpu render");
        // stddev of the RGB byte values (skip alpha): a real UI is high-variance,
        // a black/blank frame is ~0.
        let vals: Vec<f64> = img
            .as_raw()
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 4 != 3)
            .map(|(_, b)| f64::from(*b))
            .collect();
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        let stddev = (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
        assert!(
            stddev > 5.0,
            "wgpu frame looks blank (grayscale stddev={stddev})"
        );
    }
    use eframe::wgpu::Backend;

    #[test]
    fn order_delta_reports_added_removed_and_resequenced() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        // Added + removed plugins, common order preserved.
        let out = order_delta_lines(&s(&["A.esp", "B.esp"]), &s(&["A.esp", "C.esp"]));
        assert!(out.iter().any(|l| l.contains("+ C.esp")), "added: {out:?}");
        assert!(
            out.iter().any(|l| l.contains("- B.esp")),
            "removed: {out:?}"
        );
        assert!(
            !out.iter().any(|l| l.contains("resequenced")),
            "A stayed put: {out:?}"
        );
        // Same set, order flipped → resequenced note, no add/remove lines.
        let out2 = order_delta_lines(&s(&["A.esp", "B.esp"]), &s(&["B.esp", "A.esp"]));
        assert!(
            out2.iter().any(|l| l.contains("resequenced")),
            "reorder detected: {out2:?}"
        );
        assert!(
            !out2
                .iter()
                .any(|l| l.starts_with("   +") || l.starts_with("   -")),
            "no phantom add/remove: {out2:?}"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn sync_bar_parses_counts_percent_and_eta() {
        // Initial one-time message has no counts yet → indeterminate (None).
        assert!(sync_bar("Downloading the mod list… (one-time, …)", 0).is_none());
        // Mid-sync: 3660 of 73005 rows after 30s → ~5%, ETA in minutes.
        let (frac, label) =
            sync_bar("Downloading the mod list… page   47   3660/73005 rows", 30).unwrap();
        assert!((frac - 0.05).abs() < 0.02, "fraction ~5%, got {frac}");
        assert!(label.starts_with("3660 / 73005 mods · 5%"), "got: {label}");
        assert!(label.contains("min left"), "should show an ETA: {label}");
        // Complete: full bar, no ETA suffix.
        let (frac, label) = sync_bar("… 73005/73005 rows", 900).unwrap();
        assert!((frac - 1.0).abs() < f32::EPSILON, "full bar, got {frac}");
        assert!(!label.contains("left"), "no ETA at 100%: {label}");
    }

    #[test]
    fn prefers_vulkan_over_dx12() {
        // The CrossOver/Wine case: both DX12 and Vulkan enumerate; DX12
        // composites blank, Vulkan renders — pick Vulkan.
        assert_eq!(
            preferred_adapter(&[Backend::Dx12, Backend::Vulkan]),
            Some(1)
        );
        assert_eq!(
            preferred_adapter(&[Backend::Vulkan, Backend::Dx12]),
            Some(0)
        );
    }

    #[test]
    fn falls_through_to_metal_then_dx12() {
        // macOS: no Vulkan adapter, Metal is chosen.
        assert_eq!(preferred_adapter(&[Backend::Metal]), Some(0));
        // Native Windows with no Vulkan driver: DX12 is the fallback.
        assert_eq!(preferred_adapter(&[Backend::Dx12, Backend::Gl]), Some(0));
        assert_eq!(preferred_adapter(&[Backend::Gl]), Some(0));
    }

    #[test]
    fn empty_yields_none() {
        assert_eq!(preferred_adapter(&[]), None);
    }

    #[test]
    fn full_preference_order() {
        let all = [Backend::Gl, Backend::Dx12, Backend::Metal, Backend::Vulkan];
        assert_eq!(preferred_adapter(&all), Some(3)); // Vulkan wins
        let no_vk = [Backend::Gl, Backend::Dx12, Backend::Metal];
        assert_eq!(preferred_adapter(&no_vk), Some(2)); // Metal next
        let no_vk_metal = [Backend::Gl, Backend::Dx12];
        assert_eq!(preferred_adapter(&no_vk_metal), Some(1)); // Dx12 next
    }
}
