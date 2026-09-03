//! Member lookup: what does `receiver.Name` mean?
//!
//! This is the service expression type checking will sit on. Given a resolved
//! [`Type`] and a member name, [`TypeSystem::members_named`] walks the inheritance
//! chain — through source symbols and referenced assemblies alike — and returns
//! every candidate with its signature **instantiated for the receiver**: asking
//! `List<int>` for `Add` yields `(int) -> void`, not `(T) -> void`.
//!
//! Lookup rules, simplified from the C# spec:
//! - a class walks itself, then its base classes up to `System.Object`;
//! - an interface walks itself, its transitive interfaces, then `System.Object`;
//! - a type parameter walks its constraint bounds, then `System.Object`;
//! - arrays look up in `System.Array`, `T?` in `System.Nullable<T>`.
//!
//! Candidates come back nearest-first (the derived type's members before the base's),
//! with hidden members removed per §7.7.1.2: an override's base declaration, a
//! same-signature base method, or any base member shadowed by a nearer non-method.
//! Overload resolution and accessibility checks belong to the caller, which has
//! the context this layer does not.
//!
//! Everything here is read-only over the phase outputs, so the driver may call it
//! from any number of threads at once.

use std::collections::HashSet;

use men_sharp_parser::ast::EntityID;

use crate::{
    external::{ExternalMember, ExternalMemberKind, ExternalTypeKind, ExternalTypes},
    merge::Declarations,
    resolve::Signatures,
    symbol::{Accessibility, SymbolId, SymbolKind},
    types::{ExternalTypeId, MemberSignature, Type, TypeTarget},
};

/// One thing `receiver.name` might refer to.
#[derive(Debug, Clone)]
pub struct MemberCandidate {
    pub origin: MemberOrigin,
    pub kind: SymbolKind,
    pub is_static: bool,
    pub accessibility: Accessibility,
    /// The member's own generic parameter count (generic methods).
    pub arity: u32,
    /// Instantiated for the receiver. `None` when the phase produced no signature
    /// for it (enum members, whose type is the enum itself).
    pub signature: Option<MemberSignature>,
    /// The type in the chain that declared this member, receiver-instantiated.
    pub declaring_type: Type,
}

#[derive(Debug, Clone)]
pub enum MemberOrigin {
    Source(SymbolId),
    External {
        owner: ExternalTypeId,
        member: ExternalMember,
    },
    /// A local function, by its declaration node. It has no symbol: it
    /// belongs to a body, not to a type.
    LocalFunction(EntityID),
}

/// A read-only view over everything the earlier phases produced.
pub struct TypeSystem<'a, 'ast> {
    pub declarations: &'a Declarations<'ast>,
    pub signatures: &'a Signatures,
    pub external: &'a dyn ExternalTypes,
}

impl TypeSystem<'_, '_> {
    /// Every member candidate for `receiver.name`, nearest declaring type first.
    pub fn members_named(&self, receiver: &Type, name: &str) -> Vec<MemberCandidate> {
        let mut out = Vec::new();
        let mut visited: HashSet<Type> = HashSet::new();
        let mut queue: Vec<Type> = self.lookup_roots(receiver);

        while !queue.is_empty() {
            let current = queue.remove(0);
            if !visited.insert(current.clone()) {
                continue;
            }

            self.collect_at(&current, name, &mut out);

            if let Type::Named { .. } = &current {
                if self.is_interface(&current) {
                    queue.extend(self.interfaces_of(&current));
                    if let Some(object) = self.well_known("Object") {
                        queue.push(object);
                    }
                } else if let Some(base) = self.base_of(&current) {
                    queue.push(base);
                }
            }
        }

        remove_hidden(out)
    }

    /// Where lookup starts for each shape of receiver.
    fn lookup_roots(&self, receiver: &Type) -> Vec<Type> {
        match receiver {
            Type::Named { .. } => vec![receiver.clone()],
            Type::Nullable(inner) => match self.find_generic("Nullable", 1) {
                // `T?` on a value type means Nullable<T>; reference annotations
                // just look up on the type itself, which the checker sorts out
                Some(target) => vec![
                    Type::Named {
                        target,
                        arguments: vec![(**inner).clone()],
                    },
                    (**inner).clone(),
                ],
                None => vec![(**inner).clone()],
            },
            Type::Array { .. } => self.well_known("Array").into_iter().collect(),
            Type::TypeParameter(symbol) => {
                let mut roots: Vec<Type> = self
                    .signatures
                    .constraints
                    .get(symbol)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                roots.extend(self.well_known("Object"));
                roots
            }
            Type::ByRef { element, .. } => self.lookup_roots(element),
            // dynamic defers everything; Error stays quiet; nothing else has members
            _ => Vec::new(),
        }
    }

