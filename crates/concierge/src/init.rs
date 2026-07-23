//! `concierge-cli init` — scaffold an agent-ready profile folder.
//!
//! Lays down the config (TOML or the Nix front-end), a per-profile **sandbox**
//! layout (its own My Games / Saves / plugins.txt — nothing shared), a
//! `CLAUDE.md` documenting the tool surface + the reconciliation ladder + the
//! AI oracle discipline, and `.claude/skills/` wrapping the CLI. Drop Claude
//! Code (or any agent) into the folder and it can drive the whole stack.

use std::path::Path;

use concierge::error::{IoCtx as _, Result};

/// Write `path` only if absent (never clobber a user's work); returns whether it
/// was created.
fn put(path: &Path, body: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    std::fs::write(path, body).ctx(path)?;
    Ok(true)
}

pub fn init(dir: &Path, game: &str, nix: bool) -> Result<()> {
    std::fs::create_dir_all(dir).ctx(dir)?;
    // per-profile sandbox: its own My Games (INIs + Saves) + load order.
    for sub in [
        "sandbox/MyGames",
        "sandbox/Saves",
        "sandbox/AppData",
        "store",
        "state",
    ] {
        std::fs::create_dir_all(dir.join(sub)).ctx(dir)?;
    }

    let created_config = if nix {
        put(&dir.join("modpack.nix"), &nix_template(game))?
    } else {
        put(&dir.join("manifest.toml"), &toml_template(game))?
    };
    // The shared per-profile agent surface: CLAUDE.md guide,
    // slash-commands, permissions allowlist — one source of truth in core.
    concierge::provision::provision_profile(dir, game)?;
    put(
        &dir.join(".gitignore"),
        "/store/\n/state/\n/sandbox/Saves/\n",
    )?;

    println!(
        "  init      scaffolded {} profile in {}",
        game,
        dir.display()
    );
    println!(
        "  config    {}",
        if nix {
            "modpack.nix (Nix tier)"
        } else {
            "manifest.toml (TOML tier)"
        }
    );
    println!("  sandbox   sandbox/MyGames + sandbox/Saves (isolated per profile)");
    println!("  agent     CLAUDE.md + .claude/skills/ (curate / reconcile / author-pipeline)");
    if !created_config {
        println!("  note      config already existed; left it untouched");
    }
    println!("  next      edit the config's pristine/instance paths, then: concierge-cli eval");
    Ok(())
}

fn toml_template(game: &str) -> String {
    format!(
        r#"# Concierge modpack — {game}. Edit the paths, then `concierge-cli eval`.
[game]
kind = "{game}"
# PRISTINE is never written. INSTANCE is the CoW clone the game runs from.
pristine = "/absolute/path/to/vanilla/{game}"
instance = "/absolute/path/to/{game}-instance"
version = "1.0"
# Per-profile sandbox: point these at THIS folder's sandbox/ so INIs, saves,
# and load order are isolated (nothing shared with other profiles).
plugins_txt = "./sandbox/AppData/plugins.txt"
my_games = "./sandbox/MyGames"

# To fully isolate INIs + saves like an MO2 profile, set the game's REAL My
# Games path here; `concierge-cli sandbox` (and launch) will symlink it to this
# profile's sandbox/MyGames (parking + restoring your real Documents):
# [game.paths]
# canonical_my_games = "/Users/you/Documents/My Games/Fallout4"
# canonical_appdata  = "/…/drive_c/users/you/AppData/Local/Fallout4"  # plugins.txt / load order

# [[mod]] entries — a Nexus mod, or a pipeline (git/http) for off-Nexus mods.
# [[mod]]
# name = "example"
# version = "1"
# nexus_mod_id = 0
# nexus_file_id = 0
"#
    )
}

fn nix_template(game: &str) -> String {
    format!(
        r#"# Concierge modpack — {game}. Evaluated by `nix eval` into the same plan
# the TOML tier produces (differential-checked). Compose mods as a dendritic
# tree; the interpreter enforces the metamorphic laws.
let
  # adjust to where you cloned concierge (or vendor nix/ into this repo)
  cl = import ./nix/lib.nix;
  d = import ./nix/dendritic.nix;
in
d.mkDendritic {{
  game = {{
    kind = "{game}";
    pristine = "/absolute/path/to/vanilla/{game}";
    instance = "/absolute/path/to/{game}-instance";
    version = "1.0";
    paths = {{
      plugins_txt = "./sandbox/AppData/plugins.txt";
      my_games = "./sandbox/MyGames";
    }};
  }};
  modules = [
    # each module is a single-concern leaf, e.g.:
    # {{ mods = [ (cl.mkMod {{ name = "example"; version = "1";
    #     source = cl.github "owner/repo@v1"; }}) ]; }}
  ];
}}
"#
    )
}
