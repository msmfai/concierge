//! Concierge's agent-facing UI model: one pure, serializable description of the
//! screen that BOTH renderers consume — the egui GUI (for humans) and the text
//! / state-automaton renderer (for an AI driving the app headlessly).
//!
//! The principle is *one view-model, two renderers, zero drift*. The egui front
//! end builds [`UiFacts`] from its state each frame, turns it into a [`Screen`]
//! via [`build_screen`], and renders that; the headless driver builds the same
//! [`Screen`] and renders it with [`render_text`]. Because the set of legal
//! actions (the automaton's transitions, with their guards) is computed HERE,
//! the two views cannot disagree about what state the app is in or what is
//! clickable — a drift guard asserts exactly that.
//!
//! This crate is deliberately pure: no egui, no I/O, no clocks, no randomness —
//! so a `Screen` is a deterministic function of the facts, and golden snapshots
//! are stable.

use serde::Serialize;

/// The central content tab (was the Nix-flavoured `Declaration`/`Realised`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Tab {
    /// The editable plan — "Setup".
    Setup,
    /// What's deployed on disk — "Installed".
    Installed,
}

impl Tab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Installed => "Installed",
        }
    }
}

/// A pending confirmation dialog (mirrors the GUI's `Confirm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConfirmKind {
    RemoveMod,
    Uninstall,
    RestoreSave,
    Rollback,
    DeleteProfile,
}

/// The app's current automaton state — a single node chosen from the (possibly
/// overlapping) mode flags by a fixed priority: modal dialogs first, then the
/// busy/AI activity, then the base edit/locked view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum UiState {
    /// No workspace resolved (no games) — nothing to edit yet.
    NoWorkspace,
    /// A confirmation dialog is up (which one is in [`Screen::banner`]).
    Confirming(ConfirmKind),
    /// The Settings window is open.
    Settings,
    /// The catalog Browse window is open.
    Browse,
    /// The background download-manager window is open.
    Downloads,
    /// The "Preview changes" (diff) window is open.
    PreviewChanges,
    /// An action (Apply/Download/…) is running off-thread.
    Applying,
    /// The AI assistant is mid-turn.
    AiRunning,
    /// Read-only: the setup is Locked (can't be edited).
    Locked,
    /// The normal editable state.
    Editing,
}

impl UiState {
    /// Stable, greppable name for the automaton view and `assert state=…`.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::NoWorkspace => "NoWorkspace".into(),
            Self::Confirming(k) => format!("Confirming{k:?}"),
            Self::Settings => "Settings".into(),
            Self::Browse => "Browse".into(),
            Self::Downloads => "Downloads".into(),
            Self::PreviewChanges => "PreviewChanges".into(),
            Self::Applying => "Applying".into(),
            Self::AiRunning => "AiRunning".into(),
            Self::Locked => "Locked".into(),
            Self::Editing => "Editing".into(),
        }
    }
}

/// Defines [`Intent`], its stable string [`id`](Intent::id), and the exhaustive
/// [`Intent::ALL`] list from a SINGLE source — so the enum, its ids, and the
/// closed action alphabet cannot drift from each other. Adding an action means
/// adding one line here, and it automatically appears in the vocabulary both
/// views validate against.
macro_rules! intents {
    ($($variant:ident => $id:literal),+ $(,)?) => {
        /// An intent a widget fires — the id half of the automaton's alphabet. The
        /// GUI maps each to its behaviour; the text driver matches on the id.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        pub enum Intent { $($variant),+ }

        impl Intent {
            /// Stable id used in scripts (`click apply`) and the drift guard.
            #[must_use]
            pub const fn id(self) -> &'static str {
                match self { $(Self::$variant => $id),+ }
            }
            /// Every intent — the fixed half of the closed action vocabulary.
            pub const ALL: &'static [Intent] = &[$(Intent::$variant),+];
        }
    };
}

intents! {
    Download => "download",
    Apply => "apply",
    Verify => "verify",
    Play => "play",
    Uninstall => "uninstall",
    SortLoad => "sort_load",
    Requirements => "requirements",
    FindPatches => "find_patches",
    MergeConflicts => "merge_conflicts",
    Conflicts => "conflicts",
    ToggleLock => "toggle_lock",
    Undo => "undo",
    OpenSettings => "open_settings",
    CloseSettings => "close_settings",
    OpenBrowse => "open_browse",
    CloseBrowse => "close_browse",
    OpenDownloads => "open_downloads",
    CloseDownloads => "close_downloads",
    PauseAllDownloads => "dl_pause_all",
    ResumeAllDownloads => "dl_resume_all",
    ClearDownloads => "dl_clear",
    OpenPreview => "open_preview",
    ClosePreview => "close_preview",
    SelectSetupTab => "tab_setup",
    SelectInstalledTab => "tab_installed",
    ConfirmYes => "confirm_yes",
    ConfirmNo => "confirm_no",
    CheckAccount => "check_account",
    BrowseSearch => "browse_search",
    NxmAdd => "nxm_add",
    AiSend => "ai_send",
    AiInterrupt => "ai_interrupt",
    AiWork => "ai_work",
    Rescan => "rescan",
    DeleteProfile => "delete_profile",
    CreateEmpty => "create_empty",
    CreateClone => "create_clone",
    NewModpackAi => "new_modpack_ai",
    // chrome / panels (migrated from hand-rendered controls so the agent view
    // drives them too)
    LogClear => "log_clear",
    OpenQuickstart => "open_quickstart",
    ToggleQuickstart => "toggle_quickstart",
    ToggleTheme => "toggle_theme",
    ToggleAi => "toggle_ai",
    OpenDownloadSession => "open_download_session",
    OpenShell => "open_shell",
    AgentStop => "agent_stop",
    AgentClose => "agent_close",
    SyncCatalog => "sync_catalog",
    BrowseClear => "browse_clear",
    // Settings actions
    DetectSteam => "detect_steam",
    SetInstallFolder => "set_install_folder",
    SaveCompat => "save_compat",
    Enable1Click => "enable_1click",
    NexusGetKey => "nexus_get_key",
    NexusSaveKey => "nexus_save_key",
    UpdateCheck => "update_check",
    UpdateInstall => "update_install",
}

/// The parameterised `<prefix>:<arg>` action ids (raw transitions the GUI builds
/// dynamically — a mod row, a catalog hit, a download job). Together with every
/// [`Intent::id`], these are the COMPLETE, CLOSED action alphabet — the single
/// vocabulary both views share, so a control one view offers and the other can't
/// is impossible.
pub const RAW_ACTION_PREFIXES: &[&str] = &[
    "add_hit:",
    "add_game:",
    "select_game:",
    "select_profile:",
    "mod_toggle:",
    "mod_select:",
    "mod_up:",
    "mod_down:",
    "mod_remove:",
    "mod_move:",
    "rollback:",
    "restore_save:",
    "ai_quick:",
    "dl_pause:",
    "dl_resume:",
    "dl_cancel:",
    "open_page:",
    "browse_cat:",
    "browse_sort:",
];

