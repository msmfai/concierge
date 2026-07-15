//! Per-profile sandbox activation — MO2-equivalent isolation on macOS/Wine
//! without USVFS.
//!
//! Each profile owns a "My Games" directory (its own INIs, `Saves/`, and load
//! order). The game, however, always reads from one *canonical* location
//! (`~/Documents/My Games/<Game>`, or the bottle's Documents under `CrossOver`).
//! At launch we point that canonical location at the launching profile's
//! sandbox via a **symlink**, repointed per profile. Any real pre-existing
//! directory is moved aside **once** to `<canonical>.concierge-backup` and
//! restored on deactivate — the user's real Documents are never destroyed, only
//! parked. Two profiles therefore get fully distinct INIs + saves, and the
//! shared Documents stays intact.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoCtx as _, Result};

const BACKUP_SUFFIX: &str = ".concierge-backup";

fn backup_path(canonical: &Path) -> PathBuf {
    let mut s = canonical.as_os_str().to_owned();
    s.push(BACKUP_SUFFIX);
    PathBuf::from(s)
}

fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    concierge_platform::symlink_dir(target, link).ctx(link)
}

/// Point `canonical` (where the game reads My Games) at `sandbox` (this
/// profile's directory). A real directory already there is backed up once; our
/// own symlink is simply repointed. Idempotent.
pub fn activate(canonical: &Path, sandbox: &Path) -> Result<()> {
    std::fs::create_dir_all(sandbox).ctx(sandbox)?;
    if let Some(parent) = canonical.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    match std::fs::symlink_metadata(canonical) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                // an existing symlink (ours or a prior profile's) — repoint it
                std::fs::remove_file(canonical).ctx(canonical)?;
            } else {
                // a REAL directory/file — park it aside, but never over a backup
                let backup = backup_path(canonical);
                if backup.exists() {
                    return Err(Error::Other(format!(
                        "sandbox: {} is a real directory and a backup already \
                         exists at {}; move one aside manually to avoid data loss",
                        canonical.display(),
                        backup.display()
                    )));
                }
                std::fs::rename(canonical, &backup).ctx(canonical)?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Other(format!(
                "sandbox: stat {}: {e}",
                canonical.display()
            )))
        }
    }
    make_symlink(sandbox, canonical)
}

/// Undo activation: drop our symlink and restore any parked real directory.
pub fn deactivate(canonical: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(canonical) {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(canonical).ctx(canonical)?;
        }
    }
    let backup = backup_path(canonical);
    if backup.exists() {
        std::fs::rename(&backup, canonical).ctx(canonical)?;
    }
    Ok(())
}

/// Which sandbox a canonical symlink currently points at (for status/debug).
#[must_use]
pub fn active_target(canonical: &Path) -> Option<PathBuf> {
    std::fs::read_link(canonical).ok()
}

/// Wire the Bethesda save-path redirect into this profile's INI so saves land in
/// the sandbox's own `Saves/` — belt-and-suspenders alongside the My Games
/// symlink (and the sole redirect on setups where the symlink isn't honored).
/// Returns the INI written, or `None` for non-Bethesda games. Idempotent.
pub fn write_save_redirect(sandbox: &Path, game_kind: &str) -> Result<Option<PathBuf>> {
    let ini = match game_kind {
        "fallout4" => "Fallout4Custom.ini",
        "skyrimse" => "SkyrimCustom.ini",
        _ => return Ok(None),
    };
    std::fs::create_dir_all(sandbox.join("Saves")).ctx(sandbox)?;
    let path = sandbox.join(ini);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains("SLocalSavePath") {
        return Ok(Some(path)); // already redirected
    }
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    // FO4/Skyrim: saves under My Games (= this sandbox) / SLocalSavePath.
    body.push_str("[General]\nbUseMyGamesDirectory=1\nSLocalSavePath=Saves\\\n");
    std::fs::write(&path, &body).ctx(&path)?;
    Ok(Some(path))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn two_profiles_are_isolated_and_the_real_documents_survive() {
        let root = std::env::temp_dir().join(format!("cc-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let canonical = root.join("Documents/My Games/Fallout4");
        // a pre-existing REAL Documents dir with the user's own file
        write(&canonical.join("Fallout4.ini"), "REAL-USER-INI");

        let a = root.join("profileA/MyGames");
        let b = root.join("profileB/MyGames");

        // --- launch profile A: a save written "through" canonical lands in A ---
        activate(&canonical, &a).unwrap();
        write(&canonical.join("Saves/Save1.fos"), "A-save");
        assert!(
            a.join("Saves/Save1.fos").exists(),
            "A's save is in A's sandbox"
        );
        assert!(!b.join("Saves/Save1.fos").exists(), "B never saw it");
        // the real Documents was parked, not destroyed
        assert!(backup_path(&canonical).join("Fallout4.ini").exists());

        // --- switch to profile B: distinct saves; A untouched ---
        activate(&canonical, &b).unwrap();
        write(&canonical.join("Saves/Save2.fos"), "B-save");
        assert!(
            b.join("Saves/Save2.fos").exists(),
            "B's save is in B's sandbox"
        );
        assert!(!b.join("Saves/Save1.fos").exists(), "A's save is NOT in B");
        assert!(a.join("Saves/Save1.fos").exists(), "A's save is preserved");
        assert!(!a.join("Saves/Save2.fos").exists(), "B's save is NOT in A");

        // --- deactivate: the user's real Documents comes back intact ---
        deactivate(&canonical).unwrap();
        assert_eq!(
            std::fs::read_to_string(canonical.join("Fallout4.ini")).unwrap(),
            "REAL-USER-INI"
        );
        assert!(
            !backup_path(&canonical).exists(),
            "backup consumed on restore"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_redirect_is_written_idempotent_and_bethesda_only() {
        let sb = std::env::temp_dir().join(format!("cc-sr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sb);
        let ini = write_save_redirect(&sb, "fallout4")
            .unwrap()
            .expect("fo4 gets an ini");
        let body = std::fs::read_to_string(&ini).unwrap();
        assert!(body.contains("SLocalSavePath=Saves"), "redirect written");
        assert!(sb.join("Saves").is_dir(), "Saves dir created");
        // idempotent — a second call doesn't duplicate the key
        write_save_redirect(&sb, "fallout4").unwrap();
        assert_eq!(
            std::fs::read_to_string(&ini)
                .unwrap()
                .matches("SLocalSavePath")
                .count(),
            1
        );
        // non-Bethesda games get no INI redirect (the symlink alone isolates)
        assert!(write_save_redirect(&sb, "kotor2").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&sb);
    }
}
