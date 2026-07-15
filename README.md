<h1 align="center">Concierge 🧤</h1>

<p align="center"><i>Describe the game you want. Concierge builds it.</i></p>

<p align="center">
<a href="https://github.com/msmfai/concierge/actions/workflows/ci.yml"><img src="https://github.com/msmfai/concierge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://github.com/msmfai/concierge/releases/latest"><img src="https://img.shields.io/github/v/release/msmfai/concierge" alt="Latest release"></a>
<a href="https://github.com/msmfai/concierge/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="License"></a>
</p>

## Introduction

Concierge is a mod manager with a white-glove twist: tell an AI assistant the
playthrough you want — *"survival mode, but less grindy, where building
settlements actually matters"* — and it puts the modpack together for you. It
searches the mods available for your game, picks a set that fits, answers the
installer questions, fixes the load order, and health-checks everything before
you hit play.

You stay in control the whole way: every choice ends up in one small pack file
you can read, edit, and keep — and your game's own files are never touched.
Modding something this personal used to take a weekend of forum threads; now
it's a conversation.

## Features

- **Your personal mod concierge** — bring the AI assistant you already use
  (Claude Code and friends work out of the box). Every pack comes pre-loaded
  with the instructions the assistant needs, and it works inside a sandbox
  that physically cannot write to your game install or anywhere else on your
  machine it wasn't given.

- **Your game files stay untouched** — mods go into a separate copy of the
  game. One command proves your original install is still pristine; one
  command removes every trace of Concierge.

- **A pack you can keep and share** — your whole setup lives in one small
  file. Back it up, send it to a friend, or rebuild the exact same game on a
  new machine years from now.

- **Multi-game support** — one app for around 45 games, including
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
  [Valheim](https://www.nexusmods.com/valheim) and KOTOR.

- **No installer wizard fatigue** — installer questions get answered once and
  remembered forever; reinstalling never means clicking through the same
  screens again.

- **Crashes caught before you launch** — automatic load-order sorting and
  health checks that spot missing requirements and broken combinations while
  they're still easy to fix.

- **Nexus Mods built in** — downloads happen automatically with a Nexus
  Premium account, or through a guided click-along without one; browse and
  search every mod for your game right from the app.

- **Profiles** — keep separate packs per game (a survival run, a story run, a
  chaos run) and switch freely. Anything two packs share is only downloaded
  once.

- **App and command line** — a friendly app with a mod browser, previews, and
  one-click rollback to any earlier state; and every single thing it does is
  also scriptable.

## Getting Started

Grab the latest release for Linux, Windows, or macOS from the
[releases page](https://github.com/msmfai/concierge/releases/latest), or
build from source with `cargo build --release`.

Start from the example pack and point it at your game:

```sh
cp -r examples/fallout4-profile my-pack
$EDITOR my-pack/manifest.toml        # set where your game lives
export CONCIERGE_REPO=$PWD/my-pack
```

Then either hand the pack to your assistant and describe the game you want:

```sh
concierge shell --agent claude       # sandboxed; it takes it from here
```

…or drive it yourself:

```sh
concierge preview                    # see what would happen — nothing moves
concierge realize --sort             # download, install, sort
concierge doctor                     # health check
concierge launch                     # play
```

Concierge is an early release: Fallout 4 is the most play-tested game today,
and the rest share the same engine at varying levels of polish. More detail
lives in the [docs](docs/) and the [release notes](RELEASE_NOTES.md).

## Resources

- [Download Concierge](https://github.com/msmfai/concierge/releases/latest) — prebuilt binaries for Linux, Windows, and macOS
- [Documentation](docs/) — guides and reference
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
