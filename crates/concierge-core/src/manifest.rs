//! The manifest is the single source of truth the user edits.
//!
//! Loading it performs structural validation only — no filesystem or network.
//! Game-specific semantics (install roots, config rendering) are resolved by
//! the game adapter at eval time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, IoCtx, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub game: Game,
    /// INI sections composed by Bethesda-family adapters into Custom.ini.
    #[serde(default)]
    pub ini: BTreeMap<String, BTreeMap<String, String>>,
    /// **WHAT** — the mods in this pack (identity + how to acquire each). A
    /// `[[mod]]`'s own install settings live in its `[mod.config]` sub-table
    /// (see [`ModConfig`]); cross-mod concerns live in `[relations]`.
    #[serde(default, rename = "mod")]
    pub mods: Vec<Mod>,
    /// **RELATIONAL** — cross-mod concerns: explicit load order, compatibility
    /// patches, and conflict-resolution rules. Kept separate from `[[mod]]` on
    /// purpose: these are relationships *between* mods, not properties *of* one.
    #[serde(default)]
    pub relations: Relations,
    /// **ENVIRONMENT** — what this pack needs from the world outside the mods:
    /// game version, DLC, script-extender, loader, client/server side. A
    /// universal shell; the *values* are game-specific (adapter territory).
    /// Makes a pack's portability explicit instead of silently version-bound.
    #[serde(default)]
    pub compat: Compat,
    /// **CURATION** — the white-glove inputs: the experience the user wants
    /// (`brief`) plus declarative filters on the mod catalog that constrain
    /// every pick Concierge makes (min endorsements, size, recency, NSFW,
    /// categories, avoid/must-have, scope). The user says the experience + the
    /// rules; Concierge picks the exact mods within them.
    #[serde(default)]
    pub curate: Curate,
}

/// The curation inputs — a free-text experience brief and declarative filters
/// on the catalog. Hard filters (endorsements, size, recency, NSFW, categories,
/// avoid) narrow the searchable set; soft preferences (preferred categories,
/// lore-friendliness, scope, must-haves) guide the curator's choices within it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curate {
    /// Free-text description of the experience/playthrough the user wants.
    #[serde(default)]
    pub brief: Option<String>,
    /// Minimum endorsements a candidate mod must have (hard filter).
    #[serde(default)]
    pub min_endorsements: u64,
    /// Maximum archive size in MB (hard filter; None = no cap).
    #[serde(default)]
    pub max_size_mb: Option<u64>,
    /// Only consider mods updated on/after this date, e.g. `2020-01-01` (hard).
    #[serde(default)]
    pub updated_since: Option<String>,
    /// Allow adult/NSFW mods (default off; hard filter).
    #[serde(default)]
    pub nsfw: bool,
    /// Preferred categories (soft — steers the curator).
    #[serde(default)]
    pub categories_prefer: Vec<String>,
    /// Categories to exclude (hard filter).
    #[serde(default)]
    pub categories_avoid: Vec<String>,
    /// Lore-friendly preference (soft — steers the curator).
    #[serde(default)]
    pub lore_friendly: bool,
    /// Mods or authors to never pick, by name (hard filter).
    #[serde(default)]
    pub avoid: Vec<String>,
    /// Mods the pack must always include, by name (the curator keeps these).
    #[serde(default)]
    pub must_have: Vec<String>,
    /// Scope target: e.g. `lean`, `overhaul`, or a rough mod-count hint (soft).
    #[serde(default)]
    pub scope: Option<String>,
}

/// The environment/compatibility a pack needs (game version, DLC, script
/// extender, loader, side). Precedent: npm `engines`, Cargo `rust-version`,
/// Modrinth `env`, Debian `Architecture`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Minimum game version required (numeric dot form, e.g. `1.10.163`).
    #[serde(default)]
    pub game_version: Option<String>,
    /// Maximum game version supported (e.g. breaks on next-gen); inclusive.
    #[serde(default)]
    pub game_version_max: Option<String>,
    /// Required DLC (by adapter-defined name).
    #[serde(default)]
    pub dlc: Vec<String>,
    /// Minimum script-extender version (F4SE/SKSE).
    #[serde(default)]
    pub script_extender: Option<String>,
    /// Mod loader (e.g. minecraft `fabric`/`forge`) + its version.
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub loader_version: Option<String>,
    /// Which side this pack targets: `client`, `server`, or `both`.
    #[serde(default)]
    pub side: Option<String>,
}

