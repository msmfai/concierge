//! `TSLPatcher` `changes.ini` engine (the common 2DA subset), native.
//!
//! Supported for `[2DAList]` tables: `ChangeRow*` (target by `RowIndex` /
//! `RowLabel` / `LabelIndex`), `AddRow*` and `CopyRow*` (with `ExclusiveColumn`
//! dedup and `RowLabel`/`NewRowLabel`), cell values including `high()` (next
//! unused int in a column) and `2DAMEMORY<n>` / `StrRef<n>` token references,
//! and `2DAMEMORY<n>=RowIndex|RowLabel|<column>` captures. `[TLKList]`
//! (`StrRef` relocation + `dialog.tlk` append) is handled by the `install`
//! layer. This is the install-time merge model that lets KOTOR mods COEXIST
//! (append + match, not overwrite) — how we subsume `HoloPatcher`.
//!
//! Not yet handled, reported (never silently skipped): `AddColumn*`,
//! `[GFFList]`, `[CompileList]`, `[HACKList]`, `[SSFList]`.

use std::collections::BTreeMap;

use crate::twoda::TwoDa;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("changes.ini: {0}")]
    Ini(String),
    #[error("unsupported directive: {0} (would silently drop mod content)")]
    Unsupported(String),
    #[error("2da: {0}")]
    TwoDa(#[from] crate::twoda::Error),
    #[error("apply: {0}")]
    Apply(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Token memory: `2DAMEMORY<n>` and `StrRef<n>` values captured during apply.
#[derive(Debug, Default)]
pub struct Memory {
    pub twoda: BTreeMap<u32, String>,
    pub strref: BTreeMap<u32, String>,
}

/// A parsed INI: ordered sections of ordered key/value pairs.
#[derive(Debug, Default)]
pub struct Ini {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(text: &str) -> Result<Self> {
        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                sections.push((name.trim().to_owned(), Vec::new()));
            } else if let Some((k, v)) = line.split_once('=') {
                let section = sections
                    .last_mut()
                    .ok_or_else(|| Error::Ini(format!("key before any section: {line}")))?;
                section.1.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        Ok(Self { sections })
    }

    pub fn section(&self, name: &str) -> Option<&[(String, String)]> {
        self.sections
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, kv)| kv.as_slice())
    }
}

/// The 2DA merge result: modified tables, captured token memory, and the names
/// of any top-level directive lists we do NOT support (surfaced loudly so the
/// caller can report an incomplete install — never silently skipped).
#[derive(Debug)]
pub struct Applied {
    pub tables: BTreeMap<String, TwoDa>,
    pub memory: Memory,
    pub unsupported: Vec<String>,
}

/// Apply every `[2DAList]` table's modifiers found in `ini` to the matching
/// table supplied by `load` (filename -> parsed 2DA). The 2DA list is applied;
/// any other directive list (GFF/TLK/Compile/HACK/SSF) is returned in
/// `unsupported` for the caller to report — we apply what we can and never
/// silently drop the rest.
pub fn apply_2da_list(ini: &Ini, load: impl FnMut(&str) -> Result<TwoDa>) -> Result<Applied> {
    apply_2da_list_seeded(ini, Memory::default(), load)
}

/// As [`apply_2da_list`], but with a pre-seeded token memory — used to inject
/// `StrRef` tokens resolved from a TLK append before the 2DA cells that
/// reference them are evaluated.
pub fn apply_2da_list_seeded(
    ini: &Ini,
    initial: Memory,
    mut load: impl FnMut(&str) -> Result<TwoDa>,
) -> Result<Applied> {
    // [TLKList] is handled by the install layer (TLK append + StrRef); the rest
    // are not yet, so surface them.
    let unsupported: Vec<String> = ini
        .sections
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "gfflist" | "compilelist" | "hacklist" | "ssflist"
            )
        })
        .map(|(name, _)| format!("[{name}]"))
        .collect();

    let mut mem = initial;
    let mut out = BTreeMap::new();
    let Some(list) = ini.section("2DAList") else {
        return Ok(Applied {
            tables: out,
            memory: mem,
            unsupported,
        });
    };
    for (_, table_file) in list {
        let modifiers = ini
            .section(table_file)
            .ok_or_else(|| Error::Ini(format!("[{table_file}] section missing")))?;
        let mut table = load(table_file)?;
        apply_table(&mut table, modifiers, ini, &mut mem)?;
        out.insert(table_file.clone(), table);
    }
    Ok(Applied {
        tables: out,
        memory: mem,
        unsupported,
    })
}

