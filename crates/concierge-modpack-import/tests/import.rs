//! Import proofs on a SYNTHETIC fixture (`fixtures/sample.modlist.json`) — a
//! hand-authored modlist with invented ids/hashes/URLs that exercises every
//! source kind (Nexus/Http/other). A gated test parses a user-provided real
//! sample when present. Includes the metamorphic container relation: parsing the
//! JSON directly must equal parsing it wrapped in a `.wabbajack` ZIP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::Write as _;

use concierge_modpack_import::{ModList, Source};

const FIXTURE: &[u8] = include_bytes!("fixtures/sample.modlist.json");

#[test]
fn parses_fixture_into_concierge_model() {
    let list = ModList::from_modlist_json(FIXTURE).unwrap();
    assert_eq!(list.game, "Fallout4");
    assert_eq!(list.name, "Synthetic Sample Pack");
    assert_eq!(list.archives.len(), 6);

    // the 4 Nexus archives carry mod/file ids that map onto Concierge [[mod]]
    let nexus = list.nexus_mods();
    assert_eq!(nexus.len(), 4);
    for a in &nexus {
        match &a.source {
            Source::Nexus {
                game,
                mod_id,
                file_id,
            } => {
                assert_eq!(game, "Fallout4");
                assert!(*mod_id > 0 && *file_id > 0, "real nexus ids");
                assert!(!a.hash.is_empty(), "real xxHash64 (base64)");
                assert!(a.size > 0);
            }
            other => panic!("expected Nexus, got {other:?}"),
        }
    }
    // one Http (has a URL) and one Other (Mega/GDrive), recognized not dropped
    assert!(list
        .archives
        .iter()
        .any(|a| matches!(&a.source, Source::Http { url } if url.starts_with("http"))));
    assert!(list
        .archives
        .iter()
        .any(|a| matches!(a.source, Source::Other { .. })));
}

/// Wrap bytes in a minimal `.wabbajack` (a ZIP with a `modlist` entry).
fn wrap_wabbajack(json: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("modlist", opts).unwrap();
        zip.write_all(json).unwrap();
        zip.finish().unwrap();
    }
    buf
}

#[test]
fn container_roundtrip_equals_direct_parse() {
    // metamorphic: the .wabbajack container is transparent — reading the modlist
    // through the ZIP must yield the same model as reading the JSON directly.
    let bytes = wrap_wabbajack(FIXTURE);
    let tmp = std::env::temp_dir().join(format!("concierge-wj-{}.wabbajack", std::process::id()));
    std::fs::write(&tmp, &bytes).unwrap();

    let via_container = ModList::from_modpack_archive(&tmp).unwrap();
    let direct = ModList::from_modlist_json(FIXTURE).unwrap();

    assert_eq!(via_container.name, direct.name);
    assert_eq!(via_container.game, direct.game);
    assert_eq!(via_container.archives.len(), direct.archives.len());
    assert_eq!(via_container.nexus_mods().len(), direct.nexus_mods().len());
    assert_eq!(via_container.total_size(), direct.total_size());
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn rejects_garbage_without_panicking() {
    assert!(ModList::from_modlist_json(b"not json").is_err());
    // an empty JSON object is a valid (empty) modlist — total function, no panic
    let empty = ModList::from_modlist_json(b"{}").unwrap();
    assert!(empty.archives.is_empty());
}

#[test]
fn parses_the_full_real_sample_when_present() {
    // A full real modlist JSON, if provided: point CONCIERGE_WJ_SAMPLE at the
    // extracted `modlist` entry of a .wabbajack file. Skips cleanly otherwise.
    let Some(sample) = std::env::var_os("CONCIERGE_WJ_SAMPLE") else {
        eprintln!("CONCIERGE_WJ_SAMPLE not set; skipping");
        return;
    };
    let Ok(bytes) = std::fs::read(std::path::Path::new(&sample)) else {
        eprintln!("sample not readable; skipping");
        return;
    };
    let list = ModList::from_modlist_json(&bytes).unwrap();
    // Generic invariants for ANY real list — no dependence on a specific one.
    assert!(!list.game.is_empty(), "a real list names its game");
    assert!(!list.archives.is_empty(), "a real list has archives");
    assert!(
        !list.nexus_mods().is_empty(),
        "a real list has Nexus archives"
    );
    // the directive histogram is populated (FromArchive dominates a real list)
    assert!(list
        .directive_kinds
        .get("FromArchive")
        .is_some_and(|&n| n > 0));
}

#[test]
fn converts_a_modlist_to_an_evaluable_concierge_manifest() {
    let list = ModList::from_modlist_json(FIXTURE).unwrap();
    let toml = list.to_manifest_toml();
    // one [[mod]] per archive, each carrying its Wabbajack xxHash64 pin
    assert_eq!(toml.matches("[[mod]]").count(), list.archives.len());
    assert_eq!(toml.matches("xxhash =").count(), list.archives.len());
    assert!(toml.contains("[game]"));
    // and it parses back as valid TOML (structural round-trip)
    let parsed: toml::Value = toml::from_str(&toml).expect("generated manifest is valid TOML");
    assert!(parsed.get("game").is_some());
}
