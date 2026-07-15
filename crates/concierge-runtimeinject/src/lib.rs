//! The `BepInEx` family adapter — injected-runtime Unity games; plugin mods are
//! folders/dlls under `BepInEx/plugins`, catalog is `Thunderstore`. Valheim is
//! the concrete leaf; other `BepInEx` games (Lethal Company, …) join as leaves.
//! Moved out of `concierge-core` per the game-crate-tree architecture.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, RootTarget, MODLIST_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::ConfigFile;

    /// `BepInEx` lives at the game root; plugin mods are folders/dlls under
    /// `BepInEx/plugins`. Valheim leaf.
    #[derive(Debug)]
    pub struct Valheim;

    impl GameAdapter for Valheim {
        fn lexicon(&self) -> Lexicon {
            MODLIST_LEXICON
        }
        fn kind(&self) -> &'static str {
            "valheim"
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            Some("valheim") // exists, but Thunderstore is the primary catalog
        }
        fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
            &[
                ("game", RootTarget::InstanceRel("")),
                ("plugins", RootTarget::InstanceRel("BepInEx/plugins")),
            ]
        }
        fn default_install_root(&self) -> &'static str {
            "plugins"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            &[]
        }
        fn render_configs(&self, _m: &Manifest, _plugins: &[String]) -> Result<Vec<ConfigFile>> {
            Ok(Vec::new())
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            &["valheim.exe", "valheim.app", "valheim.x86_64"]
        }
    }

    pub static VALHEIM: Valheim = Valheim;

    /// Resolve a `BepInEx`-family game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        match kind {
            "valheim" => Some(&VALHEIM),
            _ => None,
        }
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        vec!["valheim"]
    }
}
