//! Plugin reader: TES4 header parse + full record inventory via GRUP walk.
//!
//! Payloads are skipped during the walk (the conflict matrix needs record
//! identity, not fields); `subrecords()` decodes a single record's payload on
//! demand, inflating compressed records and honouring XXXX size extension.

use std::path::{Path, PathBuf};

use crate::{Error, Result, FLAG_COMPRESSED, FLAG_ESL, FLAG_ESM, FLAG_LOCALIZED};

pub const RECORD_HEADER_LEN: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct PluginMeta {
    pub name: String,
    pub is_esm: bool,
    pub is_esl: bool,
    pub is_localized: bool,
    pub header_version: f32,
    pub form_version: u16,
    pub num_records: u32,
    pub next_object_id: u32,
    pub author: String,
    pub masters: Vec<String>,
    pub onam_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef {
    pub signature: [u8; 4],
    pub form_id: u32,
    pub flags: u32,
    pub form_version: u16,
    /// Absolute file offset of the record header (for on-demand decode).
    pub offset: usize,
    pub data_size: u32,
}

impl RecordRef {
    pub fn sig(&self) -> String {
        String::from_utf8_lossy(&self.signature).into_owned()
    }
    /// Master-table index byte (which file this record belongs to/overrides).
    pub fn master_index(&self) -> u8 {
        u8::try_from(self.form_id >> 24).unwrap_or(0)
    }
    pub const fn object_id(&self) -> u32 {
        self.form_id & 0x00FF_FFFF
    }
}

#[derive(Debug)]
pub struct Plugin {
    pub path: PathBuf,
    pub meta: PluginMeta,
    pub records: Vec<RecordRef>,
    data: Vec<u8>,
}

fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
}

fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
}

fn le_f32(b: &[u8], off: usize) -> Option<f32> {
    le_u32(b, off).map(f32::from_bits)
}

impl Plugin {
    pub fn read(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let bad = |offset: usize, what: &str| Error::Malformed {
            path: path.to_path_buf(),
            offset,
            what: what.to_owned(),
        };

        if data.get(..4) != Some(b"TES4") {
            return Err(bad(0, "missing TES4 header record"));
        }
        let tes4_size =
            usize::try_from(le_u32(&data, 4).ok_or_else(|| bad(4, "truncated"))?).unwrap_or(0);
        let tes4_flags = le_u32(&data, 8).ok_or_else(|| bad(8, "truncated"))?;
        let form_version = le_u16(&data, 20).ok_or_else(|| bad(20, "truncated"))?;

        let mut meta = PluginMeta {
            name: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            is_esm: tes4_flags & FLAG_ESM != 0
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("esm")),
            is_esl: tes4_flags & FLAG_ESL != 0
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("esl")),
            is_localized: tes4_flags & FLAG_LOCALIZED != 0,
            header_version: 0.0,
            form_version,
            num_records: 0,
            next_object_id: 0,
            author: String::new(),
            masters: Vec::new(),
            onam_count: 0,
        };

        // TES4 subrecords (never compressed).
        let body_start = RECORD_HEADER_LEN;
        let body_end = body_start + tes4_size;
        if data.len() < body_end {
            return Err(bad(body_start, "TES4 dataSize exceeds file"));
        }
        let mut off = body_start;
        while off < body_end {
            let sig = data
                .get(off..off + 4)
                .ok_or_else(|| bad(off, "subrecord truncated"))?;
            let size = usize::from(
                le_u16(&data, off + 4).ok_or_else(|| bad(off + 4, "subrecord truncated"))?,
            );
            let payload = data
                .get(off + 6..off + 6 + size)
                .ok_or_else(|| bad(off + 6, "subrecord payload truncated"))?;
            match sig {
                b"HEDR" => {
                    meta.header_version =
                        le_f32(payload, 0).ok_or_else(|| bad(off, "HEDR too short"))?;
                    meta.num_records =
                        le_u32(payload, 4).ok_or_else(|| bad(off, "HEDR too short"))?;
                    meta.next_object_id =
                        le_u32(payload, 8).ok_or_else(|| bad(off, "HEDR too short"))?;
                }
                b"CNAM" => {
                    meta.author = zstring(payload);
                }
                b"MAST" => meta.masters.push(zstring(payload)),
                b"ONAM" => meta.onam_count += size / 4,
                _ => {}
            }
            off += 6 + size;
        }

        // Walk the rest of the file: top-level GRUPs containing records and
        // nested GRUPs. GRUP size INCLUDES its own 24-byte header.
        let mut records = Vec::new();
        let mut off = body_end;
        while off < data.len() {
            walk(&data, path, &mut off, data.len(), &mut records)?;
        }

        Ok(Self {
            path: path.to_path_buf(),
            meta,
            records,
            data,
        })
    }

    /// Decode one record's subrecords on demand: (signature, payload) pairs.
    /// Handles compression and the XXXX size extension.
    pub fn subrecords(&self, rec: &RecordRef) -> Result<Vec<([u8; 4], Vec<u8>)>> {
        let bad = |what: &str| Error::Malformed {
            path: self.path.clone(),
            offset: rec.offset,
            what: what.to_owned(),
        };
        let start = rec.offset + RECORD_HEADER_LEN;
        let raw = self
            .data
            .get(start..start + usize::try_from(rec.data_size).unwrap_or(0))
            .ok_or_else(|| bad("record payload truncated"))?;
        let body: Vec<u8> = if rec.flags & FLAG_COMPRESSED != 0 {
            let expected = le_u32(raw, 0).ok_or_else(|| bad("compressed header truncated"))?;
            let mut out = Vec::with_capacity(usize::try_from(expected).unwrap_or(0));
            let stream = raw.get(4..).ok_or_else(|| bad("compressed body missing"))?;
            let mut dec = flate2::read::ZlibDecoder::new(stream);
            std::io::Read::read_to_end(&mut dec, &mut out)
                .map_err(|e| bad(&format!("zlib: {e}")))?;
            if out.len() != usize::try_from(expected).unwrap_or(0) {
                return Err(bad("decompressed size mismatch"));
            }
            out
        } else {
            raw.to_vec()
        };

        let mut subs = Vec::new();
        let mut off = 0usize;
        let mut extended: Option<u32> = None;
        while off < body.len() {
            let sig: [u8; 4] = body
                .get(off..off + 4)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| bad("subrecord truncated"))?;
            let declared = le_u16(&body, off + 4).ok_or_else(|| bad("subrecord truncated"))?;
            if &sig == b"XXXX" {
                extended = le_u32(&body, off + 6);
                off += 6 + usize::from(declared);
                continue;
            }
            let size = usize::try_from(extended.take().unwrap_or(u32::from(declared))).unwrap_or(0);
            let payload = body
                .get(off + 6..off + 6 + size)
                .ok_or_else(|| bad("subrecord payload truncated"))?;
            subs.push((sig, payload.to_vec()));
            off += 6 + size;
        }
        Ok(subs)
    }
}