/// Fixed action ids that aren't [`Intent`]s (rendered via `raw(...)`): the
/// add-mod form's open/confirm buttons.
pub const FIXED_RAW_IDS: &[&str] = &["add_open", "add_confirm"];

/// Is `id` a member of the closed action vocabulary — a fixed [`Intent`] id, a
/// fixed raw id, or a known parameterised prefix? Both views validate every
/// dispatched/clicked id against this, so neither can act on an id the other
/// doesn't know.
#[must_use]
pub fn is_action_id(id: &str) -> bool {
    Intent::ALL.iter().any(|i| i.id() == id)
        || FIXED_RAW_IDS.contains(&id)
        || RAW_ACTION_PREFIXES.iter().any(|p| id.starts_with(p))
}

/// A catalog search result — id-keyed so its "add" button (`add_hit:<mod_id>`)
/// can be dispatched and driven headlessly. Carries the Nexus-card fields so
/// the GUI can render a mod page-like card and the headless view can describe
/// it richly; the download action is replaced by "add to manifest".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowseHit {
    pub mod_id: u64,
    pub name: String,
    pub endorsements: u64,
    pub author: String,
    pub summary: String,
    pub category: String,
    pub downloads: u64,
    pub updated_at: String,
    /// Already declared in the manifest — the card shows "added" instead of
    /// an add button.
    pub added: bool,
}

/// A saved Setup version (generation) — its "roll back" button is `rollback:<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionRow {
    pub number: u64,
    pub hash: String,
}

/// A mod in the guided Download-session that still needs its Nexus page opened —
/// projected so the agent can drive `open_page:<mod_id>:<file_id>` too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeededPage {
    pub mod_id: u64,
    pub file_id: u64,
    pub name: String,
}

/// One row in the background download manager — projected so BOTH views see the
/// same queue. Its controls are `dl_pause:<id>` / `dl_resume:<id>` / `dl_cancel:<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadRow {
    pub id: u64,
    pub name: String,
    /// Lower-case lifecycle: `downloading` | `paused` | `done` | `cancelled` |
    /// `failed`. The GUI colours it; the text view prints it verbatim.
    pub state: String,
    pub done: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
}

/// The ids the egui GUI renders as the main action-bar row (as opposed to
/// chrome: tabs, lock, settings). The GUI renders this row FROM the screen so
/// its enabled/label/hover can't drift from the automaton.
pub const ACTION_BAR: &[&str] = &[
    "download",
    "apply",
    "sort_load",
    "requirements",
    "find_patches",
    "merge_conflicts",
    "conflicts",
    "play",
    "uninstall",
];

/// Is this transition id part of the main action-bar row?
#[must_use]
pub fn is_action_bar(id: &str) -> bool {
    ACTION_BAR.contains(&id)
}

/// One row of the mod list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModRow {
    pub order: usize,
    pub name: String,
    pub enabled: bool,
}

/// A text input the agent can `type` into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub value: String,
}

/// A read-only info region of a window (Settings paths, Preview change summary…).
/// Content is derived from app state; both renderers show it, so they can't
/// disagree about what a window displays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// What kind of control a transition renders as — so the projecting renderer
/// draws it faithfully (a tab and a toggle show selection; a button doesn't).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WidgetKind {
    Button,
    Tab,
    Toggle,
}

/// A legal transition out of the current state — the automaton's edge, and the
/// widget the GUI renders for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transition {
    pub id: String,
    pub label: String,
    pub kind: WidgetKind,
    /// Whether this control shows as selected (a Tab for the current tab; the
    /// lock Toggle when Locked).
    pub selected: bool,
    pub enabled: bool,
    /// Why it's disabled, when it is (shown to the agent).
    pub guard: Option<String>,
    /// A one-line "what it does" tooltip — the egui GUI shows this when it
    /// renders the button FROM this transition, so the rich hovers survive.
    pub hover: Option<String>,
    /// The state this leads to, when statically known.
    pub target: Option<String>,
}

/// The pure, per-frame description of the screen — the single source of truth
/// both renderers consume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Screen {
    pub state: UiState,
    pub title: String,
    /// Status lines (workspace, game/profile, active tab).
    pub status: Vec<String>,
    /// An error or confirmation banner, if any.
    pub banner: Option<String>,
    pub tab: Tab,
    pub mods: Vec<ModRow>,
    pub fields: Vec<Field>,
    /// The download-manager queue (present when the Downloads window is open).
    pub downloads: Vec<DownloadRow>,
    pub browse_hits: Vec<BrowseHit>,
    pub versions: Vec<VersionRow>,
    pub saves: Vec<String>,
    pub pin_status: Option<String>,
    pub games: Vec<String>,
    pub profiles: Vec<String>,
    pub game_idx: usize,
    pub profile_idx: usize,
    /// Read-only info regions for the visible window(s).
    pub panels: Vec<Panel>,
    /// The legal transitions from [`Screen::state`] — the automaton edges.
    pub transitions: Vec<Transition>,
    pub log_tail: Vec<String>,
}

