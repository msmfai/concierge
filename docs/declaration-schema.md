# The declaration language — three separated concerns

Concierge's declaration (`manifest.toml`) deliberately keeps **three distinct
concerns** in three distinct places, so it's always clear whether you're saying
*what mods you have*, *how one mod is set up*, or *how mods relate to each other*.

| Concern | Where it lives | What belongs here |
|---------|----------------|-------------------|
| **1. WHAT** — the mods you have | `[[mod]]` core fields | identity + acquisition: `name`, `version`, source (`nexus_mod_id`/`url`/`pipeline`/`nix`), `md5`, `file`, `enabled` |
| **2. HOW IT'S CONFIGURED** — one mod's own setup | `[mod.config]` sub-table | that mod's *isolated* install settings: `install_root`, `subdir`, `provides` (activation entries), `exclude`, `options` |
| **3. RELATIONAL** — how mods relate | `[relations]` section | cross-mod concerns, split into **facts** (`[[relations.requires]]`, `[[relations.incompatible]]`, `[[relations.provides]]`) and **resolutions** (`load_order`, `[[relations.patch]]`, `[[relations.rule]]`) |

The rule of thumb: a property **of a single mod** goes in that mod (WHAT or its
`[mod.config]`); a relationship **between mods** goes in `[relations]`.

## Example

```toml
[game]
kind = "skyrimse"
pristine = "/path/to/Skyrim Special Edition"
version = "1.6.1170"

# ── ENVIRONMENT: what the pack needs from outside the mods ──
[compat]
game_version = "1.6.1170"      # minimum; game_version_max caps it (e.g. next-gen)
script_extender = "2.2.6"      # SKSE/F4SE minimum
# minecraft: loader = "fabric", loader_version = "0.15", side = "client"

# ── 1. WHAT: the mods (identity + how to acquire each) ──
[[mod]]
name = "SkyUI"
version = "5.2"
nexus_mod_id = 12604
nexus_file_id = 749043
file = "SkyUI_5_2_SE.7z"
md5 = "cb99578726fe8dd4e30204617ffe7a52"
enabled = true

  # ── 2. HOW SkyUI IS CONFIGURED: its own install settings (isolated) ──
  [mod.config]
  install_root = "data"
  provides = ["SkyUI_SE.esp"]        # the plugins this mod contributes
  options = { fontsize = "large" }   # per-mod options (INI/FOMOD choices)

[[mod]]
name = "AWKCR Patch"
version = "1.0"
url = "https://example/awkcr-patch.7z"

# ── 3. RELATIONAL: cross-mod concerns ──
[relations]

  # ── declared FACTS (topology) — what relates to what ──
  [[relations.requires]]             # A needs B (optionally at a min version)
  name = "UniqueCreatures"
  needs = "weather-framework"        # a mod name OR a provided capability
  min_version = "6.3"
  [[relations.incompatible]]         # hard fact: these must not both be enabled
  a = "ModA"
  b = "ModB"
  note = "both replace the HUD"
  [[relations.provides]]             # virtual capability (interchangeable mods)
  name = "TrueStormsSE"
  capability = "weather-framework"

  # ── RESOLUTIONS (reconcile) — how contested outcomes are decided ──
# explicit plugin order; omit to derive from [[mod]] order + each mod's `provides`
load_order = ["SkyUI_SE.esp", "AWKCRPatch.esp"]

  [[relations.patch]]                # a compatibility patch bridges specific mods
  name = "AWKCR Patch"               # its files are the [[mod]] of the same name
  bridges = ["SkyUI", "SomeArmorMod"]
  note = "needed so both register their keywords"

  [[relations.rule]]                 # who wins a contested path
  path = "meshes/armor/x.nif"
  winner = "SomeArmorMod"
```

## Notes & compatibility

- **Backward compatible.** Older manifests that wrote install settings flat on
  the `[[mod]]` (e.g. a top-level `install_root`/`plugins`) still parse — on load
  a `[mod.config]` is *folded* onto those same fields (config wins), exactly like
  the legacy `plugins_txt`/`my_games` game fields. New manifests should prefer
  `[mod.config]`.
- **`provides` vs `plugins`.** In `[mod.config]` the activation entries are named
  `provides` (they're what the mod *provides* to the load order); the flat legacy
  field is `plugins`. They fold to the same place.
- **Load order is relational, not per-mod.** A single mod declares *what plugins
  it provides* (`provides`); the *order* those plugins load in is a relationship
  across mods and lives in `[relations].load_order`. If omitted, order is derived
  from `[[mod]]` order.
- **Patches are relationships, not just files.** A compatibility patch's files
  are an ordinary `[[mod]]`; `[[relations.patch]]` records *which mods it bridges*
  so the intent is explicit and auditable.

### Additional layers (from the prior-art check, `schema-layers-analysis.md`)

All implemented and backward-compatible:
- **Relational facts** (`[[relations.requires]]` / `[[relations.incompatible]]` /
  `[[relations.provides]]`) — topology, distinct from resolutions; checked by
  `Manifest::relation_issues()`.
- **Environment/compat** (`[compat]`) — game version window, DLC, script
  extender, loader, side; portability made explicit.
- **Metadata** (`[mod.meta]`: author/description/category/tags/nsfw/license/
  website) + **trust** (`mirrors`, `sha512`) — descriptive, distinct from identity.
- **Groups** (`[[relations.group]]`, `[mod.config].group`) — order groups, not
  every plugin.
- **Conditional install** (`[[mod.config.choice]]` + `option`) — declarative
  FOMOD; selected options' plugins fold into activation on load.
- **Profile lockfile** (`concierge.lock`, `concierge-core::lockfile`) — resolved
  pin (versions + hashes + order) for byte-reproducible sharing; written on Realize.
- **Curation** (`[curate]`) — the white-glove inputs: a free-text `brief` plus
  declarative filters on the mod catalog. Hard filters (`min_endorsements`,
  `max_size_mb`, `updated_since`, `nsfw`, `categories_avoid`, `avoid`) narrow the
  searchable set (applied in the catalog SQL via `CatalogFilter::from_curate`);
  soft preferences (`categories_prefer`, `lore_friendly`, `scope`, `must_have`)
  steer the curator. The user says the experience + the rules; Concierge picks
  the exact mods within them.

## Servicing a game without an adapter

Two doors, in increasing order of knowledge:

- **`kind = "generic"`** — the no-knowledge case. Assumes nothing about how
  modding works beyond "mods add or replace files under the game dir": every
  mod overlays into the CoW instance (a pure diff against the pristine), no
  activation registry, no launch knowledge. Point it at any install.
- **`kind = "custom"` + `[game.custom]`** — the data-driven case: the manifest
  declares roots (`dir` or external `path_key` mounts), rendered config files,
  launch candidates, and a Nexus domain. Any unknown `kind` with a
  `[game.custom]` section resolves this way too (e.g. `kind = "civ5"`).

Game-specific concerns (generated artifacts / bashed patches / previsibines,
dirty-edit cleaning data, Bash Tags) stay in per-game adapters, not the core
language.

Implemented in `concierge-core::manifest` (`Mod`, `ModConfig`, `ModMeta`,
`Relations`, `Requirement`, `Incompatibility`, `Provision`, `Patch`, `Rule`,
`Group`, `Choice`, `Compat`) + `concierge-core::lockfile`; eval honors
`[relations].load_order`. Tests: `manifest::schema_tests`, `lockfile::tests`.
