//! Signature blobs: the binary encoding of types (ECMA-335 II.23.2).
//!
//! Field, method and property rows carry their types as `#Blob` entries in a compact
//! prefix encoding. This module decodes them into [`TypeSig`] trees. Custom modifiers
//! (`modreq`/`modopt`, used for things like `volatile.` and `in` parameters) are
//! consumed and dropped: they never change what a C# signature means to overload
//! resolution at the level MenSharp needs.

use crate::{error::MetadataError, reader::Reader};

/// A type as written in a signature. `TypeDefOrRef(index)` values are raw coded
/// tokens into this same assembly's tables; `crate::assembly` resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSig {
    Void,
    Boolean,
    Char,
    SByte,
    Byte,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Single,
    Double,
    String,
    Object,
    /// `nint` / `System.IntPtr`.
    IntPtr,
    UIntPtr,
    TypedReference,
    /// A class or value type, by raw TypeDef/TypeRef/TypeSpec token.
    Named {
        token: TypeToken,
        is_value_type: bool,
    },
    /// `List<int>`: the open definition plus its arguments.
    Generic {
        token: TypeToken,
        is_value_type: bool,
        arguments: Vec<TypeSig>,
    },
    /// `T[]`.
    SzArray(Box<TypeSig>),
    /// `T[,]` and friends. Sizes and bounds are dropped; C# never writes them.
    Array {
        element: Box<TypeSig>,
        rank: u32,
    },
    Pointer(Box<TypeSig>),
    /// `ref T` in a parameter or return position.
    ByRef(Box<TypeSig>),
    /// The declaring type's generic parameter (`!0`).
    TypeParameter(u32),
    /// The method's own generic parameter (`!!0`).
    MethodTypeParameter(u32),
    /// A function pointer. Udon has no use for the shape, so it is opaque.
    FunctionPointer,
}

