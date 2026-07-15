//! The `BSDiff` patch source, end to end: a mod ships a patch (not content); the
//! build derives the target from a file the user owns. Touches no real game.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use concierge::build::build_all;
use concierge::manifest::Manifest;
use concierge::plan::eval;
use concierge::repo::Repo;

#[test]
fn a_patch_mod_derives_its_target_from_the_owned_source() {
    let base = std::env::temp_dir().join(format!("concierge-patchbuild-{}", std::process::id()));
    let profile = base.join("profile");
    std::fs::create_dir_all(profile.join("state")).unwrap();

    // The user owns `source`; the pack wants `target`; it ships only a patch.
    let source = b"BEGIN owned base file contents, mostly the same across versions END";
    let target = b"BEGIN owned base file contents, MOSTLY the same across versions!! END";
    let patch = concierge_patch::diff(source, target).unwrap();
    let owned = base.join("owned.dat");
    std::fs::write(&owned, source).unwrap();

    // A minimal custom-game profile with one patch mod.
    let md5 = "a".repeat(32); // store key; build derives, it doesn't re-verify
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"custom\"\npristine = \"{base}\"\nversion = \"1\"\n\
             [game.paths]\nloadorder = \"{base}/lo.txt\"\n\
             [game.custom]\ndefault_root = \"mods\"\n\
             [[game.custom.root]]\nname = \"mods\"\ndir = \"mods\"\n\
             [[mod]]\nname = \"patched\"\nversion = \"1\"\nurl = \"https://e/p\"\n\
             md5 = \"{md5}\"\nfile = \"patch.bsdiff\"\ninstall_root = \"mods\"\n\
             patch = {{ from = \"{owned}\", to = \"derived.dat\" }}\n",
            base = base.display(),
            owned = owned.display(),
        ),
    )
    .unwrap();

    let repo = Repo::at(&profile);
    std::fs::create_dir_all(repo.store()).unwrap();
    std::fs::write(repo.store_path(&md5, "patch.bsdiff"), &patch).unwrap();

    let m = Manifest::load(&profile).unwrap();
    let plan = eval(&m).unwrap();
    build_all(&repo, &plan).unwrap();

    // the build derived the exact target from (owned source, shipped patch)
    let derived = repo.build_path(&md5).join("derived.dat");
    assert_eq!(
        std::fs::read(&derived).unwrap(),
        target,
        "target derived at build"
    );
    // the owned source is untouched
    assert_eq!(std::fs::read(&owned).unwrap(), source, "source not mutated");

    std::fs::remove_dir_all(&base).ok();
}
