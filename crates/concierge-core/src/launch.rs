//! Launch the game through the Plan's runtime. Universal — the adapter named
//! the candidates at eval time; the runtime decides how to start them. When
//! no mod targets the game directory (BG3-style profile mods), the game runs
//! straight from the pristine install.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::plan::Plan;
use crate::runtime::{spawn, Runtime};
use crate::steam;

#[derive(Debug)]
pub struct LaunchInfo {
    pub exe: String,
    pub runtime: Runtime,
    pub from_instance: bool,
    pub steam_running: bool,
}

pub fn launch(plan: &Plan) -> Result<LaunchInfo> {
    let (base, from_instance) = match &plan.game.instance {
        Some(i) => {
            let instance = PathBuf::from(i);
            if plan.needs_instance() || instance.exists() {
                if !instance.exists() {
                    return Err(Error::NoInstance);
                }
                (instance, true)
            } else {
                (PathBuf::from(&plan.game.pristine), false)
            }
        }
        None => (PathBuf::from(&plan.game.pristine), false),
    };

    // Modded instance of a CrossOver + Steam title: launch THROUGH Steam.
    // Launched bare, Bethesda titles crash at render-init under CrossOver (no
    // Steam env/overlay); Steam must launch the instance. We point the Steam
    // library game dir at the instance and make its launch stub the extender
    // loader, so F4SE loads as a Steam child. See `steam` module.
    if from_instance {
        if let (Runtime::CrossOver { bottle }, Some(app_id), Some(stub)) = (
            &plan.game.runtime,
            plan.steam_app_id,
            steam::launch_stub(&plan.game.kind),
        ) {
            let loader = plan
                .launch_candidates
                .iter()
                .find(|c| base.join(c).exists())
                .ok_or_else(|| {
                    Error::Other(format!(
                        "no launch loader found in instance {} (candidates: {})",
                        base.display(),
                        plan.launch_candidates.join(", ")
                    ))
                })?;
            let steam_game_dir = PathBuf::from(&plan.game.pristine);
            // Point Steam at the instance's *canonical* dir (the instance path
            // may itself route through a bottle symlink); a direct link is what
            // Steam validates cleanly.
            let target = std::fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
            steam::activate_instance(&steam_game_dir, &target)?;
            steam::set_loader_as_stub(&base, stub, loader)?;
            let steam_running = steam::ensure_steam(bottle)?;
            steam::applaunch(bottle, app_id)?;
            return Ok(LaunchInfo {
                exe: format!("steam -applaunch {app_id} \u{2192} {loader}"),
                runtime: plan.game.runtime.clone(),
                from_instance: true,
                steam_running,
            });
        }
    }

    // Pristine launches of native Steam titles go through Steam (DRM);
    // instance launches must exec directly so the modded copy runs.
    if !from_instance {
        if let (Runtime::Native, Some(app_id)) = (&plan.game.runtime, plan.steam_app_id) {
            let url = format!("steam://rungameid/{app_id}");
            let status = concierge_platform::open_url(&url).map_err(|source| Error::Io {
                path: std::path::PathBuf::from(&url),
                source,
            })?;
            if !status.success() {
                return Err(Error::Other(format!("opening {url} failed")));
            }
            return Ok(LaunchInfo {
                exe: url,
                runtime: plan.game.runtime.clone(),
                from_instance: false,
                steam_running: true,
            });
        }
    }

    let exe = plan
        .launch_candidates
        .iter()
        .map(|c| base.join(c))
        .find(|p| p.exists())
        .ok_or_else(|| {
            Error::Other(format!(
                "no launch candidate found under {} (candidates: {})",
                base.display(),
                plan.launch_candidates.join(", ")
            ))
        })?;

    let steam_running = match &plan.game.runtime {
        Runtime::Native => true, // native Steam titles relaunch Steam themselves
        _ => concierge_platform::process_running("steam.exe"),
    };

    spawn(&plan.game.runtime, &exe)?;

    Ok(LaunchInfo {
        exe: exe
            .file_name()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        runtime: plan.game.runtime.clone(),
        from_instance,
        steam_running,
    })
}

/// What [`deactivate`] reversed, for reporting.
#[derive(Debug, Default)]
pub struct DeactivateInfo {
    /// The pristine was un-parked back into the Steam library path.
    pub instance_deactivated: bool,
    /// The real launch stub was restored in the instance.
    pub stub_restored: bool,
}

/// Reverse a Steam-in-bottle [`launch`]: restore the pristine at the Steam
/// library path and the real launch stub in the instance. Safe to run when no
/// launch is active (idempotent no-ops), and only touches the CrossOver+Steam
/// title case — a native/pristine launch parks nothing to reverse.
pub fn deactivate(plan: &Plan) -> Result<DeactivateInfo> {
    let mut info = DeactivateInfo::default();
    let steam_game_dir = PathBuf::from(&plan.game.pristine);
    // Restore the stub inside the instance first (if we know where it is), then
    // drop our symlink and un-park the pristine.
    if let (Some(instance), Some(stub)) = (&plan.game.instance, steam::launch_stub(&plan.game.kind))
    {
        let inst = PathBuf::from(instance);
        if inst.exists() {
            steam::restore_stub(&inst, stub)?;
            info.stub_restored = true;
        }
    }
    steam::deactivate_instance(&steam_game_dir)?;
    info.instance_deactivated = true;
    Ok(info)
}
