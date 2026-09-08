//! Body checking: types for every expression in every member body.
//!
//! This is the phase the whole pipeline was built to reach. For each method,
//! constructor, accessor and initializer it walks the statements, tracks locals,
//! resolves names through the same scopes signature resolution used, looks members
//! up through [`TypeSystem`] (inheritance and generic instantiation included),
//! resolves overloads, and records a [`Type`] for every expression node in the
//! `expression_types` side table — the input the Udon code generator will read.
//!
//! Guiding rules, agreed for MenSharp:
//! - when inference cannot be completed, prefer an error that asks for an explicit
//!   annotation ([`CannotInferTypeArguments`], [`TypeAnnotationNeeded`]) over
//!   silently inferring something the real C# compiler might not;
//! - [`Type::Error`] converts to and from everything, so one mistake produces one
//!   diagnostic, not a cascade;
//! - constructs the checker does not handle yet (lambdas, queries, `await`,
//!   pointers, ...) get an explicit [`UnsupportedExpression`] rather than a
//!   wrong type — each is scaffolding to replace, not a decision.
//!
//! Overload resolution is the spec's shape with a simpler "betterness": filter by
//! applicability (arity, `ref`/`out` agreement, implicit convertibility, with the
//! integer-literal narrowing allowance), prefer non-expanded forms, then prefer the
//! candidate with the most exact-type matches. Generic methods without explicit
//! type arguments go through structural unification of parameters against
//! arguments; what cannot be unified asks for explicit type arguments.
//!
//! [`check_file`] is one file's pure function, fanned out per file by the driver.
//!
//! [`CannotInferTypeArguments`]: crate::error::SemanticErrorKind::CannotInferTypeArguments
//! [`TypeAnnotationNeeded`]: crate::error::SemanticErrorKind::TypeAnnotationNeeded
//! [`UnsupportedExpression`]: crate::error::SemanticErrorKind::UnsupportedExpression
//!
//! This file holds what body checking *produces* — [`BodyCheck`] and the
//! resolutions in it — and the `Checker`'s own state; the walk itself is one
//! `impl` split over the modules beside it, by what each part of the language
//! needs: `declarations` (what a type declares, checked without looking at a
//! body), `statements`, `expressions`, `calls`, `operators`, `creation`,
//! `lambdas`, `locals`, `flow` (iterators, `async`, `foreach`), `exhaustive`
//! (does a `switch` cover every case?) and `checker` (the plumbing they
//! share).

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::collections::BTreeSet;
use std::ops::Range;

mod calls;
mod checker;
mod creation;
mod declarations;
mod exhaustive;
mod expressions;
mod flow;
mod lambdas;
mod locals;
mod operators;
mod statements;

use men_sharp_parser::ast::{
    ArgumentModifier, EntityID, Expression, LambdaExpression, MethodDeclaration,
};

use crate::FileId;
use crate::error::SemanticError;
use crate::semantics::merge::Declarations;
use crate::semantics::resolve::{NamespaceScope, Resolution, Resolver, Signatures};
use crate::symbol::{SymbolId, SymbolKind};
use crate::types::external::ExternalTypes;
use crate::types::infer::InferenceKey;
use crate::types::lookup::{MemberCandidate, MemberOrigin, TypeSystem};
use crate::types::{FunctionSignature, Type};

