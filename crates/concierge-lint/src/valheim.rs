//! Valheim (`BepInEx`) invariants. Mods are plugin `.dll`s under
//! `BepInEx/plugins/`, loaded by the `BepInEx` runtime.
//!
//! Encoded (Error): if plugins are deployed, `BepInEx` must be installed
//! (`BepInEx/core/` present) or nothing loads (game runs vanilla); no two
//! plugin dlls share a filename (the "installed twice / two versions" break).
//!
//! Documented gap: `BepInPlugin` GUID uniqueness and `BepInDependency`
//! resolution require reading .NET assembly custom attributes (`Mono.Cecil`-style
//! metadata) — not yet parsed. See `research/game-invariants.md`.
//!
//! Source: <https://docs.bepinex.dev/articles/user_guide/installation/index.html>,
//! <https://docs.bepinex.dev/api/BepInEx.BepInPlugin.html>.

use std::collections::HashMap;
use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;

use crate::Violation;

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let plugins = concierge::realize::target_root(plan, "plugins")?;
    let game = concierge::realize::target_root(plan, "game").ok();
    let bepinex_present = game
        .as_deref()
        .is_some_and(|g| g.join("BepInEx").join("core").is_dir());
    Ok(check(&plugins, bepinex_present))
}

fn check(plugins: &Path, bepinex_present: bool) -> Vec<Violation> {
    let dlls = list_dlls(plugins);
    let mut out = Vec::new();
    if dlls.is_empty() {
        return out;
    }
    if !bepinex_present {
        out.push(Violation::error(
            "BepInEx",
            "bepinex-missing",
            "plugin DLLs are deployed but BepInEx (BepInEx/core) isn't installed — \
             none of them load (game runs vanilla)",
        ));
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for d in &dlls {
        *counts.entry(d.to_lowercase()).or_default() += 1;
    }
    for (name, n) in &counts {
        if *n > 1 {
            out.push(Violation::warn(
                name.clone(),
                "duplicate-plugin",
                format!(
                    "{n} copies of plugin `{name}` in plugins/ — BepInEx loads one, skips the rest"
                ),
            ));
        }
    }
    out
}

fn list_dlls(plugins: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(plugins, &mut out);
    out
}

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
        {
            if let Some(name) = path.file_name() {
                out.push(name.to_string_lossy().into_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_bepinex_and_dupes() {
        let root = std::env::temp_dir().join(format!("cc-vh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok();
        std::fs::write(root.join("ModA.dll"), b"MZ").ok();
        std::fs::create_dir_all(root.join("sub")).ok();
        std::fs::write(root.join("sub").join("ModA.dll"), b"MZ").ok();
        let v = check(&root, false);
        assert!(v.iter().any(|x| x.rule == "bepinex-missing"));
        assert!(v.iter().any(|x| x.rule == "duplicate-plugin"));
        // with BepInEx present + no dupes -> clean
        let clean = std::env::temp_dir().join(format!("cc-vh2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&clean);
        std::fs::create_dir_all(&clean).ok();
        std::fs::write(clean.join("Only.dll"), b"MZ").ok();
        assert!(check(&clean, true).is_empty());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&clean);
    }
}
