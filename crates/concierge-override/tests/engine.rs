//! Prove the native KOTOR engine: a format-exact 2DA round-trips, and a
//! realistic `TSLPatcher` `changes.ini` applies (`AddRow` + `ExclusiveColumn`
//! dedup + `high()` + `2DAMEMORY`, `ChangeRow` by `RowLabel`); unsupported
//! lists error rather than silently skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use concierge_override::changes::{apply_2da_list, Ini};
use concierge_override::twoda::TwoDa;

fn sample_2da() -> TwoDa {
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
fn twoda_binary_round_trips() {
    let t = sample_2da();
    let bytes = t.serialize();
    assert_eq!(&bytes[..8], b"2DA V2.b");
    let back = TwoDa::parse(&bytes).unwrap();
    assert_eq!(back.columns, t.columns);
    assert_eq!(back.row_labels, t.row_labels);
    assert_eq!(back.cells, t.cells);
    assert_eq!(back.get(1, 1), Some("101"));
}

#[test]
fn changes_ini_addrow_changerow_apply() {
    // realistic mod changes.ini shape
    let ini_text = r"
[2DAList]
Table0=spells.2da

[spells.2da]
AddRow0=add_lightning
ChangeRow0=tweak_jump

[add_lightning]
ExclusiveColumn=label
label=FORCE_LIGHTNING
name=high()
spellid=high()
2DAMEMORY1=RowIndex

[tweak_jump]
RowLabel=1
name=999
";
    let ini = Ini::parse(ini_text).unwrap();
    let applied = apply_2da_list(&ini, |name| {
        assert_eq!(name, "spells.2da");
        Ok(sample_2da())
    })
    .unwrap();
    let (tables, mem) = (applied.tables, applied.memory);
    let t = &tables["spells.2da"];
    // AddRow appended a new FORCE_LIGHTNING row; high() gave 102 (max name 101 +1)
    assert_eq!(t.rows(), 3);
    let new = t.rows() - 1;
    assert_eq!(t.get(new, 0), Some("FORCE_LIGHTNING"));
    assert_eq!(t.get(new, 1), Some("102"), "high() over name column");
    // 2DAMEMORY1 captured the new row index
    assert_eq!(mem.twoda.get(&1).map(String::as_str), Some("2"));
    // ChangeRow by RowLabel edited row 1's name in place
    assert_eq!(t.get(1, 1), Some("999"));
    // and it round-trips as valid binary
    let _ = TwoDa::parse(&t.serialize()).unwrap();
}

#[test]
fn exclusive_column_dedups_instead_of_duplicating() {
    // adding a row whose ExclusiveColumn value already exists modifies it
    let ini_text = r"
[2DAList]
Table0=spells.2da
[spells.2da]
AddRow0=re_add
[re_add]
ExclusiveColumn=label
label=FORCE_JUMP
name=555
";
    let ini = Ini::parse(ini_text).unwrap();
    let t = apply_2da_list(&ini, |_| Ok(sample_2da())).unwrap().tables;
    let t = &t["spells.2da"];
    assert_eq!(t.rows(), 2, "no new row — existing FORCE_JUMP modified");
    assert_eq!(t.get(1, 1), Some("555"));
}

#[test]
fn unsupported_lists_reported_not_silently_skipped() {
    // a mod with only a [GFFList] applies no 2DA but SURFACES the unsupported
    // directive so an incomplete install is visible (never silent).
    let ini = Ini::parse("[GFFList]\nFile0=foo.utc\n").unwrap();
    let applied = apply_2da_list(&ini, |_| Ok(sample_2da())).unwrap();
    assert!(applied.tables.is_empty());
    assert!(
        applied
            .unsupported
            .iter()
            .any(|u| u.eq_ignore_ascii_case("[GFFList]")),
        "unsupported must be reported: {:?}",
        applied.unsupported
    );
}
