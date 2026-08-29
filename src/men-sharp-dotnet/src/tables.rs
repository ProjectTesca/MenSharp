//! The `#~` stream: raw metadata table rows.
//!
//! The stream is a packed array-of-arrays with no offsets: to reach table N one must
//! know the exact byte width of every row of every table before it, and widths vary
//! per file — an index is 2 bytes until the thing it indexes outgrows 16 bits, then
//! 4. So this module knows the column layout of *all* tables (ECMA-335 II.22), even
//! the many it does not decode, purely to size them; rows are decoded only for the
//! tables the compiler needs.
//!
//! Everything here is raw: names are still `#Strings` offsets, signatures still
//! `#Blob` offsets, cross-table references still 1-based row numbers. Turning that
//! into something usable is `crate::assembly`'s job.

use crate::{error::MetadataError, reader::Reader};

// table numbers, ECMA-335 II.22
pub(crate) const MODULE: usize = 0x00;
pub(crate) const TYPE_REF: usize = 0x01;
pub(crate) const TYPE_DEF: usize = 0x02;
pub(crate) const FIELD: usize = 0x04;
pub(crate) const METHOD_DEF: usize = 0x06;
pub(crate) const PARAM: usize = 0x08;
pub(crate) const INTERFACE_IMPL: usize = 0x09;
pub(crate) const MEMBER_REF: usize = 0x0A;
pub(crate) const CONSTANT: usize = 0x0B;
pub(crate) const CUSTOM_ATTRIBUTE: usize = 0x0C;
pub(crate) const FIELD_MARSHAL: usize = 0x0D;
pub(crate) const DECL_SECURITY: usize = 0x0E;
pub(crate) const CLASS_LAYOUT: usize = 0x0F;
pub(crate) const FIELD_LAYOUT: usize = 0x10;
pub(crate) const STANDALONE_SIG: usize = 0x11;
pub(crate) const EVENT_MAP: usize = 0x12;
pub(crate) const EVENT: usize = 0x14;
pub(crate) const PROPERTY_MAP: usize = 0x15;
pub(crate) const PROPERTY: usize = 0x17;
pub(crate) const METHOD_SEMANTICS: usize = 0x18;
pub(crate) const METHOD_IMPL: usize = 0x19;
pub(crate) const MODULE_REF: usize = 0x1A;
pub(crate) const TYPE_SPEC: usize = 0x1B;
pub(crate) const IMPL_MAP: usize = 0x1C;
pub(crate) const FIELD_RVA: usize = 0x1D;
pub(crate) const ASSEMBLY: usize = 0x20;
pub(crate) const ASSEMBLY_REF: usize = 0x23;
pub(crate) const FILE: usize = 0x26;
pub(crate) const EXPORTED_TYPE: usize = 0x27;
pub(crate) const MANIFEST_RESOURCE: usize = 0x28;
pub(crate) const NESTED_CLASS: usize = 0x29;
pub(crate) const GENERIC_PARAM: usize = 0x2A;
pub(crate) const METHOD_SPEC: usize = 0x2B;
pub(crate) const GENERIC_PARAM_CONSTRAINT: usize = 0x2C;

const TABLE_COUNT: usize = 64;

