//! From the bytes of a PE file to the CLI metadata streams.
//!
//! A managed assembly is an ordinary PE (the Windows executable format, used here as
//! a platform-neutral container) whose 15th data directory points at a CLI header,
//! which in turn points at the metadata root. The metadata root holds named streams;
//! this module digs out the four the reader needs:
//!
//! - `#~` — the tables (types, methods, ...), decoded by [`crate::tables`]
//! - `#Strings` — UTF-8 names, referenced by offset from table rows
//! - `#Blob` — length-prefixed binary blobs, mostly signatures
//! - `#GUID` — module identifiers (sized but unused)

use crate::{error::MetadataError, reader::Reader};

pub(crate) struct MetadataStreams<'data> {
    pub tables: &'data [u8],
    pub strings: &'data [u8],
    pub blobs: &'data [u8],
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

pub(crate) fn metadata_streams(data: &[u8]) -> Result<MetadataStreams<'_>, MetadataError> {
    let mut reader = Reader::new(data);

    // DOS header: "MZ", with the PE header offset at 0x3C
    if reader.bytes(2)? != b"MZ" {
        return Err(MetadataError::NotPeFile);
    }
    let mut reader = Reader::at(data, 0x3C)?;
    let pe_offset = reader.u32()? as usize;

    let mut reader = Reader::at(data, pe_offset)?;
    if reader.bytes(4)? != b"PE\0\0" {
        return Err(MetadataError::NotPeFile);
    }

    // COFF header
    reader.skip(2)?; // machine
    let section_count = reader.u16()? as usize;
    reader.skip(12)?; // timestamp, symbol table, symbol count
    let optional_header_size = reader.u16()? as usize;
    reader.skip(2)?; // characteristics

    // optional header: PE32 (0x10B) or PE32+ (0x20B) decide where the data
    // directories start
    let optional_header_start = reader.position();
    let magic = reader.u16()?;
    let directories_offset = match magic {
        0x10B => 96,
        0x20B => 112,
        _ => return Err(MetadataError::NotPeFile),
    };

    // data directory 14 is the CLI header
    let mut reader = Reader::at(data, optional_header_start + directories_offset)?;
    let directory_count = (optional_header_size - directories_offset) / 8;
    if directory_count < 15 {
        return Err(MetadataError::NotManagedAssembly);
    }
    reader.skip(14 * 8)?;
    let cli_rva = reader.u32()?;
    let cli_size = reader.u32()?;
    if cli_rva == 0 || cli_size == 0 {
        return Err(MetadataError::NotManagedAssembly);
    }

    // section table follows the optional header
    let mut reader = Reader::at(data, optional_header_start + optional_header_size)?;
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        reader.skip(8)?; // name
        let virtual_size = reader.u32()?;
        let virtual_address = reader.u32()?;
        let raw_size = reader.u32()?;
        let raw_offset = reader.u32()?;
        reader.skip(16)?; // relocations, line numbers, characteristics
        sections.push(Section {
            virtual_address,
            virtual_size,
            raw_offset,
            raw_size,
        });
    }

    // CLI header: metadata RVA + size live at offset 8
    let mut reader = Reader::at(data, rva_to_offset(&sections, cli_rva)?)?;
    reader.skip(8)?; // cb, runtime version
    let metadata_rva = reader.u32()?;
    let metadata_size = reader.u32()? as usize;

    let metadata_start = rva_to_offset(&sections, metadata_rva)?;
    if metadata_start + metadata_size > data.len() {
        return Err(MetadataError::UnexpectedEnd);
    }
    let metadata = &data[metadata_start..metadata_start + metadata_size];

    // metadata root: "BSJB", version string, then the stream headers
    let mut reader = Reader::new(metadata);
    if reader.u32()? != 0x424A_5342 {
        return Err(MetadataError::InvalidMetadata);
    }
    reader.skip(8)?; // major, minor, reserved
    let version_length = reader.u32()? as usize;
    reader.skip(version_length)?;
    reader.skip(2)?; // flags
    let stream_count = reader.u16()? as usize;

    let mut tables = None;
    let mut strings = None;
    let mut blobs = None;

    for _ in 0..stream_count {
        let offset = reader.u32()? as usize;
        let size = reader.u32()? as usize;

        // the name is null-terminated, padded to a 4-byte boundary
        let name_start = reader.position();
        let mut name_end = name_start;
        while metadata.get(name_end).copied().unwrap_or(0) != 0 {
            name_end += 1;
        }
        let name = &metadata[name_start..name_end];
        let padded = (name_end - name_start + 1).div_ceil(4) * 4;
        reader.skip(padded)?;

        if offset + size > metadata.len() {
            return Err(MetadataError::InvalidMetadata);
        }
        let stream = &metadata[offset..offset + size];

        match name {
            b"#~" => tables = Some(stream),
            b"#-" => return Err(MetadataError::UncompressedTableStream),
            b"#Strings" => strings = Some(stream),
            b"#Blob" => blobs = Some(stream),
            _ => {} // #GUID, #US, #Pdb, ...
        }
    }

    Ok(MetadataStreams {
        tables: tables.ok_or(MetadataError::InvalidMetadata)?,
        strings: strings.ok_or(MetadataError::InvalidMetadata)?,
        blobs: blobs.unwrap_or(&[]),
    })
}

fn rva_to_offset(sections: &[Section], rva: u32) -> Result<usize, MetadataError> {
    for section in sections {
        let size = section.virtual_size.max(section.raw_size);
        if rva >= section.virtual_address && rva < section.virtual_address + size {
            return Ok((rva - section.virtual_address + section.raw_offset) as usize);
        }
    }
    Err(MetadataError::InvalidRva)
}

/// Reads a `#Strings` heap entry: UTF-8, null-terminated.
pub(crate) fn heap_string(strings: &[u8], offset: u32) -> Result<&str, MetadataError> {
    let start = offset as usize;
    if start >= strings.len() {
        return Err(MetadataError::InvalidHeapOffset);
    }

    let end = strings[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| start + position)
        .ok_or(MetadataError::InvalidHeapOffset)?;

    std::str::from_utf8(&strings[start..end]).map_err(|_| MetadataError::InvalidHeapOffset)
}

/// Reads a `#Blob` heap entry: compressed length, then that many bytes.
pub(crate) fn heap_blob(blobs: &[u8], offset: u32) -> Result<&[u8], MetadataError> {
    let mut reader = Reader::at(blobs, offset as usize)?;
    let length = reader.compressed_u32()? as usize;
    reader.bytes(length)
}
