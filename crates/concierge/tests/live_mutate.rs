//! Mutating live suite: real fetch→build→realize→check→undeploy round-trips
//! against the machine's ACTUAL game installs — with every write aimed at
//! disposable targets. Gated behind `CONCIERGE_LIVE_MUTATE=1`.
//!
//! Run:
//!   `CONCIERGE_LIVE_MUTATE=1 cargo test -p concierge --test live_mutate -- --nocapture`
//!
//! Per installed game the suite builds a throwaway workspace (own store/,
//! own profile) whose manifest points at the REAL pristine install but at
//! TEMP external paths (plugins.txt, `my_games`, BG3's Documents mount), crafts
//! a tiny mod archive, seeds it into the temp store by content hash (so fetch
//! is satisfied offline), then drives the real binary through the whole
//! lifecycle and asserts:
//!   - realize clones a `CoW` instance / mounts and deploys the mod for real,
//!   - the invariant lints run and pass on the deployed result,
//!   - `check` is clean after deploy and a second realize places nothing,
//!   - undeploy removes every owned file, restores mounts, resets configs,
//!   - the pristine install is bit-for-bit untouched throughout.
//!
//! The real Documents/Larian dirs, real INIs, and real load-order files are
//! never named in the test manifests, so they cannot be written. The pristine
//! is only ever read (APFS copy-on-write clone). Filter games with
//! `CONCIERGE_LIVE_GAMES="bg3,kotor2"`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::{Path, PathBuf};

use common::{concierge, game_dirs, pristine_of, snapshot, snapshot_diff};

const BG3_TEST_UUID: &str = "11111111-2222-3333-4444-555555555555";

fn live_mutate() -> bool {
    std::env::var("CONCIERGE_LIVE_MUTATE").as_deref() == Ok("1")
}

/// What a game kind needs to round-trip: its `[game]` extras and the tiny mod.
struct Spec {
    version: &'static str,
    /// `[game.paths]` block, with `@EXT@` substituted by the temp external dir.
    paths_toml: &'static str,
    /// `plugins = [...]` line for the mod, if the kind activates by entry.
    plugins_toml: &'static str,
    /// Files inside the crafted mod archive.
    mod_files: &'static [(&'static str, &'static str)],
    /// A filename that must exist somewhere under the instance after realize
    /// (and be gone after undeploy).
    probe: &'static str,
}

fn spec(kind: &str) -> Option<Spec> {
    match kind {
        "bg3" => Some(Spec {
            version: "4.7",
            paths_toml: "[game.paths]\nprofile_mods = \"@EXT@/Mods\"\nmodsettings = \"@EXT@/PlayerProfiles/Public/modsettings.lsx\"\n",
            plugins_toml: "plugins = [\"11111111-2222-3333-4444-555555555555|TestConcierge\"]\n",
            mod_files: &[(
                "TestConcierge_11111111-2222-3333-4444-555555555555.pak",
                "LSPK test payload — not a real pak; deploy/lint only look at name+listing",
            )],
            probe: "TestConcierge_11111111-2222-3333-4444-555555555555.pak",
        }),
        "fallout4" | "skyrimse" => Some(Spec {
            version: "livetest",
            paths_toml: "[game.paths]\nplugins_txt = \"@EXT@/plugins.txt\"\nmy_games = \"@EXT@/MyGames\"\n",
            plugins_toml: "",
            mod_files: &[("ConciergeTest/concierge-livetest.txt", "live mutate probe")],
            probe: "concierge-livetest.txt",
        }),
        "kotor2" | "kotor" => Some(Spec {
            version: "livetest",
            paths_toml: "",
            plugins_toml: "",
            mod_files: &[("concierge-livetest.txt", "live mutate probe")],
            probe: "concierge-livetest.txt",
        }),
        // Civ V goes through the data-driven `custom` kind — the manifest alone
        // shapes the game, with the MODS dir as an external mount.
        "civ5" => Some(Spec {
            version: "livetest",
            paths_toml: "[game.paths]\nciv5_mods = \"@EXT@/MODS\"\n\n\
                [game.custom]\ndefault_root = \"mods\"\nlaunch = [\"Civilization V.app\"]\nsteam_app_id = 8930\n\n\
                [[game.custom.root]]\nname = \"game\"\ndir = \"\"\n\n\
                [[game.custom.root]]\nname = \"mods\"\npath_key = \"civ5_mods\"\n",
            plugins_toml: "",
            mod_files: &[("ConciergeTest (v 1)/concierge-livetest.txt", "live mutate probe")],
            probe: "concierge-livetest.txt",
        }),
        // The no-knowledge fallback: mods just add/replace files under the
        // game dir, realized as a pure diff (CoW instance) against pristine.
        "generic" => Some(Spec {
            version: "livetest",
            paths_toml: "",
            plugins_toml: "",
            mod_files: &[("ConciergeTest/concierge-livetest.txt", "live mutate probe")],
            probe: "concierge-livetest.txt",
        }),
        _ => None,
    }
}

