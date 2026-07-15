//! Concierge Bethesda accelerator crate.
//!
//! This is the first *per-game crate*: codified reconciliation logic for the
//! Creation-Engine games (Fallout 4, Skyrim SE) that `concierge-core` neither
//! knows about nor depends on. Core services these games' deploy/launch via
//! its built-in Bethesda adapter; THIS crate adds the accelerators — LOOT
//! load-order intelligence and the native plugin conflict/clean/resolver
//! engine (concierge-esp). Per the architecture: remove this crate and the
//! games are still serviceable through core + the AI driving generic
//! analysis; it just makes the common operations one command instead of many.

pub mod adapter;
pub mod assets;
mod bethesda_lints;
pub mod loot;
pub mod masterlist;
pub mod reconcile;

pub use bethesda_lints::bethesda_lints;

use std::path::{Path, PathBuf};

use concierge::error::{Error, IoCtx, Result};
use concierge::plan::Plan;
use concierge::repo::Repo;
use concierge_esp::conflicts::{self, Matrix};
use concierge_esp::reader::Plugin;

/// The base plugin + `.ccc`/`plugins.txt` load order for a Bethesda plan.
fn load_order(plan: &Plan) -> Result<(PathBuf, Vec<String>)> {
    let game_dir = PathBuf::from(plan.game_dir());
    let data = game_dir.join("Data");
    // The base + DLC come from the adapter (index 0 = base master), so every
    // plugin-order game works, not a hardcoded fallout4/skyrimse pair.
    let bases = concierge::game::try_adapter(&plan.game.kind)
        .and_then(concierge::game::GameAdapter::plugin_bases)
        .ok_or_else(|| Error::Other(format!("not a plugin-order game: '{}'", plan.game.kind)))?;
    let base = bases.first().copied().unwrap_or_default();
    let mut names: Vec<String> = bases.iter().map(|s| (*s).to_owned()).collect();
    let ccc = game_dir.join(format!("{}.ccc", base.trim_end_matches(".esm")));
    if let Ok(text) = std::fs::read_to_string(&ccc) {
        names.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from),
        );
    }
    for c in &plan.configs {
        if Path::new(&c.path)
            .file_name()
            .is_some_and(|n| n == "plugins.txt")
        {
            // Active entries — a leading `*` marks enabled on SE-era games; pre-SE
            // titles list them plain. Take either, skip the banner/blank lines.
            names.extend(
                c.content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(|l| l.strip_prefix('*').unwrap_or(l).to_owned()),
            );
        }
    }
    // A generated resolver isn't in the plan, but if it's been deployed it
    // loads last — include it so the matrix reflects what the game sees.
    let resolver = "ConciergeResolver.esp";
    if data.join(resolver).exists() && !names.iter().any(|n| n == resolver) {
        names.push(resolver.to_owned());
    }
    Ok((data, names))
}

/// Parse the plan's load order (dedup, existing files only) into plugins.
pub fn read_load_order(plan: &Plan) -> Result<Vec<Plugin>> {
    let (data, names) = load_order(plan)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut plugins = Vec::new();
    for name in &names {
        if !seen.insert(name.to_lowercase()) {
            continue;
        }
        let path = data.join(name);
        if path.exists() {
            plugins.push(Plugin::read(&path).map_err(|e| Error::Other(e.to_string()))?);
        }
    }
    Ok(plugins)
}

/// The activation truth for a plugin-order game: what's active (in load order),
/// what's deployed-but-inactive (inert), and unresolved dependencies — the
/// by-hand "43 active, 0 inert" check, first-class. Vanilla (base/DLC/CC) is
/// excluded from both lists.
#[derive(Debug, Clone, Default)]
pub struct ActivationReport {
    /// Active mod plugins, in load order (base/DLC/CC excluded).
    pub active: Vec<String>,
    /// Plugin files physically deployed but NOT activated (unpicked FOMOD
    /// options, dropped variants) — they don't load, but they're clutter and can
    /// mask intent. Excludes vanilla.
    pub inert: Vec<String>,
    /// Active plugins with an absent master.
    pub missing_deps: Vec<MissingMaster>,
}