/// The minimal snapshot of GUI state needed to build a [`Screen`]. The egui App
/// fills this from its fields each frame; the headless driver fills it from a
/// booted profile. Keep it pure data — no egui, no handles.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // a flat facts snapshot of GUI mode flags
pub struct UiFacts {
    pub has_workspace: bool,
    pub workspace_path: Option<String>,
    pub game_count: usize,
    pub active_game: Option<String>,
    pub active_profile: Option<String>,
    /// Whether a modpack/profile is actually open (an active repo AND a loaded
    /// plan) — the precondition every heavy action shares. When false, those
    /// actions are projected disabled with a "no modpack open" guard, so an
    /// action that needs a profile can't be invoked without one (in either
    /// renderer). Mirrors `run_action`'s runtime `(Some(repo), Some(plan))` gate.
    pub has_active_profile: bool,
    pub is_bethesda: bool,
    pub has_catalog: bool,
    pub tab: TabFacts,
    pub mutable: bool,
    pub has_undo: bool,
    pub busy: bool,
    pub ai_busy: bool,
    pub settings_open: bool,
    pub browse_open: bool,
    /// The background download-manager window is open.
    pub downloads_open: bool,
    /// The manager's global pause is engaged.
    pub downloads_paused_all: bool,
    /// The live download queue (projected so both views show/drive it).
    pub downloads: Vec<DownloadRow>,
    /// The embedded agent terminal is active / present (gates stop/close).
    pub agent_running: bool,
    pub agent_present: bool,
    /// The updater found a newer release (gates install vs check).
    pub update_available: bool,
    /// Browse-window filter options (projected so the agent can pick them):
    /// category names and sort-order labels.
    pub browse_categories: Vec<String>,
    pub browse_sorts: Vec<String>,
    /// Mods in the guided Download-session still needing a Nexus page opened.
    pub needed_pages: Vec<NeededPage>,
    /// Game kinds that can be added (the "+ add game" menu) — `add_game:<kind>`.
    pub addable_games: Vec<String>,
    pub diff_open: bool,
    pub confirm: Option<ConfirmKind>,
    pub confirm_prompt: Option<String>,
    pub error: Option<String>,
    pub mods: Vec<ModRow>,
    pub log_tail: Vec<String>,
    /// The active game's word for the ordered plugin list ("load order"…).
    pub order_word: String,
    /// The active game's sort-action label ("Sort load order"…).
    pub sort_label: String,
    pub nxm_input: String,
    pub search: String,
    pub browse_query: String,
    pub browse_msg: String,
    pub browse_hits: Vec<BrowseHit>,
    pub add_open: bool,
    /// The add-mod form's text fields (id/label/value), when the form is open.
    pub add_fields: Vec<Field>,
    /// Saved Setup versions (rollback) + save backups (restore) + the lock status.
    pub versions: Vec<VersionRow>,
    pub saves: Vec<String>,
    pub pin_status: Option<String>,
    /// AI assistant panel: prompt value, whether it can act, quick-action labels.
    pub ai_input: String,
    pub ai_can_send: bool,
    pub ai_quick: Vec<String>,
    /// Top bar: game/profile lists + selection, new-profile field, cache summary.
    pub games: Vec<String>,
    pub profiles: Vec<String>,
    pub game_idx: usize,
    pub profile_idx: usize,
    pub new_profile: String,
    pub cache_summary: String,
    /// Read-only info panels for the visible window(s), built from app state.
    pub panels: Vec<Panel>,
}

/// `Tab` isn't `Default`; wrap it so `UiFacts` can derive `Default`.
#[derive(Debug, Clone, Copy)]
pub struct TabFacts(pub Tab);

impl Default for TabFacts {
    fn default() -> Self {
        Self(Tab::Setup)
    }
}

/// Choose the single automaton state from the (overlapping) mode flags, by a
/// fixed priority: modal dialogs win, then activity, then the base view.
#[must_use]
pub const fn ui_state(f: &UiFacts) -> UiState {
    if !f.has_workspace || f.game_count == 0 {
        return UiState::NoWorkspace;
    }
    if let Some(k) = f.confirm {
        return UiState::Confirming(k);
    }
    if f.settings_open {
        return UiState::Settings;
    }
    if f.browse_open {
        return UiState::Browse;
    }
    if f.downloads_open {
        return UiState::Downloads;
    }
    if f.diff_open {
        return UiState::PreviewChanges;
    }
    if f.busy {
        return UiState::Applying;
    }
    if f.ai_busy {
        return UiState::AiRunning;
    }
    if f.mutable {
        UiState::Editing
    } else {
        UiState::Locked
    }
}

fn t(intent: Intent, label: impl Into<String>, enabled: bool, guard: Option<&str>) -> Transition {
    Transition {
        id: intent.id().to_owned(),
        label: label.into(),
        kind: WidgetKind::Button,
        selected: false,
        enabled,
        guard: guard.map(str::to_owned),
        hover: None,
        target: None,
    }
}

/// A tab control (shows selection).
fn tab(intent: Intent, label: impl Into<String>, selected: bool) -> Transition {
    Transition {
        kind: WidgetKind::Tab,
        selected,
        ..t(intent, label, true, None)
    }
}

/// A toggle control (shows selection).
fn toggle(intent: Intent, label: impl Into<String>, selected: bool) -> Transition {
    Transition {
        kind: WidgetKind::Toggle,
        selected,
        ..t(intent, label, true, None)
    }
}

fn to(mut tr: Transition, target: &str) -> Transition {
    tr.target = Some(target.to_owned());
    tr
}

/// A button with a raw (non-[`Intent`]) id — for dynamic ids like
/// `add_hit:<mod_id>` / `mod_up:<name>`.
fn raw(id: &str, label: impl Into<String>, enabled: bool, guard: Option<&str>) -> Transition {
    Transition {
        id: id.to_owned(),
        label: label.into(),
        kind: WidgetKind::Button,
        selected: false,
        enabled,
        guard: guard.map(str::to_owned),
        hover: None,
        target: None,
    }
}

fn hv(mut tr: Transition, hover: &str) -> Transition {
    tr.hover = Some(hover.to_owned());
    tr
}

const BUSY: &str = "an action is running";
const LOCKED: &str = "switch to Edit mode";
const NO_PROFILE: &str = "open a modpack first";

