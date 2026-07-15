# Concierge v0.1.0

First public release. Concierge is a declarative, cross-game mod manager: the
whole setup lives in one `manifest.toml`, deploys into a copy-on-write
instance, and the original game install is never written.

## What works today

**Fallout 4, end to end** — the most exercised path:

- Nexus downloads: automatic with a Premium API key, or a guided
  click-and-ingest flow without one; every archive md5-pinned in the manifest.
- FOMOD installers, declaratively: `[mod.fomod] select = [...]` records the
  installer choices; only the selected files deploy. `concierge fomod <mod>`
  lists a mod's options; a typo'd selection fails loudly.
- Load order: native LOOT-masterlist sorting (`sort --apply`, or
  `realize --sort` for the full converge in one command).
- Safety rails: invariant lints (missing masters, plugin limits, master-graph
  cycles) refuse a deploy that would crash; `doctor` gives one pass/fail
  health report; `preview` shows exactly what would deploy before anything
  moves; `check --vanilla` proves the pristine install is untouched.
- Launch on macOS via CrossOver: Steam-in-bottle launch with the script
  extender, `launch --check` for log-based launch health, `deactivate` to
  cleanly reverse everything.

**The rest of the adapter fleet**, at varying depth:

- Bethesda family (Skyrim SE/LE, Fallout 3/NV, Oblivion, Starfield): the same
  shared shape as Fallout 4 — deploy, plugins.txt, lints, sorting — but far
  less play-tested.
- Baldur's Gate 3 (pak + modsettings.lsx registry, auto-discovery, lints),
  KOTOR 1/2 (Override installs with a native TSLPatcher-style 2DA/TLK merge),
  RimWorld / Stardew / Minecraft / Valheim (folder or loader-based installs
  with per-game lints), ~30 loose-file games, and 4 merge-tool games.
- `kind = "generic"` and `[game.custom]` cover games with no adapter at all.

**Tooling around the manifest:**

- A local mod catalog synced from Nexus's public API (`db sync`), searched by
  the CLI/GUI and used by `audit` to verify every declared mod id.
- Agent-ready profiles: a provisioned command guide, slash-commands, and an
  OS-sandboxed shell (`concierge shell`) whose write-set is derived from the
  plan — the pristine install is denied at the OS level.
- An egui GUI (mod list, catalog browser, preview/apply, generations, and an
  embedded terminal running your own agent inside the sandbox).

## Known rough edges

- Fallout 4 on macOS/CrossOver is the deep path; other games' adapters are
  functional but lightly exercised, and Windows/Linux launch flows are less
  developed than macOS.
- Several GUI affordances (sort, conflicts, load-order views) currently show
  only for Bethesda games even where other ordered games could use them.
- FOMOD conditional flags cover the common installer patterns; unusual
  installers may need explicit `select` entries.
- No prebuilt binaries yet — build with cargo or the Nix flake.

## Requirements

- Rust (or Nix) to build; `bsdtar` at runtime (preinstalled on macOS and
  Windows 10+, `libarchive-tools` on Linux); optional ClickHouse for the
  catalog; a Nexus account for automatic downloads.