/// A decoded TypeDefOrRefEncoded token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeToken {
    /// 0-based row in this assembly's TypeDef table.
    Definition(u32),
    /// 0-based row in this assembly's TypeRef table.
    Reference(u32),
    /// 0-based row in the TypeSpec table (a nested signature).
    Specification(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSig {
    pub has_this: bool,
    /// Generic parameter count declared by the method itself.
    pub generic_parameter_count: u32,
    pub return_type: TypeSig,
    pub parameters: Vec<TypeSig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertySig {
    pub property_type: TypeSig,
    /// Indexer parameters; empty for ordinary properties.
    pub parameters: Vec<TypeSig>,
}

// element type constants, ECMA-335 II.23.1.16
const ELEMENT_END: u8 = 0x00;
const ELEMENT_VOID: u8 = 0x01;
const ELEMENT_PTR: u8 = 0x0F;
const ELEMENT_BYREF: u8 = 0x10;
const ELEMENT_VALUETYPE: u8 = 0x11;
const ELEMENT_CLASS: u8 = 0x12;
const ELEMENT_VAR: u8 = 0x13;
const ELEMENT_ARRAY: u8 = 0x14;
const ELEMENT_GENERICINST: u8 = 0x15;
const ELEMENT_TYPEDBYREF: u8 = 0x16;
const ELEMENT_FNPTR: u8 = 0x1B;
const ELEMENT_SZARRAY: u8 = 0x1D;
const ELEMENT_MVAR: u8 = 0x1E;
const ELEMENT_CMOD_REQD: u8 = 0x1F;
const ELEMENT_CMOD_OPT: u8 = 0x20;
const ELEMENT_SENTINEL: u8 = 0x41;
const ELEMENT_PINNED: u8 = 0x45;

const CALLING_CONVENTION_MASK: u8 = 0x0F;
const CALLING_CONVENTION_GENERIC: u8 = 0x10;
const CALLING_CONVENTION_HAS_THIS: u8 = 0x20;
const CALLING_CONVENTION_VARARG: u8 = 0x05;
const SIGNATURE_FIELD: u8 = 0x06;
const SIGNATURE_PROPERTY: u8 = 0x08;

pub(crate) fn parse_field(blob: &[u8]) -> Result<TypeSig, MetadataError> {
    let mut reader = Reader::new(blob);
    let kind = reader.u8()?;
    if kind & CALLING_CONVENTION_MASK != SIGNATURE_FIELD {
        return Err(MetadataError::InvalidSignature);
    }
    parse_type(&mut reader)
}

pub(crate) fn parse_method(blob: &[u8]) -> Result<MethodSig, MetadataError> {
    let mut reader = Reader::new(blob);
    parse_method_at(&mut reader)
}

fn parse_method_at(reader: &mut Reader) -> Result<MethodSig, MetadataError> {
    let convention = reader.u8()?;

    let generic_parameter_count = if convention & CALLING_CONVENTION_GENERIC != 0 {
        reader.compressed_u32()?
    } else {
        0
    };
    let parameter_count = reader.compressed_u32()?;
    let return_type = parse_type_or_void(reader)?;

    let mut parameters = Vec::with_capacity(parameter_count as usize);
    for _ in 0..parameter_count {
        // vararg signatures split fixed and variable parts with a sentinel; C#
        // only meets this on legacy `__arglist`, so the tail is just read as-is
        if convention & CALLING_CONVENTION_MASK == CALLING_CONVENTION_VARARG {
            skip_custom_modifiers(reader)?;
            let mut peek = *reader;
            if peek.u8()? == ELEMENT_SENTINEL {
                *reader = peek;
            }
        }
        parameters.push(parse_type_or_void(reader)?);
    }

    Ok(MethodSig {
        has_this: convention & CALLING_CONVENTION_HAS_THIS != 0,
        generic_parameter_count,
        return_type,
        parameters,
    })
}

pub(crate) fn parse_property(blob: &[u8]) -> Result<PropertySig, MetadataError> {
    let mut reader = Reader::new(blob);

    let kind = reader.u8()?;
    if kind & CALLING_CONVENTION_MASK != SIGNATURE_PROPERTY {
        return Err(MetadataError::InvalidSignature);
    }

    let parameter_count = reader.compressed_u32()?;
    let property_type = parse_type(&mut reader)?;

    let mut parameters = Vec::with_capacity(parameter_count as usize);
    for _ in 0..parameter_count {
        parameters.push(parse_type(&mut reader)?);
    }

    Ok(PropertySig {
        property_type,
        parameters,
    })
}

/// A TypeSpec blob is a bare type.
pub(crate) fn parse_type_spec(blob: &[u8]) -> Result<TypeSig, MetadataError> {
    let mut reader = Reader::new(blob);
    parse_type(&mut reader)
}

fn parse_type_or_void(reader: &mut Reader) -> Result<TypeSig, MetadataError> {
    skip_custom_modifiers(reader)?;

    let mut peek = *reader;
    if peek.u8()? == ELEMENT_VOID {
        *reader = peek;
        return Ok(TypeSig::Void);
    }
    parse_type(reader)
}

fn parse_type(reader: &mut Reader) -> Result<TypeSig, MetadataError> {
    skip_custom_modifiers(reader)?;

    let element = reader.u8()?;
    let sig = match element {
        ELEMENT_VOID => TypeSig::Void,
        0x02 => TypeSig::Boolean,
        0x03 => TypeSig::Char,
        0x04 => TypeSig::SByte,
        0x05 => TypeSig::Byte,
        0x06 => TypeSig::Int16,
        0x07 => TypeSig::UInt16,
        0x08 => TypeSig::Int32,
        0x09 => TypeSig::UInt32,
        0x0A => TypeSig::Int64,
        0x0B => TypeSig::UInt64,
        0x0C => TypeSig::Single,
        0x0D => TypeSig::Double,
        0x0E => TypeSig::String,
        0x1C => TypeSig::Object,
        0x18 => TypeSig::IntPtr,
        0x19 => TypeSig::UIntPtr,
        ELEMENT_TYPEDBYREF => TypeSig::TypedReference,
        ELEMENT_PTR => TypeSig::Pointer(Box::new(parse_type_or_void(reader)?)),
        ELEMENT_BYREF => TypeSig::ByRef(Box::new(parse_type(reader)?)),
        ELEMENT_VALUETYPE | ELEMENT_CLASS => TypeSig::Named {
            token: type_token(reader.compressed_u32()?)?,
            is_value_type: element == ELEMENT_VALUETYPE,
        },
        ELEMENT_VAR => TypeSig::TypeParameter(reader.compressed_u32()?),
        ELEMENT_MVAR => TypeSig::MethodTypeParameter(reader.compressed_u32()?),
        ELEMENT_SZARRAY => TypeSig::SzArray(Box::new(parse_type(reader)?)),
        ELEMENT_ARRAY => {
            let element = parse_type(reader)?;
            let rank = reader.compressed_u32()?;
            let sizes = reader.compressed_u32()?;
            for _ in 0..sizes {
                reader.compressed_u32()?;
            }
            let bounds = reader.compressed_u32()?;
            for _ in 0..bounds {
                reader.compressed_u32()?;
            }
            TypeSig::Array {
                element: Box::new(element),
                rank,
            }
        }
        ELEMENT_GENERICINST => {
            let kind = reader.u8()?;
            if kind != ELEMENT_CLASS && kind != ELEMENT_VALUETYPE {
                return Err(MetadataError::InvalidSignature);
            }
            let token = type_token(reader.compressed_u32()?)?;
            let count = reader.compressed_u32()?;
            let mut arguments = Vec::with_capacity(count as usize);
            for _ in 0..count {
                arguments.push(parse_type(reader)?);
            }
            TypeSig::Generic {
                token,
                is_value_type: kind == ELEMENT_VALUETYPE,
                arguments,
            }
        }
        ELEMENT_FNPTR => {
            // the pointed-to method signature has to be consumed to move past it
            parse_method_at(reader)?;
            TypeSig::FunctionPointer
        }
        ELEMENT_PINNED => parse_type(reader)?,
        ELEMENT_END => return Err(MetadataError::InvalidSignature),
        _ => return Err(MetadataError::InvalidSignature),
    };

    Ok(sig)
}

fn skip_custom_modifiers(reader: &mut Reader) -> Result<(), MetadataError> {
    loop {
        let mut peek = *reader;
        let Ok(element) = peek.u8() else {
            return Ok(());
        };
        if element != ELEMENT_CMOD_REQD && element != ELEMENT_CMOD_OPT {
            return Ok(());
        }
        peek.compressed_u32()?; // the modifier type token
        *reader = peek;
    }
}

/// TypeDefOrRefEncoded: tag in the low two bits, 1-based row above.
fn type_token(encoded: u32) -> Result<TypeToken, MetadataError> {
    let row = encoded >> 2;
    if row == 0 {
        return Err(MetadataError::InvalidTableIndex);
    }
    let row = row - 1;

    Ok(match encoded & 0x3 {
        0 => TypeToken::Definition(row),
        1 => TypeToken::Reference(row),
        2 => TypeToken::Specification(row),
        _ => return Err(MetadataError::InvalidSignature),
    })
}