/// The action bar + view controls available in the base (editable/locked/busy)
/// states. Guards mirror the GUI exactly so the drift guard holds.
#[allow(clippy::too_many_lines)] // a flat declarative table, one entry per action
fn base_transitions(f: &UiFacts) -> Vec<Transition> {
    let busy = f.busy;
    // A modpack must be open before any heavy action can run — that guard wins
    // over busy/locked, since without a profile there's nothing to act on. Fold
    // it into every profile-requiring entry so the disabled state is projected
    // (and honored by both renderers), never a silent no-op at click time.
    let has_profile = f.has_active_profile;
    let no_profile = (!has_profile).then_some(NO_PROFILE);
    // `enabled` for a plain, profile-requiring, busy-gated action.
    let act_ok = has_profile && !busy;
    let guard_busy = no_profile.or_else(|| busy.then_some(BUSY));
    let mut v = vec![
        hv(t(Intent::Download, "Download", act_ok, guard_busy), "download the mods your setup needs (queues manual downloads for free accounts)"),
        hv(t(Intent::Apply, "Apply", act_ok, guard_busy), "install your Setup into the game — backs up saves, then downloads, builds, and deploys"),
        hv(t(Intent::Verify, "Verify", act_ok, guard_busy), "check the game still matches your Setup"),
    ];
    if f.is_bethesda {
        let edit_ok = has_profile && f.mutable && !busy;
        let sort_guard = if !has_profile {
            Some(NO_PROFILE)
        } else if busy {
            Some(BUSY)
        } else if !f.mutable {
            Some(LOCKED)
        } else {
            None
        };
        v.push(hv(
            t(Intent::SortLoad, f.sort_label.clone(), edit_ok, sort_guard),
            "auto-sort the load order (LOOT) and record it",
        ));
        v.push(hv(
            t(Intent::Requirements, "Requirements", edit_ok, sort_guard),
            "find what each mod needs (masters/frameworks) and flag anything missing",
        ));
        v.push(hv(
            t(Intent::FindPatches, "Find patches", act_ok, guard_busy),
            "find compatibility patches for conflicting mods",
        ));
        v.push(hv(
            t(
                Intent::MergeConflicts,
                "Merge conflicts",
                act_ok,
                guard_busy,
            ),
            "auto-merge mods that edit the same lists (leveled lists)",
        ));
        v.push(hv(
            t(Intent::Conflicts, "Conflicts", act_ok, guard_busy),
            "see which mod wins when two change the same thing",
        ));
    }
    v.push(hv(
        t(Intent::Play, "Play", act_ok, guard_busy),
        "launch the game",
    ));
    v.push(hv(
        to(
            t(Intent::Uninstall, "Uninstall", act_ok, guard_busy),
            "ConfirmingUninstall",
        ),
        "remove the installed mods from the game (leaves your Setup intact)",
    ));
    // the lock Toggle shows the current MODE (a state, not an action) and is
    // "selected" when Locked. "Editing"/"Locked" reads as state; "Edit" alone
    // looked like an edit *menu* (i3c6).
    let lock_label = if f.mutable { "Editing" } else { "Locked" };
    let lock_hover = if f.mutable {
        "You can change this modpack. Click to lock it (make it read-only)."
    } else {
        "This modpack is read-only. Click to unlock and edit it."
    };
    v.push(hv(
        toggle(Intent::ToggleLock, lock_label, !f.mutable),
        lock_hover,
    ));
    let undo_guard = if !has_profile {
        Some(NO_PROFILE)
    } else if !f.mutable {
        Some(LOCKED)
    } else if !f.has_undo {
        Some("nothing to undo")
    } else {
        None
    };
    v.push(t(
        Intent::Undo,
        "undo",
        has_profile && f.mutable && f.has_undo,
        undo_guard,
    ));
    // add-mod form (rendered in the Setup tab header, not the action bar row).
    let add_label = if f.add_open {
        "− add mod"
    } else {
        "+ add mod"
    };
    v.push(raw(
        "add_open",
        add_label,
        has_profile && f.mutable,
        no_profile.or_else(|| (!f.mutable).then_some(LOCKED)),
    ));
    if f.add_open {
        v.push(raw(
            "add_confirm",
            "add to manifest",
            has_profile && f.mutable,
            no_profile.or_else(|| (!f.mutable).then_some(LOCKED)),
        ));
    }
    // AI assistant panel (side column; rendered by the GUI in its own place).
    if f.ai_busy {
        v.push(t(Intent::AiInterrupt, "stop", true, None));
    } else {
        let send_guard = if f.ai_can_send {
            None
        } else {
            Some("no active profile")
        };
        v.push(t(Intent::AiSend, "send", f.ai_can_send, send_guard));
        v.push(t(
            Intent::AiWork,
            "Work on this profile",
            f.ai_can_send,
            send_guard,
        ));
        for (i, label) in f.ai_quick.iter().enumerate() {
            v.push(raw(&format!("ai_quick:{i}"), label.clone(), true, None));
        }
    }
    // top bar: rescan, delete/create profile, per-game/profile select.
    v.push(t(Intent::Rescan, "rescan", true, None));
    if !f.profiles.is_empty() {
        v.push(t(Intent::DeleteProfile, "delete profile", true, None));
    }
    let named = !f.new_profile.trim().is_empty();
    let name_guard = (!named).then_some("name the profile first");
    v.push(t(Intent::CreateEmpty, "empty", named, name_guard));
    v.push(t(Intent::CreateClone, "clone active", named, name_guard));
    v.push(t(
        Intent::NewModpackAi,
        "new modpack (AI)",
        named && !f.ai_busy,
        if f.ai_busy { Some(BUSY) } else { name_guard },
    ));
    // NOTE: game + profile are IDENTITY (which pack am I on) and live in the
    // top-bar dropdowns; they are deliberately NOT rendered as tabs here. The tab
    // row is only the Setup/Installed VIEW toggle, so "which pack" and "which
    // view" stop reading as the same control.
    // per-row buttons for the setup-versions + save-backup lists.
    for ver in &f.versions {
        v.push(raw(
            &format!("rollback:{}", ver.number),
            "roll back",
            has_profile && f.mutable,
            no_profile.or_else(|| (!f.mutable).then_some(LOCKED)),
        ));
    }
    for g in &f.saves {
        v.push(raw(
            &format!("restore_save:{g}"),
            "restore",
            has_profile && !f.busy,
            no_profile.or_else(|| f.busy.then_some(BUSY)),
        ));
    }
    v.push(to(
        t(Intent::OpenSettings, "Settings", true, None),
        "Settings",
    ));
    v.push(to(
        t(Intent::OpenPreview, "Preview", true, None),
        "PreviewChanges",
    ));
    v.push(to(
        hv(
            t(Intent::OpenDownloads, "\u{2b07} Downloads", true, None),
            "the background download manager (queue, speeds, pause/cancel)",
        ),
        "Downloads",
    ));
    if f.has_catalog {
        v.push(to(
            hv(
                t(Intent::OpenBrowse, "🔎 browse", true, None),
                "search the Nexus catalog and add mods",
            ),
            "Browse",
        ));
    }
    // Chrome + panel actions (projected so the agent view drives them too).
    v.push(t(Intent::ToggleTheme, "toggle theme", true, None));
    v.push(t(Intent::ToggleAi, "toggle AI panel", true, None));
    v.push(t(Intent::ToggleQuickstart, "quick start", true, None));
    v.push(t(
        Intent::OpenQuickstart,
        "open quick-start guide",
        true,
        None,
    ));
    v.push(t(Intent::LogClear, "clear log", true, None));
    v.push(t(
        Intent::OpenDownloadSession,
        "download session",
        true,
        None,
    ));
    // Top-bar identity selectors (which game/profile am I on) + add-game menu.
    for (i, g) in f.games.iter().enumerate() {
        v.push(raw(
            &format!("select_game:{i}"),
            format!("game: {g}"),
            true,
            None,
        ));
    }
    for (i, p) in f.profiles.iter().enumerate() {
        v.push(raw(
            &format!("select_profile:{i}"),
            format!("profile: {p}"),
            true,
            None,
        ));
    }
    for k in &f.addable_games {
        v.push(raw(
            &format!("add_game:{k}"),
            format!("add game: {k}"),
            true,
            None,
        ));
    }
    for p in &f.needed_pages {
        v.push(raw(
            &format!("open_page:{}:{}", p.mod_id, p.file_id),
            format!("open Nexus page: {}", p.name),
            true,
            None,
        ));
    }
    // Per-mod row actions (Setup tab) — projected so the agent reorders/toggles
    // mods exactly as a human clicks the row buttons. Edits require Edit mode.
    if matches!(f.tab.0, Tab::Setup) {
        let edit_guard = (!f.mutable).then_some(LOCKED);
        for (i, m) in f.mods.iter().enumerate() {
            v.push(raw(
                &format!("mod_select:{}", m.name),
                format!("select {}", m.name),
                true,
                None,
            ));
            v.push(raw(
                &format!("mod_toggle:{}", m.name),
                format!("toggle {}", m.name),
                f.mutable,
                edit_guard,
            ));
            v.push(raw(
                &format!("mod_up:{}", m.name),
                format!("move {} up", m.name),
                f.mutable,
                edit_guard,
            ));
            v.push(raw(
                &format!("mod_down:{}", m.name),
                format!("move {} down", m.name),
                f.mutable,
                edit_guard,
            ));
            v.push(raw(
                &format!("mod_remove:{}", m.name),
                format!("remove {}", m.name),
                f.mutable,
                edit_guard,
            ));
            if i > 0 {
                v.push(raw(
                    &format!("mod_move:{i}:0"),
                    format!("move {} to top", m.name),
                    f.mutable,
                    edit_guard,
                ));
            }
        }
    }
    // Embedded agent terminal.
    if f.agent_running {
        v.push(t(Intent::AgentStop, "stop agent", true, None));
    } else if f.agent_present {
        v.push(t(Intent::AgentClose, "close agent terminal", true, None));
    } else {
        v.push(t(Intent::OpenShell, "open sandboxed shell", true, None));
    }
    // the two central Tabs — selection follows the active tab.
    let setup = matches!(f.tab.0, Tab::Setup);
    v.push(hv(
        tab(Intent::SelectSetupTab, "📝 Setup", setup),
        "the mods you want — your editable plan",
    ));
    v.push(hv(
        tab(Intent::SelectInstalledTab, "Installed", !setup),
        "what's actually deployed to the game right now — read-only",
    ));
    v
}

