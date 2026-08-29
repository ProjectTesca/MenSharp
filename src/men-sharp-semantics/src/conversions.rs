//! Implicit conversions, the C# spec's §10.2 in simplified form.
//!
//! [`TypeSystem::is_implicitly_convertible`] answers the one question the body
//! checker keeps asking: may a value of `from` flow into a slot of `to` without a
//! cast? Covered: identity, the implicit numeric table, reference conversions up
//! the inheritance and interface closure, boxing to `object`, `null` to anything
//! nullable, and `T -> T?`.
//!
//! [`Type::Error`] converts to and from everything — one diagnostic has already
//! been reported, and cascading follow-ups onto every use would bury it. `dynamic`
//! behaves the same by definition.
//!
//! Deliberately not here yet: generic variance (`IEnumerable<string>` to
//! `IEnumerable<object>`), user-defined implicit operators at conversion sites,
//! and constant-expression narrowing beyond the checker's integer-literal special
//! case. Each is additive when its time comes.

use crate::{
    external::ExternalTypeKind,
    lookup::TypeSystem,
    symbol::SymbolKind,
    types::{Type, TypeTarget},
};

/// The C# numeric types, for the promotion and conversion tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericKind {
    SByte,
    Byte,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Char,
    Single,
    Double,
    Decimal,
}

impl NumericKind {
    const NAMES: &'static [(&'static str, NumericKind)] = &[
        ("SByte", NumericKind::SByte),
        ("Byte", NumericKind::Byte),
        ("Int16", NumericKind::Int16),
        ("UInt16", NumericKind::UInt16),
        ("Int32", NumericKind::Int32),
        ("UInt32", NumericKind::UInt32),
        ("Int64", NumericKind::Int64),
        ("UInt64", NumericKind::UInt64),
        ("Char", NumericKind::Char),
        ("Single", NumericKind::Single),
        ("Double", NumericKind::Double),
        ("Decimal", NumericKind::Decimal),
    ];

    pub fn corlib_name(&self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(_, kind)| kind == self)
            .map(|(name, _)| *name)
            .unwrap()
    }

    pub fn is_integral(&self) -> bool {
        !matches!(
            self,
            NumericKind::Single | NumericKind::Double | NumericKind::Decimal
        )
    }
}

/// The implicit numeric conversions table (C# §10.2.3).
fn implicit_numeric(from: NumericKind, to: NumericKind) -> bool {
    use NumericKind::*;

    if from == to {
        return true;
    }

    let widened: &[NumericKind] = match from {
        SByte => &[Int16, Int32, Int64, Single, Double, Decimal],
        Byte => &[
            Int16, UInt16, Int32, UInt32, Int64, UInt64, Single, Double, Decimal,
        ],
        Int16 => &[Int32, Int64, Single, Double, Decimal],
        UInt16 => &[Int32, UInt32, Int64, UInt64, Single, Double, Decimal],
        Int32 => &[Int64, Single, Double, Decimal],
        UInt32 => &[Int64, UInt64, Single, Double, Decimal],
        Int64 => &[Single, Double, Decimal],
        UInt64 => &[Single, Double, Decimal],
        Char => &[
            UInt16, Int32, UInt32, Int64, UInt64, Single, Double, Decimal,
        ],
        Single => &[Double],
        Double | Decimal => &[],
    };
    widened.contains(&to)
}

impl TypeSystem<'_, '_> {
    /// Which numeric type this is, if it is one.
    pub fn numeric_kind(&self, ty: &Type) -> Option<NumericKind> {
        let Type::Named {
            target: TypeTarget::External(id),
            arguments,
        } = ty
        else {
            return None;
        };
        if !arguments.is_empty() {
            return None;
        }

        NumericKind::NAMES
            .iter()
            .find(|(name, _)| self.external.find_type(&["System"], name, 0) == Some(*id))
            .map(|(_, kind)| *kind)
    }

    pub fn is_bool(&self, ty: &Type) -> bool {
        matches!(ty, Type::Error | Type::Dynamic) || self.is_system_type(ty, "Boolean")
    }

    pub fn is_string(&self, ty: &Type) -> bool {
        self.is_system_type(ty, "String")
    }

    pub fn is_system_type(&self, ty: &Type, name: &str) -> bool {
        matches!(
            ty,
            Type::Named { target: TypeTarget::External(id), arguments }
                if arguments.is_empty()
                    && self.external.find_type(&["System"], name, 0) == Some(*id)
        )
    }

