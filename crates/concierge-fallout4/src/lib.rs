//! Fallout 4 — a Bethesda-family leaf crate. Specializes `concierge-pluginorder`'s
//! shape with this title's data (masters, exe, ini, Steam app id, plugin
//! prefix) and adds no code.

use concierge::game::GameAdapter;
use concierge_pluginorder::adapter::Bethesda;

/// Fallout 4's adapter — the Bethesda shape with this title's data.
pub static ADAPTER: Bethesda = Bethesda {
    kind_name: "fallout4",
    domain: "fallout4",
    custom_ini: "Fallout4Custom.ini",
    launchers: &["f4se_loader.exe", "Fallout4.exe"],
    steam_app: 377_160_u32,
    base_masters: &[
        "Fallout4.esm",
        "DLCRobot.esm",
        "DLCworkshop01.esm",
        "DLCCoast.esm",
        "DLCworkshop02.esm",
        "DLCworkshop03.esm",
        "DLCNukaWorld.esm",
        "DLCUltraHighResolution.esm",
    ],
    plugin_prefix: "*",
    script_extender: Some(concierge_pluginorder::adapter::ScriptExtender {
        id: "f4se",
        name: "F4SE",
        loader: "f4se_loader.exe",
        home: "https://f4se.silverlock.org/",
    }),
};

/// Resolve this leaf's game `kind` to its adapter.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "fallout4" => Some(&ADAPTER),
        _ => None,
    }
}

/// The game kinds this leaf serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["fallout4"]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use concierge::manifest::Manifest;

    #[test]
    fn identity_and_base_masters() {
        let a = super::resolve("fallout4").unwrap();
        assert_eq!(a.kind(), "fallout4");
        assert_eq!(a.nexus_domain(), Some("fallout4"));
        let m = Manifest::parse(
            "[game]\nkind = \"fallout4\"\npristine = \"\"\nversion = \"1.0\"\n\
             [game.paths]\nplugins_txt = \"/p.txt\"\nmy_games = \"/m\"\n",
        )
        .unwrap();
        let pt = a.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("*Fallout4.esm"), "base master leads: {pt}");
    }
}
