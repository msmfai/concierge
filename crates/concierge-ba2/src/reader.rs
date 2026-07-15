//! A bounds- and overflow-checked byte cursor. All position arithmetic is
//! `checked_*`, so a malformed/hostile archive can never index out of range or
//! wrap — the errors are values, never panics. This is where the parser
//! offloads memory-safety to the technique.

use crate::Error;

pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Move to an absolute offset (must be within the buffer).
    pub fn seek(&mut self, offset: u64) -> Result<(), Error> {
        let off = usize::try_from(offset).map_err(|_| Error::Truncated { at: 0 })?;
        if off > self.data.len() {
            return Err(Error::Truncated { at: off });
        }
        self.pos = off;
        Ok(())
    }

    /// Borrow the next `len` bytes and advance, checked against the end.
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Error::Truncated { at: self.pos })?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(Error::Truncated { at: self.pos })?;
        self.pos = end;
        Ok(slice)
    }

    /// Borrow `len` bytes at an absolute offset without moving the cursor.
    pub fn slice_at(&self, offset: u64, len: usize) -> Result<&'a [u8], Error> {
        let off = usize::try_from(offset).map_err(|_| Error::Truncated { at: 0 })?;
        let end = off.checked_add(len).ok_or(Error::Truncated { at: off })?;
        self.data.get(off..end).ok_or(Error::Truncated { at: off })
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        let b = self.take(1)?;
        b.first().copied().ok_or(Error::Truncated { at: self.pos })
    }

    pub fn u16_le(&mut self) -> Result<u16, Error> {
        let b: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| Error::Truncated { at: self.pos })?;
        Ok(u16::from_le_bytes(b))
    }

    pub fn u32_le(&mut self) -> Result<u32, Error> {
        let b: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| Error::Truncated { at: self.pos })?;
        Ok(u32::from_le_bytes(b))
    }

    pub fn u64_le(&mut self) -> Result<u64, Error> {
        let b: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| Error::Truncated { at: self.pos })?;
        Ok(u64::from_le_bytes(b))
    }

    pub fn tag(&mut self) -> Result<[u8; 4], Error> {
        self.take(4)?
            .try_into()
            .map_err(|_| Error::Truncated { at: self.pos })
    }
}
