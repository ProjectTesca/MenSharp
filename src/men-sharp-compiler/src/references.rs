//! Referenced assemblies as an [`ExternalTypes`] provider.
//!
//! The driver reads dll files into byte buffers (which the caller keeps alive),
//! parses each with `men-sharp-dotnet`, and serves the semantic layer's questions:
//! name lookup over every externally visible type, and — through the conversion in
//! this module — base types, interfaces and member signatures as semantic
//! [`Type`]s. The semantic layer never learns that dlls were involved.
//!
//! Two pieces of cross-assembly plumbing live here:
//!
//! - **TypeRef chasing**: a signature in `UnityEngine.CoreModule.dll` may name a
//!   type as "`System.Object` in assembly `mscorlib`"; the token is resolved into
//!   whichever loaded assembly really defines it.
//! - **Type forwarders**: facade assemblies (`netstandard.dll`, `System.Runtime`)
//!   define nothing and forward everything; when a name is not defined where a
//!   reference points, its forwarder entry redirects the search, transitively.
//!
//! When several assemblies define the same top-level name the first one given wins.
//! `params` on external methods is not detected yet (it lives in a custom
//! attribute, which the metadata reader does not decode).

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use men_sharp_dotnet::{
    DotNetAssembly, MetadataError, MethodSig, TypeDefinition, TypeSig, TypeToken,
};
use men_sharp_semantics::{
    Accessibility, DefaultArgument, ExternalConstant, ExternalMember, ExternalMemberKind,
    ExternalTypeId, ExternalTypeInfo, ExternalTypeKind, ExternalTypes, FunctionSignature,
    MemberSignature, ParameterPassing, ParameterSignature, Type, TypeTarget, TypeVariance,
};

pub struct ReferenceSet<'data> {
    assemblies: Vec<DotNetAssembly<'data>>,
    /// (dotted namespace, C# name, arity) -> type. Keys borrow the dll bytes.
    types: HashMap<(&'data str, &'data str, u32), ExternalTypeId>,
    /// Every dotted namespace prefix that exists in any referenced assembly.
    namespaces: HashSet<&'data str>,
    assembly_by_name: HashMap<&'data str, u32>,
    /// Per assembly: (namespace, metadata name) -> the assembly it forwards to.
    forwarders: Vec<HashMap<(&'data str, &'data str), &'data str>>,
    /// (namespace, method name) -> the static classes declaring such an extension
    /// method.
    extensions: HashMap<(&'data str, &'data str), Vec<ExternalTypeId>>,
}

impl<'data> ReferenceSet<'data> {
    /// Indexes already-parsed assemblies.
    pub fn new(assemblies: Vec<DotNetAssembly<'data>>) -> Self {
        let mut types = HashMap::default();
        let mut namespaces = HashSet::default();
        let mut assembly_by_name = HashMap::default();
        let mut forwarders = Vec::with_capacity(assemblies.len());
        let mut extensions: HashMap<(&str, &str), Vec<ExternalTypeId>> = HashMap::default();

        for (assembly_index, assembly) in assemblies.iter().enumerate() {
            assembly_by_name
                .entry(assembly.name)
                .or_insert(assembly_index as u32);

            forwarders.push(
                assembly
                    .forwarders
                    .iter()
                    .map(|forwarder| ((forwarder.namespace, forwarder.name), forwarder.assembly))
                    .collect::<HashMap<_, _>>(),
            );

            for (type_index, definition) in assembly.types.iter().enumerate() {
                if definition.enclosing_type.is_some()
                    || !assembly.is_externally_visible(type_index as u32)
                {
                    continue;
                }

                let (name, arity) = definition.name_and_arity();
                let id = ExternalTypeId {
                    assembly: assembly_index as u32,
                    type_index: type_index as u32,
                };
                types
                    .entry((definition.namespace, name, arity))
                    .or_insert(id);

                if definition.is_extension {
                    for method in &definition.methods {
                        if method.is_extension {
                            let owners = extensions
                                .entry((definition.namespace, method.name))
                                .or_default();
                            if !owners.contains(&id) {
                                owners.push(id);
                            }
                        }
                    }
                }

                // "System.Collections.Generic" also proves "System" and
                // "System.Collections"; prefixes are subslices of the same string
                let namespace = definition.namespace;
                if !namespace.is_empty() {
                    let mut end = namespace.len();
                    loop {
                        namespaces.insert(&namespace[..end]);
                        match namespace[..end].rfind('.') {
                            Some(dot) => end = dot,
                            None => break,
                        }
                    }
                }
            }
        }

        Self {
            assemblies,
            types,
            namespaces,
            assembly_by_name,
            forwarders,
            extensions,
        }
    }

