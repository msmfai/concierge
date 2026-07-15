//! Form-list (FLST) parsing and append-union merge.
//!
//! An FLST is an ordered list of `LNAM` subrecords, each a 4-byte `FormID`, after
//! an `EDID`. Unlike leveled lists there are no levels/counts and no
//! Delev/Relev semantics: the Wrye-Bash `FidListsMerger` simply takes the base
//! list and appends every override's `FormID`s that aren't already present,
//! preserving order (base first, then new entries in override load order). We
//! match that: order-preserving dedup, no removals.

use crate::reader::{Plugin, RecordRef};
use crate::{Error, Result};

pub const FLST: [u8; 4] = *b"FLST";

#[derive(Debug, Clone)]
pub struct FormList {
    pub form_id: u32,
    /// All subrecords in order; the `LNAM` run is rewritten on emit.
    pub subs: Vec<([u8; 4], Vec<u8>)>,
    /// The referenced `FormID`s, in list order.
    pub entries: Vec<u32>,
}

pub fn parse(plugin: &Plugin, rec: &RecordRef) -> Result<FormList> {
    let subs = plugin.subrecords(rec)?;
    let bad = |m: &str| Error::Other(format!("FLST {:08X}: {m}", rec.form_id));
    let mut entries = Vec::new();
    for (sig, payload) in &subs {
        if sig == b"LNAM" {
            let bytes: [u8; 4] = payload
                .get(0..4)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| bad("LNAM shorter than 4 bytes"))?;
            entries.push(u32::from_le_bytes(bytes));
        }
    }
    Ok(FormList {
        form_id: rec.form_id,
        subs,
        entries,
    })
}

/// Append-union: base entries, then every override's entries not already
/// present, preserving order. Dedup by `FormID`. No removals.
#[must_use]
pub fn merge(base: &FormList, overrides: &[&FormList]) -> FormList {
    use std::collections::HashSet;
    let mut seen: HashSet<u32> = HashSet::new();
    let mut order: Vec<u32> = Vec::new();
    for id in base
        .entries
        .iter()
        .chain(overrides.iter().flat_map(|o| o.entries.iter()))
    {
        if seen.insert(*id) {
            order.push(*id);
        }
    }
    let mut merged = base.clone();
    merged.entries = order;
    merged
}

/// Serialize with a rewritten `LNAM` run: non-`LNAM` subrecords in original
/// order, then one `LNAM` per merged `FormID`.
#[must_use]
pub fn emit_subrecords(list: &FormList) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for (sig, payload) in &list.subs {
        if sig != b"LNAM" {
            out.push((*sig, payload.clone()));
        }
    }
    for id in &list.entries {
        out.push((*b"LNAM", id.to_le_bytes().to_vec()));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn list(entries: Vec<u32>) -> FormList {
        FormList {
            form_id: 0x0100_0001,
            subs: vec![(*b"EDID", b"Test\0".to_vec())],
            entries,
        }
    }

    #[test]
    fn append_union_preserves_order_and_dedups() {
        let base = list(vec![0xAA, 0xBB]);
        let a = list(vec![0xBB, 0xCC]); // 0xBB dup, 0xCC new
        let b = list(vec![0xDD, 0xAA]); // 0xDD new, 0xAA dup
        let m = merge(&base, &[&a, &b]);
        assert_eq!(
            m.entries,
            vec![0xAA, 0xBB, 0xCC, 0xDD],
            "base first, new appended in order"
        );
    }

    #[test]
    fn emit_rebuilds_lnam_run() {
        let m = list(vec![0x11, 0x22]);
        let subs = emit_subrecords(&m);
        let lnams: Vec<_> = subs.iter().filter(|(s, _)| s == b"LNAM").collect();
        assert_eq!(lnams.len(), 2);
        assert_eq!(lnams[0].1, 0x11u32.to_le_bytes().to_vec());
    }
}
