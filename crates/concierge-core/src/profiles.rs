//! Games and profiles — the thin API the GUI (and the CLI) sit on. A game is
//! `games/<game>/`; a profile is `games/<game>/profiles/<name>/` with its own
//! `manifest.toml` and `state/`. All profiles of all games share the
//! workspace's one download cache (`store/`).

use std::path::{Path, PathBuf};

use crate::error::{Error, IoCtx, Result};
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct GameEntry {
    pub game: String,
    pub dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProfileEntry {
    pub name: String,
    pub dir: PathBuf,
}

/// The workspace root (shared cache) — the cwd or nearest ancestor holding a
/// workspace marker or a `games/` dir. Works even when the cwd has no
/// `manifest.toml` (e.g. the GUI launched from the repo root).
pub fn workspace() -> Result<PathBuf> {
    if let Ok(r) = std::env::var("CONCIERGE_REPO") {
        let p = PathBuf::from(r);
        // CONCIERGE_REPO is normally the workspace root itself (the installer
        // bakes it that way). If it already IS a workspace, use it directly —
        // do NOT walk up into an unrelated ancestor that happens to have a
        // `games/` dir (e.g. ~/games), which would hide the real games.
        if is_workspace(&p) {
            return Ok(p);
        }
        return Ok(Repo::at(&p).workspace);
    }
    let start = std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let mut d: &Path = &start;
    loop {
        if is_workspace(d) {
            return Ok(d.to_path_buf());
        }
        match d.parent() {
            Some(p) => d = p,
            None => return Err(Error::RepoNotFound(start)),
        }
    }
}

/// A directory is a workspace if it has a dev marker
/// (`Cargo.toml`/`flake.nix`/`.concierge-workspace`) OR a real `games/` tree —
/// i.e. `games/<game>/profiles/`, not just any `games/` dir. The stricter games
/// check means a bare, unrelated `~/games` folder is never mistaken for one.
fn is_workspace(dir: &Path) -> bool {
    if ["Cargo.toml", "flake.nix", ".concierge-workspace"]
        .iter()
        .any(|m| dir.join(m).exists())
    {
        return true;
    }
    std::fs::read_dir(dir.join("games")).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().join("profiles").is_dir())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod workspace_tests {
    use super::{is_workspace, workspace};

    #[test]
    fn is_workspace_needs_games_with_profiles() {
        let root = std::env::temp_dir().join(format!("cg-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // a real workspace: games/<game>/profiles/
        std::fs::create_dir_all(root.join("games").join("skyrimse").join("profiles")).unwrap();
        assert!(is_workspace(&root), "games/<game>/profiles/ is a workspace");
        // a BARE games/ folder (like ~/games/Morrowind, no profiles/) is NOT
        let bare = root.join("bare");
        std::fs::create_dir_all(bare.join("games").join("Morrowind")).unwrap();
        assert!(
            !is_workspace(&bare),
            "a bare games/ folder is not a workspace"
        );
        // an empty dir is not
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(!is_workspace(&empty), "an empty dir is not a workspace");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn concierge_repo_at_a_workspace_is_used_directly_not_an_ancestor() {
        // Regression: CONCIERGE_REPO set to a workspace root (with games/) must
        // resolve to ITSELF, not walk up into an ancestor that has a games/ dir.
        let root = std::env::temp_dir().join(format!("cg-repo-{}", std::process::id()));
        let inner = root.join("nested").join("workspace");
        let _ = std::fs::remove_dir_all(&root);
        // the ancestor (root) is ALSO a real workspace, to prove we use `inner`
        // itself and don't climb up to it.
        std::fs::create_dir_all(root.join("games").join("fallout4").join("profiles")).unwrap();
        std::fs::create_dir_all(inner.join("games").join("skyrimse").join("profiles")).unwrap();
        std::env::set_var("CONCIERGE_REPO", &inner);
        let ws = workspace().unwrap();
        std::env::remove_var("CONCIERGE_REPO");
        assert_eq!(ws, inner, "must use the workspace itself, not the ancestor");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Games under `<workspace>/games/` that have a `profiles/` directory.
pub fn list_games(workspace: &Path) -> Vec<GameEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(workspace.join("games")) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if dir.join("profiles").is_dir() {
            let game = dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            out.push(GameEntry { game, dir });
        }
    }
    out.sort_by(|a, b| a.game.cmp(&b.game));
    out
}

/// Profiles of a game (each `profiles/<name>/manifest.toml`).
pub fn list_profiles(game_dir: &Path) -> Vec<ProfileEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(game_dir.join("profiles")) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if dir.join("manifest.toml").is_file() {
            let name = dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            out.push(ProfileEntry { name, dir });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Create a new profile for a game, optionally cloning another profile's
/// manifest (the shared cache means the clone needs no re-download). Returns
/// the new profile dir. The instance path is left to the manifest — clones
/// should edit it so two profiles don't deploy to the same place.
pub fn create_profile(game_dir: &Path, name: &str, clone_from: Option<&Path>) -> Result<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\', ':']) {
        return Err(Error::Other(format!("invalid profile name '{name}'")));
    }
    let dir = game_dir.join("profiles").join(name);
    if dir.exists() {
        return Err(Error::Other(format!("profile '{name}' already exists")));
    }
    std::fs::create_dir_all(dir.join("state")).ctx(&dir)?;
    let manifest = dir.join("manifest.toml");
    if let Some(src) = clone_from {
        std::fs::copy(src.join("manifest.toml"), &manifest).ctx(&manifest)?;
    } else {
        // A minimal but VALID manifest so a fresh profile parses. The kind is
        // inferred from the game directory; the user (or the Concierge
        // assistant) fills in the real pristine path + version, then adds
        // `[[mod]]` entries.
        let kind = game_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("fallout4");
        let mut template = format!(
            "# New profile. Fill in the paths + version below, then add [[mod]]\n\
             # entries (or use the AI assistant to set it up).\n\
             [game]\n\
             kind = \"{kind}\"\n\
             pristine = \"\"   # absolute path to the game install (required)\n\
             version = \"1.0\"\n"
        );
        // Emit the adapter's required [game.paths] keys as placeholders so the
        // profile evals (an incomplete-but-valid starting point to fill in).
        if let Ok(adapter) = crate::game::adapter_for(kind) {
            use std::fmt::Write as _;
            let req = adapter.required_paths();
            if !req.is_empty() {
                template.push_str("\n[game.paths]\n");
                for key in req {
                    let _ = writeln!(template, "{key} = \"\"");
                }
            }
        }
        std::fs::write(&manifest, template).ctx(&manifest)?;
    }
    // Every profile is agent-ready from birth: guide + commands + allowlist.
    // Best-effort — a failed provision shouldn't kill creation.
    let kind = game_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("game");
    let _ = crate::provision::provision_profile(&dir, kind);
    Ok(dir)
}

/// Whether a profile is Locked — the manifest's read-only bit IS the state,
/// so the GUI, agents, `$EDITOR`, and an external `chmod` all see and honor
/// the same fact.
#[must_use]
pub fn is_locked(profile_dir: &Path) -> bool {
    std::fs::metadata(profile_dir.join("manifest.toml")).is_ok_and(|m| m.permissions().readonly())
}

/// Lock/unlock a profile by chmod'ing its declaration (manifest + lockfile)
/// read-only. The agent-shell sandbox additionally drops these paths from the
/// write-set while locked, so an agent inside the shell can't lift the chmod.
pub fn set_locked(profile_dir: &Path, locked: bool) -> Result<()> {
    for name in ["manifest.toml", "concierge.lock"] {
        let path = profile_dir.join(name);
        if !path.exists() {
            if name == "manifest.toml" {
                return Err(Error::Other(format!(
                    "not a profile (no manifest.toml): {}",
                    profile_dir.display()
                )));
            }
            continue;
        }
        // Unlock must lift the immutable flag BEFORE touching permissions;
        // lock sets permissions first, flag last.
        if !locked {
            set_immutable_flag(&path, false);
        }
        let mut perms = std::fs::metadata(&path).ctx(&path)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)] // unlocking is exactly the intent
        perms.set_readonly(locked);
        std::fs::set_permissions(&path, perms).ctx(&path)?;
        if locked {
            set_immutable_flag(&path, true);
        }
    }
    Ok(())
}

/// The read-only bit is the STATE, but a rename-over (how editors save
/// atomically) ignores it — on macOS the user-immutable flag closes that
/// hole. Best-effort: absent on other platforms, where the sandbox deny and
/// the `write_manifest` chokepoint still hold.
fn set_immutable_flag(path: &Path, on: bool) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/chflags")
            .arg(if on { "uchg" } else { "nouchg" })
            .arg(path)
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (path, on);
}

