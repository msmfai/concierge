//! The game-crate assembly. Each family/leaf crate owns its adapters; this crate
//! is the single place that wires them into `concierge-core`'s resolver at
//! process start. Adding a game means adding its crate as a dependency here plus
//! an arm in [`resolve`] (and its [`kinds`]) — never editing core.
//!
//! During the crate-tree migration, families not yet extracted still resolve via
//! core's shrinking built-in `ALL`; this crate only advertises the migrated ones.

use concierge::game::GameAdapter;

/// Resolve a game `kind` to the adapter owned by its family crate. Families are
/// tried in order; the first that owns `kind` wins.
#[must_use]
pub fn resolve(kind: &str) -> Option<&'static dyn GameAdapter> {
    // Bethesda family leaves (data-only specializations of concierge-pluginorder):
    concierge_skyrimse::resolve(kind)
        .or_else(|| concierge_fallout4::resolve(kind))
        .or_else(|| concierge_skyrim::resolve(kind))
        .or_else(|| concierge_newvegas::resolve(kind))
        .or_else(|| concierge_fallout3::resolve(kind))
        .or_else(|| concierge_oblivion::resolve(kind))
        .or_else(|| concierge_starfield::resolve(kind))
        // Other families:
        .or_else(|| concierge_override::adapter::resolve(kind))
        .or_else(|| concierge_modfolders::adapter::resolve(kind))
        .or_else(|| concierge_modlist::adapter::resolve(kind))
        .or_else(|| concierge_jarmods::adapter::resolve(kind))
        .or_else(|| concierge_runtimeinject::adapter::resolve(kind))
        .or_else(|| concierge_pakregistry::adapter::resolve(kind))
        .or_else(|| concierge_filedrop::adapter::resolve(kind))
        .or_else(|| concierge_toolmerge::adapter::resolve(kind))
}

/// Every kind the assembled registry can resolve.
#[must_use]
pub fn kinds() -> Vec<&'static str> {
    let mut ks: Vec<&'static str> = Vec::new();
    ks.extend(concierge_skyrimse::kinds());
    ks.extend(concierge_fallout4::kinds());
    ks.extend(concierge_skyrim::kinds());
    ks.extend(concierge_newvegas::kinds());
    ks.extend(concierge_fallout3::kinds());
    ks.extend(concierge_oblivion::kinds());
    ks.extend(concierge_starfield::kinds());
    ks.extend(concierge_override::adapter::kinds());
    ks.extend(concierge_modfolders::adapter::kinds());
    ks.extend(concierge_modlist::adapter::kinds());
    ks.extend(concierge_jarmods::adapter::kinds());
    ks.extend(concierge_runtimeinject::adapter::kinds());
    ks.extend(concierge_pakregistry::adapter::kinds());
    ks.extend(concierge_filedrop::adapter::kinds());
    ks.extend(concierge_toolmerge::adapter::kinds());
    ks.sort_unstable();
    ks.dedup();
    ks
}

/// A human, unambiguous name for a game `kind` slug — so the pickers read
/// "Skyrim Special Edition" / "Skyrim (Legendary Edition)" instead of two bare
/// "skyrim"/"skyrimse" rows, and "Cyberpunk 2077" instead of "cyberpunk2077".
/// Unknown kinds fall back to a prettified slug, so new games still read okay.
#[must_use]
pub fn display_name(kind: &str) -> String {
    let known = match kind {
        "skyrimse" => "Skyrim Special Edition",
        "skyrim" => "Skyrim (Legendary Edition)",
        "fallout4" => "Fallout 4",
        "fallout3" => "Fallout 3",
        "newvegas" | "falloutnv" => "Fallout: New Vegas",
        "oblivion" => "Oblivion",
        "starfield" => "Starfield",
        "cyberpunk2077" => "Cyberpunk 2077",
        "bg3" => "Baldur's Gate 3",
        "eldenring" => "Elden Ring",
        "stardewvalley" => "Stardew Valley",
        "rimworld" => "RimWorld",
        "valheim" => "Valheim",
        "minecraft" => "Minecraft",
        "thesims4" => "The Sims 4",
        "witcher3" => "The Witcher 3",
        "7daystodie" => "7 Days to Die",
        "devilmaycry5" => "Devil May Cry 5",
        "dragonage" => "Dragon Age: Origins",
        "dragonageinquisition" => "Dragon Age: Inquisition",
        "bladeandsorcery" => "Blade & Sorcery",
        "bladeandsorcerynomad" => "Blade & Sorcery: Nomad",
        "acecombat7skiesunknown" => "Ace Combat 7",
        "kotor" => "Star Wars: KOTOR",
        "kotor2" => "Star Wars: KOTOR II",
        _ => "",
    };
    if known.is_empty() {
        prettify_slug(kind)
    } else {
        known.to_owned()
    }
}

