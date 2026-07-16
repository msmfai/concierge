//! Format-preserving edits to `manifest.toml` — the pure functions behind every
//! GUI mutation (toggle a mod, reorder the load order, add/remove a mod). Each
//! takes the manifest text and returns new text, editing via `toml_edit` so
//! comments, ordering, and formatting survive. The manifest stays the single
//! source of truth: the UI applies one of these, writes the file, and re-evals.
//!
//! These are deliberately UI-agnostic and unit-tested so the GUI holds no
//! business logic.

use std::path::Path;
use std::sync::Mutex;

use crate::error::{Error, IoCtx as _, Result};

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

/// Serializes all manifest writes across the process — the GUI's edits, the
/// off-thread LOOT-sort/resolve-deps/nxm-add workers, and CLI writes can all
/// target `manifest.toml`; without this they could clobber each other.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Write `content` to `path` **atomically** (temp file + rename) and **under a
/// process-wide lock**, so a concurrent writer can't lose an update and a crash
/// mid-write can't leave a half-written (corrupt) manifest. Use for every
/// `manifest.toml` write.
pub fn write_manifest(path: &Path, content: &str) -> Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    write_atomic(path, content)
}

/// The raw locked-check + temp-file + rename. Caller holds `WRITE_LOCK`.
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    // The Locked state is the manifest's read-only bit; an atomic
    // rename would silently bypass it, so every declaration write refuses
    // here — the one chokepoint all editors go through.
    if std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly()) {
        return Err(Error::Other(format!(
            "profile is LOCKED (manifest read-only): {} — unlock with `concierge unlock` (or the GUI lock toggle)",
            path.display()
        )));
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content).ctx(&tmp)?;
    std::fs::rename(&tmp, path).ctx(path)
}

/// Append a `[[mod]]` to the manifest FILE atomically: read → `add_mod` → write
/// with the process lock held across the whole read-modify-write. This is what
/// makes concurrent/rapid adds safe — two adds can't each read the same text
/// and have the second clobber the first (the "second add vanished" bug). A
/// duplicate name is rejected (the mod is already there), which is not a lost
/// update.
pub fn add_mod_to_file(path: &Path, entry: &NewMod) -> Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let doc = std::fs::read_to_string(path).ctx(path)?;
    let updated = add_mod(&doc, entry)?;
    write_atomic(path, &updated)
}

fn parse(doc: &str) -> Result<DocumentMut> {
    doc.parse::<DocumentMut>()
        .map_err(|e| Error::Manifest(format!("manifest is not valid TOML: {e}")))
}

fn mods_mut(d: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    d.get_mut("mod")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| Error::Manifest("manifest has no [[mod]] entries".to_owned()))
}

fn table_name(t: &Table) -> Option<&str> {
    t.get("name").and_then(Item::as_str)
}

fn index_of(mods: &ArrayOfTables, name: &str) -> Option<usize> {
    mods.iter()
        .position(|t| table_name(t).is_some_and(|n| n == name))
}

/// Toggle a mod's `enabled`. `true` removes the key (the manifest default is
/// enabled, keeping files clean); `false` writes `enabled = false`.
pub fn set_mod_enabled(doc: &str, name: &str, enabled: bool) -> Result<String> {
    let mut d = parse(doc)?;
    let mods = mods_mut(&mut d)?;
    let idx =
        index_of(mods, name).ok_or_else(|| Error::Manifest(format!("mod '{name}' not found")))?;
    let Some(t) = mods.get_mut(idx) else {
        return Err(Error::Manifest(format!("mod '{name}' not found")));
    };
    if enabled {
        t.remove("enabled");
    } else {
        t.insert("enabled", value(false));
    }
    Ok(d.to_string())
}

/// Write the resolved pin onto a mod: its content `md5` and — when known —
/// the archive `file`, Nexus `file_id`, and actual `version`. This is what
/// turns a just-downloaded mod from "md5 = \"\"" into a realizable entry
/// (the fetch pin write-back). Empty optionals are left untouched.
pub fn pin_mod(
    doc: &str,
    name: &str,
    md5: &str,
    version: Option<&str>,
    file: Option<&str>,
    file_id: Option<u64>,
) -> Result<String> {
    let mut d = parse(doc)?;
    let mods = mods_mut(&mut d)?;
    let idx =
        index_of(mods, name).ok_or_else(|| Error::Manifest(format!("mod '{name}' not found")))?;
    let Some(t) = mods.get_mut(idx) else {
        return Err(Error::Manifest(format!("mod '{name}' not found")));
    };
    t.insert("md5", value(md5));
    if let Some(v) = version.filter(|v| !v.is_empty()) {
        t.insert("version", value(v));
    }
    if let Some(f) = file.filter(|f| !f.is_empty()) {
        t.insert("file", value(f));
    }
    if let Some(fid) = file_id {
        t.insert("nexus_file_id", value(i64::try_from(fid).unwrap_or(0)));
    }
    Ok(d.to_string())
}

