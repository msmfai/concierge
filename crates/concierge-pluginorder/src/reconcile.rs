//! Auto-reconcile: from the deployed load order, build the conflict matrix,
//! merge the conflicting leveled lists (LVLI/LVLN/LVSP) and form-lists (FLST)
//! into a real resolver plugin, add UDR fixes for the user's mods, and report
//! danger-class conflicts (never merged). The resolver is written with the
//! correct master table and every entry `FormID` remapped into its master
//! space.

use std::collections::BTreeMap;
use std::path::PathBuf;

use concierge::error::{Error, Result};
use concierge::plan::Plan;
use concierge::repo::Repo;
use concierge_esp::reader::{Plugin, RecordRef};
use concierge_esp::writer::{resolver_bytes_with, OverrideRecord};
use concierge_esp::{formlist, lvli};

use crate::sortrules::SortRules;

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub plugins: usize,
    pub conflicts: usize,
    pub danger: usize,
    /// Merged leveled lists (LVLI + LVLN + LVSP).
    pub leveled_merged: usize,
    /// Merged form-lists (FLST).
    pub formlist_merged: usize,
    pub udr_fixed: usize,
    pub masters: Vec<String>,
    pub resolver: PathBuf,
    pub resolver_records: usize,
}

/// The plugin a record ultimately belongs to (its originating master), and the
/// 24-bit object id — resolving the `FormID`'s master-index byte.
fn origin_of(rec_formid: u32, plugin: &Plugin) -> (String, u32) {
    origin_of_parts(rec_formid, &plugin.meta.masters, &plugin.meta.name)
}

/// Pure core of [`origin_of`]: high byte indexes the master table (else the
/// plugin itself owns the record); the low 24 bits are the object id.
fn origin_of_parts(rec_formid: u32, masters: &[String], self_name: &str) -> (String, u32) {
    let idx = usize::try_from(rec_formid >> 24).unwrap_or(0);
    let object_id = rec_formid & 0x00FF_FFFF;
    let name = masters
        .get(idx)
        .cloned()
        .unwrap_or_else(|| self_name.to_owned());
    (name, object_id)
}

fn sig4(s: &str) -> [u8; 4] {
    let mut b = [b' '; 4];
    for (i, c) in s.bytes().take(4).enumerate() {
        if let Some(slot) = b.get_mut(i) {
            *slot = c;
        }
    }
    b
}

/// One record's per-carrier parsed versions, discriminated by list kind.
enum Versions {
    Leveled([u8; 4], Vec<(String, lvli::LeveledList)>),
    Form(Vec<(String, formlist::FormList)>),
}

