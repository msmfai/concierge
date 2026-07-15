//! Profile lockfile — the resolved-state pin, distinct from the declaration.
//!
//! Like `Cargo.lock` / `flake.lock` / Terraform state: the declaration
//! (`manifest.toml`) says *what you want*; the lock (`concierge.lock`) records
//! *exactly what was resolved* — every mod's pinned version + content hash and
//! the resolved load order, plus the plan hash. That makes a profile
//! byte-reproducible and shareable: someone else with the same lock realizes
//! the identical set. Written on realize; JSON (like `flake.lock`).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoCtx as _, Result};
use crate::plan::Plan;
use crate::repo::Repo;

/// A resolved, pinned snapshot of a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lock {
    /// Hash of the plan this lock resolves (ties it to a declaration state).
    pub plan_hash: String,
    /// Seconds since the Unix epoch when written.
    pub created: u64,
    /// The resolved plugin load order.
    pub load_order: Vec<String>,
    /// Every resolved mod, pinned.
    pub mods: Vec<LockedMod>,
}

/// One pinned mod in the lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedMod {
    pub name: String,
    pub version: String,
    /// Content hash pin; empty if the mod was not yet pinned at lock time.
    pub md5: String,
    pub file: String,
}

fn lock_path(repo: &Repo) -> PathBuf {
    repo.profile.join("concierge.lock")
}

/// Write the resolved lock for `plan` to `concierge.lock`. Returns it.
pub fn write(repo: &Repo, plan: &Plan) -> Result<Lock> {
    let load_order = plan
        .mods
        .iter()
        .flat_map(|m| m.plugins.iter().cloned())
        .collect();
    let mods = plan
        .mods
        .iter()
        .map(|m| LockedMod {
            name: m.name.clone(),
            version: m.version.clone(),
            md5: m.md5.clone().unwrap_or_default(),
            file: m.file.clone(),
        })
        .collect();
    let lock = Lock {
        plan_hash: plan.hash()?,
        created: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        load_order,
        mods,
    };
    let text = serde_json::to_string_pretty(&lock)
        .map_err(|e| Error::Other(format!("serialize lock: {e}")))?;
    let path = lock_path(repo);
    std::fs::write(&path, text).ctx(&path)?;
    Ok(lock)
}

/// Read the profile's lock, if one exists and parses.
#[must_use]
pub fn read(repo: &Repo) -> Option<Lock> {
    let text = std::fs::read_to_string(lock_path(repo)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether the lock (if any) matches the current plan — i.e. the realised pin is
/// still the one the declaration resolves to.
#[must_use]
pub fn in_sync(repo: &Repo, plan: &Plan) -> bool {
    match (read(repo), plan.hash().ok()) {
        (Some(lock), Some(h)) => lock.plan_hash == h,
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn lock_round_trips() {
        let lock = Lock {
            plan_hash: "abc123".to_owned(),
            created: 1000,
            load_order: vec!["A.esp".to_owned(), "B.esp".to_owned()],
            mods: vec![LockedMod {
                name: "SkyUI".to_owned(),
                version: "5.2".to_owned(),
                md5: "deadbeef".to_owned(),
                file: "skyui.7z".to_owned(),
            }],
        };
        let text = serde_json::to_string_pretty(&lock).unwrap();
        let back: Lock = serde_json::from_str(&text).unwrap();
        assert_eq!(back.plan_hash, "abc123");
        assert_eq!(back.mods.len(), 1);
        assert_eq!(back.mods[0].md5, "deadbeef");
        assert_eq!(back.load_order, vec!["A.esp", "B.esp"]);
    }
}