    pub fn assemblies(&self) -> &[DotNetAssembly<'data>] {
        &self.assemblies
    }

    /// The definition behind an id handed out by this provider.
    pub fn type_definition(&self, id: ExternalTypeId) -> &TypeDefinition<'data> {
        self.assemblies[id.assembly as usize].type_definition(id.type_index)
    }

    /// `UnityEngine.Debug`-style display name for diagnostics.
    pub fn display_name(&self, id: ExternalTypeId) -> String {
        let definition = self.type_definition(id);
        if definition.namespace.is_empty() {
            definition.name.to_string()
        } else {
            format!("{}.{}", definition.namespace, definition.name)
        }
    }

    // ------------------------------------------------- cross-assembly lookup

    /// A type by metadata path (`namespace` + nesting chain of metadata names),
    /// starting in one assembly and following type forwarders until it lands on a
    /// real definition.
    fn resolve_metadata_path(
        &self,
        assembly_index: u32,
        namespace: &str,
        names: &[&str],
    ) -> Option<ExternalTypeId> {
        let first = *names.first()?;

        let mut current = assembly_index;
        let mut hops = 0;
        let top = loop {
            let assembly = &self.assemblies[current as usize];
            if let Some(top) = assembly.find_type(namespace, first) {
                break top;
            }

            // not defined here: follow the forwarder, if there is one
            let target = self.forwarders[current as usize].get(&(namespace, first))?;
            current = *self.assembly_by_name.get(target)?;
            hops += 1;
            if hops > self.assemblies.len() {
                return None; // a forwarder cycle in broken metadata
            }
        };

        // walk the nesting chain by metadata name
        let assembly = &self.assemblies[current as usize];
        let mut index = top;
        for name in &names[1..] {
            index = assembly
                .type_definition(index)
                .nested_types
                .iter()
                .copied()
                .find(|&nested| assembly.type_definition(nested).name == *name)?;
        }

        Some(ExternalTypeId {
            assembly: current,
            type_index: index,
        })
    }

    /// A raw signature token from `within` into a provider-wide type id.
    fn resolve_token(&self, within: u32, token: TypeToken) -> Option<ExternalTypeId> {
        match token {
            TypeToken::Definition(index) => Some(ExternalTypeId {
                assembly: within,
                type_index: index,
            }),
            TypeToken::Reference(_) => {
                let path = self.assemblies[within as usize].token_path(token)?;
                let assembly = match path.assembly {
                    Some(name) => *self.assembly_by_name.get(name)?,
                    None => within,
                };
                self.resolve_metadata_path(assembly, path.namespace, &path.names)
            }
            // never appears inside CLASS/VALUETYPE positions of a signature
            TypeToken::Specification(_) => None,
        }
    }

    // ------------------------------------------------------------ conversion

    /// A metadata signature into a semantic [`Type`]. `owner` is the type whose
    /// declaration the signature appears in — its generic parameters are what
    /// `!n` refers to.
    fn convert(&self, owner: ExternalTypeId, sig: &TypeSig) -> Type {
        match sig {
            TypeSig::Void => Type::Void,
            TypeSig::Boolean => self.corlib("Boolean"),
            TypeSig::Char => self.corlib("Char"),
            TypeSig::SByte => self.corlib("SByte"),
            TypeSig::Byte => self.corlib("Byte"),
            TypeSig::Int16 => self.corlib("Int16"),
            TypeSig::UInt16 => self.corlib("UInt16"),
            TypeSig::Int32 => self.corlib("Int32"),
            TypeSig::UInt32 => self.corlib("UInt32"),
            TypeSig::Int64 => self.corlib("Int64"),
            TypeSig::UInt64 => self.corlib("UInt64"),
            TypeSig::Single => self.corlib("Single"),
            TypeSig::Double => self.corlib("Double"),
            TypeSig::String => self.corlib("String"),
            TypeSig::Object => self.corlib("Object"),
            TypeSig::IntPtr => self.corlib("IntPtr"),
            TypeSig::UIntPtr => self.corlib("UIntPtr"),
            TypeSig::Named { token, .. } => match self.resolve_token(owner.assembly, *token) {
                Some(id) => Type::Named {
                    target: TypeTarget::External(id),
                    arguments: Vec::new(),
                },
                None => Type::Error,
            },
            TypeSig::Generic {
                token, arguments, ..
            } => match self.resolve_token(owner.assembly, *token) {
                Some(id) => Type::Named {
                    target: TypeTarget::External(id),
                    arguments: arguments
                        .iter()
                        .map(|argument| self.convert(owner, argument))
                        .collect(),
                },
                None => Type::Error,
            },
            TypeSig::SzArray(element) => Type::Array {
                element: Box::new(self.convert(owner, element)),
                rank: 1,
            },
            TypeSig::Array { element, rank } => Type::Array {
                element: Box::new(self.convert(owner, element)),
                rank: *rank,
            },
            TypeSig::Pointer(element) => Type::Pointer(Box::new(self.convert(owner, element))),
            TypeSig::ByRef(element) => Type::ByRef {
                readonly: false,
                element: Box::new(self.convert(owner, element)),
            },
            TypeSig::TypeParameter(index) => Type::ExternalTypeParameter {
                owner,
                index: *index,
            },
            TypeSig::MethodTypeParameter(index) => Type::ExternalMethodTypeParameter(*index),
            // no shape Udon could ever call; surfaced as an error type if used
            TypeSig::TypedReference | TypeSig::FunctionPointer => Type::Error,
        }
    }

    fn corlib(&self, name: &str) -> Type {
        match self.types.get(&("System", name, 0)) {
            Some(&id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => Type::Error,
        }
    }

    fn convert_function(
        &self,
        owner: ExternalTypeId,
        signature: &MethodSig,
        definitions: &[men_sharp_dotnet::ParameterDefinition<'_>],
    ) -> FunctionSignature {
        FunctionSignature {
            return_type: self.convert(owner, &signature.return_type),
            parameters: signature
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let converted = self.convert(owner, parameter);
                    // metadata spells `ref`/`out` as a byref type plus a flag
                    let (passing, parameter_type) = match converted {
                        Type::ByRef { element, .. } => (
                            if definitions
                                .get(index)
                                .is_some_and(|definition| definition.is_out())
                            {
                                ParameterPassing::Out
                            } else {
                                ParameterPassing::Ref
                            },
                            *element,
                        ),
                        other => (ParameterPassing::Value, other),
                    };
                    let definition = definitions.get(index);
                    ParameterSignature {
                        passing,
                        is_params: definition.is_some_and(|definition| definition.is_params),
                        parameter_type,
                        name: definition.map(|definition| definition.name.to_string()),
                        default_value: definition
                            .filter(|definition| definition.is_optional())
                            .map(|definition| match &definition.constant {
                                Some(men_sharp_dotnet::Constant::Null) => DefaultArgument::Null,
                                Some(constant) => convert_constant(constant)
                                    .map(DefaultArgument::Constant)
                                    .unwrap_or(DefaultArgument::Default),
                                None => DefaultArgument::Default,
                            }),
                    }
                })
                .collect(),
        }
    }

    /// A delegate extends `System.MulticastDelegate` (or `System.Delegate`).
    fn is_delegate(&self, definition: &TypeDefinition, id: ExternalTypeId) -> bool {
        let Some(TypeSig::Named { token, .. }) = &definition.extends else {
            return false;
        };
        let Some(path) = self.assemblies[id.assembly as usize].token_path(*token) else {
            return false;
        };
        path.namespace == "System"
            && matches!(path.names.as_slice(), ["MulticastDelegate"] | ["Delegate"])
    }
}