/// Build the resolver for a Bethesda plan: merge conflicting leveled/form lists
/// + UDR fixes.
#[allow(clippy::too_many_lines)]
pub fn reconcile(repo: &Repo, plan: &Plan) -> Result<ReconcileReport> {
    let plugins = crate::read_load_order(plan)?;
    let by_name: BTreeMap<String, &Plugin> = plugins
        .iter()
        .map(|p| (p.meta.name.to_lowercase(), p))
        .collect();
    let matrix =
        concierge_esp::conflicts::build(&plugins).map_err(|e| Error::Other(e.to_string()))?;

    let ml = load_sortrules(repo, &plan.game.kind);

    let mut report = ReconcileReport {
        plugins: plugins.len(),
        conflicts: matrix.conflicts.len(),
        danger: matrix.conflicts.iter().filter(|c| c.danger).count(),
        ..ReconcileReport::default()
    };

    // Mergeable, non-danger conflicts: leveled lists and form-lists.
    let mergeable: Vec<_> = matrix
        .conflicts
        .iter()
        .filter(|c| {
            !c.danger
                && (lvli::is_leveled(sig4(&c.signature)) || sig4(&c.signature) == formlist::FLST)
        })
        .collect();

    let load_order: Vec<String> = plugins.iter().map(|p| p.meta.name.clone()).collect();
    let mut needed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // (origin_plugin, object_id, versions)
    let mut parsed: Vec<(String, u32, Versions)> = Vec::new();

    // Pass 1: parse each carrier's version, collect referenced origins.
    for c in &mergeable {
        let sig = sig4(&c.signature);
        needed.insert(c.origin.clone());
        if lvli::is_leveled(sig) {
            let mut versions = Vec::new();
            for carrier in &c.carriers {
                let Some(plugin) = by_name.get(&carrier.to_lowercase()) else {
                    continue;
                };
                let Some(rec) = find_record(plugin, sig, &c.origin, c.object_id) else {
                    continue;
                };
                let list = lvli::parse(plugin, rec).map_err(|e| Error::Other(e.to_string()))?;
                for e in &list.entries {
                    needed.insert(origin_of(e.reference, plugin).0);
                }
                versions.push(((*carrier).clone(), list));
            }
            if !versions.is_empty() {
                parsed.push((
                    c.origin.clone(),
                    c.object_id,
                    Versions::Leveled(sig, versions),
                ));
            }
        } else {
            let mut versions = Vec::new();
            for carrier in &c.carriers {
                let Some(plugin) = by_name.get(&carrier.to_lowercase()) else {
                    continue;
                };
                let Some(rec) = find_record(plugin, formlist::FLST, &c.origin, c.object_id) else {
                    continue;
                };
                let list = formlist::parse(plugin, rec).map_err(|e| Error::Other(e.to_string()))?;
                for id in &list.entries {
                    needed.insert(origin_of(*id, plugin).0);
                }
                versions.push(((*carrier).clone(), list));
            }
            if !versions.is_empty() {
                parsed.push((c.origin.clone(), c.object_id, Versions::Form(versions)));
            }
        }
    }

    // Resolver master table: needed plugins, in load order.
    let masters: Vec<String> = load_order
        .iter()
        .filter(|n| needed.contains(*n))
        .cloned()
        .collect();
    let master_index: BTreeMap<String, u32> = masters
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), u32::try_from(i).unwrap_or(0)))
        .collect();
    report.masters.clone_from(&masters);

    let remap = |formid: u32, plugin: &Plugin| -> u32 {
        let (origin, object_id) = origin_of(formid, plugin);
        let idx = master_index
            .get(&origin.to_lowercase())
            .copied()
            .unwrap_or(0);
        (idx << 24) | object_id
    };
    let resolver_formid = |origin: &str, object_id: u32| -> u32 {
        let idx = master_index
            .get(&origin.to_lowercase())
            .copied()
            .unwrap_or(0);
        (idx << 24) | object_id
    };

    // Pass 2: merge each record in resolver space and emit.
    let mut records: Vec<OverrideRecord> = Vec::new();
    for (origin, object_id, versions) in &parsed {
        match versions {
            Versions::Leveled(sig, versions) => {
                let mut remapped: Vec<(String, lvli::LeveledList)> = Vec::new();
                for (name, list) in versions {
                    let Some(plugin) = by_name.get(&name.to_lowercase()) else {
                        continue;
                    };
                    let mut rl = list.clone();
                    for e in &mut rl.entries {
                        let new_ref = remap(e.reference, plugin);
                        e.reference = new_ref;
                        if let Some(slot) = e.raw_lvlo.get_mut(4..8) {
                            slot.copy_from_slice(&new_ref.to_le_bytes());
                        }
                    }
                    remapped.push((name.clone(), rl));
                }
                let Some((_, base)) = remapped.first().cloned() else {
                    continue;
                };
                let overrides: Vec<(&lvli::LeveledList, lvli::Tags)> = remapped
                    .iter()
                    .skip(1)
                    .map(|(name, l)| (l, tags_for(ml.as_ref(), name)))
                    .collect();
                let merged = lvli::merge(&base, &overrides);
                records.push(OverrideRecord {
                    signature: *sig,
                    form_id: resolver_formid(origin, *object_id),
                    flags: 0,
                    subrecords: lvli::emit_subrecords(&merged),
                });
                report.leveled_merged += 1;
            }
            Versions::Form(versions) => {
                let mut remapped: Vec<formlist::FormList> = Vec::new();
                for (name, list) in versions {
                    let Some(plugin) = by_name.get(&name.to_lowercase()) else {
                        continue;
                    };
                    let mut fl = list.clone();
                    for id in &mut fl.entries {
                        *id = remap(*id, plugin);
                    }
                    remapped.push(fl);
                }
                let Some(base) = remapped.first().cloned() else {
                    continue;
                };
                let refs: Vec<&formlist::FormList> = remapped.iter().skip(1).collect();
                let merged = formlist::merge(&base, &refs);
                records.push(OverrideRecord {
                    signature: formlist::FLST,
                    form_id: resolver_formid(origin, *object_id),
                    flags: 0,
                    subrecords: formlist::emit_subrecords(&merged),
                });
                report.formlist_merged += 1;
            }
        }
    }

    // UDR fixes — ONLY for the user's own mod plugins, never the base game or
    // its official DLCs. Emitted WITHOUT remapping only when the plugin's
    // master list is a prefix of the resolver's (base refs keep index 0).
    let mod_plugins: std::collections::BTreeSet<String> = plan
        .mods
        .iter()
        .flat_map(|m| m.plugins.iter().map(|p| p.to_lowercase()))
        .collect();
    for p in &plugins {
        if p.meta.masters.is_empty() || !mod_plugins.contains(&p.meta.name.to_lowercase()) {
            continue;
        }
        let prefix_ok = p
            .meta
            .masters
            .iter()
            .enumerate()
            .all(|(i, m)| masters.get(i).is_some_and(|rm| rm.eq_ignore_ascii_case(m)));
        if !prefix_ok {
            continue;
        }
        let carriers: Vec<Option<&Plugin>> = p
            .meta
            .masters
            .iter()
            .map(|m| by_name.get(&m.to_lowercase()).copied())
            .collect();
        let fixes = concierge_esp::clean::udr_fixes(p, &carriers)
            .map_err(|e| Error::Other(e.to_string()))?;
        for f in fixes {
            records.push(f);
            report.udr_fixed += 1;
        }
    }

    report.resolver_records = records.len();
    let bytes = resolver_bytes_with("concierge", &masters, &records)
        .map_err(|e| Error::Other(e.to_string()))?;
    let out = repo.state_dir().join("ConciergeResolver.esp");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Other(e.to_string()))?;
    }
    std::fs::write(&out, &bytes).map_err(|e| Error::Other(e.to_string()))?;
    let back = Plugin::read(&out).map_err(|e| Error::Other(format!("resolver invalid: {e}")))?;
    if back.meta.is_esl {
        // must be a plain ESP so it loads last and wins (see writer note)
        return Err(Error::Other(
            "resolver is ESL-flagged; must be a plain ESP".into(),
        ));
    }
    if back.meta.form_version != 131 {
        return Err(Error::Other("resolver has wrong form version".into()));
    }
    report.resolver = out;
    Ok(report)
}