fn is_plugin_ext(name: &str) -> bool {
    std::path::Path::new(name).extension().is_some_and(|e| {
        ["esp", "esm", "esl"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

/// Compute the [`ActivationReport`] for a plugin-order game. Errors for
/// non-plugin games (no adapter `plugin_bases`).
pub fn activation_report(plan: &Plan) -> Result<ActivationReport> {
    let (data, names) = load_order(plan)?;
    let bases: std::collections::HashSet<String> = concierge::game::try_adapter(&plan.game.kind)
        .and_then(concierge::game::GameAdapter::plugin_bases)
        .unwrap_or_default()
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let is_vanilla_name =
        |n: &str| bases.contains(&n.to_lowercase()) || n.to_lowercase().starts_with("cc");

    let mut seen = std::collections::BTreeSet::new();
    let active: Vec<String> = names
        .iter()
        .filter(|n| !is_vanilla_name(n))
        .filter(|n| seen.insert(n.to_lowercase()))
        .cloned()
        .collect();
    let active_set: std::collections::HashSet<String> =
        active.iter().map(|s| s.to_lowercase()).collect();

    let mut inert = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&data) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if is_plugin_ext(name)
                    && !is_vanilla_name(name)
                    && !active_set.contains(&name.to_lowercase())
                {
                    inert.push(name.to_owned());
                }
            }
        }
    }
    inert.sort();
    Ok(ActivationReport {
        active,
        inert,
        missing_deps: missing_masters(plan)?,
    })
}

/// A plugin whose declared masters are not all present in the load order — the
/// single most common Bethesda crash-on-load (the engine null-derefs resolving
/// a form against the absent master, e.g. `Fallout4.exe+0x1824513`). An empty
/// result means the load order is master-complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingMaster {
    pub plugin: String,
    pub missing: Vec<String>,
}

/// Detect enabled plugins with absent masters. A master counts as present if it
/// is itself a plugin in the load order (plugins.txt / `.ccc` / base) OR is
/// implicitly-loaded vanilla content physically in Data — the base game `.esm`,
/// a DLC (`DLC*`), or Creation Club (`cc*`), none of which the plan lists.
///
/// Crucially, an ARBITRARY deployed esp does NOT satisfy a master: an inactive
/// plugin sitting in Data (e.g. an unpicked FOMOD option) is not loaded by the
/// engine, so counting it would mask a real missing-master CTD — the game only
/// loads what's in the load order plus implicit vanilla content.
pub fn missing_masters(plan: &Plan) -> Result<Vec<MissingMaster>> {
    let (data, _names) = load_order(plan)?;
    let plugins = read_load_order(plan)?;
    let mut available: std::collections::HashSet<String> =
        plugins.iter().map(|p| p.meta.name.to_lowercase()).collect();
    // Implicit vanilla content = the adapter's base + DLC masters (per game:
    // Skyrim's named DLC, Fallout's DLC*, …) plus universal Creation Club (cc*).
    let bases: std::collections::HashSet<String> = concierge::game::try_adapter(&plan.game.kind)
        .and_then(concierge::game::GameAdapter::plugin_bases)
        .unwrap_or_default()
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    if let Ok(entries) = std::fs::read_dir(&data) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                let lower = name.to_lowercase();
                let is_plugin = std::path::Path::new(name).extension().is_some_and(|e| {
                    e.eq_ignore_ascii_case("esm")
                        || e.eq_ignore_ascii_case("esp")
                        || e.eq_ignore_ascii_case("esl")
                });
                // Only implicit vanilla content — never inert mod esps.
                if is_plugin && is_vanilla(&lower, &bases) {
                    available.insert(lower);
                }
            }
        }
    }
    let mut out = Vec::new();
    for p in &plugins {
        let missing: Vec<String> = p
            .meta
            .masters
            .iter()
            .filter(|m| !available.contains(&m.to_lowercase()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            out.push(MissingMaster {
                plugin: p.meta.name.clone(),
                missing,
            });
        }
    }
    Ok(out)
}

