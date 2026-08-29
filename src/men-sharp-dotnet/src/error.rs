//! Errors for a file that is not the managed assembly it claims to be.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataError {
    /// The file ended in the middle of a structure.
    UnexpectedEnd,
    /// No `MZ` / `PE\0\0` signatures — not a PE file at all.
    NotPeFile,
    /// A PE file with no CLI header: a native dll, which carries no managed metadata.
    NotManagedAssembly,
    /// The `BSJB` metadata signature was missing or a required stream was absent.
    InvalidMetadata,
    /// An RVA pointed outside every section.
    InvalidRva,
    /// The `#-` (uncompressed, edit-and-continue) table stream, which tools never
    /// ship and this reader does not support.
    UncompressedTableStream,
    /// A table this reader does not know how to size, so nothing after it could be
    /// read either.
    UnknownTable(u8),
    /// A string that was not UTF-8, an out-of-range heap offset, or similar.
    InvalidHeapOffset,
    InvalidCompressedInteger,
    /// A type/method signature blob with an element kind this reader does not know.
    InvalidSignature,
    /// A table row referred to a row that does not exist.
    InvalidTableIndex,
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataError::UnexpectedEnd => write!(f, "unexpected end of file"),
            MetadataError::NotPeFile => write!(f, "not a PE file"),
            MetadataError::NotManagedAssembly => {
                write!(
                    f,
                    "no CLI header; this is a native binary, not a managed assembly"
                )
            }
            MetadataError::InvalidMetadata => write!(f, "invalid CLI metadata"),
            MetadataError::InvalidRva => write!(f, "RVA outside all sections"),
            MetadataError::UncompressedTableStream => {
                write!(f, "uncompressed (#-) metadata streams are not supported")
            }
            MetadataError::UnknownTable(table) => write!(f, "unknown metadata table 0x{table:02X}"),
            MetadataError::InvalidHeapOffset => write!(f, "invalid heap offset"),
            MetadataError::InvalidCompressedInteger => write!(f, "invalid compressed integer"),
            MetadataError::InvalidSignature => write!(f, "invalid signature blob"),
            MetadataError::InvalidTableIndex => write!(f, "table index out of range"),
        }
    }
}

impl std::error::Error for MetadataError {}
