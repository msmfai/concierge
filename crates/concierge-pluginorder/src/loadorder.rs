//! Native load-order sort — a stable topological sort, no third-party engine.
//!
//! Reads the community CC0 sort-rule data (the public masterlists cached in
//! `state/sortdata/`): master-file edges (from plugin headers), the rules'
//! `after`/`req` and group ordering, and the overlap tie-break (the plugin
//! overriding fewer records loads later). The result is advisory — the manifest
//! stays the ordering authority — plus surfaced dirty/tag metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use concierge::error::{Error, IoCtx, Result};
use concierge::plan::Plan;
use concierge::repo::Repo;
use concierge_esp::reader::Plugin;

use crate::sortrules::SortRules;

/// `SortRules` metadata-syntax branches, newest first.
const BRANCHES: &[&str] = &["v0.26", "v0.21"];

#[derive(Debug, Default)]
pub struct SortReport {
    pub current: Vec<String>,
    pub suggested: Vec<String>,
    /// plugins whose masterlist tags apply (name, tags).
    pub tags: Vec<(String, Vec<String>)>,
    /// plugins the masterlist flags dirty (CRC-matched when possible).
    pub dirty: Vec<String>,
}

fn add_edge(before: &mut [BTreeSet<usize>], a: usize, b: usize) {
    if a != b {
        if let Some(set) = before.get_mut(b) {
            set.insert(a);
        }
    }
}

fn sortrules_repo(kind: &str) -> Option<&'static str> {
    match kind {
        "fallout4" => Some("fallout4"),
        "skyrimse" => Some("skyrimspecialedition"),
        _ => None,
    }
}

fn fetch_to(path: &Path, url: &str) -> Result<()> {
    let resp = ureq::get(url)
        .set("User-Agent", "concierge-prototype/0.1")
        .call()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    let mut out = std::fs::File::create(path).ctx(path)?;
    std::io::copy(&mut resp.into_reader(), &mut out).ctx(path)?;
    out.flush().ctx(path)?;
    Ok(())
}

fn ensure_sortrules(repo: &Repo, repo_name: &str) -> Result<PathBuf> {
    let path = repo.sortdata_dir().join(format!("{repo_name}.yaml"));
    if path.exists() {
        return Ok(path);
    }
    let mut last_err = None;
    for branch in BRANCHES {
        let url =
            format!("https://raw.githubusercontent.com/loot/{repo_name}/{branch}/masterlist.yaml");
        match fetch_to(&path, &url) {
            Ok(()) => return Ok(path),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| Error::Other("no masterlist branch fetched".into())))
}

/// The plugins in the plan's load order (base + `.ccc` + enabled mods), as
/// (filename, parsed) pairs for the ones that exist on disk.
fn load_order_plugins(plan: &Plan) -> Result<Vec<(String, Plugin)>> {
    let plugins = crate::read_load_order(plan)?;
    Ok(plugins
        .into_iter()
        .map(|p| (p.meta.name.clone(), p))
        .collect())
}

