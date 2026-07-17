//! The `GameAdapter` axis. An adapter runs at EVAL TIME ONLY: it resolves
//! install-target roots, renders config files, and names launch candidates
//! into the Plan. Everything downstream of the Plan (fetch, build, realize,
//! check, undeploy, launch) is universal and adapter-free.
//!
//! Adapter surface:
//! - `install_roots`: named targets, instance-relative OR keyed to an
//!   absolute path from `[game.paths]` (games like BG3/Paradox install mods
//!   OUTSIDE the game directory)
//! - `render_configs` / `config_resets`: generated files for deploy/undeploy
//!   (empty for games with no load-order registry)
//! - `launch_candidates`: what to start, priority-ordered, instance-relative
//! - `nexus_domain`: catalog identity where the game lives on Nexus

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::manifest::Manifest;
use crate::plan::{ConfigFile, ResolvedRoot};

/// Where a named install root points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootTarget {
    /// Directory relative to the game instance ("" = instance root).
    InstanceRel(&'static str),
    /// Absolute directory taken from `[game.paths]` by key.
    PathKey(&'static str),
}

/// Per-game vocabulary: the words a game's own community uses, which override
/// Concierge's generic UI labels. "Match Between System and the Real World"
/// applied per domain — a Bethesda modder says "load order", not "order".
#[derive(Debug, Clone, Copy)]
pub struct Lexicon {
    /// The ordered-activation concept — "load order" (Bethesda) vs "order".
    pub order: &'static str,
    /// The sort-the-order action button label.
    pub sort_action: &'static str,
    /// The activation units — "plugins" (Bethesda .esp/.esm) vs "files".
    pub plugins: &'static str,
}

impl Default for Lexicon {
    fn default() -> Self {
        Self {
            order: "order",
            sort_action: "Sort order",
            plugins: "files",
        }
    }
}

/// Bethesda (Skyrim/Fallout) — the "load order" / "plugins" vocabulary.
pub const BETHESDA_LEXICON: Lexicon = Lexicon {
    order: "load order",
    sort_action: "Sort load order",
    plugins: "plugins",
};

/// BG3 — pak mods activated via modsettings' load order.
pub const BG3_LEXICON: Lexicon = Lexicon {
    order: "load order",
    sort_action: "Sort load order",
    plugins: "paks",
};

/// `RimWorld` — a load order of mods.
pub const RIMWORLD_LEXICON: Lexicon = Lexicon {
    order: "load order",
    sort_action: "Sort load order",
    plugins: "mods",
};

/// KOTOR — files layered into the Override folder.
pub const KOTOR_LEXICON: Lexicon = Lexicon {
    order: "install order",
    sort_action: "Sort install order",
    plugins: "override files",
};

/// Non-ordered "just a list of mods" games (Stardew/Minecraft/Valheim).
pub const MODLIST_LEXICON: Lexicon = Lexicon {
    order: "mod list",
    sort_action: "Sort mods",
    plugins: "mods",
};

/// A foundational tool a game **promotes** above the ordinary mod list — a
/// script extender, a mod loader, whatever that game's world calls for. It is
/// deliberately generic: core neither defines nor requires any particular kind,
/// and imposes no shared ontology across games. A game crate decides what (if
/// anything) it promotes and what the fields mean; the player still chooses
/// whether to install it. Core only routes and surfaces it — it never knows
/// what a "script extender" is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromotedTool {
    /// Stable short id, e.g. `"f4se"`.
    pub id: &'static str,
    /// Display name, e.g. `"F4SE"`.
    pub name: &'static str,
    /// One line: what it is and why a pack wants it.
    pub blurb: &'static str,
    /// Canonical home to get it from, for the UI to point at.
    pub home: &'static str,
    /// The [`install_roots`](GameAdapter::install_roots) name its archive
    /// deploys to (e.g. `"game"`), so core can route it there.
    pub install_root: &'static str,
}

