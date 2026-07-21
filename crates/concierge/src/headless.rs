//! Headless driver for the Concierge GUI: boot a profile with no window, render
//! the agent-facing text / state-automaton view (via `concierge-ui`, the same
//! view-model the egui GUI uses), and drive it with a step script — so an AI can
//! run and test the app autonomously.
//!
//! The heavy, side-effecting actions (Apply/Download/…) are NOT executed here —
//! headless records the intent and never touches the network or real game files.
//! The navigation transitions (lock, tab, open/close modals, confirm) mutate the
//! model so the automaton can be driven and asserted.

use std::io::Write as _;

use concierge::error::{Error, Result};
use concierge::repo::Repo;
use concierge_ui::{build_screen, render_text, ConfirmKind, Panel, Screen, Tab, TabFacts, UiFacts};

/// The headless UI model: the booted profile's data plus the transient UI flags
/// the automaton navigates.
#[allow(clippy::struct_excessive_bools)] // a flat model of the GUI's mode flags
pub struct Headless {
    workspace: Option<String>,
    game_count: usize,
    game_kind: Option<String>,
    profile: Option<String>,
    is_bethesda: bool,
    has_catalog: bool,
    order_word: String,
    sort_label: String,
    game_paths: Vec<(String, String)>,
    mods: Vec<concierge_ui::ModRow>,
    // transient UI flags
    mutable: bool,
    tab: Tab,
    settings_open: bool,
    browse_open: bool,
    downloads_open: bool,
    diff_open: bool,
    confirm: Option<(ConfirmKind, String)>,
    search: String,
    nxm_input: String,
    browse_query: String,
    selected: Option<String>,
    add_open: bool,
    add_values: std::collections::BTreeMap<String, String>,
    versions: Vec<concierge_ui::VersionRow>,
    saves: Vec<String>,
    pin_status: Option<String>,
    ai_input: String,
    games: Vec<String>,
    profiles: Vec<String>,
    game_idx: usize,
    profile_idx: usize,
    new_profile: String,
    live: bool,
    health_issues: Vec<String>,
    log: Vec<String>,
}

/// Read-only health check: declared relation issues + the game's own invariant
/// lints (adapter-dispatched — every plugin-order game, not a hardcoded pair).
/// No deploy, no network.
fn compute_health(manifest: &concierge::manifest::Manifest) -> Vec<String> {
    let mut issues = manifest.relation_issues();
    if let Ok(plan) = concierge::plan::eval(manifest) {
        if let Ok(violations) = concierge_lint::validate(&plan) {
            issues.extend(
                violations
                    .iter()
                    .map(|v| format!("{}: {} — {}", v.rule, v.subject, v.detail)),
            );
        }
    }
    issues
}

