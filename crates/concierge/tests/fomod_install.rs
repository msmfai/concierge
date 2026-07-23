//! End-to-end: a mod with `[mod.fomod]` installs exactly the files its
//! `fomod/ModuleConfig.xml` maps for the recorded picks — the Vortex/MO2 model.
//! The archive mirrors Survival Options: a shared asset + MCM at the root
//! (required), and the plugin down in a per-option folder. Only the picked
//! option's esp must land, flattened to the Data root.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::path::Path;
use std::process::Command;

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn fomod_installs_only_the_selected_files() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-fomodinstall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    // Craft the archive.
    let stage = base.join("stage");
    write(&stage.join("Mod - Main.ba2"), b"BA2\x00 asset");
    write(&stage.join("MCM").join("config.txt"), b"mcm");
    let keep = concierge_esp::writer::resolver_bytes("Keep", &[]).unwrap();
    write(&stage.join("Opt_Everything").join("TheMod.esp"), &keep);
    write(
        &stage.join("Opt_None").join("TheMod.esp"),
        b"WRONG - should not deploy",
    );
    write(
        &stage.join("fomod").join("ModuleConfig.xml"),
        br#"<config>
          <moduleName>TheMod</moduleName>
          <requiredInstallFiles>
            <file source="Mod - Main.ba2" destination="Mod - Main.ba2" />
            <folder source="MCM" destination="MCM" />
          </requiredInstallFiles>
          <installSteps order="Explicit"><installStep name="s">
            <optionalFileGroups><group name="Preset" type="SelectExactlyOne"><plugins>
              <plugin name="Everything">
                <files><file source="Opt_Everything\TheMod.esp" destination="TheMod.esp"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor>
              </plugin>
              <plugin name="None">
                <files><file source="Opt_None\TheMod.esp" destination="TheMod.esp"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor>
              </plugin>
            </plugins></group></optionalFileGroups>
          </installStep></installSteps>
        </config>"#,
    );
    let zip = base.join("themod.zip");
    let out = Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&stage)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "bsdtar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md5 = concierge::repo::md5_file(&zip).unwrap();
    std::fs::copy(&zip, base.join("store").join(format!("{md5}-themod.zip"))).unwrap();

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
             md5 = \"{md5}\"\nfile = \"themod.zip\"\nplugins = [\"TheMod.esp\"]\n\
             [mod.fomod]\nselect = [\"Everything\"]\n",
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
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "realize failed:\n{log}");

    let data = base.join("instance/Data");
    // required files installed
    assert!(data.join("Mod - Main.ba2").exists(), "required asset");
    assert!(data.join("MCM/config.txt").exists(), "required folder");
    // the picked option's esp, flattened to Data root
    assert!(
        data.join("TheMod.esp").exists(),
        "selected esp at Data root"
    );
    assert_eq!(
        std::fs::read(data.join("TheMod.esp")).unwrap(),
        keep,
        "the EVERYTHING esp, not None"
    );
    // NEITHER option folder is deployed raw
    assert!(
        !data.join("Opt_Everything").exists(),
        "no raw option folder"
    );
    assert!(
        !data.join("Opt_None").exists(),
        "the unpicked option is absent entirely"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn fomod_select_naming_a_nonexistent_option_fails_loudly() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-fomodbad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("store")).unwrap();
    std::fs::write(base.join(".concierge-workspace"), "").unwrap();

    let stage = base.join("stage");
    let keep = concierge_esp::writer::resolver_bytes("Keep", &[]).unwrap();
    write(&stage.join("Opt_A").join("TheMod.esp"), &keep);
    write(
        &stage.join("fomod").join("ModuleConfig.xml"),
        br#"<config><moduleName>TheMod</moduleName>
          <installSteps><installStep name="s"><optionalFileGroups>
            <group name="G" type="SelectExactlyOne"><plugins>
              <plugin name="Option A">
                <files><file source="Opt_A\TheMod.esp" destination="TheMod.esp"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor>
              </plugin>
            </plugins></group>
          </optionalFileGroups></installStep></installSteps>
        </config>"#,
    );
    let zip = base.join("themod.zip");
    let out = std::process::Command::new("bsdtar")
        .arg("-acf")
        .arg(&zip)
        .arg(".")
        .current_dir(&stage)
        .output()
        .unwrap();
    assert!(out.status.success());
    let md5 = concierge::repo::md5_file(&zip).unwrap();
    std::fs::copy(&zip, base.join("store").join(format!("{md5}-themod.zip"))).unwrap();

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
             md5 = \"{md5}\"\nfile = \"themod.zip\"\nplugins = [\"TheMod.esp\"]\n\
             [mod.fomod]\nselect = [\"Optoin A\"]\n",
            pristine.display(),
            base.display(),
            base = base.display(),
        ),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
        .arg("realize")
        .env("CONCIERGE_REPO", &profile)
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a typo'd select must fail realize:\n{log}"
    );
    assert!(
        log.contains("no such option") && log.contains("Optoin A"),
        "names the bad pick:\n{log}"
    );

    let _ = std::fs::remove_dir_all(&base);
}
