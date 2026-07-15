//! Fallout New Vegas — a Bethesda-family leaf crate. Specializes `concierge-pluginorder`'s
//! shape with this title's data (masters, exe, ini, Steam app id, plugin
//! prefix) and adds no code.

use concierge::game::GameAdapter;
use concierge_pluginorder::adapter::Bethesda;

/// Fallout New Vegas's adapter — the Bethesda shape with this title's data.
pub static ADAPTER: Bethesda = Bethesda {
    kind_name: "newvegas",
    domain: "newvegas",
    custom_ini: "FalloutCustom.ini",
    launchers: &["nvse_loader.exe", "FalloutNV.exe"],
    steam_app: 22_380_u32,
    base_masters: &[
        "FalloutNV.esm",
        "DeadMoney.esm",
        "HonestHearts.esm",
        "OldWorldBlues.esm",
        "LonesomeRoad.esm",
        "GunRunnersArsenal.esm",
        "ClassicPack.esm",
        "MercenaryPack.esm",
        "TribalPack.esm",
        "CaravanPack.esm",
    ],
    plugin_prefix: "",
};

/// Resolve this leaf's game `kind` to its adapter.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    match kind {
        "newvegas" => Some(&ADAPTER),
        _ => None,
    }
}

/// The game kinds this leaf serves.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    vec!["newvegas"]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use concierge::manifest::Manifest;

    #[test]
    fn identity_and_base_masters() {
        let a = super::resolve("newvegas").unwrap();
        assert_eq!(a.kind(), "newvegas");
        assert_eq!(a.nexus_domain(), Some("newvegas"));
        let m = Manifest::parse(
            "[game]\nkind = \"newvegas\"\npristine = \"\"\nversion = \"1.0\"\n\
             [game.paths]\nplugins_txt = \"/p.txt\"\nmy_games = \"/m\"\n",
        )
        .unwrap();
        let pt = a.render_configs(&m, &[]).unwrap()[0].content.clone();
        assert!(pt.contains("FalloutNV.esm"), "base master leads: {pt}");
    }
}
