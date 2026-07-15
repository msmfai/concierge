//! Differential proof on real Fallout 4 BA2s: the extracted bytes must carry
//! the correct file-format signature for their extension (a `.dds` starts with
//! `DDS `, a `.nif` with `Gamebryo`), the strongest oracle available short of a
//! second extractor. Skips cleanly when the install isn't present.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use concierge_ba2::{Archive, Kind};

fn fo4_data() -> Option<PathBuf> {
    // Point CONCIERGE_FO4_DATA at a real Fallout 4 Data dir to run this test;
    // it skips cleanly otherwise.
    let p = PathBuf::from(std::env::var_os("CONCIERGE_FO4_DATA")?);
    p.is_dir().then_some(p)
}

fn build_dir() -> Option<PathBuf> {
    // MK18 mod build (GNRL + DX10 archives)
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../builds/f9e709518f1b43f0bc8860aefc9ef1f6/Data");
    p.is_dir().then_some(p)
}

fn signature_ok(ext: &str, bytes: &[u8]) -> bool {
    match ext.to_ascii_lowercase().as_str() {
        "dds" => bytes.starts_with(b"DDS "),
        "nif" => bytes.starts_with(b"Gamebryo"),
        // materials, sounds, etc. — at least non-empty and the right length is
        // checked separately; don't over-constrain unknown types here
        _ => !bytes.is_empty(),
    }
}

#[test]
fn gnrl_extracts_real_files_with_correct_signatures() {
    let Some(data) = fo4_data() else {
        eprintln!("no FO4 install; skipping");
        return;
    };
    let bytes = std::fs::read(data.join("DLCCoast - Main.ba2")).unwrap();
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(archive.kind(), Kind::General);
    assert!(archive.len() > 30_000, "DLCCoast Main has many files");

    // extract one file of each recognizable type and check its magic
    let mut checked_dds = false;
    let mut checked_nif = false;
    for entry in archive.entries() {
        let ext = entry.extension_str();
        if (ext == "dds" && !checked_dds) || (ext == "nif" && !checked_nif) {
            let out = archive.extract(&entry.name).unwrap();
            assert!(
                signature_ok(&ext, &out),
                "{} ({ext}) had wrong signature: {:02X?}",
                entry.name,
                &out.get(..8)
            );
            if ext == "dds" {
                checked_dds = true;
            } else {
                checked_nif = true;
            }
        }
        if checked_dds && checked_nif {
            break;
        }
    }
    assert!(checked_dds || checked_nif, "found no dds/nif to verify");
}

#[test]
fn dx10_reconstructs_valid_dds() {
    let Some(dir) = build_dir() else {
        eprintln!("no MK18 build; skipping");
        return;
    };
    let bytes = std::fs::read(dir.join("MK18 - Textures.BA2")).unwrap();
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(archive.kind(), Kind::Texture);
    assert!(!archive.is_empty());

    let first = &archive.entries()[0];
    let dds = archive.extract(&first.name).unwrap();
    // reconstructed DDS: magic + 124-byte header + 20-byte DX10 ext + data
    assert!(
        dds.starts_with(b"DDS "),
        "reconstructed texture must be a DDS"
    );
    assert!(
        dds.len() > 148,
        "must carry header + at least one mip chunk"
    );
    // dwSize field of DDS_HEADER is 124
    assert_eq!(&dds[4..8], &124u32.to_le_bytes());
    // the pixelformat FourCC is DX10 (offset 4 + 4 + 76 = 84)
    assert_eq!(&dds[84..88], b"DX10");
}

#[test]
fn gnrl_extracted_size_matches_declared() {
    // metamorphic self-consistency on real data: a General file decompresses to
    // exactly its declared unpacked size (enforced inside extract()).
    let Some(dir) = build_dir() else {
        eprintln!("no MK18 build; skipping");
        return;
    };
    let bytes = std::fs::read(dir.join("MK18 - Main.BA2")).unwrap();
    let archive = Archive::parse(&bytes).unwrap();
    let mut extracted = 0;
    for entry in archive.entries().iter().take(50) {
        // extract() returns Err on any size mismatch, so success == consistent
        let out = archive.extract(&entry.name).unwrap();
        assert!(!out.is_empty() || entry.name.is_empty());
        extracted += 1;
    }
    assert!(extracted > 0);
}