pub trait GameAdapter: Sync {
    fn kind(&self) -> &'static str;
    fn nexus_domain(&self) -> Option<&'static str>;
    /// The Modrinth search domain for this game's ecosystem (e.g. `minecraft`),
    /// if its mods live on Modrinth rather than Nexus. `None` = not Modrinth.
    /// Drives the catalog browser's provider choice.
    fn modrinth_domain(&self) -> Option<&'static str> {
        None
    }
    /// This game's community vocabulary; overrides generic UI labels. Generic by
    /// default — a game overrides it to speak its players' language.
    fn lexicon(&self) -> Lexicon {
        Lexicon::default()
    }
    fn install_roots(&self) -> &'static [(&'static str, RootTarget)];
    fn default_install_root(&self) -> &'static str;
    /// `[game.paths]` keys this adapter requires.
    fn required_paths(&self) -> &'static [&'static str];
    /// Config files realize writes (paths absolute, content final).
    fn render_configs(&self, m: &Manifest, plugins: &[String]) -> Result<Vec<ConfigFile>>;
    /// Config files undeploy writes to return to a no-mods state. Always
    /// writable: a reset hands the file back to the game (e.g. BG3's in-game
    /// mod manager), which must be able to rewrite it again.
    fn config_resets(&self, m: &Manifest) -> Result<Vec<ConfigFile>> {
        Ok(self
            .render_configs(m, &[])?
            .into_iter()
            .map(|c| ConfigFile::new(c.path, c.content))
            .collect())
    }
    /// Instance-relative launch candidates, priority order. `.app` bundles
    /// are allowed (native runtime opens them).
    fn launch_candidates(&self) -> &'static [&'static str];
    /// Steam app id, when launching the PRISTINE install should go through
    /// Steam (native Steam DRM titles reject direct exec). Ignored when a
    /// modded instance must run instead.
    fn steam_app_id(&self) -> Option<u32> {
        None
    }
    /// Family diff-application, run AFTER the base instance is materialized and
    /// the generic file overlay is placed. Most families' mods ARE file-level
    /// overlays (a pak, a loose file), which the generic deploy already applies —
    /// so this is a no-op by default. KOTOR overrides it to merge each mod's
    /// `TSLPatcher` `changes.ini` (`2DA`/`TLK` field-level diff) against the base
    /// into the instance's Override. Adapter-provided so core stays game-agnostic.
    fn diff_apply(&self, _ctx: &DiffCtx) -> Result<()> {
        Ok(())
    }
    /// Game-specific invariant lints — the crash-causing rules `realize`/`doctor`
    /// enforce (missing masters, plugin limits, duplicate ids, …). Default: none.
    /// Adapter-dispatched so a game contributes its own invariants by adding a
    /// crate, never by editing a central `match` in core.
    fn lints(&self, _plan: &crate::plan::Plan) -> Result<Vec<crate::lint::Violation>> {
        Ok(Vec::new())
    }
    /// For plugin-order games, the base + DLC plugins that ship with the game and
    /// load implicitly (index 0 = the base master). Lets the load-order engine
    /// and the missing-master check resolve the base from the adapter instead of
    /// a hardcoded `kind` match. Default: not a plugin-order game.
    fn plugin_bases(&self) -> Option<&'static [&'static str]> {
        None
    }
    /// Foundational tools this game promotes above the mod list (a script
    /// extender, a mod loader, …). Optional to use; most games promote nothing
    /// (default empty). A game crate — never core — decides what belongs here,
    /// so no cross-game ontology is imposed.
    fn promoted_tools(&self) -> Vec<PromotedTool> {
        Vec::new()
    }
    /// If a built archive — identified only by its top-level entry names — IS
    /// one of the promoted tools, return it. Core uses this to route the archive
    /// to the tool's [`install_root`](PromotedTool::install_root) and surface it
    /// as promoted, without knowing what the tool means. The game crate owns the
    /// recognition (e.g. "a loader exe sits at the root"). Default: recognises
    /// nothing.
    fn promoted_tool_for(&self, _top_level: &[String]) -> Option<PromotedTool> {
        None
    }
    /// Game-specific guidance for the in-profile AI assistant — how THIS game is
    /// actually modded: the load model, the foundational tools this community
    /// expects, where mods live, and the common gotchas. Appended to the
    /// profile's `CLAUDE.md` under a game heading so the assistant reasons from
    /// real community norms instead of generic advice. Markdown, no top-level
    /// `#` heading (the provisioner adds one). Default: none — the generic guide
    /// still ships. This is the "agent context" half of a game's opinion; keep
    /// it truthful and specific (named tools, real limits), never speculative.
    fn agent_guide(&self) -> Option<String> {
        None
    }
    /// Post-launch (and pre-launch) health for this game: parse the runtime's
    /// script-extender / crash log for what loaded vs failed, plus static
    /// platform-compatibility warnings. `my_games` is `[game.paths].my_games`
    /// (where the log lives). Default: the game exposes no such signal.
    fn launch_health(
        &self,
        _plan: &crate::plan::Plan,
        _my_games: &std::path::Path,
    ) -> Option<LaunchHealth> {
        None
    }
}

