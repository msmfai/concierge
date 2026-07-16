//! Evaluation: manifest -> Plan.
//!
//! `eval` is PURE — no network, no mutation, no reads outside the manifest.
//! The Plan is the fully-resolved desired state (the "derivation"): what to
//! fetch (fixed-output, hash-pinned), what to build (extractions), resolved
//! install-target roots, rendered config files, and launch candidates.
//! The game adapter runs HERE and only here — everything downstream of the
//! Plan is universal.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::manifest::Manifest;
use crate::runtime::{detect, Runtime};

pub const PLAN_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub schema: u32,
    pub game: GamePlan,
    /// Enabled mods in load order. Order is semantic: later wins file conflicts.
    pub mods: Vec<PlannedMod>,
    /// Named install roots, resolved by the adapter.
    pub root_targets: BTreeMap<String, ResolvedRoot>,
    /// Config files realize writes.
    pub configs: Vec<ConfigFile>,
    /// Config files undeploy writes (the no-mods state).
    pub config_resets: Vec<ConfigFile>,
    /// Instance-relative launch candidates, priority order.
    pub launch_candidates: Vec<String>,
    /// Steam app id for pristine launches of native Steam DRM titles.
    pub steam_app_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamePlan {
    pub kind: String,
    /// Nexus catalog domain, when the game lives on Nexus.
    pub nexus_domain: Option<String>,
    /// Modrinth search domain, when the game's mods live on Modrinth (Minecraft).
    pub modrinth_domain: Option<String>,
    pub runtime: Runtime,
    pub pristine: String,
    /// None = in-place mode: instance-relative roots resolve against pristine.
    pub instance: Option<String>,
    pub version: String,
    /// The game's save directory (`[game.paths].my_games` + `/Saves`), if
    /// known. Realize snapshots it to a versioned backup before deploying so a
    /// bad mod can't cause save-game data loss.
    #[serde(default)]
    pub saves: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRoot {
    /// true: `dir` is relative to the instance; false: `dir` is absolute.
    pub instance_relative: bool,
    pub dir: String,
    /// Wabbajack model: when set, mods deploy into the (instance-owned) `dir`
    /// and this REAL path is park+symlinked to it — so a game that reads mods
    /// from a fixed external location (BG3's Documents `Mods/`, The Sims 4) is
    /// modded in its own folder without ever mutating the real dir. `None` =
    /// an ordinary instance-relative or absolute root. `#[serde(default,
    /// skip_serializing_if)]` keeps mount-free plans byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub path: String,
    pub content: String,
    /// Mark the written file read-only. BG3's Patch 7+ in-game mod manager
    /// rewrites `modsettings.lsx` on launch and resets the load order unless the
    /// file is read-only — so the adapter that owns such a file sets this.
    /// `#[serde(default)]` keeps plans that predate this field hash-identical.
    #[serde(default)]
    pub read_only: bool,
}

impl ConfigFile {
    /// A normal, writable config file.
    #[must_use]
    pub const fn new(path: String, content: String) -> Self {
        Self {
            path,
            content,
            read_only: false,
        }
    }

    /// A config file the deployer marks read-only after writing (see
    /// [`ConfigFile::read_only`]).
    #[must_use]
    pub const fn read_only(path: String, content: String) -> Self {
        Self {
            path,
            content,
            read_only: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedMod {
    pub name: String,
    pub version: String,
    pub source: Source,
    /// Archive filename within the store.
    pub file: String,
    /// Content address. `None` = unpinned; realize will refuse until pinned.
    pub md5: Option<String>,
    /// Wabbajack's xxHash64 (base64) key, if known — used for hash-detecting a
    /// manually-downloaded file. `None` = unset. Omitted from the serialized
    /// plan when unset, so adding it left existing plan hashes byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xxhash: Option<String>,
    /// Named root from `root_targets`.
    pub install_root: String,
    pub subdir: Option<String>,
    /// Additional archive subdirs to install ON TOP of `subdir`, each stripped
    /// to the install root the same way — the file side of a selected FOMOD
    /// `[[mod.config.choice.option]]` (its `subdir`). Lets a mod merge a core
    /// folder plus a chosen option folder into one install (e.g. a mod whose
    /// assets sit at the archive root but whose plugin lives in an option dir).
    /// Omitted when empty, keeping choice-less plans byte-identical.
    /// FOMOD option picks (`Some` = install is driven by the archive's
    /// `fomod/ModuleConfig.xml` + these selections, ignoring `subdir`; an empty
    /// vec means take the installer's defaults). `None` = a plain subdir
    /// install. Omitted from the plan when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fomod: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub plugins: Vec<String>,
    /// Wabbajack model: when set, this mod's fetched `file` is a `BSDiff` patch,
    /// and its content is DERIVED at build time by applying it to a source the
    /// user owns (`patch.from`). Lets a modpack ship a patch instead of
    /// redistributing content. Omitted when unset, keeping non-patch plans
    /// byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<PatchSpec>,
}

/// A build-time binary-patch derivation: `to = apply(read(from), fetched patch)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSpec {
    /// Absolute path to the source file the user owns.
    pub from: String,
    /// Output filename (build-tree-relative) the derived target is written to.
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Url {
        url: String,
    },
    Nexus {
        mod_id: u64,
        file_id: u64,
    },
    /// User downloads in a browser; realize ingests from the inbox.
    Inbox,
    /// A declarative acquisition pipeline that produces the archive.
    Pipeline {
        steps: Vec<crate::pipeline::Step>,
    },
    /// A Nix fixed-output derivation (fetchGit/fetchurl/…). `nix build`s the
    /// expression into `/nix/store` — SRI/rev-verified by Nix — and the output
    /// lands in Concierge's store. The optional accelerator tier; needs Nix.
    Nix {
        expr: String,
    },
}

pub const GENERATED_BANNER: &str =
    "; GENERATED BY concierge — do not hand-edit; change manifest.toml and re-realize.";

pub fn eval(manifest: &Manifest) -> Result<Plan> {
    // First pass: collect activation entries (needed to render configs), then
    // ask the game (codified adapter OR data-driven [game.custom]) for its
    // shape, then validate mod install roots against it.
    // Load order (RELATIONAL): an explicit `[relations].load_order` wins;
    // otherwise it's derived from `[[mod]]` order + each mod's `provides`.
    let plugins: Vec<String> = if manifest.relations.load_order.is_empty() {
        manifest
            .mods
            .iter()
            .filter(|m| m.enabled)
            .flat_map(|m| m.plugins.iter().cloned())
            .collect()
    } else {
        manifest.relations.load_order.clone()
    };
    let shape = crate::game::shape_for(manifest, &plugins)?;

    let mut mods = Vec::new();
    for md in manifest.mods.iter().filter(|m| m.enabled) {
        let source = if let Some(steps) = &md.pipeline {
            Source::Pipeline {
                steps: steps.clone(),
            }
        } else if let Some(expr) = &md.nix {
            Source::Nix { expr: expr.clone() }
        } else {
            match (&md.url, md.nexus_mod_id, md.nexus_file_id) {
                (Some(url), _, _) => Source::Url { url: url.clone() },
                (None, Some(mod_id), Some(file_id)) => Source::Nexus { mod_id, file_id },
                _ => Source::Inbox,
            }
        };
        let install_root = md
            .install_root
            .clone()
            .unwrap_or_else(|| shape.default_root.clone());
        if !shape.root_targets.contains_key(&install_root) {
            return Err(crate::error::Error::Manifest(format!(
                "mod '{}': install_root '{install_root}' unknown for game '{}' (roots: {})",
                md.name,
                manifest.game.kind,
                shape
                    .root_targets
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        mods.push(PlannedMod {
            name: md.name.clone(),
            version: md.version.clone(),
            source,
            file: md.archive_file()?,
            md5: md.is_pinned().then(|| md.md5.clone()),
            xxhash: (!md.xxhash.is_empty()).then(|| md.xxhash.clone()),
            install_root,
            subdir: md.subdir.clone(),
            fomod: md.fomod.as_ref().map(|f| f.select.clone()),
            exclude: md.exclude.clone(),
            plugins: md.plugins.clone(),
            patch: md.patch.clone(),
        });
    }

    // Wabbajack model: an external mount (mods read from a fixed real dir like
    // BG3's Documents `Mods/`) deploys into an isolated instance and park+symlinks
    // the real dir to it. That REQUIRES an instance — without one, the mount would
    // resolve against pristine and write into the base game. Refuse, loudly.
    if manifest.game.instance.is_none() && shape.root_targets.values().any(|r| r.mount_at.is_some())
    {
        return Err(crate::error::Error::Manifest(format!(
            "game '{}' mods live in an external folder, so it must build in its own \
             instance — set [game].instance to a concierge-managed path (the real \
             folder is park+symlinked to it, never modified)",
            manifest.game.kind
        )));
    }

    Ok(Plan {
        schema: PLAN_SCHEMA,
        game: GamePlan {
            kind: manifest.game.kind.clone(),
            nexus_domain: shape.nexus_domain,
            modrinth_domain: shape.modrinth_domain,
            // The runtime is where the game actually *runs*: the instance's
            // bottle when instanced (it may live in a different, purpose-built
            // bottle than pristine), falling back to pristine otherwise.
            runtime: detect(
                manifest
                    .game
                    .instance
                    .as_ref()
                    .unwrap_or(&manifest.game.pristine),
            ),
            pristine: manifest.game.pristine.display().to_string(),
            instance: manifest
                .game
                .instance
                .as_ref()
                .map(|p| p.display().to_string()),
            version: manifest.game.version.clone(),
            saves: manifest
                .game
                .paths
                .get("my_games")
                .map(|p| p.join("Saves").display().to_string()),
        },
        mods,
        root_targets: shape.root_targets,
        configs: shape.configs,
        config_resets: shape.config_resets,
        launch_candidates: shape.launch_candidates,
        steam_app_id: shape.steam_app_id,
    })
}

impl Plan {
    /// Canonical serialization; the digest identifies the desired state.
    pub fn canonical_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn hash(&self) -> Result<String> {
        let mut h = Sha256::new();
        h.update(self.canonical_json()?.as_bytes());
        Ok(hex::encode(h.finalize()))
    }

    pub fn fully_pinned(&self) -> bool {
        self.mods.iter().all(|m| m.md5.is_some())
    }

    /// Does any mod actually target the game directory? If not, realize can
    /// skip instance materialization and launch runs from pristine.
    pub fn needs_instance(&self) -> bool {
        self.game.instance.is_some()
            && (self.mods.iter().any(|m| {
                self.root_targets
                    .get(&m.install_root)
                    .is_some_and(|r| r.instance_relative)
            }) || self.has_mounts())
    }

    /// Any root is an external mount (real path park+symlinked to the instance).
    #[must_use]
    pub fn has_mounts(&self) -> bool {
        self.root_targets.values().any(|r| r.mount_at.is_some())
    }

    /// The instance needs a `CoW` clone of the pristine GAME only when a mod
    /// targets the game tree (an instance-relative, non-mount root). A game that
    /// is modded purely through an external mount (e.g. BG3's Documents `Mods/`)
    /// gets a lightweight instance dir that just hosts the mount — the base game
    /// is never copied.
    #[must_use]
    pub fn needs_game_clone(&self) -> bool {
        self.game.instance.is_some()
            && self.mods.iter().any(|m| {
                self.root_targets
                    .get(&m.install_root)
                    .is_some_and(|r| r.instance_relative && r.mount_at.is_none())
            })
    }

    /// External-mount roots: `(instance-owned dir, real path to park+symlink)`.
    #[must_use]
    pub fn mounts(&self) -> Vec<(String, String)> {
        self.root_targets
            .values()
            .filter_map(|r| {
                r.mount_at
                    .as_ref()
                    .map(|real| (r.dir.clone(), real.clone()))
            })
            .collect()
    }

    /// DLC plugin names followed by mod plugin names, manifest order.
    pub fn game_dlc_and_plugins(&self) -> Vec<String> {
        self.configs
            .iter()
            .filter(|c| {
                std::path::Path::new(&c.path)
                    .file_name()
                    .is_some_and(|n| n == "plugins.txt")
            })
            .flat_map(|c| {
                c.content
                    .lines()
                    .filter_map(|l| l.strip_prefix('*'))
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Base directory instance-relative roots resolve against: the instance
    /// when one is declared, else the pristine install (in-place mode).
    pub fn game_dir(&self) -> &str {
        self.game.instance.as_deref().unwrap_or(&self.game.pristine)
    }
}
