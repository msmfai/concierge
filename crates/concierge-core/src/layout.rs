//! Archive-layout inference: look at an extracted mod build tree and work out
//! how it should install — the `subdir` root to strip and the Bethesda
//! plugins it activates. This is what makes a mod like JOURNEY (whose archive
//! wraps everything in a versioned `JOURNEY … v1.6.1/` folder) deploy
//! correctly without the user hand-editing the manifest.
//!
//! Deliberately small and Bethesda-shaped (plugins = `.esp`/`.esm`/`.esl`);
//! FOMOD installer choices are handled by `concierge-fomod`.

use std::path::Path;

/// What inspection inferred about a mod's archive.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LayoutHint {
    /// The single top-level folder to treat as the install root (strip it), or
    /// None when files already sit at the archive root.
    pub subdir: Option<String>,
    /// Bethesda plugin filenames found at the (stripped) root, in sorted order.
    pub plugins: Vec<String>,
}

const PLUGIN_EXTS: [&str; 3] = ["esp", "esm", "esl"];

fn is_plugin(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| PLUGIN_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Infer the layout of the extracted archive at `build_root`. If the archive
/// has exactly one top-level directory and no top-level files, that folder is
/// the `subdir` to strip; plugins are then detected inside it. Otherwise files
/// are at the root and no subdir is inferred.
/// Game-data folders that live UNDER the install root (Data) and must NOT be
/// stripped — e.g. `F4SE/Plugins/*.bin` belongs at `Data/F4SE/…`, not `Data/`.
/// Only a wrapper folder (a mod-name/version dir, or the special `Data` dir)
/// gets stripped. Lowercase for case-insensitive comparison. Covers every
/// Bethesda game, not just Fallout 4: the script-extender roots below are the
/// per-game analogues of `f4se` (SKSE for Skyrim, OBSE for Oblivion, …) and
/// must be preserved the same way.
const DATA_SUBDIRS: &[&str] = &[
    // script extenders (one per Bethesda title)
    "f4se",
    "skse",
    "skse64",
    "obse",
    "nvse",
    "fose",
    "sfse",
    // common Bethesda Data-relative folders
    "textures",
    "meshes",
    "scripts",
    "interface",
    "sound",
    "music",
    "materials",
    "strings",
    "video",
    "mcm",
    "seq",
    "lodsettings",
    "vis",
    "programs",
    "shaders",
    "aaf",
    "misc",
    "fomod",
];

fn is_data_subdir(name: &str) -> bool {
    DATA_SUBDIRS.contains(&name.to_ascii_lowercase().as_str())
}

/// The top-level FILE names of an extracted archive (dirs excluded), or empty
/// if unreadable. A generic peek — core stays ignorant of what any file means;
/// a game adapter interprets these (e.g. to recognise a promoted tool by its
/// loader exe at the root).
#[must_use]
pub fn top_level_files(build_root: &Path) -> Vec<String> {
    top_level(build_root)
        .map(|(_, files)| files)
        .unwrap_or_default()
}

#[must_use]
pub fn infer_layout(build_root: &Path) -> LayoutHint {
    let Some((dirs, files)) = top_level(build_root) else {
        return LayoutHint::default();
    };
    // A single wrapping folder (and nothing beside it) is the classic
    // versioned-root case — strip it. But NOT if that folder is itself a
    // game-data dir (F4SE/Textures/…): those map under Data as-is. `Data`
    // itself IS a wrapper for Data-relative content, so it does strip.
    if let ([sub], true) = (dirs.as_slice(), files.is_empty()) {
        if sub.eq_ignore_ascii_case("data") || !is_data_subdir(sub) {
            let inner = build_root.join(sub);
            return LayoutHint {
                subdir: Some(sub.clone()),
                plugins: plugins_in(&inner),
            };
        }
    }
    // Files/data-folders already at the root — no strip; surface plugins from
    // the root and from a top-level Data/ if present.
    let mut plugins = plugins_in(build_root);
    let data = build_root.join("Data");
    if data.is_dir() {
        plugins.extend(plugins_in(&data));
    }
    plugins.sort();
    plugins.dedup();
    LayoutHint {
        subdir: None,
        plugins,
    }
}

/// (dirs, files) directly under `dir`, names only. None if unreadable.
fn top_level(dir: &Path) -> Option<(Vec<String>, Vec<String>)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".DS_Store" {
            continue;
        }
        match e.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push(name),
            Ok(ft) if ft.is_file() => files.push(name),
            _ => {}
        }
    }
    Some((dirs, files))
}