/// Write the resolved install layout onto a mod: the `subdir` (archive root to
/// strip) and the `plugins` it activates. Used once the archive is inspected —
/// e.g. JOURNEY's versioned root folder + `Journey.esp`. Passing `None`/empty
/// leaves that field as-is; an empty subdir string clears it.
pub fn set_layout(
    doc: &str,
    name: &str,
    subdir: Option<&str>,
    plugins: &[String],
) -> Result<String> {
    let mut d = parse(doc)?;
    let mods = mods_mut(&mut d)?;
    let idx =
        index_of(mods, name).ok_or_else(|| Error::Manifest(format!("mod '{name}' not found")))?;
    let Some(t) = mods.get_mut(idx) else {
        return Err(Error::Manifest(format!("mod '{name}' not found")));
    };
    if let Some(sub) = subdir {
        if sub.is_empty() {
            t.remove("subdir");
        } else {
            t.insert("subdir", value(sub));
        }
    }
    if !plugins.is_empty() {
        let mut arr = Array::new();
        for p in plugins {
            arr.push(p.as_str());
        }
        t.insert("plugins", value(arr));
    }
    Ok(d.to_string())
}

/// Move the mod at `from` to position `to` (load order == manifest order).
pub fn move_mod(doc: &str, from: usize, to: usize) -> Result<String> {
    let mut d = parse(doc)?;
    let mods = mods_mut(&mut d)?;
    let n = mods.len();
    if from >= n || to >= n {
        return Err(Error::Manifest(format!(
            "move out of range (from={from}, to={to}, len={n})"
        )));
    }
    if from == to {
        return Ok(d.to_string());
    }
    // toml_edit tables retain their original document `position`, so on
    // serialize they sort back regardless of push order. Capture the slots'
    // ascending positions, reorder the cloned tables, then reassign those
    // positions in the new order so rendering follows it.
    let positions: Vec<Option<usize>> = (0..n)
        .map(|i| mods.get(i).and_then(Table::position))
        .collect();
    let mut tables: Vec<Table> = (0..n).filter_map(|i| mods.get(i).cloned()).collect();
    let moved = tables.remove(from);
    tables.insert(to, moved);
    mods.clear();
    for (i, mut t) in tables.into_iter().enumerate() {
        if let Some(p) = positions.get(i).copied().flatten() {
            t.set_position(p);
        }
        mods.push(t);
    }
    Ok(d.to_string())
}

/// Move a named mod up (toward index 0) or down by one.
pub fn nudge_mod(doc: &str, name: &str, up: bool) -> Result<String> {
    let d = parse(doc)?;
    let mods = d
        .get("mod")
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| Error::Manifest("manifest has no [[mod]] entries".to_owned()))?;
    let idx =
        index_of(mods, name).ok_or_else(|| Error::Manifest(format!("mod '{name}' not found")))?;
    let to = if up {
        idx.checked_sub(1)
            .ok_or_else(|| Error::Manifest("already first".to_owned()))?
    } else {
        let t = idx + 1;
        if t >= mods.len() {
            return Err(Error::Manifest("already last".to_owned()));
        }
        t
    };
    move_mod(doc, idx, to)
}

