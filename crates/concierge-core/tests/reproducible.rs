//! The reproducibility gate: a modded instance is a PURE FUNCTION of
//! `(pristine, manifest)`. Rebuilding from scratch (`realize --fresh`) yields a
//! byte-identical instance — the Wabbajack guarantee. Touches no real game.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use concierge::manifest::Manifest;
use concierge::plan::eval;
use concierge::realize::realize;
use concierge::repo::{md5_file, Repo};
use std::path::Path;

/// A content identity for a directory tree: sorted `relpath:md5` lines. Two
/// trees with the same digest have byte-identical files at the same paths.
fn tree_digest(root: &Path) -> String {
    let mut lines = Vec::new();
    for e in walkdir_files(root) {
        let rel = e.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        lines.push(format!("{rel}:{}", md5_file(&e).unwrap()));
    }
    lines.sort();
    lines.join("\n")
}

fn walkdir_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn realize_is_a_pure_function_of_pristine_and_manifest() {
    let base = std::env::temp_dir().join(format!("concierge-repro-{}", std::process::id()));
    let pristine = base.join("pristine");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();
    std::fs::write(pristine.join("Data/base.esm"), b"the pristine base game").unwrap();

    let profile = base.join("profile");
    std::fs::create_dir_all(profile.join("state")).unwrap();
    let md5 = "b".repeat(32);
    // A minimal custom game: mods overlay the instance's Data root.
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"custom\"\npristine = \"{p}\"\ninstance = \"{i}\"\nversion = \"1\"\n\
             [game.paths]\nlo = \"{b}/lo.txt\"\n\
             [game.custom]\ndefault_root = \"data\"\n\
             [[game.custom.root]]\nname = \"data\"\ndir = \"Data\"\n\
             [[mod]]\nname = \"m\"\nversion = \"1\"\nurl = \"https://e/m\"\nmd5 = \"{md5}\"\n\
             file = \"m.7z\"\ninstall_root = \"data\"\n",
            p = pristine.display(),
            i = base.join("instance").display(),
            b = base.display(),
        ),
    )
    .unwrap();

    let repo = Repo::at(&profile);
    // pre-populate the mod's build tree (skip fetch/extract — not under test)
    let build = repo.build_path(&md5);
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("mod.esp"), b"a mod file").unwrap();

    let plan = eval(&Manifest::load(&profile).unwrap()).unwrap();
    let instance = base.join("instance");

    realize(&repo, &plan, true).unwrap();
    let first = tree_digest(&instance);
    // the instance is base + overlay
    assert!(
        instance.join("Data/base.esm").is_file(),
        "pristine cloned in"
    );
    assert!(instance.join("Data/mod.esp").is_file(), "mod overlaid");

    // rebuild from scratch — must be byte-identical
    realize(&repo, &plan, true).unwrap();
    let second = tree_digest(&instance);
    assert_eq!(
        first, second,
        "instance = f(pristine, manifest), reproducibly"
    );

    // and the plan itself is deterministic (the logical identity)
    let p2 = eval(&Manifest::load(&profile).unwrap()).unwrap();
    assert_eq!(plan.hash().unwrap(), p2.hash().unwrap());

    std::fs::remove_dir_all(&base).ok();
}
