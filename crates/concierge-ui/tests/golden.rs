//! Golden-snapshot tests: the agent-facing text rendering of each automaton
//! state/window must match a committed file. Run with `BLESS=1` to regenerate
//! after an intentional change — a change to the actionable surface then shows
//! up as a reviewable golden diff. Runs in CI (Linux, no display).
#![allow(clippy::unwrap_used, clippy::panic)]

use concierge_ui::{build_screen, render_text, ConfirmKind, ModRow, TabFacts, UiFacts};

fn base() -> UiFacts {
    UiFacts {
        has_workspace: true,
        workspace_path: Some("/ws".into()),
        game_count: 2,
        active_game: Some("skyrimse".into()),
        active_profile: Some("modpack".into()),
        is_bethesda: true,
        has_catalog: true,
        tab: TabFacts(concierge_ui::Tab::Setup),
        mutable: true,
        order_word: "load order".into(),
        sort_label: "Sort load order".into(),
        mods: vec![
            ModRow {
                order: 1,
                name: "skse64".into(),
                enabled: true,
            },
            ModRow {
                order: 2,
                name: "SkyUI".into(),
                enabled: true,
            },
        ],
        ai_can_send: true,
        games: vec!["skyrimse".into(), "fallout4".into()],
        profiles: vec!["modpack".into()],
        ..UiFacts::default()
    }
}

fn golden(name: &str, facts: &UiFacts) {
    let got = render_text(&build_screen(facts));
    let path = format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("BLESS").is_ok() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        got, want,
        "golden mismatch for {name} — run BLESS=1 to update"
    );
}

#[test]
fn editing_screen_golden() {
    golden("editing", &base());
}

#[test]
fn no_workspace_golden() {
    golden("no_workspace", &UiFacts::default());
}

#[test]
fn locked_golden() {
    let mut f = base();
    f.mutable = false;
    golden("locked", &f);
}

#[test]
fn settings_golden() {
    let mut f = base();
    f.settings_open = true;
    f.panels = vec![concierge_ui::Panel {
        id: "settings".into(),
        title: "Settings".into(),
        lines: vec![
            "workspace: /ws (2 games)".into(),
            "game paths [game.paths]:".into(),
        ],
    }];
    golden("settings", &f);
}

#[test]
fn browse_golden() {
    let mut f = base();
    f.browse_open = true;
    // A rich Nexus-style hit + one already in the manifest, so the projection
    // of both the add-to-manifest and "already added" states is locked.
    f.browse_hits = vec![
        concierge_ui::BrowseHit {
            mod_id: 12604,
            name: "SkyUI".to_owned(),
            endorsements: 300_000,
            author: "SkyUI Team".to_owned(),
            summary: "An elegant, PC-friendly interface mod.".to_owned(),
            category: "User Interface".to_owned(),
            downloads: 9_000_000,
            updated_at: "2019-05-01".to_owned(),
            added: true,
        },
        concierge_ui::BrowseHit {
            mod_id: 3863,
            name: "SkySight Skins".to_owned(),
            endorsements: 12_000,
            author: "fadingsignal".to_owned(),
            summary: "High-res male textures.".to_owned(),
            category: "Models and Textures".to_owned(),
            downloads: 200_000,
            updated_at: "2020-01-02".to_owned(),
            added: false,
        },
    ];
    golden("browse", &f);
}

#[test]
fn preview_golden() {
    let mut f = base();
    f.diff_open = true;
    golden("preview", &f);
}

#[test]
fn confirming_uninstall_golden() {
    let mut f = base();
    f.confirm = Some(ConfirmKind::Uninstall);
    f.confirm_prompt = Some("Uninstall — remove the installed mods from the game?".into());
    golden("confirming_uninstall", &f);
}

#[test]
fn ai_running_golden() {
    let mut f = base();
    f.ai_busy = true;
    golden("ai_running", &f);
}
