//! Install a `TSLPatcher` mod's 2DA edits natively: read its
//! `tslpatchdata/changes.ini`, load each target 2DA (from `tslpatchdata`, which
//! ships the base tables), apply the `[2DAList]` modifiers, and write the
//! merged tables to the game's `override/`. This is the KOTOR half of Rung 2
//! (Reconcile) run during install — no `HoloPatcher`/`TSLPatcher` needed.
//!
//! Directive lists we don't handle yet (GFF/TLK/Compile/HACK/SSF) are reported,
//! not silently skipped, so an incomplete install is visible.

use std::path::{Path, PathBuf};

use crate::changes::{apply_2da_list_seeded, Ini, Memory};
use crate::twoda::TwoDa;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no tslpatchdata/changes.ini under {0}")]
    NoChangesIni(PathBuf),
    #[error("[TLKList] present but no dialog.tlk supplied to relocate StrRefs")]
    NoDialogTlk,
    #[error(transparent)]
    Tlk(#[from] crate::tlk::Error),
    #[error("io {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Changes(#[from] crate::changes::Error),
    #[error(transparent)]
    TwoDa(#[from] crate::twoda::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default)]
pub struct InstallReport {
    /// (table filename, merged row count) written to override.
    pub tables: Vec<(String, usize)>,
    /// directive lists present but not applied (GFF/Compile/…).
    pub unsupported: Vec<String>,
    /// number of 2DAMEMORY tokens captured during apply.
    pub tokens: usize,
    /// number of TLK strings appended to `dialog.tlk` (0 if no `[TLKList]`).
    pub tlk_appended: usize,
    /// number of `StrRef` tokens relocated from the TLK append.
    pub strrefs: usize,
}

/// Read a mod's `[TLKList]` and resolve its `StrRef<n>` tokens against the game
/// `dialog.tlk`'s current size (each token = `base_count + append_index`), and
/// produce the merged `dialog.tlk`. Returns (seeded memory, merged tlk bytes,
/// appended count). No-op memory if the mod has no `[TLKList]`.
fn process_tlk(
    ini: &Ini,
    tslpatchdata: &Path,
    dialog_tlk: Option<&Path>,
) -> Result<(Memory, Option<Vec<u8>>, usize)> {
    let Some(list) = ini.section("TLKList") else {
        return Ok((Memory::default(), None, 0));
    };
    let Some(dialog) = dialog_tlk else {
        // can't relocate without the base tlk; surface rather than guess
        return Err(Error::NoDialogTlk);
    };
    let base = std::fs::read(dialog).map_err(|source| Error::Io {
        path: dialog.to_path_buf(),
        source,
    })?;
    let base_count = crate::tlk::string_count(&base)?;
    let mut mem = Memory::default();
    for (token, append_idx) in list {
        // token like "StrRef4", value "4" (index into append.tlk)
        if let Some(n) = token
            .strip_prefix("StrRef")
            .and_then(|s| s.parse::<u32>().ok())
        {
            let idx: u32 = append_idx.parse().unwrap_or(0);
            mem.strref.insert(n, (base_count + idx).to_string());
        }
    }
    let append_path = tslpatchdata.join("append.tlk");
    let merged = if append_path.is_file() {
        let add = std::fs::read(&append_path).map_err(|source| Error::Io {
            path: append_path,
            source,
        })?;
        Some(crate::tlk::append(&base, &add)?)
    } else {
        None
    };
    Ok((mem, merged, list.len()))
}

/// Locate a mod's `tslpatchdata` dir (it may be at the build root or one level
/// down) and return the path to it if it holds a `changes.ini`.
pub fn find_tslpatchdata(build_dir: &Path) -> Option<PathBuf> {
    let direct = build_dir.join("tslpatchdata");
    if direct.join("changes.ini").is_file() {
        return Some(direct);
    }
    // one level down (some archives nest the mod folder)
    let entries = std::fs::read_dir(build_dir).ok()?;
    for e in entries.flatten() {
        let cand = e.path().join("tslpatchdata");
        if cand.join("changes.ini").is_file() {
            return Some(cand);
        }
    }
    None
}

/// Apply a mod's 2DA edits (and TLK append) from `tslpatchdata` into
/// `override_dir`. `dialog_tlk` is the game `dialog.tlk`, needed only when the
/// mod has a `[TLKList]` (to relocate `StrRef`s); the merged `dialog.tlk` is
/// written to `override_dir`.
pub fn install(
    tslpatchdata: &Path,
    override_dir: &Path,
    dialog_tlk: Option<&Path>,
) -> Result<InstallReport> {
    let changes_path = tslpatchdata.join("changes.ini");
    if !changes_path.is_file() {
        return Err(Error::NoChangesIni(tslpatchdata.to_path_buf()));
    }
    let text = read(&changes_path)?;
    let ini = Ini::parse(&text)?;

    // resolve StrRef tokens from the TLK append first, then apply the 2DAs
    let (seed, merged_tlk, appended) = process_tlk(&ini, tslpatchdata, dialog_tlk)?;
    let strrefs = seed.strref.len();

    let applied = apply_2da_list_seeded(&ini, seed, |name| {
        let path = tslpatchdata.join(name);
        let bytes = std::fs::read(&path)
            .map_err(|e| crate::changes::Error::Apply(format!("read {}: {e}", path.display())))?;
        TwoDa::parse(&bytes).map_err(crate::changes::Error::from)
    })?;

    std::fs::create_dir_all(override_dir).map_err(|source| Error::Io {
        path: override_dir.to_path_buf(),
        source,
    })?;
    let mut report = InstallReport {
        unsupported: applied.unsupported,
        tokens: applied.memory.twoda.len(),
        tlk_appended: appended,
        strrefs,
        ..InstallReport::default()
    };
    for (name, table) in &applied.tables {
        let out = override_dir.join(name);
        write(&out, &table.serialize())?;
        report.tables.push((name.clone(), table.rows()));
    }
    if let Some(tlk) = merged_tlk {
        write(&override_dir.join("dialog.tlk"), &tlk)?;
    }
    Ok(report)
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