#[allow(clippy::too_many_lines, clippy::indexing_slicing)] // indices are loop-bounded over equal-length vecs
pub fn sort(repo: &Repo, plan: &Plan) -> Result<SortReport> {
    let repo_name = sortrules_repo(&plan.game.kind).ok_or_else(|| {
        Error::Other(format!(
            "sort: '{}' is not a LOOT-supported game",
            plan.game.kind
        ))
    })?;
    let ml_path = ensure_sortrules(repo, repo_name)?;
    let ml = SortRules::parse(&std::fs::read_to_string(&ml_path).ctx(&ml_path)?)
        .map_err(|e| Error::Other(format!("masterlist parse: {e}")))?;

    let plugins = load_order_plugins(plan)?;
    let current: Vec<String> = plugins.iter().map(|(n, _)| n.clone()).collect();
    let index: BTreeMap<String, usize> = current
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), i))
        .collect();
    let n = plugins.len();

    // override counts for the overlap tie-break: a record is an override when
    // its FormID's master-index byte points into the plugin's master list.
    let overrides: Vec<usize> = plugins
        .iter()
        .map(|(_, p)| {
            let nm = p.meta.masters.len();
            p.records
                .iter()
                .filter(|r| usize::from(r.master_index()) < nm)
                .count()
        })
        .collect();

    // edges[b] = set of a that must load before b
    let mut before: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];

    // 1. master-file edges: every master loads before its dependent
    for (bi, (_, p)) in plugins.iter().enumerate() {
        for m in &p.meta.masters {
            if let Some(&ai) = index.get(&m.to_lowercase()) {
                add_edge(&mut before, ai, bi);
            }
        }
    }

    // 1b. master-block rule: every master-flagged plugin (ESM or ESL light
    // master) loads before every non-master (plain ESP). The engine enforces
    // this regardless of plugins.txt position, so the resolved order must too.
    let is_master: Vec<bool> = plugins
        .iter()
        .map(|(_, p)| p.meta.is_esm || p.meta.is_esl)
        .collect();
    for bi in 0..n {
        if is_master.get(bi).copied().unwrap_or(false) {
            continue;
        }
        for (ai, master) in is_master.iter().enumerate() {
            if *master {
                add_edge(&mut before, ai, bi);
            }
        }
    }

    // 2. masterlist after/req edges + group membership
    let mut group_of: Vec<String> = vec!["default".to_owned(); n];
    for (bi, (name, _)) in plugins.iter().enumerate() {
        for meta in ml.for_plugin(name) {
            if let Some(g) = &meta.group {
                group_of[bi].clone_from(g);
            }
            for r in meta.after.iter().chain(meta.req.iter()) {
                if let Some(&ai) = index.get(&r.name().to_lowercase()) {
                    add_edge(&mut before, ai, bi);
                }
            }
        }
    }

    // 3. group edges: earlier groups load before later ones (group DAG)
    let group_rank = group_ranks(&ml);
    for bi in 0..n {
        for ai in 0..n {
            if ai == bi {
                continue;
            }
            if let (Some(ra), Some(rb)) =
                (group_rank.get(&group_of[ai]), group_rank.get(&group_of[bi]))
            {
                if ra < rb {
                    add_edge(&mut before, ai, bi);
                }
            }
        }
    }

    let suggested = toposort(n, &before, &current, &overrides);

    // dirty + tags
    let mut report = SortReport {
        current,
        suggested,
        ..SortReport::default()
    };
    for (name, plugin) in &plugins {
        let metas = ml.for_plugin(name);
        let mut tags: Vec<String> = Vec::new();
        let mut dirty = false;
        for meta in &metas {
            for t in &meta.tag {
                tags.push(t.name().to_owned());
            }
            if !meta.dirty.is_empty() {
                // CRC-match when the masterlist entry provides one
                let crc = crc32(plugin);
                if meta
                    .dirty
                    .iter()
                    .any(|d| d.crc.is_none_or(|c| c == u64::from(crc)))
                {
                    dirty = true;
                }
            }
        }
        if !tags.is_empty() {
            report.tags.push((name.clone(), tags));
        }
        if dirty {
            report.dirty.push(name.clone());
        }
    }
    Ok(report)
}

/// Apply the resolved load order: rewrite the plan's `plugins.txt` so its
/// entries sit in the natively-sorted order. Optionally force one plugin to the
/// very end (the generated resolver, which must load last to win). Returns the
/// plugins.txt path and the written order (base master excluded, as it is
/// implicit). This is Rung 1 — the Topology *applied*, not just advised.
pub fn apply_order(
    repo: &Repo,
    plan: &Plan,
    force_last: Option<&str>,
) -> Result<(std::path::PathBuf, Vec<String>)> {
    let report = sort(repo, plan)?;
    let base = match plan.game.kind.as_str() {
        "fallout4" => "Fallout4.esm",
        "skyrimse" => "Skyrim.esm",
        other => return Err(Error::Other(format!("not a Bethesda game: '{other}'"))),
    };
    // suggested order minus the base master (implicit, never listed)
    let mut order: Vec<String> = report
        .suggested
        .into_iter()
        .filter(|n| !n.eq_ignore_ascii_case(base))
        .collect();
    if let Some(last) = force_last {
        order.retain(|n| !n.eq_ignore_ascii_case(last));
        // only re-add it if it's actually deployed in the instance
        let data = std::path::PathBuf::from(plan.game_dir()).join("Data");
        if data.join(last).exists() {
            order.push(last.to_owned());
        }
    }
    let path = plugins_txt_path(plan)
        .ok_or_else(|| Error::Other("plan has no plugins.txt config".into()))?;
    let mut body = String::from(
        "# GENERATED BY concierge — do not hand-edit; change manifest.toml and re-realize.\n",
    );
    for name in &order {
        body.push('*');
        body.push_str(name);
        body.push('\n');
    }
    std::fs::write(&path, body).ctx(&path)?;
    Ok((path, order))
}

