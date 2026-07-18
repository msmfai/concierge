//! Realization: make the live state match a Plan. Universal — the game
//! adapter already resolved everything game-specific into the Plan.
//! Steps: materialize instance if any mod targets it (`CoW` clone of pristine)
//! -> remove stale owned files -> hardlink desired files -> write the Plan's
//! config files -> record state. Idempotent; `fresh` re-clones first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, IoCtx, Result};
use crate::manifest::Manifest;
use crate::plan::{eval, Plan};
use crate::repo::{md5_file, Repo};
use crate::runtime::cow_clone;
use crate::state::{key, parse_key, OwnedFile, Realized};

#[derive(Debug, Default)]
pub struct RealizeReport {
    pub cloned_instance: bool,
    pub placed: usize,
    pub removed: usize,
    pub backed_up: usize,
    pub total_owned: usize,
}

/// One thing standing between the current declaration and a clean realize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightIssue {
    pub mod_name: String,
    pub kind: PreflightKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightKind {
    /// No `md5` — the archive isn't pinned (fetch, then pin).
    Unpinned,
    /// Pinned but the built tree is absent (needs fetch + build).
    NotDownloaded,
    /// The archive wraps its files in a single root folder that `subdir`
    /// doesn't strip — files would deploy one level too deep.
    LayoutUnstripped,
    /// The built tree has plugins the mod doesn't activate (they wouldn't load).
    PluginsUndetected,
}

impl PreflightKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unpinned => "unpinned",
            Self::NotDownloaded => "not downloaded",
            Self::LayoutUnstripped => "layout not resolved",
            Self::PluginsUndetected => "plugins not activated",
        }
    }
}

/// The outcome of a converged realize: what got auto-resolved, what's still
/// blocking (if it couldn't complete), and the deploy report when it did.
#[derive(Debug, Default)]
pub struct ConvergeReport {
    /// Human lines describing each auto-fix (pins written, layout resolved).
    pub resolved: Vec<String>,
    /// Issues that stopped realize (unfetchable downloads, etc.).
    pub blocked: Vec<String>,
    /// The deploy report, present only when realize actually ran.
    pub report: Option<RealizeReport>,
}