/// A metadata constant in the semantic layer's vocabulary. Integral kinds
/// widen losslessly; the field's declared type says how to narrow back.
fn convert_constant(constant: &men_sharp_dotnet::Constant) -> Option<ExternalConstant> {
    use men_sharp_dotnet::Constant;
    Some(match constant {
        Constant::Boolean(value) => ExternalConstant::Boolean(*value),
        Constant::Char(value) => ExternalConstant::Char(char::from_u32(*value as u32)?),
        Constant::SByte(value) => ExternalConstant::Int(*value as i64),
        Constant::Byte(value) => ExternalConstant::UInt(*value as u64),
        Constant::Int16(value) => ExternalConstant::Int(*value as i64),
        Constant::UInt16(value) => ExternalConstant::UInt(*value as u64),
        Constant::Int32(value) => ExternalConstant::Int(*value as i64),
        Constant::UInt32(value) => ExternalConstant::UInt(*value as u64),
        Constant::Int64(value) => ExternalConstant::Int(*value),
        Constant::UInt64(value) => ExternalConstant::UInt(*value),
        Constant::Single(value) => ExternalConstant::Single(*value),
        Constant::Double(value) => ExternalConstant::Double(*value),
        Constant::String(value) => ExternalConstant::String(value.clone()),
        Constant::Null => return None,
    })
}