/// The legal transitions from a state — the automaton's edges.
#[must_use]
#[allow(clippy::too_many_lines)] // a flat per-state transition table
pub fn transitions(f: &UiFacts, state: UiState) -> Vec<Transition> {
    match state {
        UiState::NoWorkspace => Vec::new(),
        UiState::Confirming(_) => vec![
            to(t(Intent::ConfirmYes, "Confirm", true, None), "Editing"),
            to(t(Intent::ConfirmNo, "Cancel", true, None), "Editing"),
        ],
        UiState::Settings => vec![
            t(Intent::CheckAccount, "Check account", true, None),
            t(Intent::NexusGetKey, "Get my API key", true, None),
            t(Intent::NexusSaveKey, "Save & sign in", true, None),
            t(Intent::Enable1Click, "Enable 1-click downloads", true, None),
            t(Intent::DetectSteam, "Detect via Steam", true, None),
            t(Intent::SetInstallFolder, "Set install folder", true, None),
            t(Intent::SaveCompat, "Save compatibility", true, None),
            if f.update_available {
                t(
                    Intent::UpdateInstall,
                    "Download & install update",
                    true,
                    None,
                )
            } else {
                t(Intent::UpdateCheck, "Check for updates", true, None)
            },
            to(
                t(Intent::CloseSettings, "Close settings", true, None),
                "Editing",
            ),
        ],
        UiState::Browse => {
            let nxm_ok = f.mutable && f.nxm_input.starts_with("nxm://");
            let nxm_guard = if !f.mutable {
                Some(LOCKED)
            } else if !f.nxm_input.starts_with("nxm://") {
                Some("paste an nxm:// link")
            } else {
                None
            };
            let mut v = vec![
                t(Intent::BrowseSearch, "search", true, None),
                t(Intent::NxmAdd, "add nxm", nxm_ok, nxm_guard),
                t(Intent::BrowseClear, "clear filters", true, None),
                t(Intent::SyncCatalog, "sync catalog", true, None),
            ];
            v.push(raw("browse_cat:", "category: all", true, None));
            for c in &f.browse_categories {
                v.push(raw(
                    &format!("browse_cat:{c}"),
                    format!("category: {c}"),
                    true,
                    None,
                ));
            }
            for s in &f.browse_sorts {
                v.push(raw(
                    &format!("browse_sort:{s}"),
                    format!("sort: {s}"),
                    true,
                    None,
                ));
            }
            for h in &f.browse_hits {
                let by = if h.author.is_empty() {
                    String::new()
                } else {
                    format!(" by {}", h.author)
                };
                let cat = if h.category.is_empty() {
                    String::new()
                } else {
                    format!(", {}", h.category)
                };
                let label = if h.added {
                    format!("✓ added: {}{by}", h.name)
                } else {
                    format!("+ add {}{by} (▲{}{cat})", h.name, h.endorsements)
                };
                v.push(raw(
                    &format!("add_hit:{}", h.mod_id),
                    label,
                    f.mutable && !h.added,
                    if h.added {
                        Some("already in the manifest")
                    } else {
                        (!f.mutable).then_some(LOCKED)
                    },
                ));
            }
            v.push(to(
                t(Intent::CloseBrowse, "Close browse", true, None),
                "Editing",
            ));
            v
        }
        // The background download manager: global controls + a per-row
        // pause/resume/cancel projected for every job, so the agent can drive the
        // queue exactly as a human does.
        UiState::Downloads => {
            let mut v = Vec::new();
            if f.downloads_paused_all {
                v.push(t(Intent::ResumeAllDownloads, "Resume all", true, None));
            } else {
                v.push(t(Intent::PauseAllDownloads, "Pause all", true, None));
            }
            v.push(t(Intent::ClearDownloads, "Clear finished", true, None));
            for d in &f.downloads {
                match d.state.as_str() {
                    "downloading" => {
                        v.push(raw(
                            &format!("dl_pause:{}", d.id),
                            format!("pause {}", d.name),
                            true,
                            None,
                        ));
                        v.push(raw(
                            &format!("dl_cancel:{}", d.id),
                            format!("cancel {}", d.name),
                            true,
                            None,
                        ));
                    }
                    "paused" => {
                        v.push(raw(
                            &format!("dl_resume:{}", d.id),
                            format!("resume {}", d.name),
                            true,
                            None,
                        ));
                        v.push(raw(
                            &format!("dl_cancel:{}", d.id),
                            format!("cancel {}", d.name),
                            true,
                            None,
                        ));
                    }
                    _ => {}
                }
            }
            v.push(to(
                t(Intent::CloseDownloads, "Close downloads", true, None),
                "Editing",
            ));
            v
        }
        // The preview is a read-only diff summary (the `preview` panel); it
        // closes via the window's X, which projects the close_preview transition.
        UiState::PreviewChanges => {
            vec![to(
                t(Intent::ClosePreview, "Close preview", true, None),
                "Editing",
            )]
        }
        // base states share the action bar; when busy the actions are guarded.
        UiState::Applying | UiState::AiRunning | UiState::Locked | UiState::Editing => {
            base_transitions(f)
        }
    }
}