fn apply_table(
    table: &mut TwoDa,
    modifiers: &[(String, String)],
    ini: &Ini,
    mem: &mut Memory,
) -> Result<()> {
    for (key, section_name) in modifiers {
        let lk = key.to_ascii_lowercase();
        if lk.starts_with("changerow") {
            let m = ini
                .section(section_name)
                .ok_or_else(|| Error::Ini(format!("[{section_name}] missing")))?;
            change_row(table, m, mem)?;
        } else if lk.starts_with("addrow") {
            let m = ini
                .section(section_name)
                .ok_or_else(|| Error::Ini(format!("[{section_name}] missing")))?;
            add_row(table, m, mem)?;
        } else if lk.starts_with("copyrow") {
            let m = ini
                .section(section_name)
                .ok_or_else(|| Error::Ini(format!("[{section_name}] missing")))?;
            copy_row(table, m, mem)?;
        } else if lk.starts_with("addcolumn") {
            return Err(Error::Unsupported(format!("{key} in a 2DA table")));
        }
    }
    Ok(())
}

fn find_target(table: &TwoDa, m: &[(String, String)]) -> Result<usize> {
    for (k, v) in m {
        match k.to_ascii_lowercase().as_str() {
            "rowindex" => {
                return v
                    .parse::<usize>()
                    .ok()
                    .filter(|i| *i < table.rows())
                    .ok_or_else(|| Error::Apply(format!("RowIndex {v} out of range")));
            }
            "rowlabel" => {
                return table
                    .row_labels
                    .iter()
                    .position(|l| l == v)
                    .ok_or_else(|| Error::Apply(format!("RowLabel {v} not found")));
            }
            "labelindex" => {
                let col = table
                    .column_index("label")
                    .ok_or_else(|| Error::Apply("LabelIndex needs a 'label' column".into()))?;
                // last matching row (`TSLPatcher` semantics)
                return (0..table.rows())
                    .rev()
                    .find(|&r| table.get(r, col) == Some(v.as_str()))
                    .ok_or_else(|| Error::Apply(format!("LabelIndex {v} not found")));
            }
            _ => {}
        }
    }
    Err(Error::Apply("ChangeRow needs a target".into()))
}

fn change_row(table: &mut TwoDa, m: &[(String, String)], mem: &mut Memory) -> Result<()> {
    let row = find_target(table, m)?;
    assign_cells(table, row, m, mem)?;
    capture_memory(table, row, m, mem);
    Ok(())
}

fn add_row(table: &mut TwoDa, m: &[(String, String)], mem: &mut Memory) -> Result<()> {
    // ExclusiveColumn: if a row already has the assigned value there, modify it
    let exclusive = m
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("ExclusiveColumn"))
        .map(|(_, v)| v.clone());
    let row = if let Some(colname) = exclusive {
        let col = table
            .column_index(&colname)
            .ok_or_else(|| Error::Apply(format!("ExclusiveColumn '{colname}' not found")))?;
        let wanted = m
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&colname))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| Error::Apply("ExclusiveColumn value not assigned".into()))?;
        match (0..table.rows()).find(|&r| table.get(r, col) == Some(wanted.as_str())) {
            Some(existing) => existing, // degrade to in-place modify (dedup)
            None => new_row(table, m),
        }
    } else {
        new_row(table, m)
    };
    assign_cells(table, row, m, mem)?;
    capture_memory(table, row, m, mem);
    Ok(())
}

fn new_row(table: &mut TwoDa, m: &[(String, String)]) -> usize {
    let label = m
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("RowLabel") || k.eq_ignore_ascii_case("NewRowLabel"))
        .map_or_else(|| table.rows().to_string(), |(_, v)| v.clone());
    table.add_row(label)
}

