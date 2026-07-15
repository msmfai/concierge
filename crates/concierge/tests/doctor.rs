//! `concierge doctor` aggregates the health checks (all-green on a clean pack,
//! nonzero exit on a problem), and `concierge plugins` reports active vs inert.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

/// A build with an ACTIVE plugin (Keep.esp) and an INERT one (Extra.esp,
/// deployed but not in the mod's plugins list).
fn craft(dir: &Path) -> String {
    let stage = dir.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    for name in ["Keep.esp", "Extra.esp"] {
        let esp =
            concierge_esp::writer::resolver_bytes(name.trim_end_matches(".esp"), &[]).unwrap();
        std::fs::write(stage.join(name), esp).unwrap();
    }
    let zip = dir.join("mod.zip");
    let out = Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&stage)
        .output()
        .unwrap();
    assert!(out.status.success());
    concierge::repo::md5_file(&zip).unwrap()
}

#[test]
fn doctor_is_green_and_plugins_reports_active_vs_inert() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-doctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    let md5 = craft(&base);
    std::fs::copy(
        base.join("mod.zip"),
        base.join("store").join(format!("{md5}-mod.zip")),
    )
    .unwrap();
    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();
    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest = format!(
        "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
         version = \"1.11.221\"\n\
         [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mg\"\n\n\
         [[mod]]\nname = \"themod\"\nversion = \"1\"\nnexus_mod_id = 1\n\
         md5 = \"{md5}\"\nfile = \"mod.zip\"\nplugins = [\"Keep.esp\"]\n",
        pristine.display(),
        base.display(),
        base = base.display(),
    );
    std::fs::write(profile.join("manifest.toml"), &manifest).unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concierge"))
            .args(args)
            .env("CONCIERGE_REPO", &profile)
            .output()
            .unwrap()
    };

    // realize so there's an instance to inspect
    assert!(run(&["realize"]).status.success());

    // plugins: Keep.esp active, Extra.esp inert (deployed, not activated)
    let p = run(&["plugins"]);
    let plog = String::from_utf8_lossy(&p.stdout).into_owned();
    assert!(plog.contains("1 active"), "one active plugin:\n{plog}");
    assert!(
        plog.contains("inert (deployed, not active): 1"),
        "one inert:\n{plog}"
    );
    assert!(
        plog.contains("Extra.esp"),
        "names the inert plugin:\n{plog}"
    );

    // doctor: no hard failures (the inert plugin is a WARN, not a FAIL), exit 0
    let d = run(&["doctor"]);
    let dlog = String::from_utf8_lossy(&d.stdout).into_owned();
    assert!(
        d.status.success(),
        "doctor exit 0 with only warnings:\n{dlog}{}",
        String::from_utf8_lossy(&d.stderr)
    );
    assert!(!dlog.contains("[FAIL]"), "no hard failures:\n{dlog}");
    assert!(
        dlog.contains("[WARN] no inert plugins"),
        "inert is a warning:\n{dlog}"
    );

    // Introduce a problem (unpin the mod) → doctor fails, nonzero exit.
    std::fs::write(
        profile.join("manifest.toml"),
        manifest.replace(&format!("md5 = \"{md5}\"\n"), ""),
    )
    .unwrap();
    let d2 = run(&["doctor"]);
    assert!(!d2.status.success(), "doctor fails when a mod is unpinned");
    assert!(
        String::from_utf8_lossy(&d2.stdout).contains("[FAIL] pins"),
        "flags the unpinned mod"
    );

    let _ = std::fs::remove_dir_all(&base);
}
