//! The build phase: pure extraction of store archives into immutable trees.
//!
//! builds/<md5>/ is a function of the archive alone. Every output file is
//! chmod a-w so hardlink-deployed copies cannot drift (the read-only store
//! property, ported to mod files).

use std::path::PathBuf;

use crate::error::{Error, IoCtx, Result};
use crate::plan::Plan;
use crate::repo::Repo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Present(PathBuf),
    Built(PathBuf),
    /// Unpinned mods can't be built (no content address).
    Unpinned,
}

pub fn build_all(repo: &Repo, plan: &Plan) -> Result<Vec<(String, BuildOutcome)>> {
    std::fs::create_dir_all(repo.builds()).ctx(&repo.builds())?;
    plan.mods
        .iter()
        .map(|m| {
            let Some(md5) = &m.md5 else {
                return Ok((m.name.clone(), BuildOutcome::Unpinned));
            };
            let out = repo.build_path(md5);
            if out.exists() {
                return Ok((m.name.clone(), BuildOutcome::Present(out)));
            }
            let archive = repo.store_path(md5, &m.file);
            if !archive.exists() {
                return Err(Error::StoreMiss {
                    name: m.name.clone(),
                    path: archive,
                });
            }
            if let Some(patch) = &m.patch {
                // Wabbajack model: the fetched `file` IS a BSDiff patch — derive
                // the mod's content from a source the user owns. Deterministic,
                // so builds/<md5>/ stays a pure function of (source, patch).
                let from = PathBuf::from(&patch.from);
                let source = std::fs::read(&from).ctx(&from)?;
                let patch_bytes = std::fs::read(&archive).ctx(&archive)?;
                let derived = concierge_patch::apply(&source, &patch_bytes)
                    .map_err(|e| Error::Other(format!("{}: {e}", m.name)))?;
                std::fs::create_dir_all(&out).ctx(&out)?;
                let target = out.join(&patch.to);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).ctx(parent)?;
                }
                std::fs::write(&target, derived).ctx(&target)?;
            } else {
                extract(&archive, &out)?;
            }
            Ok((m.name.clone(), BuildOutcome::Built(out)))
        })
        .collect()
}

fn extract(archive: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let tmp = out.with_extension("tmp");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).ctx(&tmp)?;
    }
    concierge_platform::extract_archive(archive, &tmp).map_err(|stderr| Error::Extract {
        archive: archive.to_path_buf(),
        stderr,
    })?;
    for entry in walkdir::WalkDir::new(&tmp) {
        let entry = entry.map_err(|e| Error::Other(format!("walk {}: {e}", tmp.display())))?;
        if entry.file_type().is_file() {
            let mut perms = std::fs::metadata(entry.path())
                .ctx(entry.path())?
                .permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(entry.path(), perms).ctx(entry.path())?;
        }
    }
    std::fs::rename(&tmp, out).ctx(out)?;
    Ok(())
}
