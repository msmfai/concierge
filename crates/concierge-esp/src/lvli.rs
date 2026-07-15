//! Leveled-item (LVLI) parsing and Wrye-Bash-style union merge.
//!
//! FO4 LVLI subrecords used here: EDID (editor id), LVLD (u8 chance-none),
//! LVLF (u8 flags), LLCT (u8 entry count), LVLO (12-byte entry: u16 level,
//! 2 unused, formid reference, u16 count, u8 chance-none, 1 unused), plus
//! COED (optional per-entry extra data). We preserve every subrecord we don't
//! model and re-emit them; the merge only rewrites the LVLO/LLCT run.
//!
//! Merge semantics (Wrye bashed patch): the union of all plugins' additions;
//! entries are identified by their target formid (listId) alone (Wrye
//! semantics) so same-target duplicates collapse; the winner supplies list-level fields
//! (LVLD/LVLF). `merge` honors Delev (remove+blacklist dropped targets)
//! and Relev (forward the tagged version of an existing target) tags;
//! `union_merge` is the untagged additive case.

use crate::reader::{Plugin, RecordRef};
use crate::{Error, Result};

pub const LVLI: [u8; 4] = *b"LVLI";
/// Leveled actor/NPC list (FO4 + Skyrim) — identical `LVLO`-entry structure.
pub const LVLN: [u8; 4] = *b"LVLN";
/// Leveled spell list (Skyrim) — identical `LVLO`-entry structure.
pub const LVSP: [u8; 4] = *b"LVSP";

/// The leveled-list record signatures whose bodies are `LVLO` entry runs, so
/// they share this module's parse/merge/emit. (`LVLI` items, `LVLN` actors,
/// `LVSP` spells.)
pub const LEVELED_SIGNATURES: [[u8; 4]; 3] = [LVLI, LVLN, LVSP];

/// Is this a leveled-list signature handled by this module?
#[must_use]
pub fn is_leveled(sig: [u8; 4]) -> bool {
    LEVELED_SIGNATURES.contains(&sig)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    pub level: u16,
    pub reference: u32,
    pub count: u16,
    pub chance_none: u8,
    /// The exact 12-byte LVLO payload (preserves the 3 unused bytes) + any
    /// trailing COED so re-emission is byte-faithful.
    pub raw_lvlo: Vec<u8>,
    pub coed: Option<Vec<u8>>,
}

impl Entry {
    /// Identity for union dedup. Wrye Bash's `ListsMerger` keys on the entry's
    /// target `FormID` (`listId`) ALONE — a same-target entry at a different
    /// level/count is a duplicate, not a new entry (verified against
    /// `wrye-bash` `common_records.py` `merge_list`). We match that exactly.
    const fn key(&self) -> u32 {
        self.reference
    }
}

#[derive(Debug, Clone)]
pub struct LeveledList {
    pub form_id: u32,
    /// All subrecords in order; the LVLO/COED/LLCT run is rewritten on emit.
    pub subs: Vec<([u8; 4], Vec<u8>)>,
    pub entries: Vec<Entry>,
}

fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
}

fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
}

pub fn parse(plugin: &Plugin, rec: &RecordRef) -> Result<LeveledList> {
    let subs = plugin.subrecords(rec)?;
    let bad = |m: &str| Error::Other(format!("LVLI {:08X}: {m}", rec.form_id));
    let mut entries = Vec::new();
    let mut i = 0;
    while i < subs.len() {
        let (sig, payload) = subs.get(i).ok_or_else(|| bad("index"))?;
        if sig == b"LVLO" {
            if payload.len() < 12 {
                return Err(bad("LVLO shorter than 12 bytes"));
            }
            let level = le_u16(payload, 0).ok_or_else(|| bad("LVLO level"))?;
            let reference = le_u32(payload, 4).ok_or_else(|| bad("LVLO ref"))?;
            let count = le_u16(payload, 8).ok_or_else(|| bad("LVLO count"))?;
            let chance_none = *payload.get(10).ok_or_else(|| bad("LVLO chance"))?;
            // an immediately-following COED belongs to this entry
            let coed = subs.get(i + 1).and_then(|(s, p)| {
                (s == b"COED").then(|| {
                    i += 1;
                    p.clone()
                })
            });
            entries.push(Entry {
                level,
                reference,
                count,
                chance_none,
                raw_lvlo: payload.clone(),
                coed,
            });
        }
        i += 1;
    }
    Ok(LeveledList {
        form_id: rec.form_id,
        subs,
        entries,
    })
}

/// Per-override bash tags that change how its entries merge (from the LOOT
/// masterlist / plugin description).
#[derive(Debug, Clone, Copy, Default)]
pub struct Tags {
    /// The override's version of an already-present target is forwarded
    /// (its level/count/chance replace the current entry).
    pub relev: bool,
    /// A base listId ABSENT from this override is removed and stays removed.
    pub delev: bool,
}

/// Wrye-style additive union of untagged overrides (dedup by listId).
pub fn union_merge(base: &LeveledList, overrides: &[&LeveledList]) -> LeveledList {
    let tagged: Vec<(&LeveledList, Tags)> =
        overrides.iter().map(|o| (*o, Tags::default())).collect();
    merge(base, &tagged)
}

