//! KOTOR 2 (TSL) invariants. Mods land in a flat `Override/`; the correct,
//! non-destructive install mechanism is `TSLPatcher` (`tslpatchdata/changes.ini`)
//! which *appends* 2DA rows and TLK strings.
//!
//! Encoded (Warn — these clobber silently rather than hard-crash): a mod that
//! ships a bare whole-file `dialog.tlk` into Override overwrites every other
//! mod's appended strings; a mod that ships a bare `appearance.2da` / `spells.2da`
//! (etc.) as a loose full file clobbers other mods' appended rows.
//!
//! Documented gaps (per the research): the true Override filename-collision
//! graph and `TSLPatcher` emitted-file set require extracting every archive and
//! simulating the patch; install order is exogenous (the curated build list).
//! See `research/game-invariants.md`.
//!
//! Source: <https://deadlystream.com/files/file/1039-tsl-patcher-tlked-and-accessories/>,
//! <https://kotor.neocities.org/modding/mod_builds/k2/full>.

use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;

use crate::Violation;

/// Core 2DAs that mods should patch (append), never ship as a full overwrite.
const CLOBBER_PRONE_2DA: &[&str] = &[
    "appearance.2da",
    "heads.2da",
    "spells.2da",
    "feat.2da",
    "baseitems.2da",
    "portraits.2da",
];

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let overrides = concierge::realize::target_root(plan, "override")?;
    Ok(check_override(&overrides))
}

fn check_override(dir: &Path) -> Vec<Violation> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
            continue;
        };
        if name == "dialog.tlk" {
            out.push(Violation::warn(
                "dialog.tlk",
                "tlk-overwrite",
                "a full dialog.tlk in Override overwrites all other mods' appended strings — \
                 the mod should append via TSLPatcher [TLKList]/append.tlk",
            ));
        } else if CLOBBER_PRONE_2DA.contains(&name.as_str()) {
            out.push(Violation::warn(
                name,
                "bare-2da-overwrite",
                "a loose full 2DA in Override clobbers other mods' appended rows — \
                 it should be applied via TSLPatcher [2DAList] AddRow/ChangeRow",
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_bare_tlk_and_2da() {
        let root = std::env::temp_dir().join(format!("cc-k2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok();
        std::fs::write(root.join("dialog.tlk"), b"x").ok();
        std::fs::write(root.join("appearance.2da"), b"x").ok();
        std::fs::write(root.join("some_texture.tga"), b"x").ok(); // fine
        let v = check_override(&root);
        assert!(v.iter().any(|x| x.rule == "tlk-overwrite"));
        assert!(v.iter().any(|x| x.rule == "bare-2da-overwrite"));
        assert_eq!(v.len(), 2, "the .tga is not flagged");
        let _ = std::fs::remove_dir_all(&root);
    }
}