/// The output of body checking for one file (or, merged, a compilation).
#[derive(Debug, Default)]
pub struct BodyCheck {
    /// A type for every checked expression node, keyed by node identity.
    pub expression_types: HashMap<EntityID, Type>,
    /// Expression-level type references (casts, `new T`, locals, ...), resolved.
    pub resolved_types: HashMap<EntityID, Type>,
    /// What each name/call/member node *bound to* — the code generator's map
    /// from syntax to program elements.
    pub targets: HashMap<EntityID, ResolvedTarget>,
    /// The attributes on fields and properties that name a class of the
    /// compilation's own, keyed by the attribute node: the class, with the
    /// construction it stands for recorded in `targets` under the same key
    /// (when the class declares constructors). What `Reflect.VisitFields`
    /// builds a `FieldInfo`'s attributes from.
    pub attribute_types: HashMap<EntityID, Type>,
    /// For every `foreach` that walks a collection by the enumerator pattern
    /// (anything but an array or a string), the three members it bound to.
    /// Keyed by the statement node.
    pub enumerations: HashMap<EntityID, ForeachEnumeration>,
    /// What each constructor runs before its own body: the `: base(...)` /
    /// `: this(...)` it wrote, or the implicit `base()`. Keyed by the
    /// constructor symbol — or, for a class that declares none, by the class
    /// symbol (its implicit default constructor). No entry: nothing to
    /// chain to (a struct, or a base that is not a source class).
    pub constructor_chains: HashMap<SymbolId, ConstructorChain>,
    pub errors: Vec<SemanticError>,
    /// Members and types of foreign files that had errors — syntax,
    /// declaration, signature or body — and the first reason. Not reported
    /// as such (a library's errors are not the user's), but compiling one
    /// is an error at the use. See [`uncompilable_foreign_members`].
    pub uncompilable: HashMap<SymbolId, String>,
    /// For every lambda: the locals and parameters of the enclosing bodies
    /// it reads or writes, by name, in a fixed order — what the code
    /// generator hands the lambda's function when the delegate is made.
    /// Keyed by the lambda node.
    pub captures: HashMap<EntityID, Vec<String>>,
    /// Per member: every local or parameter name some lambda inside it
    /// captures — the variables that have to live in a box the lambda can
    /// share, rather than in the member's own slots.
    pub captured_locals: HashMap<SymbolId, Vec<String>>,
    /// Per member: the type of every name in [`Self::captured_locals`] —
    /// what the box holds, which a local function's own body has to know
    /// without seeing the declaration.
    pub capture_types: HashMap<SymbolId, HashMap<String, Type>>,
    /// Every local function of the compilation, by its declaration node:
    /// what a call to it binds to. See [`crate::MemberOrigin::LocalFunction`].
    pub local_functions: HashMap<EntityID, LocalFunctionSignature>,
    /// For every `await`: the awaiter-pattern members it bound to. Keyed
    /// by the await node.
    pub awaits: HashMap<EntityID, ResolvedAwait>,
    /// Every method and local function whose body contains `yield`, by its
    /// declaration node.
    pub iterators: HashSet<EntityID>,
}

/// A local function's declaration, as a call site sees it.
#[derive(Debug, Clone)]
pub struct LocalFunctionSignature {
    pub signature: FunctionSignature,
    /// `static void F()`: it may use nothing of the enclosing body.
    pub is_static: bool,
}

impl BodyCheck {
    pub fn merge(&mut self, other: BodyCheck) {
        self.expression_types.extend(other.expression_types);
        self.resolved_types.extend(other.resolved_types);
        self.targets.extend(other.targets);
        self.attribute_types.extend(other.attribute_types);
        self.enumerations.extend(other.enumerations);
        self.constructor_chains.extend(other.constructor_chains);
        self.errors.extend(other.errors);
        self.uncompilable.extend(other.uncompilable);
        self.captures.extend(other.captures);
        self.captured_locals.extend(other.captured_locals);
        self.capture_types.extend(other.capture_types);
        self.local_functions.extend(other.local_functions);
        self.awaits.extend(other.awaits);
        self.iterators.extend(other.iterators);
    }
}

/// Which declarations of the foreign files an error fell inside: for each
/// error in such a file — a syntax error, a declaration or signature
/// error, a body error — the innermost type or member whose declaration
/// spans it, with the error's text. The driver stores the result in
/// [`BodyCheck::uncompilable`].
pub fn uncompilable_foreign_members(
    declarations: &Declarations<'_>,
    signatures: &Signatures,
    bodies: &BodyCheck,
) -> HashMap<SymbolId, String> {
    // per foreign file: every declaration span in it
    let mut spans: HashMap<FileId, Vec<(std::ops::Range<usize>, SymbolId)>> = HashMap::default();
    for (id, symbol) in declarations.table.iter() {
        if symbol.kind == SymbolKind::Namespace {
            continue;
        }
        for site in &symbol.declarations {
            if declarations.is_foreign(site.file) {
                spans
                    .entry(site.file)
                    .or_default()
                    .push((site.syntax.full_span(), id));
            }
        }
    }
    if spans.is_empty() {
        return HashMap::default();
    }

    let mut out: HashMap<SymbolId, String> = HashMap::default();
    let mut record = |file: FileId, span: &std::ops::Range<usize>, message: String| {
        let Some(candidates) = spans.get(&file) else {
            return;
        };
        // the innermost declaration containing the error
        let innermost = candidates
            .iter()
            .filter(|(range, _)| range.start <= span.start && span.end <= range.end)
            .min_by_key(|(range, _)| range.end - range.start);
        if let Some((_, symbol)) = innermost {
            out.entry(*symbol).or_insert(message);
        }
    };
    for (file, span, message) in &declarations.foreign_syntax_errors {
        record(*file, span, message.clone());
    }
    for error in declarations
        .errors
        .iter()
        .chain(&signatures.errors)
        .chain(&bodies.errors)
    {
        if declarations.is_foreign(error.file) {
            record(error.file, &error.span, format!("{:?}", error.kind));
        }
    }
    out
}

