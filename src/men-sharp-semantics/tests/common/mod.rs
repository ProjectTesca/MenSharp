//! What the tests around this crate share: a way to build the phases over a
//! few sources, and a stand-in for the referenced dlls.
//!
//! Every test binary compiles this whole file and uses a part of it, so the
//! rest is unused there by construction.
#![allow(dead_code, unused_imports, unused_macros)]

use men_sharp_semantics::types::{ExternalTypeId, MemberSignature, Type, TypeTarget};
use men_sharp_semantics::{Declarations, ExternalTypes, SemanticErrorKind, Signatures, SymbolId};

macro_rules! declarations {
    ($name:ident, $($source:expr),+ $(,)?) => {
        let asts: Vec<::men_sharp_parser::MenSharpAST> =
            vec![$(::men_sharp_parser::MenSharpAST::parse($source)),+];
        for ast in &asts {
            assert_eq!(ast.errors(), &[], "test source must parse cleanly");
        }
        let files = asts
            .iter()
            .enumerate()
            .map(|(index, ast)| {
                ::men_sharp_semantics::collect_file(
                    ::men_sharp_semantics::FileId(index as u32),
                    ast.ast(),
                )
            })
            .collect::<Vec<_>>();
        let $name = ::men_sharp_semantics::merge_declarations(files);
    };
}

/// Follows a dotted path of member names from the root.
pub fn find(declarations: &Declarations, path: &str) -> SymbolId {
    let mut current = declarations.table.root();
    for segment in path.split('.') {
        let matches = declarations.table.symbol(current).members_named(segment);
        assert!(!matches.is_empty(), "no symbol named {segment} in {path}");
        current = matches[0];
    }
    current
}

pub(crate) use declarations;

/// A hand-written provider standing in for referenced dlls.
pub struct MockExternal {
    /// (namespace, name, arity), position = type index.
    pub types: Vec<(&'static str, &'static str, u32)>,
    /// (parent index, name, arity), position offset by `types.len()`.
    pub nested: Vec<(u32, &'static str, u32)>,
}

impl MockExternal {
    pub fn corlib() -> Self {
        let mut mock = Self {
            types: vec![
                ("System", "Int32", 0),
                ("System", "String", 0),
                ("System", "Boolean", 0),
                ("System", "Object", 0),
                ("System", "SByte", 0),
                ("System", "Byte", 0),
                ("System", "Int16", 0),
                ("System", "UInt16", 0),
                ("System", "UInt32", 0),
                ("System", "Int64", 0),
                ("System", "UInt64", 0),
                ("System", "Char", 0),
                ("System", "Single", 0),
                ("System", "Double", 0),
                ("System", "Decimal", 0),
                ("System", "IntPtr", 0),
                ("System", "UIntPtr", 0),
                ("System", "Type", 0),
                ("System", "Array", 0),
                ("System", "ValueType", 0),
                ("System", "Enum", 0),
                ("System", "Func", 1),
                ("System", "Func", 2),
                ("System", "Func", 3),
                ("System", "Action", 0),
                ("System", "Action", 1),
                ("System.Collections.Generic", "List", 1),
                ("UnityEngine", "MonoBehaviour", 0),
                ("UnityEngine", "Debug", 0),
                ("UnityEngine.SceneManagement", "SceneManager", 0),
            ],
            nested: Vec::new(),
        };
        let list = mock
            .types
            .iter()
            .position(|&(_, name, _)| name == "List")
            .unwrap() as u32;
        mock.nested = vec![(list, "Enumerator", 1)];
        mock
    }

    pub fn id_of(&self, namespace: &str, name: &str) -> ExternalTypeId {
        let index = self
            .types
            .iter()
            .position(|&(ns, n, _)| ns == namespace && n == name)
            .unwrap();
        ExternalTypeId {
            assembly: 0,
            type_index: index as u32,
        }
    }
}

impl ExternalTypes for MockExternal {
    fn find_type(&self, namespace: &[&str], name: &str, arity: u32) -> Option<ExternalTypeId> {
        let joined = namespace.join(".");
        self.types
            .iter()
            .position(|&(ns, n, a)| ns == joined && n == name && a == arity)
            .map(|index| ExternalTypeId {
                assembly: 0,
                type_index: index as u32,
            })
    }

