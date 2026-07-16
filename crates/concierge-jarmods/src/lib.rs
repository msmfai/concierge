//! The `Minecraft` (Java) family adapter — jar mods under `mods/`, ordering
//! dependency-resolved by the loader (`Fabric`/`Forge`); catalog is
//! `Modrinth`/`CurseForge`, not Nexus. Moved out of `concierge-core` per the
//! game-crate-tree architecture.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, RootTarget, MODLIST_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::ConfigFile;

    /// Folder mods (jars) under `mods/`; loader/game-version compatibility lives
    /// in catalog metadata, not in files we deploy.
    #[derive(Debug)]
    pub struct Minecraft;

    impl GameAdapter for Minecraft {
        fn lexicon(&self) -> Lexicon {
            MODLIST_LEXICON
        }
        fn kind(&self) -> &'static str {
            "minecraft"
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            None // mods live on Modrinth, not Nexus
        }
        fn modrinth_domain(&self) -> Option<&'static str> {
            Some("minecraft")
        }
        fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
            &[
                ("game", RootTarget::InstanceRel("")),
                ("mods", RootTarget::InstanceRel("mods")),
            ]
        }
        fn default_install_root(&self) -> &'static str {
            "mods"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            &[]
        }
        fn render_configs(&self, _m: &Manifest, _plugins: &[String]) -> Result<Vec<ConfigFile>> {
            Ok(Vec::new())
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            &[] // launched via the Minecraft launcher/Prism, not an exe we own
        }
    }

    pub static MINECRAFT: Minecraft = Minecraft;

    /// Resolve a Minecraft-family game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        match kind {
            "minecraft" => Some(&MINECRAFT),
            _ => None,
        }
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        vec!["minecraft"]
    }
}
