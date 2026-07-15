//! Launch a `CrossOver` + Steam title's *modded instance* through Steam.
//!
//! On macOS/`CrossOver`, Bethesda titles crash at render-init when launched
//! bare (e.g. `f4se_loader.exe` straight from the instance): the process is
//! not a child of a Steam-launched process, so it lacks the environment and
//! overlay Steam sets up, and a renderer subsystem comes back null. Verified
//! on Fallout 4 — bare launch faults at `Fallout4.exe+0x9e5cc0`; the identical
//! files launched *by Steam* run fine.
//!
//! The reliable path, then, is to let Steam launch the instance:
//!   1. point the Steam library's game dir at the instance (a reversible
//!      symlink — the real pristine dir is parked, never destroyed);
//!   2. make the instance's Steam launch *stub* (e.g. `Fallout4Launcher.exe`)
//!      the F4SE loader, so the script extender loads as a Steam child; and
//!   3. `steam -applaunch <app_id>` inside the bottle.
//!
//! This keeps Concierge's pristine-never-touched model (the pristine is parked
//! aside, restorable via [`deactivate_instance`]) while giving Steam a real,
//! validated game directory to launch — which happens to be the modded copy.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, IoCtx as _, Result};

const CX_WINE: &str = "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine";
const STEAM_WIN: &str = "C:\\Program Files (x86)\\Steam\\steam.exe";
const PARK_SUFFIX: &str = ".concierge-pristine";
const STUB_BACKUP_SUFFIX: &str = ".concierge-stub";

fn park_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(PARK_SUFFIX);
    PathBuf::from(s)
}

/// The exe Steam launches for a game kind (the "launcher" it runs on Play),
/// which we replace with the script-extender loader. `None` = this kind has no
/// known Steam stub, so the Steam-launch path does not apply.
#[must_use]
pub fn launch_stub(kind: &str) -> Option<&'static str> {
    match kind {
        "fallout4" => Some("Fallout4Launcher.exe"),
        "skyrimse" => Some("SkyrimSELauncher.exe"),
        _ => None,
    }
}

/// Point the Steam library game dir at `instance` (reversible, idempotent).
/// A real pristine directory there is parked to `<dir>.concierge-pristine`
/// once and never overwritten; our own symlink is simply repointed.
pub fn activate_instance(steam_game_dir: &Path, instance: &Path) -> Result<()> {
    if !instance.exists() {
        return Err(Error::NoInstance);
    }
    match std::fs::symlink_metadata(steam_game_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // ours (or a prior activation's) — repoint to be safe
            std::fs::remove_file(steam_game_dir).ctx(steam_game_dir)?;
        }
        Ok(_) => {
            let park = park_path(steam_game_dir);
            if park.exists() {
                return Err(Error::Other(format!(
                    "steam: {} is a real directory and a park already exists at \
                     {}; resolve manually to avoid touching the pristine install",
                    steam_game_dir.display(),
                    park.display()
                )));
            }
            std::fs::rename(steam_game_dir, &park).ctx(steam_game_dir)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Other(format!(
                "steam: stat {}: {e}",
                steam_game_dir.display()
            )))
        }
    }
    concierge_platform::symlink_dir(instance, steam_game_dir).ctx(steam_game_dir)
}

/// Undo [`activate_instance`]: drop our symlink and restore the parked pristine.
///
/// SAFETY: never remove the activation symlink unless there is a park to put
/// back in its place. The symlink is the only remaining pointer to the game at
/// this location; removing it with no park orphans the pristine directory (its
/// data survives inside the instance, but the pristine path goes empty). If we
/// hit that state we refuse loudly rather than silently lose the pristine.
pub fn deactivate_instance(steam_game_dir: &Path) -> Result<()> {
    let park = park_path(steam_game_dir);
    let is_symlink =
        std::fs::symlink_metadata(steam_game_dir).is_ok_and(|m| m.file_type().is_symlink());
    if is_symlink {
        if park.exists() {
            std::fs::remove_file(steam_game_dir).ctx(steam_game_dir)?;
            std::fs::rename(&park, steam_game_dir).ctx(steam_game_dir)?;
        } else {
            return Err(Error::Other(format!(
                "{} is activated (a symlink) but there is no park at {} to restore — refusing to \
                 remove the symlink, which would orphan the pristine location. Resolve manually \
                 (the game data is safe inside the instance).",
                steam_game_dir.display(),
                park.display()
            )));
        }
    } else if park.exists() {
        // Not currently activated, but an interrupted activation left a park —
        // put it back.
        std::fs::rename(&park, steam_game_dir).ctx(steam_game_dir)?;
    }
    Ok(())
}

