//! Tests for the pure core: manifest validation, adapter resolution, eval
//! determinism, generated config texts, runtime detection, and state keys.
//! Nothing here touches the network or mutates the filesystem.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

use concierge::manifest::Manifest;
use concierge::plan::Source;
use concierge::runtime::{detect, game_visible_path, Runtime};
use concierge::state::{key, parse_key};

// The lib knows no game; these thin wrappers register the workspace's adapter
// crates (via the assembly) before resolving/evaluating, so every eval below
// runs against the real family/leaf adapters. `register_adapters` is idempotent.
fn eval(m: &Manifest) -> concierge::error::Result<concierge::plan::Plan> {
    concierge_games::register();
    concierge::plan::eval(m)
}
fn adapter_for(kind: &str) -> concierge::error::Result<&'static dyn concierge::game::GameAdapter> {
    concierge_games::register();
    concierge::game::adapter_for(kind)
}

const BASE: &str = r#"
[game]
pristine = "/tmp/pristine"
instance = "/tmp/fo4nix/instance"
plugins_txt = "/tmp/plugins.txt"
my_games = "/tmp/mygames"
version = "1.11.221"
dlc = ["DLCRobot.esm", "DLCCoast.esm"]

[ini.Archive]
bInvalidateOlderFiles = "1"
sResourceDataDirsFinal = ""

[[mod]]
name = "alpha"
version = "1.0"
nexus_mod_id = 1
nexus_file_id = 10
md5 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
file = "alpha.7z"
plugins = ["Alpha.esp"]

[[mod]]
name = "beta"
version = "2.0"
url = "https://example.com/beta.7z"
md5 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
install_root = "game"
subdir = "b"
exclude = ["src/"]
"#;

const BG3: &str = r#"
[game]
kind = "bg3"
pristine = "/tmp/bg3"
instance = "/tmp/concierge/bg3-instance"
version = "4.7"

[game.paths]
profile_mods = "/tmp/larian/Mods"
modsettings = "/tmp/larian/PlayerProfiles/Public/modsettings.lsx"

[[mod]]
name = "impui"
version = "1.0"
nexus_mod_id = 100
nexus_file_id = 200
md5 = "cccccccccccccccccccccccccccccccc"
file = "impui.zip"
plugins = ["3a4b5c6d-0000-1111-2222-333344445555|ImpUI"]
"#;

#[test]
fn valid_manifest_parses_with_legacy_paths() {
    let m = Manifest::parse(BASE).unwrap();
    assert_eq!(m.game.kind, "fallout4", "kind defaults to fallout4");
    assert_eq!(
        m.game.paths.get("plugins_txt").unwrap(),
        Path::new("/tmp/plugins.txt"),
        "legacy plugins_txt folds into paths"
    );
    assert_eq!(m.mods.len(), 2);
}

#[test]
fn duplicate_mod_names_rejected() {
    let dup = format!(
        "{BASE}\n[[mod]]\nname = \"alpha\"\nversion = \"9\"\nfile = \"x.7z\"\nurl = \"https://e/x.7z\"\n"
    );
    assert!(Manifest::parse(&dup)
        .unwrap_err()
        .to_string()
        .contains("duplicate mod name"));
}

