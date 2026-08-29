//! A bounds-checked little-endian byte cursor.
//!
//! Every read returns `Err` instead of panicking: the input is an arbitrary file the
//! user pointed the compiler at, so a truncated or corrupt assembly must surface as
//! a [`MetadataError`], never as an index-out-of-bounds panic.

use crate::error::MetadataError;

#[derive(Clone, Copy)]
pub(crate) struct Reader<'data> {
    data: &'data [u8],
    position: usize,
}

impl<'data> Reader<'data> {
    pub fn new(data: &'data [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn at(data: &'data [u8], position: usize) -> Result<Self, MetadataError> {
        if position > data.len() {
            return Err(MetadataError::UnexpectedEnd);
        }
        Ok(Self { data, position })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    pub fn skip(&mut self, count: usize) -> Result<(), MetadataError> {
        if self.remaining() < count {
            return Err(MetadataError::UnexpectedEnd);
        }
        self.position += count;
        Ok(())
    }

    pub fn bytes(&mut self, count: usize) -> Result<&'data [u8], MetadataError> {
        if self.remaining() < count {
            return Err(MetadataError::UnexpectedEnd);
        }
        let slice = &self.data[self.position..self.position + count];
        self.position += count;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, MetadataError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, MetadataError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, MetadataError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, MetadataError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// An index into a heap or table: 2 or 4 bytes wide depending on the file.
    pub fn index(&mut self, wide: bool) -> Result<u32, MetadataError> {
        if wide {
            self.u32()
        } else {
            Ok(self.u16()? as u32)
        }
    }

    /// ECMA-335 II.23.2 compressed unsigned integer.
    pub fn compressed_u32(&mut self) -> Result<u32, MetadataError> {
        let first = self.u8()?;

        if first & 0x80 == 0 {
            Ok(first as u32)
        } else if first & 0xC0 == 0x80 {
            let second = self.u8()?;
            Ok((((first & 0x3F) as u32) << 8) | second as u32)
        } else if first & 0xE0 == 0xC0 {
            let rest = self.bytes(3)?;
            Ok((((first & 0x1F) as u32) << 24)
                | ((rest[0] as u32) << 16)
                | ((rest[1] as u32) << 8)
                | rest[2] as u32)
        } else {
            Err(MetadataError::InvalidCompressedInteger)
        }
    }
}