/// Set the explicit `[relations].load_order` to `order` (e.g. from a LOOT sort).
/// Creates the `[relations]` table if absent; replaces any existing order.
pub fn set_load_order(doc: &str, order: &[String]) -> Result<String> {
    let mut d = parse(doc)?;
    if d.get("relations").is_none() {
        d.insert("relations", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let rel = d
        .get_mut("relations")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| Error::Manifest("[relations] is not a table".to_owned()))?;
    let mut arr = toml_edit::Array::new();
    for p in order {
        arr.push(p.as_str());
    }
    rel.insert("load_order", toml_edit::value(arr));
    Ok(d.to_string())
}

/// Append a `[[relations.requires]]` fact (`name` needs `needs`), idempotently —
/// a no-op if that exact fact already exists. Used by the dependency resolver to
/// record the inter-mod dependency graph derived from plugin masters.
pub fn add_requires(doc: &str, name: &str, needs: &str) -> Result<String> {
    let mut d = parse(doc)?;
    if d.get("relations").is_none() {
        d.insert("relations", Item::Table(Table::new()));
    }
    let rel = d
        .get_mut("relations")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| Error::Manifest("[relations] is not a table".to_owned()))?;
    if rel.get("requires").is_none() {
        rel.insert("requires", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let arr = rel
        .get_mut("requires")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| Error::Manifest("relations.requires is not array-of-tables".to_owned()))?;
    let exists = arr.iter().any(|t| {
        t.get("name").and_then(Item::as_str) == Some(name)
            && t.get("needs").and_then(Item::as_str) == Some(needs)
    });
    if !exists {
        let mut t = Table::new();
        t.insert("name", value(name));
        t.insert("needs", value(needs));
        arr.push(t);
    }
    Ok(d.to_string())
}

/// Produce a manifest that keeps everything from a shared pack (`shared`: mods,
/// relations, curate, compat) but takes the `[game]` block from `local` — so an
/// imported pack runs against THIS machine's game install, not the sharer's.
pub fn swap_game_block(shared: &str, local: &str) -> Result<String> {
    let mut out = parse(shared)?;
    let local_doc = parse(local)?;
    let game = local_doc
        .get("game")
        .cloned()
        .ok_or_else(|| Error::Manifest("local manifest has no [game] block".to_owned()))?;
    out.insert("game", game);
    Ok(out.to_string())
}

/// Set the game's `pristine` install path — the folder Concierge installs a
/// *copy* of, and the one thing a fresh profile is missing before it can deploy
/// ("not realized" until this points at a real install). An empty `path` clears
/// it. Format-preserving, like every edit here.
pub fn set_pristine(doc: &str, path: &str) -> Result<String> {
    let mut d = parse(doc)?;
    let game = d
        .get_mut("game")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| Error::Manifest("manifest has no [game] block".to_owned()))?;
    game.insert("pristine", value(path));
    Ok(d.to_string())
}

/// Remove a mod by name (from the manifest; store/instance cleanup is realize's).
pub fn remove_mod(doc: &str, name: &str) -> Result<String> {
    let mut d = parse(doc)?;
    let mods = mods_mut(&mut d)?;
    let idx =
        index_of(mods, name).ok_or_else(|| Error::Manifest(format!("mod '{name}' not found")))?;
    mods.remove(idx);
    Ok(d.to_string())
}

/// A mod source for [`add_mod`] — a direct URL or a Nexus mod/file id pair.
#[derive(Debug, Clone)]
pub enum NewSource {
    Url(String),
    Nexus { mod_id: u32, file_id: u32 },
}

/// The fields the GUI collects to add a `[[mod]]`.
#[derive(Debug, Clone)]
pub struct NewMod {
    pub name: String,
    pub version: String,
    pub source: NewSource,
    pub md5: String,
    pub file: String,
    /// `"data"` (default, omitted) or e.g. `"game"`.
    pub install_root: String,
    pub plugins: Vec<String>,
}