fn accessibility_from(access_bits: u16) -> Accessibility {
    match access_bits & 0x7 {
        0x6 => Accessibility::Public,
        0x5 => Accessibility::ProtectedInternal, // FamORAssem
        0x4 => Accessibility::Protected,         // Family
        0x3 => Accessibility::Internal,          // Assembly
        0x2 => Accessibility::PrivateProtected,  // FamANDAssem
        _ => Accessibility::Private,
    }
}

/// What the checker asks [`ExternalTypes::members_named`] for to get a
/// type's indexers, whichever name the metadata gives them.
const INDEXER_NAME: &str = men_sharp_semantics::INDEXER_LOOKUP_NAME;

impl ExternalTypes for ReferenceSet<'_> {
    fn find_type(&self, namespace: &[&str], name: &str, arity: u32) -> Option<ExternalTypeId> {
        let joined = namespace.join(".");
        self.types.get(&(joined.as_str(), name, arity)).copied()
    }

    fn find_nested_type(
        &self,
        parent: ExternalTypeId,
        name: &str,
        arity: u32,
    ) -> Option<ExternalTypeId> {
        let assembly = &self.assemblies[parent.assembly as usize];
        let definition = assembly.type_definition(parent.type_index);

        definition
            .nested_types
            .iter()
            .copied()
            .find(|&nested| {
                let nested_definition = assembly.type_definition(nested);
                nested_definition.name_and_arity() == (name, arity)
                    && assembly.is_externally_visible(nested)
            })
            .map(|nested| ExternalTypeId {
                assembly: parent.assembly,
                type_index: nested,
            })
    }

    fn namespace_exists(&self, namespace: &[&str]) -> bool {
        !namespace.is_empty() && self.namespaces.contains(namespace.join(".").as_str())
    }

    fn type_info(&self, id: ExternalTypeId) -> ExternalTypeInfo {
        let definition = self.type_definition(id);

        let kind = if definition.is_enum {
            ExternalTypeKind::Enum
        } else if definition.is_value_type {
            ExternalTypeKind::Struct
        } else if definition.is_interface() {
            ExternalTypeKind::Interface
        } else if self.is_delegate(definition, id) {
            ExternalTypeKind::Delegate
        } else {
            ExternalTypeKind::Class
        };

        ExternalTypeInfo {
            kind,
            arity: definition.generic_parameters.len() as u32,
            is_sealed: definition.is_sealed(),
            is_abstract: definition.is_abstract(),
        }
    }

    fn base_type(&self, id: ExternalTypeId) -> Option<Type> {
        let extends = self.type_definition(id).extends.as_ref()?;
        Some(self.convert(id, extends))
    }

    fn interfaces(&self, id: ExternalTypeId) -> Vec<Type> {
        self.type_definition(id)
            .interfaces
            .iter()
            .map(|interface| self.convert(id, interface))
            .collect()
    }

    fn members_named(&self, id: ExternalTypeId, name: &str) -> Vec<ExternalMember> {
        let definition = self.type_definition(id);
        let mut members = Vec::new();

        for field in &definition.fields {
            if field.name == name {
                members.push(ExternalMember {
                    name: field.name.to_string(),
                    kind: ExternalMemberKind::Field,
                    is_static: field.is_static(),
                    accessibility: accessibility_from(field.flags),
                    is_extension: false,
                    signature: MemberSignature::Field(self.convert(id, &field.field_type)),
                    constant: field.constant.as_ref().and_then(convert_constant),
                });
            }
        }

        for method in &definition.methods {
            if method.name != name {
                continue;
            }
            members.push(ExternalMember {
                name: method.name.to_string(),
                kind: if method.name == ".ctor" || method.name == ".cctor" {
                    ExternalMemberKind::Constructor
                } else {
                    ExternalMemberKind::Method {
                        type_parameter_count: method.signature.generic_parameter_count,
                    }
                },
                is_static: method.is_static(),
                accessibility: accessibility_from(method.flags),
                is_extension: method.is_extension,
                signature: MemberSignature::Function(self.convert_function(
                    id,
                    &method.signature,
                    &method.parameters,
                )),
                constant: None,
            });
        }

        for property in &definition.properties {
            // `this[]` asks for the indexers, whatever `[IndexerName]` called
            // them: `Item` almost everywhere, `Chars` on StringBuilder. Not
            // the explicit interface implementations (`IList.Item`, named
            // with their interface): those are reached through the
            // interface, as in C#, and would only make `list[0]` ambiguous
            let is_indexer =
                !property.signature.parameters.is_empty() && !property.name.contains('.');
            if property.name != name && !(name == INDEXER_NAME && is_indexer) {
                continue;
            }
            // static-ness and accessibility live on the accessor methods
            let accessor = property
                .getter
                .or(property.setter)
                .map(|local| &definition.methods[local as usize]);

            let signature = if property.signature.parameters.is_empty() {
                MemberSignature::Property(self.convert(id, &property.signature.property_type))
            } else {
                // an indexer: parameters plus an element type
                MemberSignature::Function(FunctionSignature {
                    return_type: self.convert(id, &property.signature.property_type),
                    parameters: property
                        .signature
                        .parameters
                        .iter()
                        .map(|parameter| ParameterSignature {
                            passing: ParameterPassing::Value,
                            is_params: false,
                            parameter_type: self.convert(id, parameter),
                            name: None,
                            default_value: None,
                        })
                        .collect(),
                })
            };

            members.push(ExternalMember {
                name: property.name.to_string(),
                kind: ExternalMemberKind::Property {
                    has_getter: property.getter.is_some(),
                    has_setter: property.setter.is_some(),
                },
                is_static: accessor.map(|method| method.is_static()).unwrap_or(false),
                accessibility: accessor
                    .map(|method| accessibility_from(method.flags))
                    .unwrap_or(Accessibility::Private),
                is_extension: false,
                signature,
                constant: None,
            });
        }

        for event in &definition.events {
            if event.name != name {
                continue;
            }
            let accessor = event.add.map(|local| &definition.methods[local as usize]);
            members.push(ExternalMember {
                name: event.name.to_string(),
                kind: ExternalMemberKind::Event,
                is_static: accessor.map(|method| method.is_static()).unwrap_or(false),
                accessibility: accessor
                    .map(|method| accessibility_from(method.flags))
                    .unwrap_or(Accessibility::Private),
                is_extension: false,
                signature: MemberSignature::Event(
                    event
                        .event_type
                        .as_ref()
                        .map(|event_type| self.convert(id, event_type))
                        .unwrap_or(Type::Error),
                ),
                constant: None,
            });
        }

        members
    }

    fn display_name(&self, id: ExternalTypeId) -> String {
        ReferenceSet::display_name(self, id)
    }

    fn variances(&self, id: ExternalTypeId) -> Vec<TypeVariance> {
        self.type_definition(id)
            .generic_parameters
            .iter()
            .map(|parameter| match parameter.variance {
                men_sharp_dotnet::Variance::Covariant => TypeVariance::Covariant,
                men_sharp_dotnet::Variance::Contravariant => TypeVariance::Contravariant,
                men_sharp_dotnet::Variance::Invariant => TypeVariance::Invariant,
            })
            .collect()
    }

    fn extension_method_owners(&self, namespace: &[&str], name: &str) -> Vec<ExternalTypeId> {
        let joined = namespace.join(".");
        self.extensions
            .get(&(joined.as_str(), name))
            .cloned()
            .unwrap_or_default()
    }
}

/// A parse failure tied to the file it came from.
#[derive(Debug)]
pub struct ReferenceError {
    /// Index into the byte buffers handed to [`crate::Compiler::load_references`].
    pub reference: usize,
    pub error: MetadataError,
}
