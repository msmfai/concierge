//! The per-profile agent guide (CLAUDE.md) must speak the GAME's modding norms,
//! not the same generic advice for every title. Regression for the agent-context
//! half of a game's opinion: `GameAdapter::agent_guide` is appended to the
//! generic guide by `provision_profile`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn agent_guide_speaks_the_games_modding_norms() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-prov-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let created = concierge::provision::provision_profile(&base, "fallout4").unwrap();
    assert!(
        created.iter().any(|f| f == "CLAUDE.md"),
        "CLAUDE.md provisioned: {created:?}"
    );
    let guide = std::fs::read_to_string(base.join("CLAUDE.md")).unwrap();

    // The generic scaffold is still there.
    assert!(
        guide.contains("Concierge agent guide"),
        "generic guide kept"
    );
    assert!(
        guide.contains("concierge realize"),
        "command semantics kept"
    );

    // …and the Fallout 4 adapter's own norms are appended under a game heading.
    assert!(
        guide.contains("Modding fallout4 — community norms"),
        "game section header present:\n{guide}"
    );
    assert!(
        guide.contains("F4SE"),
        "names THIS game's script extender, not a generic one"
    );
    assert!(
        guide.contains("Load order is the whole game"),
        "Bethesda load-order norm present"
    );
    assert!(
        guide.contains("bInvalidateOlderFiles"),
        "archive-invalidation gotcha present"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// The load-order UI is gated on an adapter capability (base masters present),
/// not a hardcoded `fallout4|skyrimse` list — so EVERY Bethesda title lights it
/// up, and non-plugin games don't.
#[test]
fn every_bethesda_title_reports_a_plugin_order() {
    use concierge::game::GameAdapter as _;
    concierge_games::register();
    for kind in [
        "fallout4",
        "skyrimse",
        "skyrim",
        "oblivion",
        "fallout3",
        "newvegas",
        "starfield",
    ] {
        let a = concierge::game::adapter_for(kind).unwrap();
        assert!(
            a.plugin_bases().is_some(),
            "{kind} must report a plugin load order (load-order UI gate)"
        );
    }
    for kind in ["stardew", "minecraft", "valheim", "cyberpunk2077"] {
        let a = concierge::game::adapter_for(kind).unwrap();
        assert!(
            a.plugin_bases().is_none(),
            "{kind} is not a plugin-order game"
        );
    }
}

/// Each specialist game injects its OWN norms — not a shared blurb. A handful of
/// distinct games must each name the foundational tool / catalog their community
/// actually uses.
#[test]
fn each_game_speaks_its_own_norms() {
    concierge_games::register();
    let base = std::env::temp_dir().join(format!("cg-prov-multi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);

    // (game kind, a phrase that must appear in ITS guide and no generic one)
    let cases = [
        ("valheim", "BepInEx"),
        ("minecraft", "Modrinth"),
        ("stardew", "SMAPI"),
        ("rimworld", "Harmony"),
        ("bg3", "BG3 Script Extender"),
        ("kotor2", "TSLRCM"),
        ("cyberpunk2077", "RED4ext"),
        ("eldenring", "Mod Engine 2"),
        ("residentevil42023", "REFramework"),
        ("thesims4", ".ts4script"),
    ];
    for (kind, needle) in cases {
        let dir = base.join(kind);
        concierge::provision::provision_profile(&dir, kind).unwrap();
        let guide = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        assert!(
            guide.contains(&format!("Modding {kind} — community norms")),
            "{kind}: game section header present"
        );
        assert!(
            guide.contains(needle),
            "{kind}: guide names '{needle}':\n{guide}"
        );
    }

    let _ = std::fs::remove_dir_all(&base);
}
