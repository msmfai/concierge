//! Fixture tests against the REAL Fallout 4 install (skipped politely when it
//! isn't present). Run the heavy full-install sweep explicitly:
//! `cargo test -p concierge-esp --release -- --ignored`

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use concierge_esp::reader::Plugin;
use concierge_esp::writer::resolver_bytes;
use concierge_esp::FLAG_COMPRESSED;

fn data_dir() -> Option<PathBuf> {
    // Point CONCIERGE_FO4_DATA at a real Fallout 4 Data dir to run this test;
    // it skips cleanly otherwise.
    let p = PathBuf::from(std::env::var_os("CONCIERGE_FO4_DATA")?);
    p.is_dir().then_some(p)
}

#[test]
fn dlccoast_ground_truths() {
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let p = Plugin::read(&data.join("DLCCoast.esm")).unwrap();
    assert_eq!(p.meta.form_version, 131);
    assert!((p.meta.header_version - 0.95).abs() < 1e-6);
    assert_eq!(p.meta.masters, vec!["Fallout4.esm".to_owned()]);
    assert_eq!(
        p.meta.onam_count, 492,
        "DLCCoast declares 492 ONAM overrides"
    );
    assert!(p.meta.is_esm && !p.meta.is_esl);
    assert!(
        p.records.len() >= usize::try_from(p.meta.num_records / 2).unwrap_or(0),
        "walk should find the bulk of the declared records"
    );
    // decode one compressed record end-to-end
    let compressed = p
        .records
        .iter()
        .find(|r| r.flags & FLAG_COMPRESSED != 0)
        .copied()
        .expect("DLC has compressed records");
    let subs = p.subrecords(&compressed).unwrap();
    assert!(!subs.is_empty());
}

#[test]
fn cc_esl_ground_truths() {
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let p = Plugin::read(&data.join("ccBGSFO4115-X02.esl")).unwrap();
    assert!(p.meta.is_esl && p.meta.is_esm && p.meta.is_localized);
    assert!((p.meta.header_version - 1.0).abs() < 1e-6);
    assert_eq!(p.meta.onam_count, 21);
    // own records fit the light objectID budget
    for r in &p.records {
        if usize::from(r.master_index()) >= p.meta.masters.len() {
            assert!(r.object_id() <= 0xFFF, "ESL own record beyond FE budget");
        }
    }
}

