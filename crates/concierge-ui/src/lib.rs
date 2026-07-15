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
            Self::PreviewChanges => "PreviewChanges".into(),
            Self::Applying => "Applying".into(),
            Self::AiRunning => "AiRunning".into(),
            Self::Locked => "Locked".into(),
            Self::Editing => "Editing".into(),
        }
    }
}

/// An intent a widget fires — the id half of the automaton's alphabet. The GUI
/// maps each to its concrete behaviour; the text driver matches on the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Intent {
    Download,
    Apply,
    Verify,
    Play,
    Uninstall,
    SortLoad,
    Requirements,
    FindPatches,
    MergeConflicts,
    Conflicts,
    ToggleLock,
    Undo,
    OpenSettings,
    CloseSettings,
    OpenBrowse,
    CloseBrowse,
    OpenPreview,
    ClosePreview,
    SelectSetupTab,
    SelectInstalledTab,
    ConfirmYes,
    ConfirmNo,
    CheckAccount,
    BrowseSearch,
    NxmAdd,
    AiSend,
    AiInterrupt,
    AiWork,
    Rescan,
    DeleteProfile,
    CreateEmpty,
    CreateClone,
    NewModpackAi,
}

impl Intent {
    /// Stable id used in scripts (`click apply`) and the drift guard.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Apply => "apply",
            Self::Verify => "verify",
            Self::Play => "play",
            Self::Uninstall => "uninstall",
            Self::SortLoad => "sort_load",
            Self::Requirements => "requirements",
            Self::FindPatches => "find_patches",
            Self::MergeConflicts => "merge_conflicts",
            Self::Conflicts => "conflicts",
            Self::ToggleLock => "toggle_lock",
            Self::Undo => "undo",
            Self::OpenSettings => "open_settings",
            Self::CloseSettings => "close_settings",
            Self::OpenBrowse => "open_browse",
            Self::CloseBrowse => "close_browse",
            Self::OpenPreview => "open_preview",
            Self::ClosePreview => "close_preview",
            Self::SelectSetupTab => "tab_setup",
            Self::SelectInstalledTab => "tab_installed",
            Self::ConfirmYes => "confirm_yes",
            Self::ConfirmNo => "confirm_no",
            Self::CheckAccount => "check_account",
            Self::BrowseSearch => "browse_search",
            Self::NxmAdd => "nxm_add",
            Self::AiSend => "ai_send",
            Self::AiInterrupt => "ai_interrupt",
            Self::AiWork => "ai_work",
            Self::Rescan => "rescan",
            Self::DeleteProfile => "delete_profile",
            Self::CreateEmpty => "create_empty",
            Self::CreateClone => "create_clone",
            Self::NewModpackAi => "new_modpack_ai",
        }
    }
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
    pub is_bethesda: bool,
    pub has_catalog: bool,
    pub tab: TabFacts,
    pub mutable: bool,
    pub has_undo: bool,
    pub busy: bool,
    pub ai_busy: bool,
    pub settings_open: bool,
    pub browse_open: bool,
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

/// A raw-id Tab (shows selection) — for the game/profile selectors.
fn raw_tab(id: &str, label: impl Into<String>, selected: bool) -> Transition {
    Transition {
        kind: WidgetKind::Tab,
        selected,
        ..raw(id, label, true, None)
    }
}

fn hv(mut tr: Transition, hover: &str) -> Transition {
    tr.hover = Some(hover.to_owned());
    tr
}

const BUSY: &str = "an action is running";
const LOCKED: &str = "switch to Edit mode";

