//! The public model: from raw table rows to a queryable assembly.
//!
//! Row-number plumbing (field ranges, `MethodSemantics` associations, `NestedClass`
//! links, constant parents) is resolved here once, so consumers see types that
//! simply *have* their fields, methods, properties and events.
//!
//! Strings borrow the assembly's bytes (`'data`); the caller keeps the file's
//! contents alive for as long as the model is used, same as the parser crate's
//! relationship between tree and source.

use std::collections::HashMap;

use crate::{
    error::MetadataError,
    pe::{heap_blob, heap_string, metadata_streams},
    signature::{self, MethodSig, PropertySig, TypeSig, TypeToken},
    tables::{self, CodedIndex, RawTables},
};

/// One parsed managed assembly.
#[derive(Debug)]
pub struct DotNetAssembly<'data> {
    pub name: &'data str,
    pub version: (u16, u16, u16, u16),
    /// Assemblies this one references, by name.
    pub references: Vec<AssemblyReference<'data>>,
    pub types: Vec<TypeDefinition<'data>>,
    /// Type forwarders: this assembly re-exports these names from another assembly.
    /// Facade assemblies like `System.Runtime` are almost nothing but these.
    pub forwarders: Vec<TypeForwarder<'data>>,
    type_refs: Vec<TypeReference<'data>>,
    type_specs: Vec<TypeSig>,
    top_level: HashMap<(&'data str, &'data str), u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct AssemblyReference<'data> {
    pub name: &'data str,
    pub version: (u16, u16, u16, u16),
}

/// A mention of a type that lives elsewhere.
#[derive(Debug, Clone)]
pub struct TypeReference<'data> {
    pub scope: TypeRefScope<'data>,
    pub namespace: &'data str,
    pub name: &'data str,
}

#[derive(Debug, Clone, Copy)]
pub enum TypeRefScope<'data> {
    /// Defined in the named referenced assembly.
    Assembly(&'data str),
    /// Defined in this module.
    Module,
    /// Nested inside another referenced type (index into `type_refs`).
    Nested(u32),
}

#[derive(Debug, Clone, Copy)]
pub struct TypeForwarder<'data> {
    pub namespace: &'data str,
    pub name: &'data str,
    pub assembly: &'data str,
}

#[derive(Debug)]
pub struct TypeDefinition<'data> {
    pub namespace: &'data str,
    /// The metadata name, generic arity included: `List`1`.
    pub name: &'data str,
    pub flags: u32,
    pub is_value_type: bool,
    pub is_enum: bool,
    pub extends: Option<TypeSig>,
    pub interfaces: Vec<TypeSig>,
    pub generic_parameters: Vec<GenericParameter<'data>>,
    pub fields: Vec<FieldDefinition<'data>>,
    pub methods: Vec<MethodDefinition<'data>>,
    pub properties: Vec<PropertyDefinition<'data>>,
    pub events: Vec<EventDefinition<'data>>,
    /// Indices into [`DotNetAssembly::types`].
    pub nested_types: Vec<u32>,
    pub enclosing_type: Option<u32>,
}

impl<'data> TypeDefinition<'data> {
    pub fn is_interface(&self) -> bool {
        self.flags & 0x20 != 0
    }

    pub fn is_abstract(&self) -> bool {
        self.flags & 0x80 != 0
    }

    pub fn is_sealed(&self) -> bool {
        self.flags & 0x100 != 0
    }

    /// `public` at top level, `nested public` inside another type. For nested types
    /// the enclosing chain must be visible too; see
    /// [`DotNetAssembly::is_externally_visible`].
    pub fn is_public(&self) -> bool {
        matches!(self.flags & 0x7, 0x1 | 0x2)
    }

    /// The C# name and arity: `List`1` becomes `("List", 1)`.
    pub fn name_and_arity(&self) -> (&'data str, u32) {
        split_arity(self.name)
    }
}

#[derive(Debug)]
pub struct GenericParameter<'data> {
    pub name: &'data str,
    pub variance: Variance,
    pub constraints: Vec<TypeSig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variance {
    Invariant,
    /// `out T`.
    Covariant,
    /// `in T`.
    Contravariant,
}

#[derive(Debug)]
pub struct FieldDefinition<'data> {
    pub name: &'data str,
    pub flags: u16,
    pub field_type: TypeSig,
    /// The compile-time value of a `const` field or an enum member.
    pub constant: Option<Constant>,
}

