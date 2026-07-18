//! Finding the game install and what DLC the player actually owns — the two
//! things a fresh profile can't know on its own.
//!
//! `find_steam_install` resolves a title's folder the way Vortex/MO2 do: locate
//! Steam, read every library from `libraryfolders.vdf`, then the app's
//! `appmanifest_<id>.acf` for its `installdir`. `owned_base_plugins` then reads
//! which base/DLC masters are present in that install's `Data/` folder — so the
//! rendered load order carries only DLC the player owns, not the adapter's
//! assume-everything default (some players are missing some DLC).

use std::path::{Path, PathBuf};

/// Candidate Steam roots for this platform (first existing wins). Overridable
/// with `CONCIERGE_STEAM_ROOT` for non-default installs and tests.
fn steam_roots() -> Vec<PathBuf> {
    if let Some(r) = std::env::var_os("CONCIERGE_STEAM_ROOT") {
        return vec![PathBuf::from(r)];
    }
    let home = crate::repo::home();
    #[cfg(windows)]
    {
        let mut v = Vec::new();
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(pf) = std::env::var_os(var) {
                v.push(PathBuf::from(pf).join("Steam"));
            }
        }
        v.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        v
    }
    #[cfg(target_os = "macos")]
    {
        vec![home.join("Library/Application Support/Steam")]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            home.join(".steam/steam"),
            home.join(".local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        ]
    }
}

/// Every Steam library on this machine (each root plus the extra libraries its
/// `libraryfolders.vdf` lists).
fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in steam_roots() {
        if !root.exists() {
            continue;
        }
        libs.push(root.clone());
        if let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            for p in vdf_values(&text, "path") {
                libs.push(PathBuf::from(p));
            }
        }
    }
    libs.sort();
    libs.dedup();
    libs
}

/// The install folder for a Steam `app_id`, if it's installed — resolved via the
/// app manifest's `installdir` in whichever library holds it.
#[must_use]
pub fn find_steam_install(app_id: u32) -> Option<PathBuf> {
    let acf = format!("appmanifest_{app_id}.acf");
    for lib in steam_libraries() {
        let manifest = lib.join("steamapps").join(&acf);
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        if let Some(dir) = vdf_values(&text, "installdir").into_iter().next() {
            let install = lib.join("steamapps").join("common").join(dir);
            if install.exists() {
                return Some(install);
            }
        }
    }
    None
}

/// The base + DLC masters this install actually has, for a plugin-order game:
/// the base master (index 0) always, plus each further master whose `.esm` is
/// present in `Data/`. `None` for non-plugin-order games (nothing to own).
#[must_use]
pub fn owned_base_plugins(kind: &str, pristine: &Path) -> Option<Vec<String>> {
    let bases = crate::game::try_adapter(kind).and_then(crate::game::GameAdapter::plugin_bases)?;
    let data = pristine.join("Data");
    Some(
        bases
            .iter()
            .enumerate()
            .filter(|(i, esm)| *i == 0 || data.join(esm).exists())
            .map(|(_, esm)| (*esm).to_owned())
            .collect(),
    )
}

/// Pull every value for `key` out of a Valve VDF/ACF blob — lines shaped like
/// `"key"    "value"`. Handles the doubled backslashes VDF uses in paths.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&needle)?;
            let start = rest.find('"')? + 1;
            let end = rest.get(start..)?.find('"')? + start;
            Some(rest.get(start..end)?.replace("\\\\", "\\"))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn vdf_values_extracts_quoted_pairs_including_windows_paths() {
        let text = "\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"installdir\"\t\t\"Fallout 4\"";
        assert_eq!(
            vdf_values(text, "path"),
            vec!["D:\\SteamLibrary".to_owned()]
        );
        assert_eq!(vdf_values(text, "installdir"), vec!["Fallout 4".to_owned()]);
        // exact-key match: "path" must not also match a "pathx" key
        assert!(vdf_values("\t\"pathx\"\t\"nope\"", "path").is_empty());
    }

    #[test]
    fn finds_a_steam_install_through_library_and_app_manifest() {
        let base = std::env::temp_dir().join(format!("cg-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("Steam");
        let lib = base.join("Games");
        // The library that holds the app, and its appmanifest + install dir.
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib.join("steamapps/common/Fallout 4/Data")).unwrap();
        std::fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}",
                lib.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        std::fs::write(
            lib.join("steamapps/appmanifest_377160.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"377160\"\n\t\"installdir\"\t\t\"Fallout 4\"\n}",
        )
        .unwrap();

        std::env::set_var("CONCIERGE_STEAM_ROOT", &root);
        let found = find_steam_install(377_160);
        std::env::remove_var("CONCIERGE_STEAM_ROOT");

        assert_eq!(found, Some(lib.join("steamapps/common/Fallout 4")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