// coded index groups, ECMA-335 II.24.2.6: a tag in the low bits picks the table,
// the width depends on the largest member table
pub(crate) const TYPE_DEF_OR_REF: &[usize] = &[TYPE_DEF, TYPE_REF, TYPE_SPEC];
const HAS_CONSTANT: &[usize] = &[FIELD, PARAM, PROPERTY];
const HAS_CUSTOM_ATTRIBUTE: &[usize] = &[
    METHOD_DEF,
    FIELD,
    TYPE_REF,
    TYPE_DEF,
    PARAM,
    INTERFACE_IMPL,
    MEMBER_REF,
    MODULE,
    DECL_SECURITY,
    PROPERTY,
    EVENT,
    STANDALONE_SIG,
    MODULE_REF,
    TYPE_SPEC,
    ASSEMBLY,
    ASSEMBLY_REF,
    FILE,
    EXPORTED_TYPE,
    MANIFEST_RESOURCE,
    GENERIC_PARAM,
    GENERIC_PARAM_CONSTRAINT,
    METHOD_SPEC,
];
const HAS_FIELD_MARSHAL: &[usize] = &[FIELD, PARAM];
const HAS_DECL_SECURITY: &[usize] = &[TYPE_DEF, METHOD_DEF, ASSEMBLY];
const MEMBER_REF_PARENT: &[usize] = &[TYPE_DEF, TYPE_REF, MODULE_REF, METHOD_DEF, TYPE_SPEC];
const HAS_SEMANTICS: &[usize] = &[EVENT, PROPERTY];
const METHOD_DEF_OR_REF: &[usize] = &[METHOD_DEF, MEMBER_REF];
const MEMBER_FORWARDED: &[usize] = &[FIELD, METHOD_DEF];
const IMPLEMENTATION: &[usize] = &[FILE, ASSEMBLY_REF, EXPORTED_TYPE];
/// A placeholder for tags the spec reserves as "not used": it still occupies a tag
/// slot (the tag bit count depends on slot count, not on how many are meaningful)
/// but no row ever carries it. Table 0x3F does not exist, so its row count is 0 and
/// it never influences a width computation.
const UNUSED: usize = 0x3F;
const CUSTOM_ATTRIBUTE_TYPE: &[usize] = &[UNUSED, UNUSED, METHOD_DEF, MEMBER_REF, UNUSED];
pub(crate) const RESOLUTION_SCOPE: &[usize] = &[MODULE, MODULE_REF, ASSEMBLY_REF, TYPE_REF];
const TYPE_OR_METHOD_DEF: &[usize] = &[TYPE_DEF, METHOD_DEF];

fn tag_bits(tables: &[usize]) -> u32 {
    usize::BITS - (tables.len() - 1).leading_zeros()
}

/// A decoded coded index: which table, which 1-based row (0 = null).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodedIndex {
    pub table: usize,
    pub row: u32,
}

/// Per-file layout facts: row counts and index widths.
pub(crate) struct Layout {
    pub row_counts: [u32; TABLE_COUNT],
    wide_strings: bool,
    wide_guids: bool,
    wide_blobs: bool,
}

impl Layout {
    fn string_size(&self) -> usize {
        if self.wide_strings { 4 } else { 2 }
    }

    fn guid_size(&self) -> usize {
        if self.wide_guids { 4 } else { 2 }
    }

    fn blob_size(&self) -> usize {
        if self.wide_blobs { 4 } else { 2 }
    }

    fn index_wide(&self, table: usize) -> bool {
        self.row_counts[table] > 0xFFFF
    }

    fn index_size(&self, table: usize) -> usize {
        if self.index_wide(table) { 4 } else { 2 }
    }

    fn coded_wide(&self, tables: &[usize]) -> bool {
        let max_rows = tables
            .iter()
            .map(|&table| self.row_counts[table])
            .max()
            .unwrap();
        max_rows >= 1 << (16 - tag_bits(tables))
    }

    fn coded_size(&self, tables: &[usize]) -> usize {
        if self.coded_wide(tables) { 4 } else { 2 }
    }

