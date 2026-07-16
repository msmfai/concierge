//! Skyrim Special Edition — a Bethesda-family leaf crate. Specializes `concierge-pluginorder`'s
//! shape with this title's data (masters, exe, ini, Steam app id, plugin
//! prefix) and adds no code.

use concierge::game::GameAdapter;
use concierge_pluginorder::adapter::Bethesda;

/// Skyrim Special Edition's adapter — the Bethesda shape with this title's data.
pub static ADAPTER: Bethesda = Bethesda {
    kind_name: "skyrimse",
    domain: "skyrimspecialedition",
    custom_ini: "SkyrimCustom.ini",
    launchers: &["skse64_loader.exe", "SkyrimSE.exe"],
    steam_app: 489_830_u32,
    base_masters: &[
        "Skyrim.esm",
        "Update.esm",
        "Dawnguard.esm",
        "HearthFires.esm",
        "Dragonborn.esm",
    ],
    plugin_prefix: "*",
    script_extender: Some(concierge_pluginorder::adapter::ScriptExtender {
        id: "skse64",
        name: "SKSE64",
        loader: "skse64_loader.exe",
        home: "https://skse.silverlock.org/",
    }),
};

/// Resolve this leaf's game `kind` to its adapter.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "skyrimse" => Some(&ADAPTER),
        _ => None,
    }
}

/// The game kinds this leaf serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["skyrimse"]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use concierge::manifest::Manifest;

    #[test]
    fn identity_and_base_masters() {
        let a = super::resolve("skyrimse").unwrap();
        assert_eq!(a.kind(), "skyrimse");
        assert_eq!(a.nexus_domain(), Some("skyrimspecialedition"));
        let m = Manifest::parse(
            "[game]\nkind = \"skyrimse\"\npristine = \"\"\nversion = \"1.0\"\n\
             [game.paths]\nplugins_txt = \"/p.txt\"\nmy_games = \"/m\"\n",
        )
        .unwrap();
        let pt = a.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("*Skyrim.esm"), "base master leads: {pt}");
    }
}