    /// The base class of a type, instantiated with its arguments. `None` for
    /// `System.Object`, interfaces, and non-class shapes.
    pub fn base_of(&self, ty: &Type) -> Option<Type> {
        let Type::Named { target, arguments } = ty else {
            return None;
        };

        match target {
            TypeTarget::Source(symbol) => {
                let written = self
                    .signatures
                    .base_types
                    .get(symbol)
                    .into_iter()
                    .flatten()
                    // the base list mixes the base class and interfaces
                    .find(|base| !self.is_interface(base))
                    .cloned();
                let written = written.map(|base| self.instantiate(&base, target, arguments));

                written.or_else(|| {
                    // no written base: C# fills one in per kind
                    let default = match self.declarations.table.symbol(*symbol).kind {
                        SymbolKind::Struct | SymbolKind::RecordStruct => "ValueType",
                        SymbolKind::Enum => "Enum",
                        SymbolKind::Delegate => "MulticastDelegate",
                        SymbolKind::Interface => return None,
                        _ => "Object",
                    };
                    if self.is_well_known(ty, "Object") {
                        return None;
                    }
                    self.well_known(default)
                })
            }
            TypeTarget::External(id) => self
                .external
                .base_type(*id)
                .map(|base| self.instantiate(&base, target, arguments)),
        }
    }

    /// Direct interfaces of a named type, receiver-instantiated.
    pub fn interfaces_of(&self, ty: &Type) -> Vec<Type> {
        let Type::Named { target, arguments } = ty else {
            return Vec::new();
        };

        let raw = match target {
            TypeTarget::Source(symbol) => self
                .signatures
                .base_types
                .get(symbol)
                .into_iter()
                .flatten()
                .filter(|base| self.is_interface(base))
                .cloned()
                .collect(),
            TypeTarget::External(id) => self.external.interfaces(*id),
        };

        raw.iter()
            .map(|interface| self.instantiate(interface, target, arguments))
            .collect()
    }

