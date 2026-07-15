//! The Bethesda (Creation/Gamebryo-engine) game **shape** — plugins.txt with
//! `*` prefixes, a Custom.ini for archive invalidation, an xSE loader ahead of
//! the vanilla exe. This is the family; concrete titles are thin leaf crates
//! (`concierge-skyrimse`, `concierge-fallout4`, …) that construct a [`Bethesda`]
//! with their own data (masters, exe, ini, Steam app id) and self-register.

use std::fmt::Write as _;
use std::path::PathBuf;

use concierge::error::{Error, Result};
use concierge::game::{GameAdapter, Lexicon, RootTarget, BETHESDA_LEXICON};
use concierge::manifest::Manifest;
use concierge::plan::{ConfigFile, GENERATED_BANNER};

/// Shared shape for Creation/Gamebryo-engine games. A leaf crate specializes it
/// entirely through data — no code — which is why one family serves Skyrim
/// SE/LE/VR, Fallout 3/NV/4/76, Oblivion, Starfield, and Morrowind.
#[derive(Debug)]
pub struct Bethesda {
    pub kind_name: &'static str,
    pub domain: &'static str,
    pub custom_ini: &'static str,
    pub launchers: &'static [&'static str],
    /// Steam app id — enables the Steam-launch path for a modded instance on
    /// `CrossOver` (the game must be launched by Steam, not bare).
    pub steam_app: u32,
    /// Base-game masters + DLC that always lead the load order (used when a
    /// profile declares no `[game].dlc`, so even an empty modlist shows the
    /// base game).
    pub base_masters: &'static [&'static str],
    /// Per-line prefix in `plugins.txt`. Fallout 4 / Skyrim SE / Starfield mark
    /// enabled plugins with `"*"`; pre-SE titles (Skyrim LE, Oblivion, Fallout
    /// 3/NV) list them plain (`""`). The load-order divergence, as data.
    pub plugin_prefix: &'static str,
}

impl Bethesda {
    fn path(&self, m: &Manifest, key: &str) -> Result<PathBuf> {
        m.game.paths.get(key).cloned().ok_or_else(|| {
            Error::Manifest(format!(
                "[game.paths] missing '{key}' for {}",
                self.kind_name
            ))
        })
    }
}

impl GameAdapter for Bethesda {
    fn kind(&self) -> &'static str {
        self.kind_name
    }
    fn lexicon(&self) -> Lexicon {
        BETHESDA_LEXICON
    }
    fn nexus_domain(&self) -> Option<&'static str> {
        Some(self.domain)
    }
    fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
        &[
            ("game", RootTarget::InstanceRel("")),
            ("data", RootTarget::InstanceRel("Data")),
        ]
    }
    fn default_install_root(&self) -> &'static str {
        "data"
    }
    fn required_paths(&self) -> &'static [&'static str] {
        &["plugins_txt", "my_games"]
    }
    fn render_configs(&self, m: &Manifest, plugins: &[String]) -> Result<Vec<ConfigFile>> {
        let mut lines = vec![format!("# {}", GENERATED_BANNER.trim_start_matches("; "))];
        // Base-game masters/DLC always lead the load order. Declared per-profile
        // via `[game].dlc`, but default from the leaf's data so even an empty
        // modlist shows the base game.
        let pre = self.plugin_prefix;
        if m.game.dlc.is_empty() {
            for d in self.base_masters {
                lines.push(format!("{pre}{d}"));
            }
        } else {
            for d in &m.game.dlc {
                lines.push(format!("{pre}{d}"));
            }
        }
        for p in plugins {
            lines.push(format!("{pre}{p}"));
        }
        let mut plugins_txt = lines.join("\n");
        plugins_txt.push('\n');

        let mut ini = String::new();
        ini.push_str(GENERATED_BANNER);
        ini.push('\n');
        for (section, kv) in &m.ini {
            let _ = writeln!(ini, "[{section}]");
            for (k, v) in kv {
                let _ = writeln!(ini, "{k}={v}");
            }
        }
        Ok(vec![
            ConfigFile::new(
                self.path(m, "plugins_txt")?.display().to_string(),
                plugins_txt,
            ),
            ConfigFile::new(
                self.path(m, "my_games")?
                    .join(self.custom_ini)
                    .display()
                    .to_string(),
                ini,
            ),
        ])
    }
    fn launch_candidates(&self) -> &'static [&'static str] {
        self.launchers
    }
    fn steam_app_id(&self) -> Option<u32> {
        Some(self.steam_app)
    }
    fn plugin_bases(&self) -> Option<&'static [&'static str]> {
        Some(self.base_masters)
    }
    fn lints(&self, plan: &concierge::plan::Plan) -> Result<Vec<concierge::lint::Violation>> {
        // The Bethesda plugin invariants — missing masters, the 254 full-plugin
        // limit, and master-graph cycles — for EVERY plugin-order game, resolved
        // from this adapter's data rather than a stale kind allowlist.
        crate::bethesda_lints(plan)
    }
    fn launch_health(
        &self,
        plan: &concierge::plan::Plan,
        my_games: &std::path::Path,
    ) -> Option<concierge::game::LaunchHealth> {
        let mut h = concierge::game::LaunchHealth::default();
        // Platform pre-warning: Buffout4's crash handler + version detection
        // don't work under Wine — it self-disables as "incompatible game 0".
        if matches!(
            plan.game.runtime,
            concierge::runtime::Runtime::CrossOver { .. }
        ) && plan.mods.iter().any(|m| {
            m.name.to_lowercase().contains("buffout")
                || m.plugins
                    .iter()
                    .any(|p| p.to_lowercase().contains("buffout"))
        }) {
            h.issues.push(
                "Buffout4 self-disables under CrossOver/Wine (reads the game version as 0) — \
                 expect an 'incompatible' popup; disable it on this platform"
                    .to_owned(),
            );
        }
        // Parse the script-extender log at <my_games>/<XSE>/<xse>.log.
        let extender = self.launchers.first()?.strip_suffix("_loader.exe")?;
        if let Some(log) = find_log(my_games, &format!("{extender}.log")) {
            if let Ok(text) = std::fs::read_to_string(&log) {
                for line in text.lines() {
                    let low = line.to_lowercase();
                    if low.contains("loaded correctly") {
                        if let Some(name) = line.split_whitespace().nth(1) {
                            h.loaded.push(name.to_owned());
                        }
                    } else if low.contains("disabled")
                        || low.contains("incompatible")
                        || low.contains("couldn't load")
                    {
                        h.issues.push(line.trim().to_owned());
                    }
                }
            }
        }
        Some(h)
    }
}

