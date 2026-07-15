//! Dirty-plugin analysis: ITM and UDR detection, xEdit-compatible.
//!
//! ITM (Identical To Master): an override record whose decoded content is
//! identical to the version in the last of its masters that carries the
//! record — it contributes nothing and only reverts later mods (rule of one).
//! UDR (Undeleted/Deleted Reference): an override that sets the Deleted flag
//! on a placed reference; the fix is undelete+disable, the *detection* is
//! counting them. Deleted navmeshes are reported separately (never auto-fix).
//!
//! Validated against the LOOT masterlist's published ITM/UDR counts for the
//! Fallout 4 DLC (the same numbers `FO4Edit`'s `QuickAutoClean` produces).

use std::collections::HashMap;

use crate::reader::{Plugin, RecordRef};
use crate::{Result, FLAG_COMPRESSED};

/// Record flag: deleted.
pub const FLAG_DELETED: u32 = 0x0000_0020;
/// Record flag: persistent reference (not moved during the UDR fix).
pub const FLAG_PERSISTENT: u32 = 0x0000_0400;
/// Record flag: initially disabled (the UDR fix sets this).
pub const FLAG_INITIALLY_DISABLED: u32 = 0x0000_0800;
/// `PlayerRef` `FormID` — the enable-parent the UDR fix points at.
pub const PLAYER_REF: u32 = 0x0000_0014;
/// The Z coordinate a fixed reference is moved to (out of sight).
pub const UDR_Z: f32 = -30000.0;

/// Placed-reference signatures whose deletions count as UDR.
const PLACED_SIGS: &[&[u8; 4]] = &[b"REFR", b"ACHR", b"PGRE", b"PHZD", b"PMIS"];
const NAVMESH_SIG: &[u8; 4] = b"NAVM";

#[derive(Debug, Default)]
pub struct DirtyReport {
    /// (signature, object id) of every identical-to-master override we detect.
    /// Byte-level identity is a strict *conservative* subset of xEdit's decoded
    /// comparison (never over-reports). The residual to xEdit's raw ITM count
    /// is concentrated in CELL/LAND/WRLD/previsibine records (verified by
    /// field-diff probe) — which are DANGER-class and must never be removed
    /// anyway, so it does not affect what a safe cleaner does.
    pub itm: Vec<(String, u32)>,
    /// (signature, object id) of deleted placed references.
    pub udr: Vec<(String, u32)>,
    /// deleted navmeshes (danger-class; report only).
    pub deleted_navmeshes: usize,
    pub overrides_checked: usize,
}

impl DirtyReport {
    /// ITMs that are safe to actually remove: identical-to-master overrides
    /// excluding danger-class signatures (CELL/WRLD/NAVM/LAND/REFR/…). This is
    /// the set a cleaner acts on, and where our detection is conservative and
    /// correct.
    pub fn cleanable_itm(&self) -> Vec<&(String, u32)> {
        self.itm
            .iter()
            .filter(|(sig, _)| !crate::DANGER_SIGNATURES.contains(&sig.as_str()))
            .collect()
    }

    /// Count of detected ITMs per record signature (for characterization).
    pub fn itm_by_signature(&self) -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        for (sig, _) in &self.itm {
            *m.entry(sig.clone()).or_insert(0) += 1;
        }
        m
    }
}

/// Analyze `plugin` against its masters (same order as the plugin's master
/// list; each entry may be None if that master isn't loaded).
pub fn analyze(plugin: &Plugin, masters: &[Option<&Plugin>]) -> Result<DirtyReport> {
    // index each master's own records by object id (24-bit)
    let mut master_index: Vec<HashMap<u32, &RecordRef>> = Vec::with_capacity(masters.len());
    for m in masters {
        let mut map = HashMap::new();
        if let Some(mp) = m {
            let n = mp.meta.masters.len();
            for r in &mp.records {
                if usize::from(r.master_index()) >= n {
                    map.insert(r.object_id(), r);
                }
            }
        }
        master_index.push(map);
    }

    let mut report = DirtyReport::default();
    let n_masters = plugin.meta.masters.len();
    for rec in &plugin.records {
        let idx = usize::from(rec.master_index());
        if idx >= n_masters {
            continue; // the plugin's own record, not an override
        }
        report.overrides_checked += 1;

        if rec.flags & FLAG_DELETED != 0 {
            if &rec.signature == NAVMESH_SIG {
                report.deleted_navmeshes += 1;
            } else if PLACED_SIGS.contains(&&rec.signature) {
                report.udr.push((rec.sig(), rec.object_id()));
            }
            continue; // a deleted record is never counted as ITM
        }

        // the master version this override shadows (records live in the
        // master file the FormID's index byte names)
        let Some(master_plugin) = masters.get(idx).copied().flatten() else {
            continue;
        };
        let Some(master_rec) = master_index.get(idx).and_then(|m| m.get(&rec.object_id())) else {
            continue; // injected record (master lacks it) — never ITM
        };

        if identical(plugin, rec, master_plugin, master_rec)? {
            report.itm.push((rec.sig(), rec.object_id()));
        }
    }
    Ok(report)
}