/// Plugin filenames directly under `dir`, sorted.
fn plugins_in(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_plugin(n))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn journey_versioned_root_is_stripped_and_plugin_detected() {
        let root = std::env::temp_dir().join(format!("cg-layout-j-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inner = "JOURNEY - Survival Settlement Fast Travel v1.6.1";
        touch(&root.join(inner).join("Journey.esp"));
        touch(&root.join(inner).join("Journey - Main.ba2"));
        let hint = infer_layout(&root);
        assert_eq!(hint.subdir.as_deref(), Some(inner));
        assert_eq!(hint.plugins, vec!["Journey.esp".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_at_root_need_no_strip() {
        let root = std::env::temp_dir().join(format!("cg-layout-r-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        touch(&root.join("Mod.esm"));
        touch(&root.join("textures").join("a.dds"));
        let hint = infer_layout(&root);
        assert_eq!(hint.subdir, None);
        assert_eq!(hint.plugins, vec!["Mod.esm".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn game_data_folders_are_not_stripped() {
        // address-library ships F4SE/Plugins/*.bin — the F4SE folder maps to
        // Data/F4SE and must NOT be stripped (else plugins land in Data/).
        let root = std::env::temp_dir().join(format!("cg-layout-f4se-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        touch(
            &root
                .join("F4SE")
                .join("Plugins")
                .join("version-1-11-221-0.bin"),
        );
        let hint = infer_layout(&root);
        assert_eq!(hint.subdir, None, "F4SE folder preserved, not stripped");

        // Same must hold for the other games' script extenders — a Skyrim mod's
        // SKSE/ wrapper must NOT be stripped either.
        let skse = std::env::temp_dir().join(format!("cg-layout-skse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&skse);
        touch(&skse.join("SKSE").join("Plugins").join("skee64.dll"));
        assert_eq!(
            infer_layout(&skse).subdir,
            None,
            "SKSE folder preserved too"
        );
        let _ = std::fs::remove_dir_all(&skse);

        // A `Data/` wrapper DOES strip (its contents are Data-relative).
        let root2 = std::env::temp_dir().join(format!("cg-layout-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root2);
        touch(&root2.join("Data").join("Textures").join("x.dds"));
        touch(&root2.join("Data").join("Mod.esp"));
        let hint2 = infer_layout(&root2);
        assert_eq!(hint2.subdir.as_deref(), Some("Data"));
        assert_eq!(hint2.plugins, vec!["Mod.esp".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    #[test]
    fn top_level_files_lists_root_files_not_dirs() {
        // The generic peek an adapter uses to recognise a promoted tool by its
        // loader exe at the root — core itself stays ignorant of what it means.
        let root = std::env::temp_dir().join(format!("cg-layout-tlf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        touch(&root.join("skse64_loader.exe"));
        touch(&root.join("skse64_1_5_97.dll"));
        touch(&root.join("Data").join("Scripts").join("Actor.pex"));
        let mut files = top_level_files(&root);
        files.sort();
        assert_eq!(files, vec!["skse64_1_5_97.dll", "skse64_loader.exe"]);
        // infer_layout must NOT strip it (loader + Data live at root together).
        assert_eq!(infer_layout(&root).subdir, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn multiple_top_level_dirs_are_not_stripped() {
        let root = std::env::temp_dir().join(format!("cg-layout-m-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        touch(&root.join("Data").join("A.esp"));
        touch(&root.join("Extras").join("readme.txt"));
        let hint = infer_layout(&root);
        assert_eq!(hint.subdir, None, "ambiguous — don't guess");
        let _ = std::fs::remove_dir_all(&root);
    }
}