/// Find `<name>` directly under `my_games` or one level down (the xSE log lives
/// in an `F4SE`/`SKSE` subfolder). None if absent.
fn find_log(my_games: &std::path::Path, name: &str) -> Option<PathBuf> {
    let direct = my_games.join(name);
    if direct.exists() {
        return Some(direct);
    }
    for e in std::fs::read_dir(my_games).ok()?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let p = e.path().join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::Bethesda;
    use concierge::game::GameAdapter as _;
    use concierge::manifest::Manifest;

    // A throwaway leaf, so the shape is tested without depending on a real title.
    static TESTBED: Bethesda = Bethesda {
        kind_name: "skyrimse",
        domain: "skyrimspecialedition",
        custom_ini: "SkyrimCustom.ini",
        launchers: &["skse64_loader.exe", "SkyrimSE.exe"],
        steam_app: 489_830,
        base_masters: &["Skyrim.esm", "Update.esm", "Dragonborn.esm"],
        plugin_prefix: "*",
    };

    fn manifest(dlc: &str) -> Manifest {
        Manifest::parse(&format!(
            "[game]\nkind = \"skyrimse\"\npristine = \"\"\nversion = \"1.0\"\n{dlc}\
             [game.paths]\nplugins_txt = \"/p/plugins.txt\"\nmy_games = \"/m\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn base_masters_lead_the_load_order_when_no_dlc_declared() {
        let m = manifest("");
        let cfgs = TESTBED.render_configs(&m, &["Mod.esp".into()]).unwrap();
        let pt = &cfgs[0].content;
        assert!(pt.contains("*Skyrim.esm"), "base master present: {pt}");
        assert!(pt.contains("*Dragonborn.esm"), "base DLC present");
        assert!(pt.contains("*Mod.esp"), "mod appended after base");
        assert_eq!(cfgs[0].path, "/p/plugins.txt");
        assert!(cfgs[1].path.ends_with("SkyrimCustom.ini"));
    }

    #[test]
    fn declared_dlc_overrides_the_base_masters() {
        let m = manifest("dlc = [\"Update.esm\"]\n");
        let pt = TESTBED.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("*Update.esm"));
        assert!(
            !pt.contains("*Dragonborn.esm"),
            "explicit dlc replaces the default"
        );
    }

    #[test]
    fn lexicon_speaks_bethesda() {
        assert_eq!(TESTBED.lexicon().order, "load order");
        assert_eq!(TESTBED.lexicon().plugins, "plugins");
    }
}
