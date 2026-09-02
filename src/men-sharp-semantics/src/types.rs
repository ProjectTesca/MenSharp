//! The semantic type model.
//!
//! A [`Type`] is what a written type *means* once names are resolved: `int` becomes
//! the external `System.Int32`, `Player` becomes a source symbol, `List<Player>`
//! becomes a named type with an argument. Written spellings and spans stay in the
//! syntax tree; this model is fully owned (no lifetimes) so side tables of types can
//! be stored, merged across threads and kept after individual analyses finish.
//!
//! [`Type::Error`] is the recovery value: resolution reports one diagnostic and then
//! answers `Error`, which downstream phases treat as compatible-with-anything to
//! avoid error cascades — the same philosophy as the parser's holes.

use crate::symbol::SymbolId;

/// A type defined outside the compilation, in a referenced assembly. Opaque here:
/// only the [`crate::external::ExternalTypes`] provider can look inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExternalTypeId {
    /// Which referenced assembly, in the provider's numbering.
    pub assembly: u32,
    /// The type within it, in the provider's numbering.
    pub type_index: u32,
}

/// Declaration-site variance of a generic parameter (`out T` / `in T`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TypeVariance {
    #[default]
    Invariant,
    /// `out T`.
    Covariant,
    /// `in T`.
    Contravariant,
}

/// What a resolved type name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeTarget {
    Source(SymbolId),
    External(ExternalTypeId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// A class, struct, interface, enum or delegate, with any generic arguments.
    /// For a nested type inside a generic type the outer arguments come first,
    /// mirroring metadata.
    Named {
        target: TypeTarget,
        arguments: Vec<Type>,
    },
    /// A generic parameter, by its symbol in the table.
    TypeParameter(SymbolId),
    /// A generic parameter of an external type, by position (metadata `!0`).
    ExternalTypeParameter {
        owner: ExternalTypeId,
        index: u32,
    },
    /// A generic parameter of the external method a signature belongs to, by
    /// position (metadata `!!0`). Only meaningful inside that method's signature.
    ExternalMethodTypeParameter(u32),
    Array {
        element: Box<Type>,
        rank: u32,
    },
    Pointer(Box<Type>),
    /// Written `T?`. Whether that means `Nullable<T>` or a reference annotation
    /// depends on `T`, which is a later phase's question; the spelling is kept.
    Nullable(Box<Type>),
    /// `ref T` in a return type or parameter.
    ByRef {
        readonly: bool,
        element: Box<Type>,
    },
    Tuple(Vec<TupleElement>),
    Dynamic,
    Void,
    /// The type of the `null` literal: convertible to any reference or nullable
    /// type, never the type of anything at runtime.
    Null,
    /// `var` — to be filled in by type inference.
    Infer,
    /// Resolution failed; a diagnostic has been reported.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TupleElement {
    pub name: Option<Box<str>>,
    pub element: Type,
}

/// The resolved signature of a callable: methods, constructors, operators,
/// indexers, delegates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    pub return_type: Type,
    pub parameters: Vec<ParameterSignature>,
}

#[derive(Debug, Clone)]
pub struct ParameterSignature {
    pub passing: ParameterPassing,
    /// `params` on the last parameter.
    pub is_params: bool,
    pub parameter_type: Type,
    /// The declared name, for named arguments (`F(b: 1, a: 2)`). Not part
    /// of signature identity: an override may rename its parameters.
    pub name: Option<String>,
    /// `= value`: the argument a call may leave out. Not part of signature
    /// identity either.
    pub default_value: Option<DefaultArgument>,
}

impl PartialEq for ParameterSignature {
    fn eq(&self, other: &Self) -> bool {
        self.passing == other.passing
            && self.is_params == other.is_params
            && self.parameter_type == other.parameter_type
    }
}

impl Eq for ParameterSignature {}

/// What an omitted optional argument is: baked into the call site, as C#
/// does (§12.6.2.2).
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultArgument {
    /// A metadata constant (`int count = -1`, an enum's member as its
    /// underlying value, ...).
    Constant(crate::external::ExternalConstant),
    /// `= null`.
    Null,
    /// `= default`, or `[Optional]` with no value.
    Default,
    /// Declared in source: the parameter's own `= expression`, evaluated
    /// at each call site.
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterPassing {
    Value,
    Ref,
    Out,
    In,
}

/// The resolved type side of one member symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberSignature {
    Field(Type),
    Property(Type),
    Event(Type),
    Function(FunctionSignature),
}

impl Type {
    /// Rebuilds this type with `replace` applied at every position, leaves first.
    /// `replace` sees each rebuilt node and may swap it for something else; this is
    /// the primitive behind generic substitution.
    pub fn map(&self, replace: &impl Fn(Type) -> Type) -> Type {
        let rebuilt = match self {
            Type::Named { target, arguments } => Type::Named {
                target: *target,
                arguments: arguments
                    .iter()
                    .map(|argument| argument.map(replace))
                    .collect(),
            },
            Type::Array { element, rank } => Type::Array {
                element: Box::new(element.map(replace)),
                rank: *rank,
            },
            Type::Pointer(element) => Type::Pointer(Box::new(element.map(replace))),
            Type::Nullable(element) => Type::Nullable(Box::new(element.map(replace))),
            Type::ByRef { readonly, element } => Type::ByRef {
                readonly: *readonly,
                element: Box::new(element.map(replace)),
            },
            Type::Tuple(elements) => Type::Tuple(
                elements
                    .iter()
                    .map(|element| TupleElement {
                        name: element.name.clone(),
                        element: element.element.map(replace),
                    })
                    .collect(),
            ),
            other => other.clone(),
        };
        replace(rebuilt)
    }
}

impl MemberSignature {
    pub fn map(&self, replace: &impl Fn(Type) -> Type) -> MemberSignature {
        match self {
            MemberSignature::Field(field) => MemberSignature::Field(field.map(replace)),
            MemberSignature::Property(property) => MemberSignature::Property(property.map(replace)),
            MemberSignature::Event(event) => MemberSignature::Event(event.map(replace)),
            MemberSignature::Function(function) => MemberSignature::Function(FunctionSignature {
                return_type: function.return_type.map(replace),
                parameters: function
                    .parameters
                    .iter()
                    .map(|parameter| ParameterSignature {
                        passing: parameter.passing,
                        is_params: parameter.is_params,
                        parameter_type: parameter.parameter_type.map(replace),
                        name: parameter.name.clone(),
                        default_value: parameter.default_value.clone(),
                    })
                    .collect(),
            }),
        }
    }
}
