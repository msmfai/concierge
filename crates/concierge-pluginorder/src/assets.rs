//! File-level (asset-path) conflict detection — the class the record matrix
//! misses. Two mods shipping the same `textures/…/x.dds` (loose or inside a
//! BA2) silently last-wins; this surfaces it. Report-only: binary assets aren't mergeable, so last-wins is
//! correct — this feeds the Synthesize rung (which may reorder or pick).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use concierge::error::{Error, Result};
use concierge::plan::Plan;
use concierge::repo::Repo;

/// A conflict on one asset path across the load order.
#[derive(Debug, Clone)]
pub struct AssetConflict {
    pub path: String,
    /// Providing mods, in load order.
    pub providers: Vec<String>,
    /// The mod that wins (last in load order).
    pub winner: String,
    /// True when every provider's bytes are identical (a no-op override).
    pub benign: bool,
}

#[derive(Debug, Clone)]
enum ProviderKind {
    Loose(PathBuf),
    Packed { archive: PathBuf, entry: String },
}

#[derive(Debug, Clone)]
struct Provider {
    mod_name: String,
    kind: ProviderKind,
}

/// Detect asset-path conflicts across the plan's data-root mods. Winner is the
/// last provider in load order; benign conflicts (identical bytes) are flagged.
pub fn asset_conflicts(repo: &Repo, plan: &Plan) -> Result<Vec<AssetConflict>> {
    let mut index: BTreeMap<String, Vec<Provider>> = BTreeMap::new();

    for m in &plan.mods {
        if m.install_root != "data" {
            continue; // asset conflicts live under Data/
        }
        let Some(md5) = &m.md5 else { continue };
        let mut root = repo.build_path(md5);
        if let Some(sub) = &m.subdir {
            root = root.join(sub);
        }
        if !root.is_dir() {
            continue;
        }
        for file in walk(&root) {
            let Some(rel) = file.strip_prefix(&root).ok().and_then(|p| p.to_str()) else {
                continue;
            };
            let ext = file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if ext == "ba2" {
                // packed: its entries are the provided data-relative paths
                if let Ok(bytes) = std::fs::read(&file) {
                    if let Ok(archive) = concierge_ba2::Archive::parse(&bytes) {
                        for entry in archive.entries() {
                            index
                                .entry(normalize(&entry.name))
                                .or_default()
                                .push(Provider {
                                    mod_name: m.name.clone(),
                                    kind: ProviderKind::Packed {
                                        archive: file.clone(),
                                        entry: entry.name.clone(),
                                    },
                                });
                        }
                    }
                }
            } else if ext != "bsa" {
                // a loose asset (skip .bsa — older format, not yet read)
                index.entry(normalize(rel)).or_default().push(Provider {
                    mod_name: m.name.clone(),
                    kind: ProviderKind::Loose(file.clone()),
                });
            }
        }
    }

    let mut conflicts = Vec::new();
    for (path, providers) in index {
        // distinct providing mods, in first-seen (load) order
        let mut mods: Vec<String> = Vec::new();
        for p in &providers {
            if !mods.contains(&p.mod_name) {
                mods.push(p.mod_name.clone());
            }
        }
        if mods.len() < 2 {
            continue;
        }
        let winner = mods.last().cloned().unwrap_or_default();
        let benign = all_identical(&providers)?;
        conflicts.push(AssetConflict {
            path,
            providers: mods,
            winner,
            benign,
        });
    }
    Ok(conflicts)
}

