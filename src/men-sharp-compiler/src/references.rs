//! Referenced assemblies as an [`ExternalTypes`] provider.
//!
//! The driver reads dll files into byte buffers (which the caller keeps alive),
//! parses each with `men-sharp-dotnet`, and indexes every externally visible
//! top-level type by `(namespace, name, arity)` plus every namespace prefix. That
//! is exactly the shape `men-sharp-semantics` asks its [`ExternalTypes`] trait —
//! the semantic layer never learns that dlls were involved.
//!
//! When several assemblies declare the same type name the first one given wins;
//! type forwarders (facade assemblies) are not chased yet — reference the assembly
//! that really defines the types, as Unity projects naturally do.

use std::collections::{HashMap, HashSet};

use men_sharp_dotnet::{DotNetAssembly, MetadataError};
use men_sharp_semantics::{ExternalTypeId, ExternalTypes};

pub struct ReferenceSet<'data> {
    assemblies: Vec<DotNetAssembly<'data>>,
    /// (dotted namespace, C# name, arity) -> type. Keys borrow the dll bytes.
    types: HashMap<(&'data str, &'data str, u32), ExternalTypeId>,
    /// Every dotted namespace prefix that exists in any referenced assembly.
    namespaces: HashSet<&'data str>,
}

impl<'data> ReferenceSet<'data> {
    /// Indexes already-parsed assemblies.
    pub fn new(assemblies: Vec<DotNetAssembly<'data>>) -> Self {
        let mut types = HashMap::new();
        let mut namespaces = HashSet::new();

        for (assembly_index, assembly) in assemblies.iter().enumerate() {
            for (type_index, definition) in assembly.types.iter().enumerate() {
                if definition.enclosing_type.is_some()
                    || !assembly.is_externally_visible(type_index as u32)
                {
                    continue;
                }

                let (name, arity) = definition.name_and_arity();
                types
                    .entry((definition.namespace, name, arity))
                    .or_insert(ExternalTypeId {
                        assembly: assembly_index as u32,
                        type_index: type_index as u32,
                    });

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
        }
    }

    pub fn assemblies(&self) -> &[DotNetAssembly<'data>] {
        &self.assemblies
    }

    /// The definition behind an id handed out by this provider.
    pub fn type_definition(&self, id: ExternalTypeId) -> &men_sharp_dotnet::TypeDefinition<'data> {
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
}

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
}

/// A parse failure tied to the file it came from.
#[derive(Debug)]
pub struct ReferenceError {
    /// Index into the byte buffers handed to [`crate::Compiler::load_references`].
    pub reference: usize,
    pub error: MetadataError,
}
