//! Minecraft (Forge / `NeoForge` / Fabric / Quilt) mod-jar invariants. Each mod
//! is a `.jar` under `mods/`; its metadata is a descriptor inside the jar:
//! Forge `META-INF/mods.toml` (or `META-INF/neoforge.mods.toml`), Fabric
//! `fabric.mod.json`, Quilt `quilt.mod.json`.
//!
//! Encoded (Error): a jar must carry a recognized loader descriptor; each mod
//! id is unique across the load set; every mandatory dependency resolves to a
//! loaded mod id (platform ids — minecraft/forge/fabricloader/java/… — are
//! treated as provided). Version-range satisfaction and loader-vs-instance
//! matching are documented gaps (need the instance's loader + MC version).
//!
//! Source: <https://docs.minecraftforge.net/en/latest/gettingstarted/modfiles/>,
//! <https://wiki.fabricmc.net/documentation:fabric_mod_json_spec>,
//! <https://github.com/QuiltMC/rfcs/blob/main/specification/0002-quilt.mod.json.md>.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;

use crate::Violation;

/// Platform/loader ids satisfied by the instance itself, not by another jar.
const PROVIDED: &[&str] = &[
    "minecraft",
    "forge",
    "neoforge",
    "fabric",
    "fabricloader",
    "fabric-loader",
    "java",
    "quilt_loader",
    "quiltloader",
    "quilt_base",
];

struct ModJar {
    file: String,
    ids: Vec<String>,
    mandatory_deps: Vec<String>,
    recognized: bool,
}

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let mods = concierge::realize::target_root(plan, "mods")?;
    Ok(check_mods_dir(&mods))
}

fn check_mods_dir(mods: &Path) -> Vec<Violation> {
    let jars = read_all(mods);
    let mut out = Vec::new();

    let mut id_count: HashMap<String, usize> = HashMap::new();
    for j in &jars {
        if !j.recognized {
            out.push(Violation::error(
                j.file.clone(),
                "no-loader-descriptor",
                "jar has no mods.toml / fabric.mod.json / quilt.mod.json — wrong loader or not a mod",
            ));
        }
        for id in &j.ids {
            *id_count.entry(id.to_lowercase()).or_default() += 1;
        }
    }
    for (id, n) in &id_count {
        if *n > 1 {
            out.push(Violation::error(
                id.clone(),
                "duplicate-modid",
                format!("{n} jars declare mod id `{id}` — Forge hard-crashes; Fabric/Quilt refuse to load"),
            ));
        }
    }
    for j in &jars {
        for dep in &j.mandatory_deps {
            let d = dep.to_lowercase();
            if !PROVIDED.contains(&d.as_str()) && !id_count.contains_key(&d) {
                out.push(Violation::error(
                    j.file.clone(),
                    "missing-dependency",
                    format!(
                        "mandatory dependency `{dep}` is not installed — loader errors at startup"
                    ),
                ));
            }
        }
    }
    out
}

fn read_all(mods: &Path) -> Vec<ModJar> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(mods) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
        {
            let file = path
                .file_name()
                .map_or_else(|| "<jar>".to_owned(), |n| n.to_string_lossy().into_owned());
            out.push(read_jar(&path, file));
        }
    }
    out
}

fn read_jar(path: &Path, file: String) -> ModJar {
    let mut jar = ModJar {
        file,
        ids: Vec::new(),
        mandatory_deps: Vec::new(),
        recognized: false,
    };
    let Some(mut archive) = std::fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
    else {
        return jar;
    };
    if let Some(text) = entry_text(&mut archive, "fabric.mod.json") {
        jar.recognized = true;
        parse_fabric(&text, &mut jar);
    } else if let Some(text) = entry_text(&mut archive, "quilt.mod.json") {
        jar.recognized = true;
        parse_quilt(&text, &mut jar);
    } else if let Some(text) = entry_text(&mut archive, "META-INF/mods.toml")
        .or_else(|| entry_text(&mut archive, "META-INF/neoforge.mods.toml"))
    {
        jar.recognized = true;
        parse_forge(&text, &mut jar);
    }
    jar
}