/// Build the [`Screen`] from facts — the single source of truth. Deterministic.
#[must_use]
#[allow(clippy::too_many_lines)] // linear projection of every fact onto the screen
pub fn build_screen(f: &UiFacts) -> Screen {
    let state = ui_state(f);
    let mut status = Vec::new();
    if let Some(w) = &f.workspace_path {
        status.push(format!("workspace: {w} ({} games)", f.game_count));
    }
    match (&f.active_game, &f.active_profile) {
        (Some(g), Some(p)) => status.push(format!("game: {g} · profile: {p}")),
        (Some(g), None) => status.push(format!("game: {g}")),
        _ => {}
    }
    status.push(format!(
        "mode: {} · tab: {}",
        if f.mutable { "Editing" } else { "Locked" },
        f.tab.0.label()
    ));

    let banner = f
        .confirm_prompt
        .clone()
        .or_else(|| f.error.clone().map(|e| format!("error: {e}")));

    let mut fields = vec![Field {
        id: "search".into(),
        label: "filter mods".into(),
        value: f.search.clone(),
    }];
    if matches!(state, UiState::Browse) {
        fields.push(Field {
            id: "browse_query".into(),
            label: "catalog search".into(),
            value: f.browse_query.clone(),
        });
    }
    if matches!(state, UiState::Browse) || !f.nxm_input.is_empty() {
        fields.push(Field {
            id: "nxm_input".into(),
            label: "nxm:// link".into(),
            value: f.nxm_input.clone(),
        });
    }

    if f.add_open {
        fields.extend(f.add_fields.iter().cloned());
    }
    if !f.ai_busy {
        fields.push(Field {
            id: "ai_input".into(),
            label: "ask the assistant".into(),
            value: f.ai_input.clone(),
        });
    }
    fields.push(Field {
        id: "new_profile".into(),
        label: "new profile".into(),
        value: f.new_profile.clone(),
    });

    let mut panels = f.panels.clone();
    if matches!(state, UiState::Browse) && !f.browse_msg.is_empty() {
        panels.push(Panel {
            id: "browse".into(),
            title: "Browse".into(),
            lines: vec![f.browse_msg.clone()],
        });
    }
    if matches!(state, UiState::Editing | UiState::Locked) {
        let mut vlines = Vec::new();
        if let Some(p) = &f.pin_status {
            vlines.push(p.clone());
        }
        vlines.push(format!("setup versions: {}", f.versions.len()));
        for ver in &f.versions {
            vlines.push(format!("gen {} {}", ver.number, ver.hash));
        }
        panels.push(Panel {
            id: "versions".into(),
            title: "Setup versions".into(),
            lines: vlines,
        });
        let mut slines = vec![format!("save backups: {}", f.saves.len())];
        slines.extend(f.saves.iter().cloned());
        panels.push(Panel {
            id: "saves".into(),
            title: "Save backups".into(),
            lines: slines,
        });
    }

    Screen {
        state,
        title: "Concierge".into(),
        status,
        banner,
        tab: f.tab.0,
        mods: f.mods.clone(),
        fields,
        downloads: f.downloads.clone(),
        browse_hits: f.browse_hits.clone(),
        versions: f.versions.clone(),
        saves: f.saves.clone(),
        pin_status: f.pin_status.clone(),
        games: f.games.clone(),
        profiles: f.profiles.clone(),
        game_idx: f.game_idx,
        profile_idx: f.profile_idx,
        panels,
        transitions: transitions(f, state),
        log_tail: f.log_tail.clone(),
    }
}

