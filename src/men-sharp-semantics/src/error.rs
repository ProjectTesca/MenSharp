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
}
