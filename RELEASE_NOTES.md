# Concierge v0.1.0

First public release.

Concierge is a mod manager with a white-glove twist: describe the playthrough
you want to an AI assistant and it builds the modpack for you — finds the
mods, answers the installer questions, sorts the load order, and
health-checks the result. Everything it decides lands in one small pack file
you can read, keep, and share, and your game's own files are never touched.

## Highlights

- **Your personal mod concierge** — bring the AI assistant you already use;
  every pack ships with the instructions it needs, and it runs in a sandbox
  that physically can't write outside the pack and the game copy it manages.
- **Your game files stay untouched** — mods go into a separate copy of the
  game; one command proves your install is pristine, one command removes
  every trace.
- **A pack you can keep** — back it up, share it, or rebuild the identical
  game on another machine.
- **Around 45 games** — Fallout 4, Skyrim, Baldur's Gate 3, Starfield,
  Cyberpunk 2077, Elden Ring, Stardew Valley, RimWorld, Witcher 3, The Sims 4,
  Valheim, KOTOR and more, plus a generic mode for games without dedicated
  support.
- **No installer wizard fatigue** — installer choices are answered once and
  remembered forever.
- **Crashes caught early** — automatic load-order sorting plus health checks
  for missing requirements and broken combinations, before you launch.
- **Nexus Mods built in** — automatic downloads with a Premium account or a
  guided flow without one, and a searchable local catalog of every mod for
  your game.
- **App and command line** — a friendly app with a mod browser, previews, and
  rollback; everything also scriptable.

## Downloads

Prebuilt `concierge` binaries are attached below:

| platform | file |
|---|---|
| Linux x86_64 | `concierge-v0.1.0-x86_64-linux.tar.gz` |
| Windows x86_64 | `concierge-v0.1.0-x86_64-windows.zip` |
| macOS Apple Silicon | `concierge-v0.1.0-aarch64-macos.tar.gz` |
| macOS Intel | `concierge-v0.1.0-x86_64-macos.tar.gz` |

The app portion is source-built for now: `cargo run -p concierge-gui`.

## Maturity

This is an early release. Fallout 4 is the most play-tested game (including
on a Mac via CrossOver); the others share the same engine but have seen less
real-world use. Expect rough edges, and please file issues.
