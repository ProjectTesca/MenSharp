//! Declaration-level semantic errors.
//!
//! Same shape as the parser's errors: a kind plus a span, with rendering left to the
//! diagnostic layer. The file is part of the error because, unlike a parse error, a
//! semantic error can involve more than one file — the conflicting declaration is
//! carried alongside so the renderer can point at both sites.

use std::ops::Range;

use men_sharp_diagnostics::{Hint, Label, Message};

use crate::symbol::FileId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticError {
    pub kind: SemanticErrorKind,
    pub file: FileId,
    pub span: Range<usize>,
    /// Suggestions the phase that found the error could make; see the
    /// `men-sharp-diagnostics` crate.
    pub hints: Vec<Hint>,
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
    /// `new` of an abstract class (CS0144).
    CannotInstantiateAbstractType {
        type_name: String,
    },
    /// A concrete type leaves an abstract or interface member unimplemented
    /// (CS0534 / CS0535).
    MissingImplementation {
        type_name: String,
        member: String,
    },
    /// `throw x` where `x` is not a `System.Exception` (CS0155).
    ThrowNeedsException {
        type_name: String,
    },
    /// A bare `throw;` outside a `catch` block (CS0156).
    RethrowOutsideCatch,
    /// `x is _`: a discard is not a pattern on its own (C# reads `_` as a
    /// type name there and fails to find it).
    DiscardIsNotAPattern,
    /// `==` without `!=`, `<` without `>`, `<=` without `>=` (CS0216).
    OperatorRequiresPair {
        operator: String,
        missing: String,
    },
    /// A constructor (written or implicit) has to call a base constructor,
    /// and none takes the arguments given — for the implicit `base()`, none
    /// takes zero (CS7036 / CS1729).
    NoMatchingBaseConstructor {
        type_name: String,
    },
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
    /// `a[i]` on an `int[,]`, or `a[i, j]` on an `int[]` (CS0022).
    WrongNumberOfIndices {
        expected: u32,
    },
    /// `{ { 1, 2 }, { 3 } }` for an `int[,]`: every row of a rectangular
    /// array initializer has the same length (CS0847).
    RaggedArrayInitializer,
    /// `foreach` over something with no element type.
    NotEnumerable {
        type_name: String,
    },
    /// A switch expression over a `bool`, an enum or a `[Union]` type — or
    /// a switch statement over a `[Union]` — with a case no arm handles.
    NonExhaustiveSwitch {
        subject: String,
        missing: Vec<String>,
    },
    /// `[Union]` on something other than an abstract class or an interface.
    UnionNotAbstract,
    /// `x with { ... }` on something that is not a record or a struct.
    WithNeedsRecord {
        type_name: String,
    },
    /// A type below a `[Union]` whose type arguments the union's do not fix
    /// (`class Weird<U> : Option<int>`): its cases cannot be listed.
    UnionCaseUndetermined {
        case: String,
    },
    /// `return` with a value in a `void` member, or without one elsewhere.
    ReturnValueMismatch,
    /// A lambda whose parameter list does not fit the target delegate.
    LambdaParameterMismatch,
    /// `void F<T>()` written inside a body: Udon has no way to pick a
    /// compiled instance per set of type arguments at a local call site, so
    /// a local function cannot be generic. A generic method of the type
    /// does the same job.
    GenericLocalFunction,
    /// A `static` local function used a variable of the enclosing body
    /// (CS8421): drop the `static`, or pass the value as a parameter.
    StaticLocalFunctionCapture {
        name: String,
    },
    /// A local function used a variable of the enclosing body written below
    /// it (CS0841). Local functions are visible throughout their block;
    /// variables begin where they are declared, so move the declaration
    /// above the function.
    LocalUsedBeforeDeclaration {
        name: String,
    },
    /// `await` outside an `async` method, lambda or local function
    /// (CS4032/CS4033).
    AwaitOutsideAsync,
    /// `await x` where `x` has no `GetAwaiter()` giving an awaiter with
    /// `IsCompleted`, `OnCompleted(Action)` and `GetResult()` (CS1061).
    NotAwaitable {
        type_name: String,
    },
    /// An `async` method, lambda or local function whose return type is
    /// neither `void`, `Task` nor `Task<T>` (CS1983).
    AsyncReturnType {
        type_name: String,
    },
    /// `ref`/`out`/`in` parameters on an `async` method (CS1988).
    AsyncByRefParameter,
    /// `yield` in a lambda, or in a member whose return type is not
    /// `IEnumerable<T>`/`IEnumerator<T>` (CS1621/CS1624).
    YieldOutsideIterator,
    /// `yield` where C# does not allow one: a `try` block that has a
    /// `catch`, a `catch` block, or a `finally` block
    /// (CS1626/CS1631/CS1625). `region` names which.
    YieldInsideTry {
        region: String,
    },
    /// `return value;` in an iterator (CS1622).
    ReturnInIterator,
    /// A construct the checker does not handle yet. Temporary scaffolding: each of
    /// these becomes a real implementation or a precise "unsupported on Udon"
    /// diagnostic as the checker grows.
    UnsupportedExpression,
    UnsupportedStatement,
}

