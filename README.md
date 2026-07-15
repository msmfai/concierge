# Concierge 🧤

**Your entire mod setup in one file — deployed, verified, and launched without
ever touching your game install.**

Concierge is a declarative mod manager. You describe the modded game you want
in a single `manifest.toml`; Concierge makes it real — downloads hash-pinned
archives, resolves installers, deploys into a disposable copy of the game, and
launches it. Delete everything and rebuild the identical setup from the
manifest; carry the file to another machine and rebuild it there.

- **Your install is sacred.** Mods deploy into a copy-on-write *instance*; the
  original game directory is never written, and `doctor` proves it.
- **One file is the whole truth.** Mods, versions, content hashes, installer
  choices, load order — reviewable, diffable, versionable.
- **Reproducible by construction.** Every archive is content-addressed; every
  deploy is a pure function of the manifest.
- **Cross-game.** One engine, ~45 games: the Bethesda family, Baldur's Gate 3,
  KOTOR, RimWorld, Stardew, Minecraft, Valheim, Cyberpunk 2077, Elden Ring,
  and more — plus a generic mode for anything else.
- **Agent-ready.** Profiles ship an AI-agent command guide and an OS-sandboxed
  shell whose write-boundary is derived from the plan — an agent (or you) can
  work on the modlist with the game install denied at the OS level.

**Status: early release.** Fallout 4 is the deepest, end-to-end path
(including macOS via CrossOver); other adapters are functional at varying
depth. Expect rough edges — see [RELEASE_NOTES](RELEASE_NOTES.md).

## 30-second quickstart

```sh
cp -r examples/fallout4-profile my-pack
$EDITOR my-pack/manifest.toml                # point it at your install
export CONCIERGE_REPO=$PWD/my-pack

concierge preview                            # see what WOULD deploy — nothing moves
concierge realize --sort                     # fetch → pin → build → deploy → sort
concierge doctor                             # health report: all green?
concierge launch                             # play
```

`concierge deactivate` reverses a launch; `concierge undeploy` removes every
file Concierge placed. Your original install was never touched either way.

## What it does

- **FOMOD installers, as data.** Instead of clicking through an installer
  wizard, record the choices: `[mod.fomod] select = ["..."]`. Only the
  selected files deploy; `concierge fomod <mod>` lists a mod's options, and a
  mistyped choice fails loudly instead of silently installing nothing.
- **Load order, solved.** Native LOOT-masterlist sorting; `realize --sort` is
  the whole converge in one command.
- **Safety rails everywhere.** `preview` shows the exact files and activations
  before anything deploys. Game-specific invariant lints (missing masters,
  plugin limits, dependency cycles) refuse a deploy that would crash.
  `doctor` gives one pass/fail report: pins, lints, inert plugins, drift,
  pristine safety. `launch --check` reads the script-extender log and tells
  you what failed to load and why.
- **Nexus integration.** Automatic downloads with a Premium API key, a guided
  click-and-ingest flow without one, a local searchable catalog of the whole
  Nexus index for your game, and `audit` to verify every declared mod id.
- **A GUI too.** Mod list, catalog browser, preview/apply, generations, and an
  embedded terminal that runs *your* agent inside the sandbox.

## Supported games

One `GameAdapter` per game supplies the install layout, activation registry,
lints, and vocabulary; everything downstream is game-agnostic.

- **Bethesda family** — Fallout 4 · Skyrim SE/LE · Fallout 3 · New Vegas ·
  Oblivion · Starfield: plugins.txt load order, FOMOD, LOOT sorting,
  master/plugin-limit lints.
- **Baldur's Gate 3** (pak + `modsettings.lsx` registry) · **KOTOR 1/2**
  (Override installs with a native TSLPatcher-style 2DA/TLK merge) ·
  **RimWorld** · **Stardew Valley** · **Minecraft** · **Valheim**.
- **~30 loose-file games** (Cyberpunk 2077, Witcher 3, Elden Ring, The Sims 4,
  …) via a data-driven adapter, plus four merge-tool games.
- **Anything else** via `kind = "generic"` (pure file overlay) or a
  data-driven `[game.custom]` block — per-game code is an accelerator, not a
  requirement.

## How it works

`manifest.toml` → **`eval`** → a pure, hashed *plan* (what would deploy and
activate) → **`realize`** → the world made to match: archives fetched into a
content-addressed store, extracted into immutable build trees, the pristine
install cloned copy-on-write into an *instance*, mods hardlinked in, the
game's activation registry rendered, invariant lints enforced. Profiles share
the store, so a mod used by two modlists downloads once. The full command set:

| command | what it does |
|---|---|
| `eval` / `preview` | pure plan / dry-run of files + activations |
| `realize [--fresh] [--sort]` | make the instance match the plan |
| `doctor` / `check [--vanilla]` | health report / drift detection |
| `plugins` / `sort` / `conflicts` | activation truth, LOOT order, conflict reports |
| `fomod <mod>` | installer options + current selection |
| `launch [--check]` / `deactivate` | play / cleanly reverse |
| `undeploy` | remove everything Concierge placed |
| `audit` / `db sync` | verify mod ids / build the local catalog |

## Install

With Nix (all helper tools included):

```sh
nix run github:msmfai/concierge -- --help
nix develop                       # hacking devshell; then: cargo build
```

With plain cargo:

```sh
cargo build --release             # binary at target/release/concierge
cargo run -p concierge-gui        # the GUI
```

Archive extraction uses `bsdtar` — preinstalled on macOS and Windows 10+; on
Linux install `libarchive-tools`. For automatic Nexus downloads put a Premium
API key in `~/.config/fo4nix/nexus-api-key` (or `NEXUS_API_KEY`).

## Contributing & license

Issues, fixes, and new game adapters are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md). Licensed GPL-3.0-or-later — see
[LICENSE](LICENSE).