/// Inspect each mod's built tree and write any inferred install layout into the
/// manifest — strip a single versioned root folder into `subdir`, activate
/// detected plugins. Returns one human line per mod changed (empty = nothing
/// needed resolving); the caller re-evaluates the plan when it's non-empty.
/// Shared by the GUI's converge and the CLI's `realize` so both self-heal a
/// mod like JOURNEY (versioned root + `Journey.esp`) identically.
///
/// Note: this resolves subdir + plugins, NOT `install_root` — a script
/// extender (loader/DLL at the archive root, must install to the GAME root, not
/// Data) still needs its `install_root = "game"` set explicitly.
pub fn resolve_layouts(repo: &Repo, plan: &Plan) -> Result<Vec<String>> {
    let manifest_path = repo.profile.join("manifest.toml");
    // The game's own adapter decides whether a built archive is a foundational
    // tool it promotes (a script extender, …) — core stays ignorant of what any
    // tool is and only routes it to the root the adapter names.
    let adapter = crate::game::adapter_for(&plan.game.kind).ok();
    let mut resolved = Vec::new();
    for m in &plan.mods {
        let Some(md5) = &m.md5 else { continue };
        let build = repo.build_path(md5);
        let hint = crate::layout::infer_layout(&build);
        let need_subdir = hint.subdir.is_some() && m.subdir.is_none();
        // Only INFER plugins into an empty list. A non-empty `plugins` is a
        // deliberate curation — e.g. one variant chosen from a FOMOD that ships
        // eleven mutually-exclusive .esps — and must not be clobbered by
        // re-detecting every plugin in the build tree on each realize. Mirrors
        // `subdir`, which is only inferred when absent.
        let missing_plugins: Vec<String> = if m.plugins.is_empty() {
            hint.plugins
                .iter()
                .filter(|p| !m.plugins.contains(p))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        // A promoted foundational tool (e.g. a script extender, whose loader must
        // sit at the game root, not Data/) installs to the tool's own root. The
        // adapter recognises it from the archive's top-level files; core just
        // applies the root it names (a one-off fix; skip once already there).
        let promoted =
            adapter.and_then(|a| a.promoted_tool_for(&crate::layout::top_level_files(&build)));
        let promo_root = promoted
            .map(|t| t.install_root)
            .filter(|r| m.install_root != *r);
        let need_layout = need_subdir || !missing_plugins.is_empty();
        if !need_layout && promo_root.is_none() {
            continue;
        }
        let mut plugins = m.plugins.clone();
        plugins.extend(missing_plugins);
        let mut doc = std::fs::read_to_string(&manifest_path).ctx(&manifest_path)?;
        if need_layout {
            doc = crate::manifest_edit::set_layout(
                &doc,
                &m.name,
                need_subdir.then(|| hint.subdir.as_deref().unwrap_or_default()),
                &plugins,
            )?;
        }
        if let Some(root) = promo_root {
            doc = crate::manifest_edit::set_install_root(&doc, &m.name, root)?;
        }
        crate::manifest_edit::write_manifest(&manifest_path, &doc)?;
        let promo_note = match (promoted, promo_root) {
            (Some(t), Some(root)) => format!(" ({} → {root} root)", t.name),
            _ => String::new(),
        };
        resolved.push(format!(
            "resolved layout for {}{}{}{promo_note}",
            m.name,
            hint.subdir
                .as_deref()
                .filter(|_| need_subdir)
                .map_or(String::new(), |s| format!(" (strip '{s}')")),
            if need_layout && !plugins.is_empty() {
                format!(" (plugins: {})", plugins.join(", "))
            } else {
                String::new()
            },
        ));
    }
    Ok(resolved)
}

/// Fetch → **auto-pin** the manifest → build → **auto-resolve layout** (strip
/// versioned roots, activate detected plugins) → realize, re-evaluating the
/// plan after each manifest edit. This is the "Apply just works" path: a mod
/// added through the browser (md5 = "", no subdir/plugins) converges to a
/// realizable declaration instead of dead-ending on the first unpinned entry.
///
/// The caller must have registered the game adapters (so `eval` resolves).
pub fn realize_converged(repo: &Repo, fresh: bool) -> Result<ConvergeReport> {
    let mut out = ConvergeReport::default();
    let manifest_path = repo.profile.join("manifest.toml");
    let mut manifest = Manifest::load(&repo.profile)?;
    let mut plan = eval(&manifest)?;

    // 1. Fetch, then write any freshly-computed md5 pins back into the manifest.
    let mut pinned = false;
    for (name, outcome) in crate::store::fetch_all(repo, &plan)? {
        match outcome {
            crate::store::FetchOutcome::NeedsPin { md5, .. } => {
                let doc = std::fs::read_to_string(&manifest_path).ctx(&manifest_path)?;
                let doc = crate::manifest_edit::pin_mod(&doc, &name, &md5, None, None, None)?;
                crate::manifest_edit::write_manifest(&manifest_path, &doc)?;
                out.resolved.push(format!("pinned {name} -> md5 {md5}"));
                pinned = true;
            }
            crate::store::FetchOutcome::Blocked { instructions } => {
                out.blocked.push(format!("{name}: {instructions}"));
            }
            crate::store::FetchOutcome::Present(_) | crate::store::FetchOutcome::Stored(_) => {}
        }
    }
    if !out.blocked.is_empty() {
        return Ok(out); // can't build/realize without the archives
    }
    if pinned {
        manifest = Manifest::load(&repo.profile)?;
        plan = eval(&manifest)?;
    }

    // 2. Build the (now-pinned) archives.
    crate::build::build_all(repo, &plan)?;

    // 3. Inspect each built tree and resolve install layout into the manifest.
    let relaid = resolve_layouts(repo, &plan)?;
    if !relaid.is_empty() {
        out.resolved.extend(relaid);
        manifest = Manifest::load(&repo.profile)?;
        plan = eval(&manifest)?;
    }
    let _ = manifest;

    // 4. Deploy.
    out.report = Some(realize(repo, &plan, fresh)?);
    Ok(out)
}

/// Inspect the plan against the store/build trees and name every unresolved
/// item — so Apply can say WHY it can't proceed instead of failing opaquely
/// (or worse, deploying a mod into the wrong place). Read-only.
#[must_use]
pub fn preflight(repo: &Repo, plan: &Plan) -> Vec<PreflightIssue> {
    let mut issues = Vec::new();
    for m in &plan.mods {
        let issue = |kind, detail: String| PreflightIssue {
            mod_name: m.name.clone(),
            kind,
            detail,
        };
        let Some(md5) = &m.md5 else {
            issues.push(issue(
                PreflightKind::Unpinned,
                "no md5 — download the archive, then pin it".to_owned(),
            ));
            continue;
        };
        let build = repo.build_path(md5);
        if !build.exists() {
            issues.push(issue(
                PreflightKind::NotDownloaded,
                "archive not fetched/built yet".to_owned(),
            ));
            continue;
        }
        let hint = crate::layout::infer_layout(&build);
        if hint.subdir.is_some() && m.subdir.is_none() {
            issues.push(issue(
                PreflightKind::LayoutUnstripped,
                format!(
                    "archive root '{}' should be stripped (set subdir)",
                    hint.subdir.as_deref().unwrap_or_default()
                ),
            ));
        }
        // Plugins present in the tree but not declared — only a concern when
        // the mod declares NONE (nothing would load). A mod that curated its
        // own `plugins` intentionally leaves the rest inert (FOMOD variants),
        // so don't warn about them.
        let missing: Vec<&String> = if m.plugins.is_empty() {
            hint.plugins
                .iter()
                .filter(|p| !m.plugins.contains(p))
                .collect()
        } else {
            Vec::new()
        };
        if !missing.is_empty() {
            issues.push(issue(
                PreflightKind::PluginsUndetected,
                format!(
                    "{} not in plugins — won't load",
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    issues
}

pub fn target_root(plan: &Plan, root_name: &str) -> Result<PathBuf> {
    let root = plan
        .root_targets
        .get(root_name)
        .ok_or_else(|| Error::Other(format!("unknown install root '{root_name}' in plan")))?;
    resolve_root_path(root_name, root, plan.game_dir())
}

/// Resolve an install root to its on-disk directory, REFUSING an unconfigured
/// (empty) or relative absolute-root path — so a deploy never writes to the
/// wrong / current directory. Instance-relative roots hang off the (absolute)
/// game dir; absolute roots (BG3/Paradox `my_games` etc.) must be declared per-OS.
fn resolve_root_path(
    name: &str,
    root: &crate::plan::ResolvedRoot,
    game_dir: &str,
) -> Result<PathBuf> {
    if root.instance_relative {
        return Ok(PathBuf::from(game_dir).join(&root.dir));
    }
    if root.dir.trim().is_empty() {
        return Err(Error::Other(format!(
            "install root '{name}' has no path — set its [game.paths] key for your OS \
             (Windows: %USERPROFILE%\\Documents\\My Games\\…; CrossOver: the bottle's \
             drive_c/…/Documents/My Games/…; Proton: the steamapps/compatdata/<id>/pfx prefix)"
        )));
    }
    let dir = PathBuf::from(&root.dir);
    if !dir.is_absolute() {
        return Err(Error::Other(format!(
            "install root '{name}' path '{}' is not absolute — deploy needs an absolute path \
             so it never targets the wrong directory",
            root.dir
        )));
    }
    Ok(dir)
}

/// Reject an instance path blocked by a broken symlink ancestor (a symlink
/// whose target no longer exists — e.g. a stale bottle mount). Without this,
/// `create_dir_all` fails with a cryptic "File exists (os error 17)". Names the
/// offending link and its dangling target so the fix is obvious.
fn reject_broken_symlink_ancestor(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let Ok(meta) = std::fs::symlink_metadata(ancestor) else {
            continue; // doesn't exist yet — fine, it'll be created
        };
        if meta.file_type().is_symlink() && !ancestor.exists() {
            let target = std::fs::read_link(ancestor).unwrap_or_default();
            return Err(Error::Other(format!(
                "instance path is blocked by a broken symlink: {} -> {} (target missing). \
                 Remove/fix the symlink, or set the profile's `instance` to a real path.",
                ancestor.display(),
                target.display()
            )));
        }
    }
    Ok(())
}

pub fn materialize_instance(plan: &Plan, fresh: bool) -> Result<bool> {
    let Some(instance) = &plan.game.instance else {
        return Ok(false); // in-place mode: nothing to clone
    };
    if !plan.needs_instance() {
        return Ok(false);
    }
    let inst = PathBuf::from(instance);
    // A broken symlink anywhere in the instance path makes `create_dir_all`
    // fail with a cryptic EEXIST; catch it first with an actionable message.
    reject_broken_symlink_ancestor(&inst)?;
    if fresh && inst.exists() {
        let s = inst.display().to_string();
        if !s.contains("fo4nix") && !s.contains("concierge") {
            return Err(Error::UnsafeInstancePath(inst));
        }
        std::fs::remove_dir_all(&inst).ctx(&inst)?;
    }
    if inst.exists() {
        return Ok(false);
    }
    if let Some(parent) = inst.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    if plan.needs_game_clone() {
        // A mod touches the game tree — the instance IS a CoW copy of the game.
        cow_clone(Path::new(&plan.game.pristine), &inst)?;
    } else {
        // Mount-only (e.g. BG3): a lightweight instance dir that just hosts the
        // external mount(s); the base game is never copied.
        std::fs::create_dir_all(&inst).ctx(&inst)?;
    }
    Ok(true)
}

/// The `.concierge-pristine` parking suffix for a mounted real path.
const MOUNT_PARK: &str = ".concierge-pristine";

fn mount_park(real: &Path) -> PathBuf {
    let mut s = real.as_os_str().to_owned();
    s.push(MOUNT_PARK);
    PathBuf::from(s)
}

/// Wabbajack model: point the real external path at the instance-owned mount
/// dir. A real directory there is parked to `<real>.concierge-pristine` once and
/// never overwritten; our own symlink is repointed. Idempotent.
fn mount_path(real: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target).ctx(target)?;
    match std::fs::symlink_metadata(real) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(real).ctx(real)?;
        }
        Ok(_) => {
            let park = mount_park(real);
            if park.exists() {
                return Err(Error::Other(format!(
                    "mount: {} is a real path and a park already exists at {}; \
                     resolve manually to avoid touching it",
                    real.display(),
                    park.display()
                )));
            }
            std::fs::rename(real, &park).ctx(real)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = real.parent() {
                std::fs::create_dir_all(parent).ctx(parent)?;
            }
        }
        Err(e) => {
            return Err(Error::Other(format!("mount stat {}: {e}", real.display())));
        }
    }
    concierge_platform::symlink_dir(target, real).ctx(real)
}

/// Undo [`mount_path`]: drop our symlink and restore the parked real path.
fn unmount_path(real: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(real) {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(real).ctx(real)?;
        }
    }
    let park = mount_park(real);
    if park.exists() {
        std::fs::rename(&park, real).ctx(real)?;
    }
    Ok(())
}

/// Mount every external-mount root of the plan (deploy already put the files in
/// the instance-owned dir).
fn mount_all(plan: &Plan) -> Result<()> {
    for (dir, real) in plan.mounts() {
        let target = PathBuf::from(plan.game_dir()).join(&dir);
        mount_path(&PathBuf::from(&real), &target)?;
    }
    Ok(())
}

/// Desired file overlay: key -> (source path in builds/, owning mod).
/// Manifest order, last mod wins per path.
/// A per-mod DRY deploy: the files it would place (install root + destination)
/// and the activation entries it would turn on — computed by exactly the same
/// `desired_files` path realize uses, so a preview matches the real deploy,
/// without touching the instance.
#[derive(Debug, Clone)]
pub struct ModDeploy {
    pub name: String,
    /// (install-root name, destination path relative to that root).
    pub files: Vec<(String, String)>,
    /// Activation entries this mod turns on (Bethesda plugins, BG3 `uuid|Name`, …).
    pub plugins: Vec<String>,
    /// True when the mod isn't pinned yet — nothing to preview until fetched.
    pub unpinned: bool,
}

/// Preview what `realize` would deploy, without deploying. Builds pinned mods
/// from the store (pure, idempotent) and runs `desired_files`; unpinned mods are
/// reported (`unpinned: true`), not fetched. Order follows the plan.
pub fn preview(repo: &Repo, plan: &Plan) -> Result<Vec<ModDeploy>> {
    // Only pinned mods can be built/inspected; excluding the rest keeps
    // desired_files from erroring on an unfetched mod.
    let mut pinned = plan.clone();
    pinned.mods.retain(|m| m.md5.is_some());
    crate::build::build_all(repo, &pinned)?;
    let want = desired_files(repo, &pinned)?;

    let mut by_mod: BTreeMap<String, ModDeploy> = BTreeMap::new();
    for m in &plan.mods {
        by_mod.insert(
            m.name.clone(),
            ModDeploy {
                name: m.name.clone(),
                files: Vec::new(),
                plugins: m.plugins.clone(),
                unpinned: m.md5.is_none(),
            },
        );
    }
    for (k, (_src, mod_name)) in &want {
        if let Some((root, rel)) = parse_key(k) {
            if let Some(d) = by_mod.get_mut(mod_name) {
                d.files.push((root.to_owned(), rel.to_owned()));
            }
        }
    }
    for d in by_mod.values_mut() {
        d.files.sort();
    }
    Ok(plan
        .mods
        .iter()
        .filter_map(|m| by_mod.remove(&m.name))
        .collect())
}

/// The deploy want-set, deduped CASE-INSENSITIVELY: two mods that provide the
/// same file at differently-cased paths (`Interface/HUDMenu.swf` vs
/// `interface/HUDMenu.swf`) are the SAME physical file on a case-insensitive
/// filesystem (macOS/Windows/Wine). Tracking both double-records state and
/// spuriously drifts; here the last writer (mod/plan order) wins, like MO2.
#[derive(Default)]
struct WantSet {
    map: BTreeMap<String, (PathBuf, String)>,
    /// lowercased key → the actual-cased key currently in `map`.
    ci: std::collections::HashMap<String, String>,
}

impl WantSet {
    fn insert(&mut self, key: String, val: (PathBuf, String)) {
        if let Some(old) = self.ci.insert(key.to_lowercase(), key.clone()) {
            if old != key {
                self.map.remove(&old);
            }
        }
        self.map.insert(key, val);
    }
}

fn desired_files(repo: &Repo, plan: &Plan) -> Result<BTreeMap<String, (PathBuf, String)>> {
    let mut want = WantSet::default();
    for m in &plan.mods {
        let md5 = m.md5.as_ref().ok_or_else(|| Error::Unpinned {
            name: m.name.clone(),
        })?;
        let build = repo.build_path(md5);
        if !build.exists() {
            return Err(Error::StoreMiss {
                name: m.name.clone(),
                path: build,
            });
        }
        // A FOMOD mod installs exactly the files its ModuleConfig.xml maps for
        // the recorded selections — the Vortex/MO2 model. Everything else is a
        // plain subdir (+ selected-option subdir) copy.
        if m.fomod.is_some() {
            fomod_desired(&mut want, &build, m)?;
        } else {
            plain_desired(&mut want, &build, m)?;
        }
    }
    Ok(want.map)
}

/// Copy the base subdir (or the archive root), stripped to the install root,
/// minus any excluded path prefixes.
fn plain_desired(want: &mut WantSet, build: &Path, m: &crate::plan::PlannedMod) -> Result<()> {
    let root = m
        .subdir
        .as_ref()
        .map_or_else(|| build.to_path_buf(), |s| build.join(s));
    for entry in walkdir::WalkDir::new(&root).sort_by_file_name() {
        let entry = entry.map_err(|e| Error::Other(format!("walk {}: {e}", root.display())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&root)
            .map_err(|e| Error::Other(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        if m.exclude.iter().any(|ex| rel.starts_with(ex.as_str())) {
            continue;
        }
        want.insert(
            key(&m.install_root, &rel),
            (entry.path().to_path_buf(), m.name.clone()),
        );
    }
    Ok(())
}

/// Resolve a mod's `fomod/ModuleConfig.xml` against its recorded selections and
/// add the exact `source → destination` files the installer would copy.
fn fomod_desired(want: &mut WantSet, build: &Path, m: &crate::plan::PlannedMod) -> Result<()> {
    let Some(select) = m.fomod.as_ref() else {
        return plain_desired(want, build, m);
    };
    let root = fomod_root(build, m.subdir.as_deref()).ok_or_else(|| {
        Error::Other(format!(
            "{}: [mod.fomod] is set but the archive has no fomod/ModuleConfig.xml",
            m.name
        ))
    })?;
    let xml_path = ci_join(&root, "fomod/ModuleConfig.xml")
        .ok_or_else(|| Error::Other(format!("{}: fomod/ModuleConfig.xml vanished", m.name)))?;
    let bytes = std::fs::read(&xml_path).ctx(&xml_path)?;
    let cfg = concierge_fomod::parse(&bytes)
        .map_err(|e| Error::Other(format!("{}: FOMOD parse: {e}", m.name)))?;
    // Fail loudly on a typo'd pick: a select naming an option the installer
    // doesn't have would otherwise silently install nothing for that group.
    let names = cfg.option_names();
    let known: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    if let Some(bad) = select.iter().find(|s| !known.contains(s.as_str())) {
        return Err(Error::Other(format!(
            "{}: [mod.fomod] selects \"{bad}\" but the installer has no such option \
             (run `concierge fomod {}` to list them)",
            m.name, m.name
        )));
    }
    // Explicit picks override the installer's per-group defaults; an empty
    // `select` is just the defaults.
    let selected = cfg.selection_merged(&select.iter().cloned().collect());
    for item in cfg.resolve(&selected) {
        // A source the installer references but the archive lacks (e.g. an
        // option gated by another mod's presence) is simply skipped.
        let Some(src) = ci_join(&root, &item.source) else {
            continue;
        };
        if item.is_folder {
            for entry in walkdir::WalkDir::new(&src).sort_by_file_name() {
                let entry =
                    entry.map_err(|e| Error::Other(format!("walk {}: {e}", src.display())))?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let sub = entry
                    .path()
                    .strip_prefix(&src)
                    .map_err(|e| Error::Other(e.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let dest = join_rel(&item.destination, &sub);
                want.insert(
                    key(&m.install_root, &dest),
                    (entry.path().to_path_buf(), m.name.clone()),
                );
            }
        } else {
            want.insert(
                key(&m.install_root, &item.destination),
                (src, m.name.clone()),
            );
        }
    }
    Ok(())
}

/// `dest` joined with a folder-relative `sub` (either may be empty).
fn join_rel(dest: &str, sub: &str) -> String {
    match (dest.trim_matches('/'), sub) {
        ("", s) => s.to_owned(),
        (d, "") => d.to_owned(),
        (d, s) => format!("{d}/{s}"),
    }
}

/// The directory that FOMOD `source` paths are relative to — the parent of the
/// `fomod/` folder. Prefer `<build>/<subdir>`, then `<build>`, then a shallow
/// search (some archives wrap the whole FOMOD in a versioned dir).
fn fomod_root(build: &Path, subdir: Option<&str>) -> Option<PathBuf> {
    if let Some(s) = subdir {
        let r = build.join(s);
        if ci_join(&r, "fomod/ModuleConfig.xml").is_some() {
            return Some(r);
        }
    }
    if ci_join(build, "fomod/ModuleConfig.xml").is_some() {
        return Some(build.to_path_buf());
    }
    for entry in walkdir::WalkDir::new(build)
        .max_depth(3)
        .into_iter()
        .flatten()
    {
        if entry.file_name().eq_ignore_ascii_case("ModuleConfig.xml") {
            if let Some(fomod_dir) = entry.path().parent() {
                if fomod_dir
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("fomod"))
                {
                    return fomod_dir.parent().map(Path::to_path_buf);
                }
            }
        }
    }
    None
}

/// Resolve `rel` (forward- or back-slashed, any case) under `root` against the
/// real filesystem, matching each component case-insensitively. FOMOD `source`
/// attributes routinely disagree with the extracted archive's casing.
fn ci_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for comp in rel
        .replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
    {
        // fast path: exact hit
        let exact = cur.join(comp);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let mut found = None;
        for entry in std::fs::read_dir(&cur).ok()?.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(comp)
            {
                found = Some(entry.path());
                break;
            }
        }
        cur = found?;
    }
    Some(cur)
}

/// Parse a mod's FOMOD `ModuleConfig.xml` from its built tree, if it has one.
/// Powers `concierge fomod <mod>` inspection and select validation. `None` when
/// the mod isn't built or isn't a FOMOD.
pub fn mod_fomod_config(
    repo: &Repo,
    m: &crate::plan::PlannedMod,
) -> Result<Option<concierge_fomod::FomodConfig>> {
    let Some(md5) = &m.md5 else { return Ok(None) };
    let build = repo.build_path(md5);
    if !build.exists() {
        return Ok(None);
    }
    let Some(root) = fomod_root(&build, m.subdir.as_deref()) else {
        return Ok(None);
    };
    let Some(xml) = ci_join(&root, "fomod/ModuleConfig.xml") else {
        return Ok(None);
    };
    let bytes = std::fs::read(&xml).ctx(&xml)?;
    let cfg = concierge_fomod::parse(&bytes)
        .map_err(|e| Error::Other(format!("{}: FOMOD parse: {e}", m.name)))?;
    Ok(Some(cfg))
}

fn place(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    if dst.exists() {
        std::fs::remove_file(dst).ctx(dst)?;
    }
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst).ctx(dst)?;
    }
    Ok(())
}

fn same_inode(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.ino() == mb.ino() && ma.dev() == mb.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

pub fn realize(repo: &Repo, plan: &Plan, fresh: bool) -> Result<RealizeReport> {
    let cloned_instance = materialize_instance(plan, fresh)?;
    // A fresh realize resets recorded state; wipe the on-disk backup dir to
    // match, so a stale (read-only) backup from a prior run can't block a new
    // one with "Permission denied".
    if fresh {
        force_remove_dir_all(&repo.backup_dir());
    }
    let mut report = RealizeReport {
        cloned_instance,
        ..RealizeReport::default()
    };
    let mut state = if fresh || cloned_instance {
        Realized::default()
    } else {
        Realized::load(repo)?
    };
    let want = desired_files(repo, plan)?;
    report.total_owned = want.len();

    // remove owned files no longer wanted
    let unwanted: Vec<String> = state
        .files
        .keys()
        .filter(|k| !want.contains_key(*k))
        .cloned()
        .collect();
    for k in unwanted {
        if let Some((root, rel)) = parse_key(&k) {
            if let Ok(base) = target_root(plan, root) {
                let dst = base.join(rel);
                if dst.exists() {
                    std::fs::remove_file(&dst).ctx(&dst)?;
                    report.removed += 1;
                }
            }
        }
        state.files.remove(&k);
    }
    // A removed file can leave its parent dirs empty (an unpicked FOMOD option
    // folder, a mod that dropped a subtree). Managers leave none behind.
    if report.removed > 0 {
        if let Some(instance) = &plan.game.instance {
            prune_empty_dirs(&PathBuf::from(instance));
        }
    }

    // place wanted files
    for (k, (src, mod_name)) in &want {
        let Some((root, rel)) = parse_key(k) else {
            continue;
        };
        let dst = target_root(plan, root)?.join(rel);
        let src_md5 = md5_file(src)?;
        let current = state.files.get(k);
        if let Some(cur) = current {
            if cur.md5 == src_md5 && dst.exists() && same_inode(src, &dst) {
                continue;
            }
        }
        if dst.exists() && current.is_none() {
            let bak = repo.backup_dir().join(k.replace(':', "/"));
            if let Some(parent) = bak.parent() {
                std::fs::create_dir_all(parent).ctx(parent)?;
            }
            // A stale backup from a prior run may be read-only (copied from the
            // read-only store); clear it so the fresh copy doesn't EPERM.
            if bak.exists() {
                clear_read_only(&bak);
            }
            std::fs::copy(&dst, &bak).ctx(&bak)?;
            state.backups.push(k.clone());
            report.backed_up += 1;
        }
        place(src, &dst)?;
        state.files.insert(
            k.clone(),
            OwnedFile {
                md5: src_md5,
                mod_name: mod_name.clone(),
            },
        );
        report.placed += 1;
    }

    // Wabbajack model: expose the instance-owned mount dirs at their real
    // external paths (park+symlink), so the game reads mods from its own folder
    // without the real dir ever being mutated.
    mount_all(plan)?;

    write_configs(&plan.configs)?;
    state.plan_hash = Some(plan.hash()?);
    state.save(repo)?;
    Ok(report)
}

fn write_configs(configs: &[crate::plan::ConfigFile]) -> Result<()> {
    for c in configs {
        let path = PathBuf::from(&c.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ctx(parent)?;
        }
        // A prior deploy may have left this read-only (BG3 modsettings.lsx);
        // clear it so the write — and any later undeploy — succeeds.
        clear_read_only(&path);
        std::fs::write(&path, &c.content).ctx(&path)?;
        if c.read_only {
            set_read_only(&path)?;
        }
    }
    Ok(())
}

/// Best-effort: make `path` writable if it exists (no-op when absent). Used
/// before overwriting/removing a file a prior deploy marked read-only. We only
/// ever do this to files we own and are about to rewrite, so restoring broad
/// write permission is exactly the intent.
#[allow(clippy::permissions_set_readonly_false)]
fn clear_read_only(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

/// Remove a directory tree even if it holds read-only files/dirs (backups
/// copied from the read-only store are read-only). Best-effort — a fresh
/// realize wiping stale state must never hard-fail on a leftover permission.
fn force_remove_dir_all(path: &Path) {
    if !path.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
        clear_read_only(entry.path());
    }
    let _ = std::fs::remove_dir_all(path);
}

/// Mark `path` read-only (cross-platform via `Permissions::set_readonly`).
fn set_read_only(path: &Path) -> Result<()> {
    let mut perms = std::fs::metadata(path).ctx(path)?.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms).ctx(path)?;
    Ok(())
}

pub fn undeploy(repo: &Repo, plan: &Plan, force: bool) -> Result<(usize, usize)> {
    let mut state = Realized::load(repo)?;
    let mut removed = 0usize;
    let mut skipped = 0usize;
    let keys: Vec<String> = state.files.keys().cloned().collect();
    for k in keys {
        let Some((root, rel)) = parse_key(&k) else {
            continue;
        };
        let dst = target_root(plan, root)?.join(rel);
        if dst.exists() {
            let rec_md5 = state
                .files
                .get(&k)
                .map(|r| r.md5.clone())
                .unwrap_or_default();
            if !force && md5_file(&dst)? != rec_md5 {
                eprintln!("  SKIP      {k}: modified since realize (--force to remove)");
                skipped += 1;
                continue;
            }
            std::fs::remove_file(&dst).ctx(&dst)?;
            removed += 1;
        }
        if state.backups.contains(&k) {
            let bak = repo.backup_dir().join(k.replace(':', "/"));
            if bak.exists() {
                std::fs::copy(&bak, &dst).ctx(&dst)?;
            }
        }
        state.files.remove(&k);
    }
    // Wabbajack model: restore each mounted real path (drop our symlink, move
    // the parked pristine dir back) before pruning the instance.
    for (_dir, real) in plan.mounts() {
        unmount_path(&PathBuf::from(&real))?;
    }
    if let Some(instance) = &plan.game.instance {
        prune_empty_dirs(&PathBuf::from(instance));
    }
    state.plan_hash = None;
    state.backups.clear();
    state.save(repo)?;
    write_configs(&plan.config_resets)?;
    Ok((removed, skipped))
}

fn prune_empty_dirs(base: &Path) {
    if !base.exists() {
        return;
    }
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(base)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_dir())
        .map(|e| e.path().to_path_buf())
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        if d != base {
            let _ = std::fs::remove_dir(&d); // fails when non-empty: fine
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::WantSet;
    use super::{
        force_remove_dir_all, mount_park, mount_path, prune_empty_dirs,
        reject_broken_symlink_ancestor, resolve_root_path, set_read_only, unmount_path,
        write_configs,
    };
    use crate::plan::{ConfigFile, ResolvedRoot};
    use std::path::PathBuf;

    #[test]
    fn wantset_dedups_case_colliding_paths_last_wins() {
        // Two mods providing the same file at different-cased keys — one physical
        // file on a case-insensitive FS. The last writer wins; no phantom entry.
        let mut w = WantSet::default();
        w.insert(
            "data:Interface/HUDMenu.swf".into(),
            (PathBuf::from("/a"), "hudframework".into()),
        );
        w.insert(
            "data:interface/HUDMenu.swf".into(),
            (PathBuf::from("/b"), "def-ui".into()),
        );
        assert_eq!(w.map.len(), 1, "collapsed to one file");
        let (_, owner) = w.map.values().next().unwrap();
        assert_eq!(owner, "def-ui", "last writer wins");
        // A genuinely different file is kept alongside.
        w.insert("data:Foo.esp".into(), (PathBuf::from("/c"), "x".into()));
        assert_eq!(w.map.len(), 2);
    }

    #[test]
    fn prune_empty_dirs_removes_emptied_subtrees_keeps_populated() {
        let base = std::env::temp_dir().join(format!("cg-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // an emptied option folder + a still-populated one
        std::fs::create_dir_all(base.join("SurvivalOptions_None")).unwrap();
        std::fs::create_dir_all(base.join("keep")).unwrap();
        std::fs::write(base.join("keep/x.esp"), b"x").unwrap();
        prune_empty_dirs(&base);
        assert!(
            !base.join("SurvivalOptions_None").exists(),
            "empty option dir pruned"
        );
        assert!(base.join("keep").exists(), "populated dir kept");
        assert!(base.exists(), "base itself never removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn force_remove_dir_all_defeats_read_only_backups() {
        let base = std::env::temp_dir().join(format!("cg-forcerm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let f = base.join("data/fomod/info.xml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"stale").unwrap();
        set_read_only(&f).unwrap();
        assert!(std::fs::metadata(&f).unwrap().permissions().readonly());
        // a plain remove would EPERM the way `realize --fresh` did; force wins.
        force_remove_dir_all(&base);
        assert!(!base.exists(), "read-only backup tree fully removed");
    }

    #[test]
    #[cfg(unix)] // exercises a POSIX symlink; the guard itself is cross-platform
    fn broken_symlink_instance_gives_a_clear_error() {
        let base = std::env::temp_dir().join(format!("cg-brokenlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A symlink pointing at a missing target, with the instance "inside" it.
        let link = base.join("bottle-mount");
        std::os::unix::fs::symlink(base.join("gone"), &link).unwrap();
        let inst = link.join("game");

        let err = reject_broken_symlink_ancestor(&inst)
            .unwrap_err()
            .to_string();
        assert!(err.contains("broken symlink"), "actionable message: {err}");
        assert!(
            err.contains("bottle-mount"),
            "names the offending link: {err}"
        );

        // A real path (no broken link) passes.
        std::fs::create_dir_all(base.join("real")).unwrap();
        assert!(reject_broken_symlink_ancestor(&base.join("real/game")).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_root_refuses_unconfigured_or_relative_absolute_roots() {
        // an unconfigured (empty) absolute root -> refuse, don't target cwd
        let empty = ResolvedRoot {
            instance_relative: false,
            dir: String::new(),
            mount_at: None,
        };
        assert!(resolve_root_path("my_games", &empty, "/game").is_err());
        // a relative "absolute" root -> refuse
        let rel = ResolvedRoot {
            instance_relative: false,
            dir: "relative/mods".into(),
            mount_at: None,
        };
        assert!(resolve_root_path("profile_mods", &rel, "/game").is_err());
        // a real absolute root -> ok
        let abs = ResolvedRoot {
            instance_relative: false,
            dir: "/abs/mods".into(),
            mount_at: None,
        };
        assert_eq!(
            resolve_root_path("profile_mods", &abs, "/game").unwrap(),
            PathBuf::from("/abs/mods")
        );
        // instance-relative hangs off the (absolute) game dir, no abs requirement
        let ir = ResolvedRoot {
            instance_relative: true,
            dir: "data".into(),
            mount_at: None,
        };
        assert_eq!(
            resolve_root_path("data", &ir, "/game").unwrap(),
            PathBuf::from("/game/data")
        );
    }

    #[test]
    fn read_only_config_is_locked_and_a_reapply_still_overwrites() {
        let dir = std::env::temp_dir().join(format!("cfg-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("modsettings.lsx");
        let p = path.display().to_string();

        // first deploy: read-only file, locked on disk
        write_configs(&[ConfigFile::read_only(p.clone(), "v1".into())]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());

        // re-apply over the locked file must succeed (clear-before-write) and
        // re-lock — this is what makes Apply idempotent for BG3 modsettings
        write_configs(&[ConfigFile::read_only(p.clone(), "v2".into())]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());

        // a plain config overwriting the same path leaves it writable
        write_configs(&[ConfigFile::new(p, "v3".into())]).unwrap();
        assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mount_parks_the_real_dir_symlinks_the_instance_then_restores_on_unmount() {
        // The Wabbajack-model primitive: the base/profile dir is never mutated;
        // the modded content lives in the instance and is exposed via a symlink;
        // unmount restores the user's original bytes exactly.
        let base = std::env::temp_dir().join(format!("concierge-mount-{}", std::process::id()));
        let real = base.join("Mods"); // the game's real external mods dir
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("user.pak"), b"original").unwrap();
        let inst_mount = base.join("instance/mounts/mods");
        std::fs::create_dir_all(&inst_mount).unwrap();
        std::fs::write(inst_mount.join("mod.pak"), b"modded").unwrap();

        mount_path(&real, &inst_mount).unwrap();
        // the real path is now our symlink into the instance
        assert!(std::fs::symlink_metadata(&real)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(
            real.join("mod.pak").exists(),
            "instance content visible via symlink"
        );
        assert!(
            !real.join("user.pak").exists(),
            "the real dir is not in the way"
        );
        // the user's original bytes are parked, untouched
        assert_eq!(
            std::fs::read_to_string(mount_park(&real).join("user.pak")).unwrap(),
            "original"
        );

        unmount_path(&real).unwrap();
        assert!(
            !std::fs::symlink_metadata(&real)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink dropped"
        );
        assert_eq!(
            std::fs::read_to_string(real.join("user.pak")).unwrap(),
            "original",
            "the real dir is restored byte-for-byte"
        );
        std::fs::remove_dir_all(&base).ok();
    }
}