    /// Byte width of one row of `table` — required for every table that exists in
    /// the file, decoded or not, because tables are stored back to back.
    fn row_size(&self, table: usize) -> Result<usize, MetadataError> {
        let size = match table {
            MODULE => 2 + self.string_size() + 3 * self.guid_size(),
            TYPE_REF => self.coded_size(RESOLUTION_SCOPE) + 2 * self.string_size(),
            TYPE_DEF => {
                4 + 2 * self.string_size()
                    + self.coded_size(TYPE_DEF_OR_REF)
                    + self.index_size(FIELD)
                    + self.index_size(METHOD_DEF)
            }
            0x03 => self.index_size(FIELD), // FieldPtr
            FIELD => 2 + self.string_size() + self.blob_size(),
            0x05 => self.index_size(METHOD_DEF), // MethodPtr
            METHOD_DEF => 8 + self.string_size() + self.blob_size() + self.index_size(PARAM),
            0x07 => self.index_size(PARAM), // ParamPtr
            PARAM => 4 + self.string_size(),
            INTERFACE_IMPL => self.index_size(TYPE_DEF) + self.coded_size(TYPE_DEF_OR_REF),
            MEMBER_REF => {
                self.coded_size(MEMBER_REF_PARENT) + self.string_size() + self.blob_size()
            }
            CONSTANT => 2 + self.coded_size(HAS_CONSTANT) + self.blob_size(),
            CUSTOM_ATTRIBUTE => {
                self.coded_size(HAS_CUSTOM_ATTRIBUTE)
                    + self.coded_size(CUSTOM_ATTRIBUTE_TYPE)
                    + self.blob_size()
            }
            FIELD_MARSHAL => self.coded_size(HAS_FIELD_MARSHAL) + self.blob_size(),
            DECL_SECURITY => 2 + self.coded_size(HAS_DECL_SECURITY) + self.blob_size(),
            CLASS_LAYOUT => 6 + self.index_size(TYPE_DEF),
            FIELD_LAYOUT => 4 + self.index_size(FIELD),
            STANDALONE_SIG => self.blob_size(),
            EVENT_MAP => self.index_size(TYPE_DEF) + self.index_size(EVENT),
            0x13 => self.index_size(EVENT), // EventPtr
            EVENT => 2 + self.string_size() + self.coded_size(TYPE_DEF_OR_REF),
            PROPERTY_MAP => self.index_size(TYPE_DEF) + self.index_size(PROPERTY),
            0x16 => self.index_size(PROPERTY), // PropertyPtr
            PROPERTY => 2 + self.string_size() + self.blob_size(),
            METHOD_SEMANTICS => 2 + self.index_size(METHOD_DEF) + self.coded_size(HAS_SEMANTICS),
            METHOD_IMPL => self.index_size(TYPE_DEF) + 2 * self.coded_size(METHOD_DEF_OR_REF),
            MODULE_REF => self.string_size(),
            TYPE_SPEC => self.blob_size(),
            IMPL_MAP => {
                2 + self.coded_size(MEMBER_FORWARDED)
                    + self.string_size()
                    + self.index_size(MODULE_REF)
            }
            FIELD_RVA => 4 + self.index_size(FIELD),
            0x1E => 8, // ENCLog
            0x1F => 4, // ENCMap
            ASSEMBLY => 16 + self.blob_size() + 2 * self.string_size(),
            0x21 => 4,  // AssemblyProcessor
            0x22 => 12, // AssemblyOS
            ASSEMBLY_REF => 12 + 2 * self.blob_size() + 2 * self.string_size(),
            0x24 => 4 + self.index_size(ASSEMBLY_REF), // AssemblyRefProcessor
            0x25 => 12 + self.index_size(ASSEMBLY_REF), // AssemblyRefOS
            FILE => 4 + self.string_size() + self.blob_size(),
            EXPORTED_TYPE => 8 + 2 * self.string_size() + self.coded_size(IMPLEMENTATION),
            MANIFEST_RESOURCE => 8 + self.string_size() + self.coded_size(IMPLEMENTATION),
            NESTED_CLASS => 2 * self.index_size(TYPE_DEF),
            GENERIC_PARAM => 4 + self.coded_size(TYPE_OR_METHOD_DEF) + self.string_size(),
            METHOD_SPEC => self.coded_size(METHOD_DEF_OR_REF) + self.blob_size(),
            GENERIC_PARAM_CONSTRAINT => {
                self.index_size(GENERIC_PARAM) + self.coded_size(TYPE_DEF_OR_REF)
            }
            _ => return Err(MetadataError::UnknownTable(table as u8)),
        };
        Ok(size)
    }
}

// ---------------------------------------------------------------------------
// raw rows for the tables the compiler decodes
// ---------------------------------------------------------------------------

pub(crate) struct TypeRefRow {
    pub resolution_scope: CodedIndex,
    pub name: u32,
    pub namespace: u32,
}

pub(crate) struct TypeDefRow {
    pub flags: u32,
    pub name: u32,
    pub namespace: u32,
    pub extends: CodedIndex,
    pub field_list: u32,
    pub method_list: u32,
}