/// The relational layer of the declaration — everything that is about how mods
/// relate to each other rather than what any single mod is.
///
/// It separates **declared facts** (topology: `requires`/`incompatible`/
/// `provides`) from **resolutions** (reconcile: `load_order`/`patch`/`rule`).
/// Prior art (Debian relationships, LOOT `req`/`inc`, package-manager
/// `provides`) keeps these apart; so do we.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relations {
    // ── declared FACTS (topology) ──
    /// `name` needs `needs` (optionally at `min_version`); satisfiable by a
    /// matching mod name or a `provides` capability.
    #[serde(default)]
    pub requires: Vec<Requirement>,
    /// Hard incompatibilities: `a` and `b` must not both be enabled.
    #[serde(default)]
    pub incompatible: Vec<Incompatibility>,
    /// Virtual capabilities a mod satisfies (so interchangeable mods can fill a
    /// single `requires`, e.g. "any weather framework").
    #[serde(default)]
    pub provides: Vec<Provision>,

    // ── RESOLUTIONS (reconcile) ──
    /// Explicit plugin load order. Empty → derived from `[[mod]]` order.
    #[serde(default)]
    pub load_order: Vec<String>,
    /// Compatibility patches — each declares which mods it bridges.
    #[serde(default, rename = "patch")]
    pub patches: Vec<Patch>,
    /// Conflict-resolution rules — who wins a contested path.
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
    /// Named ordering groups (order groups, not every plugin).
    #[serde(default, rename = "group")]
    pub groups: Vec<Group>,
}

/// A declared dependency fact: `name` requires `needs` (optionally `min_version`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub name: String,
    pub needs: String,
    #[serde(default)]
    pub min_version: Option<String>,
}

/// A declared incompatibility fact: `a` and `b` must not both be enabled.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Incompatibility {
    pub a: String,
    pub b: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// A declared virtual capability: mod `name` provides `capability`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provision {
    pub name: String,
    pub capability: String,
}