/// Whether a plugin physically in Data is implicitly-loaded VANILLA content —
/// one of the adapter's base/DLC masters, or Creation Club (`cc*`, universal) —
/// and so may satisfy a master without a load-order entry. A normal mod esp
/// matches none of these, so an inert deployed copy can't mask a missing master.
fn is_vanilla(name_lower: &str, bases: &std::collections::HashSet<String>) -> bool {
    bases.contains(name_lower) || name_lower.starts_with("cc")
}

/// The deterministic dependency graph derived from plugin masters (Bethesda's
/// real dependency signal — every plugin's TES4 header lists its masters).
#[derive(Debug, Clone, Default)]
pub struct DepResolution {
    /// Inter-mod dependencies to record as facts: `(mod, needs_mod)` — mod X's
    /// plugin declares a master that mod Z provides.
    pub requires: Vec<(String, String)>,
    /// A required master no mod in the pack (nor the base game/DLC) provides:
    /// `(mod_that_needs_it, missing_master)`.
    pub missing: Vec<(String, String)>,
}

/// Resolve dependencies from plugin masters: for each enabled mod's plugins,
/// map their masters to the mod that provides them (an inter-mod `requires`), or
/// flag masters nothing supplies. Fully deterministic — no external metadata.
pub fn resolve_dependencies(plan: &Plan) -> Result<DepResolution> {
    let plugins = read_load_order(plan)?;
    // plugin filename (lowercase) -> providing mod name
    let mut provider: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in &plan.mods {
        for pl in &m.plugins {
            provider.insert(pl.to_lowercase(), m.name.clone());
        }
    }
    // base game / DLC / CC plugins physically in Data count as satisfied
    let (data, _names) = load_order(plan)?;
    let mut base: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(&data) {
        for e in entries.flatten() {
            if let Some(n) = e.file_name().to_str() {
                base.insert(n.to_lowercase());
            }
        }
    }
    let mut requires = std::collections::BTreeSet::new();
    let mut missing = Vec::new();
    for p in &plugins {
        let Some(owner) = provider.get(&p.meta.name.to_lowercase()) else {
            continue;
        };
        for mast in &p.meta.masters {
            let ml = mast.to_lowercase();
            if let Some(prov) = provider.get(&ml) {
                if prov != owner {
                    requires.insert((owner.clone(), prov.clone()));
                }
            } else if !base.contains(&ml) {
                missing.push((owner.clone(), mast.clone()));
            }
        }
    }
    Ok(DepResolution {
        requires: requires.into_iter().collect(),
        missing,
    })
}

/// Build the record-level conflict matrix for a Bethesda plan.
pub fn conflict_matrix(plan: &Plan) -> Result<(usize, Matrix)> {
    let plugins = read_load_order(plan)?;
    let matrix = conflicts::build(&plugins).map_err(|e| Error::Other(e.to_string()))?;
    Ok((plugins.len(), matrix))
}

/// Serialize conflict findings to `JSONEachRow` for the `ClickHouse` store.
pub fn conflict_rows(plan: &Plan, matrix: &Matrix, now_unix: &str) -> Result<String> {
    use std::fmt::Write as _;
    let mut rows = String::new();
    let domain = plan
        .game
        .nexus_domain
        .clone()
        .unwrap_or_else(|| plan.game.kind.clone());
    let plan_hash = plan.hash()?;
    for c in &matrix.conflicts {
        let losers: Vec<&String> = c.carriers.iter().filter(|p| **p != c.winner).collect();
        let row = serde_json::json!({
            "game_domain": domain,
            "plan_hash": plan_hash,
            "record_signature": c.signature,
            "form_id": c.object_id,
            "winner_plugin": c.winner,
            "loser_plugin": losers.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(";"),
            "field_path": "",
            "severity": if c.danger { "danger" } else { "normal" },
            "found_at": now_unix,
        });
        let _ = writeln!(rows, "{row}");
    }
    Ok(rows)
}