/// The constructor call a constructor makes first (§15.11.2): to a base
/// constructor or to a sibling. `call` is `None` when the target class
/// declares no constructor at all and the synthesized default one runs.
#[derive(Debug, Clone)]
pub struct ConstructorChain {
    pub kind: ConstructorChainKind,
    pub target_type: Type,
    pub call: Option<ResolvedCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorChainKind {
    Base,
    This,
}

/// What a `foreach` over a non-array collection lowers to — the pattern the
/// language defines (§13.9.5): `GetEnumerator()` once, then `MoveNext()` /
/// `Current` per iteration. Each is resolved like the call it is, so the code
/// generator emits them exactly as it would the written-out loop.
#[derive(Debug, Clone)]
pub struct ForeachEnumeration {
    pub get_enumerator: ResolvedCall,
    pub enumerator_type: Type,
    pub move_next: ResolvedCall,
    pub current: ResolvedMember,
    /// The enumerator's `Dispose()`, when it has one: `foreach` calls it
    /// however the loop is left, which is what runs an iterator's pending
    /// `finally` blocks.
    pub dispose: Option<ResolvedCall>,
}

/// What a checked node resolved to. Keyed by node identity in
/// [`BodyCheck::targets`]:
/// - identifiers and member segments that name a value → [`ResolvedTarget::Local`]
///   or [`ResolvedTarget::Member`];
/// - invocation suffixes, `new` expressions, indexer accesses and user-defined
///   operator applications → [`ResolvedTarget::Call`].
#[derive(Debug, Clone)]
pub enum ResolvedTarget {
    /// A local variable or parameter; the code generator tracks scopes itself.
    Local,
    Member(ResolvedMember),
    Call(ResolvedCall),
}

/// A field, property, event or enum-member access, receiver-instantiated.
#[derive(Debug, Clone)]
pub struct ResolvedMember {
    pub origin: MemberOrigin,
    pub kind: SymbolKind,
    pub is_static: bool,
    pub declaring_type: Type,
    /// The member's value type after generic substitution.
    pub member_type: Type,
}

/// What one `await e` calls, in order: `e.GetAwaiter()`, then on the
/// awaiter `IsCompleted`, `OnCompleted(Action)` when it has to wait, and
/// `GetResult()` once it is done — C#'s awaiter pattern, bound to source
/// members (the mini-corlib's `Task`, or an awaitable of the user's).
#[derive(Debug, Clone)]
pub struct ResolvedAwait {
    pub get_awaiter: ResolvedCall,
    pub awaiter_type: Type,
    pub is_completed: ResolvedMember,
    pub on_completed: ResolvedCall,
    pub get_result: ResolvedCall,
    /// What the `await` expression is: `GetResult()`'s return type.
    pub result_type: Type,
}

/// A resolved invocation: the chosen overload with everything substituted.
#[derive(Debug, Clone)]
pub struct ResolvedCall {
    pub origin: MemberOrigin,
    pub is_static: bool,
    /// The receiver was prepended as argument 0 (extension-method form).
    pub is_extension: bool,
    pub declaring_type: Type,
    /// Parameter and return types, fully instantiated for this call site.
    pub signature: FunctionSignature,
    /// The method's own generic arguments, explicit or inferred.
    pub type_arguments: Vec<Type>,
    /// For each argument as written (the extension receiver first, when
    /// there is one), the index of the parameter it binds to. Identity
    /// unless named arguments reorder them; arguments are still evaluated
    /// in written order.
    pub parameter_of_argument: Vec<usize>,
    /// `Some(n)` when the call used the expanded form of a `params`
    /// parameter: parameters `0..n` are the ordinary ones and every argument
    /// bound at index `n` or beyond is an element of the array the callee
    /// receives as parameter `n`. `parameter_of_argument` counts in that
    /// expanded list; `signature` is the declared one.
    pub params_expansion: Option<usize>,
}

/// Checks every member body in one file. Pure over shared state; the driver runs
/// one call per file in parallel and merges.
pub fn check_file(
    declarations: &Declarations<'_>,
    signatures: &Signatures,
    external: &dyn ExternalTypes,
    file_index: usize,
) -> BodyCheck {
    let file = &declarations.files[file_index];

    let mut checker = Checker {
        resolver: Resolver {
            declarations,
            external,
            file: file.file,
            out: Signatures::default(),
        },
        signatures,
        scopes: Vec::new(),
        type_stack: Vec::new(),
        locals: Vec::new(),
        this_type: None,
        static_context: true,
        return_type: Type::Void,
        lambda_probe_returns: None,
        lambda_stack: Vec::new(),
        current_member: None,
        captures: HashMap::default(),
        captured_locals: HashMap::default(),
        capture_types: HashMap::default(),
        local_functions: HashMap::default(),
        local_calls: HashMap::default(),
        declared_names: HashMap::default(),
        static_local_functions: HashSet::default(),
        local_order: 0,
        local_function_order: HashMap::default(),
        expression_types: HashMap::default(),
        attribute_types: HashMap::default(),
        targets: HashMap::default(),
        pattern_inputs: HashMap::default(),
        enumerations: HashMap::default(),
        constructor_chains: HashMap::default(),
        catch_depth: 0,
        in_async: false,
        awaits: HashMap::default(),
        iterator_element: None,
        yield_seen: false,
        value_return_seen: false,
        guarded_depth: 0,
        catch_depth_for_yield: 0,
        finally_depth: 0,
        iterators: HashSet::default(),
    };

    // rebuild the same file scope signature resolution used
    checker.scopes.push(NamespaceScope {
        path: Vec::new(),
        symbol: Some(declarations.table.root()),
        usings: Vec::new(),
    });
    let mut root_usings = Vec::new();
    for (_, using) in declarations.global_usings() {
        if let Some(resolved) = checker.resolve_using(using) {
            root_usings.push(resolved);
        }
    }
    for using in &file.usings {
        if using.global.is_none()
            && let Some(resolved) = checker.resolve_using(using)
        {
            root_usings.push(resolved);
        }
    }
    // using-target resolution errors were already reported by the resolve phase;
    // this rebuild must not duplicate them
    checker.resolver.out.errors.clear();
    checker.scopes[0].usings = root_usings;

    checker.walk_nodes(&file.members);
    checker.close_captures();

    BodyCheck {
        expression_types: checker.expression_types,
        resolved_types: checker.resolver.out.type_of,
        targets: checker.targets,
        attribute_types: checker.attribute_types,
        enumerations: checker.enumerations,
        constructor_chains: checker.constructor_chains,
        errors: checker.resolver.out.errors,
        uncompilable: HashMap::default(),
        captures: checker
            .captures
            .into_iter()
            .map(|(lambda, names)| (lambda, names.into_iter().collect()))
            .collect(),
        captured_locals: checker
            .captured_locals
            .into_iter()
            .map(|(member, names)| (member, names.into_iter().collect()))
            .collect(),
        capture_types: checker.capture_types,
        local_functions: checker.local_functions,
        awaits: checker.awaits,
        iterators: checker.iterators,
    }
}

// ---------------------------------------------------------------------------

/// What a primary-expression head or chain link denotes.
enum Meaning<'ast> {
    Value(Type),
    /// A type name, awaiting static member access.
    TypeName(Type),
    Namespace(Resolution<'ast>),
    /// An uninvoked method name.
    Group(MethodGroup<'ast>),
    Error,
}

/// How a member is being reached, for the static/instance rules.
struct AccessContext {
    receiver: Option<Type>,
    /// Through a type name (`Debug.Log`), so instance members are unusable.
    via_type: bool,
    /// Through the implicit `this` of a bare identifier.
    implicit_this: bool,
}

struct MethodGroup<'ast> {
    candidates: Vec<MemberCandidate>,
    explicit_arguments: Vec<Type>,
    /// Accessed through a type name, so instance members are unusable.
    via_type: bool,
    /// The member name, for the extension-method fallback and diagnostics.
    name: &'ast str,
    /// The receiver value — it becomes the first argument of an extension call.
    receiver: Option<Type>,
    /// Whether extension methods may be consulted when instance resolution fails
    /// (`receiver.M(...)` yes, `M(...)` and `Type.M(...)` no).
    allow_extensions: bool,
    receiver_display: String,
    span: Range<usize>,
}

/// What one pass of overload resolution concluded.
enum AttemptOutcome {
    Selected(SelectedOverload),
    Ambiguous,
    NoMatch { inference_failed: bool },
}

/// The winning candidate: its instantiated signature plus everything the code
/// generator needs to identify it again.
struct SelectedOverload {
    signature: FunctionSignature,
    /// Index into the group's candidate list.
    candidate: usize,
    /// The method's own generic arguments, explicit or inferred.
    type_arguments: Vec<Type>,
    /// Written argument index → parameter index (see [`ResolvedCall`]).
    parameter_of_argument: Vec<usize>,
    /// See [`ResolvedCall::params_expansion`].
    params_expansion: Option<usize>,
}

/// A candidate that fits the arguments, with what ranks it against the others.
struct Applicable {
    selected: SelectedOverload,
    /// Arguments whose type is exactly the parameter's.
    exact: usize,
    /// Optional parameters the call left out.
    omitted: usize,
    /// Applicable only with its `params` parameter expanded.
    expanded: bool,
}

/// One call argument. Lambdas are *deferred*: their bodies are typed during
/// overload resolution, against each candidate's parameter type, exactly as the
/// spec's two-phase inference demands.
enum ArgumentShape<'ast> {
    Value(Type),
    Lambda(&'ast LambdaExpression<'ast, 'ast>),
}

struct CallArgument<'ast> {
    shape: ArgumentShape<'ast>,
    /// `name: value` — binds to the parameter of that name.
    name: Option<&'ast str>,
    /// The argument's expression node, for recording its final type.
    expression: Option<&'ast Expression<'ast, 'ast>>,
    modifier: Option<ArgumentModifier>,
    /// Allows the constant narrowing rule (`byte b = F(5)`-ish positions).
    is_integer_literal: bool,
    /// `out var x` / `out int x` — the local to bind once an overload is chosen.
    out_declaration: Option<(&'ast str, bool)>,
    /// The receiver of an extension-method call, prepended as argument 0:
    /// only an identity, reference or boxing conversion may take it to the
    /// `this` parameter (§12.8.10.3), never a user-defined one — `int[]`
    /// reaches `IEnumerable<int>` but not `Span<int>`.
    is_receiver: bool,
    span: Range<usize>,
}

impl CallArgument<'_> {
    fn value_type(&self) -> Type {
        match &self.shape {
            ArgumentShape::Value(ty) => ty.clone(),
            ArgumentShape::Lambda(_) => Type::Error,
        }
    }
}

