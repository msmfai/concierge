//! Metamorphic / property-based coverage for the leveled-list and form-list
//! merges — the reconciliation spine. These encode the relations documented in
//! meta `CLAUDE.md` and let proptest hunt violations across the input space,
//! rather than checking one hand-picked example.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;

use concierge_esp::formlist::{self, FormList};
use concierge_esp::lvli::{self, Entry, LeveledList};
use proptest::prelude::*;

// --- constructors (public fields; no plugin needed) ---

fn mk_entry(level: u16, reference: u32, count: u16) -> Entry {
    let mut raw = vec![0u8; 12];
    raw[0..2].copy_from_slice(&level.to_le_bytes());
    raw[4..8].copy_from_slice(&reference.to_le_bytes());
    raw[8..10].copy_from_slice(&count.to_le_bytes());
    Entry {
        level,
        reference,
        count,
        chance_none: 0,
        raw_lvlo: raw,
        coed: None,
    }
}

fn mk_leveled(entries: Vec<Entry>) -> LeveledList {
    LeveledList {
        form_id: 0x0100_0001,
        subs: vec![(*b"LLCT", vec![0])],
        entries,
    }
}

fn refset_lvli(l: &LeveledList) -> BTreeSet<u32> {
    l.entries.iter().map(|e| e.reference).collect()
}

/// Decode `emit_subrecords` back to (level, reference, count) — the round-trip
/// oracle for the leveled writer.
fn decode_lvlo(subs: &[([u8; 4], Vec<u8>)]) -> Vec<(u16, u32, u16)> {
    subs.iter()
        .filter(|(s, _)| s == b"LVLO")
        .map(|(_, p)| {
            let l = u16::from_le_bytes([p[0], p[1]]);
            let r = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
            let c = u16::from_le_bytes([p[8], p[9]]);
            (l, r, c)
        })
        .collect()
}