fn find_record<'a>(
    plugin: &'a Plugin,
    sig: [u8; 4],
    origin: &str,
    object_id: u32,
) -> Option<&'a RecordRef> {
    plugin.records.iter().find(|r| {
        r.signature == sig
            && r.object_id() == object_id
            && origin_of(r.form_id, plugin).0.eq_ignore_ascii_case(origin)
    })
}

fn load_sortrules(repo: &Repo, kind: &str) -> Option<SortRules> {
    let repo_name = match kind {
        "fallout4" => "fallout4",
        "skyrimse" => "skyrimspecialedition",
        _ => return None,
    };
    let path = repo.sortdata_dir().join(format!("{repo_name}.yaml"));
    let text = std::fs::read_to_string(path).ok()?;
    SortRules::parse(&text).ok()
}

fn tags_for(ml: Option<&SortRules>, plugin: &str) -> lvli::Tags {
    let mut tags = lvli::Tags::default();
    if let Some(ml) = ml {
        for meta in ml.for_plugin(plugin) {
            for t in &meta.tag {
                match t.name().to_ascii_lowercase().as_str() {
                    "delev" => tags.delev = true,
                    "relev" => tags.relev = true,
                    _ => {}
                }
            }
        }
    }
    tags
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod props {
    use super::origin_of_parts;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        /// The invariant reconcile's FormID remap relies on: resolving a formid
        /// to (origin, object_id) in one plugin's master space, then re-encoding
        /// under the resolver's master table, round-trips the SAME identity.
        #[test]
        fn formid_identity_roundtrips_into_resolver_space(
            masters in proptest::collection::vec("[a-z]{1,6}\\.es[mp]", 1..40),
            self_name in "[a-z]{1,6}\\.esp",
            object_id in 0u32..0x00FF_FFFF,
            src_idx in 0usize..48,
        ) {
            // distinct master names (a real master table has no duplicates)
            let mut seen = std::collections::BTreeSet::new();
            let masters: Vec<String> =
                masters.into_iter().filter(|m| seen.insert(m.to_lowercase())).collect();
            prop_assume!(!masters.is_empty());

            // a source formid pointing at src_idx (or the plugin itself)
            let src_formid = (u32::try_from(src_idx).unwrap() << 24) | object_id;
            let (origin, oid) = origin_of_parts(src_formid, &masters, &self_name);
            prop_assert_eq!(oid, object_id);

            // the resolver's master table must contain the origin to remap it
            let resolver_masters: Vec<String> = {
                let mut m = masters;
                if !m.iter().any(|x| x == &origin) {
                    m.push(origin.clone());
                }
                m
            };
            let ri = resolver_masters.iter().position(|x| x == &origin).unwrap();
            prop_assume!(ri < 256); // a byte-wide master index
            let remapped = (u32::try_from(ri).unwrap() << 24) | oid;

            // reading the remapped formid back in resolver space yields the same
            // (origin, object_id)
            let (origin2, oid2) = origin_of_parts(remapped, &resolver_masters, &self_name);
            prop_assert_eq!(origin2, origin);
            prop_assert_eq!(oid2, object_id);
        }
    }
}