/// Full Wrye-semantics merge with per-override Delev/Relev tags: additions
/// union by listId; Relev forwards the tagged version of an existing target;
/// Delev removes-and-blacklists base targets the override drops.
pub fn merge(base: &LeveledList, overrides: &[(&LeveledList, Tags)]) -> LeveledList {
    use std::collections::{HashMap, HashSet};
    let mut by_id: HashMap<u32, Entry> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    for e in &base.entries {
        if by_id.insert(e.key(), e.clone()).is_none() {
            order.push(e.key());
        }
    }
    let mut blacklist: HashSet<u32> = HashSet::new();

    for (ov, tags) in overrides {
        let ov_ids: HashSet<u32> = ov.entries.iter().map(Entry::key).collect();
        if tags.delev {
            let dropped: Vec<u32> = order
                .iter()
                .copied()
                .filter(|id| !ov_ids.contains(id))
                .collect();
            for id in dropped {
                by_id.remove(&id);
                order.retain(|x| *x != id);
                blacklist.insert(id);
            }
        }
        for e in &ov.entries {
            let id = e.key();
            if blacklist.contains(&id) {
                continue;
            }
            if let Some(existing) = by_id.get_mut(&id) {
                if tags.relev {
                    *existing = e.clone();
                }
            } else {
                by_id.insert(id, e.clone());
                order.push(id);
            }
        }
    }

    let mut merged = base.clone();
    merged.entries = order
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect();
    // CK keeps entries sorted ascending by level, then reference.
    merged.entries.sort_by_key(|e| (e.level, e.reference));
    merged
}

/// Serialize the merged list's subrecords with a rewritten LVLO/LLCT run:
/// non-entry subrecords in their original order, LLCT set to the merged count,
/// then all LVLO(+COED) entries in sorted order. Returns (sig, payload) pairs
/// the writer turns into a record.
pub fn emit_subrecords(list: &LeveledList) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let count = u8::try_from(list.entries.len()).unwrap_or(u8::MAX);
    for (sig, payload) in &list.subs {
        match sig {
            b"LVLO" | b"COED" => {} // dropped; re-emitted from `entries`
            b"LLCT" => out.push((*b"LLCT", vec![count])),
            _ => out.push((*sig, payload.clone())),
        }
    }
    // ensure an LLCT exists even if the base had none
    if !out.iter().any(|(s, _)| s == b"LLCT") {
        out.push((*b"LLCT", vec![count]));
    }
    for e in &list.entries {
        out.push((*b"LVLO", e.raw_lvlo.clone()));
        if let Some(coed) = &e.coed {
            out.push((*b"COED", coed.clone()));
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::unwrap_used
)]
mod tests {
    use super::*;

    fn entry(level: u16, reference: u32, count: u16) -> Entry {
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

    fn list(entries: Vec<Entry>) -> LeveledList {
        LeveledList {
            form_id: 0x0100_0001,
            subs: vec![(*b"LLCT", vec![u8::try_from(entries.len()).unwrap_or(0)])],
            entries,
        }
    }

    #[test]
    fn leveled_signatures_share_the_machinery() {
        // LVLI/LVLN/LVSP all use LVLO entries, so parse/merge/emit are
        // signature-agnostic — the record signature is just metadata the
        // resolver stamps on emit. This is why LVLN (leveled NPCs) and LVSP
        // (leveled spells) reuse this module unchanged.
        assert!(is_leveled(LVLI) && is_leveled(LVLN) && is_leveled(LVSP));
        assert!(!is_leveled(*b"FLST"));
        // the merge itself never inspects the signature: same union result
        let base = list(vec![entry(1, 0xAA, 1)]);
        let ov = list(vec![entry(2, 0xBB, 1)]);
        let m = union_merge(&base, &[&ov]);
        assert_eq!(
            m.entries.len(),
            2,
            "union works identically for any leveled sig"
        );
    }

    #[test]
    fn union_dedups_by_listid_only() {
        let base = list(vec![entry(1, 0xAA, 1), entry(1, 0xBB, 1)]);
        // override adds a NEW target (0xCC) and a dup of 0xAA at a new level
        let ov = list(vec![entry(5, 0xAA, 9), entry(1, 0xCC, 1)]);
        let m = union_merge(&base, &[&ov]);
        assert_eq!(m.entries.len(), 3, "0xAA dup ignored, 0xCC added");
        // untagged: existing 0xAA keeps the BASE version (level 1, count 1)
        let aa = m.entries.iter().find(|e| e.reference == 0xAA).unwrap();
        assert_eq!((aa.level, aa.count), (1, 1));
    }

    #[test]
    fn relev_forwards_existing_target() {
        let base = list(vec![entry(1, 0xAA, 1)]);
        let ov = list(vec![entry(5, 0xAA, 9)]);
        let m = merge(
            &base,
            &[(
                &ov,
                Tags {
                    relev: true,
                    delev: false,
                },
            )],
        );
        assert_eq!(m.entries.len(), 1);
        let aa = &m.entries[0];
        assert_eq!((aa.level, aa.count), (5, 9), "Relev forwards the tweak");
    }

    #[test]
    fn delev_removes_and_blacklists() {
        let base = list(vec![entry(1, 0xAA, 1), entry(1, 0xBB, 1)]);
        let delev = list(vec![entry(1, 0xAA, 1)]); // drops 0xBB
        let later = list(vec![entry(1, 0xBB, 1)]); // tries to re-add it
        let m = merge(
            &base,
            &[
                (
                    &delev,
                    Tags {
                        delev: true,
                        relev: false,
                    },
                ),
                (&later, Tags::default()),
            ],
        );
        assert!(
            m.entries.iter().all(|e| e.reference != 0xBB),
            "Delev removal stays removed despite later re-add"
        );
        assert_eq!(m.entries.len(), 1);
    }
}