/// Make the instance's Steam launch `stub` run the script-extender `loader`, so
/// the extender loads as a Steam child. The real stub is backed up once to
/// `<stub>.concierge-stub`. Idempotent.
pub fn set_loader_as_stub(instance: &Path, stub: &str, loader: &str) -> Result<()> {
    let loader_p = instance.join(loader);
    if !loader_p.exists() {
        return Err(Error::Other(format!(
            "steam: launch loader {} not found in instance",
            loader_p.display()
        )));
    }
    let stub_p = instance.join(stub);
    let backup = instance.join(format!("{stub}{STUB_BACKUP_SUFFIX}"));
    if stub_p.exists() {
        if backup.exists() {
            // stub already managed by us — drop it, re-copy the loader below
            // `w` is only mutated on Unix (chmod +w); on Windows it's used as-is.
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut w = std::fs::metadata(&stub_p).ctx(&stub_p)?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                w.set_mode(w.mode() | 0o200);
            }
            std::fs::set_permissions(&stub_p, w).ctx(&stub_p)?;
            std::fs::remove_file(&stub_p).ctx(&stub_p)?;
        } else {
            std::fs::rename(&stub_p, &backup).ctx(&stub_p)?;
        }
    }
    std::fs::copy(&loader_p, &stub_p).ctx(&stub_p)?;
    Ok(())
}

/// Undo [`set_loader_as_stub`]: restore the real launch stub from its backup.
/// Idempotent — a no-op when no backup exists (nothing was ever managed).
pub fn restore_stub(instance: &Path, stub: &str) -> Result<()> {
    let backup = instance.join(format!("{stub}{STUB_BACKUP_SUFFIX}"));
    if !backup.exists() {
        return Ok(());
    }
    let stub_p = instance.join(stub);
    if stub_p.exists() {
        // our loader-copy — clear the read-only the store leaves on it.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut w = std::fs::metadata(&stub_p).ctx(&stub_p)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            w.set_mode(w.mode() | 0o200);
        }
        std::fs::set_permissions(&stub_p, w).ctx(&stub_p)?;
        std::fs::remove_file(&stub_p).ctx(&stub_p)?;
    }
    std::fs::rename(&backup, &stub_p).ctx(&stub_p)
}

/// True when Steam is running in *any* bottle (best-effort; the game needs it).
#[must_use]
pub fn steam_running() -> bool {
    concierge_platform::process_running("steam.exe")
}

/// Start Steam in `bottle` if it is not already running. Does not wait for
/// login (a one-time, interactive step the user completes once per bottle).
pub fn ensure_steam(bottle: &str) -> Result<bool> {
    if steam_running() {
        return Ok(true);
    }
    Command::new(CX_WINE)
        .args(["--bottle", bottle, STEAM_WIN, "-silent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| Error::Io {
            path: PathBuf::from(CX_WINE),
            source,
        })?;
    Ok(false)
}

