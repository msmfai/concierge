//! Download → verify → stage → swap-and-relaunch.
//!
//! Swap strategy: a running executable can be **renamed** on Windows, macOS and
//! Linux (the running image stays mapped) even though it can't be overwritten in
//! place. So we rename the current binary aside (`.old-*`) and drop the new one at
//! its path, then relaunch — no elevated helper, no in-place-lock failure. Stale
//! `.old-*` files are swept on the next start.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::{Error, Result};

/// Generous timeout for a ~20 MB release asset over a slow link. (std has no
/// `from_mins`, so the pedantic "use a larger unit" lint has no better form.)
#[allow(clippy::duration_suboptimal_units)]
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// A downloaded, checksum-recorded update archive ready to apply.
#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub archive: PathBuf,
    pub tag: String,
    pub sha256: String,
}

fn io<E: std::fmt::Display>(e: E) -> Error {
    Error::Io(e.to_string())
}

/// Download `url` into `staging_dir` and record its SHA-256. The archive is named
/// after the release tag so a partial/previous download can't be mistaken for it.
pub fn stage_update(url: &str, tag: &str, staging_dir: &Path) -> Result<StagedUpdate> {
    std::fs::create_dir_all(staging_dir).map_err(io)?;
    let archive = staging_dir.join(format!("concierge-{tag}-update"));
    concierge_platform::diag(&format!("update: downloading {tag} from {url}"));
    let resp = ureq::get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .set("User-Agent", "concierge-updater")
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&archive).map_err(io)?;
    std::io::copy(&mut reader, &mut file).map_err(io)?;
    drop(file);
    let sha256 = sha256_file(&archive)?;
    concierge_platform::diag(&format!("update: staged {tag} sha256={sha256}"));
    Ok(StagedUpdate {
        archive,
        tag: tag.to_owned(),
        sha256,
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n).unwrap_or(&[]));
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Extract the staged archive and swap the two Concierge binaries next to
/// `install_dir` (the directory the running exe lives in). Returns the path of the
/// new GUI executable to relaunch. `.tar.gz` and `.zip` are both handled by the
/// system `tar` (bsdtar / Win10+ tar / libarchive).
///
/// NOTE (macOS): replacing the binary inside a signed `.app` invalidates its
/// ad-hoc signature; callers on macOS should prefer the reveal/website fallback
/// until we re-sign post-swap. The swap itself still succeeds.
pub fn apply_staged(staged: &StagedUpdate, install_dir: &Path) -> Result<PathBuf> {
    let tmp = install_dir.join(".concierge-update-extract");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(io)?;
    // bsdtar autodetects zip and tar.gz from content with -xf.
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&staged.archive)
        .current_dir(&tmp)
        .status()
        .map_err(io)?;
    if !status.success() {
        return Err(Error::Io(format!("tar extract failed ({status})")));
    }
    let (gui, cli) = (exe_name("concierge-gui"), exe_name("concierge"));
    let new_gui =
        find_file(&tmp, &gui).ok_or_else(|| Error::Verify(format!("{gui} not in archive")))?;
    swap_in(&new_gui, &install_dir.join(&gui))?;
    if let Some(new_cli) = find_file(&tmp, &cli) {
        // best-effort: the CLI sometimes isn't beside the GUI (e.g. macOS .app)
        let _ = swap_in(&new_cli, &install_dir.join(&cli));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(install_dir.join(&gui))
}

/// Rename the live binary aside and drop the new one at its path.
fn swap_in(new_bin: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        let aside = target.with_extension("old-pending");
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(target, &aside).map_err(io)?;
    }
    std::fs::copy(new_bin, target).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
    }
    concierge_platform::diag(&format!("update: swapped {}", target.display()));
    Ok(())
}

/// Sweep `.old-pending` binaries left by a prior swap. Call once at startup.
pub fn cleanup_old(install_dir: &Path) {
    for name in [exe_name("concierge-gui"), exe_name("concierge")] {
        let aside = install_dir.join(&name).with_extension("old-pending");
        let _ = std::fs::remove_file(aside);
    }
}

/// Spawn the (new) GUI detached so the current process can exit and release its
/// image. Best-effort — a failure just means the user relaunches manually.
pub fn relaunch(exe: &Path) {
    let _ = std::process::Command::new(exe).spawn();
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// Depth-first search for a file named `name` under `dir`.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    subdirs.into_iter().find_map(|d| find_file(&d, name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        // sha256("") = e3b0c442...; write an empty file and hash it.
        let dir = std::env::temp_dir().join(format!("cg-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn swap_renames_live_binary_aside_then_replaces() {
        let dir = std::env::temp_dir().join(format!("cg-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("concierge-gui");
        std::fs::write(&target, b"OLD").unwrap();
        let newbin = dir.join("new");
        std::fs::write(&newbin, b"NEW").unwrap();
        swap_in(&newbin, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        // the old one was moved aside, and cleanup removes it
        assert!(target.with_extension("old-pending").exists());
        cleanup_old(&dir);
        assert!(!target.with_extension("old-pending").exists());
    }

    #[test]
    fn find_file_descends() {
        let dir = std::env::temp_dir().join(format!("cg-find-{}", std::process::id()));
        let nested = dir.join("concierge-v1-x86_64-linux");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("concierge-gui"), b"x").unwrap();
        assert_eq!(
            find_file(&dir, "concierge-gui").unwrap(),
            nested.join("concierge-gui")
        );
        assert!(find_file(&dir, "nope").is_none());
    }
}