/// What a game's launch log / platform check reports — which components
/// loaded, which failed, and any platform-compatibility warnings.
#[derive(Debug, Default, Clone)]
pub struct LaunchHealth {
    /// Components that loaded correctly (e.g. script-extender plugins).
    pub loaded: Vec<String>,
    /// Failures, incompatibilities, and platform pre-warnings — each a line
    /// naming the component and why.
    pub issues: Vec<String>,
}

/// Inputs for [`GameAdapter::diff_apply`]: the materialized instance, the
/// pristine base (to read base game files a diff merges against, e.g. KOTOR's
/// `dialog.tlk`), and each mod's build-tree dir. All paths, no game knowledge —
/// the adapter interprets them.
#[derive(Debug)]
pub struct DiffCtx<'a> {
    /// The materialized instance dir (`plan.game_dir()`), where diffs land.
    pub instance_dir: &'a std::path::Path,
    /// The pristine game dir (read-only source for base files).
    pub base_dir: &'a std::path::Path,
    /// `(mod name, build-tree dir)` in manifest order.
    pub mods: Vec<(String, std::path::PathBuf)>,
}

/// Resolves a game `kind` to its adapter. Injected by the workspace assembly
/// crate (`concierge-games`) at startup so the concrete adapters can live in
/// their own family/leaf crates while `concierge-core` keeps only the engine.
type Resolver = fn(&str) -> Option<&'static dyn GameAdapter>;
static RESOLVER: std::sync::OnceLock<Resolver> = std::sync::OnceLock::new();
static KNOWN_KINDS: std::sync::OnceLock<fn() -> Vec<&'static str>> = std::sync::OnceLock::new();

/// Register the workspace's adapter registry. Called once at process start by
/// `concierge_games::register()`. Idempotent (subsequent calls are ignored), so
/// adding a game means adding a crate + a registry row, never editing core.
pub fn register_adapters(resolve: Resolver, kinds: fn() -> Vec<&'static str>) {
    let _ = RESOLVER.set(resolve);
    let _ = KNOWN_KINDS.set(kinds);
}

/// Every kind the process can resolve — injected registry ∪ core builtins.
fn known_kinds() -> Vec<&'static str> {
    let mut ks: Vec<&'static str> = KNOWN_KINDS.get().map(|f| f()).unwrap_or_default();
    ks.extend(ALL.iter().map(|a| a.kind()));
    ks.sort_unstable();
    ks.dedup();
    ks
}

/// A codified adapter for `kind`, or None when the manifest must supply the
/// game shape itself via `[game.custom]` (the data-driven path). Prefers the
/// injected registry; falls back to any (shrinking) core builtins.
pub fn try_adapter(kind: &str) -> Option<&'static dyn GameAdapter> {
    if let Some(a) = RESOLVER.get().and_then(|r| r(kind)) {
        return Some(a);
    }
    ALL.iter().find(|a| a.kind() == kind).copied()
}

