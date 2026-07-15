//! The conflict matrix: resolve every record's identity through the load
//! order and group overrides by canonical origin. Rule of one: the engine
//! takes the LAST loaded version of a record wholesale, so any record with
//! two or more overriding plugins is a conflict candidate.

use std::collections::BTreeMap;

use crate::reader::Plugin;
use crate::{Result, DANGER_SIGNATURES};

/// Canonical record identity: (origin plugin lowercase, 24-bit object id).
pub type Key = (String, u32);

#[derive(Debug, Clone)]
pub struct Conflict {
    pub origin: String,
    pub object_id: u32,
    pub signature: String,
    /// Plugins carrying a version of this record, in load order
    /// (origin first when it is in the load order).
    pub carriers: Vec<String>,
    /// Last carrier wins (rule of one).
    pub winner: String,
    pub danger: bool,
}

#[derive(Debug, Default)]
pub struct Matrix {
    pub conflicts: Vec<Conflict>,
    /// records overridden by exactly one plugin (plain overrides, not conflicts)
    pub overrides: usize,
    pub records_scanned: usize,
}

/// Build the matrix from plugins in load order.
pub fn build(plugins: &[Plugin]) -> Result<Matrix> {
    let mut carriers: BTreeMap<Key, (String, Vec<String>)> = BTreeMap::new();
    let mut scanned = 0usize;

    for plugin in plugins {
        let self_name = plugin.meta.name.to_lowercase();
        let n_masters = plugin.meta.masters.len();
        for rec in &plugin.records {
            scanned += 1;
            let idx = usize::from(rec.master_index());
            let origin = if idx < n_masters {
                plugin
                    .meta
                    .masters
                    .get(idx)
                    .map_or_else(|| self_name.clone(), |m| m.to_lowercase())
            } else {
                self_name.clone()
            };
            let key = (origin, rec.object_id());
            let entry = carriers
                .entry(key)
                .or_insert_with(|| (rec.sig(), Vec::new()));
            if !entry.1.contains(&plugin.meta.name) {
                entry.1.push(plugin.meta.name.clone());
            }
        }
    }

    let mut matrix = Matrix {
        records_scanned: scanned,
        ..Matrix::default()
    };
    for ((origin, object_id), (signature, list)) in carriers {
        // one carrier = the origin's own record; two = a single override;
        // three or more = competing overrides (conflict).
        match list.len() {
            0 | 1 => {}
            2 => matrix.overrides += 1,
            _ => {
                let winner = list.last().cloned().unwrap_or_default();
                let danger = DANGER_SIGNATURES.contains(&signature.as_str());
                matrix.conflicts.push(Conflict {
                    origin,
                    object_id,
                    signature,
                    carriers: list,
                    winner,
                    danger,
                });
            }
        }
    }
    Ok(matrix)
}