/// A compatibility patch: names which mods it makes work together. The patch's
/// own files are declared as an ordinary `[[mod]]` of the same `name`; this
/// entry records the *relationship*.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub name: String,
    /// The mods this patch bridges (by mod name).
    #[serde(default)]
    pub bridges: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// A conflict-resolution rule: which mod wins a contested file/record path.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub path: String,
    pub winner: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Game {
    /// Which adapter interprets this manifest (fallout4, skyrimse, stardew,
    /// kotor2, bg3, rimworld, minecraft, valheim).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// The store-managed install. Never written to.
    pub pristine: PathBuf,
    /// The disposable clone the game runs from. Omit for IN-PLACE mode:
    /// mods deploy into the pristine install itself (ownership tracking +
    /// backups + undeploy are the rollback story) and native Steam titles
    /// launch through the user's actual Steam.
    #[serde(default)]
    pub instance: Option<PathBuf>,
    pub version: String,
    /// Adapter-required absolute paths (e.g. fallout4: `plugins_txt`, `my_games`;
    /// bg3: `profile_mods`, `modsettings`; rimworld: `mods_config`).
    #[serde(default)]
    pub paths: BTreeMap<String, PathBuf>,
    /// Base-game plugin entries always activated ahead of mods
    /// (Bethesda DLC lists; unused by adapters without a load-order file).
    #[serde(default)]
    pub dlc: Vec<String>,
    /// Data-driven adapter for `kind = "custom"` — proves that core alone
    /// services any game: install roots, config templates, and launch
    /// candidates come entirely from manifest data, no per-game crate.
    #[serde(default)]
    pub custom: Option<CustomGame>,
    // Legacy fallout4 fields, folded into `paths` on load.
    #[serde(default)]
    plugins_txt: Option<PathBuf>,
    #[serde(default)]
    my_games: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomGame {
    pub default_root: String,
    #[serde(default)]
    pub nexus_domain: Option<String>,
    #[serde(default)]
    pub modrinth_domain: Option<String>,
    #[serde(default)]
    pub launch: Vec<String>,
    #[serde(default)]
    pub steam_app_id: Option<u32>,
    #[serde(default, rename = "root")]
    pub roots: Vec<CustomRoot>,
    #[serde(default, rename = "config")]
    pub configs: Vec<CustomConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRoot {
    pub name: String,
    /// Instance-relative directory ("" = instance root). Mutually exclusive
    /// with `path_key`.
    #[serde(default)]
    pub dir: Option<String>,
    /// Absolute directory taken from `[game.paths]` (mods outside the game).
    #[serde(default)]
    pub path_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomConfig {
    /// Target: an absolute `[game.paths]` key, or a literal path.
    #[serde(default)]
    pub path_key: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Template; `{{plugins}}` expands to activation entries, one per line,
    /// each prefixed with `line_prefix`.
    pub template: String,
    #[serde(default)]
    pub line_prefix: String,
}

fn default_kind() -> String {
    "fallout4".to_owned()
}

/// A single mod. Its fields fall in two concerns kept distinct in the language:
/// **WHAT** (identity + acquisition — `name`/`version`/source/`md5`/`enabled`),
/// and **HOW IT'S CONFIGURED** (its own install settings — preferably written
/// in a `[mod.config]` sub-table, see [`ModConfig`], and folded onto these flat
/// fields on load for backward compatibility). Cross-mod *relations* (load
/// order, patches) are NOT here — they live in `[relations]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mod {
    // ── WHAT: identity + acquisition ──
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub nexus_mod_id: Option<u64>,
    #[serde(default)]
    pub nexus_file_id: Option<u64>,
    /// Content address of the archive. Empty string = not yet pinned.
    #[serde(default)]
    pub md5: String,
    /// Wabbajack's archive key (xxHash64, base64). Empty = unset. Lets a
    /// manually-downloaded file be hash-detected the way Wabbajack does, and
    /// carries through from imported `.wabbajack` lists.
    #[serde(default)]
    pub xxhash: String,
    /// Archive filename in the store. Defaults to the url basename.
    #[serde(default)]
    pub file: Option<String>,
    /// A declarative acquisition pipeline (for mods not on Nexus/http): an
    /// ordered list of typed steps producing the archive. Mutually exclusive
    /// with url/nexus in practice; takes precedence if set.
    #[serde(default)]
    pub pipeline: Option<Vec<crate::pipeline::Step>>,
    /// A Nix fetcher expression (fetchGit/fetchurl/…) — the optional Nix-FOD
    /// acquisition tier. `nix build`s it (SRI/rev-verified) into the store.
    #[serde(default)]
    pub nix: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Trust/resilience: extra download mirrors (tried in order after the
    /// primary source) and a second, stronger hash (sha512/sha256). Precedent:
    /// Modrinth `downloads[]` + paired sha1/sha512.
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub sha512: Option<String>,
    /// Descriptive **metadata** (author, category, tags, NSFW) — kept distinct
    /// from functional identity/acquisition. Folded from a `[mod.meta]` table.
    #[serde(default)]
    pub meta: ModMeta,
    /// Organizational group this mod belongs to (see `[[relations.group]]`).
    /// Folded from `[mod.config].group`.
    #[serde(default)]
    pub group: Option<String>,
    /// Conditional/optional install choices; selected options' plugins fold
    /// into `plugins` on load. Folded from `[[mod.config.choice]]`.
    #[serde(default)]
    pub choices: Vec<Choice>,

    // ── HOW IT'S CONFIGURED: this mod's own install settings ──
    // Preferred form is a `[mod.config]` sub-table ([`ModConfig`]); for
    // backward compatibility these may also appear flat and are read directly.
    // `finish()` folds any `[mod.config]` onto these fields.
    /// Named install root (adapter-defined; adapter default when omitted).
    #[serde(default)]
    pub install_root: Option<String>,
    /// Path inside the archive treated as the install root.
    #[serde(default)]
    pub subdir: Option<String>,
    /// Activation entries, in order (Bethesda: plugin filenames; bg3:
    /// `uuid|Name`; rimworld: package ids; unused by config-less games).
    #[serde(default)]
    pub plugins: Vec<String>,
    /// Archive-relative path prefixes to skip.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Wabbajack model: when set, this mod's fetched `file` is a `BSDiff` patch
    /// and its content is DERIVED at build time from a source the user owns —
    /// so the pack ships a patch, not redistributable content.
    #[serde(default)]
    pub patch: Option<crate::plan::PatchSpec>,
    /// Free-form per-mod options (e.g. INI tweaks, FOMOD choices) — declared,
    /// carried through the plan; adapters may consume them.
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// The preferred, clearly-separated place for this mod's install config.
    /// Folded onto the flat fields above by `finish()`, then cleared.
    #[serde(default)]
    pub config: Option<ModConfig>,
    /// FOMOD installer selections (`[mod.fomod]`). When present, deploy is
    /// driven by the archive's `fomod/ModuleConfig.xml` and these recorded
    /// picks — exactly the files the installer would copy for those options —
    /// instead of a raw subdir/exclude. This is how Vortex/MO2 install a FOMOD.
    #[serde(default)]
    pub fomod: Option<Fomod>,
}

/// Recorded FOMOD option picks — the declarative equivalent of clicking through
/// the installer. `select` lists the chosen option names (by their
/// `<plugin name>`); required options and `SelectAll` groups are always
/// included by the resolver. Empty `select` = take the installer's
/// recommended defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fomod {
    #[serde(default)]
    pub select: Vec<String>,
}

/// The **HOW IT'S CONFIGURED** concern for one mod, as a `[mod.config]`
/// sub-table — kept visually distinct from the mod's identity/acquisition.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModConfig {
    #[serde(default)]
    pub install_root: Option<String>,
    #[serde(default)]
    pub subdir: Option<String>,
    /// Activation entries this mod provides, in order (a.k.a. `plugins`).
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// Organizational group this mod belongs to (folded onto `Mod.group`).
    #[serde(default)]
    pub group: Option<String>,
    /// Conditional/optional install choices (FOMOD-style, declarative).
    #[serde(default, rename = "choice")]
    pub choices: Vec<Choice>,
}

/// Descriptive **metadata** for a mod — distinct from functional identity.
/// A `[mod.meta]` sub-table. Precedent: Cargo `[package]` (description/
/// keywords/categories/license), Wabbajack `Author`/`IsNSFW`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModMeta {
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
}

