//! KOTOR `diff_apply`: a `TSLPatcher` mod is applied as a field-level `2DA` diff
//! INTO the instance's Override, against the (CoW-of-pristine) base — never a
//! loose-drop of the mod's files, and never mutating the base.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use concierge::game::{DiffCtx, GameAdapter as _};
use concierge_override::adapter::KOTOR2;
use concierge_override::twoda::TwoDa;

fn sample_spells() -> TwoDa {
    let mut t = TwoDa {
        columns: vec!["label".into(), "name".into(), "spellid".into()],
        ..Default::default()
    };
    let r = t.add_row("0");
    t.set(r, 0, "FORCE_PUSH");
    t.set(r, 1, "100");
    t.set(r, 2, "0");
    let r = t.add_row("1");
    t.set(r, 0, "FORCE_JUMP");
    t.set(r, 1, "101");
    t.set(r, 2, "1");
    t
}

#[test]
fn tslpatcher_mod_is_merged_into_the_instance_override_not_loose_dropped() {
    let base = std::env::temp_dir().join(format!("concierge-diff-{}", std::process::id()));
    // a materialized instance with the KOTOR layout (dialog.tlk present)
    let gamedata = base.join("instance/KOTOR2.app/Contents/GameData");
    std::fs::create_dir_all(&gamedata).unwrap();
    std::fs::write(gamedata.join("dialog.tlk"), b"TLK ").unwrap();

    // a mod build tree: tslpatchdata/{changes.ini, spells.2da}
    let tsl = base.join("build/tslpatchdata");
    std::fs::create_dir_all(&tsl).unwrap();
    let source_2da = sample_spells().serialize();
    std::fs::write(tsl.join("spells.2da"), &source_2da).unwrap();
    std::fs::write(
        tsl.join("changes.ini"),
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=add_lightning\n\n\
         [add_lightning]\nExclusiveColumn=label\nlabel=FORCE_LIGHTNING\nname=high()\nspellid=high()\n",
    )
    .unwrap();

    KOTOR2
        .diff_apply(&DiffCtx {
            instance_dir: &base.join("instance"),
            base_dir: &base.join("instance"),
            mods: vec![("stars".into(), base.join("build"))],
        })
        .unwrap();

    // the MERGED 2da landed in the instance Override (3 rows = 2 base + 1 added),
    // i.e. the diff was applied — not a raw copy of the 2-row source.
    let out = gamedata.join("Override/spells.2da");
    assert!(out.is_file(), "merged 2da written to the instance Override");
    let merged = TwoDa::parse(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(merged.rows(), 3, "AddRow applied");
    assert_eq!(merged.get(2, 0), Some("FORCE_LIGHTNING"));

    // the source (the mod's shipped 2da) is untouched — non-destructive.
    let src = TwoDa::parse(&std::fs::read(tsl.join("spells.2da")).unwrap()).unwrap();
    assert_eq!(src.rows(), 2, "the mod source is not mutated");

    std::fs::remove_dir_all(&base).ok();
}