pub(crate) struct FieldRow {
    pub flags: u16,
    pub name: u32,
    pub signature: u32,
}

pub(crate) struct MethodDefRow {
    pub flags: u16,
    pub name: u32,
    pub signature: u32,
    pub param_list: u32,
}

pub(crate) struct ParamRow {
    pub flags: u16,
    pub sequence: u16,
    pub name: u32,
}

pub(crate) struct InterfaceImplRow {
    pub class: u32,
    pub interface: CodedIndex,
}

pub(crate) struct MemberRefRow {
    pub class: CodedIndex,
}

pub(crate) struct CustomAttributeRow {
    pub parent: CodedIndex,
    pub constructor: CodedIndex,
}

pub(crate) struct ConstantRow {
    pub element_type: u8,
    pub parent: CodedIndex,
    pub value: u32,
}

pub(crate) struct EventMapRow {
    pub parent: u32,
    pub event_list: u32,
}

pub(crate) struct EventRow {
    pub name: u32,
    pub event_type: CodedIndex,
}

pub(crate) struct PropertyMapRow {
    pub parent: u32,
    pub property_list: u32,
}

pub(crate) struct PropertyRow {
    pub name: u32,
    pub signature: u32,
}

pub(crate) struct MethodSemanticsRow {
    pub semantics: u16,
    pub method: u32,
    pub association: CodedIndex,
}

pub(crate) struct AssemblyRow {
    pub version: (u16, u16, u16, u16),
    pub name: u32,
}

pub(crate) struct AssemblyRefRow {
    pub version: (u16, u16, u16, u16),
    pub name: u32,
}

pub(crate) struct NestedClassRow {
    pub nested: u32,
    pub enclosing: u32,
}

pub(crate) struct GenericParamRow {
    pub flags: u16,
    pub owner: CodedIndex,
    pub name: u32,
}

pub(crate) struct GenericParamConstraintRow {
    pub owner: u32,
    pub constraint: CodedIndex,
}

pub(crate) struct ExportedTypeRow {
    pub name: u32,
    pub namespace: u32,
    pub implementation: CodedIndex,
}

/// Every decoded table, still in raw (offset/row-number) form.
#[derive(Default)]
pub(crate) struct RawTables {
    pub type_refs: Vec<TypeRefRow>,
    pub type_defs: Vec<TypeDefRow>,
    pub fields: Vec<FieldRow>,
    pub methods: Vec<MethodDefRow>,
    pub params: Vec<ParamRow>,
    pub interface_impls: Vec<InterfaceImplRow>,
    pub member_refs: Vec<MemberRefRow>,
    pub custom_attributes: Vec<CustomAttributeRow>,
    pub constants: Vec<ConstantRow>,
    pub event_maps: Vec<EventMapRow>,
    pub events: Vec<EventRow>,
    pub property_maps: Vec<PropertyMapRow>,
    pub properties: Vec<PropertyRow>,
    pub method_semantics: Vec<MethodSemanticsRow>,
    pub module_refs: Vec<u32>,
    pub type_specs: Vec<u32>,
    pub assembly: Option<AssemblyRow>,
    pub assembly_refs: Vec<AssemblyRefRow>,
    pub nested_classes: Vec<NestedClassRow>,
    pub generic_params: Vec<GenericParamRow>,
    pub generic_param_constraints: Vec<GenericParamConstraintRow>,
    pub exported_types: Vec<ExportedTypeRow>,
}

struct TableReader<'stream> {
    reader: Reader<'stream>,
    layout: Layout,
}

impl TableReader<'_> {
    fn string(&mut self) -> Result<u32, MetadataError> {
        self.reader.index(self.layout.wide_strings)
    }

    fn blob(&mut self) -> Result<u32, MetadataError> {
        self.reader.index(self.layout.wide_blobs)
    }

    fn index(&mut self, table: usize) -> Result<u32, MetadataError> {
        self.reader.index(self.layout.index_wide(table))
    }

    fn coded(&mut self, tables: &[usize]) -> Result<CodedIndex, MetadataError> {
        let value = self.reader.index(self.layout.coded_wide(tables))?;
        let bits = tag_bits(tables);
        let tag = (value & ((1 << bits) - 1)) as usize;

        Ok(CodedIndex {
            table: *tables.get(tag).ok_or(MetadataError::InvalidTableIndex)?,
            row: value >> bits,
        })
    }
}