impl Headless {
    /// Boot from the discovered repo (`CONCIERGE_REPO` / cwd walk) — the same
    /// discovery the egui GUI uses. No side effects. In `live` mode, run the
    /// read-only health check (eval + relation issues + missing masters) so it
    /// can be asserted; still no deploy or network.
    #[allow(clippy::too_many_lines)] // linear boot sequence
    pub fn boot(live: bool) -> Result<Self> {
        let repo = Repo::discover()?;
        let manifest = concierge::manifest::Manifest::load(&repo.profile)?;
        let kind = manifest.game.kind.clone();
        // Feeds the GUI's action-gating (UiFacts.is_bethesda). "Has a plugin load
        // order with base masters" — asked of the adapter, not a hardcoded kind
        // list (which silently excluded Skyrim LE/Oblivion/FO3/NV/Starfield).
        let is_bethesda =
            concierge::game::adapter_for(&kind).is_ok_and(|a| a.plugin_bases().is_some());
        let health_issues = if live {
            compute_health(&manifest)
        } else {
            Vec::new()
        };
        let (order_word, sort_label, has_catalog) = concierge::game::adapter_for(&kind)
            .map_or_else(
                |_| ("order".to_owned(), "Sort order".to_owned(), false),
                |a| {
                    let lex = a.lexicon();
                    (
                        lex.order.to_owned(),
                        lex.sort_action.to_owned(),
                        a.nexus_domain().is_some(),
                    )
                },
            );
        let mods = manifest
            .mods
            .iter()
            .enumerate()
            .map(|(i, m)| concierge_ui::ModRow {
                order: i + 1,
                name: m.name.clone(),
                enabled: m.enabled,
            })
            .collect();
        let game_paths = manifest
            .game
            .paths
            .iter()
            .map(|(k, v)| (k.clone(), v.display().to_string()))
            .collect();
        let versions = concierge::generations::list(&repo)
            .into_iter()
            .map(|g| concierge_ui::VersionRow {
                number: g.number,
                hash: g.plan_hash.chars().take(10).collect(),
            })
            .collect();
        let saves = concierge::saves::list(&repo);
        let pin_status = concierge::lockfile::read(&repo).map(|lock| {
            format!(
                "pinned {} ({} mods)",
                lock.plan_hash.chars().take(10).collect::<String>(),
                lock.mods.len()
            )
        });
        let workspace = concierge::profiles::workspace().ok();
        let game_list = workspace
            .as_ref()
            .map(|w| concierge::profiles::list_games(w))
            .unwrap_or_default();
        let games: Vec<String> = game_list.iter().map(|g| g.game.clone()).collect();
        let game_count = games.len();
        let game_idx = games.iter().position(|g| g == &kind).unwrap_or(0);
        let profiles: Vec<String> = game_list
            .get(game_idx)
            .map(|g| {
                concierge::profiles::list_profiles(&g.dir)
                    .into_iter()
                    .map(|p| p.name)
                    .collect()
            })
            .unwrap_or_default();
        let profile = repo
            .profile
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let profile_idx = profile
            .as_ref()
            .and_then(|p| profiles.iter().position(|x| x == p))
            .unwrap_or(0);

        Ok(Self {
            workspace: workspace.map(|w| w.display().to_string()),
            game_count,
            game_kind: Some(kind),
            profile,
            is_bethesda,
            has_catalog,
            order_word,
            sort_label,
            game_paths,
            mods,
            mutable: true,
            tab: Tab::Setup,
            settings_open: false,
            browse_open: false,
            downloads_open: false,
            diff_open: false,
            confirm: None,
            search: String::new(),
            nxm_input: String::new(),
            browse_query: String::new(),
            selected: None,
            add_open: false,
            add_values: std::collections::BTreeMap::new(),
            versions,
            saves,
            pin_status,
            ai_input: String::new(),
            games,
            profiles,
            game_idx,
            profile_idx,
            new_profile: String::new(),
            live,
            health_issues,
            log: Vec::new(),
        })
    }

    fn panels(&self) -> Vec<Panel> {
        let mut panels = Vec::new();
        if self.settings_open {
            let mut lines = Vec::new();
            if let Some(w) = &self.workspace {
                lines.push(format!("workspace: {w} ({} games)", self.game_count));
            }
            lines.push("game paths [game.paths]:".to_owned());
            for (k, v) in &self.game_paths {
                lines.push(format!("  {k} = {v}"));
            }
            panels.push(Panel {
                id: "settings".into(),
                title: "Settings".into(),
                lines,
            });
        }
        if self.diff_open {
            // Headless has no deployed state to diff; show the header so the
            // window is enumerable + assertable (the GUI computes the full diff).
            panels.push(Panel {
                id: "preview".into(),
                title: "Preview changes".into(),
                lines: vec![
                    "Preview of your Setup vs what is Installed. Nothing changes until you Apply."
                        .to_owned(),
                ],
            });
        }
        panels
    }