/// `CopyRow`: duplicate the row identified by the target (`RowIndex`/`RowLabel`/
/// `LabelIndex` names the SOURCE), then modify the copy. `ExclusiveColumn`
/// dedups on the new row's identity (modify in place if it already exists).
fn copy_row(table: &mut TwoDa, m: &[(String, String)], mem: &mut Memory) -> Result<()> {
    let source = find_target(table, m)?;
    let exclusive = m
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("ExclusiveColumn"))
        .map(|(_, v)| v.clone());
    let row = if let Some(colname) = exclusive {
        let col = table
            .column_index(&colname)
            .ok_or_else(|| Error::Apply(format!("ExclusiveColumn '{colname}' not found")))?;
        let wanted = m
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&colname))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| Error::Apply("ExclusiveColumn value not assigned".into()))?;
        match (0..table.rows()).find(|&r| table.get(r, col) == Some(wanted.as_str())) {
            Some(existing) => existing,
            None => copy_new_row(table, source, m),
        }
    } else {
        copy_new_row(table, source, m)
    };
    assign_cells(table, row, m, mem)?;
    capture_memory(table, row, m, mem);
    Ok(())
}

/// Append a clone of `source`'s cells as a new row. Its row label is
/// `NewRowLabel` if given, else the next index — NOT `RowLabel`, which for
/// `CopyRow` names the source row.
fn copy_new_row(table: &mut TwoDa, source: usize, m: &[(String, String)]) -> usize {
    let label = m
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("NewRowLabel"))
        .map_or_else(|| table.rows().to_string(), |(_, v)| v.clone());
    let cells = table.cells.get(source).cloned().unwrap_or_default();
    table.row_labels.push(label);
    table.cells.push(cells);
    table.rows() - 1
}

fn assign_cells(table: &mut TwoDa, row: usize, m: &[(String, String)], mem: &Memory) -> Result<()> {
    for (k, v) in m {
        let lk = k.to_ascii_lowercase();
        // skip directive keys, not column names
        if matches!(
            lk.as_str(),
            "rowindex" | "rowlabel" | "labelindex" | "newrowlabel" | "exclusivecolumn"
        ) || lk.starts_with("2damemory")
            || lk.starts_with("strref")
        {
            continue;
        }
        let Some(col) = table.column_index(k) else {
            continue; // not a column -> ignore (TSLPatcher tolerates)
        };
        let value = resolve_value(v, table, col, mem)?;
        table.set(row, col, value);
    }
    Ok(())
}

/// Resolve a cell value: `high()`, `2DAMEMORY<n>`, `StrRef<n>`, or literal.
fn resolve_value(v: &str, table: &TwoDa, col: usize, mem: &Memory) -> Result<String> {
    if v.eq_ignore_ascii_case("high()") {
        let max = (0..table.rows())
            .filter_map(|r| table.get(r, col).and_then(|c| c.parse::<i64>().ok()))
            .max()
            .unwrap_or(-1);
        return Ok((max + 1).to_string());
    }
    if let Some(n) = v
        .strip_prefix("2DAMEMORY")
        .and_then(|s| s.parse::<u32>().ok())
    {
        return mem
            .twoda
            .get(&n)
            .cloned()
            .ok_or_else(|| Error::Apply(format!("2DAMEMORY{n} used before set")));
    }
    if let Some(n) = v.strip_prefix("StrRef").and_then(|s| s.parse::<u32>().ok()) {
        return mem
            .strref
            .get(&n)
            .cloned()
            .ok_or_else(|| Error::Apply(format!("StrRef{n} used before set")));
    }
    Ok(v.to_owned())
}

/// `2DAMEMORY<n>=RowIndex|RowLabel|<column>` captures into token memory.
fn capture_memory(table: &TwoDa, row: usize, m: &[(String, String)], mem: &mut Memory) {
    for (k, v) in m {
        let Some(n) = k
            .strip_prefix("2DAMEMORY")
            .or_else(|| k.strip_prefix("2damemory"))
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let captured = match v.to_ascii_lowercase().as_str() {
            "rowindex" => row.to_string(),
            "rowlabel" => table.row_labels.get(row).cloned().unwrap_or_default(),
            other => table
                .column_index(other)
                .and_then(|c| table.get(row, c))
                .unwrap_or_default()
                .to_owned(),
        };
        mem.twoda.insert(n, captured);
    }
}
