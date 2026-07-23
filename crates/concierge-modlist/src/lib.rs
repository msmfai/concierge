//! The `RimWorld` family adapter — folder mods under `Mods/`, activated by an
//! ordered `ModsConfig.xml` (package ids). Conflict substance is `Harmony`
//! runtime patching, invisible to file-level deploy. Moved out of
//! `concierge-core` per the game-crate-tree architecture.

pub mod adapter {
    use std::fmt::Write as _;

    use concierge::error::{Error, Result};
    use concierge::game::{GameAdapter, Lexicon, RootTarget, RIMWORLD_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::{ConfigFile, GENERATED_BANNER};

    /// Folder mods under `Mods/`, activated by an ORDERED `ModsConfig.xml`.
    #[derive(Debug)]
    pub struct RimWorld;

    impl GameAdapter for RimWorld {
        fn lexicon(&self) -> Lexicon {
            RIMWORLD_LEXICON
        }
        fn kind(&self) -> &'static str {
            "rimworld"
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            Some("rimworld")
        }
        fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
            &[
                ("game", RootTarget::InstanceRel("")),
                ("mods", RootTarget::InstanceRel("Mods")),
            ]
        }
        fn default_install_root(&self) -> &'static str {
            "mods"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            &["mods_config"]
        }
        fn render_configs(&self, m: &Manifest, plugins: &[String]) -> Result<Vec<ConfigFile>> {
            let mut active = String::new();
            for pid in std::iter::once("ludeon.rimworld")
                .map(String::from)
                .chain(plugins.iter().cloned())
            {
                let _ = writeln!(active, "        <li>{pid}</li>");
            }
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
                 <!-- {} -->\n\
                 <ModsConfigData>\n\
                 \x20   <activeMods>\n{active}\x20   </activeMods>\n\
                 </ModsConfigData>\n",
                GENERATED_BANNER.trim_start_matches("; ")
            );
            let target = m.game.paths.get("mods_config").ok_or_else(|| {
                Error::Manifest("[game.paths] missing 'mods_config' for rimworld".into())
            })?;
            Ok(vec![ConfigFile::new(target.display().to_string(), content)])
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            &["RimWorldMac.app", "RimWorldWin64.exe", "RimWorldLinux"]
        }
        fn steam_app_id(&self) -> Option<u32> {
            Some(294_100)
        }
        fn agent_guide(&self) -> Option<String> {
            Some(
                "- **Load order is authored in `ModsConfig.xml`** (package ids, in order). Core \
                 (`ludeon.rimworld`) loads first, then owned DLC, then mods; a mod that loads before \
                 its dependency throws errors. `concierge-cli sort` orders them.\n\
                 - **Harmony is the near-universal dependency.** Most C# mods patch the game through \
                 Harmony (`brrainz.harmony`) — install it and load it right after Core. It installs \
                 like any other mod (a folder under `Mods/`); it just has to come early.\n\
                 - **DLC are package ids too:** `ludeon.rimworld.royalty`, `.ideology`, `.biotech`, \
                 `.anomaly` load between Core and mods when owned.\n\
                 - **Two catalogs.** Steam Workshop hosts most RimWorld mods; Nexus (rimworld) is \
                 the DRM-free mirror. Prefer whichever the pack can fetch.\n\
                 - **XML mods stack (later-wins on defs/textures); C# mods patch.** Order matters \
                 most for patch mods — keep frameworks (Harmony, HugsLib) high and content low."
                    .to_owned(),
            )
        }
    }

    pub static RIMWORLD: RimWorld = RimWorld;

    /// Resolve a RimWorld-family game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        match kind {
            "rimworld" => Some(&RIMWORLD),
            _ => None,
        }
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        vec!["rimworld"]
    }
}