/// One nested scope of a body: the variables it declares and the local
/// functions written in it. Both live in the same declaration space, and
/// both go out of scope together.
#[derive(Default)]
struct Scope<'ast> {
    locals: HashMap<&'ast str, LocalVariable>,
    functions: HashMap<&'ast str, LocalFunctionEntry<'ast>>,
}

/// A declared local or parameter: its type, and when it was written —
/// what a local function declared above it may not reach (CS0841).
struct LocalVariable {
    ty: Type,
    order: usize,
}

/// A local function in scope: what a call to it binds to.
struct LocalFunctionEntry<'ast> {
    node: &'ast MethodDeclaration<'ast, 'ast>,
    /// `None` for a declaration the checker refused (a generic one): calls
    /// still resolve to it, quietly, so one error is reported and not one
    /// per call.
    signature: Option<FunctionSignature>,
}

struct Checker<'a, 'ast> {
    resolver: Resolver<'a, 'ast>,
    signatures: &'a Signatures,
    scopes: Vec<NamespaceScope<'ast>>,
    type_stack: Vec<SymbolId>,
    locals: Vec<Scope<'ast>>,
    this_type: Option<Type>,
    static_context: bool,
    return_type: Type,
    /// When probing a lambda body for its return type, `return` statements push
    /// here instead of being validated against `return_type`.
    lambda_probe_returns: Option<Vec<Type>>,
    /// The lambdas being checked, outermost first, each with the number of
    /// local scopes that were open when it began: a local found below that
    /// depth is captured from outside the lambda.
    lambda_stack: Vec<(EntityID, usize)>,
    /// The member whose body is being checked.
    current_member: Option<SymbolId>,
    /// See [`BodyCheck::captures`] and [`BodyCheck::captured_locals`].
    captures: HashMap<EntityID, BTreeSet<String>>,
    captured_locals: HashMap<SymbolId, BTreeSet<String>>,
    /// See [`BodyCheck::capture_types`].
    capture_types: HashMap<SymbolId, HashMap<String, Type>>,
    /// See [`BodyCheck::local_functions`].
    local_functions: HashMap<EntityID, LocalFunctionSignature>,
    /// Which local functions each lambda or local function calls: a caller
    /// has to hand the callee everything the callee captures, so capture
    /// sets travel along these edges (see [`Self::close_captures`]).
    local_calls: HashMap<EntityID, HashSet<EntityID>>,
    /// Every name a lambda or local function declares itself — the names it
    /// owns, which no caller has to pass it.
    declared_names: HashMap<EntityID, BTreeSet<String>>,
    /// The `static` local functions, which may capture nothing (CS8421).
    static_local_functions: HashSet<EntityID>,
    /// Every local and parameter is stamped as it is declared, in order.
    local_order: usize,
    /// Where each local function stands in that order: it may use the
    /// variables written above it, and no others (CS0841). Local functions
    /// themselves are visible throughout their block, above and below.
    local_function_order: HashMap<EntityID, usize>,
    expression_types: HashMap<EntityID, Type>,
    targets: HashMap<EntityID, ResolvedTarget>,
    /// See [`BodyCheck::attribute_types`].
    attribute_types: HashMap<EntityID, Type>,
    /// The type each pattern was matched against, for the exhaustiveness
    /// check (see `exhaustive.rs`).
    pattern_inputs: HashMap<EntityID, Type>,
    enumerations: HashMap<EntityID, ForeachEnumeration>,
    constructor_chains: HashMap<SymbolId, ConstructorChain>,
    /// How many `catch` blocks enclose the current position — where a bare
    /// `throw;` is legal.
    catch_depth: usize,
    /// Inside an `async` body: `await` is allowed, and `return_type` is
    /// what `return` hands the task (`Task<T>` → `T`).
    in_async: bool,
    /// See [`BodyCheck::awaits`].
    awaits: HashMap<EntityID, ResolvedAwait>,
    /// Inside a body whose return type allows `yield`: the element type.
    iterator_element: Option<Type>,
    /// Whether the current body has met a `yield` / a `return value;`.
    yield_seen: bool,
    value_return_seen: bool,
    /// How many `try`/`catch`/`finally` blocks enclose the current statement.
    /// How many `try` blocks that have a `catch` enclose this statement,
    /// and how many `catch` and `finally` blocks do. C# allows `yield
    /// return` in a `try` that only has a `finally`, and nowhere else
    /// inside an exception region.
    guarded_depth: usize,
    catch_depth_for_yield: usize,
    finally_depth: usize,
    /// See [`BodyCheck::iterators`].
    iterators: HashSet<EntityID>,
}

