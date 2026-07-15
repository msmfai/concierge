<h1 align="center">Concierge 🧤</h1>

<p align="center"><i>Your modded game, in one file.</i></p>

<p align="center">
<a href="https://github.com/msmfai/concierge/actions/workflows/ci.yml"><img src="https://github.com/msmfai/concierge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://github.com/msmfai/concierge/releases/latest"><img src="https://img.shields.io/github/v/release/msmfai/concierge" alt="Latest release"></a>
<a href="https://github.com/msmfai/concierge/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="License"></a>
</p>

## Introduction

Concierge is a declarative mod manager. Your entire setup — every mod,
version, installer choice, and the load order — lives in a single
`manifest.toml`, and Concierge turns it into a running modded game.

Our goal is a stable modded game you can trust: your original game files are
never touched, every download is verified, and the exact same setup can be
rebuilt from scratch on any machine. Spend less time managing mods and more
time playing your games.

## Features

- **Multi-game support** — one engine for around 45 games, including
  [Fallout 4](https://www.nexusmods.com/fallout4),
  [Skyrim](https://www.nexusmods.com/skyrimspecialedition),
  [Baldur's Gate 3](https://www.nexusmods.com/baldursgate3/),
  [Starfield](https://www.nexusmods.com/starfield/),
  [Cyberpunk 2077](https://www.nexusmods.com/cyberpunk2077/),
  [Elden Ring](https://www.nexusmods.com/eldenring),
  [Stardew Valley](https://www.nexusmods.com/stardewvalley/),
  [RimWorld](https://www.nexusmods.com/rimworld),
  [Witcher 3](https://www.nexusmods.com/witcher3),
  [The Sims 4](https://www.nexusmods.com/thesims4),
  [Valheim](https://www.nexusmods.com/valheim) and KOTOR — plus a generic
  mode for games without dedicated support.

- **Your game files stay untouched** — mods deploy into a disposable copy of
  the game, never into your install. One command verifies it; one command
  removes everything Concierge ever placed.

- **Setups you can share and rebuild** — because the whole setup is one file,
  you can back it up, put it in git, send it to a friend, or rebuild it on a
  new machine and get the identical result.

- **Installer choices as data** — FOMOD installer options are recorded in the
  manifest and replayed exactly, so reinstalling never means clicking through
  wizards again.

- **Automatic load order and health checks** — built-in LOOT-based sorting,
  and safety checks that catch missing dependencies, plugin-limit problems,
  and broken setups *before* the game crashes.

- **Nexus Mods integration** — automatic downloads with a Premium API key or
  a guided flow without one, plus a locally searchable catalog of every mod
  for your game.

- **Profiles** — keep independent modlists per game and switch between them;
  shared downloads mean a mod two profiles use is only fetched once.

- **Built for automation** — everything works from the command line, and
  profiles are AI-agent-ready out of the box, with a sandbox that keeps any
  agent away from your game install at the operating-system level.

- **A GUI too** — mod list, mod browser, preview and apply, and rollback to
  any previous state.

## Getting Started

Download the latest release for Linux, Windows, or macOS from the
[releases page](https://github.com/msmfai/concierge/releases/latest), or
build from source with `cargo build --release` (or the Nix flake).

Then point a profile at your game and go:

```sh
cp -r examples/fallout4-profile my-pack
$EDITOR my-pack/manifest.toml        # set your game paths
export CONCIERGE_REPO=$PWD/my-pack

concierge preview                    # see what would happen — nothing moves
concierge realize --sort             # download, install, sort the load order
concierge doctor                     # health check
concierge launch                     # play
```

Concierge is an early release: Fallout 4 is the most play-tested path today,
and the other games share the same engine at varying levels of polish. More
detail lives in the [docs](docs/) and the
[release notes](RELEASE_NOTES.md).

## Resources

- [Download Concierge](https://github.com/msmfai/concierge/releases/latest) — prebuilt binaries for Linux, Windows, and macOS
- [Documentation](docs/) — schemas, coverage, and design notes
- [Issues](https://github.com/msmfai/concierge/issues) — bug reports and feature requests

## Contributing

Concierge is open source and we'd love to have you involved — whether that's
fixing bugs, adding support for a new game, improving documentation, or just
spreading the word.

- **Bug report** — if something breaks or surprises you, please
  [open an issue](https://github.com/msmfai/concierge/issues/new).
- **Feature request** — if there's a capability you're missing, please
  [tell us about it](https://github.com/msmfai/concierge/issues/new).
- **Code** — see [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test,
  and submit changes.

## License

This project is licensed under the [GPL-3.0](LICENSE) license.