    pub fn is_interface(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => self.declarations.table.symbol(*symbol).kind == SymbolKind::Interface,
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.external.type_info(*id).kind == ExternalTypeKind::Interface,
            _ => false,
        }
    }

    // ------------------------------------------------------------ collection

    fn collect_at(&self, ty: &Type, name: &str, out: &mut Vec<MemberCandidate>) {
        let Type::Named { target, arguments } = ty else {
            return;
        };

        match target {
            TypeTarget::Source(type_symbol) => {
                for &id in self
                    .declarations
                    .table
                    .symbol(*type_symbol)
                    .members_named(name)
                {
                    let member = self.declarations.table.symbol(id);
                    if member.is_explicit_implementation || member.kind == SymbolKind::Namespace {
                        continue;
                    }

                    let signature =
                        self.signatures.members.get(&id).map(|signature| {
                            self.instantiate_signature(signature, target, arguments)
                        });

                    out.push(MemberCandidate {
                        origin: MemberOrigin::Source(id),
                        kind: member.kind,
                        is_static: member.is_static,
                        accessibility: member.accessibility,
                        arity: member.arity,
                        signature,
                        declaring_type: ty.clone(),
                    });
                }
            }
            TypeTarget::External(id) => {
                for member in self.external.members_named(*id, name) {
                    let signature =
                        self.instantiate_signature(&member.signature, target, arguments);

                    out.push(MemberCandidate {
                        kind: match member.kind {
                            ExternalMemberKind::Field => SymbolKind::Field,
                            ExternalMemberKind::Method { .. } => SymbolKind::Method,
                            ExternalMemberKind::Property { .. } => SymbolKind::Property,
                            ExternalMemberKind::Event => SymbolKind::Event,
                            ExternalMemberKind::Constructor => SymbolKind::Constructor,
                        },
                        is_static: member.is_static,
                        accessibility: member.accessibility,
                        arity: match member.kind {
                            ExternalMemberKind::Method {
                                type_parameter_count,
                            } => type_parameter_count,
                            _ => 0,
                        },
                        signature: Some(signature),
                        declaring_type: ty.clone(),
                        origin: MemberOrigin::External { owner: *id, member },
                    });
                }
            }
        }
    }

    // --------------------------------------------------------- substitution

    /// Replaces the generic parameters `owner` declares with `arguments`.
    fn instantiate(&self, ty: &Type, owner: &TypeTarget, arguments: &[Type]) -> Type {
        if arguments.is_empty() {
            return ty.clone();
        }

        match owner {
            TypeTarget::Source(symbol) => {
                let parameters = self.source_type_parameters(*symbol);
                ty.map(&|node| match node {
                    Type::TypeParameter(parameter) => parameters
                        .iter()
                        .position(|&candidate| candidate == parameter)
                        .and_then(|position| arguments.get(position))
                        .cloned()
                        .unwrap_or(Type::TypeParameter(parameter)),
                    other => other,
                })
            }
            TypeTarget::External(id) => {
                let id = *id;
                ty.map(&|node| match node {
                    Type::ExternalTypeParameter { owner, index } if owner == id => arguments
                        .get(index as usize)
                        .cloned()
                        .unwrap_or(Type::ExternalTypeParameter { owner, index }),
                    other => other,
                })
            }
        }
    }

    pub(crate) fn instantiate_signature(
        &self,
        signature: &MemberSignature,
        owner: &TypeTarget,
        arguments: &[Type],
    ) -> MemberSignature {
        if arguments.is_empty() {
            return signature.clone();
        }

        match owner {
            TypeTarget::Source(symbol) => {
                let parameters = self.source_type_parameters(*symbol);
                signature.map(&|node| match node {
                    Type::TypeParameter(parameter) => parameters
                        .iter()
                        .position(|&candidate| candidate == parameter)
                        .and_then(|position| arguments.get(position))
                        .cloned()
                        .unwrap_or(Type::TypeParameter(parameter)),
                    other => other,
                })
            }
            TypeTarget::External(id) => {
                let id = *id;
                signature.map(&|node| match node {
                    Type::ExternalTypeParameter { owner, index } if owner == id => arguments
                        .get(index as usize)
                        .cloned()
                        .unwrap_or(Type::ExternalTypeParameter { owner, index }),
                    other => other,
                })
            }
        }
    }

    /// The generic parameters a source type binds, enclosing types' first — the
    /// same order [`Type::Named`] arguments use.
    fn source_type_parameters(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let mut chain = Vec::new();
        let mut current = Some(symbol);
        while let Some(id) = current {
            let entry = self.declarations.table.symbol(id);
            if entry.kind.is_type() {
                chain.push(id);
            }
            current = entry.parent;
        }

        let mut parameters = Vec::new();
        for &id in chain.iter().rev() {
            parameters.extend(&self.declarations.table.symbol(id).type_parameters);
        }
        parameters
    }

    // ------------------------------------------------------------- helpers

    fn well_known(&self, name: &str) -> Option<Type> {
        self.external
            .find_type(&["System"], name, 0)
            .map(|id| Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            })
    }

    fn find_generic(&self, name: &str, arity: u32) -> Option<TypeTarget> {
        self.external
            .find_type(&["System"], name, arity)
            .map(TypeTarget::External)
    }

    fn is_well_known(&self, ty: &Type, name: &str) -> bool {
        match (ty, self.external.find_type(&["System"], name, 0)) {
            (
                Type::Named {
                    target: TypeTarget::External(id),
                    ..
                },
                Some(expected),
            ) => *id == expected,
            _ => false,
        }
    }
}

/// C# hiding (§7.7.1.2), applied to a nearest-first candidate list: a member
/// hides base-class members of the same name — methods hide by signature
/// (which also removes the base declarations of overrides), everything else
/// hides by name. Members of the same declaring type never hide each other.
fn remove_hidden(candidates: Vec<MemberCandidate>) -> Vec<MemberCandidate> {
    let mut kept: Vec<MemberCandidate> = Vec::new();
    'candidates: for candidate in candidates {
        for nearer in &kept {
            if nearer.declaring_type != candidate.declaring_type && hides(nearer, &candidate) {
                continue 'candidates;
            }
        }
        kept.push(candidate);
    }
    kept
}

fn hides(nearer: &MemberCandidate, farther: &MemberCandidate) -> bool {
    let nearer_is_method = matches!(nearer.kind, SymbolKind::Method);
    let farther_is_method = matches!(farther.kind, SymbolKind::Method);
    if !nearer_is_method || !farther_is_method {
        // non-methods hide by name; a method hides non-method base members too
        return true;
    }
    if nearer.arity != farther.arity {
        return false;
    }
    match (&nearer.signature, &farther.signature) {
        (Some(MemberSignature::Function(a)), Some(MemberSignature::Function(b))) => {
            a.parameters.len() == b.parameters.len()
                && a.parameters
                    .iter()
                    .zip(&b.parameters)
                    .all(|(x, y)| x.passing == y.passing && x.parameter_type == y.parameter_type)
        }
        _ => false,
    }
}