/// The plugins.txt path declared by the plan, if any.
fn plugins_txt_path(plan: &Plan) -> Option<std::path::PathBuf> {
    plan.configs
        .iter()
        .map(|c| std::path::PathBuf::from(&c.path))
        .find(|p| p.file_name().is_some_and(|n| n == "plugins.txt"))
}

fn crc32(plugin: &Plugin) -> u32 {
    std::fs::read(&plugin.path).map_or(0, |bytes| crc32fast::hash(&bytes))
}

/// Group name -> rank (0 = earliest). Groups form a DAG via `after`; we
/// longest-path rank them. Unknown groups get a middle rank.
fn depth(
    name: &str,
    after: &BTreeMap<String, Vec<String>>,
    rank: &mut BTreeMap<String, usize>,
    stack: &mut Vec<String>,
) -> usize {
    if let Some(r) = rank.get(name) {
        return *r;
    }
    if stack.iter().any(|s| s == name) {
        return 0; // cycle guard
    }
    stack.push(name.to_owned());
    let d = after
        .get(name)
        .into_iter()
        .flatten()
        .map(|a| depth(a, after, rank, stack) + 1)
        .max()
        .unwrap_or(0);
    stack.pop();
    rank.insert(name.to_owned(), d);
    d
}

fn group_ranks(ml: &SortRules) -> BTreeMap<String, usize> {
    let mut after: BTreeMap<String, Vec<String>> = BTreeMap::new();
    after.entry("default".to_owned()).or_default();
    for g in &ml.groups {
        let e = after.entry(g.name.clone()).or_default();
        for a in &g.after {
            e.push(a.name().to_owned());
        }
    }
    // rank = longest chain of `after` predecessors (memoized DFS)
    let mut rank: BTreeMap<String, usize> = BTreeMap::new();
    let names: Vec<String> = after.keys().cloned().collect();
    for name in names {
        let mut stack = Vec::new();
        depth(&name, &after, &mut rank, &mut stack);
    }
    rank
}