impl FieldDefinition<'_> {
    pub fn is_public(&self) -> bool {
        self.flags & 0x7 == 0x6
    }

    pub fn is_static(&self) -> bool {
        self.flags & 0x10 != 0
    }

    /// `const` (and enum members).
    pub fn is_literal(&self) -> bool {
        self.flags & 0x40 != 0
    }

    /// `readonly`.
    pub fn is_init_only(&self) -> bool {
        self.flags & 0x20 != 0
    }
}

#[derive(Debug)]
pub struct MethodDefinition<'data> {
    pub name: &'data str,
    pub flags: u16,
    pub signature: MethodSig,
    /// Parameter names and flags, aligned with `signature.parameters`.
    pub parameters: Vec<ParameterDefinition<'data>>,
}

impl MethodDefinition<'_> {
    pub fn is_public(&self) -> bool {
        self.flags & 0x7 == 0x6
    }

    /// `protected`, including `protected internal`.
    pub fn is_family(&self) -> bool {
        matches!(self.flags & 0x7, 0x4 | 0x5)
    }

    pub fn is_static(&self) -> bool {
        self.flags & 0x10 != 0
    }

    pub fn is_virtual(&self) -> bool {
        self.flags & 0x40 != 0
    }

    pub fn is_abstract(&self) -> bool {
        self.flags & 0x400 != 0
    }

    /// Accessors, operators, constructors: methods C# does not call by name.
    pub fn is_special_name(&self) -> bool {
        self.flags & 0x800 != 0
    }
}

#[derive(Debug, Default, Clone)]
pub struct ParameterDefinition<'data> {
    pub name: &'data str,
    pub flags: u16,
    /// The default value of an optional parameter.
    pub constant: Option<Constant>,
}

impl ParameterDefinition<'_> {
    pub fn is_out(&self) -> bool {
        self.flags & 0x2 != 0
    }

    pub fn is_optional(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

#[derive(Debug)]
pub struct PropertyDefinition<'data> {
    pub name: &'data str,
    pub signature: PropertySig,
    /// Indices into the declaring type's `methods`.
    pub getter: Option<u32>,
    pub setter: Option<u32>,
}

#[derive(Debug)]
pub struct EventDefinition<'data> {
    pub name: &'data str,
    /// The delegate type; token form, like [`TypeSig::Named`].
    pub event_type: Option<TypeSig>,
    pub add: Option<u32>,
    pub remove: Option<u32>,
}

/// A compile-time constant from the `Constant` table.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Boolean(bool),
    Char(u16),
    SByte(i8),
    Byte(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Single(f32),
    Double(f64),
    String(String),
    Null,
}

impl<'data> DotNetAssembly<'data> {
    /// Parses a managed assembly from its file contents.
    pub fn parse(data: &'data [u8]) -> Result<Self, MetadataError> {
        let streams = metadata_streams(data)?;
        let raw = tables::read_tables(streams.tables)?;
        build(&raw, streams.strings, streams.blobs)
    }

    /// A top-level type by namespace and metadata name (`"System"`, `"List`1"`).
    pub fn find_type(&self, namespace: &str, name: &str) -> Option<u32> {
        self.top_level.get(&(namespace, name)).copied()
    }

    pub fn type_definition(&self, index: u32) -> &TypeDefinition<'data> {
        &self.types[index as usize]
    }

    pub fn type_reference(&self, index: u32) -> &TypeReference<'data> {
        &self.type_refs[index as usize]
    }

    /// The already-parsed signature of a TypeSpec token.
    pub fn type_spec(&self, index: u32) -> &TypeSig {
        &self.type_specs[index as usize]
    }

    /// Whether other assemblies can see this type: public, and if nested, nested
    /// public all the way out.
    pub fn is_externally_visible(&self, index: u32) -> bool {
        let definition = self.type_definition(index);
        if !definition.is_public() {
            return false;
        }
        match definition.enclosing_type {
            Some(enclosing) => self.is_externally_visible(enclosing),
            None => true,
        }
    }

    /// The (namespace, name) path a token points at, when it can be answered
    /// without another assembly: the nesting chain, innermost last.
    pub fn token_path(&self, token: TypeToken) -> Option<TokenPath<'data>> {
        match token {
            TypeToken::Definition(index) => {
                let mut names = Vec::new();
                let mut current = Some(index);
                let mut namespace = "";
                while let Some(index) = current {
                    let definition = self.type_definition(index);
                    names.push(definition.name);
                    namespace = definition.namespace;
                    current = definition.enclosing_type;
                }
                names.reverse();
                Some(TokenPath {
                    assembly: None,
                    namespace,
                    names,
                })
            }
            TypeToken::Reference(index) => {
                let mut names = Vec::new();
                let mut current = index;
                loop {
                    let reference = self.type_reference(current);
                    names.push(reference.name);
                    match reference.scope {
                        TypeRefScope::Nested(enclosing) => current = enclosing,
                        TypeRefScope::Module => {
                            names.reverse();
                            return Some(TokenPath {
                                assembly: None,
                                namespace: reference.namespace,
                                names,
                            });
                        }
                        TypeRefScope::Assembly(assembly) => {
                            names.reverse();
                            return Some(TokenPath {
                                assembly: Some(assembly),
                                namespace: reference.namespace,
                                names,
                            });
                        }
                    }
                }
            }
            TypeToken::Specification(_) => None,
        }
    }
}

