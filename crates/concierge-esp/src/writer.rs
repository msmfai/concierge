//! Resolver-plugin writer: a byte-valid, plain-ESP (loads-last), correctly-mastered
//! plugin per the ground-truthed spec (header 1.0, form version 131,
//! `MAST` order = `FormID` master-index order, `DATA` u64=0 after each `MAST`).
//! v0 writes the empty shell the resolver will grow into; it must round-trip
//! through our own reader.

use std::path::Path;

use crate::{Error, Result};

pub const HEADER_VERSION: f32 = 1.0;
pub const FORM_VERSION: u16 = 131;

fn push_subrecord(out: &mut Vec<u8>, sig: [u8; 4], payload: &[u8]) -> Result<()> {
    let len: u16 = payload
        .len()
        .try_into()
        .map_err(|_| Error::Other("subrecord payload exceeds u16".into()))?;
    out.extend_from_slice(&sig);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// An override record the resolver carries: its `FormID` (with the correct
/// master-index byte) and the subrecords to serialize.
#[derive(Debug, Clone)]
pub struct OverrideRecord {
    pub signature: [u8; 4],
    pub form_id: u32,
    /// Record header flags (0 for a plain override; UDR fixes set
    /// Initially-Disabled).
    pub flags: u32,
    pub subrecords: Vec<([u8; 4], Vec<u8>)>,
}

fn serialize_record(rec: &OverrideRecord) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for (sig, payload) in &rec.subrecords {
        push_subrecord(&mut body, *sig, payload)?;
    }
    let data_size: u32 = body
        .len()
        .try_into()
        .map_err(|_| Error::Other("record body exceeds u32".into()))?;
    let mut out = Vec::with_capacity(24 + body.len());
    out.extend_from_slice(&rec.signature);
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&rec.flags.to_le_bytes());
    out.extend_from_slice(&rec.form_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // VC info
    out.extend_from_slice(&FORM_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // VC info 2
    out.extend_from_slice(&body);
    Ok(out)
}

/// One top-level GRUP (type 0) holding all records of a single signature.
/// GRUP size INCLUDES the 24-byte header.
fn serialize_top_group(signature: [u8; 4], records: &[Vec<u8>]) -> Result<Vec<u8>> {
    let inner: usize = records.iter().map(Vec::len).sum();
    let group_size: u32 = (24 + inner)
        .try_into()
        .map_err(|_| Error::Other("GRUP size exceeds u32".into()))?;
    let mut out = Vec::with_capacity(24 + inner);
    out.extend_from_slice(b"GRUP");
    out.extend_from_slice(&group_size.to_le_bytes());
    out.extend_from_slice(&signature); // label = record signature
    out.extend_from_slice(&0i32.to_le_bytes()); // groupType 0 (top-level)
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // VC info
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    for r in records {
        out.extend_from_slice(r);
    }
    Ok(out)
}

/// Serialize a resolver plugin carrying `overrides` (grouped into top-level
/// GRUPs by signature). `masters` in load order. Empty `overrides` yields the
/// minimal valid shell.
pub fn resolver_bytes_with(
    author: &str,
    masters: &[String],
    overrides: &[OverrideRecord],
) -> Result<Vec<u8>> {
    // group serialized records by signature, preserving first-seen order
    let mut order: Vec<[u8; 4]> = Vec::new();
    let mut groups: std::collections::BTreeMap<[u8; 4], Vec<Vec<u8>>> =
        std::collections::BTreeMap::new();
    for rec in overrides {
        if !groups.contains_key(&rec.signature) {
            order.push(rec.signature);
        }
        groups
            .entry(rec.signature)
            .or_default()
            .push(serialize_record(rec)?);
    }
    let mut group_blobs = Vec::new();
    let mut num_records: u32 = 0;
    for sig in &order {
        if let Some(recs) = groups.get(sig) {
            num_records += 1; // the GRUP
            num_records += u32::try_from(recs.len()).unwrap_or(0);
            group_blobs.push(serialize_top_group(*sig, recs)?);
        }
    }

    let mut body = Vec::new();
    let mut hedr = Vec::with_capacity(12);
    hedr.extend_from_slice(&HEADER_VERSION.to_le_bytes());
    hedr.extend_from_slice(&num_records.to_le_bytes());
    hedr.extend_from_slice(&0x800u32.to_le_bytes()); // nextObjectID
    push_subrecord(&mut body, *b"HEDR", &hedr)?;

    let mut cnam = author.as_bytes().to_vec();
    cnam.push(0);
    push_subrecord(&mut body, *b"CNAM", &cnam)?;

    for master in masters {
        let mut mast = master.as_bytes().to_vec();
        mast.push(0);
        push_subrecord(&mut body, *b"MAST", &mast)?;
        push_subrecord(&mut body, *b"DATA", &0u64.to_le_bytes())?;
    }

    let data_size: u32 = body
        .len()
        .try_into()
        .map_err(|_| Error::Other("TES4 body exceeds u32".into()))?;
    let mut out = Vec::with_capacity(24 + body.len());
    out.extend_from_slice(b"TES4");
    out.extend_from_slice(&data_size.to_le_bytes());
    // PLAIN ESP (flags 0), NOT ESL: Fallout 4 loads ESM/ESL-flagged plugins in
    // the master block, BEFORE regular ESPs, so an ESL resolver would load
    // early and LOSE conflicts to ESP mods. A merge patch must be a plain ESP
    // loaded last to win — exactly how Wrye Bash / xEdit merged patches work.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // FormID 0
    out.extend_from_slice(&0u32.to_le_bytes()); // VC info
    out.extend_from_slice(&FORM_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // VC info 2
    out.extend_from_slice(&body);
    for g in group_blobs {
        out.extend_from_slice(&g);
    }
    Ok(out)
}

/// Minimal resolver shell (no override records).
pub fn resolver_bytes(author: &str, masters: &[String]) -> Result<Vec<u8>> {
    resolver_bytes_with(author, masters, &[])
}

pub fn write_resolver(path: &Path, author: &str, masters: &[String]) -> Result<()> {
    let bytes = resolver_bytes(author, masters)?;
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