/// Stable topological sort. Among ready nodes, pick by: fewest incoming order
/// changes first — concretely, keep existing relative order, breaking genuine
/// ties with the overlap heuristic (fewer overrides loads later => more
/// overrides first) then filename.
#[allow(clippy::indexing_slicing)] // indices are 0..n over equal-length vecs
fn toposort(
    n: usize,
    before: &[BTreeSet<usize>],
    names: &[String],
    overrides: &[usize],
) -> Vec<String> {
    let mut indeg = vec![0usize; n];
    for b in 0..n {
        indeg[b] = before[b].len();
    }
    let mut placed = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        // ready = indeg 0, not placed
        let mut ready: Vec<usize> = (0..n).filter(|&i| !placed[i] && indeg[i] == 0).collect();
        if ready.is_empty() {
            // cycle: append remaining in existing order (defensive)
            for i in 0..n {
                if !placed[i] {
                    out.push(names[i].clone());
                    placed[i] = true;
                }
            }
            break;
        }
        // stable pick: existing index, then more-overrides-first, then name
        ready.sort_by(|&a, &b| {
            a.cmp(&b)
                .then(overrides[b].cmp(&overrides[a]))
                .then(names[a].to_lowercase().cmp(&names[b].to_lowercase()))
        });
        let pick = ready[0];
        placed[pick] = true;
        out.push(names[pick].clone());
        for b in 0..n {
            if !placed[b] && before[b].contains(&pick) {
                indeg[b] = indeg[b].saturating_sub(1);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod props {
    use super::{add_edge, toposort};
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    /// A random plugin set: master flags, forward-only dependency edges (i<j,
    /// so acyclic), and override counts. Returns `(n, is_master, edges, overrides)`.
    type Graph = (usize, Vec<bool>, Vec<(usize, usize)>, Vec<usize>);

    fn graph() -> impl Strategy<Value = Graph> {
        (2usize..8).prop_flat_map(|n| {
            let masters = proptest::collection::vec(any::<bool>(), n);
            let overrides = proptest::collection::vec(0usize..50, n);
            // candidate forward edges i<j
            let pairs: Vec<(usize, usize)> = (0..n)
                .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
                .collect();
            let edges =
                proptest::collection::vec(any::<bool>(), pairs.len()).prop_map(move |keep| {
                    pairs
                        .iter()
                        .zip(keep)
                        .filter(|(_, k)| *k)
                        .map(|(e, _)| *e)
                        .collect::<Vec<_>>()
                });
            (Just(n), masters, edges, overrides)
        })
    }

    /// A dependency edge a→b is only realistic when it can't contradict the
    /// master-block rule: a plain ESP is never a master of a master file, so an
    /// edge into a master must originate from a master. Drop the impossible ones
    /// (real LOOT would flag such a cycle; we don't model contradictory input).
    fn realistic(edges: &[(usize, usize)], is_master: &[bool]) -> Vec<(usize, usize)> {
        edges
            .iter()
            .copied()
            .filter(|&(a, b)| is_master[a] || !is_master[b])
            .collect()
    }

    fn build(
        n: usize,
        is_master: &[bool],
        edges: &[(usize, usize)],
        master_block: bool,
    ) -> Vec<BTreeSet<usize>> {
        let mut before = vec![BTreeSet::new(); n];
        for &(a, b) in edges {
            add_edge(&mut before, a, b);
        }
        if master_block {
            for b in 0..n {
                if is_master[b] {
                    continue;
                }
                for (a, m) in is_master.iter().enumerate() {
                    if *m {
                        add_edge(&mut before, a, b);
                    }
                }
            }
        }
        before
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        #[test]
        fn toposort_is_valid_complete_and_deterministic(
            (n, is_master, edges, overrides) in graph()
        ) {
            let names: Vec<String> = (0..n).map(|i| format!("p{i:02}")).collect();
            let before = build(n, &is_master, &edges, false);
            let out = toposort(n, &before, &names, &overrides);
            // complete permutation
            prop_assert_eq!(out.len(), n);
            let set: BTreeSet<&String> = out.iter().collect();
            prop_assert_eq!(set.len(), n);
            // every dependency edge respected: a precedes b
            let pos: std::collections::HashMap<&str, usize> =
                out.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
            for &(a, b) in &edges {
                prop_assert!(pos[names[a].as_str()] < pos[names[b].as_str()]);
            }
            // deterministic
            prop_assert_eq!(&out, &toposort(n, &before, &names, &overrides));
        }

        #[test]
        fn master_block_rule_holds(
            (n, is_master, edges, overrides) in graph()
        ) {
            let names: Vec<String> = (0..n).map(|i| format!("p{i:02}")).collect();
            let edges = realistic(&edges, &is_master);
            let before = build(n, &is_master, &edges, true);
            let out = toposort(n, &before, &names, &overrides);
            let pos: std::collections::HashMap<&str, usize> =
                out.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
            let last_master = (0..n).filter(|&i| is_master[i]).map(|i| pos[names[i].as_str()]).max();
            let first_plain = (0..n).filter(|&i| !is_master[i]).map(|i| pos[names[i].as_str()]).min();
            if let (Some(lm), Some(fp)) = (last_master, first_plain) {
                prop_assert!(lm < fp, "every ESM/ESL must precede every plain ESP");
            }
        }

        #[test]
        fn reindexing_by_output_is_idempotent(
            (n, is_master, edges, overrides) in graph()
        ) {
            // sort(sort) == sort: re-label indices in the resolved order and
            // rebuild the edges; a stable topo sort must reproduce that order.
            let names: Vec<String> = (0..n).map(|i| format!("p{i:02}")).collect();
            let edges = realistic(&edges, &is_master);
            let before = build(n, &is_master, &edges, true);
            let out1 = toposort(n, &before, &names, &overrides);
            let perm: Vec<usize> = out1.iter()
                .map(|s| names.iter().position(|x| x == s).unwrap())
                .collect();
            // new index k corresponds to old index perm[k]
            let old_to_new: std::collections::HashMap<usize, usize> =
                perm.iter().enumerate().map(|(k, &old)| (old, k)).collect();
            let names2: Vec<String> = (0..n).map(|k| names[perm[k]].clone()).collect();
            let overrides2: Vec<usize> = (0..n).map(|k| overrides[perm[k]]).collect();
            // before2[b] (new index) = the new indices of old-node perm[b]'s
            // predecessors — preserve edge direction (a -> perm[b] becomes
            // new_a -> b).
            let mut before2 = vec![BTreeSet::new(); n];
            for b in 0..n {
                for &a in &before[perm[b]] {
                    before2[b].insert(old_to_new[&a]);
                }
            }
            let out2 = toposort(n, &before2, &names2, &overrides2);
            prop_assert_eq!(out1, out2);
        }
    }
}
