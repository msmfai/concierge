//! Oblivion — a Bethesda-family leaf crate. Specializes `concierge-pluginorder`'s
//! shape with this title's data (masters, exe, ini, Steam app id, plugin
//! prefix) and adds no code.

use concierge::game::GameAdapter;
use concierge_pluginorder::adapter::Bethesda;

/// Oblivion's adapter — the Bethesda shape with this title's data.
pub static ADAPTER: Bethesda = Bethesda {
    kind_name: "oblivion",
    domain: "oblivion",
    custom_ini: "Oblivion.ini",
    launchers: &["obse_loader.exe", "Oblivion.exe"],
    steam_app: 22_330_u32,
    base_masters: &["Oblivion.esm", "DLCShiveringIsles.esp"],
    plugin_prefix: "",
};

/// Resolve this leaf's game `kind` to its adapter.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "oblivion" => Some(&ADAPTER),
        _ => None,
    }
}

/// The game kinds this leaf serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["oblivion"]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use concierge::manifest::Manifest;

    #[test]
    fn identity_and_base_masters() {
        let a = super::resolve("oblivion").unwrap();
        assert_eq!(a.kind(), "oblivion");
        assert_eq!(a.nexus_domain(), Some("oblivion"));
        let m = Manifest::parse(
            "[game]\nkind = \"oblivion\"\npristine = \"\"\nversion = \"1.0\"\n\
             [game.paths]\nplugins_txt = \"/p.txt\"\nmy_games = \"/m\"\n",
        )
        .unwrap();
        let pt = a.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("Oblivion.esm"), "base master leads: {pt}");
    }
}
