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
            } else if deploy_whole(&m.file) {
                // A Java mod (Fabric/Forge `.jar`) is a LOADER ARTIFACT, not an
                // archive of loose files: it must land in `mods/` whole. A `.jar`
                // is technically a zip, so the generic extractor would splay its
                // contents (META-INF/, assets/, …) into the install root — never
                // a loadable mod. Place the file whole instead; the single-file
                // build tree then deploys as `<install_root>/<file>`.
                place_whole(&archive, &out, &m.file)?;
            } else {
                extract(&archive, &out)?;
            }
            Ok((m.name.clone(), BuildOutcome::Built(out)))
        })
        .collect()
}

/// Whether `file` is a whole-file mod artifact that must NOT be unpacked — a
/// Java loader jar (Fabric/Forge/Quilt). These deploy into `mods/` as-is.
fn deploy_whole(file: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
}

/// Place a whole-file artifact into a single-file build tree `out/<file>`, made
/// read-only like every other build output so hardlinked deploys can't drift.
fn place_whole(archive: &std::path::Path, out: &std::path::Path, file: &str) -> Result<()> {
    let tmp = out.with_extension("tmp");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).ctx(&tmp)?;
    }
    std::fs::create_dir_all(&tmp).ctx(&tmp)?;
    // The stored file may carry an md5 prefix; deploy it under its real name.
    let target = tmp.join(file);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    std::fs::copy(archive, &target).ctx(&target)?;
    let mut perms = std::fs::metadata(&target).ctx(&target)?.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms).ctx(&target)?;
    std::fs::rename(&tmp, out).ctx(out)?;
    Ok(())
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{deploy_whole, place_whole};

    #[test]
    fn jars_deploy_whole_other_archives_do_not() {
        assert!(deploy_whole("sodium-fabric-0.9.1.jar"));
        assert!(deploy_whole("Mod.JAR"), "case-insensitive");
        assert!(!deploy_whole("SkyUI-12604.zip"));
        assert!(!deploy_whole("mod.7z"));
        assert!(!deploy_whole("noext"));
    }

    #[test]
    fn place_whole_makes_a_single_file_readonly_build_tree() {
        let dir = std::env::temp_dir().join(format!("cg-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("abc123-sodium-fabric-0.9.1.jar");
        std::fs::write(&archive, b"PK\x03\x04 jar bytes").unwrap();
        let out = dir.join("build");
        place_whole(&archive, &out, "sodium-fabric-0.9.1.jar").unwrap();
        // Exactly one file, under the mod's real name (md5 prefix dropped) —
        // so it deploys as <install_root>/sodium-fabric-0.9.1.jar, whole.
        let entries: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["sodium-fabric-0.9.1.jar".to_owned()]);
        let jar = out.join("sodium-fabric-0.9.1.jar");
        assert_eq!(std::fs::read(&jar).unwrap(), b"PK\x03\x04 jar bytes");
        assert!(std::fs::metadata(&jar).unwrap().permissions().readonly());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
