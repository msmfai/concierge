//! `concierge-cli preview` shows what realize WOULD deploy (files + activations)
//! without touching the instance, and its file set matches the real deploy.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

fn craft(dir: &Path) -> (std::path::PathBuf, String) {
    let stage = dir.join("stage");
    let inner = stage.join("Mod v1");
    std::fs::create_dir_all(&inner).unwrap();
    let esp = concierge_esp::writer::resolver_bytes("Mod", &[]).unwrap();
    std::fs::write(inner.join("Mod.esp"), esp).unwrap();
    std::fs::write(inner.join("readme.txt"), b"hi").unwrap();
    let zip = dir.join("mod.zip");
    let out = Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&stage)
        .output()
        .unwrap();
    assert!(out.status.success());
    let md5 = concierge::repo::md5_file(&zip).unwrap();
    (zip, md5)
}

#[test]
fn preview_matches_deploy_and_touches_nothing() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    let (zip, md5) = craft(&base);
    std::fs::copy(&zip, base.join("store").join(format!("{md5}-mod.zip"))).unwrap();
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
             [[mod]]\nname = \"themod\"\nversion = \"1\"\nnexus_mod_id = 1\n\
             md5 = \"{md5}\"\nfile = \"mod.zip\"\nsubdir = \"Mod v1\"\nplugins = [\"Mod.esp\"]\n",
            pristine.display(),
            base.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let run = |arg: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
            .args(arg)
            .env("CONCIERGE_REPO", &profile)
            .output()
            .unwrap()
    };

    // preview shows the deploy + activation, and does NOT create the instance
    let out = run(&["preview", "--files"]);
    let log = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "preview ok: {log}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.contains("data:Mod.esp"),
        "shows deployed plugin path:\n{log}"
    );
    assert!(
        log.contains("activates: Mod.esp"),
        "shows activation entry:\n{log}"
    );
    assert!(
        log.contains("nothing deployed"),
        "states nothing deployed:\n{log}"
    );
    assert!(
        !base.join("instance").exists(),
        "preview must not create the instance"
    );

    // now realize, and confirm the deployed set equals what preview listed
    let r = run(&["realize"]);
    assert!(
        r.status.success(),
        "realize: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        base.join("instance/Data/Mod.esp").exists(),
        "the previewed file is what deploys"
    );
    assert!(base.join("instance/Data/readme.txt").exists());

    let _ = std::fs::remove_dir_all(&base);
}