#[test]
fn bad_md5_rejected() {
    let bad = BASE.replace("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "nothex");
    assert!(Manifest::parse(&bad)
        .unwrap_err()
        .to_string()
        .contains("md5 must be 32 hex chars"));
}

#[test]
fn disabled_entries_may_be_parked_unresolved() {
    // An agent's honest state for an unverified suggestion: nexus_mod_id
    // known, no file picked yet, enabled = false. Parses and evals (the
    // disabled entry is simply not in the plan); the SAME entry enabled is
    // rejected because it can't be fetched.
    let parked = format!(
        "{BASE}\n[[mod]]\nname = \"unresolved\"\nversion = \"latest\"\nnexus_mod_id = 99\nenabled = false\n"
    );
    let m = Manifest::parse(&parked).unwrap();
    assert!(eval(&m).is_ok());
    assert!(!eval(&m)
        .unwrap()
        .mods
        .iter()
        .any(|md| md.name == "unresolved"));
    let live = parked.replace("enabled = false", "enabled = true");
    assert!(Manifest::parse(&live)
        .unwrap_err()
        .to_string()
        .contains("needs `file`"));
}

#[test]
fn missing_source_rejected() {
    let orphan = format!("{BASE}\n[[mod]]\nname = \"gamma\"\nversion = \"1\"\n");
    assert!(Manifest::parse(&orphan)
        .unwrap_err()
        .to_string()
        .contains("no source"));
}

#[test]
fn unknown_game_kind_without_custom_rejected_at_eval() {
    // An unknown kind with no codified adapter AND no [game.custom] fails —
    // but the door to servicing it (add [game.custom]) is named in the error.
    let weird = BASE.replace(
        "pristine = \"/tmp/pristine\"",
        "kind = \"doom1993\"\npristine = \"/tmp/pristine\"",
    );
    let m = Manifest::parse(&weird).unwrap();
    assert!(eval(&m)
        .unwrap_err()
        .to_string()
        .contains("no codified adapter and no [game.custom]"));
}

#[test]
fn unknown_install_root_rejected_at_eval() {
    let bad = BASE.replace("install_root = \"game\"", "install_root = \"weird\"");
    let m = Manifest::parse(&bad).unwrap();
    assert!(eval(&m)
        .unwrap_err()
        .to_string()
        .contains("install_root 'weird' unknown"));
}

#[test]
fn eval_is_deterministic_and_order_sensitive() {
    let m = Manifest::parse(BASE).unwrap();
    assert_eq!(
        eval(&m).unwrap().hash().unwrap(),
        eval(&m).unwrap().hash().unwrap()
    );
    let mut reordered = m.clone();
    reordered.mods.reverse();
    assert_ne!(
        eval(&m).unwrap().hash().unwrap(),
        eval(&reordered).unwrap().hash().unwrap(),
        "load order must be semantic"
    );
}

#[test]
fn fallout4_configs_render() {
    let m = Manifest::parse(BASE).unwrap();
    let plan = eval(&m).unwrap();
    assert_eq!(plan.game.kind, "fallout4");
    assert_eq!(plan.configs.len(), 2);
    let plugins = &plan.configs[0];
    assert_eq!(plugins.path, "/tmp/plugins.txt");
    let lines: Vec<&str> = plugins.content.lines().collect();
    assert!(lines[0].starts_with('#'));
    assert_eq!(lines[1], "*DLCRobot.esm");
    assert_eq!(lines[2], "*DLCCoast.esm");
    assert_eq!(lines[3], "*Alpha.esp");
    let ini = &plan.configs[1];
    assert_eq!(ini.path, "/tmp/mygames/Fallout4Custom.ini");
    assert!(ini.content.contains("bInvalidateOlderFiles=1\n"));
    // resets drop mod plugins but keep DLC + ini
    assert!(plan.config_resets[0].content.contains("*DLCCoast.esm"));
    assert!(!plan.config_resets[0].content.contains("*Alpha.esp"));
}

#[test]
fn fallout4_roots_and_launch() {
    let m = Manifest::parse(BASE).unwrap();
    let plan = eval(&m).unwrap();
    assert!(plan.root_targets["data"].instance_relative);
    assert_eq!(plan.root_targets["data"].dir, "Data");
    assert_eq!(plan.launch_candidates[0], "f4se_loader.exe");
    assert!(plan.needs_instance(), "fo4 mods target the game dir");
}

#[test]
fn bg3_profile_mods_deploy_into_an_instance_mount_not_the_real_dir() {
    // Wabbajack model: BG3's external Documents `Mods/` becomes an instance-owned
    // MOUNT — mods deploy into `<instance>/mounts/mods`, and the real dir is
    // park+symlinked to it at realize time (never mutated in place).
    let m = Manifest::parse(BG3).unwrap();
    let plan = eval(&m).unwrap();
    let mods_root = &plan.root_targets["mods"];
    assert!(
        mods_root.instance_relative,
        "mount deploys into the instance"
    );
    assert_eq!(mods_root.dir, "mounts/mods");
    assert_eq!(
        mods_root.mount_at.as_deref(),
        Some("/tmp/larian/Mods"),
        "the real Documents path is remembered for park+symlink"
    );
    assert!(plan.needs_instance(), "an instance hosts the mount");
    assert!(
        !plan.needs_game_clone(),
        "mount-only: the BG3 game is never copied"
    );
    let settings = &plan.configs[0];
    assert!(settings.path.ends_with("modsettings.lsx"));
    assert!(settings
        .content
        .contains("3a4b5c6d-0000-1111-2222-333344445555"));
    assert!(settings.content.contains("GustavX"), "base module present");
}

#[test]
fn loading_a_profile_defaults_an_instance_so_mount_games_isolate() {
    // A BG3 profile with NO [game].instance: Manifest::load must default one so
    // the Documents mount resolves into the instance (not pristine) and eval's
    // mount guard is satisfied — the Wabbajack model, automatic per profile.
    concierge_games::register();
    let dir = std::env::temp_dir().join(format!("concierge-inst-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        "[game]\nkind = \"bg3\"\npristine = \"/tmp/bg3\"\nversion = \"4.7\"\n\
         [game.paths]\nprofile_mods = \"/tmp/larian/Mods\"\n\
         modsettings = \"/tmp/larian/PlayerProfiles/Public/modsettings.lsx\"\n",
    )
    .unwrap();
    let m = Manifest::load(&dir).unwrap();
    let inst = m.game.instance.clone().expect("instance defaulted");
    assert!(
        inst.ends_with("state/instance"),
        "defaults under the profile: {inst:?}"
    );
    // and it now evals (without a default instance the mount guard would refuse it)
    assert!(
        eval(&m).is_ok(),
        "mount game evals once isolated: {:?}",
        eval(&m).err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn crossover_game_defaults_its_instance_into_the_bottle() {
    // A CrossOver/Windows game with NO explicit instance must NOT default to a
    // repo path — that detects as `native` and a Windows .exe can't launch
    // natively (the "surival plus" launch bug). It defaults into the pristine's
    // bottle so the profile is launchable out of the box.
    let base = std::env::temp_dir().join(format!("concierge-cx-{}", std::process::id()));
    let profile = base.join("games/fallout4/profiles/my pack");
    std::fs::create_dir_all(&profile).unwrap();
    let pristine = "/Users/x/Library/Application Support/CrossOver/Bottles/Games/drive_c/Program Files (x86)/Steam/steamapps/common/Fallout 4";
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{pristine}\"\nversion = \"1\"\n\
             [game.paths]\nplugins_txt = \"/tmp/p.txt\"\nmy_games = \"/tmp/mg\"\n"
        ),
    )
    .unwrap();
    let m = Manifest::load(&profile).unwrap();
    let inst = m
        .game
        .instance
        .clone()
        .expect("instance defaulted")
        .display()
        .to_string();
    // In the SAME bottle, under concierge/<profile>, whitespace sanitized.
    assert!(
        inst == "/Users/x/Library/Application Support/CrossOver/Bottles/Games/drive_c/concierge/my-pack",
        "CrossOver instance defaults into the bottle, got: {inst}"
    );
    // Runtime resolves to CrossOver (launchable), not native.
    concierge_games::register();
    let plan = eval(&m).unwrap();
    assert!(
        matches!(
            plan.game.runtime,
            concierge::runtime::Runtime::CrossOver { .. }
        ),
        "runtime is CrossOver, got {:?}",
        plan.game.runtime
    );

    // An explicit instance is still left untouched.
    std::fs::write(
        profile.join("manifest.toml"),
        format!(
            "[game]\nkind = \"fallout4\"\npristine = \"{pristine}\"\n\
             instance = \"/custom/run/dir\"\nversion = \"1\"\n\
             [game.paths]\nplugins_txt = \"/tmp/p.txt\"\nmy_games = \"/tmp/mg\"\n"
        ),
    )
    .unwrap();
    let m = Manifest::load(&profile).unwrap();
    assert_eq!(
        m.game.instance.unwrap().display().to_string(),
        "/custom/run/dir"
    );
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn bg3_rejects_malformed_plugin_entries() {
    let bad = BG3.replace("3a4b5c6d-0000-1111-2222-333344445555|ImpUI", "no-pipe-here");
    let m = Manifest::parse(&bad).unwrap();
    assert!(eval(&m).unwrap_err().to_string().contains("uuid|Folder"));
}

#[test]
fn bg3_requires_profile_paths() {
    let missing = BG3.replace("profile_mods = \"/tmp/larian/Mods\"\n", "");
    let m = Manifest::parse(&missing).unwrap();
    assert!(eval(&m)
        .unwrap_err()
        .to_string()
        .contains("requires [game.paths] key 'profile_mods'"));
}

#[test]
fn adapters_cover_the_basis() {
    for kind in [
        "fallout4",
        "skyrimse",
        "stardew",
        "kotor2",
        "bg3",
        "rimworld",
        "minecraft",
        "valheim",
    ] {
        let a = adapter_for(kind).unwrap();
        assert_eq!(a.kind(), kind);
        assert!(
            a.install_roots()
                .iter()
                .any(|(name, _)| *name == a.default_install_root()),
            "{kind}: default root must exist"
        );
    }
}

#[test]
fn runtime_detection() {
    assert_eq!(
        detect(Path::new(
            "/Users/x/Library/Application Support/CrossOver/Bottles/Games/drive_c/Program Files (x86)/Steam/steamapps/common/Fallout 4"
        )),
        Runtime::CrossOver {
            bottle: "Games".into()
        }
    );
    assert_eq!(
        detect(Path::new(
            "/Users/x/Library/Application Support/Steam/steamapps/common/Baldurs Gate 3"
        )),
        Runtime::Native
    );
    assert_eq!(
        detect(Path::new("/home/x/.wine/drive_c/Games/Skyrim")),
        Runtime::WinePrefix {
            prefix: "/home/x/.wine".into()
        }
    );
}

#[test]
fn wine_path_mapping() {
    let rt = Runtime::CrossOver {
        bottle: "Games".into(),
    };
    let host = Path::new("/x/Bottles/Games/drive_c/fo4nix/game/f4se_loader.exe");
    assert_eq!(
        game_visible_path(&rt, host).unwrap(),
        "C:\\fo4nix\\game\\f4se_loader.exe"
    );
    assert_eq!(
        game_visible_path(&Runtime::Native, Path::new("/a/b")).unwrap(),
        "/a/b"
    );
}

#[test]
fn sources_resolve_correctly() {
    let m = Manifest::parse(BASE).unwrap();
    let plan = eval(&m).unwrap();
    assert!(matches!(
        plan.mods[0].source,
        Source::Nexus {
            mod_id: 1,
            file_id: 10
        }
    ));
    assert!(matches!(plan.mods[1].source, Source::Url { .. }));
}

#[test]
fn unpinned_detected() {
    let unpinned = BASE.replace("md5 = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"", "md5 = \"\"");
    let m = Manifest::parse(&unpinned).unwrap();
    let plan = eval(&m).unwrap();
    assert!(!plan.fully_pinned());
    assert!(plan.mods[0].md5.is_none());
}

#[test]
fn state_key_round_trips() {
    for (root, rel) in [
        ("data", "textures/foo/bar.dds"),
        ("game", "f4se_loader.exe"),
        ("override", "appearance.2da"),
        ("mods", "SomeMod/manifest.json"),
        ("data", "weird:name/with:colons.txt"),
    ] {
        let k = key(root, rel);
        let (r2, rel2) = parse_key(&k).unwrap();
        assert_eq!(r2, root);
        assert_eq!(rel2, rel);
    }
}

const CUSTOM: &str = r##"
[game]
kind = "custom"
pristine = "/tmp/somegame"
version = "1.0"

[game.paths]
loadorder = "/tmp/somegame/loadorder.txt"

[game.custom]
default_root = "mods"
nexus_domain = "somegame"
launch = ["SomeGame.exe", "SomeGame.app"]
steam_app_id = 999000

[[game.custom.root]]
name = "game"
dir = ""
[[game.custom.root]]
name = "mods"
dir = "mods"

[[game.custom.config]]
path_key = "loadorder"
line_prefix = "enabled:"
template = "# concierge\n{{plugins}}\n"

[[mod]]
name = "coolmod"
version = "1.0"
url = "https://example.com/coolmod.zip"
md5 = "dddddddddddddddddddddddddddddddd"
plugins = ["CoolMod"]
"##;

#[test]
fn custom_kind_services_a_game_with_core_alone() {
    // The architecture proof: no codified adapter, no per-game crate — the
    // manifest's [game.custom] section fully determines the game shape.
    let m = Manifest::parse(CUSTOM).unwrap();
    let plan = eval(&m).unwrap();
    assert_eq!(plan.game.kind, "custom");
    assert_eq!(plan.game.nexus_domain.as_deref(), Some("somegame"));
    assert_eq!(plan.steam_app_id, Some(999_000));
    assert_eq!(plan.launch_candidates, vec!["SomeGame.exe", "SomeGame.app"]);
    assert!(plan.root_targets["mods"].instance_relative);
    assert_eq!(plan.root_targets["mods"].dir, "mods");
    assert_eq!(plan.mods[0].install_root, "mods");
    let cfg = &plan.configs[0];
    assert_eq!(cfg.path, "/tmp/somegame/loadorder.txt");
    assert!(cfg.content.contains("enabled:CoolMod"));
    assert!(cfg.content.starts_with("# concierge"));
    assert!(!plan.config_resets[0].content.contains("CoolMod"));
}

#[test]
fn generic_kind_needs_no_section_at_all() {
    // The last-resort door: kind = "generic" services ANY game with zero
    // assumptions — mods just add/replace files under the game dir, realized
    // as a pure diff (CoW instance) against the pristine. No [game.custom],
    // no paths, no configs, no launch knowledge.
    let m = Manifest::parse(
        r#"
[game]
kind = "generic"
pristine = "/tmp/somegame"
instance = "/tmp/concierge/somegame-instance"
version = "1.0"

[[mod]]
name = "overlay"
version = "1.0"
url = "https://example.com/overlay.zip"
md5 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
"#,
    )
    .unwrap();
    let plan = eval(&m).unwrap();
    assert_eq!(plan.game.kind, "generic");
    let game = &plan.root_targets["game"];
    assert!(game.instance_relative, "overlay lands in the instance");
    assert_eq!(game.dir, "", "the whole game dir is the overlay root");
    assert_eq!(plan.mods[0].install_root, "game");
    assert!(plan.configs.is_empty(), "no activation registry is assumed");
    assert!(plan.launch_candidates.is_empty(), "no launch knowledge");
    assert!(
        plan.needs_instance() && plan.needs_game_clone(),
        "pure diff = CoW clone"
    );
    // A [game.custom] section still wins for users who outgrow generic.
    let refined = Manifest::parse(
        r#"
[game]
kind = "generic"
pristine = "/tmp/somegame"
instance = "/tmp/concierge/somegame-instance"
version = "1.0"

[game.custom]
default_root = "mods"
launch = ["Some.app"]

[[game.custom.root]]
name = "mods"
dir = "mods"

[[mod]]
name = "overlay"
version = "1.0"
url = "https://example.com/overlay.zip"
md5 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
"#,
    )
    .unwrap();
    let plan = eval(&refined).unwrap();
    assert_eq!(plan.root_targets["mods"].dir, "mods");
    assert_eq!(plan.launch_candidates, vec!["Some.app"]);
}

#[test]
fn custom_kind_requires_its_section() {
    let bad = CUSTOM
        .lines()
        .take_while(|l| !l.starts_with("[game.custom]"))
        .collect::<Vec<_>>()
        .join("\n");
    let m = Manifest::parse(&bad).unwrap();
    assert!(eval(&m)
        .unwrap_err()
        .to_string()
        .contains("no codified adapter and no [game.custom]"));
}

#[test]
fn locked_profile_is_a_fact_on_disk() {
    // Locked = manifest read-only (+ immutable flag on macOS). The
    // write_manifest chokepoint refuses politely; a rename-over (how editors
    // save atomically) is physically blocked; unlock restores everything.
    use concierge::manifest_edit::write_manifest;
    use concierge::profiles::{is_locked, set_locked};
    let dir = std::env::temp_dir().join(format!("concierge-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("manifest.toml");
    std::fs::write(&manifest, "a = 1\n").unwrap();

    assert!(!is_locked(&dir));
    set_locked(&dir, true).unwrap();
    assert!(is_locked(&dir));
    let err = write_manifest(&manifest, "a = 2\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("LOCKED"), "polite refusal: {err}");
    if cfg!(target_os = "macos") {
        // The immutable flag blocks even a rename-over.
        let tmp = dir.join("sneaky.toml");
        std::fs::write(&tmp, "a = 3\n").unwrap();
        assert!(
            std::fs::rename(&tmp, &manifest).is_err(),
            "rename-over must fail while locked"
        );
    }
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "a = 1\n");

    set_locked(&dir, false).unwrap();
    assert!(!is_locked(&dir));
    write_manifest(&manifest, "a = 2\n").unwrap();
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "a = 2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profiles_enumerate_and_create() {
    use concierge::profiles::{create_profile, list_profiles};
    let tmp = std::env::temp_dir().join(format!("concierge-prof-{}", std::process::id()));
    let game = tmp.join("games").join("testgame");
    std::fs::create_dir_all(game.join("profiles").join("default")).unwrap();
    std::fs::write(
        game.join("profiles").join("default").join("manifest.toml"),
        "x = 1\n",
    )
    .unwrap();
    assert_eq!(list_profiles(&game).len(), 1);
    // clone creates a second profile sharing... nothing to download here, just
    // proves the API + naming guard
    let dir = create_profile(
        &game,
        "second",
        Some(&game.join("profiles").join("default")),
    )
    .unwrap();
    assert!(dir.join("manifest.toml").exists());
    assert_eq!(list_profiles(&game).len(), 2);
    assert!(create_profile(&game, "bad/name", None).is_err());
    assert!(create_profile(&game, "second", None).is_err(), "no dup");
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn each_game_speaks_its_own_vocabulary() {
    // Per-game lexicons, resolved through the assembled registry (adapter_for
    // wrapper registers it).
    assert_eq!(adapter_for("bg3").unwrap().lexicon().plugins, "paks");
    assert_eq!(adapter_for("bg3").unwrap().lexicon().order, "load order");
    assert_eq!(
        adapter_for("rimworld").unwrap().lexicon().order,
        "load order"
    );
    assert_eq!(
        adapter_for("kotor2").unwrap().lexicon().order,
        "install order"
    );
    assert_eq!(adapter_for("stardew").unwrap().lexicon().plugins, "mods");
    assert_eq!(
        adapter_for("minecraft").unwrap().lexicon().order,
        "mod list"
    );
    assert_eq!(adapter_for("valheim").unwrap().lexicon().plugins, "mods");
}

#[test]
fn new_profile_manifest_parses_and_evals() {
    // Moved from the profiles lib test: it needs a real adapter (required-path
    // stubbing + eval), so it lives here where core links a single copy and the
    // registered family adapters are visible. The `eval` wrapper registers them.
    use concierge::profiles::create_profile;
    concierge_games::register(); // stubbing needs the fallout4 adapter resolvable
    let base = std::env::temp_dir().join(format!("concierge-newprof-{}", std::process::id()));
    let game_dir = base.join("fallout4");
    std::fs::create_dir_all(&game_dir).unwrap();
    let dir = create_profile(&game_dir, "fresh", None).unwrap();
    let text = std::fs::read_to_string(dir.join("manifest.toml")).unwrap();
    let m = Manifest::parse(&text).unwrap();
    assert_eq!(m.game.kind, "fallout4", "kind inferred from game dir");
    assert!(m.mods.is_empty());
    assert!(
        m.game.paths.contains_key("plugins_txt"),
        "required path stubbed"
    );
    assert!(
        eval(&m).is_ok(),
        "fresh profile evals: {:?}",
        eval(&m).err()
    );
    std::fs::remove_dir_all(&base).ok();
}
