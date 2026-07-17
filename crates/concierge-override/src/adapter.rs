//! The KOTOR game adapters (KOTOR 1 + KOTOR II). Deploy is DIFF-application,
//! not loose-drop: a `TSLPatcher` mod is a field-level `2DA`/`TLK` diff
//! (`changes.ini`), which `diff_apply` merges against the base into the
//! instance's Override using this crate's own engine — so the base game is
//! never mutated and mods stack reproducibly (the Wabbajack model).
//! Non-`TSLPatcher` loose files still ride the generic overlay.

use concierge::error::{Error, Result};
use concierge::game::{DiffCtx, GameAdapter, Lexicon, RootTarget, KOTOR_LEXICON};
use concierge::manifest::Manifest;
use concierge::plan::ConfigFile;

/// Resolve the KOTOR game-data dir (Aspyr macOS bundles or a bare Windows
/// layout) inside `root` — the one that actually holds `dialog.tlk`.
fn gamedata(root: &std::path::Path) -> Option<std::path::PathBuf> {
    [
        "KOTOR2.app/Contents/GameData",
        "swkotor2.app/Contents/GameData",
        "Knights of the Old Republic.app/Contents/Assets",
        "",
    ]
    .into_iter()
    .map(|rel| root.join(rel))
    .find(|c| c.join("dialog.tlk").is_file())
}

/// The minimal adapter, parameterized per KOTOR game: loose files into the
/// override dir, no loader, no ordering, no config. Exists in the basis
/// precisely because it proves how small the universal surface can go.
#[derive(Debug)]
pub struct Kotor {
    kind: &'static str,
    roots: &'static [(&'static str, RootTarget)],
    launch: &'static [&'static str],
    app_id: u32,
}

impl GameAdapter for Kotor {
    fn lexicon(&self) -> Lexicon {
        KOTOR_LEXICON
    }
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn nexus_domain(&self) -> Option<&'static str> {
        Some(self.kind)
    }
    fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
        self.roots
    }
    fn default_install_root(&self) -> &'static str {
        "override"
    }
    fn required_paths(&self) -> &'static [&'static str] {
        &[]
    }
    fn render_configs(&self, _m: &Manifest, _plugins: &[String]) -> Result<Vec<ConfigFile>> {
        Ok(Vec::new())
    }
    fn launch_candidates(&self) -> &'static [&'static str] {
        self.launch
    }
    fn steam_app_id(&self) -> Option<u32> {
        Some(self.app_id)
    }
    fn diff_apply(&self, ctx: &DiffCtx) -> Result<()> {
        // Apply each TSLPatcher mod's changes.ini as a field-level diff into the
        // instance's Override, against the instance's (CoW-of-pristine) base. The
        // pristine game is never modified; the merged 2DA/dialog.tlk are
        // materialized only in the instance.
        let Some(gamedata) = gamedata(ctx.instance_dir) else {
            return Ok(()); // no resolvable KOTOR layout in the instance
        };
        // KOTOR II ships `Override/`; the Aspyr KOTOR 1 bundle `override/`.
        let override_dir = ["Override", "override"]
            .into_iter()
            .map(|n| gamedata.join(n))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| gamedata.join("Override"));
        let dialog = gamedata.join("dialog.tlk");
        for (name, build) in &ctx.mods {
            let Some(tsl) = crate::install::find_tslpatchdata(build) else {
                continue; // a non-TSLPatcher mod — the generic overlay handled it
            };
            crate::install::install(&tsl, &override_dir, Some(&dialog))
                .map_err(|e| Error::Other(format!("kotor TSLPatcher '{name}': {e}")))?;
        }
        Ok(())
    }
    fn agent_guide(&self) -> Option<String> {
        // The community super-patch differs by title — name the right one.
        let superpatch = if self.kind == "kotor2" {
            "For KOTOR II install **TSLRCM** (The Sith Lords Restored Content Mod) first, and \
             **M4-78EP** if you want the restored droid planet — most other K2 mods are built \
             against them."
        } else {
            "For KOTOR I the baseline is the **K1 Community Patch (K1CP)** — install it first; many \
             mods assume it."
        };
        Some(format!(
            "- **Install via TSLPatcher/HoloPatcher — Concierge does the merge.** Most KOTOR mods \
             ship a `tslpatchdata/changes.ini` (field-level 2DA/TLK/GFF edits). Concierge applies \
             each as a diff into the instance's `Override/` against the base, so you never run the \
             patcher by hand and the pristine game is never touched.\n\
             - **The community super-patch comes first.** {superpatch}\n\
             - **Override precedence:** a loose file in `Override/` beats the game's archives; when \
             two mods provide the same file, later-installed wins. TSLPatcher 2DA/TLK merges STACK \
             rather than clobber — that's why order-sensitive mods ship as patchers.\n\
             - **Mods live on Nexus and Deadly Stream** (the KOTOR hub). Essentials like TSLRCM and \
             M4-78EP are Deadly Stream releases — add them by URL."
        ))
    }
}

pub static KOTOR2: Kotor = Kotor {
    kind: "kotor2",
    roots: &[
        ("game", RootTarget::InstanceRel("")),
        // Aspyr macOS port: game data lives INSIDE the app bundle. A
        // Windows KOTOR2 needs a port-specific layout hook (SPEC open item).
        (
            "override",
            RootTarget::InstanceRel("KOTOR2.app/Contents/GameData/Override"),
        ),
    ],
    launch: &["KOTOR2.app", "swkotor2.exe"],
    app_id: 208_580,
};

pub static KOTOR1: Kotor = Kotor {
    kind: "kotor",
    roots: &[
        ("game", RootTarget::InstanceRel("")),
        // Aspyr macOS port: data under Contents/Assets, lowercase override.
        (
            "override",
            RootTarget::InstanceRel("Knights of the Old Republic.app/Contents/Assets/override"),
        ),
    ],
    launch: &["Knights of the Old Republic.app", "swkotor.exe"],
    app_id: 32_370,
};

/// Resolve a KOTOR game `kind` to its adapter (the family's registry entry).
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "kotor2" => Some(&KOTOR2),
        "kotor" => Some(&KOTOR1),
        _ => None,
    }
}

/// The game kinds this family serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["kotor2", "kotor"]
}