    pub fn is_enum_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => self.declarations.table.symbol(*symbol).kind == SymbolKind::Enum,
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.external.type_info(*id).kind == ExternalTypeKind::Enum,
            _ => false,
        }
    }

    /// Whether a value of this type lives on the heap behind a reference — which is
    /// what `null` can inhabit.
    pub fn is_reference_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Array { .. } | Type::Dynamic | Type::Error | Type::Null => true,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => matches!(
                self.declarations.table.symbol(*symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::Record
                    | SymbolKind::Delegate
            ),
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => matches!(
                self.external.type_info(*id).kind,
                ExternalTypeKind::Class | ExternalTypeKind::Interface | ExternalTypeKind::Delegate
            ),
            Type::TypeParameter(_) => false, // unconstrained: unknown, be strict
            _ => false,
        }
    }

    /// C# §10.2: is there an implicit conversion from `from` to `to`?
    pub fn is_implicitly_convertible(&self, from: &Type, to: &Type) -> bool {
        // recovery and dynamic swallow everything
        if matches!(from, Type::Error | Type::Dynamic) || matches!(to, Type::Error | Type::Dynamic)
        {
            return true;
        }

        // identity
        if from == to {
            return true;
        }

        // null literal into anything nullable
        if matches!(from, Type::Null) {
            return matches!(to, Type::Nullable(_))
                || self.is_reference_type(to)
                || self.is_nullable_of(to).is_some();
        }

        // T -> T? (written form or the underlying Nullable<T>)
        if let Type::Nullable(inner) = to {
            if self.is_reference_type(inner) {
                // a reference annotation: `string` -> `string?` and friends
                return self.is_implicitly_convertible(from, inner);
            }
            return self.is_implicitly_convertible(from, inner);
        }
        if let Some(inner) = self.is_nullable_of(to) {
            return self.is_implicitly_convertible(from, &inner);
        }
        // `string?` (annotation on a reference type) flows into `string`
        if let Type::Nullable(inner) = from
            && self.is_reference_type(inner)
        {
            return self.is_implicitly_convertible(inner, to);
        }

        // numeric widening
        if let (Some(from_kind), Some(to_kind)) = (self.numeric_kind(from), self.numeric_kind(to)) {
            return implicit_numeric(from_kind, to_kind);
        }

        // everything boxes or upcasts to object
        if self.is_system_type(to, "Object") {
            return !matches!(from, Type::Pointer(_) | Type::Void);
        }

        // reference conversion: `to` somewhere in `from`'s base/interface closure
        if matches!(from, Type::Named { .. }) && matches!(to, Type::Named { .. }) {
            return self.inheritance_closure_contains(from, to);
        }

        // arrays convert to System.Array (and through it, above, to object)
        if matches!(from, Type::Array { .. }) && self.is_system_type(to, "Array") {
            return true;
        }

        // a type parameter converts to its bounds (and whatever they convert to)
        if let Type::TypeParameter(symbol) = from {
            return self
                .signatures
                .constraints
                .get(symbol)
                .into_iter()
                .flatten()
                .any(|bound| bound == to || self.is_implicitly_convertible(bound, to));
        }

        false
    }

    /// `Nullable<T>` in its named form.
    fn is_nullable_of(&self, ty: &Type) -> Option<Type> {
        let Type::Named {
            target: TypeTarget::External(id),
            arguments,
        } = ty
        else {
            return None;
        };
        if arguments.len() == 1 && self.external.find_type(&["System"], "Nullable", 1) == Some(*id)
        {
            Some(arguments[0].clone())
        } else {
            None
        }
    }

    /// Walks base classes and interfaces (instantiated) looking for `wanted`.
    fn inheritance_closure_contains(&self, from: &Type, wanted: &Type) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut queue = vec![from.clone()];

        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if &current != from && &current == wanted {
                return true;
            }

            queue.extend(self.interfaces_of(&current));
            if let Some(base) = self.base_of(&current) {
                queue.push(base);
            }
        }

        false
    }

    /// A human-readable rendering for diagnostics.
    pub fn display(&self, ty: &Type) -> String {
        match ty {
            Type::Named { target, arguments } => {
                let name = match target {
                    TypeTarget::Source(symbol) => {
                        self.declarations.table.fully_qualified_name(*symbol)
                    }
                    TypeTarget::External(id) => self.external.display_name(*id),
                };
                if arguments.is_empty() {
                    name
                } else {
                    format!(
                        "{name}<{}>",
                        arguments
                            .iter()
                            .map(|argument| self.display(argument))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Type::TypeParameter(symbol) => self.declarations.table.symbol(*symbol).name.to_string(),
            Type::ExternalTypeParameter { index, .. } => format!("!{index}"),
            Type::ExternalMethodTypeParameter(index) => format!("!!{index}"),
            Type::Array { element, rank } => {
                format!(
                    "{}[{}]",
                    self.display(element),
                    ",".repeat(*rank as usize - 1)
                )
            }
            Type::Pointer(element) => format!("{}*", self.display(element)),
            Type::Nullable(element) => format!("{}?", self.display(element)),
            Type::ByRef { element, .. } => format!("ref {}", self.display(element)),
            Type::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| self.display(&element.element))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Dynamic => "dynamic".to_string(),
            Type::Void => "void".to_string(),
            Type::Null => "null".to_string(),
            Type::Infer => "var".to_string(),
            Type::Error => "?".to_string(),
        }
    }
}