/// `steam -applaunch <app_id>` inside `bottle`. Steam launches whatever the
/// library game dir points at — the instance, once [`activate_instance`] ran.
pub fn applaunch(bottle: &str, app_id: u32) -> Result<()> {
    Command::new(CX_WINE)
        .args([
            "--bottle",
            bottle,
            STEAM_WIN,
            "-applaunch",
            &app_id.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| Error::Io {
            path: PathBuf::from(CX_WINE),
            source,
        })?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn launch_stub_known_kinds() {
        assert_eq!(launch_stub("fallout4"), Some("Fallout4Launcher.exe"));
        assert_eq!(launch_stub("kotor2"), None);
    }

    #[test]
    fn activate_parks_pristine_then_deactivate_restores_it() {
        let root = std::env::temp_dir().join(format!("cc-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam_dir = root.join("steamapps/common/Fallout 4");
        let instance = root.join("fo4nix/game");
        std::fs::create_dir_all(&instance).unwrap();
        std::fs::write(instance.join("Fallout4.exe"), b"MODDED").unwrap();
        // a real pristine dir with its own marker
        std::fs::create_dir_all(&steam_dir).unwrap();
        std::fs::write(steam_dir.join("Fallout4.exe"), b"PRISTINE").unwrap();

        activate_instance(&steam_dir, &instance).unwrap();
        assert!(std::fs::symlink_metadata(&steam_dir)
            .unwrap()
            .file_type()
            .is_symlink());
        // Steam now sees the modded exe through the symlink...
        assert_eq!(
            std::fs::read(steam_dir.join("Fallout4.exe")).unwrap(),
            b"MODDED"
        );
        // ...and the real pristine is parked, not destroyed
        assert_eq!(
            std::fs::read(park_path(&steam_dir).join("Fallout4.exe")).unwrap(),
            b"PRISTINE"
        );

        // idempotent: a second activate repoints without error
        activate_instance(&steam_dir, &instance).unwrap();

        deactivate_instance(&steam_dir).unwrap();
        assert!(!std::fs::symlink_metadata(&steam_dir)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(steam_dir.join("Fallout4.exe")).unwrap(),
            b"PRISTINE"
        );
        assert!(!park_path(&steam_dir).exists(), "park consumed on restore");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deactivate_refuses_to_orphan_a_parkless_symlink() {
        // A symlink with NO park is the only pointer to the game data; removing
        // it would leave the pristine location empty. deactivate must refuse.
        let root = std::env::temp_dir().join(format!("cc-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("real-data");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("Fallout4.exe"), b"GAME").unwrap();
        let steam_dir = root.join("steamapps/common/Fallout 4");
        std::fs::create_dir_all(steam_dir.parent().unwrap()).unwrap();
        concierge_platform::symlink_dir(&target, &steam_dir).unwrap();
        assert!(!park_path(&steam_dir).exists(), "precondition: no park");

        let err = deactivate_instance(&steam_dir).unwrap_err();
        assert!(
            err.to_string().contains("orphan"),
            "refuses with a clear reason: {err}"
        );
        // The symlink — and thus the pointer to the data — must survive.
        assert!(std::fs::symlink_metadata(&steam_dir)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(steam_dir.join("Fallout4.exe")).unwrap(),
            b"GAME"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn set_loader_as_stub_backs_up_and_is_idempotent() {
        let inst = std::env::temp_dir().join(format!("cc-stub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&inst);
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::write(inst.join("f4se_loader.exe"), b"LOADER").unwrap();
        std::fs::write(inst.join("Fallout4Launcher.exe"), b"REAL-LAUNCHER").unwrap();

        set_loader_as_stub(&inst, "Fallout4Launcher.exe", "f4se_loader.exe").unwrap();
        // Steam's stub now IS the loader
        assert_eq!(
            std::fs::read(inst.join("Fallout4Launcher.exe")).unwrap(),
            b"LOADER"
        );
        // the real launcher is preserved
        assert_eq!(
            std::fs::read(inst.join("Fallout4Launcher.exe.concierge-stub")).unwrap(),
            b"REAL-LAUNCHER"
        );
        // idempotent — second call keeps the loader, does not clobber the backup
        set_loader_as_stub(&inst, "Fallout4Launcher.exe", "f4se_loader.exe").unwrap();
        assert_eq!(
            std::fs::read(inst.join("Fallout4Launcher.exe.concierge-stub")).unwrap(),
            b"REAL-LAUNCHER"
        );

        // restore puts the real launcher back and consumes the backup
        restore_stub(&inst, "Fallout4Launcher.exe").unwrap();
        assert_eq!(
            std::fs::read(inst.join("Fallout4Launcher.exe")).unwrap(),
            b"REAL-LAUNCHER"
        );
        assert!(
            !inst.join("Fallout4Launcher.exe.concierge-stub").exists(),
            "backup consumed on restore"
        );
        // idempotent — restoring again with no backup is a clean no-op
        restore_stub(&inst, "Fallout4Launcher.exe").unwrap();
        assert_eq!(
            std::fs::read(inst.join("Fallout4Launcher.exe")).unwrap(),
            b"REAL-LAUNCHER"
        );
        let _ = std::fs::remove_dir_all(&inst);
    }
}
