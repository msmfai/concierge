//! Property-based coverage for the KOTOR 2DA format + changes.ini apply.
//! Round-trip is the involution oracle; the apply invariants encode what a
//! merge must and must not do.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use concierge_override::changes::{apply_2da_list, Ini};
use concierge_override::twoda::TwoDa;
use proptest::prelude::*;

// 2DA cells/labels are tab- and NUL-delimited on disk, so realistic content
// excludes those bytes; generate safe identifier-ish strings (and empties).
fn token() -> impl Strategy<Value = String> {
    "[a-z0-9_]{0,8}"
}

fn table_strategy() -> impl Strategy<Value = TwoDa> {
    (1usize..5, 0usize..6).prop_flat_map(|(cols, rows)| {
        (
            proptest::collection::vec(token(), cols),
            proptest::collection::vec(token(), rows),
            proptest::collection::vec(proptest::collection::vec(token(), cols), rows),
        )
            .prop_map(|(columns, row_labels, cells)| TwoDa {
                columns,
                row_labels,
                cells,
            })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn twoda_serialize_parse_is_identity(t in table_strategy()) {
        // involution: parse ∘ serialize == id, incl. the deduplicated string pool
        let bytes = t.serialize();
        let back = TwoDa::parse(&bytes).expect("serialized 2DA must parse");
        prop_assert_eq!(&back.columns, &t.columns);
        prop_assert_eq!(&back.row_labels, &t.row_labels);
        prop_assert_eq!(&back.cells, &t.cells);
        // and it is a stable fixed point (serialize again == same bytes)
        prop_assert_eq!(back.serialize(), bytes);
    }

    #[test]
    fn addrow_grows_by_one_changerow_preserves_count(t in table_strategy(), v in token()) {
        prop_assume!(t.column_index("label").is_none()); // avoid collision with our fixture col
        // give the table a known 'label' column so directives target something
        let mut base = t;
        base.columns.push("label".to_owned());
        for row in &mut base.cells {
            row.push(String::new());
        }
        let rows0 = base.rows();

        // AddRow with a fresh label -> exactly one more row
        let ini = Ini::parse(&format!(
            "[2DAList]\nT0=x.2da\n[x.2da]\nAddRow0=a\n[a]\nlabel=UNIQUE_{v}z\n"
        )).unwrap();
        let out = apply_2da_list(&ini, |_| Ok(base.clone())).unwrap();
        prop_assert_eq!(out.tables["x.2da"].rows(), rows0 + 1);

        // ChangeRow never changes the row count
        if rows0 > 0 {
            let ini = Ini::parse("[2DAList]\nT0=x.2da\n[x.2da]\nChangeRow0=c\n[c]\nRowIndex=0\nlabel=Z\n").unwrap();
            let out = apply_2da_list(&ini, |_| Ok(base.clone())).unwrap();
            prop_assert_eq!(out.tables["x.2da"].rows(), rows0);
        }
    }

    #[test]
    fn exclusive_column_never_duplicates(t in table_strategy(), v in token()) {
        prop_assume!(t.column_index("label").is_none());
        let mut base = t;
        base.columns.push("label".to_owned());
        for row in &mut base.cells {
            row.push(String::new());
        }
        let label = format!("EXCL_{v}q");
        // add the row twice with the same ExclusiveColumn value; the second must
        // modify, not duplicate
        let sec = format!("[a]\nExclusiveColumn=label\nlabel={label}\n");
        let ini = Ini::parse(&format!(
            "[2DAList]\nT0=x.2da\n[x.2da]\nAddRow0=a\nAddRow1=a2\n{sec}[a2]\nExclusiveColumn=label\nlabel={label}\n"
        )).unwrap();
        let out = apply_2da_list(&ini, |_| Ok(base.clone())).unwrap();
        let table = &out.tables["x.2da"];
        let col = table.column_index("label").unwrap();
        let count = (0..table.rows()).filter(|&r| table.get(r, col) == Some(label.as_str())).count();
        prop_assert_eq!(count, 1, "ExclusiveColumn value must be unique after apply");
    }
}
