# Concierge v0.1.0

> [!WARNING]
> **Alpha release.** This is a tool I made for myself I thought others may
> benefit from. Please note docs are AI generated for now.

Concierge is a mod manager. Your mod list lives in a text file; Concierge
downloads the mods, verifies them, installs them into a separate copy of the
game, sorts the load order, and launches. The original game install is never
modified. It can be driven by hand or by an AI assistant running in a sandbox
that can only write where the pack allows.

## In this release

- About 45 supported games; Fallout 4 is the most tested (including on macOS
  via CrossOver), the rest have seen less use.
- Nexus Mods downloads (automatic with a Premium key, guided without),
  checksum verification, and a searchable local mod catalog.
- FOMOD installer choices stored in the pack file and replayed on install.
- LOOT-based load-order sorting; health checks for missing dependencies,
  plugin limits, on-disk drift, and pristine-install verification; `preview`
  before any install.
- Multiple packs per game with shared downloads; rollback to earlier states;
  `undeploy` removes everything.
- Agent operation: per-pack assistant instructions and a sandboxed
  `concierge shell`.
- CLI plus an egui GUI (mod browser, preview/apply, rollback). The GUI is
  source-built for now: `cargo run -p concierge-gui`.

## Downloads

| platform | file |
|---|---|
| Linux x86_64 | `concierge-v0.1.0-x86_64-linux.tar.gz` |
| Windows x86_64 | `concierge-v0.1.0-x86_64-windows.zip` |
| macOS Apple Silicon | `concierge-v0.1.0-aarch64-macos.tar.gz` |
| macOS Intel | `concierge-v0.1.0-x86_64-macos.tar.gz` |

Runtime dependency: `bsdtar` (preinstalled on macOS and Windows 10+;
`libarchive-tools` on Linux).

## Known limitations

- Windows and Linux launch flows are less developed than macOS.
- Some GUI views (sorting, conflicts, load order) only appear for Bethesda
  games.
- Unusual FOMOD installers may need choices written into the pack file by
  hand.
