//! Profile provisioning: every profile folder carries the
//! agent guide (CLAUDE.md), slash-commands, and a permissions allowlist — so
//! opening ANY agent in the folder gives the same capabilities the GUI's
//! agent view gets. The GUI is a convenience; the folder is the interface.
//!
//! Everything is written only-if-absent: provisioning never clobbers a
//! user's edits. The guide states COMMAND SEMANTICS precisely — an agent with
//! an underspecified command surface fills the gaps with invented advice, so
//! the guide is the single place that knowledge lives.

use std::path::Path;

use crate::error::{IoCtx as _, Result};

/// Write `path` only if absent; returns whether it was created.
fn put(path: &Path, body: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    std::fs::write(path, body).ctx(path)?;
    Ok(true)
}

/// Provision a profile dir with the agent surface. Idempotent; returns the
/// files created this call (empty = everything was already there).
pub fn provision_profile(dir: &Path, kind: &str) -> Result<Vec<String>> {
    let mut created = Vec::new();
    // The same guide serves every agent: claude reads CLAUDE.md, codex and
    // opencode read AGENTS.md — so the sandboxed shell is preconfigured whichever
    // one you run.
    let g = guide(kind);
    let files: &[(&str, String)] = &[
        ("CLAUDE.md", g.clone()),
        ("AGENTS.md", g),
        (".claude/settings.json", SETTINGS.to_owned()),
        (".claude/commands/health.md", HEALTH.to_owned()),
        (".claude/commands/sort.md", SORT.to_owned()),
        (".claude/commands/conflicts.md", CONFLICTS.to_owned()),
        (".claude/commands/diagnose-launch.md", DIAGNOSE.to_owned()),
        (".claude/commands/curate.md", CURATE.to_owned()),
        (".claude/commands/audit-ids.md", AUDIT_IDS.to_owned()),
    ];
    for (rel, body) in files {
        if put(&dir.join(rel), body)? {
            created.push((*rel).to_owned());
        }
    }
    Ok(created)
}

/// The per-profile agent guide. Precise about what each command does — an
/// agent should never have to guess semantics (guessing is how "run
/// `concierge check` to validate ids" happened).
fn guide(kind: &str) -> String {
    let generic = generic_guide(kind);
    // Append this game's own modding norms, if its adapter offers them, so the
    // assistant reasons from community reality — not the same generic advice for
    // every game. Falls back to the generic guide when no adapter is registered
    // (e.g. a bare unit test) or the game declares no guidance.
    match crate::game::adapter_for(kind)
        .ok()
        .and_then(crate::game::GameAdapter::agent_guide)
    {
        Some(game) => format!("{generic}\n## Modding {kind} — community norms\n\n{game}\n"),
        None => generic,
    }
}

/// The game-agnostic guide body — precise command semantics every profile needs.
fn generic_guide(kind: &str) -> String {
    format!(
        r#"# Concierge agent guide — {kind} profile

This folder IS the interface: `manifest.toml` is the single source of truth
for the modded game. Run commands with this dir as CONCIERGE_REPO (it already
is, inside `concierge shell`). The pristine game install is never written —
mods realize into a disposable CoW instance; the OS sandbox (`concierge
shell`) enforces exactly that write-set.

## Command semantics (precise — don't guess)
- `concierge eval` — manifest -> pure hashed Plan. Prints UNPINNED and
  unaudited counts. Read-only apart from state/plan.json.
- `concierge fetch` — download declared archives into the shared store
  (content-addressed); prints md5 pins to commit into the manifest.
- `concierge realize` — fetch + build + deploy into the instance; runs the
  game's invariant lints and REFUSES on violations. Not id validation.
- `concierge check [--vanilla]` — DRIFT detection: deployed files vs recorded
  state (+ pristine vs its inventory with --vanilla). It never contacts
  Nexus and cannot validate a mod id.
- `concierge audit` — THE id validator: checks every `nexus_mod_id` against
  the synced catalog (OK / NAME MISMATCH / UNKNOWN ID), records
  state/audit.json, exits nonzero on any unverified id.
- `concierge db sync <domain>` — populate the local catalog from Nexus's
  public GraphQL (state/catalog.sqlite). Needed once before audit/search.
- `concierge ai --catalog "<query>"` — search that catalog (name,
  endorsements, mod id). NEVER invent a mod or an id: search, or say so.
- `concierge nexus resolve <mod_id>` — pick the mod's MAIN file: prints
  nexus_file_id + file for the manifest entry.
- `concierge sort [--apply]` / `concierge conflicts [--assets]` /
  `concierge reconcile` — load order, conflict matrix, deterministic merges.
- `concierge inventory [--force]` — hash the pristine into the per-game
  vanilla baseline that `check --vanilla` verifies.
- `concierge lock` / `unlock` — the declaration becomes read-only ON DISK
  (chmod + immutable flag). While locked: explain and audit only; realize
  and every manifest write refuse. Never try to lift the lock yourself.
- `concierge shell [--agent <cmd>]` — this sandbox. Writes outside the
  plan's write-set fail with EPERM; that is intended, not a bug to work
  around.

## Adding a mod (the only honest flow)
1. Find it: `concierge ai --catalog "<query>"` (respect [curate] filters).
2. Add `[[mod]]`: name, version, nexus_mod_id from the catalog.
3. Pin its file: `concierge nexus resolve <mod_id>` -> set nexus_file_id +
   file. `concierge fetch` downloads and prints the md5 pin.
4. `concierge audit` before anything is enabled for realize.
If a mod can't be resolved yet, PARK it: `enabled = false` with the
nexus_mod_id — parse-valid, excluded from the plan, auditable. An enabled
entry without a resolvable file does not eval.

## Curation rules
The profile's [curate] block is the user's brief + hard filters
(min_endorsements, nsfw, categories_avoid, avoid) and soft preferences
(lore_friendly, scope, must_have). Respect them in EVERY pick; the catalog
search already applies the hard filters. Never remove a user's pin; never
pick from the avoid list.

## The reconciliation ladder (resolve at the lowest rung that works)
0. Acquire — fetch mods (Nexus, http, or a git/http pipeline:
   `concierge ai --propose-pipeline "<name>" --steps '<json>'`).
1. Topology — load order (`concierge sort --apply`).
2. Reconcile — deterministic merges (`concierge reconcile`).
3. Synthesize — AI authors what the lower rungs refuse
   (`concierge ai --resolve "<path>" --winner "<mod>"` — the core verifies).
Climb only on failure; the deterministic core is your oracle.

## Discipline
- Deterministic first, AI last: sort -> reconcile -> only then synthesize.
- A fix counts only if the core verifies it (eval/audit/check/lints pass).
- Report what you changed, and what you could NOT verify, explicitly.
"#
    )
}

