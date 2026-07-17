//! The Larian family adapter — Baldur's Gate 3. Mods are `.pak` archives
//! installed OUTSIDE the game directory (profile `Mods/`), activated by a UUID
//! registry (`modsettings.lsx`) the adapter renders. Moved out of
//! `concierge-core` per the game-crate-tree architecture; other Larian titles
//! would join as leaves over this crate.

pub mod adapter {
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use concierge::error::{Error, Result};
    use concierge::game::{GameAdapter, Lexicon, PromotedTool, RootTarget, BG3_LEXICON};
    use concierge::manifest::Manifest;
    use concierge::plan::{ConfigFile, GENERATED_BANNER};

    /// Larian model: mods are `.pak` archives installed OUTSIDE the game
    /// directory (profile `Mods/` under `[game.paths].profile_mods`) and
    /// activated by a UUID registry (`modsettings.lsx`) the adapter renders.
    /// `plugins` entries in the manifest carry `uuid|name` pairs.
    #[derive(Debug)]
    pub struct Bg3;

    /// BG3's Patch 8 base module — always registered first.
    const GUSTAVX_UUID: &str = "cb555efe-2d9e-131f-8195-a89329d218ea";
    /// `GustavX`'s real `Version64` (from BG3 Mod Manager's `IgnoredMods.json`);
    /// used so the base entry matches what the game/BG3MM would write.
    const GUSTAVX_VERSION64: &str = "145241946983074840";
    /// BG3's packed version 1.0.0.0 — a safe default `Version64` when a mod's
    /// real version isn't known; BG3 doesn't reject a mod on this value.
    const BG3_DEFAULT_VERSION64: &str = "36028797018963968";