/// An organizational group for the load order — order *groups*, not every
/// plugin, so ordering stays tractable at scale. Precedent: LOOT `group`
/// (name + `after`), MO2 separators.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    /// Groups this one loads after.
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A conditional/optional install choice for a mod — the declarative form of a
/// FOMOD step: a named group of options with a select mode, where the user's
/// picks are *recorded* (`selected`) rather than made at install time, so the
/// install stays declarative and reproducible. Selected options' `plugins`
/// fold into the mod's activation entries on load.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub name: String,
    /// `one` (radio) · `any` (checkbox) · `all`.
    #[serde(default = "default_select")]
    pub select: String,
    #[serde(default, rename = "option")]
    pub options: Vec<ChoiceOption>,
}

/// One option within a [`Choice`]; `selected` records the user's pick.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub label: String,
    #[serde(default)]
    pub selected: bool,
    /// Archive subdir installed when this option is selected.
    #[serde(default)]
    pub subdir: Option<String>,
    /// Activation entries added when this option is selected.
    #[serde(default)]
    pub plugins: Vec<String>,
}

fn default_select() -> String {
    "one".to_owned()
}

const fn default_true() -> bool {
    true
}

/// A filesystem-safe token derived from a mod name (for generated archive names).
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Where a profile's instance lands when the manifest sets none. A Windows game
/// in a `CrossOver` bottle MUST run inside a bottle — a repo instance would
/// detect as native and be unlaunchable — so it defaults into the pristine's
/// bottle (`<bottle>/drive_c/concierge/<profile>`). Native games build under
/// the profile's `state/` (the Wabbajack model). An explicit instance always
/// wins over this (see `with_default_instance`).
fn default_instance(pristine: &Path, repo: &Path) -> PathBuf {
    if matches!(
        crate::runtime::detect(pristine),
        crate::runtime::Runtime::CrossOver { .. }
    ) {
        if let Some((bottle_head, _)) = pristine.to_str().and_then(|s| s.split_once("/drive_c/")) {
            let profile = repo.file_name().and_then(|n| n.to_str()).map_or_else(
                || "default".to_owned(),
                |n| n.replace(char::is_whitespace, "-"),
            );
            return PathBuf::from(format!("{bottle_head}/drive_c/concierge/{profile}"));
        }
    }
    repo.join("state").join("instance")
}