/// Render a [`Screen`] as pure text for an agent: an ASCII screen plus a
/// state-automaton block (STATE + guarded TRANSITIONS). Deterministic and
/// stable — safe for golden snapshots.
#[must_use]
pub fn render_text(s: &Screen) -> String {
    use core::fmt::Write as _;
    let mut o = String::new();
    let line = "+----------------------------------------------------------------+\n";
    o.push_str(line);
    let _ = writeln!(o, "| {:<62} |", s.title);
    o.push_str(line);
    for st in &s.status {
        let _ = writeln!(o, "| {:<62} |", truncate(st, 62));
    }
    if let Some(b) = &s.banner {
        o.push_str(line);
        let _ = writeln!(o, "| ! {:<60} |", truncate(b, 60));
    }
    o.push_str(line);

    if !s.mods.is_empty() {
        o.push_str("MODS:\n");
        for m in &s.mods {
            let mark = if m.enabled { "[x]" } else { "[ ]" };
            let _ = writeln!(o, "  {:>2}. {mark} {}", m.order, m.name);
        }
    }
    if !s.fields.is_empty() {
        o.push_str("FIELDS:\n");
        for fld in &s.fields {
            let _ = writeln!(o, "  {} = \"{}\"  ({})", fld.id, fld.value, fld.label);
        }
    }
    if !s.downloads.is_empty() {
        o.push_str("DOWNLOADS:\n");
        for d in &s.downloads {
            let total = d.total.map_or_else(|| "?".to_owned(), |t| t.to_string());
            let _ = writeln!(
                o,
                "  #{} [{}] {} — {}/{} bytes @ {}/s",
                d.id, d.state, d.name, d.done, total, d.bytes_per_sec
            );
        }
    }
    for p in &s.panels {
        let _ = writeln!(o, "PANEL {} — {}:", p.id, p.title);
        for l in &p.lines {
            let _ = writeln!(o, "  {}", truncate(l, 60));
        }
    }
    if !s.log_tail.is_empty() {
        o.push_str("LOG:\n");
        for l in &s.log_tail {
            let _ = writeln!(o, "  {}", truncate(l, 60));
        }
    }

    // The automaton block — designed for the agent to grep + act on.
    o.push_str("====\n");
    let _ = writeln!(o, "STATE: {}", s.state.name());
    o.push_str("TRANSITIONS:\n");
    for tr in &s.transitions {
        let status = if tr.enabled {
            "enabled".to_owned()
        } else {
            format!("guarded: {}", tr.guard.as_deref().unwrap_or("unavailable"))
        };
        let arrow = tr
            .target
            .as_deref()
            .map_or(String::new(), |g| format!("  -> {g}"));
        let sel = if tr.selected { "*" } else { " " };
        let _ = writeln!(o, " {sel}{:<16} {:<24} [{status}]{arrow}", tr.id, tr.label);
    }
    o
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// The set of enabled transition ids in a screen — the drift guard compares
/// this against the actions the egui GUI actually wires this frame.
#[must_use]
pub fn enabled_intents(s: &Screen) -> Vec<String> {
    let mut v: Vec<String> = s
        .transitions
        .iter()
        .filter(|t| t.enabled)
        .map(|t| t.id.clone())
        .collect();
    v.sort();
    v
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn base_facts() -> UiFacts {
        UiFacts {
            has_workspace: true,
            workspace_path: Some("/ws".into()),
            game_count: 2,
            active_game: Some("skyrimse".into()),
            active_profile: Some("modpack".into()),
            has_active_profile: true,
            is_bethesda: true,
            has_catalog: true,
            mutable: true,
            order_word: "load order".into(),
            sort_label: "Sort load order".into(),
            mods: vec![ModRow {
                order: 1,
                name: "SkyUI".into(),
                enabled: true,
            }],
            ..UiFacts::default()
        }
    }

    #[test]
    fn no_workspace_has_no_transitions() {
        let f = UiFacts::default();
        assert_eq!(ui_state(&f), UiState::NoWorkspace);
        assert!(transitions(&f, ui_state(&f)).is_empty());
    }

    #[test]
    fn editing_offers_the_action_bar_with_load_order_wording() {
        let f = base_facts();
        assert_eq!(ui_state(&f), UiState::Editing);
        let s = build_screen(&f);
        let ids: Vec<&str> = s.transitions.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"apply"));
        assert!(ids.contains(&"sort_load"));
        assert!(s
            .transitions
            .iter()
            .any(|t| t.id == "sort_load" && t.label.contains("Sort load order")));
    }

    #[test]
    fn locked_disables_edit_only_actions() {
        let mut f = base_facts();
        f.mutable = false;
        let s = build_screen(&f);
        assert_eq!(s.state, UiState::Locked);
        let sort = s.transitions.iter().find(|t| t.id == "sort_load").unwrap();
        assert!(!sort.enabled);
        assert_eq!(sort.guard.as_deref(), Some(LOCKED));
        // apply is still available in Locked
        assert!(s.transitions.iter().any(|t| t.id == "apply" && t.enabled));
    }

    #[test]
    fn busy_guards_everything_actionable() {
        let mut f = base_facts();
        f.busy = true;
        let s = build_screen(&f);
        assert_eq!(s.state, UiState::Applying);
        let apply = s.transitions.iter().find(|t| t.id == "apply").unwrap();
        assert!(!apply.enabled);
        assert_eq!(apply.guard.as_deref(), Some(BUSY));
    }

    #[test]
    fn confirm_dialog_offers_only_yes_no() {
        let mut f = base_facts();
        f.confirm = Some(ConfirmKind::Uninstall);
        f.confirm_prompt = Some("Uninstall — remove the installed mods?".into());
        let s = build_screen(&f);
        assert_eq!(s.state, UiState::Confirming(ConfirmKind::Uninstall));
        let ids: Vec<&str> = s.transitions.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["confirm_yes", "confirm_no"]);
        assert!(s.banner.as_deref().unwrap().contains("Uninstall"));
    }

    #[test]
    fn no_active_profile_disables_every_heavy_action() {
        // The guardrail: with no modpack open, the profile-requiring actions are
        // projected DISABLED with the "open a modpack first" guard — so neither
        // renderer can invoke one without a profile (no silent no-op at click).
        let mut f = base_facts();
        f.has_active_profile = false;
        let s = build_screen(&f);
        for id in [
            "download",
            "apply",
            "verify",
            "play",
            "uninstall",
            "sort_load",
        ] {
            let tr = s
                .transitions
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("missing transition {id}"));
            assert!(!tr.enabled, "{id} must be disabled with no profile");
            assert_eq!(
                tr.guard.as_deref(),
                Some(NO_PROFILE),
                "{id} must carry the no-profile guard"
            );
        }
        // ...but profile-agnostic chrome stays available.
        assert!(s
            .transitions
            .iter()
            .any(|t| t.id == "open_settings" && t.enabled));
        assert!(s.transitions.iter().any(|t| t.id == "rescan" && t.enabled));
    }

    #[test]
    fn non_bethesda_hides_bethesda_actions() {
        let mut f = base_facts();
        f.is_bethesda = false;
        let s = build_screen(&f);
        assert!(!s.transitions.iter().any(|t| t.id == "sort_load"));
        assert!(!s.transitions.iter().any(|t| t.id == "conflicts"));
    }

    #[test]
    fn render_text_shows_state_and_transitions() {
        let s = build_screen(&base_facts());
        let txt = render_text(&s);
        assert!(txt.contains("STATE: Editing"));
        assert!(txt.contains("TRANSITIONS:"));
        assert!(txt.contains("apply"));
        assert!(txt.contains("[enabled]"));
        assert!(txt.contains("SkyUI"));
    }

    #[test]
    fn enabled_intents_is_sorted_and_stable() {
        let s = build_screen(&base_facts());
        let a = enabled_intents(&s);
        let mut b = a.clone();
        b.sort();
        assert_eq!(a, b);
    }

    // --- drift guard: the GUI renders its action bar FROM these transitions, so
    // every ACTION_BAR id must be a real transition (a typo would render nothing
    // in the GUI while the automaton still listed it — silent drift).
    #[test]
    fn action_bar_ids_are_all_real_transitions() {
        let s = build_screen(&base_facts());
        let ids: std::collections::BTreeSet<&str> =
            s.transitions.iter().map(|t| t.id.as_str()).collect();
        for id in ACTION_BAR {
            assert!(
                ids.contains(id),
                "ACTION_BAR id '{id}' is not a real transition"
            );
        }
    }

    #[test]
    fn action_bar_items_carry_hovers() {
        let s = build_screen(&base_facts());
        for t in s.transitions.iter().filter(|t| is_action_bar(&t.id)) {
            assert!(t.hover.is_some(), "action-bar '{}' has no hover", t.id);
        }
    }

    #[test]
    fn build_screen_is_a_pure_function_of_facts() {
        let f = base_facts();
        assert_eq!(build_screen(&f), build_screen(&f));
    }

    // The closed-vocabulary contract: the view-model can NEVER project a
    // transition whose id isn't in the shared action alphabet — otherwise a
    // control could exist that the machine view can't name. Checked across every
    // reachable window/mode.
    #[test]
    fn every_projected_transition_is_a_known_action() {
        let mut variants = vec![base_facts()];
        // toggle each window/mode so all states are exercised
        for mutate in [
            |f: &mut UiFacts| f.settings_open = true,
            |f: &mut UiFacts| f.browse_open = true,
            |f: &mut UiFacts| f.downloads_open = true,
            |f: &mut UiFacts| f.diff_open = true,
            |f: &mut UiFacts| f.busy = true,
            |f: &mut UiFacts| f.mutable = false,
            |f: &mut UiFacts| f.has_active_profile = false,
            |f: &mut UiFacts| f.confirm = Some(ConfirmKind::Uninstall),
        ] {
            let mut f = base_facts();
            mutate(&mut f);
            variants.push(f);
        }
        // populate the dynamic rows so their raw ids are generated too
        for f in &mut variants {
            f.downloads = vec![DownloadRow {
                id: 1,
                name: "m".into(),
                state: "downloading".into(),
                done: 0,
                total: None,
                bytes_per_sec: 0,
            }];
            f.versions = vec![VersionRow {
                number: 1,
                hash: "h".into(),
            }];
            f.saves = vec!["s".into()];
        }
        for f in &variants {
            for tr in build_screen(f).transitions {
                assert!(
                    is_action_id(&tr.id),
                    "transition '{}' is not in the closed action vocabulary",
                    tr.id
                );
            }
        }
    }

    // The REVERSE contract: every action in the vocabulary is projected as a
    // transition in SOME reachable state — so the GUI can never dispatch an
    // action the agent view has no way to reach. Together with the forward
    // contract, the two views' action sets are provably equal.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn every_vocabulary_action_is_projected_somewhere() {
        // A rich facts set that lights up every window/mode + dynamic row.
        let mut all_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let states: Vec<UiFacts> = {
            let mut base = base_facts();
            base.has_catalog = true;
            base.browse_categories = vec!["Gameplay".into()];
            base.browse_sorts = vec!["downloads".into()];
            base.browse_hits = vec![BrowseHit {
                mod_id: 9,
                name: "SkyUI".into(),
                endorsements: 1,
                author: "a".into(),
                summary: String::new(),
                category: String::new(),
                downloads: 0,
                updated_at: String::new(),
                added: false,
            }];
            base.downloads = vec![DownloadRow {
                id: 1,
                name: "m".into(),
                state: "downloading".into(),
                done: 0,
                total: None,
                bytes_per_sec: 0,
            }];
            base.needed_pages = vec![NeededPage {
                mod_id: 1,
                file_id: 2,
                name: "n".into(),
            }];
            base.versions = vec![VersionRow {
                number: 1,
                hash: "h".into(),
            }];
            base.saves = vec!["s".into()];
            base.has_undo = true;
            base.ai_quick = vec!["fix".into()];
            base.games = vec!["g".into()];
            base.profiles = vec!["p".into()];
            base.addable_games = vec!["skyrimse".into()];
            base.new_profile = "x".into();
            base.add_open = true;
            base.mods = vec![
                ModRow {
                    order: 1,
                    name: "A".into(),
                    enabled: true,
                },
                ModRow {
                    order: 2,
                    name: "B".into(),
                    enabled: true,
                },
            ];
            let mut v = Vec::new();
            for open in [
                None,
                Some("settings"),
                Some("browse"),
                Some("downloads"),
                Some("confirm"),
                Some("preview"),
                Some("agent_running"),
                Some("agent_present"),
                Some("update"),
                Some("paused_dl"),
                Some("ai_busy"),
            ] {
                let mut f = base.clone();
                match open {
                    Some("ai_busy") => f.ai_busy = true,
                    Some("settings") => f.settings_open = true,
                    Some("browse") => f.browse_open = true,
                    Some("downloads") => f.downloads_open = true,
                    Some("confirm") => f.confirm = Some(ConfirmKind::Uninstall),
                    Some("preview") => f.diff_open = true,
                    Some("agent_running") => f.agent_running = true,
                    Some("agent_present") => f.agent_present = true,
                    Some("update") => {
                        f.settings_open = true;
                        f.update_available = true;
                    }
                    Some("paused_dl") => {
                        f.downloads_open = true;
                        f.downloads_paused_all = true;
                        f.downloads = vec![DownloadRow {
                            id: 1,
                            name: "m".into(),
                            state: "paused".into(),
                            done: 0,
                            total: None,
                            bytes_per_sec: 0,
                        }];
                    }
                    _ => {}
                }
                v.push(f);
            }
            v
        };
        for f in &states {
            for tr in build_screen(f).transitions {
                all_ids.insert(tr.id);
            }
        }
        // Every fixed Intent must appear.
        for i in Intent::ALL {
            let want = i.id();
            let hit = all_ids.contains(want) || all_ids.iter().any(|got| got == want);
            assert!(
                hit,
                "Intent '{want}' is in the vocabulary but never projected"
            );
        }
        // Every raw prefix must appear on at least one projected id.
        for pre in RAW_ACTION_PREFIXES {
            assert!(
                all_ids.iter().any(|id| id.starts_with(pre)),
                "raw action prefix '{pre}' is never projected"
            );
        }
        for id in FIXED_RAW_IDS {
            assert!(
                all_ids.contains(*id),
                "fixed raw id '{id}' is never projected"
            );
        }
    }

    #[test]
    fn intent_ids_are_unique_and_complete() {
        let ids: std::collections::BTreeSet<&str> = Intent::ALL.iter().map(|i| i.id()).collect();
        assert_eq!(ids.len(), Intent::ALL.len(), "duplicate Intent id");
        // spot-check the macro wired id() to ALL correctly
        assert!(is_action_id("download") && is_action_id("dl_pause_all"));
        assert!(is_action_id("dl_pause:7") && is_action_id("mod_toggle:x"));
        assert!(!is_action_id("totally_made_up") && !is_action_id("set_bandwidth"));
    }

    // Drift guard for the download manager: the whole surface (open, the global
    // controls, per-job controls, the observable queue) must be in the view-model,
    // so the headless/agent view drives exactly what the GUI does.
    #[test]
    fn download_manager_is_fully_projected_and_drivable() {
        let mut f = base_facts();
        // Entry point exists from the base state.
        assert!(build_screen(&f)
            .transitions
            .iter()
            .any(|t| t.id == "open_downloads"));
        // Open it → Downloads state with a live queue.
        f.downloads_open = true;
        f.downloads = vec![DownloadRow {
            id: 7,
            name: "SkyUI".into(),
            state: "downloading".into(),
            done: 50,
            total: Some(100),
            bytes_per_sec: 2048,
        }];
        let s = build_screen(&f);
        assert_eq!(s.state, UiState::Downloads);
        let ids: std::collections::BTreeSet<&str> =
            s.transitions.iter().map(|t| t.id.as_str()).collect();
        for id in [
            "dl_pause_all",
            "dl_clear",
            "dl_pause:7",
            "dl_cancel:7",
            "close_downloads",
        ] {
            assert!(ids.contains(id), "download control '{id}' not projected");
        }
        // The queue is observable in the text (agent) view.
        let txt = render_text(&s);
        assert!(
            txt.contains("DOWNLOADS:") && txt.contains("SkyUI"),
            "queue not in text view"
        );
        // Global pause flips pause-all → resume-all; a paused job offers resume.
        f.downloads_paused_all = true;
        f.downloads = vec![DownloadRow {
            id: 7,
            name: "SkyUI".into(),
            state: "paused".into(),
            done: 50,
            total: Some(100),
            bytes_per_sec: 0,
        }];
        let s2 = build_screen(&f);
        let ids2: std::collections::BTreeSet<&str> =
            s2.transitions.iter().map(|t| t.id.as_str()).collect();
        assert!(ids2.contains("dl_resume_all"));
        assert!(ids2.contains("dl_resume:7"));
    }
}