/// Dirty-plugin (ITM/UDR/navmesh) report for one plugin against the load order.
pub fn dirty_report(plan: &Plan, plugin_name: &str) -> Result<concierge_esp::clean::DirtyReport> {
    let plugins = read_load_order(plan)?;
    let target = plugins
        .iter()
        .find(|p| p.meta.name.eq_ignore_ascii_case(plugin_name))
        .ok_or_else(|| Error::Other(format!("{plugin_name} not in the load order")))?;
    // masters, in the target's declared order
    let masters: Vec<Option<&Plugin>> = target
        .meta
        .masters
        .iter()
        .map(|m| plugins.iter().find(|p| p.meta.name.eq_ignore_ascii_case(m)))
        .collect();
    concierge_esp::clean::analyze(target, &masters).map_err(|e| Error::Other(e.to_string()))
}

/// Write the resolver plugin shell for a Bethesda plan; re-parse to validate.
pub fn write_resolver(repo: &Repo, plan: &Plan) -> Result<(PathBuf, usize)> {
    let masters: Vec<String> = std::iter::once(match plan.game.kind.as_str() {
        "fallout4" => "Fallout4.esm".to_owned(),
        "skyrimse" => "Skyrim.esm".to_owned(),
        other => return Err(Error::Other(format!("not a Bethesda game: '{other}'"))),
    })
    .chain(plan.game_dlc_and_plugins())
    .collect();
    let out = repo.state_dir().join("ConciergeResolver.esp");
    concierge_esp::writer::write_resolver(&out, "concierge", &masters)
        .map_err(|e| Error::Other(e.to_string()))?;
    let back = Plugin::read(&out).map_err(|e| Error::Other(format!("re-parse failed: {e}")))?;
    if back.meta.is_esl || back.meta.form_version != 131 {
        return Err(Error::Other(
            "resolver failed self-validation (must be a plain ESP, form v131)".into(),
        ));
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    Ok((out, back.meta.masters.len()))
}

#[cfg(test)]
mod tests {
    use super::is_vanilla;
    use std::collections::HashSet;

    fn bases(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_lowercase()).collect()
    }

    #[test]
    fn vanilla_detection_uses_the_adapter_bases_plus_cc() {
        // Fallout 4 bases (base + DLC*) from the adapter, plus universal cc*.
        let fo4 = bases(&["Fallout4.esm", "DLCCoast.esm", "DLCNukaWorld.esm"]);
        assert!(is_vanilla("fallout4.esm", &fo4));
        assert!(is_vanilla("dlccoast.esm", &fo4));
        assert!(
            is_vanilla("ccbgsfo4001-pipboy.esl", &fo4),
            "cc* is always vanilla"
        );

        // Skyrim SE named DLC — recognised because they're in the adapter's bases,
        // not via any DLC* prefix heuristic.
        let sse = bases(&[
            "Skyrim.esm",
            "Update.esm",
            "Dawnguard.esm",
            "HearthFires.esm",
            "Dragonborn.esm",
        ]);
        assert!(is_vanilla("dawnguard.esm", &sse));
        assert!(is_vanilla("hearthfires.esm", &sse));
        assert!(is_vanilla("ccbgssse001-fish.esm", &sse));

        // A normal mod esp is NOT vanilla — an inert deployed copy can't mask a master.
        assert!(!is_vanilla("bettersettlers.esp", &fo4));
        assert!(!is_vanilla("dawnguard.esm", &fo4), "not a FO4 base");
        assert!(!is_vanilla("someskyrimmod.esp", &sse));
    }
}
