//! Nix-style version control for the declaration.
//!
//! Each rebuild (realize) snapshots the profile's `manifest.toml` as an
//! immutable, monotonically-numbered **generation** — exactly like NixOS system
//! generations. You can list the history and roll back to any prior declaration.
//! This is distinct from the in-memory per-edit undo stack: generations are
//! per-rebuild, persistent, and rollback-able.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, IoCtx as _, Result};
use crate::repo::Repo;

/// A recorded declaration generation.
#[derive(Debug, Clone)]
pub struct Generation {
    /// Monotonic generation number (1-based).
    pub number: u64,
    /// Seconds since the Unix epoch when it was recorded.
    pub created: u64,
    /// The plan hash realised from this declaration (empty if unknown).
    pub plan_hash: String,
}

fn gens_root(repo: &Repo) -> PathBuf {
    repo.state_dir().join("generations")
}

fn parse_meta(dir: &Path) -> Option<Generation> {
    let number = dir.file_name()?.to_str()?.parse::<u64>().ok()?;
    let meta = std::fs::read_to_string(dir.join("meta.txt")).unwrap_or_default();
    let mut created = 0;
    let mut plan_hash = String::new();
    for line in meta.lines() {
        if let Some(v) = line.strip_prefix("created=") {
            created = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("plan_hash=") {
            v.trim().clone_into(&mut plan_hash);
        }
    }
    Some(Generation {
        number,
        created,
        plan_hash,
    })
}

/// Snapshot `manifest_text` as the next generation. Idempotent-ish: if the newest
/// generation already holds identical text, no new generation is created (a
/// rebuild that changed nothing shouldn't spam history).
pub fn snapshot(repo: &Repo, manifest_text: &str, plan_hash: &str) -> Result<Generation> {
    let root = gens_root(repo);
    let existing = list(repo);
    if let Some(latest) = existing.first() {
        let prev = read(repo, latest.number).unwrap_or_default();
        if prev == manifest_text {
            return Ok(latest.clone());
        }
    }
    let number = existing.first().map_or(1, |g| g.number + 1);
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dir = root.join(number.to_string());
    std::fs::create_dir_all(&dir).ctx(&dir)?;
    std::fs::write(dir.join("manifest.toml"), manifest_text).ctx(&dir)?;
    let meta = format!("created={created}\nplan_hash={plan_hash}\n");
    std::fs::write(dir.join("meta.txt"), meta).ctx(&dir)?;
    Ok(Generation {
        number,
        created,
        plan_hash: plan_hash.to_owned(),
    })
}

/// All recorded generations, newest first.
#[must_use]
pub fn list(repo: &Repo) -> Vec<Generation> {
    let mut gens: Vec<Generation> = std::fs::read_dir(gens_root(repo))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| parse_meta(&e.path()))
        .collect();
    gens.sort_by_key(|g| std::cmp::Reverse(g.number));
    gens
}

/// The `manifest.toml` text recorded in generation `number`.
pub fn read(repo: &Repo, number: u64) -> Result<String> {
    let path = gens_root(repo)
        .join(number.to_string())
        .join("manifest.toml");
    std::fs::read_to_string(&path).ctx(&path)
}

/// Roll the profile's declaration back to generation `number`: overwrite the
/// live `manifest.toml` with that generation's text. Returns the restored text.
/// (Snapshotting the *current* declaration first, if desired, is the caller's
/// job — a rollback is itself an edit that the next rebuild will record.)
pub fn rollback(repo: &Repo, number: u64) -> Result<String> {
    let text = read(repo, number)?;
    let live = repo.profile.join("manifest.toml");
    if !live.exists() {
        return Err(Error::Other(format!(
            "profile has no manifest.toml at {}",
            live.display()
        )));
    }
    std::fs::write(&live, &text).ctx(&live)?;
    Ok(text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_numbers_and_dedupes() {
        let tmp = std::env::temp_dir().join(format!("concierge-gens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("profile")).unwrap();
        std::fs::write(tmp.join("profile/manifest.toml"), "x").unwrap();
        let repo = Repo::at(&tmp.join("profile"));

        let g1 = snapshot(&repo, "a = 1\n", "h1").unwrap();
        assert_eq!(g1.number, 1);
        // identical text → no new generation
        let g1b = snapshot(&repo, "a = 1\n", "h1").unwrap();
        assert_eq!(g1b.number, 1);
        let g2 = snapshot(&repo, "a = 2\n", "h2").unwrap();
        assert_eq!(g2.number, 2);

        let gens = list(&repo);
        assert_eq!(gens.len(), 2);
        assert_eq!(gens[0].number, 2, "newest first");
        assert_eq!(read(&repo, 1).unwrap(), "a = 1\n");

        let restored = rollback(&repo, 1).unwrap();
        assert_eq!(restored, "a = 1\n");
        assert_eq!(
            std::fs::read_to_string(tmp.join("profile/manifest.toml")).unwrap(),
            "a = 1\n"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
