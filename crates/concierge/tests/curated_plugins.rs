//! Regression: `resolve_layouts` must INFER plugins only into an empty list,
//! never clobber a curated one. A FOMOD-style mod ships many mutually-exclusive
//! .esps (e.g. Better Settlers' 11 variants); the manifest activates ONE. Before
//! the fix, every realize re-detected all of them and appended the other ten
//! back — reactivating patches whose masters aren't present and failing the
//! post-deploy master check on an otherwise-valid pack.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

#[test]
fn resolve_layouts_keeps_a_curated_plugins_list() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-curated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    // A fake pristine so eval/realize have a game to clone.
    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();

    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest_path = profile.join("manifest.toml");

    // A pinned mod with a CURATED single-plugin activation, already laid out.
    let md5 = "abcdef0123456789abcdef0123456789";
    std::fs::write(
        &manifest_path,
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
             version = \"1.11.221\"\n\
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mg\"\n\n\
             [[mod]]\nname = \"better-settlers\"\nversion = \"1.0\"\nnexus_mod_id = 4772\n\
             md5 = \"{md5}\"\nfile = \"bs.zip\"\nplugins = [\"BetterSettlers.esp\"]\n",
            pristine.display(),
            base.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let repo = concierge::repo::Repo::at(&profile);

    // Seed the build tree with the curated plugin PLUS extra variants that
    // inference would otherwise re-activate.
    let build = repo.build_path(md5);
    touch(&build.join("BetterSettlers.esp"));
    touch(&build.join("BetterSettlersCCAPack2.0.esp"));
    touch(&build.join("BetterSettlersMostlyMale.esp"));

    let plan =
        concierge::plan::eval(&concierge::manifest::Manifest::load(&profile).unwrap()).unwrap();
    let changed = concierge::realize::resolve_layouts(&repo, &plan).unwrap();

    // No change for a mod whose plugins are already curated.
    assert!(
        !changed.iter().any(|l| l.contains("better-settlers")),
        "curated mod should not be re-resolved: {changed:?}"
    );
    // The manifest still activates exactly the one curated plugin.
    let m = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        m.contains("plugins = [\"BetterSettlers.esp\"]"),
        "curation intact:\n{m}"
    );
    assert!(!m.contains("CCAPack"), "variant NOT re-added:\n{m}");
    assert!(!m.contains("MostlyMale"), "variant NOT re-added:\n{m}");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn resolve_layouts_still_infers_into_an_empty_list() {
    // The convenience path must keep working: a mod with NO plugins declared
    // gets the build tree's plugins activated for it.
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-curated-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();
    let pristine = base.join("Fallout 4");
    std::fs::create_dir_all(pristine.join("Data")).unwrap();
    let profile = base.join("games/fallout4/profiles/test");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest_path = profile.join("manifest.toml");

    let md5 = "0123456789abcdef0123456789abcdef";
    std::fs::write(
        &manifest_path,
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{}\"\ninstance = \"{}/instance\"\n\
             version = \"1.11.221\"\n\
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{base}/mg\"\n\n\
             [[mod]]\nname = \"solo-mod\"\nversion = \"1.0\"\nnexus_mod_id = 1\n\
             md5 = \"{md5}\"\nfile = \"s.zip\"\n",
            pristine.display(),
            base.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let repo = concierge::repo::Repo::at(&profile);
    touch(&repo.build_path(md5).join("SoloMod.esp"));

    let plan =
        concierge::plan::eval(&concierge::manifest::Manifest::load(&profile).unwrap()).unwrap();
    let changed = concierge::realize::resolve_layouts(&repo, &plan).unwrap();

    assert!(
        changed
            .iter()
            .any(|l| l.contains("solo-mod") && l.contains("SoloMod.esp")),
        "empty list gets inferred: {changed:?}"
    );
    let m = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        m.contains("SoloMod.esp"),
        "plugin activated into empty list:\n{m}"
    );

    let _ = std::fs::remove_dir_all(&base);
}
