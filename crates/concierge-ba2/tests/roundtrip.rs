//! Metamorphic round-trip: build a synthetic GNRL BA2 (the writer here is
//! test-only), parse it back, and assert `extract == original` for every file,
//! over proptest-generated inputs (random names, contents, and per-file
//! compression). The property covers the whole input space — a single hardcoded
//! example never would.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write as _;

use concierge_ba2::Archive;
use proptest::prelude::*;

/// Serialize a minimal valid GNRL BA2 from (name, content, compress) specs.
/// Layout: header(24) · records(36·n) · data blocks · name table.
fn build_gnrl(files: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let n = files.len();
    let records_len = 36 * n;
    let data_start = 24 + records_len;

    // lay out data blocks, remembering (offset, packed, unpacked) per file
    let mut blocks: Vec<u8> = Vec::new();
    let mut layout: Vec<(u64, u32, u32)> = Vec::new();
    for (_, content, compress) in files {
        let offset = u64::try_from(data_start + blocks.len()).unwrap();
        let unpacked = u32::try_from(content.len()).unwrap();
        if *compress && !content.is_empty() {
            let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(content).unwrap();
            let packed = enc.finish().unwrap();
            layout.push((offset, u32::try_from(packed.len()).unwrap(), unpacked));
            blocks.extend_from_slice(&packed);
        } else {
            layout.push((offset, 0, unpacked));
            blocks.extend_from_slice(content);
        }
    }
    let name_table_offset = u64::try_from(data_start + blocks.len()).unwrap();

    let mut out = Vec::new();
    out.extend_from_slice(b"BTDX");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(b"GNRL");
    out.extend_from_slice(&u32::try_from(n).unwrap().to_le_bytes());
    out.extend_from_slice(&name_table_offset.to_le_bytes());
    for (i, (name, _, _)) in files.iter().enumerate() {
        let ext = extension_of(name);
        let (offset, packed, unpacked) = layout[i];
        out.extend_from_slice(&0u32.to_le_bytes()); // name hash
        out.extend_from_slice(&ext); // extension
        out.extend_from_slice(&0u32.to_le_bytes()); // dir hash
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&packed.to_le_bytes());
        out.extend_from_slice(&unpacked.to_le_bytes());
        out.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes()); // align
    }
    out.extend_from_slice(&blocks);
    for (name, _, _) in files {
        let bytes = name.as_bytes();
        out.extend_from_slice(&u16::try_from(bytes.len()).unwrap().to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

fn extension_of(name: &str) -> [u8; 4] {
    let ext = name.rsplit('.').next().unwrap_or("");
    let mut out = [0u8; 4];
    for (slot, b) in out.iter_mut().zip(ext.bytes()) {
        *slot = b;
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn gnrl_roundtrip_extracts_originals(
        files in proptest::collection::vec(
            (
                "[a-z]{1,8}\\\\[a-z]{1,8}\\.(dds|nif|bgsm)",
                proptest::collection::vec(any::<u8>(), 0..300),
                any::<bool>(),
            ),
            1..8,
        )
    ) {
        // names must be unique for extract-by-name to be well-defined
        let mut seen = std::collections::HashSet::new();
        let files: Vec<_> = files
            .into_iter()
            .filter(|(name, _, _)| seen.insert(name.to_lowercase()))
            .collect();
        prop_assume!(!files.is_empty());

        let bytes = build_gnrl(&files);
        let archive = Archive::parse(&bytes).expect("synthetic BA2 must parse");
        prop_assert_eq!(archive.len(), files.len());

        for (name, content, _) in &files {
            let got = archive.extract(name).expect("named entry extracts");
            prop_assert_eq!(&got, content, "round-trip must return original bytes");
        }
    }
}

#[test]
fn rejects_bad_magic_without_panicking() {
    // total function: garbage is an Err value, never a panic
    assert!(Archive::parse(b"not a ba2 at all").is_err());
    assert!(Archive::parse(&[]).is_err());
    assert!(Archive::parse(b"BTDX\x01\x00\x00\x00GNRL").is_err()); // truncated
}