/// Recursively find a file by name; symlinks are followed (BG3 mounts).
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        let meta = std::fs::metadata(&p).ok()?;
        if meta.is_dir() {
            if let Some(hit) = find_file(&p, name) {
                return Some(hit);
            }
        } else if e.file_name().to_string_lossy() == name {
            return Some(p);
        }
    }
    None
}

/// Same-filesystem guard: a cross-device instance clone would silently fall
/// back from `CoW` to a full copy (tens of GB) — skip instead.
#[cfg(unix)]
fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev(),
        _ => false,
    }
}
#[cfg(not(unix))]
fn same_device(_a: &Path, _b: &Path) -> bool {
    true
}

/// Build the throwaway workspace + profile for one game; returns the profile
/// dir. Everything writable lives under `ws`; the pristine is only referenced.
fn build_workspace(ws: &Path, kind: &str, pristine: &Path, s: &Spec) -> PathBuf {
    let ext = ws.join("external");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::create_dir_all(ws.join("store")).unwrap();
    std::fs::write(ws.join(".concierge-workspace"), "").unwrap();

    // Craft the mod archive with the same archiver the build phase shells to.
    let staging = ws.join("staging");
    for (rel, content) in s.mod_files {
        let p = staging.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
    }
    let file = format!("concierge-livetest-{kind}.zip");
    let zip = ws.join(&file);
    let out = std::process::Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&staging)
        .output()
        .expect("bsdtar not runnable — the build phase needs it too");
    assert!(
        out.status.success(),
        "bsdtar failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Seed the temp store by content hash — fetch finds it, no network.
    let md5 = concierge::repo::md5_file(&zip).unwrap();
    std::fs::copy(&zip, ws.join("store").join(format!("{md5}-{file}"))).unwrap();

    let profile = ws.join("games").join(kind).join("profiles").join("test");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest = format!(
        "[game]\nkind = \"{kind}\"\npristine = \"{}\"\nversion = \"{}\"\n{}\n\
         [[mod]]\nname = \"concierge-livetest\"\nversion = \"1.0\"\n\
         url = \"https://invalid.example/{file}\"\nmd5 = \"{md5}\"\nfile = \"{file}\"\n{}",
        pristine.display(),
        s.version,
        s.paths_toml.replace("@EXT@", &ext.display().to_string()),
        s.plugins_toml,
    );
    std::fs::write(profile.join("manifest.toml"), manifest).unwrap();
    profile
}

/// Drive one game through the full lifecycle; returns this game's failures.
fn round_trip(kind: &str, s: &Spec, pristine: &Path, ws: &Path) -> Vec<String> {
    let mut failures = Vec::new();
    let before = snapshot(pristine);
    let profile = build_workspace(ws, kind, pristine, s);
    let instance = profile.join("state").join("instance");
    let mut fail = |msg: String| failures.push(format!("{kind}: {msg}"));

    // eval → realize (fetch/build resolve from the seeded store).
    let (ok, out) = concierge(&profile, &["eval"], None);
    if !ok {
        fail(format!("eval failed:\n{out}"));
        return failures;
    }
    let (ok, out) = concierge(&profile, &["realize"], None);
    if !ok {
        fail(format!("realize failed:\n{out}"));
        return failures;
    }
    println!("  realize ok");

    // The mod must be deployed for real, inside the disposable instance.
    match find_file(&instance, s.probe) {
        Some(hit) => println!(
            "  deployed  {}",
            hit.strip_prefix(ws).unwrap_or(&hit).display()
        ),
        None => fail(format!(
            "probe {} not deployed under {}",
            s.probe,
            instance.display()
        )),
    }
    if kind == "bg3" {
        let ms = ws.join("external/PlayerProfiles/Public/modsettings.lsx");
        match std::fs::read_to_string(&ms) {
            Ok(t) if t.contains(BG3_TEST_UUID) => println!("  modsettings lists the test pak"),
            Ok(_) => fail("modsettings.lsx missing the test pak entry".into()),
            Err(e) => fail(format!("modsettings.lsx unreadable: {e}")),
        }
        let mods_link = ws.join("external/Mods");
        if !std::fs::symlink_metadata(&mods_link).is_ok_and(|m| m.file_type().is_symlink()) {
            fail("external Mods dir was not mounted (symlink) into the instance".into());
        }
    }

    // Clean drift, and a second realize must be a no-op.
    let (ok, out) = concierge(&profile, &["check"], None);
    if ok {
        println!("  check clean");
    } else {
        fail(format!("check after realize:\n{out}"));
    }
    let (ok, out) = concierge(&profile, &["realize"], None);
    if !ok || !out.contains("placed    0 files") {
        fail(format!("second realize not idempotent:\n{out}"));
    } else {
        println!("  re-realize idempotent");
    }

    // Round-trip back out.
    let (ok, out) = concierge(&profile, &["undeploy"], None);
    if !ok {
        fail(format!("undeploy failed:\n{out}"));
    } else if let Some(hit) = find_file(&instance, s.probe) {
        fail(format!("probe survived undeploy: {}", hit.display()));
    } else {
        println!("  undeploy clean");
    }
    if kind == "bg3" {
        if std::fs::symlink_metadata(ws.join("external/Mods"))
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            fail("undeploy left the external Mods mount symlinked".into());
        }
        let reset =
            std::fs::read_to_string(ws.join("external/PlayerProfiles/Public/modsettings.lsx"))
                .unwrap_or_default();
        if reset.contains(BG3_TEST_UUID) {
            fail("config reset still lists the test pak".into());
        }
    }

    // The real install never changed.
    let after = snapshot(pristine);
    if before == after {
        println!("  pristine untouched ({} files)", after.len());
    } else {
        fail(format!(
            "PRISTINE TOUCHED: {}",
            snapshot_diff(&before, &after).join(", ")
        ));
    }
    failures
}

