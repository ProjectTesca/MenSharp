//! Declaration-level semantic errors.
//!
//! Same shape as the parser's errors: a kind plus a span, with rendering left to the
//! diagnostic layer. The file is part of the error because, unlike a parse error, a
//! semantic error can involve more than one file — the conflicting declaration is
//! carried alongside so the renderer can point at both sites.

use std::ops::Range;

use crate::symbol::FileId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticError {
    pub kind: SemanticErrorKind,
    pub file: FileId,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticErrorKind {
    /// A second definition of a type that is not `partial` on both sides.
    DuplicateTypeDefinition {
        first_file: FileId,
        first_span: Range<usize>,
    },
    /// `partial class Foo` in one place, `partial struct Foo` in another.
    PartialKindMismatch {
        first_file: FileId,
        first_span: Range<usize>,
    },
    /// A namespace and a type with the same name in the same container.
    TypeNamespaceConflict {
        first_file: FileId,
        first_span: Range<usize>,
    },
    /// Two members share a name and are not overloads of one another.
    DuplicateMemberName {
        first_file: FileId,
        first_span: Range<usize>,
    },
    /// `class C<T, T>`.
    DuplicateTypeParameter {
        first_file: FileId,
        first_span: Range<usize>,
    },
    /// No type or namespace with this name (and arity) is in scope.
    UnresolvedTypeName,
    /// Two `using` imports supply different types under the same name.
    AmbiguousTypeName,
    /// A namespace name where a type was required.
    NamespaceUsedAsType,
    /// The target of a `using` directive does not exist.
    UnresolvedUsingTarget,
    // ---------- body checking ----------
    /// A simple name that is neither a local, a member, a type nor a namespace.
    UnknownIdentifier,
    /// `receiver.Name` where the receiver's type has no such member.
    UnknownMember {
        type_name: String,
    },
    /// Something that is not a method or delegate was called.
    NotCallable {
        type_name: String,
    },
    /// No overload accepts these arguments.
    NoMatchingOverload,
    /// More than one overload fits equally well.
    AmbiguousOverload,
    /// Generic method type arguments could not be inferred from the arguments;
    /// writing them explicitly avoids this error.
    CannotInferTypeArguments,
    /// The type cannot be worked out here; writing it explicitly avoids this error.
    TypeAnnotationNeeded,
    /// No implicit conversion from `found` to `expected`.
    TypeMismatch {
        expected: String,
        found: String,
    },
    /// An `if`/`while`/`for` condition that is not `bool`.
    ConditionNotBoolean {
        found: String,
    },
    /// No built-in or user-defined operator takes these operands.
    InvalidOperator {
        left: String,
        right: Option<String>,
    },
    /// An instance member used where no instance exists.
    InstanceMemberInStaticContext,
    /// A static member accessed through an instance.
    StaticMemberViaInstance,
    /// A type name in a position that needs a value.
    TypeUsedAsValue,
    /// A namespace name in a position that needs a value.
    NamespaceUsedAsValue,
    /// Indexing something that has no indexer.
    NotIndexable {
        type_name: String,
    },
    /// `foreach` over something with no element type.
    NotEnumerable {
        type_name: String,
    },
    /// `return` with a value in a `void` member, or without one elsewhere.
    ReturnValueMismatch,
    /// A lambda whose parameter list does not fit the target delegate.
    LambdaParameterMismatch,
    /// A construct the checker does not handle yet. Temporary scaffolding: each of
    /// these becomes a real implementation or a precise "unsupported on Udon"
    /// diagnostic as the checker grows.
    UnsupportedExpression,
    UnsupportedStatement,
}