// generate entries whose references collide across lists (small pool)
fn entries_strategy() -> impl Strategy<Value = Vec<Entry>> {
    proptest::collection::vec(
        (0u16..40, 1u32..12, 0u16..5).prop_map(|(lv, r, c)| mk_entry(lv, r, c)),
        0..10,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn lvli_union_is_superset_of_every_input(
        base in entries_strategy(),
        a in entries_strategy(),
        b in entries_strategy(),
    ) {
        let (base, a, b) = (mk_leveled(base), mk_leveled(a), mk_leveled(b));
        let m = lvli::union_merge(&base, &[&a, &b]);
        let refs = refset_lvli(&m);
        // union ⊇ each input's reference set (no non-Delev entry is dropped)
        for l in [&base, &a, &b] {
            prop_assert!(refset_lvli(l).is_subset(&refs));
        }
        // and it introduces nothing that wasn't in some input
        let all: BTreeSet<u32> = refset_lvli(&base)
            .union(&refset_lvli(&a)).copied().collect::<BTreeSet<_>>()
            .union(&refset_lvli(&b)).copied().collect();
        prop_assert_eq!(refs, all);
    }

    #[test]
    fn lvli_union_is_idempotent(base in entries_strategy(), a in entries_strategy()) {
        let m = lvli::union_merge(&mk_leveled(base), &[&mk_leveled(a)]);
        let again = lvli::union_merge(&m, &[&m]);
        // merging the result into itself changes nothing (dedup by listId)
        prop_assert_eq!(&again.entries, &m.entries);
    }

    #[test]
    fn lvli_union_is_commutative_and_associative(
        base in entries_strategy(), a in entries_strategy(), b in entries_strategy(),
    ) {
        let (base, a, b) = (mk_leveled(base), mk_leveled(a), mk_leveled(b));
        // commutative up to the tie-break: the reference SET is order-independent
        let ab = refset_lvli(&lvli::union_merge(&base, &[&a, &b]));
        let ba = refset_lvli(&lvli::union_merge(&base, &[&b, &a]));
        prop_assert_eq!(&ab, &ba);
        // associative (by reference set)
        let left = lvli::union_merge(&base, &[&a]);
        let assoc = refset_lvli(&lvli::union_merge(&left, &[&b]));
        prop_assert_eq!(&ab, &assoc);
    }

    #[test]
    fn lvli_delev_removal_persists_against_later_readd(
        base in entries_strategy(), delev in entries_strategy(), later in entries_strategy(),
    ) {
        use concierge_esp::lvli::Tags;
        let (base, delev, later) = (mk_leveled(base), mk_leveled(delev), mk_leveled(later));
        // refs the Delev override drops (present in base, absent from delev)
        let dropped: BTreeSet<u32> =
            refset_lvli(&base).difference(&refset_lvli(&delev)).copied().collect();
        let m = lvli::merge(&base, &[
            (&delev, Tags { delev: true, relev: false }),
            (&later, Tags::default()),
        ]);
        // a Delev-removed target stays removed even if a later override re-adds it
        for r in &dropped {
            prop_assert!(!refset_lvli(&m).contains(r), "delev-dropped {r} reappeared");
        }
    }

    #[test]
    fn lvli_entries_are_sorted_and_roundtrip(base in entries_strategy(), a in entries_strategy()) {
        let m = lvli::union_merge(&mk_leveled(base), &[&mk_leveled(a)]);
        // Creation-Kit invariant: entries sorted ascending by (level, reference)
        let mut sorted = m.entries.clone();
        sorted.sort_by_key(|e| (e.level, e.reference));
        prop_assert_eq!(
            m.entries.iter().map(|e| (e.level, e.reference)).collect::<Vec<_>>(),
            sorted.iter().map(|e| (e.level, e.reference)).collect::<Vec<_>>()
        );
        // emit -> decode round-trips the entry run exactly
        let decoded = decode_lvlo(&lvli::emit_subrecords(&m));
        let expected: Vec<_> = m.entries.iter().map(|e| (e.level, e.reference, e.count)).collect();
        prop_assert_eq!(decoded, expected);
    }
}

// --- form-lists (append-union) ---

fn mk_form(entries: Vec<u32>) -> FormList {
    FormList {
        form_id: 0x0100_0002,
        subs: vec![(*b"EDID", b"T\0".to_vec())],
        entries,
    }
}

fn formvec() -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::vec(1u32..15, 0..10)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn flst_append_union_superset_and_dedup(base in formvec(), a in formvec(), b in formvec()) {
        let (base, a, b) = (mk_form(base), mk_form(a), mk_form(b));
        let m = formlist::merge(&base, &[&a, &b]);
        let set: BTreeSet<u32> = m.entries.iter().copied().collect();
        // no duplicates
        prop_assert_eq!(set.len(), m.entries.len());
        // superset of every input; nothing invented
        let all: BTreeSet<u32> = base.entries.iter().chain(&a.entries).chain(&b.entries).copied().collect();
        prop_assert_eq!(set, all);
        // base entries keep their relative order at the front
        let base_dedup: Vec<u32> = {
            let mut seen = BTreeSet::new();
            base.entries.iter().copied().filter(|x| seen.insert(*x)).collect()
        };
        prop_assert_eq!(&m.entries[..base_dedup.len()], &base_dedup[..]);
    }

    #[test]
    fn flst_is_idempotent_and_permutation_invariant(base in formvec(), a in formvec(), b in formvec()) {
        let (base, a, b) = (mk_form(base), mk_form(a), mk_form(b));
        let m = formlist::merge(&base, &[&a, &b]);
        // idempotent
        let again = formlist::merge(&m, &[&m]);
        prop_assert_eq!(&again.entries, &m.entries);
        // permutation-invariant as a set
        let ab: BTreeSet<u32> = m.entries.iter().copied().collect();
        let ba: BTreeSet<u32> = formlist::merge(&base, &[&b, &a]).entries.into_iter().collect();
        prop_assert_eq!(ab, ba);
    }

    #[test]
    fn flst_emit_roundtrips(base in formvec(), a in formvec()) {
        let m = formlist::merge(&mk_form(base), &[&mk_form(a)]);
        let lnam: Vec<u32> = formlist::emit_subrecords(&m)
            .iter()
            .filter(|(s, _)| s == b"LNAM")
            .map(|(_, p)| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        prop_assert_eq!(lnam, m.entries);
    }
}
