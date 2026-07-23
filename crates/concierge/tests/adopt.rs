//! `concierge-cli adopt` — the automation surface for setting up games concierge
//! doesn't know (the GUI wizard's CLI twin, usable by agents). Hermetic: runs
//! the real binary against a temp workspace + fake install; no real game, no
//! network, no gate env var.

#![allow(
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn ws() -> PathBuf {
    let ws = std::env::temp_dir().join(format!("concierge-adopt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(ws.join("games")).unwrap();
    std::fs::write(ws.join(".concierge-workspace"), "").unwrap();
    // A fake install so eval's pristine warnings stay quiet.
    std::fs::create_dir_all(ws.join("fakegame")).unwrap();
    std::fs::write(ws.join("fakegame/data.bin"), "payload").unwrap();
    ws
}

fn adopt(ws: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
        .arg("adopt")
        .args(args)
        .env("CONCIERGE_REPO", ws)
        .output()
        .expect("run concierge adopt");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn adopt_scaffolds_generic_and_custom_games() {
    let ws = ws();
    let pristine = ws.join("fakegame").display().to_string();

    // No modding knowledge → generic: pure file overlay, evals immediately.
    let (ok, out) = adopt(&ws, &["mygame", "--pristine", &pristine]);
    assert!(ok, "generic adopt failed:\n{out}");
    assert!(
        out.contains("kind generic"),
        "reports the generic kind:\n{out}"
    );
    assert!(out.contains("hash"), "evals to a plan hash:\n{out}");
    let manifest =
        std::fs::read_to_string(ws.join("games/mygame/profiles/default/manifest.toml")).unwrap();
    assert!(manifest.contains("kind = \"generic\""));
    assert!(
        !manifest.contains("[game.custom]"),
        "no assumptions were added"
    );

    // Adopting the same name twice is refused — edit, don't clobber.
    let (ok, out) = adopt(&ws, &["mygame", "--pristine", &pristine]);
    assert!(
        !ok && out.contains("already exists"),
        "duplicate must refuse:\n{out}"
    );

    // Users can add as many generics as they want — a second one coexists.
    let (ok, out) = adopt(&ws, &["othergame", "--pristine", &pristine]);
    assert!(ok, "second generic adopt failed:\n{out}");

    // Described modding knowledge → data-driven [game.custom].
    let external = ws.join("external-mods").display().to_string();
    let (ok, out) = adopt(
        &ws,
        &[
            "described",
            "--pristine",
            &pristine,
            "--mods-path",
            &external,
            "--launch",
            "Described.app",
            "--nexus-domain",
            "described",
        ],
    );
    assert!(ok, "custom adopt failed:\n{out}");
    assert!(
        out.contains("kind described"),
        "custom keeps the game's name:\n{out}"
    );
    let manifest =
        std::fs::read_to_string(ws.join("games/described/profiles/default/manifest.toml")).unwrap();
    assert!(manifest.contains("[game.custom]"));
    assert!(
        manifest.contains("path_key = \"mods\""),
        "external path is a mount"
    );
    assert!(manifest.contains("launch = [\"Described.app\"]"));

    // Adopted profiles are agent-ready from birth: guide, commands,
    // settings — and the guide only references real subcommands.
    let profile = ws.join("games/mygame/profiles/default");
    assert!(profile.join("CLAUDE.md").exists(), "guide provisioned");
    assert!(
        profile.join(".claude/settings.json").exists(),
        "allowlist provisioned"
    );
    assert!(
        profile.join(".claude/commands/audit-ids.md").exists(),
        "commands provisioned"
    );
    let help = Command::new(env!("CARGO_BIN_EXE_concierge-cli"))
        .arg("--help")
        .output()
        .expect("run --help");
    let help = String::from_utf8_lossy(&help.stdout).to_lowercase();
    let guide = std::fs::read_to_string(profile.join("CLAUDE.md")).unwrap();
    for cmd in [
        "eval",
        "fetch",
        "realize",
        "check",
        "audit",
        "sort",
        "conflicts",
        "reconcile",
        "inventory",
        "lock",
        "shell",
        "db",
        "ai",
        "nexus",
    ] {
        assert!(
            guide.contains(&format!("concierge-cli {cmd}")),
            "guide teaches {cmd}"
        );
        assert!(
            help.contains(cmd),
            "guide references unreal subcommand {cmd}"
        );
    }
    assert!(
        guide.contains("unlock") && help.contains("unlock"),
        "unlock is taught and real"
    );

    // Guardrails: relative pristine and hostile names are refused.
    let (ok, out) = adopt(&ws, &["badpath", "--pristine", "relative/path"]);
    assert!(
        !ok && out.contains("absolute"),
        "relative pristine must refuse:\n{out}"
    );
    let (ok, out) = adopt(&ws, &["../escape", "--pristine", &pristine]);
    assert!(!ok, "path-escaping name must refuse:\n{out}");

    let _ = std::fs::remove_dir_all(&ws);
}
