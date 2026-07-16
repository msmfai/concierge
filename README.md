# Concierge

> [!WARNING]
> **Alpha release.** This is a tool I made for myself I thought others may
> benefit from. Please note docs are AI generated for now.

<a href="https://github.com/msmfai/concierge/actions/workflows/ci.yml"><img src="https://github.com/msmfai/concierge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://github.com/msmfai/concierge/releases/latest"><img src="https://img.shields.io/github/v/release/msmfai/concierge" alt="Latest release"></a>
<a href="https://github.com/msmfai/concierge/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="License"></a>

![Concierge managing a Cyberpunk 2077 mod pack: on the left, the pack (Cyber Engine Tweaks, redscript, RED4ext, ArchiveXL) with toggles and Download / Apply / Play; on the right, Claude Code running in the profile sandbox — it ran the pack's checks and recommends adding TweakXL and Codeware, catalog-verified.](docs/hero.png)

Concierge is a mod manager. Your mod list lives in a text file; Concierge
downloads the mods, verifies them, installs them into a separate copy of the
game, sorts the load order, and launches. The original game install is never
modified.

It is built to be operated by an AI assistant as well as by hand: each pack
carries the instructions an assistant needs, and `concierge shell` runs one in
a sandbox that can only write where the pack allows. You can tell an assistant
what kind of playthrough you want and let it assemble and maintain the pack;
everything it does ends up in the same text file you can inspect and edit.
The command line is the primary interface; there is also a GUI.

## What it does

- Works on about 45 games — the Bethesda titles (Fallout 4, Skyrim,
  Starfield, …), Baldur's Gate 3, KOTOR 1/2, RimWorld, Stardew Valley,
  Minecraft, Valheim, Cyberpunk 2077, Elden Ring, The Sims 4, and others —
  plus a generic mode for unsupported games. Fallout 4 is the most tested;
  the rest have seen less use.
- Downloads from Nexus Mods: automatic with a Premium API key, or a guided
  manual flow without one. Every file is checksum-verified. A local catalog
  of your game's mods is searchable from the CLI and GUI.
- Stores FOMOD installer choices in the pack file and replays them on every
  install, instead of re-running the installer wizard.
- Sorts the load order (LOOT rules) and runs health checks: missing
  dependencies, plugin limits, files that changed on disk, whether your
  original install is still untouched. `preview` shows what an install would
  do before it does it.
- Keeps multiple packs per game; downloads are shared between them. Any
  earlier state can be rolled back to, and `undeploy` removes everything
  Concierge placed.
- On macOS, launches Windows games through CrossOver, including
  script-extender setups.

## Getting started

Download a build from the
[releases page](https://github.com/msmfai/concierge/releases/latest) — each
archive contains both the `concierge` command-line tool and the `concierge-gui`
app for your platform — or build from source with `cargo build --release`.

**The app** sets itself up on first run: launch `concierge-gui`, use the
**“+ add game”** menu to pick your game, then create a profile — no paths or
config files to hand-edit to get started.

**The command line** starts from an example profile:

```sh
cp -r examples/fallout4-profile my-pack
$EDITOR my-pack/manifest.toml        # set where your game lives
export CONCIERGE_REPO=$PWD/my-pack

concierge preview                    # show what would be installed
concierge realize --sort             # download, install, sort
concierge doctor                     # health check
concierge launch                     # run the game
```

To let an assistant do the work instead:

```sh
concierge shell --agent claude       # sandboxed agent session in the pack
```

Runtime dependency: `bsdtar` (preinstalled on macOS and Windows 10+;
`libarchive-tools` on Linux). More detail in [docs/](docs/) and the
[release notes](RELEASE_NOTES.md).

## Contributing

Bug reports and feature requests: [issues](https://github.com/msmfai/concierge/issues).
Code contributions: see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[GPL-3.0](LICENSE).