impl Mod {
    pub fn archive_file(&self) -> Result<String> {
        if let Some(f) = &self.file {
            return Ok(f.clone());
        }
        // pipeline mods: derive from the last producing step (http basename,
        // else a deterministic tarball named after the mod).
        if let Some(steps) = &self.pipeline {
            if let Some(url) = steps.iter().rev().find_map(|s| s.http.as_deref()) {
                if let Some(b) = url.rsplit('/').next().filter(|b| !b.is_empty()) {
                    return Ok(b.to_owned());
                }
            }
            return Ok(format!("{}.tar", safe_name(&self.name)));
        }
        if self.nix.is_some() {
            return Ok(format!("{}.tar", safe_name(&self.name)));
        }
        self.url
            .as_deref()
            .and_then(|u| u.rsplit('/').next())
            .filter(|b| !b.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                Error::Manifest(format!(
                    "mod '{}': needs `file` (or a `url` to derive it from)",
                    self.name
                ))
            })
    }

    pub const fn is_pinned(&self) -> bool {
        !self.md5.is_empty()
    }
}

impl Manifest {
    /// Load a profile's config. A `modpack.nix` (the Nix-language tier) takes
    /// precedence over `manifest.toml` (the zero-dependency tier); both produce
    /// the identical `Manifest`.
    pub fn load(repo: &Path) -> Result<Self> {
        #[cfg(feature = "nix-source")]
        {
            let nix = repo.join("modpack.nix");
            if nix.is_file() {
                return crate::nix::eval_manifest(&nix).map(|m| m.with_default_instance(repo));
            }
        }
        let path = repo.join("manifest.toml");
        let text = std::fs::read_to_string(&path).ctx(&path)?;
        Self::parse(&text).map(|m| m.with_default_instance(repo))
    }

    /// Wabbajack model: every profile builds in its OWN folder. Default the
    /// instance when the manifest doesn't set one, so mods never deploy into the
    /// real game/profile dir. An explicit `instance` is always left untouched.
    #[must_use]
    pub fn with_default_instance(mut self, repo: &Path) -> Self {
        if self.game.instance.is_none() {
            self.game.instance = Some(default_instance(&self.game.pristine, repo));
        }
        self
    }

    pub fn parse(text: &str) -> Result<Self> {
        Self::finish(toml::from_str(text)?)
    }

    /// Deserialize + validate a `Manifest` from JSON (the Nix-eval output).
    pub fn from_json(text: &str) -> Result<Self> {
        Self::finish(serde_json::from_str(text)?)
    }

