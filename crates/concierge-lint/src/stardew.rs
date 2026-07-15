//! Stardew Valley (SMAPI) `manifest.json` invariants — replicating SMAPI's own
//! pre-load validation. Every mod is a folder under `Mods/` with a
//! `manifest.json` (SMAPI scans recursively).
//!
//! Encoded (Error): `UniqueID`/`Name`/`Version` present; `EntryDll` XOR
//! `ContentPackFor`; `UniqueID` globally unique; required `Dependencies` and
//! the `ContentPackFor` target resolve to an installed `UniqueID`.
//!
//! Source: <https://stardewvalleywiki.com/Modding:Modder_Guide/APIs/Manifest>,
//! <https://smapi.io/json>.

use std::collections::HashMap;
use std::path::Path;

use concierge::error::Result;
use concierge::plan::Plan;
use serde_json::Value;

use crate::Violation;

pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    let mods = concierge::realize::target_root(plan, "mods")?;
    Ok(check_mods_dir(&mods))
}

struct Manifest {
    subject: String,
    unique_id: Option<String>,
    json: Value,
}

fn check_mods_dir(mods: &Path) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut manifests = Vec::new();
    collect_manifests(mods, mods, &mut manifests);

    // per-manifest structural checks
    for m in &manifests {
        out.extend(check_manifest(m));
    }

    // uniqueness + dependency resolution across the set
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for m in &manifests {
        if let Some(id) = &m.unique_id {
            *by_id.entry(id.to_lowercase()).or_default() += 1;
        }
    }
    for (id, n) in &by_id {
        if *n > 1 {
            out.push(Violation::error(
                id.clone(),
                "duplicate-uniqueid",
                format!("{n} installed mods share UniqueID `{id}` — SMAPI skips all copies"),
            ));
        }
    }
    for m in &manifests {
        out.extend(check_dependencies(m, &by_id));
    }
    out
}

fn collect_manifests(root: &Path, dir: &Path, out: &mut Vec<Manifest>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_manifests(root, &path, out);
        } else if path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("manifest.json"))
        {
            let subject = dir
                .strip_prefix(root)
                .unwrap_or(dir)
                .to_string_lossy()
                .into_owned();
            let subject = if subject.is_empty() {
                "<mod>".to_owned()
            } else {
                subject
            };
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| parse_lenient(&s))
            {
                Some(json) => {
                    let unique_id = get_ci(&json, "UniqueID")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                    out.push(Manifest {
                        subject,
                        unique_id,
                        json,
                    });
                }
                None => out.push(Manifest {
                    subject,
                    unique_id: None,
                    json: Value::Null,
                }),
            }
        }
    }
}

/// SMAPI tolerates JSON5-ish input; we try strict JSON, then a minimal cleanup
/// (strip `//`/`/* */` comments and trailing commas) before giving up.
fn parse_lenient(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(s) {
        return Some(v);
    }
    let mut cleaned = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            cleaned.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    cleaned.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cleaned.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        cleaned.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => cleaned.push(c),
        }
    }
    // strip trailing commas before } or ]
    let no_trailing = cleaned
        .replace(",\n}", "\n}")
        .replace(",}", "}")
        .replace(",\n]", "\n]")
        .replace(",]", "]");
    serde_json::from_str(&no_trailing).ok()
}

fn get_ci<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, val)| val)
}

fn check_manifest(m: &Manifest) -> Vec<Violation> {
    let mut out = Vec::new();
    if m.json.is_null() {
        out.push(Violation::warn(
            m.subject.clone(),
            "unparseable-manifest",
            "manifest.json could not be parsed — SMAPI would skip this mod",
        ));
        return out;
    }
    for field in ["UniqueID", "Name", "Version"] {
        if get_ci(&m.json, field).and_then(json_nonempty).is_none() {
            out.push(Violation::error(
                m.subject.clone(),
                "manifest-field",
                format!("manifest.json missing required `{field}` — SMAPI skips the mod"),
            ));
        }
    }
    let has_entry = get_ci(&m.json, "EntryDll")
        .and_then(Value::as_str)
        .is_some();
    let has_pack = get_ci(&m.json, "ContentPackFor").is_some();
    if has_entry == has_pack {
        out.push(Violation::error(
            m.subject.clone(),
            "entry-xor-contentpack",
            "manifest.json must have exactly one of `EntryDll` or `ContentPackFor`",
        ));
    }
    out
}