pub fn adapter_for(kind: &str) -> Result<&'static dyn GameAdapter> {
    try_adapter(kind).ok_or_else(|| {
        Error::Manifest(format!(
            "no codified adapter for game kind '{kind}' (known: {}); use kind=\"custom\" with \
             [game.custom], or kind=\"generic\" for a plain file-overlay game",
            known_kinds().join(", ")
        ))
    })
}

/// The four Plan inputs a game supplies, whether from a codified adapter or
/// from `[game.custom]` manifest data.
#[derive(Debug)]
pub struct GameShape {
    pub nexus_domain: Option<String>,
    pub modrinth_domain: Option<String>,
    pub default_root: String,
    pub root_targets: BTreeMap<String, ResolvedRoot>,
    pub configs: Vec<ConfigFile>,
    pub config_resets: Vec<ConfigFile>,
    pub launch_candidates: Vec<String>,
    pub steam_app_id: Option<u32>,
}

/// The no-knowledge fallback a user opts into with `kind = "generic"`: no
/// adapter, no `[game.custom]`, no assumptions about how modding works beyond
/// "mods add or replace files under the game dir". Mods overlay into the `CoW`
/// instance — a pure diff against the pristine — with no configs, no
/// activation registry, and no launch knowledge.
fn generic_shape() -> GameShape {
    let mut root_targets = BTreeMap::new();
    root_targets.insert(
        "game".to_owned(),
        ResolvedRoot {
            instance_relative: true,
            dir: String::new(),
            mount_at: None,
        },
    );
    GameShape {
        nexus_domain: None,
        modrinth_domain: None,
        default_root: "game".to_owned(),
        root_targets,
        configs: Vec::new(),
        config_resets: Vec::new(),
        launch_candidates: Vec::new(),
        steam_app_id: None,
    }
}

/// Build the game shape: prefer a codified adapter; fall back to the
/// data-driven `[game.custom]` section, or — for `kind = "generic"` with no
/// custom section — the pure file-overlay shape. This is where "per-game
/// crates are accelerators, not requirements" is enforced in code.
pub fn shape_for(m: &Manifest, plugins: &[String]) -> Result<GameShape> {
    if m.game.kind == "generic" && m.game.custom.is_none() {
        return Ok(generic_shape());
    }
    if let Some(a) = try_adapter(&m.game.kind) {
        return Ok(GameShape {
            nexus_domain: a.nexus_domain().map(str::to_owned),
            modrinth_domain: a.modrinth_domain().map(str::to_owned),
            default_root: a.default_install_root().to_owned(),
            root_targets: resolve_roots(a, m)?,
            configs: a.render_configs(m, plugins)?,
            config_resets: a.config_resets(m)?,
            launch_candidates: a
                .launch_candidates()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            steam_app_id: a.steam_app_id(),
        });
    }
    custom_shape(m, plugins)
}