const SETTINGS: &str = r#"{
  "permissions": {
    "allow": [
      "Bash(concierge eval*)",
      "Bash(concierge status*)",
      "Bash(concierge check*)",
      "Bash(concierge audit*)",
      "Bash(concierge conflicts*)",
      "Bash(concierge sort)",
      "Bash(concierge ai --catalog*)",
      "Bash(concierge nexus resolve*)",
      "Bash(concierge nexus files*)",
      "Bash(concierge nexus tracked*)",
      "Bash(concierge nexus updates*)",
      "Bash(concierge db sync*)"
    ]
  }
}
"#;

const HEALTH: &str = r"Run the read-only health checks for this profile and give a short report:
`concierge eval` (note UNPINNED / unaudited counts), `concierge check`
(drift), `concierge audit` if a catalog exists, and the invariant lints
(surfaced by `concierge realize` — do NOT realize just for lints; read the
manifest and deployed state instead). Report: missing masters, plugin-limit,
drift, unverified ids, and anything else that would break or crash the game.
Keep every claim tied to command output.
";

const SORT: &str = r"Sort this profile's load order (LOOT rules) with `concierge sort`, review the
proposed order, then apply it with `concierge sort --apply` so the manifest's
mod order matches. Summarize what moved and why.
";

const CONFLICTS: &str = r"Run `concierge conflicts` (and `--assets` if useful) for this profile.
Summarize which mods overwrite which records/files, which conflicts are
dangerous, and how to resolve them — lowest rung first: load order
(`concierge sort`), then deterministic merges (`concierge reconcile`), and
only then AI-authored resolution.
";

const DIAGNOSE: &str = r"The most recent game launch may have failed. Read the runtime logs (CrossOver
CX_LOG, the game's crash / script-extender logs), diagnose the cause from the
evidence — don't guess — and propose or apply a fix. Keep every claim tied to
a log line.
";

const CURATE: &str = r"You are curating a modpack for this profile, conversationally, BEFORE anything
is downloaded. Work in phases: (1) INTERVIEW — a few sharp questions about the
playthrough (theme, difficulty, scope, must-haves, hard nos, hardware); listen,
don't lecture. (2) RESEARCH — `concierge ai --catalog` for every candidate
(sync first if empty: `concierge db sync <domain>`); NEVER invent a mod or an
id. (3) ASSEMBLE — add chosen mods to manifest.toml, pinned via `concierge
nexus resolve` where possible, PARKED (`enabled = false`) where not; declare
[relations] facts; fill [curate] with the brief. Finish with `concierge audit`
and tell the user exactly what is verified and what is parked.
";

const AUDIT_IDS: &str = r#"Verify every Nexus id in this manifest. If state/catalog.sqlite is missing,
sync it first (`concierge db sync <domain>` — a few minutes). Run `concierge
audit`; for each MISMATCH/UNKNOWN, find the real mod with `concierge ai
--catalog "<name>"`, fix the manifest entry, and re-audit until clean. Never
leave a wrong id in place, and never invent one.
"#;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_is_idempotent_and_complete() {
        let dir = std::env::temp_dir().join(format!("concierge-prov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let created = provision_profile(&dir, "fallout4").unwrap();
        assert_eq!(
            created.len(),
            9,
            "guide (CLAUDE.md + AGENTS.md) + settings + 6 commands: {created:?}"
        );
        assert!(dir.join("CLAUDE.md").exists());
        assert!(dir.join("AGENTS.md").exists());
        assert!(dir.join(".claude/settings.json").exists());
        assert!(dir.join(".claude/commands/audit-ids.md").exists());
        // Second run creates nothing and clobbers nothing.
        std::fs::write(dir.join("CLAUDE.md"), "user edited").unwrap();
        let again = provision_profile(&dir, "fallout4").unwrap();
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "user edited"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guide_states_the_command_semantics_precisely() {
        let g = guide("fallout4");
        // The exact gaps that produced bad advice must be closed in writing.
        assert!(g.contains("cannot validate a mod id"), "check semantics");
        assert!(g.contains("THE id validator"), "audit semantics");
        assert!(
            g.contains("PARK it: `enabled = false`"),
            "parked-entry flow"
        );
        assert!(g.contains("NEVER invent a mod or an id"));
    }
}
