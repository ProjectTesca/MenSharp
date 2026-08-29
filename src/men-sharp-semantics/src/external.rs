//! The boundary to types this compilation never parsed.
//!
//! Name resolution has to see `UnityEngine.Debug` and `System.Int32`, and member
//! lookup has to see what they contain, but where that knowledge comes from —
//! parsed dlls, a bundled table, a test mock — is not this crate's business. The
//! compiler driver hands analysis an [`ExternalTypes`] implementation; everything
//! semantic stays behind these questions.
//!
//! Answers use the semantic [`Type`] model directly: an external `List<T>` reports
//! its members with [`Type::ExternalTypeParameter`] in place of `T`, and the member
//! lookup layer substitutes real arguments in, exactly as it does for source types.
//!
//! Namespaces are passed as segment slices (`["System", "Collections", "Generic"]`)
//! because that is the shape resolution naturally has in hand while walking scopes.

use crate::{
    symbol::Accessibility,
    types::{ExternalTypeId, MemberSignature, Type},
};

/// What kind of thing an external type is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalTypeKind {
    Class,
    Struct,
    Interface,
    Enum,
    Delegate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalTypeInfo {
    pub kind: ExternalTypeKind,
    /// Generic parameter count (outer types' parameters included, as metadata does).
    pub arity: u32,
    pub is_sealed: bool,
    pub is_abstract: bool,
}

/// One member of an external type. Signatures use the defining type's own generic
/// parameters ([`Type::ExternalTypeParameter`]) — instantiation is the caller's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalMember {
    /// The metadata name: `.ctor`, `op_Addition` and friends follow the same
    /// conventions as source symbols.
    pub name: String,
    pub kind: ExternalMemberKind,
    pub is_static: bool,
    pub accessibility: Accessibility,
    pub signature: MemberSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalMemberKind {
    Field,
    /// `type_parameter_count` is the method's own (`!!n`) parameters.
    Method {
        type_parameter_count: u32,
    },
    Property {
        has_getter: bool,
        has_setter: bool,
    },
    Event,
    Constructor,
}

pub trait ExternalTypes: Sync {
    /// A top-level type by namespace path, C# name (no arity suffix) and generic
    /// arity. `find_type(&["System"], "Int32", 0)`, `find_type(&["System",
    /// "Collections", "Generic"], "List", 1)`.
    fn find_type(&self, namespace: &[&str], name: &str, arity: u32) -> Option<ExternalTypeId>;

    /// A type nested inside another external type.
    fn find_nested_type(
        &self,
        parent: ExternalTypeId,
        name: &str,
        arity: u32,
    ) -> Option<ExternalTypeId>;

    /// Whether any referenced assembly declares something under this namespace
    /// path, so `using UnityEngine;` and the `UnityEngine` in `UnityEngine.Debug`
    /// resolve even though a namespace is not itself a type.
    fn namespace_exists(&self, namespace: &[&str]) -> bool;

    /// Shape facts about a type this provider handed out.
    fn type_info(&self, id: ExternalTypeId) -> ExternalTypeInfo;

    /// The base class, in this type's own generic parameters. `None` for
    /// `System.Object`, interfaces and anything unresolvable.
    fn base_type(&self, id: ExternalTypeId) -> Option<Type>;

    /// Implemented (or, for an interface, extended) interfaces.
    fn interfaces(&self, id: ExternalTypeId) -> Vec<Type>;

    /// All members with the given metadata name, overloads included.
    fn members_named(&self, id: ExternalTypeId, name: &str) -> Vec<ExternalMember>;

    /// A human-readable name (`UnityEngine.Debug`) for diagnostics.
    fn display_name(&self, id: ExternalTypeId) -> String;
}

/// A provider with no types at all. Resolution still works for purely
/// self-contained source; every predefined type comes out as [`Type::Error`].
pub struct NoExternalTypes;

impl ExternalTypes for NoExternalTypes {
    fn find_type(&self, _: &[&str], _: &str, _: u32) -> Option<ExternalTypeId> {
        None
    }

    fn find_nested_type(&self, _: ExternalTypeId, _: &str, _: u32) -> Option<ExternalTypeId> {
        None
    }

    fn namespace_exists(&self, _: &[&str]) -> bool {
        false
    }

    fn type_info(&self, _: ExternalTypeId) -> ExternalTypeInfo {
        ExternalTypeInfo {
            kind: ExternalTypeKind::Class,
            arity: 0,
            is_sealed: false,
            is_abstract: false,
        }
    }

    fn base_type(&self, _: ExternalTypeId) -> Option<Type> {
        None
    }

    fn interfaces(&self, _: ExternalTypeId) -> Vec<Type> {
        Vec::new()
    }

    fn members_named(&self, _: ExternalTypeId, _: &str) -> Vec<ExternalMember> {
        Vec::new()
    }

    fn display_name(&self, id: ExternalTypeId) -> String {
        format!("<external {}:{}>", id.assembly, id.type_index)
    }
}
