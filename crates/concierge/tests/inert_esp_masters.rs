//! An inert deployed esp must NOT satisfy another plugin's master. A mod can
//! ship plugins it doesn't activate (unpicked FOMOD options land in Data but
//! not in plugins.txt); the engine only loads the load order plus implicit
//! vanilla content (base/DLC/CC), so a deployed-but-inactive esp is NOT loaded
//! and cannot satisfy a dependency. The missing-master lint must still flag it,
//! otherwise it hides a real crash-on-load.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

/// Build a zip containing one esp per (name, author, masters) tuple.
fn craft(dir: &Path, label: &str, esps: &[(&str, &str, &[String])]) -> std::path::PathBuf {
    let stage = dir.join(format!("stage-{label}"));
    std::fs::create_dir_all(&stage).unwrap();
    for (name, author, masters) in esps {
        let bytes = concierge_esp::writer::resolver_bytes(author, masters).unwrap();
        std::fs::write(stage.join(name), bytes).unwrap();
    }
    let zip = dir.join(format!("{label}.zip"));
    let out = Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&stage)
        .output()
        .expect("bsdtar");
    assert!(
        out.status.success(),
        "bsdtar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    zip
}

#[test]
fn inert_deployed_esp_does_not_satisfy_a_master() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-inert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    // The "framework" mod ships two esps but activates only Keep.esp, so
    // Framework.esp deploys to Data yet stays OUT of the load order (the inert
    // case). A non-empty plugins list also stops layout inference re-activating
    // it.
    let fw = craft(
        &base,
        "Framework",
        &[
            ("Keep.esp", "Keep", &[]),
            ("Framework.esp", "Framework", &[]),
        ],
    );
    // Dependent.esp is active and declares the inert Framework.esp as a master.
    let dep = craft(
        &base,
        "Dependent",
        &[(
            "Dependent.esp",
            "Dependent",
            &["Framework.esp".to_owned()][..],
        )],
    );
    let fw_md5 = concierge::repo::md5_file(&fw).unwrap();
    let dep_md5 = concierge::repo::md5_file(&dep).unwrap();
    std::fs::copy(
        &fw,
        base.join("store").join(format!("{fw_md5}-Framework.zip")),
    )
    .unwrap();
    std::fs::copy(
        &dep,
        base.join("store").join(format!("{dep_md5}-Dependent.zip")),
    )
    .unwrap();

    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();
    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
             version = \"1.11.221\"\n\
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mg\"\n\n\
             [[mod]]\nname = \"framework\"\nversion = \"1\"\nnexus_mod_id = 1\n\
             md5 = \"{fw_md5}\"\nfile = \"Framework.zip\"\nplugins = [\"Keep.esp\"]\n\n\
             [[mod]]\nname = \"dependent\"\nversion = \"1\"\nnexus_mod_id = 2\n\
             md5 = \"{dep_md5}\"\nfile = \"Dependent.zip\"\nplugins = [\"Dependent.esp\"]\n",
            pristine.display(),
            base.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
        .arg("realize")
        .env("CONCIERGE_REPO", &profile)
        .output()
        .expect("run realize");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Framework.esp IS on disk (deployed) but NOT activated, so the game would
    // never load it — the lint must fail and name the missing master.
    assert!(
        !out.status.success(),
        "realize must fail on the masked missing master:\n{log}"
    );
    assert!(
        log.contains("missing-master"),
        "flagged as missing-master:\n{log}"
    );
    assert!(
        log.contains("Framework.esp"),
        "names the absent master:\n{log}"
    );
    assert!(
        base.join("instance/Data/Framework.esp").exists(),
        "Framework.esp really is deployed (the point: on disk yet still flagged)"
    );

    let _ = std::fs::remove_dir_all(&base);
}