    fn facts(&self) -> UiFacts {
        UiFacts {
            has_workspace: self.workspace.is_some(),
            workspace_path: self.workspace.clone(),
            game_count: self.game_count,
            active_game: self.game_kind.clone(),
            active_profile: self.profile.clone(),
            // Parallel to the GUI's repo+plan gate: a profile selected AND a game
            // loaded. Keeps the projected action guards identical across renderers.
            has_active_profile: self.profile.is_some() && self.game_kind.is_some(),
            is_bethesda: self.is_bethesda,
            has_catalog: self.has_catalog,
            tab: TabFacts(self.tab),
            mutable: self.mutable,
            has_undo: false,
            busy: false,
            ai_busy: false,
            settings_open: self.settings_open,
            browse_open: self.browse_open,
            downloads_open: self.downloads_open,
            downloads_paused_all: false,
            downloads: Vec::new(),
            agent_running: false,
            agent_present: false,
            update_available: false,
            browse_categories: Vec::new(),
            browse_sorts: Vec::new(),
            needed_pages: Vec::new(),
            addable_games: Vec::new(),
            settings: Vec::new(),
            diff_open: self.diff_open,
            confirm: self.confirm.as_ref().map(|(k, _)| *k),
            confirm_prompt: self.confirm.as_ref().map(|(_, p)| p.clone()),
            error: None,
            mods: self.mods.clone(),
            log_tail: self.log.iter().rev().take(5).rev().cloned().collect(),
            order_word: self.order_word.clone(),
            sort_label: self.sort_label.clone(),
            nxm_input: self.nxm_input.clone(),
            search: self.search.clone(),
            browse_query: self.browse_query.clone(),
            browse_msg: String::new(),
            browse_hits: Vec::new(),
            add_open: self.add_open,
            add_fields: if self.add_open {
                self.add_fields()
            } else {
                Vec::new()
            },
            versions: self.versions.clone(),
            saves: self.saves.clone(),
            pin_status: self.pin_status.clone(),
            ai_input: self.ai_input.clone(),
            ai_can_send: self.game_kind.is_some(),
            ai_quick: concierge_ai::agent::quick_actions()
                .iter()
                .map(|qa| qa.label.to_owned())
                .collect(),
            games: self.games.clone(),
            profiles: self.profiles.clone(),
            game_idx: self.game_idx,
            profile_idx: self.profile_idx,
            new_profile: self.new_profile.clone(),
            cache_summary: String::new(),
            panels: self.panels(),
        }
    }

    fn add_fields(&self) -> Vec<concierge_ui::Field> {
        [
            ("add_name", "name"),
            ("add_version", "version"),
            ("add_nexus_mod", "nexus mod id"),
            ("add_nexus_file", "nexus file id"),
            ("add_url", "or url"),
            ("add_md5", "md5"),
            ("add_file", "file"),
            ("add_plugins", "plugins (comma-sep)"),
        ]
        .iter()
        .map(|(id, label)| concierge_ui::Field {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
            value: self.add_values.get(*id).cloned().unwrap_or_default(),
        })
        .collect()
    }

    fn screen(&self) -> Screen {
        build_screen(&self.facts())
    }