fn zstring(payload: &[u8]) -> String {
    let end = payload
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(payload.len());
    String::from_utf8_lossy(payload.get(..end).unwrap_or_default()).into_owned()
}

/// Walk one GRUP (or bare record) at `*off`, appending records.
fn walk(
    data: &[u8],
    path: &Path,
    off: &mut usize,
    end: usize,
    records: &mut Vec<RecordRef>,
) -> Result<()> {
    let bad = |offset: usize, what: &str| Error::Malformed {
        path: path.to_path_buf(),
        offset,
        what: what.to_owned(),
    };
    let start = *off;
    let sig = data
        .get(start..start + 4)
        .ok_or_else(|| bad(start, "header truncated"))?;
    if sig == b"GRUP" {
        let group_size = usize::try_from(
            le_u32(data, start + 4).ok_or_else(|| bad(start + 4, "GRUP truncated"))?,
        )
        .unwrap_or(0);
        if group_size < RECORD_HEADER_LEN || start + group_size > end {
            return Err(bad(start, "GRUP size out of bounds"));
        }
        let group_end = start + group_size;
        *off = start + RECORD_HEADER_LEN;
        while *off < group_end {
            walk(data, path, off, group_end, records)?;
        }
        *off = group_end;
        return Ok(());
    }
    // ordinary record
    let data_size = le_u32(data, start + 4).ok_or_else(|| bad(start + 4, "record truncated"))?;
    let flags = le_u32(data, start + 8).ok_or_else(|| bad(start + 8, "record truncated"))?;
    let form_id = le_u32(data, start + 12).ok_or_else(|| bad(start + 12, "record truncated"))?;
    let form_version =
        le_u16(data, start + 20).ok_or_else(|| bad(start + 20, "record truncated"))?;
    let sig_arr: [u8; 4] = sig.try_into().map_err(|_| bad(start, "bad signature"))?;
    records.push(RecordRef {
        signature: sig_arr,
        form_id,
        flags,
        form_version,
        offset: start,
        data_size,
    });
    let next = start + RECORD_HEADER_LEN + usize::try_from(data_size).unwrap_or(0);
    if next > end {
        return Err(bad(start, "record overruns container"));
    }
    *off = next;
    Ok(())
}
