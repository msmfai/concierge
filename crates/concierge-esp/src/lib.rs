//! Bethesda plugin format for Concierge — the parts no external tool gives
//! us: a fast native reader (record inventory for the conflict matrix) and a
//! resolver-plugin writer. Format facts ground-truthed against a real FO4
//! install and the xEdit/Mutagen/UESP sources (see SPEC/research notes):
//! 24-byte record headers, form version 131, GRUP size INCLUDES its header,
//! ESL flag 0x200 with the FE slot space, compressed records are
//! u32-decompressed-size + zlib, XXXX extends the next subrecord.

pub mod clean;
pub mod conflicts;
pub mod formlist;
pub mod lvli;
pub mod reader;
pub mod writer;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: malformed at offset {offset}: {what}")]
    Malformed {
        path: std::path::PathBuf,
        offset: usize,
        what: String,
    },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// TES4 record flag bits.
pub const FLAG_ESM: u32 = 0x0000_0001;
pub const FLAG_LOCALIZED: u32 = 0x0000_0080;
pub const FLAG_ESL: u32 = 0x0000_0200;
/// Any-record flag: payload is u32 decompressed size + zlib stream.
pub const FLAG_COMPRESSED: u32 = 0x0004_0000;

/// Record signatures whose conflicts are DANGER-class in Fallout 4:
/// previsibines (CELL/WRLD) and navmesh data must never be naively merged,
/// and placed-reference conflicts routinely implicate them.
pub const DANGER_SIGNATURES: &[&str] = &[
    "CELL", "WRLD", "NAVM", "NAVI", "LAND", "REFR", "ACHR", "PGRE", "PHZD",
];
