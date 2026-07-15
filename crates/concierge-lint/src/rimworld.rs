//! `RimWorld` `About/About.xml` invariants. Every mod is a folder under `Mods/`
//! with `About/About.xml` (root `<ModMetaData>`).
//!
//! Encoded (Error): `<packageId>` present; `<packageId>` unique (case-insensitive
//! — the engine lowercases ids); every `<modDependencies>` entry resolves to an
//! installed packageId; no `<incompatibleWith>` pair both active. `<loadAfter>`/
//! `<loadBefore>` ordering and `<supportedVersions>` (needs the game version) are
//! documented gaps.
//!
//! Source: <https://rimworldmodding.wiki.gg/wiki/About_File>,
//! <https://rimworldwiki.com/wiki/Modding_Tutorials/About.xml>.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;

use crate::Violation;

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let mods = concierge::realize::target_root(plan, "mods")?;
    Ok(check_mods_dir(&mods))
}

struct ModAbout {
    folder: String,
    package_id: Option<String>,
    dependencies: Vec<String>,
    incompatible: Vec<String>,
}

fn check_mods_dir(mods: &Path) -> Vec<Violation> {
    let about = read_all(mods);
    let mut out = Vec::new();

    let mut ids: HashMap<String, usize> = HashMap::new();
    for m in &about {
        match &m.package_id {
            Some(id) => *ids.entry(id.to_lowercase()).or_default() += 1,
            None => out.push(Violation::error(
                m.folder.clone(),
                "missing-packageid",
                "About.xml has no <packageId> — RimWorld can't load or resolve this mod",
            )),
        }
    }
    for (id, n) in &ids {
        if *n > 1 {
            out.push(Violation::error(
                id.clone(),
                "duplicate-packageid",
                format!("{n} active mods share packageId `{id}` — only one loads, ordering breaks"),
            ));
        }
    }
    let active: HashSet<String> = ids.keys().cloned().collect();
    for m in &about {
        for dep in &m.dependencies {
            if !active.contains(&dep.to_lowercase()) {
                out.push(Violation::error(
                    m.folder.clone(),
                    "missing-dependency",
                    format!("requires `{dep}` which isn't active — RimWorld errors on load"),
                ));
            }
        }
        for inc in &m.incompatible {
            if active.contains(&inc.to_lowercase()) {
                out.push(Violation::error(
                    m.folder.clone(),
                    "incompatible-mod",
                    format!("declared incompatible with active mod `{inc}` — crash/corrupt saves"),
                ));
            }
        }
    }
    out
}

fn read_all(mods: &Path) -> Vec<ModAbout> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(mods) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(xml) = find_about(&dir).and_then(|p| std::fs::read_to_string(p).ok()) else {
            continue;
        };
        let folder = dir
            .file_name()
            .map_or_else(|| "<mod>".to_owned(), |n| n.to_string_lossy().into_owned());
        out.push(parse_about(&folder, &xml));
    }
    out
}

/// `About/About.xml`, tolerating case differences in the `About` dir name.
fn find_about(dir: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        if e.path().is_dir() && e.file_name().eq_ignore_ascii_case("About") {
            let x = e.path().join("About.xml");
            if x.exists() {
                return Some(x);
            }
        }
    }
    None
}

fn parse_about(folder: &str, xml: &str) -> ModAbout {
    let mut about = ModAbout {
        folder: folder.to_owned(),
        package_id: None,
        dependencies: Vec::new(),
        incompatible: Vec::new(),
    };
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return about;
    };
    let root = doc.root_element();
    for child in root.children().filter(roxmltree::Node::is_element) {
        match child.tag_name().name() {
            "packageId" => {
                about.package_id = child
                    .text()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
            "modDependencies" | "modDependenciesByVersion" => {
                for li in child.descendants().filter(|n| n.has_tag_name("li")) {
                    if let Some(pid) = child_text(li, "packageId") {
                        about.dependencies.push(pid);
                    }
                }
            }
            "incompatibleWith" => {
                for li in child.children().filter(|n| n.has_tag_name("li")) {
                    if let Some(t) = li.text().map(str::trim).filter(|s| !s.is_empty()) {
                        about.incompatible.push(t.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    about
}

fn child_text(node: roxmltree::Node, name: &str) -> Option<String> {
    node.children()
        .find(|c| c.has_tag_name(name))
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn write_mod(root: &Path, folder: &str, about_xml: &str) {
        let d = root.join(folder).join("About");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("About.xml"), about_xml).unwrap();
    }

    #[test]
    fn deps_and_incompat_and_dupes() {
        let root = std::env::temp_dir().join(format!("cc-rw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_mod(
            &root,
            "Harmony",
            "<ModMetaData><packageId>brrainz.harmony</packageId></ModMetaData>",
        );
        write_mod(&root, "Good", "<ModMetaData><packageId>me.good</packageId><modDependencies><li><packageId>brrainz.harmony</packageId></li></modDependencies></ModMetaData>");
        let v = check_mods_dir(&root);
        assert!(v.is_empty(), "clean set, got {v:?}");

        write_mod(&root, "NeedsMissing", "<ModMetaData><packageId>me.needs</packageId><modDependencies><li><packageId>nobody.absent</packageId></li></modDependencies></ModMetaData>");
        write_mod(&root, "NoId", "<ModMetaData><name>x</name></ModMetaData>");
        write_mod(&root, "Conflicts", "<ModMetaData><packageId>me.conf</packageId><incompatibleWith><li>me.good</li></incompatibleWith></ModMetaData>");
        let v = check_mods_dir(&root);
        assert!(v.iter().any(|x| x.rule == "missing-dependency"));
        assert!(v.iter().any(|x| x.rule == "missing-packageid"));
        assert!(v.iter().any(|x| x.rule == "incompatible-mod"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