impl SemanticErrorKind {
    /// The message, as a catalog key with its arguments.
    pub fn message(&self) -> Message {
        match self {
            SemanticErrorKind::DuplicateTypeDefinition { .. } => {
                Message::key("semantics.duplicate_type_definition")
            }
            SemanticErrorKind::PartialKindMismatch { .. } => {
                Message::key("semantics.partial_kind_mismatch")
            }
            SemanticErrorKind::TypeNamespaceConflict { .. } => {
                Message::key("semantics.type_namespace_conflict")
            }
            SemanticErrorKind::DuplicateMemberName { .. } => {
                Message::key("semantics.duplicate_member_name")
            }
            SemanticErrorKind::DuplicateTypeParameter { .. } => {
                Message::key("semantics.duplicate_type_parameter")
            }
            SemanticErrorKind::UnresolvedTypeName => Message::key("semantics.unresolved_type_name"),
            SemanticErrorKind::AmbiguousTypeName => Message::key("semantics.ambiguous_type_name"),
            SemanticErrorKind::NamespaceUsedAsType => {
                Message::key("semantics.namespace_used_as_type")
            }
            SemanticErrorKind::UnresolvedUsingTarget => {
                Message::key("semantics.unresolved_using_target")
            }
            SemanticErrorKind::UnknownIdentifier => Message::key("semantics.unknown_identifier"),
            SemanticErrorKind::UnknownMember { type_name } => {
                Message::key("semantics.unknown_member").arg("type_name", type_name)
            }
            SemanticErrorKind::NotCallable { type_name } => {
                Message::key("semantics.not_callable").arg("type_name", type_name)
            }
            SemanticErrorKind::NoMatchingOverload => Message::key("semantics.no_matching_overload"),
            SemanticErrorKind::CannotInstantiateAbstractType { type_name } => {
                Message::key("semantics.cannot_instantiate_abstract_type")
                    .arg("type_name", type_name)
            }
            SemanticErrorKind::MissingImplementation { type_name, member } => {
                Message::key("semantics.missing_implementation")
                    .arg("type_name", type_name)
                    .arg("member", member)
            }
            SemanticErrorKind::ThrowNeedsException { type_name } => {
                Message::key("semantics.throw_needs_exception").arg("type_name", type_name)
            }
            SemanticErrorKind::RethrowOutsideCatch => {
                Message::key("semantics.rethrow_outside_catch")
            }
            SemanticErrorKind::DiscardIsNotAPattern => {
                Message::key("semantics.discard_is_not_a_pattern")
            }
            SemanticErrorKind::OperatorRequiresPair { operator, missing } => {
                Message::key("semantics.operator_requires_pair")
                    .arg("operator", operator)
                    .arg("missing", missing)
            }
            SemanticErrorKind::NoMatchingBaseConstructor { type_name } => {
                Message::key("semantics.no_matching_base_constructor").arg("type_name", type_name)
            }
            SemanticErrorKind::AmbiguousOverload => Message::key("semantics.ambiguous_overload"),
            SemanticErrorKind::CannotInferTypeArguments => {
                Message::key("semantics.cannot_infer_type_arguments")
            }
            SemanticErrorKind::TypeAnnotationNeeded => {
                Message::key("semantics.type_annotation_needed")
            }
            SemanticErrorKind::TypeMismatch { expected, found } => {
                Message::key("semantics.type_mismatch")
                    .arg("expected", expected)
                    .arg("found", found)
            }
            SemanticErrorKind::ConditionNotBoolean { found } => {
                Message::key("semantics.condition_not_boolean").arg("found", found)
            }
            SemanticErrorKind::InvalidOperator {
                left,
                right: Some(right),
            } => Message::key("semantics.invalid_operator_binary")
                .arg("left", left)
                .arg("right", right),
            SemanticErrorKind::InvalidOperator { left, right: None } => {
                Message::key("semantics.invalid_operator").arg("left", left)
            }
            SemanticErrorKind::InstanceMemberInStaticContext => {
                Message::key("semantics.instance_member_in_static_context")
            }
            SemanticErrorKind::StaticMemberViaInstance => {
                Message::key("semantics.static_member_via_instance")
            }
            SemanticErrorKind::TypeUsedAsValue => Message::key("semantics.type_used_as_value"),
            SemanticErrorKind::NamespaceUsedAsValue => {
                Message::key("semantics.namespace_used_as_value")
            }
            SemanticErrorKind::NotIndexable { type_name } => {
                Message::key("semantics.not_indexable").arg("type_name", type_name)
            }
            SemanticErrorKind::WrongNumberOfIndices { expected } => {
                Message::key("semantics.wrong_number_of_indices").arg("expected", expected)
            }
            SemanticErrorKind::RaggedArrayInitializer => {
                Message::key("semantics.ragged_array_initializer")
            }
            SemanticErrorKind::NotEnumerable { type_name } => {
                Message::key("semantics.not_enumerable").arg("type_name", type_name)
            }
            SemanticErrorKind::NonExhaustiveSwitch { subject, missing } => {
                Message::key("semantics.non_exhaustive_switch")
                    .arg("subject", subject)
                    .list("missing", missing)
            }
            SemanticErrorKind::UnionNotAbstract => Message::key("semantics.union_not_abstract"),
            SemanticErrorKind::WithNeedsRecord { type_name } => {
                Message::key("semantics.with_needs_record").arg("type_name", type_name)
            }
            SemanticErrorKind::UnionCaseUndetermined { case } => {
                Message::key("semantics.union_case_undetermined").arg("case", case)
            }
            SemanticErrorKind::ReturnValueMismatch => {
                Message::key("semantics.return_value_mismatch")
            }
            SemanticErrorKind::LambdaParameterMismatch => {
                Message::key("semantics.lambda_parameter_mismatch")
            }
            SemanticErrorKind::GenericLocalFunction => {
                Message::key("semantics.generic_local_function")
            }
            SemanticErrorKind::StaticLocalFunctionCapture { name } => {
                Message::key("semantics.static_local_function_capture").arg("name", name)
            }
            SemanticErrorKind::LocalUsedBeforeDeclaration { name } => {
                Message::key("semantics.local_used_before_declaration").arg("name", name)
            }
            SemanticErrorKind::AwaitOutsideAsync => Message::key("semantics.await_outside_async"),
            SemanticErrorKind::NotAwaitable { type_name } => {
                Message::key("semantics.not_awaitable").arg("type_name", type_name)
            }
            SemanticErrorKind::AsyncReturnType { type_name } => {
                Message::key("semantics.async_return_type").arg("type_name", type_name)
            }
            SemanticErrorKind::AsyncByRefParameter => {
                Message::key("semantics.async_by_ref_parameter")
            }
            SemanticErrorKind::YieldOutsideIterator => {
                Message::key("semantics.yield_outside_iterator")
            }
            SemanticErrorKind::YieldInsideTry { region } => {
                Message::key("semantics.yield_inside_try").arg("region", region)
            }
            SemanticErrorKind::ReturnInIterator => Message::key("semantics.return_in_iterator"),
            SemanticErrorKind::UnsupportedExpression => {
                Message::key("semantics.unsupported_expression")
            }
            SemanticErrorKind::UnsupportedStatement => {
                Message::key("semantics.unsupported_statement")
            }
        }
    }

