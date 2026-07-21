//! Baldur's Gate 3 invariants. Mods are `.pak` (LSPK) files under the profile
//! `Mods/` dir; the global `modsettings.lsx` (XML) is the enabled/order list.
//!
//! Encoded (Error): if `.pak` mods are deployed, `modsettings.lsx` must actually
//! list mods (`ModuleShortDesc` entries) — a pak present but absent from
//! modsettings is silently disabled, and a subfolder or stale entry makes BG3
//! reset the load order (and can reject saves).
//!
//! Documented gaps (need an LSPK unpacker to read each pak's `meta.lsx`):
//! per-pak UUID/Folder ↔ modsettings reconciliation, dependency closure,
//! Script Extender requirement, `GustavX`-first (Patch 8 base module). See
//! `research/game-invariants.md`.
//!
//! Source: <https://bg3.wiki/wiki/Modding:Meta.lsx>,
//! <https://nexus-mods.github.io/NexusMods.App/developers/games/0003-BaldursGate3/>.

use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;

use crate::Violation;

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let mods = concierge::realize::target_root(plan, "mods")?;
    // modsettings.lsx is a rendered CONFIG (an absolute [game.paths] target),
    // not an install root — resolve it from the plan's configs and read the
    // deployed file (realize writes configs before linting).
    let ms_text = plan
        .configs
        .iter()
        .find(|c| c.path.ends_with("modsettings.lsx"))
        .and_then(|c| read_to_string_bounded(c.path.as_ref()));
    Ok(check(&mods, ms_text.as_deref()))
}

/// Read a file without letting a slow/blocking `open()` wedge the caller.
///
/// `modsettings.lsx` lives under the user's Documents, which on macOS is often
/// iCloud-managed: an *evicted* (dataless) file blocks `open()` while the OS
/// downloads it — for a long time, or forever when offline — and this runs on
/// the GUI thread during plan reload, so a naive read freezes the whole app on
/// launch. Bound it to a short budget on a worker thread; on timeout treat the
/// file as unreadable (`None`), exactly like a genuine read error, so the lint
/// simply skips the modsettings cross-check instead of hanging.
fn read_to_string_bounded(path: &Path) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let owned = path.to_path_buf();
    // The worker may stay blocked in `open()` if the file never materialises; it
    // unwinds when the OS finishes (or the process exits). That's fine — we've
    // already moved on.
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::read_to_string(&owned).ok());
    });
    rx.recv_timeout(std::time::Duration::from_millis(750))
        .ok()
        .flatten()
}

fn check(mods: &Path, modsettings: Option<&str>) -> Vec<Violation> {
    let paks = count_paks(mods);
    if paks == 0 {
        return Vec::new();
    }
    let listed = modsettings.map_or(0, count_module_entries);
    if listed == 0 {
        return vec![Violation::error(
            format!("{paks} pak(s)"),
            "not-in-modsettings",
            "mods are deployed but modsettings.lsx lists no mods — BG3 loads none of them \
             (and resets the load order on launch)",
        )];
    }
    Vec::new()
}

fn count_paks(mods: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(mods) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("pak"))
        })
        .count()
}

/// Larian's own base modules — present in every `modsettings.lsx` but not "a
/// mod", so a file listing only these still means no mods are enabled.
const BG3_BASE_UUIDS: &[&str] = &[
    "cb555efe-2d9e-131f-8195-a89329d218ea", // GustavX (Patch 8)
    "28ac9ce2-2aba-8cda-b3b5-6e922f71b6b8", // GustavDev
    "991c9c7a-fb80-40cb-8f0d-b92d4e80e9b1", // Gustav
    "ed539163-bb70-431b-96a7-f5b2eda5376b", // Shared
    "3d0c5ff8-c95d-c907-ff3e-34b204f1c630", // SharedDev
];

/// Number of NON-base `ModuleShortDesc` entries. A `modsettings` that lists only
/// Larian's base modules (e.g. `GustavX`) still loads zero mods — so the base
/// doesn't count toward "are any mods actually enabled?".
fn count_module_entries(xml: &str) -> usize {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return 0;
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("node") && n.attribute("id") == Some("ModuleShortDesc"))
        .filter(|n| {
            let uuid = n
                .children()
                .find(|a| a.has_tag_name("attribute") && a.attribute("id") == Some("UUID"))
                .and_then(|a| a.attribute("value"));
            uuid.is_none_or(|u| !BG3_BASE_UUIDS.contains(&u))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paks_without_modsettings_entries_flagged() {
        let root = std::env::temp_dir().join(format!("cc-bg3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok();
        std::fs::write(root.join("MyMod.pak"), b"LSPK").ok();
        // empty modsettings (only GustavDev base, no mod entries)
        let empty = r#"<save><region id="ModuleSettings"><node id="root"><children><node id="Mods"><children></children></node></children></node></region></save>"#;
        assert!(check(&root, Some(empty))
            .iter()
            .any(|v| v.rule == "not-in-modsettings"));

        // modsettings that lists the mod -> clean
        let listed = r#"<save><node id="Mods"><children><node id="ModuleShortDesc"><attribute id="UUID" value="x"/></node></children></node></save>"#;
        assert!(check(&root, Some(listed)).is_empty());

        // base-only (just GustavX, no real mod) is STILL flagged — the base
        // doesn't count as an enabled mod.
        let base_only = r#"<save><node id="Mods"><children><node id="ModuleShortDesc"><attribute id="UUID" value="cb555efe-2d9e-131f-8195-a89329d218ea"/></node></children></node></save>"#;
        assert!(check(&root, Some(base_only))
            .iter()
            .any(|v| v.rule == "not-in-modsettings"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
