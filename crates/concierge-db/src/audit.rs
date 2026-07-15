//! Manifest-id audit: verify each declared `nexus_mod_id` against the synced
//! catalog, so an invented or mistyped id is structurally loud instead of
//! silently trusted. No agent harness should be able to ship unverified ids
//! quietly.

use crate::catalog::Catalog;
use crate::error::Result;

/// The audit outcome for one manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The id exists and its catalog name agrees with the manifest name.
    Ok { catalog_name: String },
    /// The id exists but names it something else entirely — likely a guess.
    NameMismatch { catalog_name: String },
    /// No such mod id in the synced catalog.
    UnknownId,
}

/// One audited entry: the manifest's slug + id, and what the catalog says.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub name: String,
    pub mod_id: u64,
    pub verdict: Verdict,
}

/// Audit `entries` (manifest name, `nexus_mod_id`) against the catalog for
/// `domain`. Every entry gets a definitive verdict; interpretation (warn,
/// refuse, record) is the caller's.
pub fn audit(
    catalog: &Catalog,
    domain: &str,
    entries: &[(String, u64)],
) -> Result<Vec<AuditEntry>> {
    let ids: Vec<u64> = entries.iter().map(|(_, id)| *id).collect();
    let known = catalog.names(domain, &ids)?;
    Ok(entries
        .iter()
        .map(|(name, id)| {
            let verdict = known.iter().find(|(kid, _)| kid == id).map_or(
                Verdict::UnknownId,
                |(_, catalog_name)| {
                    if names_agree(name, catalog_name) {
                        Verdict::Ok {
                            catalog_name: catalog_name.clone(),
                        }
                    } else {
                        Verdict::NameMismatch {
                            catalog_name: catalog_name.clone(),
                        }
                    }
                },
            );
            AuditEntry {
                name: name.clone(),
                mod_id: *id,
                verdict,
            }
        })
        .collect())
}

/// Does a manifest slug plausibly name the catalog mod? Normalized
/// containment either way ("journey" ⊆ "Journey - Survival Mode Fast
/// Travel"), or an acronym match ("mcm" = "Mod Configuration Menu",
/// "f4se" = "Fallout 4 Script Extender"). Deliberately generous: the audit
/// exists to catch WRONG ids, not to police naming style.
#[must_use]
pub fn names_agree(slug: &str, catalog_name: &str) -> bool {
    let a = normalize(slug);
    let b = normalize(catalog_name);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.contains(&b) || b.contains(&a) {
        return true;
    }
    acronym(catalog_name) == a
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// First alphanumeric of each word: "Fallout 4 Script Extender" → "f4se".
fn acronym(s: &str) -> String {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::catalog::Row;

    #[test]
    fn name_agreement_heuristics() {
        for (slug, name) in [
            ("journey", "Journey - Survival Mode Fast Travel"),
            ("survival-options", "Survival Options"),
            ("buffout4-ng", "Buffout 4 NG"),
            ("campsite", "Campsite - Simple Wasteland Camping"),
            ("mcm", "Mod Configuration Menu"),
            ("f4se", "Fallout 4 Script Extender"),
            ("address-library", "Address Library for F4SE Plugins"),
            ("sim-settlements-2", "Sim Settlements 2"),
        ] {
            assert!(names_agree(slug, name), "{slug} should match {name}");
        }
        for (slug, name) in [
            ("journey", "CBBE Body Replacer"),
            ("salvage-beacons", "We Are The Minutemen"),
            ("", "Anything"),
        ] {
            assert!(!names_agree(slug, name), "{slug} must not match {name}");
        }
    }

    #[test]
    fn audit_gives_every_entry_a_definitive_verdict() {
        let dir = std::env::temp_dir().join(format!("concierge-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cat = Catalog::open(&dir.join("catalog.sqlite")).unwrap();
        cat.upsert(&[
            Row {
                game_domain: "fallout4".into(),
                mod_id: 100,
                name: "Journey - Survival Mode Fast Travel".into(),
                ..Row::default()
            },
            Row {
                game_domain: "fallout4".into(),
                mod_id: 200,
                name: "We Are The Minutemen".into(),
                ..Row::default()
            },
        ])
        .unwrap();
        let got = audit(
            &cat,
            "fallout4",
            &[
                ("journey".into(), 100),
                ("salvage-beacons".into(), 200),
                ("campsite".into(), 999),
            ],
        )
        .unwrap();
        assert!(matches!(got[0].verdict, Verdict::Ok { .. }), "{:?}", got[0]);
        assert!(
            matches!(got[1].verdict, Verdict::NameMismatch { .. }),
            "{:?}",
            got[1]
        );
        assert_eq!(got[2].verdict, Verdict::UnknownId, "{:?}", got[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
