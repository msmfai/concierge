//! KOTOR talk-table (TLK) — enough to append strings and relocate `StrRef`s.
//!
//! Layout: 20-byte header (`"TLK V3.0"`, `u32` language id, `u32` string count,
//! `u32` string-entries offset), then `count` 40-byte string-data elements
//! (flags, 16-byte sound resref, volume/pitch variance, `u32` offset-to-string
//! relative to the entries offset, `u32` string size, `f32` sound length),
//! then the raw string bytes. `TSLPatcher` appends a mod's `append.tlk` to the
//! game `dialog.tlk`; a `StrRef<n>` token resolves to `base_count + n`.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tlk: {0}")]
    Format(String),
}
pub type Result<T> = std::result::Result<T, Error>;

pub const SIGNATURE: &[u8] = b"TLK V3.0";
const HEADER: usize = 20;
const ENTRY: usize = 40;
const OFFSET_FIELD: usize = 28; // offset-to-string within a 40-byte entry

fn u32_at(b: &[u8], p: usize) -> Result<u32> {
    b.get(p..p + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| Error::Format(format!("truncated at {p}")))
}

/// The number of strings already in a TLK (its `StrRef` base).
pub fn string_count(bytes: &[u8]) -> Result<u32> {
    if bytes.get(..SIGNATURE.len()) != Some(SIGNATURE) {
        return Err(Error::Format("missing TLK V3.0 signature".into()));
    }
    u32_at(bytes, 12)
}

/// Append `add`'s strings to `base`, returning a valid merged TLK. Base strings
/// keep their `StrRef`s; each appended string lands at `base_count + i`.
pub fn append(base: &[u8], add: &[u8]) -> Result<Vec<u8>> {
    let bad = |m: &str| Error::Format(m.to_owned());
    if base.get(..8) != Some(SIGNATURE) || add.get(..8) != Some(SIGNATURE) {
        return Err(bad("missing TLK signature"));
    }
    let base_count = usize::try_from(string_count(base)?).unwrap_or(0);
    let add_count = usize::try_from(string_count(add)?).unwrap_or(0);
    let base_entries_off = usize::try_from(u32_at(base, 16)?).unwrap_or(0);
    let add_entries_off = usize::try_from(u32_at(add, 16)?).unwrap_or(0);
    let base_strings = base
        .get(base_entries_off..)
        .ok_or_else(|| bad("base string section out of range"))?;
    let add_strings = add
        .get(add_entries_off..)
        .ok_or_else(|| bad("add string section out of range"))?;

    let merged_count = base_count + add_count;
    let new_entries_off = HEADER + merged_count * ENTRY;

    let mut out = Vec::with_capacity(new_entries_off + base_strings.len() + add_strings.len());
    // header: reuse base language id, new count + entries offset
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&u32_at(base, 8)?.to_le_bytes());
    out.extend_from_slice(&u32::try_from(merged_count).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&u32::try_from(new_entries_off).unwrap_or(0).to_le_bytes());

    // base entries verbatim (their string offsets are relative to the string
    // section, and base strings still come first, so they're unchanged)
    for i in 0..base_count {
        let start = HEADER + i * ENTRY;
        out.extend_from_slice(
            base.get(start..start + ENTRY)
                .ok_or_else(|| bad("base entry"))?,
        );
    }
    // appended entries: bump offset-to-string past the base string block
    let shift = u32::try_from(base_strings.len()).unwrap_or(0);
    for i in 0..add_count {
        let start = HEADER + i * ENTRY;
        let mut entry = add
            .get(start..start + ENTRY)
            .ok_or_else(|| bad("add entry"))?
            .to_vec();
        let off = u32::from_le_bytes(
            entry
                .get(OFFSET_FIELD..OFFSET_FIELD + 4)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| bad("add entry offset"))?,
        );
        if let Some(slot) = entry.get_mut(OFFSET_FIELD..OFFSET_FIELD + 4) {
            slot.copy_from_slice(&(off + shift).to_le_bytes());
        }
        out.extend_from_slice(&entry);
    }
    out.extend_from_slice(base_strings);
    out.extend_from_slice(add_strings);
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Build a tiny TLK with the given strings for round-trip testing.
    fn make(strings: &[&str]) -> Vec<u8> {
        let count = strings.len();
        let entries_off = HEADER + count * ENTRY;
        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE);
        out.extend_from_slice(&0u32.to_le_bytes()); // language
        out.extend_from_slice(&u32::try_from(count).unwrap().to_le_bytes());
        out.extend_from_slice(&u32::try_from(entries_off).unwrap().to_le_bytes());
        let mut running = 0u32;
        let mut blob = Vec::new();
        for s in strings {
            let mut e = vec![0u8; ENTRY];
            e[OFFSET_FIELD..OFFSET_FIELD + 4].copy_from_slice(&running.to_le_bytes());
            e[32..36].copy_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
            out.extend_from_slice(&e);
            blob.extend_from_slice(s.as_bytes());
            running += u32::try_from(s.len()).unwrap();
        }
        out.extend_from_slice(&blob);
        out
    }

    #[test]
    fn append_bumps_count_and_offsets() {
        let base = make(&["hello", "world"]);
        let add = make(&["new1", "new2"]);
        assert_eq!(string_count(&base).unwrap(), 2);
        let merged = append(&base, &add).unwrap();
        assert_eq!(string_count(&merged).unwrap(), 4);
        // appended entry 0 (index 2) offset must be past the base strings (10)
        let off = u32::from_le_bytes(
            merged[HEADER + 2 * ENTRY + OFFSET_FIELD..HEADER + 2 * ENTRY + OFFSET_FIELD + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(off, 10, "new string offset shifted past base strings");
    }

    /// Decode string `i` from a TLK (via its entry's offset+size).
    fn decode(bytes: &[u8], i: usize) -> String {
        let entries_off =
            usize::try_from(u32::from_le_bytes(bytes[16..20].try_into().unwrap())).unwrap_or(0);
        let e = HEADER + i * ENTRY;
        let off = usize::try_from(u32::from_le_bytes(
            bytes[e + OFFSET_FIELD..e + OFFSET_FIELD + 4]
                .try_into()
                .unwrap(),
        ))
        .unwrap_or(0);
        let size = usize::try_from(u32::from_le_bytes(
            bytes[e + 32..e + 36].try_into().unwrap(),
        ))
        .unwrap_or(0);
        let start = entries_off + off;
        String::from_utf8_lossy(&bytes[start..start + size]).into_owned()
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(300))]

        #[test]
        fn append_preserves_base_and_resolves_every_string(
            base in proptest::collection::vec("[a-z ]{0,12}", 0..8),
            add in proptest::collection::vec("[a-z ]{0,12}", 0..8),
        ) {
            use proptest::prelude::*;
            let b = make(&base.iter().map(String::as_str).collect::<Vec<_>>());
            let a = make(&add.iter().map(String::as_str).collect::<Vec<_>>());
            let merged = append(&b, &a).unwrap();
            // count adds
            prop_assert_eq!(usize::try_from(string_count(&merged).unwrap()).unwrap_or(0), base.len() + add.len());
            // every base string is unchanged at its original StrRef
            for (i, s) in base.iter().enumerate() {
                prop_assert_eq!(&decode(&merged, i), s);
            }
            // every appended string resolves at base_count + j to its content
            for (j, s) in add.iter().enumerate() {
                prop_assert_eq!(&decode(&merged, base.len() + j), s);
            }
        }
    }
}