fn entry_text<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut f = archive.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn parse_forge(text: &str, jar: &mut ModJar) {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return;
    };
    if let Some(mods) = doc.get("mods").and_then(toml::Value::as_array) {
        for m in mods {
            if let Some(id) = m.get("modId").and_then(toml::Value::as_str) {
                jar.ids.push(id.to_owned());
            }
        }
    }
    // dependencies: table keyed by modid -> array of dependency tables
    if let Some(deps) = doc.get("dependencies").and_then(toml::Value::as_table) {
        for arr in deps.values().filter_map(toml::Value::as_array) {
            for dep in arr {
                let mandatory = dep
                    .get("mandatory")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true);
                if let Some(id) = dep.get("modId").and_then(toml::Value::as_str) {
                    if mandatory {
                        jar.mandatory_deps.push(id.to_owned());
                    }
                }
            }
        }
    }
}

fn parse_fabric(text: &str, jar: &mut ModJar) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if let Some(id) = v.get("id").and_then(serde_json::Value::as_str) {
        jar.ids.push(id.to_owned());
    }
    if let Some(provides) = v.get("provides").and_then(serde_json::Value::as_array) {
        for p in provides.iter().filter_map(serde_json::Value::as_str) {
            jar.ids.push(p.to_owned());
        }
    }
    if let Some(depends) = v.get("depends").and_then(serde_json::Value::as_object) {
        for key in depends.keys() {
            jar.mandatory_deps.push(key.clone());
        }
    }
}

fn parse_quilt(text: &str, jar: &mut ModJar) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(loader) = v.get("quilt_loader") else {
        return;
    };
    if let Some(id) = loader.get("id").and_then(serde_json::Value::as_str) {
        jar.ids.push(id.to_owned());
    }
    if let Some(depends) = loader.get("depends").and_then(serde_json::Value::as_array) {
        for dep in depends {
            match dep {
                serde_json::Value::String(s) => jar.mandatory_deps.push(s.clone()),
                serde_json::Value::Object(o) => {
                    // an entry with "optional": true is not mandatory
                    let optional = o
                        .get("optional")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    if !optional {
                        if let Some(id) = o.get("id").and_then(serde_json::Value::as_str) {
                            jar.mandatory_deps.push(id.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn jar_with(dir: &Path, name: &str, entry: &str, body: &str) {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zw.start_file(entry, opts).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
        zw.finish().unwrap();
    }

    #[test]
    fn fabric_missing_dep_and_duplicate_id() {
        let root = std::env::temp_dir().join(format!("cc-mc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A depends on minecraft (provided) + libB (present) -> ok
        jar_with(
            &root,
            "a.jar",
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"mod_a","version":"1","depends":{"minecraft":"*","libb":"*"}}"#,
        );
        jar_with(
            &root,
            "b.jar",
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"libb","version":"1"}"#,
        );
        assert!(check_mods_dir(&root).is_empty(), "clean set");

        // C needs an absent mod; D duplicates mod_a; E is not a mod jar
        jar_with(
            &root,
            "c.jar",
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"mod_c","version":"1","depends":{"absentmod":"*"}}"#,
        );
        jar_with(
            &root,
            "d.jar",
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"mod_a","version":"1"}"#,
        );
        jar_with(&root, "e.jar", "not-a-descriptor.txt", "hi");
        let v = check_mods_dir(&root);
        assert!(v.iter().any(|x| x.rule == "missing-dependency"));
        assert!(v.iter().any(|x| x.rule == "duplicate-modid"));
        assert!(v.iter().any(|x| x.rule == "no-loader-descriptor"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn forge_mods_toml_parses() {
        let root = std::env::temp_dir().join(format!("cc-mcf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let toml = "modLoader=\"javafml\"\nloaderVersion=\"[47,)\"\n[[mods]]\nmodId=\"examplemod\"\n[[dependencies.examplemod]]\nmodId=\"minecraft\"\nmandatory=true\nversionRange=\"[1.20,)\"\n";
        jar_with(&root, "f.jar", "META-INF/mods.toml", toml);
        assert!(
            check_mods_dir(&root).is_empty(),
            "minecraft dep is provided"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