/// Author a resolution for an asset conflict: make `winner_mod` win the path by
/// deploying its bytes as a LOOSE file in the instance `Data/` (loose overrides
/// any BA2 in Fallout 4). Returns the deployed path + the winning bytes' hash.
/// This is a Rung-3 artifact — a *judgment* (which mod should win) the
/// deterministic layer only reports, never makes.
pub fn resolve_by_winner(
    repo: &Repo,
    plan: &Plan,
    path: &str,
    winner_mod: &str,
) -> Result<(PathBuf, u64)> {
    // find the winner's bytes for this path (loose or packed)
    let want = normalize(path);
    let m = plan
        .mods
        .iter()
        .find(|m| m.name == winner_mod)
        .ok_or_else(|| Error::Other(format!("winner '{winner_mod}' not in plan")))?;
    let md5 = m
        .md5
        .as_ref()
        .ok_or_else(|| Error::Other(format!("{winner_mod} unpinned")))?;
    let mut root = repo.build_path(md5);
    if let Some(sub) = &m.subdir {
        root = root.join(sub);
    }
    let bytes = find_bytes(&root, &want)?
        .ok_or_else(|| Error::Other(format!("{winner_mod} does not provide {path}")))?;

    // deploy loose into the instance Data (loose beats BA2)
    let data = PathBuf::from(plan.game_dir()).join("Data");
    let dest = data.join(path.replace('\\', "/"));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Other(e.to_string()))?;
    }
    std::fs::write(&dest, &bytes).map_err(|e| Error::Other(e.to_string()))?;
    Ok((dest, concierge_hash::xxhash64(&bytes)))
}

/// Verify a resolution: the deployed loose file's bytes hash to `expected`
/// (i.e. the winner's content is what's now on disk).
pub fn verify_resolution(deployed: &Path, expected_hash: u64) -> Result<bool> {
    let bytes = std::fs::read(deployed).map_err(|e| Error::Other(e.to_string()))?;
    Ok(concierge_hash::xxhash64(&bytes) == expected_hash)
}

/// Find a normalized data-relative path's bytes under a mod root (loose file or
/// inside any BA2), if present.
fn find_bytes(root: &Path, want: &str) -> Result<Option<Vec<u8>>> {
    for file in walk(root) {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ext == "ba2" {
            if let Ok(raw) = std::fs::read(&file) {
                if let Ok(a) = concierge_ba2::Archive::parse(&raw) {
                    if let Some(e) = a.entries().iter().find(|e| normalize(&e.name) == want) {
                        return Ok(Some(
                            a.extract(&e.name)
                                .map_err(|x| Error::Other(x.to_string()))?,
                        ));
                    }
                }
            }
        } else if let Ok(rel) = file.strip_prefix(root) {
            if rel.to_str().is_some_and(|r| normalize(r) == want) {
                return Ok(Some(
                    std::fs::read(&file).map_err(|e| Error::Other(e.to_string()))?,
                ));
            }
        }
    }
    Ok(None)
}

/// Do all providers of a path yield byte-identical content? (one hash per
/// distinct mod is enough)
fn all_identical(providers: &[Provider]) -> Result<bool> {
    let mut seen_mod: Vec<String> = Vec::new();
    let mut hashes: Vec<u64> = Vec::new();
    for p in providers {
        if seen_mod.contains(&p.mod_name) {
            continue;
        }
        seen_mod.push(p.mod_name.clone());
        let bytes = read_provider(&p.kind)?;
        hashes.push(concierge_hash::xxhash64(&bytes));
    }
    Ok(hashes.windows(2).all(|w| w.first() == w.get(1)))
}

fn read_provider(kind: &ProviderKind) -> Result<Vec<u8>> {
    match kind {
        ProviderKind::Loose(path) => {
            std::fs::read(path).map_err(|e| Error::Other(format!("{}: {e}", path.display())))
        }
        ProviderKind::Packed { archive, entry } => {
            let bytes = std::fs::read(archive)
                .map_err(|e| Error::Other(format!("{}: {e}", archive.display())))?;
            let a =
                concierge_ba2::Archive::parse(&bytes).map_err(|e| Error::Other(e.to_string()))?;
            a.extract(entry).map_err(|e| Error::Other(e.to_string()))
        }
    }
}

/// Recursively list files (not dirs) under `root`.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Case-insensitive path key with `/` and `\` unified.
fn normalize(p: &str) -> String {
    p.chars()
        .map(|c| match c {
            '/' => '\\',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}
