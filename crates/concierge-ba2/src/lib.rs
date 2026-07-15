//! Native Bethesda Archive (BA2, magic `BTDX`, version 1) reader — the packed
//! archive format Fallout 4 ships assets in. Two variants: `GNRL` (general
//! files, optionally zlib-compressed) and `DX10` (textures stored as mip
//! chunks; extraction reconstructs a `.dds`). Implemented from the published
//! format layout.
//!
//! Scope (rung 0, Acquire): list entries and extract a named file's bytes, so a
//! mod that ships packed BA2s is inspectable and game-resident records are
//! reachable.

#![deny(clippy::arithmetic_side_effects)] // parser: checked arithmetic only
mod dds;
mod reader;

use std::io::Read as _;

use reader::Cursor;

pub const MAGIC: [u8; 4] = *b"BTDX";
const GNRL: [u8; 4] = *b"GNRL";
const DX10: [u8; 4] = *b"DX10";
const GNRL_RECORD_LEN: usize = 36;
const DX10_CHUNK_LEN: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a BA2: bad magic")]
    BadMagic,
    #[error("unsupported BA2 version {0} (only v1)")]
    Version(u32),
    #[error("unknown BA2 type {0:?}")]
    UnknownType([u8; 4]),
    #[error("truncated archive at byte {at}")]
    Truncated { at: usize },
    #[error("zlib: {0}")]
    Zlib(String),
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("declared size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: usize, actual: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    General,
    Texture,
}

/// One archived file. `name` is the archive-relative path (backslashes as in
/// the name table). For `General` the data lives at `offset`; for `Texture` the
/// mip `chunks` are concatenated under a reconstructed DDS header.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub extension: [u8; 4],
    kind: EntryData,
}

#[derive(Debug, Clone)]
enum EntryData {
    General {
        offset: u64,
        packed: u32,
        unpacked: u32,
    },
    Texture {
        width: u16,
        height: u16,
        num_mips: u8,
        format: u8,
        is_cubemap: bool,
        chunks: Vec<Chunk>,
    },
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    offset: u64,
    packed: u32,
    unpacked: u32,
}

impl Entry {
    #[must_use]
    pub fn extension_str(&self) -> String {
        String::from_utf8_lossy(&self.extension)
            .trim_end_matches('\0')
            .trim_end()
            .to_owned()
    }
}