    /// The pure-navigation intents change model state; the heavy actions are
    /// recorded, not run. Returns a one-line note.
    #[allow(clippy::too_many_lines)] // one arm per intent
    fn apply_intent(&mut self, id: &str) -> String {
        match id {
            "toggle_lock" => {
                self.mutable = !self.mutable;
                format!("mutable = {}", self.mutable)
            }
            "tab_setup" => {
                self.tab = Tab::Setup;
                "tab = Setup".into()
            }
            "tab_installed" => {
                self.tab = Tab::Installed;
                "tab = Installed".into()
            }
            "open_settings" => {
                self.settings_open = true;
                "settings opened".into()
            }
            "close_settings" => {
                self.settings_open = false;
                "settings closed".into()
            }
            "open_browse" => {
                self.browse_open = true;
                "browse opened".into()
            }
            "close_browse" => {
                self.browse_open = false;
                "browse closed".into()
            }
            "open_downloads" => {
                self.downloads_open = true;
                "downloads opened".into()
            }
            "close_downloads" => {
                self.downloads_open = false;
                "downloads closed".into()
            }
            "open_preview" => {
                self.diff_open = true;
                "preview opened".into()
            }
            "close_preview" => {
                self.diff_open = false;
                "preview closed".into()
            }
            "uninstall" => {
                self.confirm = Some((
                    ConfirmKind::Uninstall,
                    "Uninstall — remove the installed mods from the game?".into(),
                ));
                "confirm: uninstall".into()
            }
            "confirm_yes" => {
                let k = self.confirm.take().map(|(k, _)| k);
                format!("confirmed {k:?} (headless: action not executed)")
            }
            "confirm_no" => {
                self.confirm = None;
                "cancelled".into()
            }
            "add_open" => {
                self.add_open = !self.add_open;
                format!("add form = {}", self.add_open)
            }
            "add_confirm" => "add_confirm (headless: not executed)".into(),
            _ if id.starts_with("rollback:") => {
                let n = id.trim_start_matches("rollback:");
                self.confirm = Some((
                    ConfirmKind::Rollback,
                    format!("Restore your setup to version {n}?"),
                ));
                format!("confirm: rollback {n}")
            }
            _ if id.starts_with("restore_save:") => {
                let g = id.trim_start_matches("restore_save:");
                self.confirm = Some((
                    ConfirmKind::RestoreSave,
                    format!("Restore saves from backup {g}?"),
                ));
                format!("confirm: restore {g}")
            }
            "ai_send" | "ai_interrupt" | "ai_work" => format!("{id} (headless: AI not run)"),
            _ if id.starts_with("ai_quick:") => format!("{id} (headless: AI not run)"),
            "rescan" | "create_empty" | "create_clone" | "new_modpack_ai" | "show_window"
            | "quit" => {
                // `show_window`/`quit` are real actions for the GUI's tray; the
                // headless view has no window/process to act on, so it records them.
                format!("{id} (headless: not executed)")
            }
            "delete_profile" => {
                let name = self
                    .profiles
                    .get(self.profile_idx)
                    .cloned()
                    .unwrap_or_default();
                self.confirm = Some((
                    ConfirmKind::DeleteProfile,
                    format!("Delete profile '{name}'?"),
                ));
                format!("confirm: delete {name}")
            }
            _ if id.starts_with("select_game:") => {
                if let Ok(i) = id.trim_start_matches("select_game:").parse::<usize>() {
                    if let Some(g) = self.games.get(i).cloned() {
                        self.game_idx = i;
                        self.game_kind = Some(g.clone());
                        return format!("game = {g}");
                    }
                }
                "bad game index".into()
            }
            _ if id.starts_with("select_profile:") => {
                if let Ok(i) = id.trim_start_matches("select_profile:").parse::<usize>() {
                    if let Some(p) = self.profiles.get(i).cloned() {
                        self.profile_idx = i;
                        self.profile = Some(p.clone());
                        return format!("profile = {p}");
                    }
                }
                "bad profile index".into()
            }
            // heavy, side-effecting actions: recorded, never run headless
            other => format!("would run '{other}' (headless: not executed)"),
        }
    }