/// Best-effort readable name from a slug: `cyberpunk2077` -> `Cyberpunk 2077`,
/// `some_game` -> `Some Game`.
fn prettify_slug(slug: &str) -> String {
    let mut out = String::new();
    let mut start_word = true;
    let mut prev_digit = false;
    for ch in slug.chars() {
        if ch == '_' || ch == '-' {
            out.push(' ');
            start_word = true;
            prev_digit = false;
            continue;
        }
        let is_digit = ch.is_ascii_digit();
        if is_digit && !prev_digit && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
            start_word = true;
        }
        if start_word && ch.is_ascii_alphabetic() {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
        start_word = false;
        prev_digit = is_digit;
    }
    out
}

/// Register the assembled adapter registry into core. Call once at process start
/// (idempotent). Afterwards `concierge::game::adapter_for` resolves every
/// migrated game through its owning crate.
pub fn register() {
    concierge::game::register_adapters(resolve, kinds);
}

/// The intended-game-set seed: Nexus's top games by mod count, each mapped to
/// its owning family/topology and a coverage status. Seeded from
/// `v1/games.json`; the coverage gate verifies it.
#[must_use]
pub const fn games_seed() -> &'static str {
    include_str!("../games.toml")
}

/// Every seed domain the registry can actually service — an adapter whose Nexus
/// domain OR kind matches. This is what "covered" must mean; used by the ledger
/// gate and the coverage report.
#[must_use]
pub fn covered_domains() -> std::collections::BTreeSet<&'static str> {
    let mut out = std::collections::BTreeSet::new();
    for k in kinds() {
        if let Some(a) = resolve(k) {
            out.insert(k);
            if let Some(d) = a.nexus_domain() {
                out.insert(d);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    #[test]
    fn display_names_disambiguate_and_prettify() {
        use super::display_name;
        // the two "skyrim" rows become distinct, readable names
        assert_eq!(display_name("skyrimse"), "Skyrim Special Edition");
        assert_eq!(display_name("skyrim"), "Skyrim (Legendary Edition)");
        assert_eq!(display_name("cyberpunk2077"), "Cyberpunk 2077");
        // unknown slug falls back to a prettified form (digit boundary + caps)
        assert_eq!(display_name("some_new_game"), "Some New Game");
        assert_eq!(display_name("coolgame3"), "Coolgame 3");
    }

    /// The gate: the seed parses, domains are unique, statuses are valid, and —
    /// crucially — every game the seed marks `covered` actually RESOLVES through
    /// the registry (matched by Nexus domain or kind). The seed cannot claim
    /// coverage the crate tree doesn't provide.
    #[test]
    fn every_covered_seed_game_actually_resolves() {
        let reachable = super::covered_domains();
        let v: toml::Value = toml::from_str(super::games_seed()).unwrap();
        let games = v.get("game").and_then(|g| g.as_array()).unwrap();
        let mut domains = std::collections::BTreeSet::new();
        let mut covered = 0usize;
        for g in games {
            let domain = g.get("domain").and_then(|d| d.as_str()).unwrap();
            assert!(
                domains.insert(domain.to_owned()),
                "duplicate domain {domain}"
            );
            assert!(
                g.get("family").and_then(|f| f.as_str()).is_some(),
                "{domain}: family"
            );
            let status = g.get("status").and_then(|s| s.as_str()).unwrap();
            assert!(
                ["covered", "planned", "new-family", "triage", "skip"].contains(&status),
                "{domain}: unknown status {status}"
            );
            if status == "covered" {
                covered += 1;
                assert!(
                    reachable.contains(domain),
                    "{domain}: marked covered but no adapter resolves it"
                );
            }
        }
        assert_eq!(
            covered,
            super::kinds().len(),
            "seed 'covered' count must equal registered adapter kinds"
        );
    }
}
