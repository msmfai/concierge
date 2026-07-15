//! Save-game backup + versioning.
//!
//! Before a rebuild (realize), snapshot the game's save directory to a
//! versioned backup so a bad mod can't cause save-game data loss. This is
//! **copy-only** — the user's real saves are never moved or deleted, only read.
//! Each snapshot is a new generation under `<repo backup>/saves/<generation>/`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, IoCtx as _, Result};
use crate::plan::Plan;
use crate::repo::Repo;

/// A completed save snapshot.
#[derive(Debug, Clone)]
pub struct SaveBackup {
    /// Generation id (seconds since the Unix epoch — human-sortable).
    pub generation: String,
    /// Where the snapshot was written.
    pub dir: PathBuf,
    /// Number of save files copied.
    pub files: usize,
}

fn saves_root(repo: &Repo) -> PathBuf {
    repo.backup_dir().join("saves")
}

/// Snapshot the plan's save directory to a new versioned backup. Returns `None`
/// if the plan declares no saves directory, or it doesn't exist / is empty.
/// Copies only; the originals are never modified.
pub fn backup(repo: &Repo, plan: &Plan) -> Result<Option<SaveBackup>> {
    let Some(src) = plan.game.saves.as_deref().map(Path::new) else {
        return Ok(None);
    };
    if !src.is_dir() {
        return Ok(None);
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let generation = secs.to_string();
    let dir = saves_root(repo).join(&generation);
    let files = copy_tree(src, &dir)?;
    if files == 0 {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    }
    Ok(Some(SaveBackup {
        generation,
        dir,
        files,
    }))
}

/// Recursively copy `src` into `dest`, returning the number of files copied.
fn copy_tree(src: &Path, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest).ctx(dest)?;
    let mut n = 0;
    for entry in std::fs::read_dir(src).ctx(src)? {
        let entry = entry.ctx(src)?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let ft = entry.file_type().ctx(&from)?;
        if ft.is_dir() {
            n += copy_tree(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).ctx(&from)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Restore a save-backup generation over the live saves directory. Copies the
/// generation's files into the plan's saves dir (back up first via [`backup`]).
/// Returns the number of files restored.
pub fn restore(repo: &Repo, plan: &Plan, generation: &str) -> Result<usize> {
    let dest = plan
        .game
        .saves
        .as_deref()
        .map(Path::new)
        .ok_or_else(|| Error::Other("plan declares no saves directory".to_owned()))?;
    let src = saves_root(repo).join(generation);
    if !src.is_dir() {
        return Err(Error::Other(format!(
            "no save-backup generation '{generation}'"
        )));
    }
    copy_tree(&src, dest)
}

/// Existing save-backup generations, newest first.
#[must_use]
pub fn list(repo: &Repo) -> Vec<String> {
    let mut gens: Vec<String> = std::fs::read_dir(saves_root(repo))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    gens.sort_unstable();
    gens.reverse();
    gens
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn copy_tree_counts_and_nests() {
        let tmp = std::env::temp_dir().join(format!("concierge-saves-test-{}", std::process::id()));
        let src = tmp.join("src");
        let sub = src.join("Character1");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(src.join("save1.ess"), b"a").unwrap();
        std::fs::write(sub.join("save2.ess"), b"bb").unwrap();
        let dest = tmp.join("dest");
        let n = copy_tree(&src, &dest).unwrap();
        assert_eq!(n, 2);
        assert!(dest.join("save1.ess").is_file());
        assert!(dest.join("Character1/save2.ess").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