    /// Is `s` a canonical GUID (8-4-4-4-12 hex)?
    fn is_bg3_guid(s: &str) -> bool {
        let mut parts = s.split('-');
        let shaped = [8usize, 4, 4, 4, 12].iter().all(|&n| {
            parts
                .next()
                .is_some_and(|p| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
        });
        shaped && parts.next().is_none()
    }

    /// Parse a BG3MM-style pak stem `"<Folder>_<UUID>"` into `(uuid, folder)`.
    /// `None` when it doesn't end in a GUID (we'd need to unpack the meta.lsx).
    fn bg3_module_from_pak_stem(stem: &str) -> Option<(&str, &str)> {
        let (folder, uuid) = stem.rsplit_once('_')?;
        is_bg3_guid(uuid).then_some((uuid, folder))
    }

    /// Read a `<attribute id="<id>" ... value="<X>"/>` value out of an LSX snippet.
    fn lsx_attr<'a>(block: &'a str, id: &str) -> Option<&'a str> {
        let after = &block[block.find(&format!("id=\"{id}\""))?..];
        let v = &after[after.find("value=\"")? + "value=\"".len()..];
        Some(&v[..v.find('"')?])
    }

    /// Derive a BG3 module `(uuid, folder, version64)` from a pak's `meta.lsx` by
    /// scanning the pak bytes for the `<save>…</save>` block that carries the
    /// `ModuleInfo` node. Works when meta.lsx is stored as (uncompressed) LSX —
    /// the common case; falls back to the filename heuristic otherwise. Avoids a
    /// full LSPK/LZ4/LSF parser.
    fn bg3_module_from_pak_bytes(bytes: &[u8]) -> Option<(String, String, String)> {
        let text = String::from_utf8_lossy(bytes);
        let mut from = 0;
        while let Some(rel) = text[from..].find("<save") {
            let start = from + rel;
            let Some(erel) = text[start..].find("</save>") else {
                break;
            };
            let end = start + erel + "</save>".len();
            let block = &text[start..end];
            from = end;
            if !block.contains("ModuleInfo") {
                continue;
            }
            if let Some(uuid) = lsx_attr(block, "UUID").filter(|u| is_bg3_guid(u)) {
                let folder = lsx_attr(block, "Folder").unwrap_or(uuid);
                let ver = lsx_attr(block, "Version64").unwrap_or(BG3_DEFAULT_VERSION64);
                return Some((uuid.to_owned(), folder.to_owned(), ver.to_owned()));
            }
        }
        None
    }

    fn write_bg3_order(out: &mut String, uuid: &str) {
        let _ = write!(
            out,
            "                    <node id=\"Module\">\n\
             \x20                       <attribute id=\"UUID\" type=\"FixedString\" value=\"{uuid}\"/>\n\
             \x20                   </node>\n"
        );
    }

    fn write_bg3_desc(out: &mut String, folder: &str, uuid: &str, version64: &str) {
        let _ = write!(
            out,
            "                    <node id=\"ModuleShortDesc\">\n\
             \x20                       <attribute id=\"Folder\" type=\"LSString\" value=\"{folder}\"/>\n\
             \x20                       <attribute id=\"MD5\" type=\"LSString\" value=\"\"/>\n\
             \x20                       <attribute id=\"Name\" type=\"LSString\" value=\"{folder}\"/>\n\
             \x20                       <attribute id=\"UUID\" type=\"FixedString\" value=\"{uuid}\"/>\n\
             \x20                       <attribute id=\"Version64\" type=\"int64\" value=\"{version64}\"/>\n\
             \x20                   </node>\n"
        );
    }

    /// Render a COMPLETE `modsettings.lsx`: a `ModOrder` node (the load order BG3
    /// actually reads) AND a `Mods` registry, base game first then each mod.
    /// Emitting only `Mods` with no `ModOrder` is why BG3 was resetting the order
    /// on launch.
    fn render_bg3_modsettings(modules: &[(String, String, String)]) -> String {
        let mut order = String::new();
        let mut mods = String::new();
        write_bg3_order(&mut order, GUSTAVX_UUID);
        write_bg3_desc(&mut mods, "GustavX", GUSTAVX_UUID, GUSTAVX_VERSION64);
        for (uuid, folder, version64) in modules {
            write_bg3_order(&mut order, uuid);
            write_bg3_desc(&mut mods, folder, uuid, version64);
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!-- {} -->\n\
             <save>\n\
             \x20   <version major=\"4\" minor=\"7\" revision=\"1\" build=\"3\"/>\n\
             \x20   <region id=\"ModuleSettings\">\n\
             \x20       <node id=\"root\">\n\
             \x20           <children>\n\
             \x20               <node id=\"ModOrder\">\n\
             \x20                   <children>\n\
             {order}\
             \x20                   </children>\n\
             \x20               </node>\n\
             \x20               <node id=\"Mods\">\n\
             \x20                   <children>\n\
             {mods}\
             \x20                   </children>\n\
             \x20               </node>\n\
             \x20           </children>\n\
             \x20       </node>\n\
             \x20   </region>\n\
             </save>\n",
            GENERATED_BANNER.trim_start_matches("; ")
        )
    }

    impl GameAdapter for Bg3 {
        fn lexicon(&self) -> Lexicon {
            BG3_LEXICON
        }
        fn kind(&self) -> &'static str {
            "bg3"
        }
        fn nexus_domain(&self) -> Option<&'static str> {
            Some("baldursgate3")
        }
        fn install_roots(&self) -> &'static [(&'static str, RootTarget)] {
            &[
                ("game", RootTarget::InstanceRel("")),
                ("mods", RootTarget::PathKey("profile_mods")),
            ]
        }
        fn default_install_root(&self) -> &'static str {
            "mods"
        }
        fn required_paths(&self) -> &'static [&'static str] {
            &["profile_mods", "modsettings"]
        }
        fn render_configs(&self, m: &Manifest, plugins: &[String]) -> Result<Vec<ConfigFile>> {
            let mut modules: Vec<(String, String, String)> = Vec::new();
            let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            // Explicit manifest entries: "uuid|Folder" (or "uuid|Folder|Version64").
            for p in plugins {
                let mut it = p.split('|');
                let (Some(uuid), Some(folder)) = (it.next(), it.next()) else {
                    return Err(Error::Manifest(format!(
                        "bg3 plugin entries are 'uuid|Folder' pairs, got '{p}'"
                    )));
                };
                let version64 = it.next().unwrap_or(BG3_DEFAULT_VERSION64);
                if seen.insert(uuid.to_owned()) {
                    modules.push((uuid.to_owned(), folder.to_owned(), version64.to_owned()));
                }
            }
            // Auto-discover deployed paks the manifest didn't declare, from the
            // meta.lsx (authoritative: real Folder + Version64), falling back to
            // the BG3MM `<Folder>_<UUID>.pak` filename — so a mod loads without
            // the user hand-authoring its UUID.
            if let Some(mods_dir) = m.game.paths.get("profile_mods") {
                if let Ok(rd) = std::fs::read_dir(mods_dir) {
                    let mut paks: Vec<PathBuf> = rd
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pak")))
                        .collect();
                    paks.sort();
                    for pak in &paks {
                        let from_meta = std::fs::read(pak)
                            .ok()
                            .and_then(|b| bg3_module_from_pak_bytes(&b));
                        let module = from_meta.or_else(|| {
                            let stem = pak.file_stem()?.to_string_lossy();
                            bg3_module_from_pak_stem(&stem).map(|(u, f)| {
                                (u.to_owned(), f.to_owned(), BG3_DEFAULT_VERSION64.to_owned())
                            })
                        });
                        if let Some((uuid, folder, ver)) = module {
                            if seen.insert(uuid.clone()) {
                                modules.push((uuid, folder, ver));
                            }
                        }
                    }
                }
            }
            let content = render_bg3_modsettings(&modules);
            let target = m.game.paths.get("modsettings").ok_or_else(|| {
                Error::Manifest("[game.paths] missing 'modsettings' for bg3".into())
            })?;
            // Read-only: BG3's in-game mod manager rewrites this on launch and
            // resets the load order unless the file can't be written (see
            // `ConfigFile::read_only`). The deployer clears the flag before any
            // re-apply, so this stays idempotent.
            Ok(vec![ConfigFile::read_only(
                target.display().to_string(),
                content,
            )])
        }
        fn config_resets(&self, m: &Manifest) -> Result<Vec<ConfigFile>> {
            // A reset means NO mods: base module only. Unlike `render_configs`,
            // this must NOT auto-discover paks still sitting on disk — otherwise
            // undeploy would leave `modsettings.lsx` referencing modules it just
            // removed. Writable, so the game's in-game mod manager takes it back.
            let content = render_bg3_modsettings(&[]);
            let target = m.game.paths.get("modsettings").ok_or_else(|| {
                Error::Manifest("[game.paths] missing 'modsettings' for bg3".into())
            })?;
            Ok(vec![ConfigFile::new(target.display().to_string(), content)])
        }
        fn launch_candidates(&self) -> &'static [&'static str] {
            &["Baldur's Gate 3.app", "bin/bg3.exe", "bin/bg3_dx11.exe"]
        }
        fn steam_app_id(&self) -> Option<u32> {
            Some(1_086_940)
        }
        fn promoted_tools(&self) -> Vec<PromotedTool> {
            vec![PromotedTool {
                id: "bg3se",
                name: "BG3 Script Extender",
                blurb:
                    "The scripting runtime many gameplay mods require; installs into the game's \
                        bin/ (the game root) as a loader the game launches through — not a .pak in \
                        the mod list.",
                home: "https://github.com/Norbyte/bg3se/releases",
                install_root: "game",
            }]
        }
        fn agent_guide(&self) -> Option<String> {
            Some(
                "- **Mods are `.pak` archives, activated by UUID in `modsettings.lsx`** — not loose \
                 files. Concierge renders `modsettings.lsx` from each pak's `meta.lsx` (module UUID \
                 + name) in load order; a pak that's present but not registered does nothing.\n\
                 - **BG3 Script Extender (BG3SE) is the scripting foundation.** Many gameplay mods \
                 need it. It's a *promoted tool*: it installs into the game's `bin/` (the game \
                 root) as a DLL loader the game launches through — add it with `install_root = \
                 \"game\"`, not as a `.pak`. Get it from the Norbyte releases.\n\
                 - **Load order matters for overrides** — later paks win on shared files/stats, and \
                 dependencies (Mod Configuration Menu, the Script Extender) must load first.\n\
                 - **Two catalogs:** Nexus (baldursgate3) hosts `.pak` mods Concierge installs; \
                 Larian's in-game **mod.io** manager hosts others that load in-game. Prefer Nexus \
                 paks for a reproducible pack."
                    .to_owned(),
            )
        }
    }

    pub static BG3: Bg3 = Bg3;

    /// Resolve a Larian-family game `kind` to its adapter.
    #[must_use]
    pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
        match kind {
            "bg3" => Some(&BG3),
            _ => None,
        }
    }

    /// The game kinds this family serves.
    #[must_use]
    pub fn kinds() -> Vec<&'static str> {
        vec!["bg3"]
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    mod tests {
        use concierge::game::GameAdapter as _;
        use concierge::manifest::Manifest;

        #[test]
        fn bg3_pak_stem_yields_uuid_and_folder() {
            assert_eq!(
                super::bg3_module_from_pak_stem("ImpUI_26922ba9-6018-5252-075d-7ff2ba6ed879"),
                Some(("26922ba9-6018-5252-075d-7ff2ba6ed879", "ImpUI"))
            );
            // a folder with underscores keeps them; only the trailing GUID splits off
            assert_eq!(
                super::bg3_module_from_pak_stem("My_Cool_Mod_aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                Some(("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "My_Cool_Mod"))
            );
            // no trailing GUID -> can't derive without unpacking meta.lsx
            assert_eq!(super::bg3_module_from_pak_stem("JustAName"), None);
            assert_eq!(super::bg3_module_from_pak_stem("Name_notaguid"), None);
        }

        #[test]
        fn bg3_meta_lsx_extracted_from_pak_bytes() {
            // an uncompressed meta.lsx <save> block embedded between binary noise.
            let meta = r#"<save><version major="4"/><region id="Config"><node id="root"><children><node id="ModuleInfo"><attribute id="Folder" type="LSString" value="ImpUI"/><attribute id="Name" type="LSString" value="ImpUI"/><attribute id="UUID" type="FixedString" value="26922ba9-6018-5252-075d-7ff2ba6ed879"/><attribute id="Version64" type="int64" value="144396667511017472"/></node></children></node></region></save>"#;
            let mut bytes = vec![0u8, 1, 2, 255, 200];
            bytes.extend_from_slice(meta.as_bytes());
            bytes.extend_from_slice(&[0xFF, 0x00, 0x13]);
            let (uuid, folder, ver) = super::bg3_module_from_pak_bytes(&bytes).unwrap();
            assert_eq!(uuid, "26922ba9-6018-5252-075d-7ff2ba6ed879");
            assert_eq!(folder, "ImpUI");
            assert_eq!(ver, "144396667511017472");
            // no ModuleInfo -> None
            assert!(super::bg3_module_from_pak_bytes(b"just some bytes").is_none());
        }

        #[test]
        fn bg3_modsettings_has_modorder_and_lists_the_mod() {
            let mods = vec![(
                "26922ba9-6018-5252-075d-7ff2ba6ed879".to_owned(),
                "ImpUI".to_owned(),
                super::BG3_DEFAULT_VERSION64.to_owned(),
            )];
            let out = super::render_bg3_modsettings(&mods);
            // the missing-ModOrder bug is fixed
            assert!(
                out.contains(r#"<node id="ModOrder">"#),
                "has ModOrder: {out}"
            );
            assert!(out.contains(r#"<node id="Mods">"#), "has Mods");
            // the mod appears in BOTH the order and the registry, with a version
            assert_eq!(
                out.matches("26922ba9-6018-5252-075d-7ff2ba6ed879").count(),
                2
            );
            assert!(out.contains(r#"value="ImpUI""#), "folder listed");
            assert!(out.contains(r#"id="Version64""#), "version present");
            // base game still registered first, with its REAL Version64 (matches
            // BG3MM's IgnoredMods.json), not the generic default
            assert!(out.contains(super::GUSTAVX_UUID));
            assert!(
                out.contains(super::GUSTAVX_VERSION64),
                "GustavX real version present"
            );
        }

        #[test]
        fn bg3_reset_is_base_only_even_with_paks_on_disk() {
            // a profile_mods dir that still holds a deployed pak
            let dir = std::env::temp_dir().join(format!("bg3-reset-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let pak = dir.join("ImpUI_26922ba9-6018-5252-075d-7ff2ba6ed879.pak");
            std::fs::write(&pak, b"fake").unwrap();
            let mods = dir.display();
            let m = Manifest::parse(&format!(
                "[game]\nkind = \"bg3\"\npristine = \"/game\"\nversion = \"patch8\"\n[game.paths]\nprofile_mods = \"{mods}\"\nmodsettings = \"{mods}/modsettings.lsx\"\n"
            ))
            .unwrap();

            // render_configs auto-discovers the pak (deploy path)…
            let deploy = super::BG3.render_configs(&m, &[]).unwrap();
            assert!(deploy[0].content.contains("ImpUI"), "deploy lists the pak");
            assert!(deploy[0].read_only, "deployed modsettings is read-only");

            // …but a RESET must be base-only (GustavX) and writable, never listing
            // a pak it's about to leave behind.
            let reset = super::BG3.config_resets(&m).unwrap();
            assert!(
                reset[0].content.contains("GustavX"),
                "reset keeps base module"
            );
            assert!(!reset[0].content.contains("ImpUI"), "reset drops the pak");
            assert!(!reset[0].read_only, "reset hands the file back to the game");

            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
