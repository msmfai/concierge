//! The host×runtime axis. Adapters declare WHAT to launch and WHERE files
//! go; the runtime decides HOW on this host: path mapping into Wine prefixes
//! and the copy-on-write ladder for instance materialization.
//!
//! Core mandate: Linux, macOS, macOS/Linux with Wine, and Windows. The
//! variants below cover what this machine can exercise (native + `CrossOver`);
//! plain Wine prefixes and Proton share the `CrossOver` shape (a prefix root
//! + a Windows path) and slot in as further variants without touching adapters.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoCtx, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    /// The game runs directly on the host OS (macOS/Linux/Windows native).
    Native,
    /// Windows game inside a `CrossOver` bottle (macOS/Linux).
    CrossOver { bottle: String },
    /// Windows game inside a plain Wine prefix (macOS/Linux). Not yet
    /// exercised live; same mechanics as `CrossOver` minus bottle tooling.
    WinePrefix { prefix: String },
}

/// Infer the runtime from where the game lives. A path inside
/// `.../Bottles/<name>/drive_c/...` is a `CrossOver` install; anything else
/// is native. (Proton/Wine-prefix detection lands with a Linux host.)
pub fn detect(game_root: &Path) -> Runtime {
    let s = game_root.display().to_string();
    if let Some((head, _)) = s.split_once("/drive_c/") {
        if let Some((_, bottle)) = head.rsplit_once("/Bottles/") {
            return Runtime::CrossOver {
                bottle: bottle.to_owned(),
            };
        }
        return Runtime::WinePrefix {
            prefix: head.to_owned(),
        };
    }
    Runtime::Native
}

/// Host path -> path the game's own runtime understands (Windows path for
/// Wine-family runtimes, unchanged for native).
pub fn game_visible_path(rt: &Runtime, host_path: &Path) -> Result<String> {
    match rt {
        Runtime::Native => Ok(host_path.display().to_string()),
        Runtime::CrossOver { .. } | Runtime::WinePrefix { .. } => {
            let s = host_path.display().to_string();
            let (_, rel) = s.split_once("/drive_c/").ok_or_else(|| {
                Error::Other(format!("{s} is not inside the wine prefix drive_c"))
            })?;
            Ok(format!("C:\\{}", rel.replace('/', "\\")))
        }
    }
}

/// Copy-on-write ladder: APFS clonefile (`cp -Rc`, macOS) -> reflink
/// (`cp -R --reflink=auto`, Linux btrfs/XFS) -> a portable pure-Rust recursive
/// copy. The last rung has no external dependency and works on every OS
/// (Windows included); the fast copy-on-write rungs are an optimization tried
/// first on Unix. (Windows block-clone can slot in as a faster rung.)
pub fn cow_clone(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if try_cp_cow(src, dst) {
            return Ok(());
        }
        // A failed `cp` may have left a partial tree; clear it before falling back.
        if dst.exists() {
            std::fs::remove_dir_all(dst).ctx(dst)?;
        }
    }
    copy_dir_all(src, dst)
}

/// Try the OS copy-on-write clone via `cp`. Returns whether it succeeded.
#[cfg(unix)]
fn try_cp_cow(src: &Path, dst: &Path) -> bool {
    let flags: &[&str] = if cfg!(target_os = "macos") {
        &["-Rc"]
    } else {
        &["-R", "--reflink=auto"]
    };
    Command::new("cp")
        .args(flags)
        .arg(src)
        .arg(dst)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Portable recursive directory copy — the dependency-free baseline rung.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).ctx(dst)?;
    for entry in std::fs::read_dir(src).ctx(src)? {
        let entry = entry.ctx(src)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().ctx(&from)?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).ctx(&to)?;
        }
    }
    Ok(())
}

/// Launch an executable/app through the runtime. `exe` is a host path.
pub fn spawn(rt: &Runtime, exe: &Path) -> Result<()> {
    match rt {
        Runtime::Native => {
            if exe.extension().is_some_and(|e| e == "app") {
                let status = Command::new("open").arg(exe).status().ctx(exe)?;
                if !status.success() {
                    return Err(Error::Other(format!("open failed for {}", exe.display())));
                }
            } else {
                Command::new(exe)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ctx(exe)?;
            }
            Ok(())
        }
        Runtime::CrossOver { bottle } => {
            const CX_WINE: &str =
                "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine";
            let win = game_visible_path(rt, exe)?;
            Command::new(CX_WINE)
                .args(["--bottle", bottle, "--wait-children", &win])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|source| Error::Io {
                    path: PathBuf::from(CX_WINE),
                    source,
                })?;
            Ok(())
        }
        Runtime::WinePrefix { prefix } => {
            let win = game_visible_path(rt, exe)?;
            Command::new("wine")
                .env("WINEPREFIX", prefix)
                .arg(&win)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|source| Error::Io {
                    path: PathBuf::from("wine"),
                    source,
                })?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{copy_dir_all, detect, Runtime};

    #[test]
    fn copy_dir_all_copies_a_nested_tree() {
        let base = std::env::temp_dir().join(format!("cg-cow-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"world").unwrap();
        copy_dir_all(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello");
        assert_eq!(
            std::fs::read(dst.join("sub").join("b.txt")).unwrap(),
            b"world"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detect_native_vs_crossover() {
        assert_eq!(
            detect(std::path::Path::new("/games/skyrim")),
            Runtime::Native
        );
        let cx = detect(std::path::Path::new(
            "/x/Bottles/Games/drive_c/Program Files/Skyrim",
        ));
        assert_eq!(
            cx,
            Runtime::CrossOver {
                bottle: "Games".to_owned()
            }
        );
        // a Proton prefix (steamapps/compatdata/<id>/pfx/drive_c) is a Wine-family
        // runtime, NOT mis-detected as Native.
        let proton = detect(std::path::Path::new(
            "/steam/steamapps/compatdata/489830/pfx/drive_c/Program Files/Skyrim",
        ));
        assert!(
            matches!(proton, Runtime::WinePrefix { .. }),
            "proton -> wine-family, got {proton:?}"
        );
    }
}