    /// Shared post-processing: fold legacy fields, then validate. Keeps the
    /// TOML and JSON front-ends behaviourally identical.
    fn finish(mut m: Self) -> Result<Self> {
        if let Some(p) = m.game.plugins_txt.take() {
            m.game.paths.entry("plugins_txt".into()).or_insert(p);
        }
        if let Some(p) = m.game.my_games.take() {
            m.game.paths.entry("my_games".into()).or_insert(p);
        }
        // Fold each mod's `[mod.config]` (the preferred, separated form) onto the
        // flat fields the rest of the core reads. `config` values win.
        for md in &mut m.mods {
            if let Some(c) = md.config.take() {
                if c.install_root.is_some() {
                    md.install_root = c.install_root;
                }
                if c.subdir.is_some() {
                    md.subdir = c.subdir;
                }
                if !c.provides.is_empty() {
                    md.plugins = c.provides;
                }
                if !c.exclude.is_empty() {
                    md.exclude = c.exclude;
                }
                if !c.options.is_empty() {
                    md.options = c.options;
                }
                if c.group.is_some() {
                    md.group = c.group;
                }
                if !c.choices.is_empty() {
                    md.choices = c.choices;
                }
            }
            // Apply recorded selections: selected options contribute their
            // plugins to the mod's activation entries (declarative FOMOD).
            for choice in &md.choices {
                for opt in choice.options.iter().filter(|o| o.selected) {
                    md.plugins.extend(opt.plugins.iter().cloned());
                }
            }
        }
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for md in &self.mods {
            if !seen.insert(md.name.as_str()) {
                return Err(Error::Manifest(format!("duplicate mod name '{}'", md.name)));
            }
            if md.url.is_none()
                && md.nexus_mod_id.is_none()
                && md.file.is_none()
                && md.pipeline.is_none()
                && md.nix.is_none()
            {
                return Err(Error::Manifest(format!(
                    "mod '{}': no source (url, nexus_mod_id, pipeline, nix, or inbox file)",
                    md.name
                )));
            }
            if let Some(steps) = &md.pipeline {
                if steps.is_empty() {
                    return Err(Error::Manifest(format!(
                        "mod '{}': empty pipeline",
                        md.name
                    )));
                }
            }
            // A DISABLED entry may be parked unresolved (nexus_mod_id known,
            // file not yet picked via `nexus resolve`) — the honest state for
            // an unverified suggestion. Only enabled mods must be fetchable.
            if md.enabled {
                md.archive_file()?;
            }
            if !md.md5.is_empty()
                && (md.md5.len() != 32 || !md.md5.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Error::Manifest(format!(
                    "mod '{}': md5 must be 32 hex chars or empty",
                    md.name
                )));
            }
        }
        Ok(())
    }

    /// Check the declared relational **facts** (topology) against the enabled
    /// mod set: unmet `requires` and active `incompatible` pairs. Advisory
    /// (surfaced by the UI); facts inform, they don't block loading — resolution
    /// is a separate concern (patches/rules).
    #[must_use]
    pub fn relation_issues(&self) -> Vec<String> {
        use std::collections::{BTreeMap, BTreeSet};
        let enabled: BTreeMap<&str, &str> = self
            .mods
            .iter()
            .filter(|m| m.enabled)
            .map(|m| (m.name.as_str(), m.version.as_str()))
            .collect();
        let caps: BTreeSet<&str> = self
            .relations
            .provides
            .iter()
            .filter(|p| enabled.contains_key(p.name.as_str()))
            .map(|p| p.capability.as_str())
            .collect();
        let mut issues = Vec::new();
        for inc in &self.relations.incompatible {
            if enabled.contains_key(inc.a.as_str()) && enabled.contains_key(inc.b.as_str()) {
                let note = inc
                    .note
                    .as_deref()
                    .map_or_else(String::new, |n| format!(" ({n})"));
                issues.push(format!(
                    "incompatible: '{}' and '{}' are both enabled{note}",
                    inc.a, inc.b
                ));
            }
        }
        for req in &self.relations.requires {
            if !enabled.contains_key(req.name.as_str()) {
                continue;
            }
            if !enabled.contains_key(req.needs.as_str()) && !caps.contains(req.needs.as_str()) {
                issues.push(format!(
                    "requires: '{}' needs '{}', which is not enabled or provided",
                    req.name, req.needs
                ));
            } else if let (Some(min), Some(have)) =
                (&req.min_version, enabled.get(req.needs.as_str()))
            {
                if version_lt(have, min) {
                    issues.push(format!(
                        "requires: '{}' needs '{}' >= {min}, but {have} is present",
                        req.name, req.needs
                    ));
                }
            }
        }
        // ENVIRONMENT: game-version window vs the declared game version.
        let gv = &self.game.version;
        if let Some(min) = &self.compat.game_version {
            if version_lt(gv, min) {
                issues.push(format!(
                    "compat: pack needs game >= {min}, but game.version is {gv}"
                ));
            }
        }
        if let Some(max) = &self.compat.game_version_max {
            if version_lt(max, gv) {
                issues.push(format!(
                    "compat: pack supports game <= {max}, but game.version is {gv}"
                ));
            }
        }
        issues
    }
}

