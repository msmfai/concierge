# Concierge v0.1.0

First public release.

Concierge is a **declarative mod manager**: your entire modded game lives in
one `manifest.toml` — every mod, version, content hash, installer choice, and
the load order. Concierge turns that file into a running game and can rebuild
the identical setup from scratch, on any machine, at any time.

## Highlights

- **Your game install is never touched.** Mods deploy into a disposable
  copy-on-write *instance*; the original directory stays pristine, verifiably.
- **One file is the whole truth.** Review it, diff it, version it, share it.
  `preview` shows exactly what would deploy before anything moves.
- **~45 games, one engine.** Bethesda titles, Baldur's Gate 3, KOTOR 1/2,
  RimWorld, Stardew Valley, Minecraft, Valheim, Cyberpunk 2077, Elden Ring,
  The Sims 4 and more — plus `generic`/`custom` modes that cover games with
  no adapter at all.
- **Installers as data.** FOMOD choices are recorded in the manifest
  (`[mod.fomod] select = [...]`) and replayed exactly — no wizard clicking,
  no mystery files.
- **Safety rails.** Per-game invariant lints refuse deploys that would crash;
  `doctor` gives one pass/fail health report; `launch --check` reads the
  game's own logs to tell you what loaded and what didn't; everything is
  cleanly reversible (`deactivate`, `undeploy`).
- **Nexus integration.** Automatic downloads with a Premium key or a guided
  flow without one; a locally-synced, searchable catalog; `audit` verifies
  every declared mod id against it.
- **Agent-ready.** Profiles ship a command guide and slash-commands for AI
  agents, and `concierge shell` runs any agent inside an OS sandbox whose
  write-boundary is derived from the plan.
- **CLI + GUI.** Everything scriptable; the egui app adds a catalog browser,
  preview/apply, generations, and an embedded sandboxed terminal.

## Downloads

Prebuilt `concierge` CLI binaries are attached below:

| platform | file |
|---|---|
| Linux x86_64 | `concierge-v0.1.0-x86_64-linux.tar.gz` |
| Windows x86_64 | `concierge-v0.1.0-x86_64-windows.zip` |
| macOS Apple Silicon | `concierge-v0.1.0-aarch64-macos.tar.gz` |
| macOS Intel | `concierge-v0.1.0-x86_64-macos.tar.gz` |

Runtime needs `bsdtar` (preinstalled on macOS and Windows 10+;
`libarchive-tools` on Linux). The GUI is source-built for now:
`cargo run -p concierge-gui`.

## Maturity

This is an early release. The most play-tested path is Fallout 4 (including
macOS via CrossOver); the other adapters share the same engine but have seen
less real-world use, and launch flows are strongest on macOS. Expect rough
edges, and please file issues.