fn check_dependencies(m: &Manifest, by_id: &HashMap<String, usize>) -> Vec<Violation> {
    let mut out = Vec::new();
    let installed = |id: &str| by_id.contains_key(&id.to_lowercase());
    // ContentPackFor target is a required dependency on the host framework
    if let Some(id) = get_ci(&m.json, "ContentPackFor")
        .and_then(|v| get_ci(v, "UniqueID"))
        .and_then(Value::as_str)
    {
        if !installed(id) {
            out.push(Violation::error(
                m.subject.clone(),
                "missing-dependency",
                format!("ContentPackFor host `{id}` is not installed — SMAPI skips this pack"),
            ));
        }
    }
    if let Some(deps) = get_ci(&m.json, "Dependencies").and_then(Value::as_array) {
        for dep in deps {
            let required = get_ci(dep, "IsRequired")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let Some(id) = get_ci(dep, "UniqueID").and_then(Value::as_str) else {
                continue;
            };
            if required && !installed(id) {
                out.push(Violation::error(
                    m.subject.clone(),
                    "missing-dependency",
                    format!("required dependency `{id}` is not installed — SMAPI skips this mod"),
                ));
            }
        }
    }
    out
}

fn json_nonempty(v: &Value) -> Option<&Value> {
    match v {
        Value::String(s) if s.trim().is_empty() => None,
        Value::Null => None,
        other => Some(other),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("manifest.json"), body).unwrap();
    }

    #[test]
    fn clean_set_passes_and_violations_fire() {
        let root = std::env::temp_dir().join(format!("cc-sdv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // a content pack whose host IS installed -> clean
        write(
            &root,
            "CP",
            r#"{"Name":"CP","Version":"1.0","UniqueID":"me.cp","ContentPackFor":{"UniqueID":"Pathoschild.ContentPatcher"}}"#,
        );
        write(
            &root,
            "Host",
            r#"{"Name":"CPatcher","Version":"2.0","UniqueID":"Pathoschild.ContentPatcher","EntryDll":"ContentPatcher.dll"}"#,
        );
        let v = check_mods_dir(&root);
        assert!(v.is_empty(), "clean set has no violations, got {v:?}");

        // break it: missing dependency + missing UniqueID + dup + both entry&pack
        write(
            &root,
            "Bad",
            r#"{"Name":"Bad","Version":"1.0","Dependencies":[{"UniqueID":"nobody.here"}]}"#,
        );
        write(
            &root,
            "DupA",
            r#"{"Name":"A","Version":"1.0","UniqueID":"dup.id","EntryDll":"A.dll"}"#,
        );
        write(
            &root,
            "DupB",
            r#"{"Name":"B","Version":"1.0","UniqueID":"dup.id","EntryDll":"B.dll"}"#,
        );
        let v = check_mods_dir(&root);
        assert!(v.iter().any(|x| x.rule == "missing-dependency"));
        assert!(v.iter().any(|x| x.rule == "manifest-field")); // Bad has no UniqueID
        assert!(v.iter().any(|x| x.rule == "entry-xor-contentpack")); // Bad has neither
        assert!(v.iter().any(|x| x.rule == "duplicate-uniqueid"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tolerates_json5_comments_and_trailing_commas() {
        let j = "{\n// a comment\n\"Name\":\"X\",\n\"Version\":\"1.0\",\n\"UniqueID\":\"a.b\",\n\"EntryDll\":\"X.dll\",\n}";
        assert!(parse_lenient(j).is_some());
    }
}