    /// Execute one `click <id>` step, enforcing the automaton: the intent must be
    /// an enabled transition in the current state.
    fn click(&mut self, id: &str) -> Result<String> {
        let screen = self.screen();
        let tr = screen
            .transitions
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| {
                Error::Other(format!(
                    "intent '{id}' is not available in state {}",
                    screen.state.name()
                ))
            })?;
        if !tr.enabled {
            return Err(Error::Other(format!(
                "intent '{id}' is guarded in state {}: {}",
                screen.state.name(),
                tr.guard.clone().unwrap_or_default()
            )));
        }
        let note = self.apply_intent(id);
        self.log.push(format!("> {id}: {note}"));
        Ok(note)
    }

    fn type_field(&mut self, field: &str, value: &str) -> Result<()> {
        match field {
            "search" => value.clone_into(&mut self.search),
            "nxm_input" => value.clone_into(&mut self.nxm_input),
            "browse_query" => value.clone_into(&mut self.browse_query),
            "ai_input" => value.clone_into(&mut self.ai_input),
            "new_profile" => value.clone_into(&mut self.new_profile),
            _ if field.starts_with("add_") => {
                self.add_values.insert(field.to_owned(), value.to_owned());
            }
            other => return Err(Error::Other(format!("unknown field '{other}'"))),
        }
        Ok(())
    }

    fn snapshot(&self) -> String {
        render_text(&self.screen())
    }

    /// Run a step script; returns the process exit code (non-zero on a failed
    /// assert or an illegal action). Prints per-step output to `out`.
    pub fn run_script(&mut self, script: &str, out: &mut dyn std::io::Write) -> i32 {
        let mut failures = 0u32;
        for (n, raw) in script.lines().enumerate() {
            let step = raw.trim();
            if step.is_empty() || step.starts_with('#') {
                continue;
            }
            match self.run_step(step) {
                Ok(Some(text)) => {
                    let _ = writeln!(out, "{text}");
                }
                Ok(None) => {}
                Err(e) => {
                    failures += 1;
                    let _ = writeln!(out, "FAIL (line {}): {e}", n + 1);
                }
            }
        }
        if failures == 0 {
            let _ = writeln!(out, "OK: all steps passed");
            0
        } else {
            let _ = writeln!(out, "FAILED: {failures} step(s)");
            1
        }
    }

    fn run_step(&mut self, step: &str) -> Result<Option<String>> {
        let (verb, rest) = step.split_once(char::is_whitespace).unwrap_or((step, ""));
        let rest = rest.trim();
        match verb {
            "snapshot" => Ok(Some(self.snapshot())),
            "state" => Ok(Some(format!("STATE: {}", self.screen().state.name()))),
            "health" => {
                if !self.live {
                    return Err(Error::Other("`health` needs --live".into()));
                }
                if self.health_issues.is_empty() {
                    Ok(Some(
                        "health: GO (no relation issues / missing masters)".into(),
                    ))
                } else {
                    Ok(Some(format!(
                        "health: NO-GO ({} issue(s)): {}",
                        self.health_issues.len(),
                        self.health_issues.join("; ")
                    )))
                }
            }
            "click" => {
                let note = self.click(rest)?;
                Ok(Some(format!(
                    "clicked {rest} -> {note}  [STATE: {}]",
                    self.screen().state.name()
                )))
            }
            "tab" => {
                let id = match rest {
                    "setup" | "Setup" => "tab_setup",
                    "installed" | "Installed" => "tab_installed",
                    o => return Err(Error::Other(format!("unknown tab '{o}'"))),
                };
                self.click(id)?;
                Ok(Some(format!("tab -> {rest}")))
            }
            "type" => {
                let (field, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                self.type_field(field, value.trim())?;
                Ok(Some(format!("typed {field} = \"{}\"", value.trim())))
            }
            "toggle" => {
                let m = self
                    .mods
                    .iter_mut()
                    .find(|m| m.name == rest)
                    .ok_or_else(|| Error::Other(format!("no mod '{rest}'")))?;
                m.enabled = !m.enabled;
                let now = m.enabled;
                self.log.push(format!("> toggle {rest}: {now}"));
                Ok(Some(format!("toggled {rest} -> enabled={now}")))
            }
            "select" => {
                self.pos(rest)?;
                self.selected = Some(rest.to_owned());
                Ok(Some(format!("selected {rest}")))
            }
            "move-up" | "move-down" => {
                let p = self.pos(rest)?;
                let target = if verb == "move-up" {
                    p.checked_sub(1)
                } else {
                    (p + 1 < self.mods.len()).then_some(p + 1)
                };
                let Some(q) = target else {
                    return Err(Error::Other(format!("{rest} can't {verb}")));
                };
                self.mods.swap(p, q);
                self.renumber();
                Ok(Some(format!("{verb} {rest}")))
            }
            "remove" => {
                self.pos(rest)?;
                self.mods.retain(|m| m.name != rest);
                self.renumber();
                Ok(Some(format!("removed {rest}")))
            }
            "assert" => {
                self.assert(rest)?;
                Ok(Some(format!("ok: assert {rest}")))
            }
            other => Err(Error::Other(format!("unknown step '{other}'"))),
        }
    }

    /// Position of a mod in the list by name (errors if absent).
    fn pos(&self, name: &str) -> Result<usize> {
        self.mods
            .iter()
            .position(|m| m.name == name)
            .ok_or_else(|| Error::Other(format!("no mod '{name}'")))
    }

    fn renumber(&mut self) {
        for (i, m) in self.mods.iter_mut().enumerate() {
            m.order = i + 1;
        }
    }

    fn assert(&self, spec: &str) -> Result<()> {
        let screen = self.screen();
        if let Some(want) = spec.strip_prefix("health ") {
            if !self.live {
                return Err(Error::Other("`assert health` needs --live".into()));
            }
            let go = self.health_issues.is_empty();
            return match want.trim() {
                "go" if go => Ok(()),
                "no-go" if !go => Ok(()),
                "go" => Err(Error::Other(format!(
                    "health NO-GO: {}",
                    self.health_issues.join("; ")
                ))),
                "no-go" => Err(Error::Other("health is GO, expected NO-GO".into())),
                o => Err(Error::Other(format!("unknown health condition '{o}'"))),
            };
        }
        if let Some(want) = spec.strip_prefix("state=") {
            let have = screen.state.name();
            if have == want.trim() {
                return Ok(());
            }
            return Err(Error::Other(format!("state is {have}, expected {want}")));
        }
        if let Some(rest) = spec.strip_prefix("mod ") {
            // `mod <name> enabled|disabled|present|absent|order <n>`
            let mut it = rest.splitn(2, char::is_whitespace);
            let name = it.next().unwrap_or("").trim();
            let cond = it.next().unwrap_or("present").trim();
            let found = self.mods.iter().find(|m| m.name == name);
            if let Some(n) = cond.strip_prefix("order ") {
                let want: usize = n
                    .trim()
                    .parse()
                    .map_err(|_| Error::Other(format!("bad order '{n}'")))?;
                return match found {
                    Some(m) if m.order == want => Ok(()),
                    Some(m) => Err(Error::Other(format!(
                        "mod {name} order is {}, expected {want}",
                        m.order
                    ))),
                    None => Err(Error::Other(format!("mod {name} absent"))),
                };
            }
            return match cond {
                "present" => found
                    .map(|_| ())
                    .ok_or_else(|| Error::Other(format!("mod {name} absent"))),
                "absent" => {
                    if found.is_none() {
                        Ok(())
                    } else {
                        Err(Error::Other(format!("mod {name} present, expected absent")))
                    }
                }
                "enabled" => match found {
                    Some(m) if m.enabled => Ok(()),
                    _ => Err(Error::Other(format!("mod {name} not enabled"))),
                },
                "disabled" => match found {
                    Some(m) if !m.enabled => Ok(()),
                    _ => Err(Error::Other(format!("mod {name} not disabled"))),
                },
                o => Err(Error::Other(format!("unknown mod condition '{o}'"))),
            };
        }
        if let Some(rest) = spec.strip_prefix("intent ") {
            // `intent <id> <enabled|disabled|present|absent>`
            let (id, cond) = rest
                .split_once(char::is_whitespace)
                .unwrap_or((rest, "present"));
            let found = screen.transitions.iter().find(|t| t.id == id.trim());
            return match cond.trim() {
                "present" => found
                    .map(|_| ())
                    .ok_or_else(|| Error::Other(format!("intent {id} absent"))),
                "absent" => {
                    if found.is_none() {
                        Ok(())
                    } else {
                        Err(Error::Other(format!(
                            "intent {id} present, expected absent"
                        )))
                    }
                }
                "enabled" => match found {
                    Some(t) if t.enabled => Ok(()),
                    _ => Err(Error::Other(format!("intent {id} not enabled"))),
                },
                "disabled" => match found {
                    Some(t) if !t.enabled => Ok(()),
                    _ => Err(Error::Other(format!("intent {id} not disabled"))),
                },
                o => Err(Error::Other(format!("unknown intent condition '{o}'"))),
            };
        }
        Err(Error::Other(format!("unknown assert '{spec}'")))
    }
}

