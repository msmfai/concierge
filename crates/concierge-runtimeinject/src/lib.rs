//! The `BepInEx` family adapter — injected-runtime Unity games; plugin mods are
//! folders/dlls under `BepInEx/plugins`, catalog is `Thunderstore`. Valheim is
//! the concrete leaf; other `BepInEx` games (Lethal Company, …) join as leaves.
//! Moved out of `concierge-core` per the game-crate-tree architecture.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, PromotedTool, RootTarget, MODLIST_LEXICON};
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
        fn steam_app_id(&self) -> Option<u32> {
            Some(892_970)
        }
        fn promoted_tools(&self) -> Vec<PromotedTool> {
            vec![PromotedTool {
                id: "bepinex",
                name: "BepInEx",
                blurb:
                    "The injected runtime every plugin loads through — installs to the game root \
                        (its winhttp.dll doorstop) and the game launches through it. Without it no \
                        plugin loads.",
                home: "https://thunderstore.io/c/valheim/p/denikson/BepInExPack_Valheim/",
                install_root: "game",
            }]
        }
        fn promoted_tool_for(&self, top_level: &[String]) -> Option<PromotedTool> {
            // BepInEx ships the Unity doorstop (winhttp.dll / doorstop_config.ini)
            // plus a BepInEx/ folder at the archive root — that marks the injector,
            // which must land at the game root, not under BepInEx/plugins.
            let marks = top_level.iter().any(|f| {
                f.eq_ignore_ascii_case("winhttp.dll")
                    || f.eq_ignore_ascii_case("doorstop_config.ini")
            });
            marks
                .then(|| self.promoted_tools().into_iter().next())
                .flatten()
        }
        fn agent_guide(&self) -> Option<String> {
            Some(
                "- **BepInEx is the foundation.** Almost every Valheim mod is a BepInEx plugin (a \
                 `.dll` under `BepInEx/plugins`). BepInEx itself is a *promoted tool*: add its \
                 archive and Concierge routes it to the game root (its `winhttp.dll` doorstop \
                 injects at launch). Without it, no plugin loads.\n\
                 - **The catalog is Thunderstore, not Nexus.** Most Valheim mods and BepInEx are \
                 published on Thunderstore; the Nexus domain exists but is secondary. Add \
                 Thunderstore mods by URL.\n\
                 - **Plugins are load-order-free.** BepInEx loads every dll in `BepInEx/plugins`; \
                 dependencies resolve at runtime. Per-plugin config appears under `BepInEx/config` \
                 after the first launch.\n\
                 - **Gotcha:** a plugin built for a different BepInEx major version (5 vs 6) won't \
                 load — keep the pack's BepInEx and its plugins on the same major."
                    .to_owned(),
            )
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
