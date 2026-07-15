# Declaration-language layers — prior-art check

We separated the declaration into three concerns (WHAT / HOW-CONFIGURED /
RELATIONAL — see `declaration-schema.md`). This checks that against how
established package managers and mod formats structure their manifests, to see
what layers we haven't considered. Two research passes (sources at the end).

**Verdict:** the three-layer core is sound and matches prior art, but the prior
art distinguishes **more** layers. The single clearest gap: our RELATIONAL layer
holds only **resolutions** (patches, conflict-resolution rules) — it's missing
the **declared facts** (requires / incompatible / provides) that *every* format
keeps as a first-class, separate thing. That split also matches our own
reconciliation ladder: **facts belong in Topology, resolutions in Reconcile.**

## The layers, and whether we have them

| Layer | Prior art | Us |
|---|---|---|
| Identity + acquisition | all (Cargo `[package]`, Modrinth `files[]`, Thunderstore `name/version`) | ✅ WHAT |
| Per-mod config / options | NixOS options, MO2 `meta.ini` | ✅ HOW-CONFIGURED |
| Explicit load order | LOOT `after`, MO2 `loadorder.txt` | ✅ RELATIONAL |
| Conflict **resolution** (who wins a file) | our patches/rules | ✅ RELATIONAL |
| **Requirements** — `A needs B (≥ v)` | Debian `Depends(>=)`, Cargo/npm semver, Thunderstore/Modrinth/CurseForge `dependencies`, **LOOT `req`**, FOMOD `moduleDependencies` | ❌ **missing** |
| **Incompatibilities** — declared fact `A ✗ B` | Debian `Conflicts`/`Breaks`, **LOOT `inc`** | ❌ (we resolve, don't declare) |
| **Virtual capabilities** — `provides "weather-framework"` | Debian `Provides` | ❌ missing |
| **Environment/compat** — game ver, DLC, script-extender, loader, client/server | npm `engines`/`os`, Cargo `rust-version`/`[target]`, **Modrinth `env` + loader deps**, CurseForge `modLoaders`, Wabbajack `GameType` | ❌ missing |
| **Groups / categories** — order groups, not mods | **LOOT `group`** graph, MO2 separators, `meta.ini category` | ❌ missing |
| **Conditional/optional install** — flag-gated user choices | **FOMOD** `installSteps`/`conditionalFileInstalls`, Modrinth `env:optional`, CurseForge `required:false` | ⚠ static only |
| **Metadata / trust** — author, site, NSFW, **multi-mirror**, **dual-hash** | Cargo/npm/Debian metadata, Wabbajack `Author`/`IsNSFW`, Modrinth `downloads[]` + sha1+sha512 | ⚠ single hash only |
| **Locked resolved state vs intent** | Cargo.lock, flake.lock, Terraform state | ⚠ per-mod hash + generations = partial |
| **Derived/generated artifacts vs source** — previsibines, merged/bashed patches, FNIS/Nemesis, LOD | Cargo `[profile]`/build outputs; LOOT `tag`, Wabbajack `MergedPatch`/`CreateBSA` | ❌ missing |
| **Dirty-edit / cleaning info** — CRC/ITM/UDR/NAV | **LOOT `dirty`/`clean`** | ❌ missing |

## Recommendation — universal core vs game-specific

### Add to the core language (universal), prioritized

1. **Declared relational *facts*, split from resolutions.** Add to `[relations]`:
   - `requires` — `A` needs `B` (optionally `>= version`); drives auto-acquisition **and** implies ordering. (Debian `Depends`, LOOT `req`.)
   - `incompatible` — declared fact `A` ✗ `B` (refuse the combo up front). (Debian `Conflicts`, LOOT `inc`.)
   - `provides` — virtual capability so interchangeable mods satisfy one `requires` ("any weather framework"). (Debian `Provides`.)
   Keep `patch`/`rule` as the *resolution* side. **This is the #1 gap and low-risk/additive.**
2. **Environment / compatibility block** (universal shell, game-specific values):
   a per-manifest and/or per-mod `requires` on the environment — game version range, required DLC, script-extender version, loader, client/server side. Without it a modpack is silently non-portable. (Modrinth `env`, npm `engines`.)
3. **Groups** — a `[[relations.group]]` (name + `after`) so ordering scales to 100+ mods by ordering *groups*, not individual plugins. (LOOT `group`.)
4. **Conditional/optional install mechanism** — a flag-gated install-choice language (distinct from static `options`); this is also what FOMOD ingestion needs.
5. **Metadata/trust** — author/site/NSFW + **multiple download mirrors** + **dual hash** (sha1+sha512). Resilience + legal/UI. Lower urgency.
6. **Profile lockfile** — a resolved-state artifact (versions + order + hashes + resolution decisions) pinning the *whole* profile for byte-reproducible sharing. We have per-mod hashes + declaration *generations*; the profile-level lock is the missing piece (Cargo.lock / flake.lock / Terraform state).

### Leave to per-game adapters (specialization — as you noted)

- **Generated-artifact directives** — bashed/merged patches, `CreateBSA`, previsibines, FNIS/Nemesis (feed the Synthesize rung; Bethesda-specific).
- **Dirty/cleaning info** (CRC/ITM/UDR/NAV) — Bethesda plugin QA.
- **The values inside the environment block** — which loaders/versions/sides exist per game.
- **Bash Tags** — Bethesda merge hints.

## The one-line takeaway
Every established format separates **declared relational facts** (requires /
incompatible / provides) from **resolutions** (patches / order) — we only have
the latter. Adding the facts layer (plus an environment-compat block) is the
highest-value, lowest-risk evolution, and it's exactly what our own ladder wants
(fact → Topology, resolution → Reconcile). Groups, conditional-install,
metadata/trust, and a profile lockfile follow. Generated artifacts, cleaning
data, and Bash Tags are per-game adapter concerns.

## Sources
Package managers: [Debian Policy ch.7 (relationships)](https://www.debian.org/doc/debian-policy/ch-relationships.html), [npm package.json](https://docs.npmjs.com/cli/v10/configuring-npm/package-json), [Cargo manifest](https://doc.rust-lang.org/cargo/reference/manifest.html), [NixOS flakes](https://wiki.nixos.org/wiki/Flakes), [Terraform providers](https://developer.hashicorp.com/terraform/language/providers/requirements), [Flatpak permissions](https://docs.flatpak.org/en/latest/sandbox-permissions.html).
Mod formats: [Modrinth .mrpack](https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack), [Thunderstore package](https://wiki.thunderstore.io/mods/creating-a-package), [LOOT plugin](https://loot-api.readthedocs.io/en/latest/metadata/data_structures/plugin.html) + [group](https://loot-api.readthedocs.io/en/latest/metadata/data_structures/group.html), [Wabbajack DTOs](https://github.com/wabbajack-tools/wabbajack/tree/main/Wabbajack.DTOs/ModList), [FOMOD ModuleConfig](https://fomod-docs.readthedocs.io/en/latest/_static/ModuleConfig.html), [CurseForge manifest](https://gdlauncher.com/docs/modpack-manifest-format/).