/// What the user (or an agent acting for them) knows about a game concierge
/// has no adapter for: where it is, and — optionally — how modding works.
/// Everything optional left empty means "no assumptions" (`kind = "generic"`,
/// pure file overlay); any modding knowledge upgrades it to a data-driven
/// `[game.custom]` manifest.
#[derive(Debug, Default)]
pub struct AdoptSpec {
    /// Absolute path to the game install (required).
    pub pristine: PathBuf,
    /// Game version string for the manifest (cosmetic; defaults to "unknown").
    pub version: String,
    /// Mods live in this dir INSIDE the game tree (instance-relative).
    pub mods_dir: Option<String>,
    /// Mods live at this EXTERNAL absolute path (becomes a Wabbajack mount).
    pub mods_path: Option<PathBuf>,
    /// Launch candidates (app bundle / exe names), most preferred first.
    pub launch: Vec<String>,
    /// Nexus domain, when the game has a Nexus community.
    pub nexus_domain: Option<String>,
    pub steam_app_id: Option<u32>,
}

impl AdoptSpec {
    /// Any declared modding knowledge means the custom (data-driven) shape;
    /// none means generic (pure overlay).
    const fn is_custom(&self) -> bool {
        self.mods_dir.is_some()
            || self.mods_path.is_some()
            || !self.launch.is_empty()
            || self.nexus_domain.is_some()
            || self.steam_app_id.is_some()
    }

