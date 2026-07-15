//! The merge-tool family — games whose mods aren't loose files the engine reads
//! directly, but inputs to an external merge/repack tool (`Frosty` for Frostbite
//! games, `ME3Tweaks` for Mass Effect, `SMPC` for Spider-Man). Concierge stages
//! the mods into the tool's manifest-declared input folder and undeploys them;
//! the tool performs the binary archive merge — the same delegation `Vortex` uses.
//! The tool is a documented external dependency, like Steam/CrossOver/LOOT.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, RootTarget, MODLIST_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::ConfigFile;

    /// A game modded via an external merge tool. Mods stage into a
    /// manifest-declared `[game.paths].mods` (the tool's input folder); the tool
    /// (`tool`) merges them into the game's archives on the user's command.
    #[derive(Debug)]
    pub struct MergeTool {
        pub kind_name: &'static str,
        pub domain: &'static str,
        pub launchers: &'static [&'static str],
        pub steam_app: Option<u32>,
        /// The external tool that performs the merge (documented dependency).
        pub tool: &'static str,
    }

    // Mods stage into a manifest-declared folder (the tool's input), never the
    // game dir — so this is always a PathKey root.
    const ROOTS: &[(&str, RootTarget)] = &[
        ("game", RootTarget::InstanceRel("")),
        ("mods", RootTarget::PathKey("mods")),
    ];

    impl GameAdapter for MergeTool {
        fn kind(&self) -> &'static str {
            self.kind_name
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            Some(self.domain)
        }
        fn lexicon(&self) -> Lexicon {
            MODLIST_LEXICON
        }
        fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
            ROOTS
        }
        fn default_install_root(&self) -> &'static str {
            "mods"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            &["mods"]
        }
        fn render_configs(&self, _m: &Manifest, _plugins: &[String]) -> Result<Vec<ConfigFile>> {
            Ok(Vec::new())
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            self.launchers
        }
        fn steam_app_id(&self) -> Option<u32> {
            self.steam_app
        }
    }

    /// Which external tool a merge-tool game delegates to (for docs / the ledger).
    #[must_use]
    pub fn tool_for(kind: &str) -> Option<&'static str> {
        resolve_mt(kind).map(|m| m.tool)
    }

    macro_rules! mt {
        ($stat:ident, $kind:literal, $domain:literal, $tool:literal, $launchers:expr, $app:expr) => {
            pub static $stat: MergeTool = MergeTool {
                kind_name: $kind,
                domain: $domain,
                launchers: $launchers,
                steam_app: $app,
                tool: $tool,
            };
        };
    }

    mt!(
        BATTLEFRONT2,
        "starwarsbattlefront22017",
        "starwarsbattlefront22017",
        "Frosty Mod Manager",
        &["starwarsbattlefrontii.exe"],
        Some(1_237_970)
    );
    mt!(
        DAINQUISITION,
        "dragonageinquisition",
        "dragonageinquisition",
        "Frosty Mod Manager",
        &["DragonAgeInquisition.exe"],
        Some(1_222_690)
    );
    mt!(
        MELE,
        "masseffectlegendaryedition",
        "masseffectlegendaryedition",
        "ME3Tweaks Mod Manager",
        &["MassEffectLauncher.exe"],
        Some(1_328_670)
    );
    mt!(
        SPIDERMAN,
        "marvelsspidermanremastered",
        "marvelsspidermanremastered",
        "SMPC Tool",
        &["Spider-Man.exe"],
        Some(1_817_070)
    );

    const ALL: &[&MergeTool] = &[&BATTLEFRONT2, &DAINQUISITION, &MELE, &SPIDERMAN];

    fn resolve_mt(kind: &str) -> Option<&'static MergeTool> {
        ALL.iter().find(|m| m.kind_name == kind).copied()
    }

    /// Resolve a merge-tool game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        resolve_mt(kind).map(|m| {
            let a: &'static dyn GameAdapter = m;
            a
        })
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        ALL.iter().map(|m| m.kind_name).collect()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    mod tests {

        #[test]
        fn merge_tool_games_stage_to_a_declared_mods_path_and_name_their_tool() {
            for kind in super::kinds() {
                let a = super::resolve(kind).unwrap();
                assert_eq!(a.kind(), kind);
                assert!(a.nexus_domain().is_some(), "{kind}: domain");
                // mods root is a manifest-declared path, not the game dir
                assert_eq!(a.required_paths(), &["mods"], "{kind}: needs a mods path");
                assert!(
                    super::tool_for(kind).is_some(),
                    "{kind}: names its merge tool"
                );
                // renders no activation file — the tool merges
                let m = concierge::manifest::Manifest::parse(&format!(
                    "[game]\nkind = \"{kind}\"\npristine = \"\"\nversion = \"1\"\n\
                     [game.paths]\nmods = \"/staging\"\n"
                ))
                .unwrap();
                assert!(a.render_configs(&m, &[]).unwrap().is_empty());
            }
        }
    }
}