/// Append a `[[mod]]` built from `entry`. Duplicate names are rejected.
pub fn add_mod(doc: &str, entry: &NewMod) -> Result<String> {
    let mut d = parse(doc)?;
    if d.get("mod").is_none() {
        d.insert("mod", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let mods = mods_mut(&mut d)?;
    if index_of(mods, &entry.name).is_some() {
        return Err(Error::Manifest(format!(
            "a mod named '{}' already exists",
            entry.name
        )));
    }
    let mut t = Table::new();
    t.insert("name", value(entry.name.as_str()));
    t.insert("version", value(entry.version.as_str()));
    match &entry.source {
        NewSource::Url(u) => {
            t.insert("url", value(u.as_str()));
        }
        NewSource::Nexus { mod_id, file_id } => {
            t.insert("nexus_mod_id", value(i64::from(*mod_id)));
            t.insert("nexus_file_id", value(i64::from(*file_id)));
        }
    }
    t.insert("md5", value(entry.md5.as_str()));
    t.insert("file", value(entry.file.as_str()));
    if entry.install_root != "data" && !entry.install_root.is_empty() {
        t.insert("install_root", value(entry.install_root.as_str()));
    }
    if !entry.plugins.is_empty() {
        let mut arr = Array::new();
        for p in &entry.plugins {
            arr.push(p.as_str());
        }
        t.insert("plugins", value(arr));
    }
    mods.push(t);
    Ok(d.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const DOC: &str = "\
[game]
kind = \"fallout4\"

# keep this comment
[[mod]]
name = \"aaa\"
version = \"1\"

[[mod]]
name = \"bbb\"
version = \"2\"
enabled = false

[[mod]]
name = \"ccc\"
version = \"3\"
";

    fn names(doc: &str) -> Vec<String> {
        doc.parse::<DocumentMut>()
            .unwrap()
            .get("mod")
            .and_then(Item::as_array_of_tables)
            .unwrap()
            .iter()
            .filter_map(|t| table_name(t).map(str::to_owned))
            .collect()
    }

    #[test]
    fn set_pristine_writes_game_path_and_preserves_the_rest() {
        let out = set_pristine(DOC, "/home/me/games/Fallout 4").unwrap();
        // pristine landed in [game], parseable back
        let d = out.parse::<DocumentMut>().unwrap();
        assert_eq!(
            d["game"]["pristine"].as_str(),
            Some("/home/me/games/Fallout 4")
        );
        assert_eq!(d["game"]["kind"].as_str(), Some("fallout4")); // untouched
        assert!(out.contains("# keep this comment")); // comments survive
        assert_eq!(names(&out), vec!["aaa", "bbb", "ccc"]); // mods untouched
        // re-setting overwrites rather than duplicating
        let again = set_pristine(&out, "/other/path").unwrap();
        assert_eq!(
            again.parse::<DocumentMut>().unwrap()["game"]["pristine"].as_str(),
            Some("/other/path")
        );
    }

    #[test]
    fn toggle_enabled_round_trips_and_preserves_comment() {
        let off = set_mod_enabled(DOC, "aaa", false).unwrap();
        assert!(off.contains("keep this comment"), "comments preserved");
        assert!(off.contains("enabled = false"));
        let on = set_mod_enabled(&off, "aaa", true).unwrap();
        let doc = on.parse::<DocumentMut>().unwrap();
        let aaa = doc
            .get("mod")
            .unwrap()
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap();
        assert!(aaa.get("enabled").is_none(), "re-enable drops the key");
    }

    #[test]
    fn move_and_nudge_reorder() {
        assert_eq!(names(DOC), ["aaa", "bbb", "ccc"]);
        let moved = move_mod(DOC, 0, 2).unwrap();
        assert_eq!(
            names(&moved),
            ["bbb", "ccc", "aaa"],
            "got {:?}",
            names(&moved)
        );
        assert!(moved.contains("enabled = false"), "bbb keeps enabled=false");
        let up = nudge_mod(DOC, "ccc", true).unwrap();
        assert_eq!(names(&up), ["aaa", "ccc", "bbb"]);
        assert!(nudge_mod(DOC, "aaa", true).is_err(), "first can't go up");
    }

    #[test]
    fn set_load_order_writes_relations() {
        let out = set_load_order(DOC, &["B.esp".to_owned(), "A.esp".to_owned()]).unwrap();
        assert!(out.contains("[relations]"), "got {out}");
        assert!(
            out.contains("B.esp") && out.contains("A.esp"),
            "order written: {out}"
        );
        // re-setting replaces the order
        let out2 = set_load_order(&out, &["X.esp".to_owned()]).unwrap();
        assert!(
            out2.contains("X.esp") && !out2.contains("A.esp"),
            "replaced: {out2}"
        );
    }

    #[test]
    fn add_requires_appends_and_dedups() {
        let out = add_requires(DOC, "ModA", "Framework").unwrap();
        assert!(out.contains("[[relations.requires]]"), "got {out}");
        assert!(out.contains("ModA") && out.contains("Framework"));
        // idempotent: same fact doesn't duplicate
        let out2 = add_requires(&out, "ModA", "Framework").unwrap();
        assert_eq!(
            out.matches("Framework").count(),
            out2.matches("Framework").count()
        );
        // a different fact does append
        let out3 = add_requires(&out2, "ModB", "Other").unwrap();
        assert!(out3.contains("ModB") && out3.contains("Other"));
    }

    #[test]
    fn swap_game_block_keeps_local_paths() {
        let shared = "[game]\nkind = \"skyrimse\"\npristine = \"/sharer/path\"\n\n\
                      [[mod]]\nname = \"SkyUI\"\nversion = \"5\"\n";
        let local = "[game]\nkind = \"skyrimse\"\npristine = \"/my/local/path\"\n";
        let out = swap_game_block(shared, local).unwrap();
        assert!(
            out.contains("/my/local/path"),
            "local game path kept: {out}"
        );
        assert!(!out.contains("/sharer/path"), "sharer path dropped");
        assert!(out.contains("SkyUI"), "shared mods kept");
    }

    #[test]
    fn write_manifest_is_atomic_and_serialized() {
        let path = std::env::temp_dir().join(format!("cg-wm-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // 8 concurrent writers, each writing a complete valid doc. Under the
        // lock + atomic rename the final file is exactly ONE of them — never a
        // half-written or interleaved manifest.
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = path.clone();
                std::thread::spawn(move || {
                    super::write_manifest(&p, &format!("[game]\nkind = \"g{i}\"\n")).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let final_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            final_content.matches("kind = ").count(),
            1,
            "not interleaved: {final_content}"
        );
        assert!(final_content.parse::<DocumentMut>().is_ok(), "valid TOML");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_and_remove() {
        let added = add_mod(
            DOC,
            &NewMod {
                name: "ddd".to_owned(),
                version: "9".to_owned(),
                source: NewSource::Nexus {
                    mod_id: 42,
                    file_id: 7,
                },
                md5: String::new(),
                file: "ddd.7z".to_owned(),
                install_root: "data".to_owned(),
                plugins: vec!["ddd.esp".to_owned()],
            },
        )
        .unwrap();
        assert_eq!(names(&added), ["aaa", "bbb", "ccc", "ddd"]);
        assert!(added.contains("nexus_mod_id = 42"));
        assert!(added.contains("\"ddd.esp\""));
        assert!(!added.contains("install_root"), "default root omitted");
        let removed = remove_mod(&added, "bbb").unwrap();
        assert_eq!(names(&removed), ["aaa", "ccc", "ddd"]);
    }

    #[test]
    fn pin_mod_writes_md5_version_file_id() {
        let doc = "[[mod]]\nname = \"m\"\nversion = \"1\"\nnexus_mod_id = 12685\nmd5 = \"\"\n";
        let out = pin_mod(
            doc,
            "m",
            "786c5e40",
            Some("1.6.1"),
            Some("Journey.rar"),
            Some(57612),
        )
        .unwrap();
        assert!(out.contains("md5 = \"786c5e40\""));
        assert!(
            out.contains("version = \"1.6.1\""),
            "version updated from 1"
        );
        assert!(out.contains("file = \"Journey.rar\""));
        assert!(out.contains("nexus_file_id = 57612"));
        // Empty optionals leave fields untouched; only md5 changes.
        let only = pin_mod(&out, "m", "abc123", None, None, None).unwrap();
        assert!(only.contains("md5 = \"abc123\""));
        assert!(only.contains("version = \"1.6.1\""), "version kept");
        assert!(pin_mod(doc, "nope", "x", None, None, None).is_err());
    }

    #[test]
    fn concurrent_adds_all_land_none_dropped() {
        // The "second add vanished" regression: N threads each append a
        // distinct mod to the same file; with the locked read-modify-write all
        // N must survive (no lost updates).
        let dir = std::env::temp_dir().join(format!("cg-addrace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manifest.toml");
        std::fs::write(&path, "[game]\nkind = \"fallout4\"\n").unwrap();

        let n = 12;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let entry = NewMod {
                        name: format!("mod{i}"),
                        version: "1".to_owned(),
                        source: NewSource::Url(format!("https://e/{i}.7z")),
                        md5: String::new(),
                        file: format!("{i}.7z"),
                        install_root: "data".to_owned(),
                        plugins: Vec::new(),
                    };
                    add_mod_to_file(&path, &entry).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let got = names(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(got.len(), n, "all {n} adds present, got {got:?}");
        for i in 0..n {
            assert!(got.contains(&format!("mod{i}")), "mod{i} survived");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_layout_writes_subdir_and_plugins() {
        let doc = "[[mod]]\nname = \"j\"\nversion = \"1\"\n";
        let out = set_layout(
            doc,
            "j",
            Some("JOURNEY v1.6.1"),
            &["Journey.esp".to_owned()],
        )
        .unwrap();
        assert!(out.contains("subdir = \"JOURNEY v1.6.1\""));
        assert!(out.contains("plugins = [\"Journey.esp\"]"));
        // Empty subdir clears it; empty plugins leave the field as-is.
        let cleared = set_layout(&out, "j", Some(""), &[]).unwrap();
        assert!(!cleared.contains("subdir"), "empty subdir removes the key");
        assert!(
            cleared.contains("plugins = [\"Journey.esp\"]"),
            "plugins kept"
        );
    }
}