pub(crate) fn read_tables(stream: &[u8]) -> Result<RawTables, MetadataError> {
    let mut reader = Reader::new(stream);

    reader.skip(6)?; // reserved, major, minor
    let heap_sizes = reader.u8()?;
    reader.skip(1)?; // reserved
    let valid = reader.u64()?;
    reader.skip(8)?; // sorted

    let mut layout = Layout {
        row_counts: [0; TABLE_COUNT],
        wide_strings: heap_sizes & 0x1 != 0,
        wide_guids: heap_sizes & 0x2 != 0,
        wide_blobs: heap_sizes & 0x4 != 0,
    };
    for (table, count) in layout.row_counts.iter_mut().enumerate() {
        if valid & (1 << table) != 0 {
            *count = reader.u32()?;
        }
    }

    // the *Ptr indirection tables only exist in unoptimized edit-and-continue
    // output; if one were present the list ranges below would be indirected and
    // silently wrong, so refuse instead
    for pointer_table in [0x03, 0x05, 0x07, 0x13, 0x16] {
        if layout.row_counts[pointer_table] > 0 {
            return Err(MetadataError::UnknownTable(pointer_table as u8));
        }
    }

    let mut tables = TableReader { reader, layout };
    let mut raw = RawTables::default();

    for table in 0..TABLE_COUNT {
        let rows = tables.layout.row_counts[table];
        if rows == 0 {
            continue;
        }

        match table {
            TYPE_REF => {
                raw.type_refs = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(TypeRefRow {
                            resolution_scope: t.coded(RESOLUTION_SCOPE)?,
                            name: t.string()?,
                            namespace: t.string()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            TYPE_DEF => {
                raw.type_defs = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(TypeDefRow {
                            flags: t.reader.u32()?,
                            name: t.string()?,
                            namespace: t.string()?,
                            extends: t.coded(TYPE_DEF_OR_REF)?,
                            field_list: t.index(FIELD)?,
                            method_list: t.index(METHOD_DEF)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            FIELD => {
                raw.fields = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(FieldRow {
                            flags: t.reader.u16()?,
                            name: t.string()?,
                            signature: t.blob()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            METHOD_DEF => {
                raw.methods = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        t.reader.skip(6)?; // rva, impl flags
                        Ok(MethodDefRow {
                            flags: t.reader.u16()?,
                            name: t.string()?,
                            signature: t.blob()?,
                            param_list: t.index(PARAM)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            PARAM => {
                raw.params = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(ParamRow {
                            flags: t.reader.u16()?,
                            sequence: t.reader.u16()?,
                            name: t.string()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            INTERFACE_IMPL => {
                raw.interface_impls = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(InterfaceImplRow {
                            class: t.index(TYPE_DEF)?,
                            interface: t.coded(TYPE_DEF_OR_REF)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            MEMBER_REF => {
                raw.member_refs = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        let class = t.coded(MEMBER_REF_PARENT)?;
                        t.string()?; // name (always .ctor here)
                        t.blob()?; // signature
                        Ok(MemberRefRow { class })
                    },
                    &mut tables,
                )?;
            }
            CUSTOM_ATTRIBUTE => {
                raw.custom_attributes = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        let parent = t.coded(HAS_CUSTOM_ATTRIBUTE)?;
                        let constructor = t.coded(CUSTOM_ATTRIBUTE_TYPE)?;
                        t.blob()?; // constructor arguments
                        Ok(CustomAttributeRow {
                            parent,
                            constructor,
                        })
                    },
                    &mut tables,
                )?;
            }
            CONSTANT => {
                raw.constants = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        let element_type = t.reader.u8()?;
                        t.reader.skip(1)?; // padding
                        Ok(ConstantRow {
                            element_type,
                            parent: t.coded(HAS_CONSTANT)?,
                            value: t.blob()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            EVENT_MAP => {
                raw.event_maps = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(EventMapRow {
                            parent: t.index(TYPE_DEF)?,
                            event_list: t.index(EVENT)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            EVENT => {
                raw.events = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        t.reader.skip(2)?; // flags
                        Ok(EventRow {
                            name: t.string()?,
                            event_type: t.coded(TYPE_DEF_OR_REF)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            PROPERTY_MAP => {
                raw.property_maps = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(PropertyMapRow {
                            parent: t.index(TYPE_DEF)?,
                            property_list: t.index(PROPERTY)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            PROPERTY => {
                raw.properties = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        t.reader.skip(2)?; // flags
                        Ok(PropertyRow {
                            name: t.string()?,
                            signature: t.blob()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            METHOD_SEMANTICS => {
                raw.method_semantics = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(MethodSemanticsRow {
                            semantics: t.reader.u16()?,
                            method: t.index(METHOD_DEF)?,
                            association: t.coded(HAS_SEMANTICS)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            MODULE_REF => {
                raw.module_refs = read_rows(rows, |t: &mut TableReader| t.string(), &mut tables)?;
            }
            TYPE_SPEC => {
                raw.type_specs = read_rows(rows, |t: &mut TableReader| t.blob(), &mut tables)?;
            }
            ASSEMBLY => {
                let mut assembly = None;
                for _ in 0..rows {
                    tables.reader.skip(4)?; // hash algorithm
                    let version = (
                        tables.reader.u16()?,
                        tables.reader.u16()?,
                        tables.reader.u16()?,
                        tables.reader.u16()?,
                    );
                    tables.reader.skip(4)?; // flags
                    tables.blob()?; // public key
                    let name = tables.string()?;
                    tables.string()?; // culture
                    assembly.get_or_insert(AssemblyRow { version, name });
                }
                raw.assembly = assembly;
            }
            ASSEMBLY_REF => {
                raw.assembly_refs = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        let version = (
                            t.reader.u16()?,
                            t.reader.u16()?,
                            t.reader.u16()?,
                            t.reader.u16()?,
                        );
                        t.reader.skip(4)?; // flags
                        t.blob()?; // public key or token
                        let name = t.string()?;
                        t.string()?; // culture
                        t.blob()?; // hash
                        Ok(AssemblyRefRow { version, name })
                    },
                    &mut tables,
                )?;
            }
            EXPORTED_TYPE => {
                raw.exported_types = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        t.reader.skip(8)?; // flags, TypeDefId hint
                        Ok(ExportedTypeRow {
                            name: t.string()?,
                            namespace: t.string()?,
                            implementation: t.coded(IMPLEMENTATION)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            NESTED_CLASS => {
                raw.nested_classes = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(NestedClassRow {
                            nested: t.index(TYPE_DEF)?,
                            enclosing: t.index(TYPE_DEF)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            GENERIC_PARAM => {
                raw.generic_params = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        // rows are sorted by (owner, number), so order alone is enough
                        t.reader.skip(2)?; // number
                        Ok(GenericParamRow {
                            flags: t.reader.u16()?,
                            owner: t.coded(TYPE_OR_METHOD_DEF)?,
                            name: t.string()?,
                        })
                    },
                    &mut tables,
                )?;
            }
            GENERIC_PARAM_CONSTRAINT => {
                raw.generic_param_constraints = read_rows(
                    rows,
                    |t: &mut TableReader| {
                        Ok(GenericParamConstraintRow {
                            owner: t.index(GENERIC_PARAM)?,
                            constraint: t.coded(TYPE_DEF_OR_REF)?,
                        })
                    },
                    &mut tables,
                )?;
            }
            _ => {
                // sized but not decoded
                let size = tables.layout.row_size(table)?;
                tables.reader.skip(size * rows as usize)?;
            }
        }
    }

    Ok(raw)
}

fn read_rows<T>(
    rows: u32,
    mut read: impl FnMut(&mut TableReader) -> Result<T, MetadataError>,
    tables: &mut TableReader,
) -> Result<Vec<T>, MetadataError> {
    let mut result = Vec::with_capacity(rows as usize);
    for _ in 0..rows {
        result.push(read(tables)?);
    }
    Ok(result)
}
