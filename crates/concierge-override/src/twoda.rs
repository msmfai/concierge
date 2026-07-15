//! KOTOR 2DA binary format (v2.b) — native read/write. This is the table
//! format `TSLPatcher`/`HoloPatcher` edit; owning it means we never redistribute
//! their tools. Layout: `"2DA V2.b"` + `0x0A`; tab-terminated column labels
//! ending in `0x00`; `u32` row count; tab-terminated row labels; a
//! `rows*cols` table of `u16` cell offsets into the data block; a `u16` data
//! size; then the null-terminated string data (offsets may share strings).

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("2da: {0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub const SIGNATURE: &[u8] = b"2DA V2.b";

#[derive(Debug, Clone, Default)]
pub struct TwoDa {
    pub columns: Vec<String>,
    pub row_labels: Vec<String>,
    /// `rows` × `cols` cell strings, row-major.
    pub cells: Vec<Vec<String>>,
}

impl TwoDa {
    pub const fn cols(&self) -> usize {
        self.columns.len()
    }
    pub const fn rows(&self) -> usize {
        self.row_labels.len()
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, row: usize, col: usize) -> Option<&str> {
        self.cells
            .get(row)
            .and_then(|r| r.get(col))
            .map(String::as_str)
    }

    pub fn set(&mut self, row: usize, col: usize, value: impl Into<String>) {
        if let Some(cell) = self.cells.get_mut(row).and_then(|r| r.get_mut(col)) {
            *cell = value.into();
        }
    }

    /// Append a row (label + a blank cell per column); returns its index.
    pub fn add_row(&mut self, label: impl Into<String>) -> usize {
        self.row_labels.push(label.into());
        self.cells.push(vec![String::new(); self.columns.len()]);
        self.rows() - 1
    }

    // --- read ---------------------------------------------------------------

    pub fn parse(data: &[u8]) -> Result<Self> {
        let bad = |m: &str| Error::Format(m.to_owned());
        if data.get(..SIGNATURE.len()) != Some(SIGNATURE) {
            return Err(bad("missing 2DA V2.b signature"));
        }
        let mut pos = SIGNATURE.len();
        // newline after signature
        if data.get(pos) == Some(&0x0A) {
            pos += 1;
        }
        // column labels: each terminated by 0x09, section ends at 0x00
        let mut columns = Vec::new();
        loop {
            match data.get(pos) {
                Some(0x00) => {
                    pos += 1;
                    break;
                }
                Some(_) => {
                    let (s, next) = read_until(data, pos, 0x09)?;
                    columns.push(s);
                    pos = next;
                }
                None => return Err(bad("truncated in column labels")),
            }
        }
        let row_count =
            usize::try_from(read_u32(data, pos).ok_or_else(|| bad("truncated row count"))?)
                .unwrap_or(0);
        pos += 4;
        let mut row_labels = Vec::with_capacity(row_count);
        for _ in 0..row_count {
            let (s, next) = read_until(data, pos, 0x09)?;
            row_labels.push(s);
            pos = next;
        }
        let cols = columns.len();
        let cell_count = row_count * cols;
        let mut offsets = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            offsets.push(read_u16(data, pos).ok_or_else(|| bad("truncated offset table"))?);
            pos += 2;
        }
        let _data_size = read_u16(data, pos).ok_or_else(|| bad("truncated data size"))?;
        pos += 2;
        let data_start = pos;
        let mut cells = Vec::with_capacity(row_count);
        for r in 0..row_count {
            let mut row = Vec::with_capacity(cols);
            for c in 0..cols {
                let off = usize::from(
                    *offsets
                        .get(r * cols + c)
                        .ok_or_else(|| bad("offset index out of range"))?,
                );
                let (s, _) = read_until(data, data_start + off, 0x00)?;
                row.push(s);
            }
            cells.push(row);
        }
        Ok(Self {
            columns,
            row_labels,
            cells,
        })
    }

    // --- write --------------------------------------------------------------

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE);
        out.push(0x0A);
        for c in &self.columns {
            out.extend_from_slice(c.as_bytes());
            out.push(0x09);
        }
        out.push(0x00);
        out.extend_from_slice(&u32::try_from(self.rows()).unwrap_or(0).to_le_bytes());
        for l in &self.row_labels {
            out.extend_from_slice(l.as_bytes());
            out.push(0x09);
        }
        // deduplicated string pool -> offsets
        let mut pool: Vec<u8> = Vec::new();
        let mut seen: std::collections::HashMap<&str, u16> = std::collections::HashMap::new();
        let mut offsets: Vec<u16> = Vec::with_capacity(self.rows() * self.cols());
        for row in &self.cells {
            for cell in row {
                let off = if let Some(&o) = seen.get(cell.as_str()) {
                    o
                } else {
                    let o = u16::try_from(pool.len()).unwrap_or(0);
                    pool.extend_from_slice(cell.as_bytes());
                    pool.push(0x00);
                    seen.insert(cell.as_str(), o);
                    o
                };
                offsets.push(off);
            }
        }
        for o in &offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&u16::try_from(pool.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&pool);
        out
    }
}

fn read_u16(d: &[u8], p: usize) -> Option<u16> {
    d.get(p..p + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
}
fn read_u32(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
}

/// Read bytes from `p` up to (not including) `term`; return (string, index
/// after the terminator).
fn read_until(d: &[u8], p: usize, term: u8) -> Result<(String, usize)> {
    let mut i = p;
    while let Some(&b) = d.get(i) {
        if b == term {
            let s = String::from_utf8_lossy(d.get(p..i).unwrap_or_default()).into_owned();
            return Ok((s, i + 1));
        }
        i += 1;
    }
    Err(Error::Format("unterminated string".into()))
}