    fn find_nested_type(
        &self,
        parent: ExternalTypeId,
        name: &str,
        arity: u32,
    ) -> Option<ExternalTypeId> {
        self.nested
            .iter()
            .position(|&(p, n, a)| p == parent.type_index && n == name && a == arity)
            .map(|index| ExternalTypeId {
                assembly: 0,
                type_index: (self.types.len() + index) as u32,
            })
    }

    fn namespace_exists(&self, namespace: &[&str]) -> bool {
        let joined = namespace.join(".");
        self.types
            .iter()
            .any(|&(ns, ..)| ns == joined || ns.starts_with(&format!("{joined}.")))
    }

    fn type_info(
        &self,
        id: men_sharp_semantics::types::ExternalTypeId,
    ) -> men_sharp_semantics::ExternalTypeInfo {
        // the primitives are structs, as in the real corlib — `int?`
        // must stay `Nullable<int>` while `string?` is `string`
        let is_value = self
            .types
            .get(id.type_index as usize)
            .is_some_and(|&(ns, name, _)| {
                ns == "System"
                    && matches!(
                        name,
                        "Int32"
                            | "Boolean"
                            | "SByte"
                            | "Byte"
                            | "Int16"
                            | "UInt16"
                            | "UInt32"
                            | "Int64"
                            | "UInt64"
                            | "Char"
                            | "Single"
                            | "Double"
                            | "Decimal"
                            | "IntPtr"
                            | "UIntPtr"
                            | "Nullable"
                            | "ValueType"
                    )
            });
        men_sharp_semantics::ExternalTypeInfo {
            kind: if is_value {
                men_sharp_semantics::ExternalTypeKind::Struct
            } else {
                men_sharp_semantics::ExternalTypeKind::Class
            },
            arity: self
                .types
                .get(id.type_index as usize)
                .map(|&(.., arity)| arity)
                .unwrap_or(0),
            is_sealed: false,
            is_abstract: false,
        }
    }

    fn base_type(&self, _: men_sharp_semantics::types::ExternalTypeId) -> Option<Type> {
        None
    }

    fn interfaces(&self, _: men_sharp_semantics::types::ExternalTypeId) -> Vec<Type> {
        Vec::new()
    }

    fn members_named(
        &self,
        _: men_sharp_semantics::types::ExternalTypeId,
        _: &str,
    ) -> Vec<men_sharp_semantics::ExternalMember> {
        Vec::new()
    }

    fn display_name(&self, id: men_sharp_semantics::types::ExternalTypeId) -> String {
        self.types
            .get(id.type_index as usize)
            .map(|&(ns, name, _)| format!("{ns}.{name}"))
            .unwrap_or_else(|| "<nested>".to_string())
    }

    fn variances(
        &self,
        _: men_sharp_semantics::types::ExternalTypeId,
    ) -> Vec<men_sharp_semantics::TypeVariance> {
        Vec::new()
    }

    fn extension_method_owners(
        &self,
        _: &[&str],
        _: &str,
    ) -> Vec<men_sharp_semantics::types::ExternalTypeId> {
        Vec::new()
    }
}

pub fn external(id: ExternalTypeId) -> Type {
    Type::Named {
        target: TypeTarget::External(id),
        arguments: Vec::new(),
    }
}

pub fn member_signature(
    declarations: &Declarations,
    signatures: &Signatures,
    path: &str,
    member: &str,
) -> MemberSignature {
    let type_symbol = find(declarations, path);
    let member = declarations.table.symbol(type_symbol).members_named(member)[0];
    signatures.members.get(&member).unwrap().clone()
}

pub fn named_source(symbol: SymbolId, arguments: Vec<Type>) -> Type {
    Type::Named {
        target: TypeTarget::Source(symbol),
        arguments,
    }
}

macro_rules! checked {
    ($check:ident, $($source:expr),+ $(,)?) => {
        $crate::common::declarations!(declarations, $($source),+);
        let mock = $crate::common::MockExternal::corlib();
        let signatures = ::men_sharp_semantics::resolve_signatures(&declarations, &mock);
        assert_eq!(signatures.errors, vec![], "signatures must resolve cleanly");
        let mut $check = ::men_sharp_semantics::BodyCheck::default();
        for index in 0..declarations.files.len() {
            $check.merge(::men_sharp_semantics::check_file(
                &declarations,
                &signatures,
                &mock,
                index,
            ));
        }
    };
}

pub fn error_kinds(check: &men_sharp_semantics::BodyCheck) -> Vec<&SemanticErrorKind> {
    check.errors.iter().map(|error| &error.kind).collect()
}

pub(crate) use checked;