fn custom_shape(m: &Manifest, plugins: &[String]) -> Result<GameShape> {
    let c = m.game.custom.as_ref().ok_or_else(|| {
        Error::Manifest(format!(
            "game kind '{}' has no codified adapter and no [game.custom] section; declare one, \
             or use kind = \"generic\" for a plain file-overlay game (mods just add/replace files)",
            m.game.kind
        ))
    })?;
    let mut root_targets = BTreeMap::new();
    for r in &c.roots {
        let resolved = match (&r.dir, &r.path_key) {
            (Some(dir), None) => ResolvedRoot {
                instance_relative: true,
                dir: dir.clone(),
                mount_at: None,
            },
            // Same Wabbajack mount as codified adapters: an external path key
            // becomes an instance-owned mount.
            (None, Some(key)) => {
                let p = m.game.paths.get(key).ok_or_else(|| {
                    Error::Manifest(format!(
                        "custom root '{}' needs [game.paths] key '{key}'",
                        r.name
                    ))
                })?;
                ResolvedRoot {
                    instance_relative: true,
                    dir: format!("mounts/{}", r.name),
                    mount_at: Some(p.display().to_string()),
                }
            }
            _ => {
                return Err(Error::Manifest(format!(
                    "custom root '{}' needs exactly one of dir / path_key",
                    r.name
                )))
            }
        };
        root_targets.insert(r.name.clone(), resolved);
    }
    if !root_targets.contains_key(&c.default_root) {
        return Err(Error::Manifest(format!(
            "custom default_root '{}' is not among the declared roots",
            c.default_root
        )));
    }
    let render = |entries: &[String]| -> Result<Vec<ConfigFile>> {
        let mut out = Vec::new();
        for cfg in &c.configs {
            let body: String = entries
                .iter()
                .map(|e| format!("{}{e}", cfg.line_prefix))
                .collect::<Vec<_>>()
                .join("\n");
            let content = cfg.template.replace("{{plugins}}", &body);
            let path = match (&cfg.path, &cfg.path_key) {
                (Some(p), None) => p.display().to_string(),
                (None, Some(key)) => m
                    .game
                    .paths
                    .get(key)
                    .ok_or_else(|| {
                        Error::Manifest(format!("custom config needs [game.paths] key '{key}'"))
                    })?
                    .display()
                    .to_string(),
                _ => {
                    return Err(Error::Manifest(
                        "custom config needs exactly one of path / path_key".into(),
                    ))
                }
            };
            out.push(ConfigFile::new(path, content));
        }
        Ok(out)
    };
    Ok(GameShape {
        nexus_domain: c.nexus_domain.clone(),
        modrinth_domain: c.modrinth_domain.clone(),
        default_root: c.default_root.clone(),
        root_targets,
        configs: render(plugins)?,
        config_resets: render(&[])?,
        launch_candidates: c.launch.clone(),
        steam_app_id: c.steam_app_id,
    })
}

/// Resolve an adapter's roots against the manifest's declared paths.
pub fn resolve_roots(
    adapter: &dyn GameAdapter,
    m: &Manifest,
) -> Result<BTreeMap<String, ResolvedRoot>> {
    for key in adapter.required_paths() {
        if !m.game.paths.contains_key(*key) {
            return Err(Error::Manifest(format!(
                "game '{}' requires [game.paths] key '{key}'",
                adapter.kind()
            )));
        }
    }
    let mut out = BTreeMap::new();
    for (name, target) in adapter.install_roots() {
        let resolved = match target {
            RootTarget::InstanceRel(dir) => ResolvedRoot {
                instance_relative: true,
                dir: (*dir).to_owned(),
                mount_at: None,
            },
            // Wabbajack model: a root that names an EXTERNAL location (mods read
            // from a fixed dir outside the game — BG3's Documents `Mods/`, The
            // Sims 4, a merge tool's input) becomes an instance-owned mount. Mods
            // deploy into `<instance>/mounts/<name>`; realize park+symlinks the
            // real path to it, so the real dir is never mutated and undeploy
            // restores it. The base game/profile stays pristine.
            RootTarget::PathKey(key) => {
                let p = m.game.paths.get(*key).ok_or_else(|| {
                    Error::Manifest(format!(
                        "game '{}' root '{name}' needs [game.paths] key '{key}'",
                        adapter.kind()
                    ))
                })?;
                ResolvedRoot {
                    instance_relative: true,
                    dir: format!("mounts/{name}"),
                    mount_at: Some(p.display().to_string()),
                }
            }
        };
        out.insert((*name).to_owned(), resolved);
    }
    Ok(out)
}

/// Core has no built-in adapters — every game lives in its own family/leaf
/// crate, wired in via the injected registry (`concierge-games`). Kept as an
/// (empty) fallback so `try_adapter` needs no special-casing.
pub static ALL: &[&dyn GameAdapter] = &[];
