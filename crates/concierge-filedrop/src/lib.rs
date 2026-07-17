//! The loose-drop family — games modded by copying files into a fixed mod
//! folder, with no load-order file to render. One parameterized shape
//! ([`LooseDrop`]); each game is a data entry (its mod subdir + launch + Steam
//! app id). New loose games are one const + one static + one `resolve` arm.

pub mod adapter {
    use concierge::error::Result;
    use concierge::game::{GameAdapter, Lexicon, RootTarget, MODLIST_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::ConfigFile;

    /// A game whose mods are loose files dropped into `roots["mods"]`; ordering
    /// (if any) is resolved by the game/loader at runtime, so nothing is rendered.
    #[derive(Debug)]
    pub struct LooseDrop {
        pub kind_name: &'static str,
        pub domain: &'static str,
        /// Install roots; must include a `"mods"` entry (the default root).
        pub roots: &'static [(&'static str, RootTarget)],
        pub launchers: &'static [&'static str],
        pub steam_app: Option<u32>,
        /// `[game.paths]` keys the manifest must supply — non-empty when a mods
        /// root is a `PathKey` (mods live outside the game dir, e.g. The Sims 4
        /// under Documents), empty for instance-relative games.
        pub required: &'static [&'static str],
    }

    impl GameAdapter for LooseDrop {
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
            self.roots
        }
        fn default_install_root(&self) -> &'static str {
            "mods"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            self.required
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
        // A long but flat data table: one match arm of prose per game/family.
        #[allow(clippy::too_many_lines)]
        fn agent_guide(&self) -> Option<String> {
            // Loose-drop covers wildly different scenes; each names the tools and
            // layout ITS community actually uses. Games without a specific note
            // fall through to the generic guide (None). Frameworks that live at
            // the game root (RED4ext, UE4SS, REFramework, …) install with
            // `install_root = "game"`, not the default content folder.
            let g: &str = match self.kind_name {
                "cyberpunk2077" => {
                    "- **Mods layer on frameworks that live in different folders.** The stack: \
                     **RED4ext** (native plugins), **Cyber Engine Tweaks / CET** (Lua), \
                     **redscript** (`r6/scripts`), then **ArchiveXL + TweakXL + Codeware**. Game \
                     content `.archive` files go in `archive/pc/mod` (the default `mods` root here).\n\
                     - **Install the frameworks first, to the game root** (`install_root = \"game\"`) \
                     — they deploy under the game dir, not `archive/pc/mod`. Content archives use the \
                     default.\n\
                     - **Everything must match the game patch** — a CET/RED4ext built for an older \
                     version silently fails after an update.\n\
                     - **Mods live on Nexus (cyberpunk2077).**"
                }
                "eldenring" => {
                    "- **Mod through Mod Engine 2 — never touch the game files.** ME2 loads a mod \
                     folder at launch without modifying the install; run OFFLINE with EAC disabled \
                     when modded, or risk a ban.\n\
                     - **Seamless Co-op** is the standard co-op mod (its own launcher + save \
                     namespace).\n\
                     - **`regulation.bin` is the conflict hotspot** — most gameplay mods edit it and \
                     two can't both win without a merge; install one or use a regulation merger.\n\
                     - **Mods live on Nexus (eldenring).**"
                }
                "witcher3" => {
                    "- **Script conflicts are resolved by Script Merger, not Concierge** — if two \
                     mods edit the same `.ws` script, run Script Merger to produce \
                     `Mods/mod0000_MergedFiles`. Concierge deploys the mods; the merge is a manual \
                     tool step.\n\
                     - **Two mod types:** `Mods/` (gameplay/scripts) and `dlc/` (content); menu xmls \
                     need entries in `input.settings`/`user.settings`.\n\
                     - **Next-gen (4.x) vs classic (1.32)** broke many mods — match each mod to your \
                     game version.\n\
                     - **Mods live on Nexus (witcher3).**"
                }
                "morrowind" => {
                    "- **Morrowind is really a plugin-order game, but Concierge currently deploys it \
                     as a loose drop into `Data Files` — so it does NOT render the `Morrowind.ini` \
                     load order for you.** Set `[Game Files]` order in `Morrowind.ini` (or use Wrye \
                     Mash / mlox) after deploy.\n\
                     - **Foundational tools:** **MWSE** (script extender, needed by most modern and \
                     all MWSE-Lua mods) and **MGE XE** (distant land/graphics). **OpenMW** is a full \
                     engine replacement with its own mod handling — a separate path.\n\
                     - **Masters lead:** `Morrowind.esm` + `Tribunal.esm`/`Bloodmoon.esm` first; a \
                     plugin missing its master won't load.\n\
                     - **Mods live on Nexus (morrowind).**"
                }
                "fallout76" => {
                    "- **Fallout 76 is online — modding is deliberately limited and risky.** No \
                     script extender; only loose files under `Data` (textures, meshes, sound, UI, \
                     ini edits). Anything touching gameplay or the economy can get you banned.\n\
                     - **Keep it cosmetic/QoL** — texture, UI (`.swf`), and sound replacers are the \
                     accepted category; enable loose files with archive invalidation in the ini.\n\
                     - **Mods live on Nexus (fallout76).**"
                }
                "thesims4" => {
                    "- **Two mod kinds, different rules.** `.package` files (CC, tuning) go anywhere \
                     under `Mods/` (up to 5 folders deep). `.ts4script` mods (Python) must sit **at \
                     most one folder deep** or they won't load, and need 'Script Mods Enabled' in \
                     game options.\n\
                     - **Mods live under Documents** (`Electronic Arts/The Sims 4/Mods`), outside the \
                     game — the pack points `[game.paths].mods` there; `Resource.cfg` controls \
                     package scanning.\n\
                     - **After every patch,** delete `localthumbcache.package` and re-check script \
                     mods — EA patches routinely break them.\n\
                     - **Mods live on Nexus (thesims4).**"
                }
                "dragonage" => {
                    "- **Two install paths.** `.dazip` mods install through DAUpdater/DAModder into \
                     the AddIns registry (`addins.xml`); loose override files drop into `override/` \
                     and win over the game's data.\n\
                     - **Override precedence is filename-based** — prefix files to control which \
                     wins.\n\
                     - **Mods live on Nexus (dragonage).**"
                }
                "reddeadredemption2" => {
                    "- **Lenny's Mod Loader (LML) is the loader** — `lml/` and script/ASI mods load \
                     through it, alongside **ScriptHookRDR2**. Install both first.\n\
                     - **Play story mode offline when modded** — RDR Online bans for mods.\n\
                     - **Mods live on Nexus (reddeadredemption2).**"
                }
                "nomanssky" => {
                    "- **`.pak` mods drop into `GAMEDATA/PCBANKS/MODS`, and you must delete \
                     `DISABLEMODS.txt`** for the game to read them. Only one mod can edit a given \
                     game file — conflicts need merging (AMUMSS generates compatible paks).\n\
                     - **Every update breaks mods** — NMS patches often; rebuild paks against the new \
                     version.\n\
                     - **Mods live on Nexus (nomanssky).**"
                }
                "mountandblade2bannerlord" => {
                    "- **Mods are Modules under `Modules/`, ordered in the launcher.** The framework \
                     stack most mods need, in load order: **Harmony**, **ButterLib**, \
                     **UIExtenderEx**, **MCM** — install and load these first.\n\
                     - **Load order matters** (dependencies before dependents); the official launcher \
                     persists it.\n\
                     - **Mods live on Nexus (mountandblade2bannerlord) and Steam Workshop.**"
                }
                "7daystodie" => {
                    "- **Each mod is a folder under `Mods/` with a `ModInfo.xml`.** Pure-XML (XPath) \
                     mods stack safely; DLL/Harmony mods need EAC disabled to load.\n\
                     - **Match the game version** (Alpha vs 1.0) — 7DTD mods break hard across major \
                     versions.\n\
                     - **Mods live on Nexus (7daystodie) and the official forums.**"
                }
                "oblivionremastered" => {
                    "- **A UE5 remaster over the original Gamebryo engine — two mod flavours.** UE \
                     `.pak` mods drop into `~mods` (the default here); classic-engine plugins/loose \
                     files go under the bundled `Data` with archive invalidation. **UE4SS** \
                     (Lua/Blueprint) and **OBSE64** (engine-side scripts) are the foundational \
                     tools.\n\
                     - **Install frameworks to the game root** (`install_root = \"game\"`); pak \
                     content uses the default.\n\
                     - **Mods live on Nexus (oblivionremastered).**"
                }
                "residentevil42023" | "devilmaycry5" | "monsterhunterrise" | "monsterhunterwilds"
                | "streetfighter6" => {
                    "- **REFramework is the mod foundation** for this RE Engine title — a \
                     `dinput8.dll` loader at the game root that hosts Lua scripts \
                     (`reframework/autorun`) and enables `.pak` mods. Install it first, to the game \
                     root (`install_root = \"game\"`).\n\
                     - **Fluffy Mod Manager** is the community-standard tool; mods are `.pak`s and \
                     `natives/` overrides.\n\
                     - **Match the game version** — RE Engine updates routinely break REFramework and \
                     its scripts.\n\
                     - **Mods live on Nexus.**"
                }
                "palworld" | "readyornot" | "acecombat7skiesunknown" | "marvelrivals"
                | "stellarblade" => {
                    "- **Unreal Engine `.pak` mods** drop into the `~mods` folder (the default here) \
                     and load by name — a later pak wins on shared assets. For script/Lua/Blueprint \
                     mods, **UE4SS** (a `dwmapi.dll`/`xinput` loader in the game's binaries folder) \
                     is the foundation — install it to the game root (`install_root = \"game\"`) \
                     first.\n\
                     - **Anti-cheat/online caution:** several of these are live-service — run modded \
                     content offline/solo where allowed; online play will reject modified paks.\n\
                     - **Mods live on Nexus.**"
                }
                _ => return None,
            };
            Some(g.to_owned())
        }
    }

    // Each game: a const roots slice (the mod folder is the only real datum) +
    // a static LooseDrop. Verified mod dirs; deploy copies files there.
    macro_rules! loose {
        ($stat:ident, $roots:ident, $kind:literal, $domain:literal, $mods:literal,
         $launchers:expr, $app:expr) => {
            const $roots: &[(&str, RootTarget)] = &[
                ("game", RootTarget::InstanceRel("")),
                ("mods", RootTarget::InstanceRel($mods)),
            ];
            pub static $stat: LooseDrop = LooseDrop {
                kind_name: $kind,
                domain: $domain,
                roots: $roots,
                launchers: $launchers,
                steam_app: $app,
                required: &[],
            };
        };
    }

    // External-path variant: mods live OUTSIDE the game dir, at a manifest-
    // declared `[game.paths].mods` (e.g. The Sims 4 / Dragon Age under Documents).
    macro_rules! loose_ext {
        ($stat:ident, $roots:ident, $kind:literal, $domain:literal,
         $launchers:expr, $app:expr) => {
            const $roots: &[(&str, RootTarget)] = &[
                ("game", RootTarget::InstanceRel("")),
                ("mods", RootTarget::PathKey("mods")),
            ];
            pub static $stat: LooseDrop = LooseDrop {
                kind_name: $kind,
                domain: $domain,
                roots: $roots,
                launchers: $launchers,
                steam_app: $app,
                required: &["mods"],
            };
        };
    }

    loose!(
        CYBERPUNK2077,
        CP_ROOTS,
        "cyberpunk2077",
        "cyberpunk2077",
        "archive/pc/mod",
        &["bin/x64/Cyberpunk2077.exe"],
        Some(1_091_500)
    );
    loose!(
        WITCHER3,
        W3_ROOTS,
        "witcher3",
        "witcher3",
        "Mods",
        &["bin/x64/witcher3.exe", "bin/x64_dx12/witcher3.exe"],
        Some(292_030)
    );
    loose!(
        MONSTERHUNTERWORLD,
        MHW_ROOTS,
        "monsterhunterworld",
        "monsterhunterworld",
        "nativePC",
        &["MonsterHunterWorld.exe"],
        Some(582_010)
    );
    loose!(
        ELDENRING,
        ER_ROOTS,
        "eldenring",
        "eldenring",
        "mods",
        &["Game/eldenring.exe"],
        Some(1_245_620)
    );
    loose!(
        RDR2,
        RDR2_ROOTS,
        "reddeadredemption2",
        "reddeadredemption2",
        "lml",
        &["RDR2.exe"],
        Some(1_174_180)
    );
    loose!(
        PALWORLD,
        PAL_ROOTS,
        "palworld",
        "palworld",
        "Pal/Content/Paks/~mods",
        &["Palworld.exe"],
        Some(1_623_730)
    );
    loose!(
        NOMANSSKY,
        NMS_ROOTS,
        "nomanssky",
        "nomanssky",
        "GAMEDATA/PCBANKS/MODS",
        &["Binaries/NMS.exe"],
        Some(275_850)
    );
    loose!(
        SEVENDAYS,
        SEVEN_ROOTS,
        "7daystodie",
        "7daystodie",
        "Mods",
        &["7DaysToDie.exe"],
        Some(251_570)
    );
    loose!(
        BANNERLORD,
        BANNER_ROOTS,
        "mountandblade2bannerlord",
        "mountandblade2bannerlord",
        "Modules",
        &["bin/Win64_Shipping_Client/Bannerlord.exe"],
        Some(261_550)
    );
    loose!(
        KCD2,
        KCD2_ROOTS,
        "kingdomcomedeliverance2",
        "kingdomcomedeliverance2",
        "Mods",
        &["bin/Win64MasterMasterSteamPGO/KingdomCome.exe"],
        Some(1_771_300)
    );
    loose!(
        MORROWIND,
        MW_ROOTS,
        "morrowind",
        "morrowind",
        "Data Files",
        &["Morrowind.exe"],
        Some(22_320)
    );
    loose!(
        BLADEANDSORCERY,
        BAS_ROOTS,
        "bladeandsorcery",
        "bladeandsorcery",
        "BladeAndSorcery_Data/StreamingAssets/Mods",
        &["BladeAndSorcery.exe"],
        Some(629_730)
    );
    loose!(
        MYSUMMERCAR,
        MSC_ROOTS,
        "mysummercar",
        "mysummercar",
        "Mods",
        &["mysummercar.exe"],
        Some(516_750)
    );
    loose!(
        HELLDIVERS2,
        HD2_ROOTS,
        "helldivers2",
        "helldivers2",
        "data",
        &["bin/helldivers2.exe"],
        Some(553_850)
    );
    loose!(
        FALLOUT76,
        F76_ROOTS,
        "fallout76",
        "fallout76",
        "Data",
        &["Fallout76.exe"],
        Some(1_151_340)
    );
    loose!(
        READYORNOT,
        RON_ROOTS,
        "readyornot",
        "readyornot",
        "ReadyOrNot/Content/Paks/~mods",
        &["ReadyOrNot.exe"],
        Some(1_144_200)
    );
    loose!(
        ACECOMBAT7,
        AC7_ROOTS,
        "acecombat7skiesunknown",
        "acecombat7skiesunknown",
        "Nimbus/Content/Paks/~mods",
        &["Ace7Game.exe"],
        Some(502_500)
    );
    // RE Engine titles — REFramework loader (mods under reframework/):
    loose!(
        RE4,
        RE4_ROOTS,
        "residentevil42023",
        "residentevil42023",
        "reframework",
        &["re4.exe"],
        Some(2_050_650)
    );
    loose!(
        DMC5,
        DMC5_ROOTS,
        "devilmaycry5",
        "devilmaycry5",
        "reframework",
        &["DevilMayCry5.exe"],
        Some(601_150)
    );
    loose!(
        MHRISE,
        MHR_ROOTS,
        "monsterhunterrise",
        "monsterhunterrise",
        "reframework",
        &["MonsterHunterRise.exe"],
        Some(1_446_780)
    );
    loose!(
        MHWILDS,
        MHW2_ROOTS,
        "monsterhunterwilds",
        "monsterhunterwilds",
        "reframework",
        &["MonsterHunterWilds.exe"],
        Some(2_246_340)
    );
    // UE pak mods (~mods). Anticheat titles deploy cleanly; running them online
    // modded is the user's risk, not a deploy blocker.
    loose!(
        OBLIVIONREMASTERED,
        OBR_ROOTS,
        "oblivionremastered",
        "oblivionremastered",
        "OblivionRemastered/Content/Paks/~mods",
        &["OblivionRemastered.exe"],
        Some(2_623_190)
    );
    loose!(
        MARVELRIVALS,
        MR_ROOTS,
        "marvelrivals",
        "marvelrivals",
        "MarvelGame/Marvel/Content/Paks/~mods",
        &["MarvelRivals_Launcher.exe"],
        Some(2_767_030)
    );
    loose!(
        STELLARBLADE,
        SB_ROOTS,
        "stellarblade",
        "stellarblade",
        "SB/Content/Paks/~mods",
        &["SB.exe"],
        Some(3_489_700)
    );
    loose!(
        SF6,
        SF6_ROOTS,
        "streetfighter6",
        "streetfighter6",
        "reframework",
        &["StreetFighter6.exe"],
        Some(1_364_780)
    );
    // External-path (Documents) mods:
    loose_ext!(
        SIMS4,
        SIMS4_ROOTS,
        "thesims4",
        "thesims4",
        &["Game/Bin/TS4_x64.exe"],
        Some(1_222_670)
    );
    loose_ext!(
        DRAGONAGE,
        DAO_ROOTS,
        "dragonage",
        "dragonage",
        &["bin_ship/daorigins.exe"],
        Some(47_810)
    );
    // Blade & Sorcery: Nomad (Quest) — loose mods into a Mods folder on headset
    // storage; the manifest points [game.paths].mods at the mounted/adb path.
    loose_ext!(
        BASNOMAD,
        BASN_ROOTS,
        "bladeandsorcerynomad",
        "bladeandsorcerynomad",
        &[],
        None
    );

    const ALL: &[&LooseDrop] = &[
        &CYBERPUNK2077,
        &WITCHER3,
        &MONSTERHUNTERWORLD,
        &ELDENRING,
        &RDR2,
        &PALWORLD,
        &NOMANSSKY,
        &SEVENDAYS,
        &BANNERLORD,
        &KCD2,
        &MORROWIND,
        &BLADEANDSORCERY,
        &MYSUMMERCAR,
        &HELLDIVERS2,
        &FALLOUT76,
        &READYORNOT,
        &ACECOMBAT7,
        &RE4,
        &DMC5,
        &MHRISE,
        &MHWILDS,
        &OBLIVIONREMASTERED,
        &MARVELRIVALS,
        &STELLARBLADE,
        &SF6,
        &SIMS4,
        &DRAGONAGE,
        &BASNOMAD,
    ];

    /// Resolve a loose-drop game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        ALL.iter().find(|g| g.kind_name == kind).map(|g| {
            let a: &'static dyn GameAdapter = *g;
            a
        })
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        ALL.iter().map(|g| g.kind_name).collect()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    mod tests {

        #[test]
        fn every_loose_game_has_a_mods_root_and_no_configs() {
            for kind in super::kinds() {
                let a = super::resolve(kind).unwrap();
                assert_eq!(a.kind(), kind);
                assert!(a.nexus_domain().is_some(), "{kind}: domain");
                assert!(
                    a.install_roots().iter().any(|(n, _)| *n == "mods"),
                    "{kind}: has a mods root"
                );
                assert_eq!(a.default_install_root(), "mods");
            }
        }

        #[test]
        fn cyberpunk_mods_land_in_the_redmod_archive_dir() {
            let a = super::resolve("cyberpunk2077").unwrap();
            let (_, target) = a
                .install_roots()
                .iter()
                .find(|(n, _)| *n == "mods")
                .unwrap();
            assert_eq!(format!("{target:?}"), r#"InstanceRel("archive/pc/mod")"#);
        }
    }
}
