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
        fn agent_guide(&self) -> Option<String> {
            Some(
                "- **The mod loader is the foundation, and it lives in the launcher — not the \
                 pack.** Every jar targets a loader (Fabric / Forge / NeoForge / Quilt) AND a game \
                 version. Set both in `[compat]` (`loader`, `game_version`); Concierge resolves each \
                 Modrinth mod to the build matching them. A mismatch means the jar silently fails to \
                 load or the game won't start.\n\
                 - **Mods are jars under `mods/`, no load order** — the loader resolves \
                 dependencies. Fabric mods usually also need **Fabric API**; Quilt needs **QSL** — \
                 include the API mod for the loader you chose.\n\
                 - **The catalog is Modrinth** (open, no key): `concierge browse` and `concierge \
                 modrinth <slug>` resolve free CDN downloads. CurseForge is the other major host and \
                 needs its own flow.\n\
                 - **Keep every mod on one game version.** A pack mixing e.g. 1.20.1 and 1.21 jars \
                 will crash on load — hold every mod to the `[compat].game_version`."
                    .to_owned(),
            )
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