#[test]
#[ignore = "heavy: parses every plugin incl. the 330MB base ESM"]
fn every_plugin_in_the_install_parses() {
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let mut count = 0usize;
    for entry in std::fs::read_dir(&data).unwrap() {
        let path = entry.unwrap().path();
        let is_plugin = path.extension().is_some_and(|e| {
            ["esm", "esp", "esl"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        });
        if !is_plugin {
            continue;
        }
        let p = Plugin::read(&path).unwrap();
        assert_eq!(p.meta.form_version, 131, "{}", path.display());
        count += 1;
        if path.file_name().is_some_and(|n| n == "Fallout4.esm") {
            assert_eq!(p.meta.num_records, 1_741_853);
            assert!((p.meta.header_version - 1.0).abs() < 1e-6);
            assert!(p.records.len() > 1_000_000);
        }
    }
    assert!(count >= 15, "expected the DLC + CC plugin population");
}

#[test]
fn resolver_round_trips_through_our_reader() {
    let masters = vec!["Fallout4.esm".to_owned(), "DLCCoast.esm".to_owned()];
    let bytes = resolver_bytes("concierge", &masters).unwrap();
    let dir = std::env::temp_dir().join("concierge-esp-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ConciergeResolver.esp");
    std::fs::write(&path, &bytes).unwrap();
    let p = Plugin::read(&path).unwrap();
    // PLAIN ESP, not ESL/ESM: a merge patch must load last (after ESPs) to win;
    // ESL/ESM would load in the master block and lose. See writer note.
    assert!(!p.meta.is_esl, "resolver must be a plain ESP, not ESL");
    assert!(!p.meta.is_esm);
    assert_eq!(p.meta.form_version, 131);
    assert!((p.meta.header_version - 1.0).abs() < 1e-6);
    assert_eq!(p.meta.author, "concierge");
    assert_eq!(p.meta.masters, masters);
    assert_eq!(p.meta.num_records, 0);
    assert!(p.records.is_empty());
    assert_eq!(p.meta.next_object_id, 0x800);
    // flags byte-exact: no flags set (plain ESP)
    assert_eq!(bytes.get(8..12).unwrap(), 0u32.to_le_bytes());
}

#[test]
fn udr_and_navmesh_match_the_loot_oracle_exactly() {
    // LOOT masterlist v0.29, FO4Edit QuickAutoClean-derived dirty counts
    // (next-gen CRC rows). UDR and deleted-navmesh detection are byte-level
    // trivial and reproduce xEdit EXACTLY; ITM is asserted separately as a
    // lower bound (see below) because xEdit's "identical" is a decoded
    // element-tree comparison, not a raw-subrecord compare.
    //   (name, udr, deleted-navmeshes)
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let base = Plugin::read(&data.join("Fallout4.esm")).unwrap();
    let oracle: &[(&str, usize, usize)] = &[
        ("DLCRobot.esm", 38, 1),
        ("DLCworkshop01.esm", 0, 0),
        ("DLCCoast.esm", 86, 0),
        ("DLCworkshop02.esm", 0, 0),
        ("DLCworkshop03.esm", 10, 0),
        ("DLCNukaWorld.esm", 418, 7),
    ];
    for (name, udr, nav) in oracle {
        let p = Plugin::read(&data.join(name)).unwrap();
        let report = concierge_esp::clean::analyze(&p, &[Some(&base)]).unwrap();
        assert_eq!(report.udr.len(), *udr, "{name} UDR (exact vs xEdit)");
        assert_eq!(report.deleted_navmeshes, *nav, "{name} deleted navmeshes");
    }
}

#[test]
fn itm_is_a_conservative_lower_bound() {
    // Byte-level ITM undercounts xEdit (which normalizes on a decoded element
    // tree). It must never OVER-report (that would risk removing a real edit),
    // and it must find *some* on the known-dirty DLCs. Exact xEdit parity is
    // the "replicate-with-effort" tier (needs a typed record decoder).
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let base = Plugin::read(&data.join("Fallout4.esm")).unwrap();
    for (name, xedit_itm) in [("DLCCoast.esm", 83usize), ("DLCNukaWorld.esm", 193)] {
        let p = Plugin::read(&data.join(name)).unwrap();
        let report = concierge_esp::clean::analyze(&p, &[Some(&base)]).unwrap();
        assert!(!report.itm.is_empty(), "{name}: should find some ITM");
        assert!(
            report.itm.len() <= xedit_itm,
            "{name}: byte-ITM {} must not exceed xEdit's {xedit_itm}",
            report.itm.len()
        );
    }
}

#[test]
fn leveled_list_union_merge_into_valid_resolver() {
    use concierge_esp::lvli::{self, LVLI};
    use concierge_esp::writer::{resolver_bytes_with, OverrideRecord};

    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let base = Plugin::read(&data.join("Fallout4.esm")).unwrap();
    // pick a base-game LVLI with entries to merge against a synthetic override
    let rec = base
        .records
        .iter()
        .filter(|r| r.signature == LVLI)
        .find_map(|r| {
            let ll = lvli::parse(&base, r).ok()?;
            (ll.entries.len() >= 2).then_some((*r, ll))
        });
    let Some((rref, base_ll)) = rec else {
        panic!("no LVLI with >=2 entries in the base game");
    };

    // synthetic override that adds one entry with a NEW target FormID (Wrye
    // dedups by reference/listId alone, so a new level on an existing target
    // would NOT count — the entry must reference a form not already listed).
    let mut ov = base_ll.clone();
    let existing: std::collections::HashSet<u32> =
        base_ll.entries.iter().map(|e| e.reference).collect();
    let mut new_entry = ov.entries[0].clone();
    let mut new_ref = new_entry.reference.wrapping_add(1);
    while existing.contains(&new_ref) {
        new_ref = new_ref.wrapping_add(1);
    }
    new_entry.reference = new_ref;
    new_entry.raw_lvlo[4..8].copy_from_slice(&new_ref.to_le_bytes());
    ov.entries.push(new_entry);

    // a same-reference-different-level entry must be treated as a duplicate
    let mut dup = base_ll.clone();
    let mut same_ref = dup.entries[0].clone();
    same_ref.level = same_ref.level.wrapping_add(9);
    same_ref.raw_lvlo[0..2].copy_from_slice(&same_ref.level.to_le_bytes());
    dup.entries.push(same_ref);
    assert_eq!(
        lvli::union_merge(&base_ll, &[&dup]).entries.len(),
        base_ll.entries.len(),
        "same-listId entry is a Wrye duplicate — not added"
    );

    let merged = lvli::union_merge(&base_ll, &[&ov]);
    assert_eq!(
        merged.entries.len(),
        base_ll.entries.len() + 1,
        "union adds exactly the one new-target entry"
    );
    // sorted ascending by (level, reference)
    for w in merged.entries.windows(2) {
        assert!((w[0].level, w[0].reference) <= (w[1].level, w[1].reference));
    }

    let subs = lvli::emit_subrecords(&merged);
    let llct = subs
        .iter()
        .find(|(s, _)| s == b"LLCT")
        .expect("LLCT present");
    assert_eq!(usize::from(llct.1[0]), merged.entries.len(), "LLCT recount");

    let over = OverrideRecord {
        signature: LVLI,
        form_id: rref.form_id, // keeps the Fallout4.esm master index (0x00)
        flags: 0,
        subrecords: subs,
    };
    let bytes = resolver_bytes_with(
        "concierge",
        &["Fallout4.esm".to_owned()],
        std::slice::from_ref(&over),
    )
    .unwrap();

    // round-trip through our own reader (structural xEdit-equivalent validation)
    let dir = std::env::temp_dir().join("concierge-esp-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ConciergeResolver-lvli.esp");
    std::fs::write(&path, &bytes).unwrap();
    let back = Plugin::read(&path).unwrap();
    assert!(!back.meta.is_esl, "plain ESP, loads last to win");
    assert_eq!(back.meta.num_records, 2, "1 GRUP + 1 record");
    let recs: Vec<_> = back
        .records
        .iter()
        .filter(|r| r.signature == LVLI)
        .collect();
    assert_eq!(recs.len(), 1, "one merged LVLI override");
    assert_eq!(recs[0].form_id, rref.form_id);
    // the merged entries survive the round trip
    let reparsed = lvli::parse(&back, recs[0]).unwrap();
    assert_eq!(reparsed.entries.len(), merged.entries.len());
}

#[test]
fn udr_fixes_emit_valid_undelete_disable_records() {
    use concierge_esp::clean::{udr_fixes, FLAG_INITIALLY_DISABLED, PLAYER_REF};
    use concierge_esp::writer::resolver_bytes_with;

    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let base = Plugin::read(&data.join("Fallout4.esm")).unwrap();
    // DLCCoast has 86 UDRs, all overriding Fallout4.esm refs
    let coast = Plugin::read(&data.join("DLCCoast.esm")).unwrap();
    let fixes = udr_fixes(&coast, &[Some(&base)]).unwrap();
    assert!(!fixes.is_empty(), "should build UDR fixes");
    assert!(fixes.len() <= 86, "no more than the detected UDRs");

    for f in &fixes {
        assert_eq!(
            f.flags, FLAG_INITIALLY_DISABLED,
            "fix is initially disabled"
        );
        assert_ne!(&f.signature, b"NAVM", "navmesh never fixed");
        // an XESP pointing at the player with opposite-of-parent flag
        let xesp = f
            .subrecords
            .iter()
            .find(|(s, _)| s == b"XESP")
            .expect("fix has an enable parent");
        assert_eq!(&xesp.1[0..4], &PLAYER_REF.to_le_bytes());
        assert_eq!(xesp.1[4], 0x01, "opposite of parent");
        // no leftover teleport/old enable-parent
        assert!(!f.subrecords.iter().any(|(s, _)| s == b"XTEL"));
    }

    // emit the fixes into a valid resolver and round-trip
    let bytes = resolver_bytes_with("concierge", &["Fallout4.esm".to_owned()], &fixes).unwrap();
    let dir = std::env::temp_dir().join("concierge-esp-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ConciergeResolver-udr.esp");
    std::fs::write(&path, &bytes).unwrap();
    let back = Plugin::read(&path).unwrap();
    assert!(!back.meta.is_esl, "plain ESP, loads last to win");
    let disabled = back
        .records
        .iter()
        .filter(|r| r.flags & FLAG_INITIALLY_DISABLED != 0)
        .count();
    assert_eq!(disabled, fixes.len(), "all fixes round-trip disabled");
}

#[test]
fn cleanable_itm_is_safe_and_danger_free() {
    // Byte-ITM undercounts xEdit's raw number, but the residual lives in
    // danger-class records (CELL/LAND/WRLD/NAVM) we never clean. The set we'd
    // actually remove (cleanable_itm) contains NO danger signatures and stays
    // a strict conservative subset.
    let Some(data) = data_dir() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let base = Plugin::read(&data.join("Fallout4.esm")).unwrap();
    for name in ["DLCCoast.esm", "DLCNukaWorld.esm"] {
        let p = Plugin::read(&data.join(name)).unwrap();
        let report = concierge_esp::clean::analyze(&p, &[Some(&base)]).unwrap();
        let cleanable = report.cleanable_itm();
        for (sig, _) in &cleanable {
            assert!(
                !concierge_esp::DANGER_SIGNATURES.contains(&sig.as_str()),
                "{name}: cleanable ITM must exclude danger sig {sig}"
            );
        }
        // cleanable is a subset of all detected ITM
        assert!(cleanable.len() <= report.itm.len());
    }
}