    fn manifest(&self, name: &str) -> String {
        use std::fmt::Write as _;
        let mut m = format!(
            "# Adopted game — set up from a user/agent description ({}).\n\
             # Add [[mod]] entries (url or nexus_mod_id + md5 pin) and realize.\n\
             [game]\nkind = \"{}\"\npristine = \"{}\"\nversion = \"{}\"\n",
            if self.is_custom() {
                "data-driven [game.custom]"
            } else {
                "generic: mods just add/replace files, no other assumptions"
            },
            if self.is_custom() { name } else { "generic" },
            self.pristine.display(),
            self.version,
        );
        if !self.is_custom() {
            return m;
        }
        if let Some(p) = &self.mods_path {
            let _ = write!(m, "\n[game.paths]\nmods = \"{}\"\n", p.display());
        }
        let has_mods_root = self.mods_dir.is_some() || self.mods_path.is_some();
        let _ = write!(
            m,
            "\n[game.custom]\ndefault_root = \"{}\"\n",
            if has_mods_root { "mods" } else { "game" }
        );
        if let Some(d) = &self.nexus_domain {
            let _ = writeln!(m, "nexus_domain = \"{d}\"");
        }
        if !self.launch.is_empty() {
            let quoted: Vec<String> = self.launch.iter().map(|l| format!("\"{l}\"")).collect();
            let _ = writeln!(m, "launch = [{}]", quoted.join(", "));
        }
        if let Some(id) = self.steam_app_id {
            let _ = writeln!(m, "steam_app_id = {id}");
        }
        m.push_str("\n[[game.custom.root]]\nname = \"game\"\ndir = \"\"\n");
        if let Some(d) = &self.mods_dir {
            let _ = write!(
                m,
                "\n[[game.custom.root]]\nname = \"mods\"\ndir = \"{d}\"\n"
            );
        } else if self.mods_path.is_some() {
            m.push_str("\n[[game.custom.root]]\nname = \"mods\"\npath_key = \"mods\"\n");
        }
        m
    }
}

