//! Repo layout — Wabbajack's model.
//!
//! One **shared, content-addressed download cache** (`store/`) and shared
//! `builds/` live at the WORKSPACE root, so a mod two profiles both use is
//! downloaded once. Each **profile** (`games/<game>/profiles/<name>/`) has its
//! own `manifest.toml` and `state/` (realized.json, plan.json, backups). The
//! pristine game install is never written.
//!
//! `fetch`/`build` resolve against the shared workspace; `realize`/drift/
//! undeploy state resolve against the active profile.

use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

use crate::error::{Error, IoCtx, Result};

#[derive(Debug, Clone)]
pub struct Repo {
    /// Workspace root: shared `store/` + `builds/` (the global downloads cache).
    pub workspace: PathBuf,
    /// Active profile dir: `manifest.toml` + `state/`. Equals `workspace` for
    /// a flat single-manifest repo (back-compat).
    pub profile: PathBuf,
}

/// Workspace-root markers, in priority order.
const WORKSPACE_MARKERS: &[&str] = &["Cargo.toml", "flake.nix", ".concierge-workspace"];

impl Repo {
    /// Discover the active profile from the cwd (nearest `manifest.toml`) and
    /// the shared workspace (nearest ancestor with a workspace marker, else the
    /// profile itself).
    pub fn discover() -> Result<Self> {
        if let Ok(r) = std::env::var("CONCIERGE_REPO") {
            let p = PathBuf::from(r);
            return Ok(Self::at(&p));
        }
        let start = std::env::current_dir().map_err(|source| Error::Io {
            path: PathBuf::from("."),
            source,
        })?;
        let mut d: &Path = &start;
        loop {
            // a profile is marked by either config tier: TOML or the Nix front-end
            if d.join("manifest.toml").exists() || d.join("modpack.nix").exists() {
                return Ok(Self::at(d));
            }
            match d.parent() {
                Some(p) => d = p,
                None => return Err(Error::RepoNotFound(start)),
            }
        }
    }

    /// Build a `Repo` for a profile dir, locating the shared workspace above it.
    pub fn at(profile: &Path) -> Self {
        let workspace = find_workspace(profile).unwrap_or_else(|| profile.to_path_buf());
        Self {
            workspace,
            profile: profile.to_path_buf(),
        }
    }

    /// Retarget to a different profile dir under the same workspace.
    #[must_use]
    pub fn with_profile(&self, profile: &Path) -> Self {
        Self {
            workspace: self.workspace.clone(),
            profile: profile.to_path_buf(),
        }
    }

    // --- shared cache (workspace) ------------------------------------------

    pub fn store(&self) -> PathBuf {
        // A user-configured download folder (Settings) overrides the default
        // per-workspace `store/`. The cache is content-addressed either way, so a
        // shared external folder dedupes across every workspace/profile.
        crate::settings::download_dir_override().unwrap_or_else(|| self.workspace.join("store"))
    }

    pub fn builds(&self) -> PathBuf {
        self.workspace.join("builds")
    }

    pub fn store_path(&self, md5: &str, file: &str) -> PathBuf {
        self.store().join(format!("{md5}-{file}"))
    }

    pub fn build_path(&self, md5: &str) -> PathBuf {
        self.builds().join(md5)
    }

    /// Legacy `ClickHouse` metadata store dir (kept for migration only).
    pub fn db_dir(&self) -> PathBuf {
        self.workspace.join("state").join("db")
    }

    /// The mod catalog — an embedded `SQLite` file, workspace-global. Cross-
    /// platform (no external database binary); the default catalog backend.
    pub fn catalog_path(&self) -> PathBuf {
        self.workspace.join("state").join("catalog.sqlite")
    }

    /// Shared cache of fetched load-order sort rules (community CC0 masterlists) —
    /// workspace-global.
    pub fn sortdata_dir(&self) -> PathBuf {
        self.workspace.join("state").join("sortdata")
    }

    // --- per-profile state -------------------------------------------------

    pub fn state_dir(&self) -> PathBuf {
        self.profile.join("state")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir().join("realized.json")
    }

    pub fn plan_file(&self) -> PathBuf {
        self.state_dir().join("plan.json")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.state_dir().join("backup")
    }

    /// The vanilla baseline is per-GAME (shared across that game's profiles):
    /// `games/<game>/vanilla-inventory.tsv`, or the profile dir for a flat repo.
    pub fn vanilla_inventory(&self) -> PathBuf {
        self.game_dir().join("vanilla-inventory.tsv")
    }

    /// The game dir owning this profile (`games/<game>/`), or the profile dir
    /// itself when not under a `profiles/<name>/` layout.
    pub fn game_dir(&self) -> PathBuf {
        if self
            .profile
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "profiles")
        {
            self.profile
                .parent()
                .and_then(Path::parent)
                .map_or_else(|| self.profile.clone(), Path::to_path_buf)
        } else {
            self.profile.clone()
        }
    }
}

/// Nearest ancestor of `profile` (excluding `profile`) that holds a workspace
/// marker, so a profile deep under `games/<g>/profiles/<p>/` finds the shared
/// root. Falls back to None (caller uses the profile as its own workspace).
fn find_workspace(profile: &Path) -> Option<PathBuf> {
    let mut d = profile.parent();
    while let Some(dir) = d {
        // A dev checkout has a Cargo.toml/flake.nix marker; a *packaged* install
        // has neither, so also recognize a workspace by its `games/` dir (same
        // rule as the free `workspace()` fn). Without this an installed
        // workspace can't find its own state/db (the mod catalog).
        let is_workspace =
            WORKSPACE_MARKERS.iter().any(|m| dir.join(m).exists()) || dir.join("games").is_dir();
        if is_workspace {
            return Some(dir.to_path_buf());
        }
        d = dir.parent();
    }
    None
}

pub fn md5_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).ctx(path)?;
    let mut h = Md5::new();
    std::io::copy(&mut f, &mut h).ctx(path)?;
    Ok(hex::encode(h.finalize()))
}

pub fn inbox_dir() -> PathBuf {
    home().join("Downloads")
}

pub fn home() -> PathBuf {
    // Cross-platform (HOME on Unix, USERPROFILE on Windows) — see concierge-platform.
    concierge_platform::home_dir()
}
