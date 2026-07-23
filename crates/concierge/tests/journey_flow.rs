//! Acceptance: a Nexus mod added through the browser realizes end-to-end, with
//! JOURNEY (mod 12685, v1.6.1) as the case. Hermetic — no network, no real
//! game: a crafted archive mirroring JOURNEY's versioned-root layout, served
//! to fetch via a `file://` url, a tiny fake Fallout 4 pristine.
//!
//! Drives add → fetch → PIN WRITE-BACK → build → LAYOUT RESOLVE → realize and
//! asserts: the manifest ends pinned (md5 set, version 1.6.1 kept), the
//! versioned root is stripped into `subdir`, `Journey.esp` is activated, and
//! the plugin lands in the instance's Data. Also checks the preflight names
//! the unpinned mod before converge and is clean after.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

fn craft_journey_archive(dir: &Path) -> std::path::PathBuf {
    // JOURNEY's real layout: everything under a single versioned folder.
    let root = dir.join("stage");
    let inner = root.join("JOURNEY - Survival Settlement Fast Travel v1.6.1");
    std::fs::create_dir_all(&inner).unwrap();
    // A REAL (minimal, master-less) plugin so the post-deploy master check can
    // parse it — a garbage .esp fails the TES4 header parse.
    let esp = concierge_esp::writer::resolver_bytes("Journey", &[]).unwrap();
    std::fs::write(inner.join("Journey.esp"), esp).unwrap();
    std::fs::write(inner.join("Journey - Main.ba2"), b"BA2\x00 fake archive").unwrap();
    let zip = dir.join("Journey-1.6.1-12685.zip");
    let out = Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&root)
        .output()
        .expect("bsdtar");
    assert!(
        out.status.success(),
        "bsdtar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    zip
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// The CLI `concierge-cli realize` must self-heal install layout the same way the
/// GUI's Apply does — strip JOURNEY's versioned root into `subdir` and activate
/// `Journey.esp` — so a pinned mod with no layout config still deploys right.
#[test]
fn cli_realize_self_heals_journey_layout() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-journey-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    let archive = craft_journey_archive(&base);
    let file = "Journey-1.6.1-12685.zip";
    let md5 = concierge::repo::md5_file(&archive).unwrap();
    // Seed the content-addressed store so fetch finds it PINNED (offline).
    std::fs::copy(&archive, base.join("store").join(format!("{md5}-{file}"))).unwrap();

    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();
    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest_path = profile.join("manifest.toml");
    // JOURNEY pinned (md5 + file) but NO subdir/plugins — the deploy must
    // infer them.
    std::fs::write(
        &manifest_path,
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
             version = \"1.11.221\"\n\
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mg\"\n\n\
             [[mod]]\nname = \"journey\"\nversion = \"1.6.1\"\nnexus_mod_id = 12685\n\
             md5 = \"{md5}\"\nfile = \"{file}\"\n",
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
        .expect("run concierge realize");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "realize failed:\n{log}");
    assert!(
        log.contains("resolved") && log.contains("journey"),
        "self-heal logged:\n{log}"
    );

    // Manifest gained the layout; plugin deployed to instance Data.
    let m = read(&manifest_path);
    assert!(
        m.contains("JOURNEY - Survival Settlement Fast Travel v1.6.1"),
        "subdir written:\n{m}"
    );
    assert!(m.contains("Journey.esp"), "plugin activated:\n{m}");
    assert!(
        base.join("instance/Data/Journey.esp").exists(),
        "Journey.esp deployed to Data"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn journey_adds_pins_resolves_and_realizes() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-journey-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let archive = craft_journey_archive(&base);

    // A tiny fake Fallout 4 pristine (with Data/ so the CoW clone has one).
    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();

    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();
    let manifest_path = profile.join("manifest.toml");

    // JOURNEY as it would look straight after a browser add: version captured
    // (1.6.1), but UNPINNED (no md5) and no layout resolved. Sourced from the
    // crafted archive via file:// so fetch produces a NeedsPin.
    std::fs::write(
        &manifest_path,
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
             version = \"1.11.221\"\n\
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mygames\"\n\n\
             [[mod]]\nname = \"journey\"\nversion = \"1.6.1\"\n\
             url = \"file://{}\"\nfile = \"Journey-1.6.1-12685.zip\"\n",
            pristine.display(),
            base.display(),
            archive.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let repo = concierge::repo::Repo::at(&profile);

    // Preflight before converge: the unpinned mod is named, not hidden.
    let plan0 =
        concierge::plan::eval(&concierge::manifest::Manifest::load(&profile).unwrap()).unwrap();
    let pre = concierge::realize::preflight(&repo, &plan0);
    assert!(
        pre.iter()
            .any(|i| i.kind == concierge::realize::PreflightKind::Unpinned
                && i.mod_name == "journey"),
        "preflight should flag the unpinned mod: {pre:?}"
    );

    // Converge: fetch → pin → build → resolve layout → realize.
    let report = concierge::realize::realize_converged(&repo, false).expect("converge");
    assert!(
        report.blocked.is_empty(),
        "nothing should be blocked: {:?}",
        report.blocked
    );
    assert!(report.report.is_some(), "realize ran");
    assert!(
        report.resolved.iter().any(|l| l.contains("pinned journey")),
        "pin write-back happened: {:?}",
        report.resolved
    );
    assert!(
        report
            .resolved
            .iter()
            .any(|l| l.contains("resolved layout for journey")),
        "layout resolved: {:?}",
        report.resolved
    );

    // The manifest ends pinned + laid out, with the real version kept.
    let m = read(&manifest_path);
    assert!(
        m.contains("md5 = \"") && !m.contains("md5 = \"\""),
        "pinned:\n{m}"
    );
    assert!(m.contains("version = \"1.6.1\""), "real version kept:\n{m}");
    assert!(
        m.contains("JOURNEY - Survival Settlement Fast Travel v1.6.1"),
        "subdir stripped:\n{m}"
    );
    assert!(m.contains("Journey.esp"), "plugin activated:\n{m}");

    // The plugin actually deployed into the instance's Data.
    let esp = base.join("instance/Data/Journey.esp");
    assert!(
        esp.exists(),
        "Journey.esp deployed to instance Data at {}",
        esp.display()
    );
    assert!(
        base.join("instance/Data/Journey - Main.ba2").exists(),
        "ba2 deployed too"
    );

    // Preflight after converge is clean.
    let plan1 =
        concierge::plan::eval(&concierge::manifest::Manifest::load(&profile).unwrap()).unwrap();
    assert!(
        concierge::realize::preflight(&repo, &plan1).is_empty(),
        "no unresolved items remain"
    );

    let _ = std::fs::remove_dir_all(&base);
}