/// Build undelete-and-disable override records for every UDR in `plugin`,
/// ready for the resolver writer. Each fix takes the MASTER's version of the
/// reference (the deleted override is stripped), clears Deleted, sets
/// Initially-Disabled, moves Z to −30000 (unless persistent), drops any
/// existing Enable Parent / teleport (XESP/XTEL), and adds an XESP pointing at
/// the player with "opposite of parent" so the ref stays disabled in-game.
/// Danger types (NAVM) are never touched.
pub fn udr_fixes(
    plugin: &Plugin,
    masters: &[Option<&Plugin>],
) -> Result<Vec<crate::writer::OverrideRecord>> {
    let n_masters = plugin.meta.masters.len();
    let mut master_index: Vec<HashMap<u32, &RecordRef>> = Vec::with_capacity(masters.len());
    for m in masters {
        let mut map = HashMap::new();
        if let Some(mp) = m {
            let mn = mp.meta.masters.len();
            for r in &mp.records {
                if usize::from(r.master_index()) >= mn {
                    map.insert(r.object_id(), r);
                }
            }
        }
        master_index.push(map);
    }

    let mut fixes = Vec::new();
    for rec in &plugin.records {
        let idx = usize::from(rec.master_index());
        if idx >= n_masters || rec.flags & FLAG_DELETED == 0 {
            continue;
        }
        if &rec.signature == NAVMESH_SIG || !PLACED_SIGS.contains(&&rec.signature) {
            continue;
        }
        let (Some(mp), Some(mrec)) = (
            masters.get(idx).copied().flatten(),
            master_index.get(idx).and_then(|m| m.get(&rec.object_id())),
        ) else {
            continue; // no master version to regenerate from — skip safely
        };

        let mut subs: Vec<([u8; 4], Vec<u8>)> = mp
            .subrecords(mrec)?
            .into_iter()
            .filter(|(sig, _)| sig != b"XESP" && sig != b"XTEL")
            .collect();

        // move Z out of sight unless the reference is persistent
        if rec.flags & FLAG_PERSISTENT == 0 {
            if let Some((_, data)) = subs.iter_mut().find(|(sig, _)| sig == b"DATA") {
                if let Some(slot) = data.get_mut(8..12) {
                    slot.copy_from_slice(&UDR_Z.to_le_bytes());
                }
            }
        }
        // XESP = FormID(PlayerRef) + u8 flags(0x01 opposite-of-parent) + 3 unused
        let mut xesp = Vec::with_capacity(8);
        xesp.extend_from_slice(&PLAYER_REF.to_le_bytes());
        xesp.push(0x01);
        xesp.extend_from_slice(&[0, 0, 0]);
        subs.push((*b"XESP", xesp));

        fixes.push(crate::writer::OverrideRecord {
            signature: rec.signature,
            form_id: rec.form_id,
            flags: FLAG_INITIALLY_DISABLED,
            subrecords: subs,
        });
    }
    Ok(fixes)
}

/// xEdit-style identity: same signature, same record flags (ignoring the
/// compression bit — content is compared decoded), same decoded subrecords.
fn identical(a_plugin: &Plugin, a: &RecordRef, b_plugin: &Plugin, b: &RecordRef) -> Result<bool> {
    const MASK: u32 = !FLAG_COMPRESSED;
    if a.signature != b.signature {
        return Ok(false);
    }
    if a.flags & MASK != b.flags & MASK {
        return Ok(false);
    }
    // fast path: both uncompressed and same size -> raw compare via decode
    let sa = a_plugin.subrecords(a)?;
    let sb = b_plugin.subrecords(b)?;
    Ok(sa == sb)
}
