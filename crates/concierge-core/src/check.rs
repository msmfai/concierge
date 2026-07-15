//! Drift detection: owned files vs recorded state, and (optionally) the
//! pristine install vs the committed vanilla inventory.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoCtx, Result};
use crate::plan::Plan;
use crate::realize::target_root;
use crate::repo::{md5_file, Repo};
use crate::state::{parse_key, Realized};

#[derive(Debug, Clone)]
pub enum Drift {
    Missing { key: String, owner: String },
    Modified { key: String, owner: String },
    PristineMissing { rel: String },
    PristineChanged { rel: String },
    PlanMismatch { realized: String, current: String },
}

pub fn check(repo: &Repo, plan: &Plan, vanilla: bool) -> Result<Vec<Drift>> {
    let state = Realized::load(repo)?;
    let mut drift = Vec::new();

    if let Some(realized_hash) = &state.plan_hash {
        let current = plan.hash()?;
        if *realized_hash != current {
            drift.push(Drift::PlanMismatch {
                realized: realized_hash.clone(),
                current,
            });
        }
    }

    for (k, rec) in &state.files {
        let Some((root, rel)) = parse_key(k) else {
            continue;
        };
        let dst = target_root(plan, root)?.join(rel);
        if !dst.exists() {
            drift.push(Drift::Missing {
                key: k.clone(),
                owner: rec.mod_name.clone(),
            });
        } else if md5_file(&dst)? != rec.md5 {
            drift.push(Drift::Modified {
                key: k.clone(),
                owner: rec.mod_name.clone(),
            });
        }
    }

    if vanilla {
        let inv = repo.vanilla_inventory();
        let text = std::fs::read_to_string(&inv).ctx(&inv)?;
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(md5), Some(size), Some(rel)) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(Error::Other(format!("bad inventory line: {line}")));
            };
            let p = PathBuf::from(&plan.game.pristine).join(rel);
            if !p.exists() {
                drift.push(Drift::PristineMissing {
                    rel: rel.to_owned(),
                });
                continue;
            }
            let meta = std::fs::metadata(&p).ctx(&p)?;
            if meta.len().to_string() != size || md5_file(&p)? != md5 {
                drift.push(Drift::PristineChanged {
                    rel: rel.to_owned(),
                });
            }
        }
    }

    Ok(drift)
}

/// Snapshot the pristine install into the per-game vanilla inventory
/// (`md5\tsize\trel`, sorted) — the baseline `check --vanilla` verifies
/// against. One-time setup per game; refuses to overwrite an existing
/// baseline without `force`, because re-blessing a possibly-polluted install
/// must be deliberate.
pub fn write_vanilla_inventory(repo: &Repo, plan: &Plan, force: bool) -> Result<(PathBuf, usize)> {
    let inv = repo.vanilla_inventory();
    if inv.exists() && !force {
        return Err(Error::Other(format!(
            "{} already exists — pass --force to re-bless the CURRENT pristine as the baseline",
            inv.display()
        )));
    }
    let root = PathBuf::from(&plan.game.pristine);
    if !root.is_dir() {
        return Err(Error::Other(format!(
            "pristine install not found: {}",
            root.display()
        )));
    }
    let mut rels = Vec::new();
    walk_pristine(&root, &root, &mut rels)?;
    if rels.is_empty() {
        return Err(Error::Other(format!(
            "pristine install is empty: {}",
            root.display()
        )));
    }
    rels.sort();
    let mut out = String::new();
    for rel in &rels {
        use std::fmt::Write as _;
        let p = root.join(rel);
        let meta = std::fs::metadata(&p).ctx(&p)?;
        let _ = writeln!(out, "{}\t{}\t{rel}", md5_file(&p)?, meta.len());
    }
    std::fs::write(&inv, out).ctx(&inv)?;
    Ok((inv, rels.len()))
}

/// Collect every regular file under `dir` as a `/`-separated path relative to
/// `root`. Follows symlinks (macOS .app bundles contain framework links);
/// skips `.DS_Store` noise so a Finder visit doesn't read as drift.
fn walk_pristine(root: &Path, dir: &Path, rels: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).ctx(dir)? {
        let entry = entry.ctx(dir)?;
        let path = entry.path();
        if entry.file_name().to_string_lossy() == ".DS_Store" {
            continue;
        }
        let meta = std::fs::metadata(&path).ctx(&path)?;
        if meta.is_dir() {
            walk_pristine(root, &path, rels)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| Error::Other(format!("path escapes root: {}", path.display())))?;
            rels.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}