/// Where a type token leads: an optional assembly, a namespace, and the nesting
/// chain of metadata names (outermost first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPath<'data> {
    pub assembly: Option<&'data str>,
    pub namespace: &'data str,
    pub names: Vec<&'data str>,
}

/// `"List`1"` -> `("List", 1)`.
pub fn split_arity(metadata_name: &str) -> (&str, u32) {
    match metadata_name.rsplit_once('`') {
        Some((name, arity)) => match arity.parse() {
            Ok(arity) => (name, arity),
            Err(_) => (metadata_name, 0),
        },
        None => (metadata_name, 0),
    }
}

// ---------------------------------------------------------------------------
// building
// ---------------------------------------------------------------------------

fn build<'data>(
    raw: &RawTables,
    strings: &'data [u8],
    blobs: &'data [u8],
) -> Result<DotNetAssembly<'data>, MetadataError> {
    let type_specs = raw
        .type_specs
        .iter()
        .map(|&blob| signature::parse_type_spec(heap_blob(blobs, blob)?))
        .collect::<Result<Vec<_>, _>>()?;

    let assembly_ref_names = raw
        .assembly_refs
        .iter()
        .map(|row| heap_string(strings, row.name))
        .collect::<Result<Vec<_>, _>>()?;

    let type_refs = raw
        .type_refs
        .iter()
        .map(|row| {
            let scope = match row.resolution_scope {
                CodedIndex {
                    table: tables::ASSEMBLY_REF,
                    row,
                } if row > 0 => TypeRefScope::Assembly(
                    assembly_ref_names
                        .get(row as usize - 1)
                        .copied()
                        .ok_or(MetadataError::InvalidTableIndex)?,
                ),
                CodedIndex {
                    table: tables::TYPE_REF,
                    row,
                } if row > 0 => TypeRefScope::Nested(row - 1),
                _ => TypeRefScope::Module,
            };
            Ok(TypeReference {
                scope,
                namespace: heap_string(strings, row.namespace)?,
                name: heap_string(strings, row.name)?,
            })
        })
        .collect::<Result<Vec<_>, MetadataError>>()?;

    // constants, keyed by parent row
    let mut field_constants = HashMap::new();
    let mut param_constants = HashMap::new();
    for row in &raw.constants {
        let value = decode_constant(row.element_type, heap_blob(blobs, row.value)?)?;
        match row.parent.table {
            tables::FIELD => field_constants.insert(row.parent.row, value),
            tables::PARAM => param_constants.insert(row.parent.row, value),
            _ => None,
        };
    }

    let field_lists: Vec<u32> = raw.type_defs.iter().map(|row| row.field_list).collect();
    let method_lists: Vec<u32> = raw.type_defs.iter().map(|row| row.method_list).collect();
    let param_lists: Vec<u32> = raw.methods.iter().map(|row| row.param_list).collect();

    // 1-based method row -> (type index, local index), from the method ranges
    let method_owner = owner_of_ranges(&method_lists, raw.methods.len() as u32);

    let mut types = Vec::with_capacity(raw.type_defs.len());
    for (index, row) in raw.type_defs.iter().enumerate() {
        let extends = if row.extends.row == 0 {
            None
        } else {
            Some(coded_to_sig(row.extends, &type_specs)?)
        };

        let (field_start, field_end) = list_range(&field_lists, index, raw.fields.len() as u32);
        let mut fields = Vec::with_capacity((field_end - field_start) as usize);
        for field_row in field_start..field_end {
            let field = &raw.fields[field_row as usize - 1];
            fields.push(FieldDefinition {
                name: heap_string(strings, field.name)?,
                flags: field.flags,
                field_type: signature::parse_field(heap_blob(blobs, field.signature)?)?,
                constant: field_constants.get(&field_row).cloned(),
            });
        }

        let (method_start, method_end) = list_range(&method_lists, index, raw.methods.len() as u32);
        let mut methods = Vec::with_capacity((method_end - method_start) as usize);
        for method_row in method_start..method_end {
            let method = &raw.methods[method_row as usize - 1];
            let sig = signature::parse_method(heap_blob(blobs, method.signature)?)?;

            let (param_start, param_end) = list_range(
                &param_lists,
                method_row as usize - 1,
                raw.params.len() as u32,
            );
            let mut parameters = vec![ParameterDefinition::default(); sig.parameters.len()];
            for param_row in param_start..param_end {
                let param = &raw.params[param_row as usize - 1];
                // sequence 0 is the return value; parameters start at 1
                if param.sequence >= 1 && (param.sequence as usize) <= parameters.len() {
                    parameters[param.sequence as usize - 1] = ParameterDefinition {
                        name: heap_string(strings, param.name)?,
                        flags: param.flags,
                        constant: param_constants.get(&param_row).cloned(),
                    };
                }
            }

            methods.push(MethodDefinition {
                name: heap_string(strings, method.name)?,
                flags: method.flags,
                signature: sig,
                parameters,
            });
        }

        types.push(TypeDefinition {
            namespace: heap_string(strings, row.namespace)?,
            name: heap_string(strings, row.name)?,
            flags: row.flags,
            is_value_type: false, // filled in below
            is_enum: false,
            extends,
            interfaces: Vec::new(),
            generic_parameters: Vec::new(),
            fields,
            methods,
            properties: Vec::new(),
            events: Vec::new(),
            nested_types: Vec::new(),
            enclosing_type: None,
        });
    }

    // value-type / enum detection off the base type's name
    for index in 0..types.len() {
        let Some(extends) = &types[index].extends else {
            continue;
        };
        let token = match extends {
            TypeSig::Named { token, .. } => *token,
            _ => continue,
        };
        let path = match token {
            TypeToken::Definition(definition) => {
                let base = &types[definition as usize];
                (base.namespace, base.name)
            }
            TypeToken::Reference(reference) => {
                let base = type_refs
                    .get(reference as usize)
                    .ok_or(MetadataError::InvalidTableIndex)?;
                (base.namespace, base.name)
            }
            TypeToken::Specification(_) => continue,
        };

        let own_name = (types[index].namespace, types[index].name);
        types[index].is_enum = path == ("System", "Enum");
        types[index].is_value_type = types[index].is_enum
            || (path == ("System", "ValueType") && own_name != ("System", "Enum"));
    }

    for row in &raw.interface_impls {
        if row.class == 0 || row.interface.row == 0 {
            continue;
        }
        let sig = coded_to_sig(row.interface, &type_specs)?;
        types
            .get_mut(row.class as usize - 1)
            .ok_or(MetadataError::InvalidTableIndex)?
            .interfaces
            .push(sig);
    }

    for row in &raw.nested_classes {
        let nested = row
            .nested
            .checked_sub(1)
            .ok_or(MetadataError::InvalidTableIndex)? as usize;
        let enclosing = row
            .enclosing
            .checked_sub(1)
            .ok_or(MetadataError::InvalidTableIndex)?;
        types
            .get_mut(nested)
            .ok_or(MetadataError::InvalidTableIndex)?
            .enclosing_type = Some(enclosing);
        types
            .get_mut(enclosing as usize)
            .ok_or(MetadataError::InvalidTableIndex)?
            .nested_types
            .push(nested as u32);
    }

    // generic parameters, with their constraints grouped by owning parameter first
    let mut constraints_of: HashMap<u32, Vec<CodedIndex>> = HashMap::new();
    for constraint in &raw.generic_param_constraints {
        if constraint.constraint.row > 0 {
            constraints_of
                .entry(constraint.owner)
                .or_default()
                .push(constraint.constraint);
        }
    }

    for (index, row) in raw.generic_params.iter().enumerate() {
        let mut parameter = GenericParameter {
            name: heap_string(strings, row.name)?,
            variance: match row.flags & 0x3 {
                0x1 => Variance::Covariant,
                0x2 => Variance::Contravariant,
                _ => Variance::Invariant,
            },
            constraints: Vec::new(),
        };
        for &constraint in constraints_of
            .get(&(index as u32 + 1))
            .into_iter()
            .flatten()
        {
            parameter
                .constraints
                .push(coded_to_sig(constraint, &type_specs)?);
        }

        if row.owner.table == tables::TYPE_DEF && row.owner.row > 0 {
            let owner = types
                .get_mut(row.owner.row as usize - 1)
                .ok_or(MetadataError::InvalidTableIndex)?;
            // rows are sorted by owner then number, so pushing keeps declaration order
            owner.generic_parameters.push(parameter);
        }
        // method generic parameter names are dropped: the signature carries the count
    }

    // properties and events, wired to their accessor methods
    let mut property_accessors: HashMap<u32, (Option<u32>, Option<u32>)> = HashMap::new();
    let mut event_accessors: HashMap<u32, (Option<u32>, Option<u32>)> = HashMap::new();
    for row in &raw.method_semantics {
        let Some(&(_, local)) = method_owner.get(&row.method) else {
            continue;
        };
        match row.association.table {
            tables::PROPERTY => {
                let entry = property_accessors.entry(row.association.row).or_default();
                match row.semantics {
                    0x2 => entry.0 = Some(local),
                    0x1 => entry.1 = Some(local),
                    _ => {}
                }
            }
            tables::EVENT => {
                let entry = event_accessors.entry(row.association.row).or_default();
                match row.semantics {
                    0x8 => entry.0 = Some(local),
                    0x10 => entry.1 = Some(local),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    for (index, map) in raw.property_maps.iter().enumerate() {
        let end = raw
            .property_maps
            .get(index + 1)
            .map(|next| next.property_list)
            .unwrap_or(raw.properties.len() as u32 + 1);
        let (start, end) = clamp_range(map.property_list, end, raw.properties.len() as u32);
        let owner = types
            .get_mut(map.parent as usize - 1)
            .ok_or(MetadataError::InvalidTableIndex)?;
        for property_row in start..end {
            let property = &raw.properties[property_row as usize - 1];
            let (getter, setter) = property_accessors
                .get(&property_row)
                .copied()
                .unwrap_or_default();
            owner.properties.push(PropertyDefinition {
                name: heap_string(strings, property.name)?,
                signature: signature::parse_property(heap_blob(blobs, property.signature)?)?,
                getter,
                setter,
            });
        }
    }

    for (index, map) in raw.event_maps.iter().enumerate() {
        let end = raw
            .event_maps
            .get(index + 1)
            .map(|next| next.event_list)
            .unwrap_or(raw.events.len() as u32 + 1);
        let (start, end) = clamp_range(map.event_list, end, raw.events.len() as u32);
        let owner = types
            .get_mut(map.parent as usize - 1)
            .ok_or(MetadataError::InvalidTableIndex)?;
        for event_row in start..end {
            let event = &raw.events[event_row as usize - 1];
            let (add, remove) = event_accessors.get(&event_row).copied().unwrap_or_default();
            let event_type = if event.event_type.row == 0 {
                None
            } else {
                Some(coded_to_sig(event.event_type, &type_specs)?)
            };
            owner.events.push(EventDefinition {
                name: heap_string(strings, event.name)?,
                event_type,
                add,
                remove,
            });
        }
    }

    let forwarders = raw
        .exported_types
        .iter()
        .filter(|row| {
            row.implementation.table == tables::ASSEMBLY_REF && row.implementation.row > 0
        })
        .map(|row| {
            Ok(TypeForwarder {
                namespace: heap_string(strings, row.namespace)?,
                name: heap_string(strings, row.name)?,
                assembly: assembly_ref_names
                    .get(row.implementation.row as usize - 1)
                    .copied()
                    .ok_or(MetadataError::InvalidTableIndex)?,
            })
        })
        .collect::<Result<Vec<_>, MetadataError>>()?;

    let mut top_level = HashMap::new();
    for (index, definition) in types.iter().enumerate() {
        if definition.enclosing_type.is_none() {
            top_level.insert((definition.namespace, definition.name), index as u32);
        }
    }

    let (name, version) = match &raw.assembly {
        Some(assembly) => (heap_string(strings, assembly.name)?, assembly.version),
        None => ("", (0, 0, 0, 0)),
    };

    let references = raw
        .assembly_refs
        .iter()
        .zip(&assembly_ref_names)
        .map(|(row, &name)| AssemblyReference {
            name,
            version: row.version,
        })
        .collect();

    Ok(DotNetAssembly {
        name,
        version,
        references,
        types,
        forwarders,
        type_refs,
        type_specs,
        top_level,
    })
}

/// A coded TypeDefOrRef into a [`TypeSig`]. TypeSpecs are already parsed, so a
/// generic base type (`: List<int>`) comes out fully structured.
fn coded_to_sig(coded: CodedIndex, type_specs: &[TypeSig]) -> Result<TypeSig, MetadataError> {
    let row = coded
        .row
        .checked_sub(1)
        .ok_or(MetadataError::InvalidTableIndex)?;
    Ok(match coded.table {
        tables::TYPE_DEF => TypeSig::Named {
            token: TypeToken::Definition(row),
            is_value_type: false,
        },
        tables::TYPE_REF => TypeSig::Named {
            token: TypeToken::Reference(row),
            is_value_type: false,
        },
        tables::TYPE_SPEC => type_specs
            .get(row as usize)
            .cloned()
            .ok_or(MetadataError::InvalidTableIndex)?,
        _ => return Err(MetadataError::InvalidTableIndex),
    })
}

/// The half-open 1-based row range `[starts[i], starts[i+1])` of an owner, with the
/// last owner running to the end of the owned table.
fn list_range(starts: &[u32], index: usize, owned_rows: u32) -> (u32, u32) {
    let start = starts.get(index).copied().unwrap_or(1);
    let end = starts.get(index + 1).copied().unwrap_or(owned_rows + 1);
    clamp_range(start, end, owned_rows)
}

fn clamp_range(start: u32, end: u32, owned_rows: u32) -> (u32, u32) {
    let start = start.max(1);
    (start, end.clamp(start, owned_rows + 1))
}

/// 1-based row -> (owner index, local index within the owner's range).
fn owner_of_ranges(starts: &[u32], owned_rows: u32) -> HashMap<u32, (u32, u32)> {
    let mut map = HashMap::new();

    for (owner, &start) in starts.iter().enumerate() {
        let end = starts.get(owner + 1).copied().unwrap_or(owned_rows + 1);
        for (local, row) in (start..end).enumerate() {
            map.insert(row, (owner as u32, local as u32));
        }
    }

    map
}

fn decode_constant(element_type: u8, blob: &[u8]) -> Result<Constant, MetadataError> {
    let need = |count: usize| {
        if blob.len() < count {
            Err(MetadataError::UnexpectedEnd)
        } else {
            Ok(())
        }
    };

    Ok(match element_type {
        0x02 => {
            need(1)?;
            Constant::Boolean(blob[0] != 0)
        }
        0x03 => {
            need(2)?;
            Constant::Char(u16::from_le_bytes([blob[0], blob[1]]))
        }
        0x04 => {
            need(1)?;
            Constant::SByte(blob[0] as i8)
        }
        0x05 => {
            need(1)?;
            Constant::Byte(blob[0])
        }
        0x06 => {
            need(2)?;
            Constant::Int16(i16::from_le_bytes([blob[0], blob[1]]))
        }
        0x07 => {
            need(2)?;
            Constant::UInt16(u16::from_le_bytes([blob[0], blob[1]]))
        }
        0x08 => {
            need(4)?;
            Constant::Int32(i32::from_le_bytes(blob[..4].try_into().unwrap()))
        }
        0x09 => {
            need(4)?;
            Constant::UInt32(u32::from_le_bytes(blob[..4].try_into().unwrap()))
        }
        0x0A => {
            need(8)?;
            Constant::Int64(i64::from_le_bytes(blob[..8].try_into().unwrap()))
        }
        0x0B => {
            need(8)?;
            Constant::UInt64(u64::from_le_bytes(blob[..8].try_into().unwrap()))
        }
        0x0C => {
            need(4)?;
            Constant::Single(f32::from_le_bytes(blob[..4].try_into().unwrap()))
        }
        0x0D => {
            need(8)?;
            Constant::Double(f64::from_le_bytes(blob[..8].try_into().unwrap()))
        }
        0x0E => {
            // UTF-16LE
            let units: Vec<u16> = blob
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            Constant::String(String::from_utf16_lossy(&units))
        }
        0x12 | 0x1C => Constant::Null, // CLASS / OBJECT: always a null reference
        _ => return Err(MetadataError::InvalidSignature),
    })
}