/// Non-supported games as the generic case: pick a real install nothing in
/// concierge covers and round-trip a pure file-overlay against it — proving a
/// user can point `kind = "generic"` at ANY game dir and get the full deploy/
/// check/undeploy lifecycle with the install untouched.
#[test]
fn generic_kind_round_trips_an_unsupported_install() {
    if !live_mutate() {
        println!("skipped (set CONCIERGE_LIVE_MUTATE=1)");
        return;
    }
    let steam = concierge::repo::home().join("Library/Application Support/Steam/steamapps/common");
    let candidates = [
        "FTL Faster Than Light",
        "Galimulator",
        "PlagueInc",
        "Proteus",
        "Roadwarden",
    ];
    let Some(pristine) = candidates
        .iter()
        .map(|c| steam.join(c))
        .find(|p| p.is_dir())
    else {
        println!("no unsupported install found to exercise — skipped");
        return;
    };
    let root = std::env::temp_dir().join(format!("concierge-live-generic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    if !same_device(&pristine, &root) {
        println!("temp dir on a different filesystem (no CoW) — skipped");
        return;
    }
    println!("generic: round-tripping against {}", pristine.display());
    let failures = round_trip(
        "generic",
        &spec("generic").unwrap(),
        &pristine,
        &root.join("generic"),
    );
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        failures.is_empty(),
        "generic round-trip failures:\n{}",
        failures.join("\n")
    );
}

/// The suite: deploy a real mod through the real binary into each installed
/// game's disposable instance, verify, round-trip back out, and prove the
/// pristine never changed.
#[test]
fn realize_round_trips_against_real_installs() {
    if !live_mutate() {
        println!("skipped (set CONCIERGE_LIVE_MUTATE=1)");
        return;
    }
    let root = std::env::temp_dir().join(format!("concierge-live-mutate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut failures: Vec<String> = Vec::new();
    let mut covered = 0usize;

    for game_dir in game_dirs() {
        let kind = game_dir.file_name().unwrap().to_string_lossy().into_owned();
        // Adopted generic games (any dir name, kind = "generic") round-trip
        // with the generic spec; other unknown games need a spec written.
        let s = spec(&kind).or_else(|| {
            (common::kind_of(&game_dir).as_deref() == Some("generic"))
                .then(|| spec("generic").expect("generic spec exists"))
        });
        let Some(s) = s else {
            println!("{kind}: no mutate spec — skipped");
            continue;
        };
        let Some(pristine) = pristine_of(&game_dir) else {
            println!("{kind}: no loadable profile — skipped");
            continue;
        };
        if !pristine.is_dir() {
            println!("{kind}: pristine not installed — skipped");
            continue;
        }
        std::fs::create_dir_all(&root).unwrap();
        if !same_device(&pristine, &root) {
            println!("{kind}: temp dir on a different filesystem (no CoW) — skipped");
            continue;
        }
        covered += 1;
        println!("{kind}: round-tripping against {}", pristine.display());
        failures.extend(round_trip(&kind, &s, &pristine, &root.join(&kind)));
    }

    let _ = std::fs::remove_dir_all(&root);
    assert!(covered > 0, "no game installs found on this machine");
    assert!(
        failures.is_empty(),
        "live mutate failures:\n{}",
        failures.join("\n")
    );
}