/// Numeric dot-separated version compare: is `a` strictly less than `b`?
/// Non-numeric components are treated as 0.
fn version_lt(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a
        .split('.')
        .map(|s| s.trim().parse().unwrap_or(0))
        .collect();
    let pb: Vec<u64> = b
        .split('.')
        .map(|s| s.trim().parse().unwrap_or(0))
        .collect();
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x < y;
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod schema_tests {
    use super::*;

    // The three concerns, written distinctly: WHAT (identity), HOW (config),
    // RELATIONAL (load order / patches / rules).
    const THREE_LAYER: &str = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6.1170"

[[mod]]
name = "SkyUI"
version = "5.2"
nexus_mod_id = 12604
file = "SkyUI.7z"
[mod.config]
install_root = "data"
provides = ["SkyUI_SE.esp"]
options = { fontsize = "big" }

[[mod]]
name = "AWKCR Patch"
version = "1"
url = "https://x/awkcr.7z"

[relations]
load_order = ["SkyUI_SE.esp", "AWKCRPatch.esp"]
[[relations.patch]]
name = "AWKCR Patch"
bridges = ["SkyUI", "SomeArmor"]
[[relations.rule]]
path = "meshes/x.nif"
winner = "SkyUI"
"#;

    #[test]
    fn three_layer_parses_and_folds() {
        let m = Manifest::parse(THREE_LAYER).unwrap();
        // WHAT
        assert_eq!(m.mods.len(), 2);
        assert_eq!(m.mods[0].name, "SkyUI");
        // HOW: [mod.config] folded onto flat fields
        assert_eq!(m.mods[0].install_root.as_deref(), Some("data"));
        assert_eq!(m.mods[0].plugins, vec!["SkyUI_SE.esp"]);
        assert_eq!(
            m.mods[0].options.get("fontsize").map(String::as_str),
            Some("big")
        );
        assert!(m.mods[0].config.is_none(), "config folded + cleared");
        // RELATIONAL
        assert_eq!(
            m.relations.load_order,
            vec!["SkyUI_SE.esp", "AWKCRPatch.esp"]
        );
        assert_eq!(m.relations.patches.len(), 1);
        assert_eq!(m.relations.patches[0].bridges, vec!["SkyUI", "SomeArmor"]);
        assert_eq!(m.relations.rules[0].winner, "SkyUI");
    }

    #[test]
    fn legacy_flat_form_still_parses() {
        // Old manifests (flat install_root/plugins, no [relations]) keep working.
        let flat = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6"
[[mod]]
name = "SkyUI"
version = "5.2"
nexus_mod_id = 12604
file = "SkyUI.7z"
install_root = "data"
plugins = ["SkyUI_SE.esp"]
"#;
        let m = Manifest::parse(flat).unwrap();
        assert_eq!(m.mods[0].plugins, vec!["SkyUI_SE.esp"]);
        assert!(m.relations.load_order.is_empty());
    }

    #[test]
    fn declared_facts_checked() {
        let t = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6"
[[mod]]
name = "Frost"
version = "2.0"
url = "https://x/frost.7z"
[[mod]]
name = "TrueStorms"
version = "1.5"
url = "https://x/ts.7z"
[[mod]]
name = "OldFramework"
version = "1.0"
url = "https://x/of.7z"
enabled = false
[relations]
[[relations.requires]]
name = "Frost"
needs = "weather-framework"
[[relations.requires]]
name = "Frost"
needs = "OldFramework"
min_version = "2.0"
[[relations.provides]]
name = "TrueStorms"
capability = "weather-framework"
[[relations.incompatible]]
a = "Frost"
b = "TrueStorms"
note = "both hook the weather system"
"#;
        let m = Manifest::parse(t).unwrap();
        let issues = m.relation_issues();
        // provides satisfies the "weather-framework" requirement → no issue for it
        assert!(
            !issues.iter().any(|i| i.contains("weather-framework")),
            "provides satisfies it: {issues:?}"
        );
        // OldFramework is disabled → the requires-min_version is unmet (not enabled)
        assert!(
            issues.iter().any(|i| i.contains("OldFramework")),
            "unmet dep flagged: {issues:?}"
        );
        // Frost + TrueStorms both enabled → incompatible flagged
        assert!(
            issues
                .iter()
                .any(|i| i.contains("incompatible") && i.contains("Frost")),
            "incompat flagged: {issues:?}"
        );
    }

    #[test]
    fn compat_game_version_window_checked() {
        let t = r#"
[game]
kind = "fallout4"
pristine = "/tmp/fo4"
version = "1.9.4"
[compat]
game_version = "1.10.163"
dlc = ["Automatron"]
script_extender = "0.7.8"
"#;
        let m = Manifest::parse(t).unwrap();
        assert_eq!(m.compat.dlc, vec!["Automatron"]);
        assert_eq!(m.compat.script_extender.as_deref(), Some("0.7.8"));
        // game.version 1.9.4 < required 1.10.163 → flagged
        assert!(
            m.relation_issues()
                .iter()
                .any(|i| i.contains("needs game >=")),
            "{:?}",
            m.relation_issues()
        );
    }

    #[test]
    fn metadata_trust_and_groups_parse() {
        let t = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6"
[[mod]]
name = "SkyUI"
version = "5.2"
url = "https://x/skyui.7z"
mirrors = ["https://mirror/skyui.7z"]
sha512 = "abc"
[mod.meta]
author = "SkyUI Team"
category = "UI"
tags = ["interface", "essential"]
nsfw = false
[mod.config]
group = "UI"
[relations]
[[relations.group]]
name = "UI"
after = ["Frameworks"]
"#;
        let m = Manifest::parse(t).unwrap();
        assert_eq!(m.mods[0].meta.author.as_deref(), Some("SkyUI Team"));
        assert_eq!(m.mods[0].meta.tags, vec!["interface", "essential"]);
        assert_eq!(m.mods[0].mirrors, vec!["https://mirror/skyui.7z"]);
        assert_eq!(m.mods[0].sha512.as_deref(), Some("abc"));
        assert_eq!(
            m.mods[0].group.as_deref(),
            Some("UI"),
            "group folded from config"
        );
        assert_eq!(m.relations.groups[0].after, vec!["Frameworks"]);
    }

    #[test]
    fn conditional_install_choices_apply_selected() {
        let t = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6"
[[mod]]
name = "BigMod"
version = "1"
url = "https://x/big.7z"
[mod.config]
provides = ["Big.esp"]
[[mod.config.choice]]
name = "Optional patches"
select = "any"
[[mod.config.choice.option]]
label = "USSEP patch"
selected = true
plugins = ["Big-USSEP.esp"]
[[mod.config.choice.option]]
label = "3DNPC patch"
selected = false
plugins = ["Big-3DNPC.esp"]
"#;
        let m = Manifest::parse(t).unwrap();
        assert_eq!(m.mods[0].choices.len(), 1);
        // base provides + the selected option's plugin; the unselected one is absent
        assert!(m.mods[0].plugins.contains(&"Big.esp".to_owned()));
        assert!(
            m.mods[0].plugins.contains(&"Big-USSEP.esp".to_owned()),
            "selected folded in"
        );
        assert!(
            !m.mods[0].plugins.contains(&"Big-3DNPC.esp".to_owned()),
            "unselected omitted"
        );
    }

    #[test]
    fn curate_block_parses() {
        let t = r#"
[game]
kind = "skyrimse"
pristine = "/tmp/sk"
version = "1.6"
[curate]
brief = "cozy lore-friendly survival"
min_endorsements = 5000
max_size_mb = 200
nsfw = false
lore_friendly = true
avoid = ["SomeAuthor", "cheaty mod"]
must_have = ["SkyUI"]
scope = "lean"
"#;
        let m = Manifest::parse(t).unwrap();
        assert_eq!(
            m.curate.brief.as_deref(),
            Some("cozy lore-friendly survival")
        );
        assert_eq!(m.curate.min_endorsements, 5000);
        assert_eq!(m.curate.max_size_mb, Some(200));
        assert!(m.curate.lore_friendly);
        assert_eq!(m.curate.avoid, vec!["SomeAuthor", "cheaty mod"]);
        assert_eq!(m.curate.must_have, vec!["SkyUI"]);
        assert_eq!(m.curate.scope.as_deref(), Some("lean"));
    }
}
