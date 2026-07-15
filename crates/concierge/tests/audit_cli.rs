//! `concierge audit` end-to-end against a fixture catalog.
//! Hermetic: temp workspace, seeded `SQLite` catalog, parked (disabled,
//! unresolved) entries — the exact shape an agent's unverified suggestions
//! take. No network, no gate env var.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use concierge_db::catalog::{Catalog, Row};

fn run(profile: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_concierge"))
        .args(args)
        .env("CONCIERGE_REPO", profile)
        .output()
        .expect("run concierge");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn row(id: u64, name: &str) -> Row {
    Row {
        game_domain: "testgame".into(),
        mod_id: id,
        name: name.into(),
        ..Row::default()
    }
}

#[test]
fn audit_flags_invented_ids_and_eval_surfaces_unaudited() {
    let ws = std::env::temp_dir().join(format!("concierge-auditcli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let profile = ws.join("games/testgame/profiles/default");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::create_dir_all(ws.join("state")).unwrap();
    std::fs::write(ws.join(".concierge-workspace"), "").unwrap();
    let pristine = ws.join("fakegame");
    std::fs::create_dir_all(&pristine).unwrap();

    // Three parked Nexus suggestions: one real id, one invented id that names a
    // different mod, one id that doesn't exist at all.
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"testgame\"\npristine = \"{}\"\nversion = \"1\"\n\
             [game.custom]\ndefault_root = \"game\"\nnexus_domain = \"testgame\"\n\
             launch = [\"Game.app\"]\n\
             [[game.custom.root]]\nname = \"game\"\ndir = \"\"\n\
             [[mod]]\nname = \"journey\"\nversion = \"latest\"\nnexus_mod_id = 100\nenabled = false\n\
             [[mod]]\nname = \"campsite\"\nversion = \"latest\"\nnexus_mod_id = 200\nenabled = false\n\
             [[mod]]\nname = \"salvage-beacons\"\nversion = \"latest\"\nnexus_mod_id = 999\nenabled = false\n",
            pristine.display()
        ),
    )
    .unwrap();
    let mut cat = Catalog::open(&ws.join("state/catalog.sqlite")).unwrap();
    cat.upsert(&[
        row(100, "Journey - Survival Mode Fast Travel"),
        row(200, "We Are The Minutemen"), // NOT campsite: an invented id
    ])
    .unwrap();

    // Before any audit: eval surfaces all three as unaudited.
    let (ok, out) = run(&profile, &["eval"]);
    assert!(ok, "eval failed:\n{out}");
    assert!(
        out.contains("3 Nexus entries unaudited"),
        "eval must surface unaudited:\n{out}"
    );

    // Audit: definitive verdicts, nonzero exit because two ids are wrong.
    let (ok, out) = run(&profile, &["audit"]);
    assert!(!ok, "audit must fail loudly on unverified ids:\n{out}");
    assert!(out.contains("ok        journey"), "{out}");
    assert!(
        out.contains("MISMATCH  campsite") && out.contains("We Are The Minutemen"),
        "an invented id must name the real mod:\n{out}"
    );
    assert!(out.contains("UNKNOWN   salvage-beacons"), "{out}");
    assert!(out.contains("2 unverified id(s)"), "{out}");

    // The verdicts are recorded; eval now counts only the two bad ones.
    let audit_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(profile.join("state/audit.json")).unwrap())
            .unwrap();
    assert_eq!(audit_json["100"]["verdict"], "ok");
    assert_eq!(audit_json["200"]["verdict"], "name-mismatch");
    assert_eq!(audit_json["999"]["verdict"], "unknown-id");
    let (ok, out) = run(&profile, &["eval"]);
    assert!(ok);
    assert!(
        out.contains("2 Nexus entries unaudited"),
        "only bad ids remain:\n{out}"
    );

    // Without a catalog the door is named, not silently skipped.
    std::fs::remove_file(ws.join("state/catalog.sqlite")).unwrap();
    let (ok, out) = run(&profile, &["audit"]);
    assert!(
        !ok && out.contains("db sync testgame"),
        "missing catalog names the fix:\n{out}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = PathBuf::new();
}
