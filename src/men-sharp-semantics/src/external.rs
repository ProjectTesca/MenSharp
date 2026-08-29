//! The boundary to types this compilation never parsed.
//!
//! Name resolution has to see `UnityEngine.Debug` and `System.Int32`, but where
//! they come from — parsed dlls, a bundled table, a test mock — is not this crate's
//! business. The compiler driver hands resolution an [`ExternalTypes`]
//! implementation; everything semantic stays behind these few questions.
//!
//! Namespaces are passed as segment slices (`["System", "Collections", "Generic"]`)
//! because that is the shape resolution naturally has in hand while walking scopes.

use crate::types::ExternalTypeId;

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
}

/// A provider with no types at all. Resolution still works for purely
/// self-contained source; every predefined type comes out as [`crate::types::Type::Error`].
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
}