/// Adopt a game concierge doesn't know: create `games/<name>/profiles/default`
/// under the workspace from an [`AdoptSpec`]. This is the automation surface
/// behind the GUI's add-game wizard — a single call an agent can make once the
/// user explains where the game is and (optionally) how modding works.
pub fn adopt_game(workspace: &Path, name: &str, spec: &AdoptSpec) -> Result<PathBuf> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Other(format!(
            "invalid game name '{name}' (ascii letters/digits/-/_ only — it becomes games/<name>/)"
        )));
    }
    if !self_contained(&spec.pristine) {
        return Err(Error::Other(format!(
            "pristine must be an absolute path, got '{}'",
            spec.pristine.display()
        )));
    }
    if let Some(p) = &spec.mods_path {
        if !self_contained(p) {
            return Err(Error::Other(format!(
                "--mods-path must be absolute, got '{}'",
                p.display()
            )));
        }
    }
    let game_dir = workspace.join("games").join(name);
    let profile = game_dir.join("profiles").join("default");
    if profile.join("manifest.toml").exists() {
        return Err(Error::Other(format!(
            "games/{name}/profiles/default already exists — edit its manifest.toml instead"
        )));
    }
    std::fs::create_dir_all(&profile).ctx(&profile)?;
    let manifest = profile.join("manifest.toml");
    std::fs::write(&manifest, spec.manifest(name)).ctx(&manifest)?;
    let kind = if spec.is_custom() { name } else { "generic" };
    let _ = crate::provision::provision_profile(&profile, kind);
    Ok(profile)
}

/// Absolute and non-empty — the "never deploy to the wrong directory" rule.
fn self_contained(p: &Path) -> bool {
    p.is_absolute() && !p.as_os_str().is_empty()
}

/// Delete a profile — removes its whole directory (manifest, state, lock,
/// generations, save backups). Irreversible on disk; the shared download cache
/// is untouched. Refuses anything that isn't a profile (no `manifest.toml`).
pub fn delete_profile(profile_dir: &Path) -> Result<()> {
    if !profile_dir.join("manifest.toml").is_file() {
        return Err(Error::Other(format!(
            "not a profile (no manifest.toml): {}",
            profile_dir.display()
        )));
    }
    std::fs::remove_dir_all(profile_dir).ctx(profile_dir)?;
    Ok(())
}

/// Shared-cache stats for the GUI: archive count + total bytes in `store/`.
pub fn cache_stats(repo: &Repo) -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0;
    if let Ok(entries) = std::fs::read_dir(repo.store()) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file()) {
                count += 1;
                bytes += e.metadata().map_or(0, |m| m.len());
            }
        }
    }
    (count, bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // `new_profile_manifest_parses_and_evals` moved to `tests/pure_core.rs`: it
    // exercises a real adapter (required-path stubbing + eval), which needs the
    // single-linkage the integration-test crate provides (a lib unit test
    // compiles core twice — the injected registry wouldn't reach the harness).

    #[test]
    fn delete_profile_removes_it() {
        let base = std::env::temp_dir().join(format!("concierge-del-{}", std::process::id()));
        let game_dir = base.join("skyrimse");
        std::fs::create_dir_all(&game_dir).unwrap();
        let dir = create_profile(&game_dir, "gone", None).unwrap();
        assert!(dir.exists());
        delete_profile(&dir).unwrap();
        assert!(!dir.exists(), "profile dir removed");
        // refuses a non-profile dir
        std::fs::create_dir_all(base.join("notaprofile")).unwrap();
        assert!(delete_profile(&base.join("notaprofile")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