/// The action bar + view controls available in the base (editable/locked/busy)
/// states. Guards mirror the GUI exactly so the drift guard holds.
#[allow(clippy::too_many_lines)] // a flat declarative table, one entry per action
fn base_transitions(f: &UiFacts) -> Vec<Transition> {
    let busy = f.busy;
    let guard_busy = busy.then_some(BUSY);
    let mut v = vec![
        hv(t(Intent::Download, "Download", !busy, guard_busy), "download the mods your setup needs (queues manual downloads for free accounts)"),
        hv(t(Intent::Apply, "Apply", !busy, guard_busy), "install your Setup into the game — backs up saves, then downloads, builds, and deploys"),
        hv(t(Intent::Verify, "Verify", !busy, guard_busy), "check the game still matches your Setup"),
    ];
    if f.is_bethesda {
        let edit_ok = f.mutable && !busy;
        let sort_guard = if busy {
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
            t(Intent::FindPatches, "Find patches", !busy, guard_busy),
            "find compatibility patches for conflicting mods",
        ));
        v.push(hv(
            t(Intent::MergeConflicts, "Merge conflicts", !busy, guard_busy),
            "auto-merge mods that edit the same lists (leveled lists)",
        ));
        v.push(hv(
            t(Intent::Conflicts, "Conflicts", !busy, guard_busy),
            "see which mod wins when two change the same thing",
        ));
    }
    v.push(hv(
        t(Intent::Play, "Play", !busy, guard_busy),
        "launch the game",
    ));
    v.push(hv(
        to(
            t(Intent::Uninstall, "Uninstall", !busy, guard_busy),
            "ConfirmingUninstall",
        ),
        "remove the installed mods from the game (leaves your Setup intact)",
    ));
    // the lock Toggle shows the current mode and is "selected" when Locked.
    let lock_label = if f.mutable { "Edit" } else { "Locked" };
    v.push(hv(
        toggle(Intent::ToggleLock, lock_label, !f.mutable),
        "Locked mods can't be changed — switch to Edit to modify your setup",
    ));
    let undo_guard = if !f.mutable {
        Some(LOCKED)
    } else if !f.has_undo {
        Some("nothing to undo")
    } else {
        None
    };
    v.push(t(Intent::Undo, "undo", f.mutable && f.has_undo, undo_guard));
    // add-mod form (rendered in the Setup tab header, not the action bar row).
    let add_label = if f.add_open {
        "− add mod"
    } else {
        "+ add mod"
    };
    v.push(raw(
        "add_open",
        add_label,
        f.mutable,
        (!f.mutable).then_some(LOCKED),
    ));
    if f.add_open {
        v.push(raw(
            "add_confirm",
            "add to manifest",
            f.mutable,
            (!f.mutable).then_some(LOCKED),
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
    for (i, g) in f.games.iter().enumerate() {
        v.push(raw_tab(
            &format!("select_game:{i}"),
            g.clone(),
            i == f.game_idx,
        ));
    }
    for (i, p) in f.profiles.iter().enumerate() {
        v.push(raw_tab(
            &format!("select_profile:{i}"),
            p.clone(),
            i == f.profile_idx,
        ));
    }
    // per-row buttons for the setup-versions + save-backup lists.
    for ver in &f.versions {
        v.push(raw(
            &format!("rollback:{}", ver.number),
            "roll back",
            f.mutable,
            (!f.mutable).then_some(LOCKED),
        ));
    }
    for g in &f.saves {
        v.push(raw(
            &format!("restore_save:{g}"),
            "restore",
            !f.busy,
            f.busy.then_some(BUSY),
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
    if f.has_catalog {
        v.push(to(
            hv(
                t(Intent::OpenBrowse, "🔎 browse", true, None),
                "search the Nexus catalog and add mods",
            ),
            "Browse",
        ));
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
pub fn transitions(f: &UiFacts, state: UiState) -> Vec<Transition> {
    match state {
        UiState::NoWorkspace => Vec::new(),
        UiState::Confirming(_) => vec![
            to(t(Intent::ConfirmYes, "Confirm", true, None), "Editing"),
            to(t(Intent::ConfirmNo, "Cancel", true, None), "Editing"),
        ],
        UiState::Settings => vec![
            t(Intent::CheckAccount, "Check account", true, None),
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
            ];
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
        if f.mutable { "Edit" } else { "Locked" },
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
}