/// Public entry for the `tui` subcommand. With no script, prints one snapshot;
/// with a script (file path or `-` for stdin), runs it and exits non-zero on
/// any failed step.
pub fn run(script: Option<&str>, live: bool) -> Result<i32> {
    let mut app = Headless::boot(live)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match script {
        None => {
            let _ = writeln!(out, "{}", app.snapshot());
            Ok(0)
        }
        Some(path) => {
            let text = if path == "-" {
                use std::io::Read as _;
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .map_err(|e| Error::Other(e.to_string()))?;
                s
            } else {
                std::fs::read_to_string(path).map_err(|e| Error::Other(e.to_string()))?
            };
            Ok(app.run_script(&text, &mut out))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use concierge_ui::ui_state;

    fn model() -> Headless {
        Headless {
            workspace: Some("/ws".into()),
            game_count: 2,
            game_kind: Some("skyrimse".into()),
            profile: Some("modpack".into()),
            is_bethesda: true,
            has_catalog: true,
            order_word: "load order".into(),
            sort_label: "Sort load order".into(),
            game_paths: vec![("my_games".into(), "/tmp/mg".into())],
            mods: vec![concierge_ui::ModRow {
                order: 1,
                name: "SkyUI".into(),
                enabled: true,
            }],
            mutable: true,
            tab: Tab::Setup,
            settings_open: false,
            browse_open: false,
            downloads_open: false,
            diff_open: false,
            confirm: None,
            search: String::new(),
            nxm_input: String::new(),
            browse_query: String::new(),
            selected: None,
            add_open: false,
            add_values: std::collections::BTreeMap::new(),
            versions: Vec::new(),
            saves: Vec::new(),
            pin_status: None,
            ai_input: String::new(),
            games: vec!["skyrimse".into(), "fallout4".into()],
            profiles: vec!["modpack".into()],
            game_idx: 0,
            profile_idx: 0,
            new_profile: String::new(),
            live: false,
            health_issues: Vec::new(),
            log: Vec::new(),
        }
    }

    #[test]
    fn toggle_lock_moves_editing_to_locked() {
        let mut m = model();
        assert_eq!(ui_state(&m.facts()).name(), "Editing");
        m.click("toggle_lock").unwrap();
        assert_eq!(ui_state(&m.facts()).name(), "Locked");
    }

    #[test]
    fn cannot_click_an_unavailable_intent() {
        let mut m = model();
        // confirm_yes is only offered inside a Confirming state
        assert!(m.click("confirm_yes").is_err());
    }

    #[test]
    fn uninstall_opens_confirm_then_cancel_returns() {
        let mut m = model();
        m.click("uninstall").unwrap();
        assert_eq!(ui_state(&m.facts()).name(), "ConfirmingUninstall");
        m.click("confirm_no").unwrap();
        assert_eq!(ui_state(&m.facts()).name(), "Editing");
    }

    #[test]
    fn script_runs_and_asserts() {
        let mut m = model();
        let script = "assert state=Editing\nclick open_settings\nassert state=Settings\nclick close_settings\nassert intent apply enabled\n";
        let mut out = Vec::new();
        let code = m.run_script(script, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("OK: all steps passed"));
    }

    #[test]
    fn a_failed_assert_exits_nonzero() {
        let mut m = model();
        let mut out = Vec::new();
        let code = m.run_script("assert state=Locked\n", &mut out);
        assert_eq!(code, 1);
    }

    // The agent/headless view can navigate + drive the background download manager
    // exactly as a human does — proof the two views share this surface.
    #[test]
    fn agent_can_drive_the_download_manager() {
        let mut m = model();
        let script = "assert state=Editing\n\
                      click open_downloads\n\
                      assert state=Downloads\n\
                      click dl_pause_all\n\
                      click dl_clear\n\
                      click close_downloads\n\
                      assert state=Editing\n";
        let mut out = Vec::new();
        let code = m.run_script(script, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{text}");
    }

    #[test]
    fn core_states_are_reachable_and_return_to_editing() {
        let state = |m: &Headless| ui_state(&m.facts()).name();
        let mut visited = std::collections::BTreeSet::new();
        let mut m = model();
        visited.insert(state(&m));
        // each modal/mode is reachable from Editing and returns to it.
        for (open, close) in [
            ("open_settings", "close_settings"),
            ("open_browse", "close_browse"),
            ("open_preview", "close_preview"),
            ("toggle_lock", "toggle_lock"),
            ("uninstall", "confirm_no"),
        ] {
            m.click(open).unwrap();
            visited.insert(state(&m));
            m.click(close).unwrap();
            assert_eq!(state(&m), "Editing", "{open} did not return to Editing");
        }
        for want in [
            "Editing",
            "Settings",
            "Browse",
            "PreviewChanges",
            "Locked",
            "ConfirmingUninstall",
        ] {
            assert!(visited.contains(want), "state {want} was not reached");
        }
    }
}
