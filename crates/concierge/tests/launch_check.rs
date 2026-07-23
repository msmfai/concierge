//! `concierge-cli launch --check` parses the script-extender log — loaded plugins
//! vs incompatible ones — instead of launching. The automated "weird dll bug".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

#[test]
fn launch_check_parses_the_script_extender_log() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-launchcheck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    // A my_games with an F4SE/f4se.log: one loaded plugin, one incompatible.
    let mg = base.join("mg");
    std::fs::create_dir_all(mg.join("F4SE")).unwrap();
    std::fs::write(
        mg.join("F4SE").join("f4se.log"),
        "F4SE runtime: initialize\n\
         plugin Good.dll (00000001 Good 1) loaded correctly (handle 1)\n\
         plugin Buffout4.dll (00000001 Buffout4 1) disabled, incompatible with current version of the game 0 (handle 0)\n",
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
             [game.paths]\nplugins_txt = \"{base}/plugins.txt\"\nmy_games = \"{}\"\n\n\
             [[mod]]\nname = \"m\"\nversion = \"1\"\nnexus_mod_id = 1\nfile = \"m.zip\"\n",
            pristine.display(),
            base.display(),
            mg.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
        .args(["launch", "--check"])
        .env("CONCIERGE_REPO", &profile)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "launch --check ran:\n{log}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.contains("1 plugin(s) loaded"),
        "counts the loaded plugin:\n{log}"
    );
    assert!(
        log.contains("[WARN]") && log.to_lowercase().contains("incompatible"),
        "flags the incompatible plugin:\n{log}"
    );

    let _ = std::fs::remove_dir_all(&base);
}