/// A parsed BA2 borrowing the source bytes (zero-copy until extraction).
#[derive(Debug)]
pub struct Archive<'a> {
    data: &'a [u8],
    kind: Kind,
    entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Parse the header, records, and name table. Records are validated on the
    /// way in — an `Entry` that exists is one whose offsets fit the buffer.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let mut cur = Cursor::new(data);
        if cur.tag()? != MAGIC {
            return Err(Error::BadMagic);
        }
        let version = cur.u32_le()?;
        if version != 1 {
            return Err(Error::Version(version));
        }
        let type_tag = cur.tag()?;
        let kind = match type_tag {
            GNRL => Kind::General,
            DX10 => Kind::Texture,
            other => return Err(Error::UnknownType(other)),
        };
        let file_count = cur.u32_le()?;
        let name_table_offset = cur.u64_le()?;

        // records
        let count = usize::try_from(file_count).map_err(|_| Error::Truncated { at: 0 })?;
        let raw: Vec<(EntryData, [u8; 4])> = match kind {
            Kind::General => parse_general_records(&mut cur, count)?,
            Kind::Texture => parse_texture_records(&mut cur, count)?,
        };

        // names (u16 length + bytes), one per file, in record order
        let names = parse_names(data, name_table_offset, count)?;

        let entries = raw
            .into_iter()
            .zip(names)
            .map(|((kind, extension), name)| Entry {
                name,
                extension,
                kind,
            })
            .collect();
        Ok(Self {
            data,
            kind,
            entries,
        })
    }

    fn find(&self, name: &str) -> Option<&Entry> {
        let want = normalize(name);
        self.entries.iter().find(|e| normalize(&e.name) == want)
    }

    /// Extract a named file's bytes: the raw (zlib-decompressed) file for
    /// `General`, or a reconstructed `.dds` for `Texture`. `name` matches
    /// case-insensitively with `/` and `\` treated the same.
    pub fn extract(&self, name: &str) -> Result<Vec<u8>, Error> {
        let entry = self
            .find(name)
            .ok_or_else(|| Error::NotFound(name.to_owned()))?;
        match &entry.kind {
            EntryData::General {
                offset,
                packed,
                unpacked,
            } => self.read_block(*offset, *packed, *unpacked),
            EntryData::Texture {
                width,
                height,
                num_mips,
                format,
                is_cubemap,
                chunks,
            } => {
                let mut out = dds::header(*width, *height, *num_mips, *format, *is_cubemap);
                for chunk in chunks {
                    let block = self.read_block(chunk.offset, chunk.packed, chunk.unpacked)?;
                    out.extend_from_slice(&block);
                }
                Ok(out)
            }
        }
    }

    /// Read a data block: if `packed != 0` it is a zlib stream decompressing to
    /// `unpacked` bytes; otherwise `unpacked` raw bytes at `offset`.
    fn read_block(&self, offset: u64, packed: u32, unpacked: u32) -> Result<Vec<u8>, Error> {
        let cur = Cursor::new(self.data);
        let unpacked_len = usize::try_from(unpacked).map_err(|_| Error::Truncated { at: 0 })?;
        if packed == 0 {
            return Ok(cur.slice_at(offset, unpacked_len)?.to_vec());
        }
        let packed_len = usize::try_from(packed).map_err(|_| Error::Truncated { at: 0 })?;
        let stream = cur.slice_at(offset, packed_len)?;
        let mut out = Vec::with_capacity(unpacked_len);
        flate2::read::ZlibDecoder::new(stream)
            .read_to_end(&mut out)
            .map_err(|e| Error::Zlib(e.to_string()))?;
        if out.len() != unpacked_len {
            return Err(Error::SizeMismatch {
                expected: unpacked_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

fn parse_general_records(
    cur: &mut Cursor<'_>,
    count: usize,
) -> Result<Vec<(EntryData, [u8; 4])>, Error> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let _name_hash = cur.u32_le()?;
        let extension = cur.tag()?;
        let _dir_hash = cur.u32_le()?;
        let _flags = cur.u32_le()?;
        let offset = cur.u64_le()?;
        let packed = cur.u32_le()?;
        let unpacked = cur.u32_le()?;
        let _align = cur.u32_le()?; // 0xBAADF00D
        out.push((
            EntryData::General {
                offset,
                packed,
                unpacked,
            },
            extension,
        ));
    }
    debug_assert_eq!(GNRL_RECORD_LEN, 36);
    Ok(out)
}

fn parse_texture_records(
    cur: &mut Cursor<'_>,
    count: usize,
) -> Result<Vec<(EntryData, [u8; 4])>, Error> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let _name_hash = cur.u32_le()?;
        let extension = cur.tag()?;
        let _dir_hash = cur.u32_le()?;
        let _unk = cur.u8()?;
        let num_chunks = cur.u8()?;
        let _chunk_header_size = cur.u16_le()?;
        let height = cur.u16_le()?;
        let width = cur.u16_le()?;
        let num_mips = cur.u8()?;
        let format = cur.u8()?;
        let is_cubemap = cur.u16_le()? == 2049;
        let mut chunks = Vec::with_capacity(usize::from(num_chunks));
        for _ in 0..num_chunks {
            let offset = cur.u64_le()?;
            let packed = cur.u32_le()?;
            let unpacked = cur.u32_le()?;
            let _start_mip = cur.u16_le()?;
            let _end_mip = cur.u16_le()?;
            let _align = cur.u32_le()?; // 0xBAADF00D
            chunks.push(Chunk {
                offset,
                packed,
                unpacked,
            });
        }
        out.push((
            EntryData::Texture {
                width,
                height,
                num_mips,
                format,
                is_cubemap,
                chunks,
            },
            extension,
        ));
    }
    debug_assert_eq!(DX10_CHUNK_LEN, 24);
    Ok(out)
}

fn parse_names(data: &[u8], table_offset: u64, count: usize) -> Result<Vec<String>, Error> {
    let mut cur = Cursor::new(data);
    cur.seek(table_offset)?;
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::from(cur.u16_le()?);
        let bytes = cur.take(len)?;
        names.push(String::from_utf8_lossy(bytes).into_owned());
    }
    Ok(names)
}

/// Case-insensitive path key with `\` and `/` unified.
fn normalize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' => '\\',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}
