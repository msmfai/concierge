//! The SMAPI family adapter — Stardew Valley and other SMAPI-loader games.
//! Mods are folders under `Mods/`; there is NO load-order file (SMAPI resolves
//! dependencies from each mod's own `manifest.json` at startup). Moved out of
//! `concierge-core` per the game-crate-tree architecture.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, RootTarget, MODLIST_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::ConfigFile;

    /// SMAPI model: mods are folders under `Mods/`; ordering data lives inside
    /// the mods (their `manifest.json`), not beside them, so `render_configs`
    /// is empty.
    #[derive(Debug)]
    pub struct Stardew;

    impl GameAdapter for Stardew {
        fn lexicon(&self) -> Lexicon {
            MODLIST_LEXICON
        }
        fn kind(&self) -> &'static str {
            "stardew"
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            Some("stardewvalley")
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
            &[]
        }
        fn render_configs(&self, _m: &Manifest, _plugins: &[String]) -> Result<Vec<ConfigFile>> {
            Ok(Vec::new())
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            &["StardewModdingAPI", "Stardew Valley.app", "StardewValley"]
        }
    }

    pub static STARDEW: Stardew = Stardew;

    /// Resolve a SMAPI-family game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        match kind {
            "stardew" => Some(&STARDEW),
            _ => None,
        }
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        vec!["stardew"]
    }
}
