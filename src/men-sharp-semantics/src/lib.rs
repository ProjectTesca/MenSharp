//! Semantic analysis for MenSharp: from parsed files to a type for every
//! expression.
//!
//! The crate is in two halves, and the module tree says which is which.
//!
//! - [`semantics`] holds the phases, in the order the driver runs them:
//!   [`collect`](semantics::collect) reads one syntax tree,
//!   [`merge`](semantics::merge) makes one view of the whole compilation,
//!   [`resolve`](semantics::resolve) gives every member its signature, and
//!   [`check`](semantics::check) walks the bodies.
//! - [`types`] holds the type system those phases ask: what a [`Type`] is,
//!   what a member lookup finds ([`TypeSystem`]), what converts to what,
//!   and how a generic call is inferred.
//!
//! [`symbol`] and [`error`] sit above both: the identities everything names
//! things with, and the diagnostics everything reports.
//!
//! The phases read the type system freely; the type system reads the phases
//! in exactly one place — [`TypeSystem`] is a view assembled over
//! [`Declarations`] and [`Signatures`], which is what makes a member lookup
//! possible at all.
//!
//! Everything here is synchronous and single-threaded on purpose.
//! [`collect_file`] is a pure function of one syntax tree, so the compiler
//! driver may fan it out over however many threads its settings allow;
//! [`merge_declarations`] is the sequential barrier that makes the result
//! deterministic. This crate never learns which of the two happened.
//!
//! ```
//! use men_sharp_parser::MenSharpAST;
//! use men_sharp_semantics::{collect_file, merge_declarations, FileId};
//!
//! let ast = MenSharpAST::parse("namespace App { public class Program {} }");
//! let file = collect_file(FileId(0), ast.ast());
//! let declarations = merge_declarations(vec![file]);
//!
//! let root = declarations.table.root();
//! let app = declarations.table.symbol(root).members_named("App")[0];
//! assert_eq!(declarations.table.symbol(app).members_named("Program").len(), 1);
//! ```

pub mod error;
pub mod semantics;
pub mod symbol;
pub mod types;

pub use error::{SemanticError, SemanticErrorKind};
pub use semantics::check::{
    BodyCheck, ConstructorChain, ConstructorChainKind, ForeachEnumeration, ResolvedAwait,
    ResolvedCall, ResolvedMember, ResolvedTarget, check_file, uncompilable_foreign_members,
};
pub use semantics::collect::{
    DeclarationNode, FileDeclarations, MemberNode, NamespaceNode, TypeNode, collect_file,
};
pub use semantics::merge::{Declarations, SourceText, merge_declarations};
pub use semantics::resolve::{Signatures, apply_suffixes, resolve_file, resolve_signatures};
pub use symbol::{
    Accessibility, DeclarationSite, FileId, Symbol, SymbolId, SymbolKind, SymbolTable, SyntaxRef,
};
pub use types::conversions::{ConversionOperator, NumericKind};
pub use types::external::{
    ExternalConstant, ExternalMember, ExternalMemberKind, ExternalTypeInfo, ExternalTypeKind,
    ExternalTypes, NoExternalTypes,
};
pub use types::lookup::{MemberCandidate, MemberOrigin, TypeSystem};
pub use types::{
    DefaultArgument, ExternalTypeId, FunctionSignature, MemberSignature, ParameterPassing,
    ParameterSignature, TupleElement, Type, TypeTarget, TypeVariance, tuple_element_index,
};