    /// The heading the error is reported under: what kind of problem it
    /// is, in a word.
    pub fn heading(&self) -> &'static str {
        use SemanticErrorKind::*;
        match self {
            TypeMismatch { .. }
            | ConditionNotBoolean { .. }
            | InvalidOperator { .. }
            | ReturnValueMismatch
            | NotIndexable { .. }
            | WrongNumberOfIndices { .. }
            | RaggedArrayInitializer
            | NotEnumerable { .. }
            | ThrowNeedsException { .. }
            | NotAwaitable { .. }
            | AsyncReturnType { .. }
            | WithNeedsRecord { .. }
            | LambdaParameterMismatch
            | CannotInferTypeArguments
            | TypeAnnotationNeeded
            | TypeUsedAsValue
            | NamespaceUsedAsValue
            | NamespaceUsedAsType
            | CannotInstantiateAbstractType { .. }
            | NotCallable { .. }
            | NoMatchingOverload
            | AmbiguousOverload
            | NoMatchingBaseConstructor { .. } => "TypeError",
            UnknownIdentifier
            | UnknownMember { .. }
            | UnresolvedTypeName
            | AmbiguousTypeName
            | UnresolvedUsingTarget
            | LocalUsedBeforeDeclaration { .. } => "NameError",
            DuplicateTypeDefinition { .. }
            | PartialKindMismatch { .. }
            | TypeNamespaceConflict { .. }
            | DuplicateMemberName { .. }
            | DuplicateTypeParameter { .. }
            | MissingImplementation { .. }
            | OperatorRequiresPair { .. }
            | UnionNotAbstract
            | UnionCaseUndetermined { .. }
            | GenericLocalFunction => "DeclarationError",
            UnsupportedExpression | UnsupportedStatement => "UnsupportedError",
            RethrowOutsideCatch
            | DiscardIsNotAPattern
            | InstanceMemberInStaticContext
            | StaticMemberViaInstance
            | NonExhaustiveSwitch { .. }
            | StaticLocalFunctionCapture { .. }
            | AwaitOutsideAsync
            | AsyncByRefParameter
            | YieldOutsideIterator
            | YieldInsideTry { .. }
            | ReturnInIterator => "SemanticsError",
        }
    }

    /// A second place the error points at: the earlier declaration a
    /// duplicate collides with.
    pub fn labels(&self) -> Vec<Label> {
        match self {
            SemanticErrorKind::DuplicateTypeDefinition {
                first_file,
                first_span,
            }
            | SemanticErrorKind::PartialKindMismatch {
                first_file,
                first_span,
            }
            | SemanticErrorKind::TypeNamespaceConflict {
                first_file,
                first_span,
            }
            | SemanticErrorKind::DuplicateMemberName {
                first_file,
                first_span,
            }
            | SemanticErrorKind::DuplicateTypeParameter {
                first_file,
                first_span,
            } => vec![Label {
                file: first_file.0,
                span: first_span.clone(),
                message: Message::key("label.first_declared_here"),
            }],
            _ => Vec::new(),
        }
    }
}