// ---------------------------------------------------------------------------

/// The inference keys of a method's own type parameters.
fn method_parameter_keys(system: &TypeSystem, candidate: &MemberCandidate) -> Vec<InferenceKey> {
    match &candidate.origin {
        MemberOrigin::Source(symbol) => system
            .declarations
            .table
            .symbol(*symbol)
            .type_parameters
            .iter()
            .map(|&parameter| InferenceKey::Source(parameter))
            .collect(),
        MemberOrigin::External { .. } => (0..candidate.arity).map(InferenceKey::External).collect(),
        // a local function cannot be generic
        MemberOrigin::LocalFunction(_) => Vec::new(),
    }
}

/// How many type parameters a declared parameter type mentions, counting
/// each occurrence: what makes one parameter list less specific than
/// another (§12.6.4.5 — a type parameter is less specific than anything
/// else, and a constructed type is as specific as its arguments).
fn type_parameter_leaves(ty: &Type) -> usize {
    match ty {
        Type::TypeParameter(_) | Type::ExternalMethodTypeParameter(_) => 1,
        Type::Named { arguments, .. } => arguments.iter().map(type_parameter_leaves).sum(),
        Type::Array { element, .. } => type_parameter_leaves(element),
        Type::Nullable(inner) => type_parameter_leaves(inner),
        Type::Tuple(elements) => elements
            .iter()
            .map(|element| type_parameter_leaves(&element.element))
            .sum(),
        _ => 0,
    }
}
