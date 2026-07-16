//! Starfield — a Bethesda-family leaf crate. Specializes `concierge-pluginorder`'s
//! shape with this title's data (masters, exe, ini, Steam app id, plugin
//! prefix) and adds no code.

use concierge::game::GameAdapter;
use concierge_pluginorder::adapter::Bethesda;

/// Starfield's adapter — the Bethesda shape with this title's data.
pub static ADAPTER: Bethesda = Bethesda {
    kind_name: "starfield",
    domain: "starfield",
    custom_ini: "StarfieldCustom.ini",
    launchers: &["sfse_loader.exe", "Starfield.exe"],
    steam_app: 1_716_740_u32,
    base_masters: &[
        "Starfield.esm",
        "Constellation.esm",
        "OldMars.esm",
        "BlueprintShips-Starfield.esm",
    ],
    plugin_prefix: "*",
    script_extender: Some(concierge_pluginorder::adapter::ScriptExtender {
        id: "sfse",
        name: "SFSE",
        loader: "sfse_loader.exe",
        home: "https://github.com/ianpatt/sfse/releases",
    }),
};

/// Resolve this leaf's game `kind` to its adapter.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "starfield" => Some(&ADAPTER),
        _ => None,
    }
}

/// The game kinds this leaf serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["starfield"]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use concierge::manifest::Manifest;

    #[test]
    fn identity_and_base_masters() {
        let a = super::resolve("starfield").unwrap();
        assert_eq!(a.kind(), "starfield");
        assert_eq!(a.nexus_domain(), Some("starfield"));
        let m = Manifest::parse(
            "[game]\nkind = \"starfield\"\npristine = \"\"\nversion = \"1.0\"\n\
             [game.paths]\nplugins_txt = \"/p.txt\"\nmy_games = \"/m\"\n",
        )
        .unwrap();
        let pt = a.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("*Starfield.esm"), "base master leads: {pt}");
    }
}
