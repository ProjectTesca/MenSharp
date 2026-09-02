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
//!   annotation ([`SemanticErrorKind::CannotInferTypeArguments`],
//!   [`SemanticErrorKind::TypeAnnotationNeeded`]) over silently inferring something
//!   the real C# compiler might not;
//! - [`Type::Error`] converts to and from everything, so one mistake produces one
//!   diagnostic, not a cascade;
//! - constructs the checker does not handle yet (lambdas, queries, `await`,
//!   pointers, ...) get an explicit [`SemanticErrorKind::UnsupportedExpression`]
//!   rather than a wrong type — each is scaffolding to replace, not a decision.
//!
//! Overload resolution is the spec's shape with a simpler "betterness": filter by
//! applicability (arity, `ref`/`out` agreement, implicit convertibility, with the
//! integer-literal narrowing allowance), prefer non-expanded forms, then prefer the
//! candidate with the most exact-type matches. Generic methods without explicit
//! type arguments go through structural unification of parameters against
//! arguments; what cannot be unified asks for explicit type arguments.
//!
//! [`check_file`] is one file's pure function, fanned out per file by the driver.

use std::collections::HashMap;
use std::ops::Range;

use men_sharp_parser::ast::{
    Argument, ArgumentModifier, ArgumentValue, AssignmentOperator, BinaryOperator, Block,
    ConstructorInitializerKind, EntityID, Expression, ForInitializer, FunctionBody,
    InitializerValue, InterpolationPart, LambdaBody, LambdaExpression, LambdaParameters,
    LiteralExpression, LocalVariableDeclaration, Pattern, PrimaryExpression, PrimaryLeft,
    PrimaryRight, Statement, SwitchLabel, UnaryOperator, UsingResource, VariableDesignation,
};

use crate::symbol::SyntaxRef;
use crate::{
    collect::{DeclarationNode, MemberNode, NamespaceNode, TypeNode},
    conversions::NumericKind,
    error::{SemanticError, SemanticErrorKind},
    external::{ExternalTypeKind, ExternalTypes},
    infer::{Inference, InferenceKey, best_common_type},
    lookup::{MemberCandidate, MemberOrigin, TypeSystem},
    merge::Declarations,
    resolve::{NamespaceScope, Resolution, Resolver, Signatures, apply_suffixes},
    symbol::{SymbolId, SymbolKind},
    types::{FunctionSignature, MemberSignature, TupleElement, Type, TypeTarget},
};

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
}

impl BodyCheck {
    pub fn merge(&mut self, other: BodyCheck) {
        self.expression_types.extend(other.expression_types);
        self.resolved_types.extend(other.resolved_types);
        self.targets.extend(other.targets);
        self.enumerations.extend(other.enumerations);
        self.constructor_chains.extend(other.constructor_chains);
        self.errors.extend(other.errors);
    }
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
        expression_types: HashMap::new(),
        targets: HashMap::new(),
        enumerations: HashMap::new(),
        constructor_chains: HashMap::new(),
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

    BodyCheck {
        expression_types: checker.expression_types,
        resolved_types: checker.resolver.out.type_of,
        targets: checker.targets,
        enumerations: checker.enumerations,
        constructor_chains: checker.constructor_chains,
        errors: checker.resolver.out.errors,
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

struct Checker<'a, 'ast> {
    resolver: Resolver<'a, 'ast>,
    signatures: &'a Signatures,
    scopes: Vec<NamespaceScope<'ast>>,
    type_stack: Vec<SymbolId>,
    locals: Vec<HashMap<&'ast str, Type>>,
    this_type: Option<Type>,
    static_context: bool,
    return_type: Type,
    /// When probing a lambda body for its return type, `return` statements push
    /// here instead of being validated against `return_type`.
    lambda_probe_returns: Option<Vec<Type>>,
    expression_types: HashMap<EntityID, Type>,
    targets: HashMap<EntityID, ResolvedTarget>,
    enumerations: HashMap<EntityID, ForeachEnumeration>,
    constructor_chains: HashMap<SymbolId, ConstructorChain>,
}

impl<'a, 'ast> Checker<'a, 'ast> {
    // ------------------------------------------------------------- plumbing

    fn system(&self) -> TypeSystem<'a, 'ast> {
        TypeSystem {
            declarations: self.resolver.declarations,
            signatures: self.signatures,
            external: self.resolver.external,
        }
    }

    fn error(&mut self, kind: SemanticErrorKind, span: Range<usize>) {
        self.resolver.error(kind, span);
    }

    fn display(&self, ty: &Type) -> String {
        self.system().display(ty)
    }

    fn resolve_type(&mut self, node: &men_sharp_parser::ast::TypeRef<'ast, 'ast>) -> Type {
        let Checker {
            resolver,
            scopes,
            type_stack,
            ..
        } = self;
        resolver.resolve_type_ref(node, scopes, type_stack)
    }

    fn lookup_name(
        &mut self,
        name: &'ast str,
        arity: u32,
        span: &Range<usize>,
    ) -> Option<Resolution<'ast>> {
        let Checker {
            resolver,
            scopes,
            type_stack,
            ..
        } = self;
        resolver.try_lookup_unqualified(name, arity, span, scopes, type_stack)
    }

    fn resolve_using(
        &mut self,
        using: &men_sharp_parser::ast::UsingDirective<'ast, 'ast>,
    ) -> Option<crate::resolve::ResolvedUsing<'ast>> {
        let Checker {
            resolver, scopes, ..
        } = self;
        resolver.resolve_using(using, scopes)
    }

    fn record(&mut self, expression: &Expression<'ast, 'ast>, ty: Type) -> Type {
        self.expression_types
            .insert(EntityID::from(expression), ty.clone());
        ty
    }

    fn corlib(&self, name: &str) -> Type {
        match self.resolver.external.find_type(&["System"], name, 0) {
            Some(id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => Type::Error,
        }
    }

    fn declare_local(&mut self, name: &'ast str, ty: Type) {
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name, ty);
        }
    }

    fn local(&self, name: &str) -> Option<&Type> {
        self.locals.iter().rev().find_map(|scope| scope.get(name))
    }

    /// `this` inside the innermost enclosing type: its own generic parameters
    /// applied to itself.
    fn self_type(&self) -> Option<Type> {
        let symbol = self
            .type_stack
            .iter()
            .rev()
            .copied()
            .find(|&id| self.resolver.declarations.table.symbol(id).kind.is_type())?;

        let mut arguments = Vec::new();
        let mut chain = Vec::new();
        let mut current = Some(symbol);
        while let Some(id) = current {
            let entry = self.resolver.declarations.table.symbol(id);
            if entry.kind.is_type() {
                chain.push(id);
            }
            current = entry.parent;
        }
        for &id in chain.iter().rev() {
            for &parameter in &self.resolver.declarations.table.symbol(id).type_parameters {
                arguments.push(Type::TypeParameter(parameter));
            }
        }

        Some(Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        })
    }

    fn require_convertible(&mut self, from: &Type, to: &Type, literal: bool, span: Range<usize>) {
        let ok = self.system().is_implicitly_convertible(from, to)
            || (literal && self.integer_literal_fits(to));
        if !ok {
            let kind = SemanticErrorKind::TypeMismatch {
                expected: self.display(to),
                found: self.display(from),
            };
            self.error(kind, span);
        }
    }

    /// The constant-expression allowance, without evaluating: an integer literal
    /// may sit in any integral slot (the C# LSP checks the actual range).
    fn integer_literal_fits(&self, to: &Type) -> bool {
        self.system()
            .numeric_kind(to)
            .map(|kind| kind.is_integral())
            .unwrap_or(false)
    }

    fn is_integer_literal(expression: &Expression) -> bool {
        match expression {
            Expression::Primary(primary) => {
                primary.chain.is_empty()
                    && matches!(
                        primary.left,
                        PrimaryLeft::Literal(LiteralExpression::Integer(_))
                    )
            }
            Expression::Unary(unary) => {
                matches!(
                    unary.operator.value,
                    UnaryOperator::Minus | UnaryOperator::Plus
                ) && unary
                    .operand
                    .as_ref()
                    .map(|operand| Self::is_integer_literal(operand))
                    .unwrap_or(false)
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------- the walk

    fn walk_nodes(&mut self, nodes: &[DeclarationNode<'ast>]) {
        for node in nodes {
            match node {
                DeclarationNode::Namespace(namespace) => self.walk_namespace(namespace),
                DeclarationNode::Type(type_node) => self.check_type_declaration(type_node),
            }
        }
    }

    fn walk_namespace(&mut self, node: &NamespaceNode<'ast>) {
        let pushed = node.name.len();

        for segment in node.name {
            let previous = self.scopes.last().unwrap();
            let mut path = previous.path.clone();
            path.push(segment.value);

            let symbol = previous.symbol.and_then(|symbol| {
                self.resolver
                    .declarations
                    .table
                    .symbol(symbol)
                    .members_named(segment.value)
                    .iter()
                    .copied()
                    .find(|&id| {
                        self.resolver.declarations.table.symbol(id).kind == SymbolKind::Namespace
                    })
            });

            self.scopes.push(NamespaceScope {
                path,
                symbol,
                usings: Vec::new(),
            });
        }

        let mut usings = Vec::new();
        for using in &node.usings {
            if let Some(resolved) = self.resolve_using(using) {
                usings.push(resolved);
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.usings = usings;
        }

        self.walk_nodes(&node.members);

        let keep = self.scopes.len() - pushed;
        self.scopes.truncate(keep);
    }

    fn check_type_declaration(&mut self, node: &TypeNode<'ast>) {
        let Some(symbol) = self
            .resolver
            .declarations
            .symbol_of(node.syntax.entity_id())
        else {
            return;
        };
        self.type_stack.push(symbol);

        for member in &node.members {
            self.check_member(member);
        }
        if let SyntaxRef::Class(declaration) = &node.syntax {
            let span = declaration
                .name
                .as_ref()
                .ok()
                .map(|name| name.span.clone())
                .unwrap_or_else(|| declaration.span.clone());
            self.check_contracts(symbol, span.clone());
            self.check_implicit_constructor(symbol, span);
            self.check_operator_pairs(symbol);
        }
        for nested in &node.nested {
            self.check_type_declaration(nested);
        }

        self.type_stack.pop();
    }

    // ------------------------------------------------- operator pairs

    /// `==`/`!=`, `<`/`>` and `<=`/`>=` come in pairs with the same
    /// parameter types (CS0216).
    fn check_operator_pairs(&mut self, symbol: SymbolId) {
        const PAIRS: [(&str, &str); 6] = [
            ("op_Equality", "op_Inequality"),
            ("op_Inequality", "op_Equality"),
            ("op_LessThan", "op_GreaterThan"),
            ("op_GreaterThan", "op_LessThan"),
            ("op_LessThanOrEqual", "op_GreaterThanOrEqual"),
            ("op_GreaterThanOrEqual", "op_LessThanOrEqual"),
        ];
        let entry = self.resolver.declarations.table.symbol(symbol);
        let operators: Vec<(SymbolId, &'ast str)> = entry
            .members
            .iter()
            .map(|&member| (member, self.resolver.declarations.table.symbol(member)))
            .filter(|(_, member)| member.kind == SymbolKind::Operator)
            .map(|(id, member)| (id, member.name))
            .collect();
        for (member, name) in &operators {
            let Some((_, partner)) = PAIRS.iter().find(|(own, _)| own == name) else {
                continue;
            };
            let Some(MemberSignature::Function(signature)) = self.signatures.members.get(member)
            else {
                continue;
            };
            let parameters: Vec<Type> = signature
                .parameters
                .iter()
                .map(|parameter| parameter.parameter_type.clone())
                .collect();
            let paired = operators.iter().any(|(other, other_name)| {
                other_name == partner
                    && matches!(
                        self.signatures.members.get(other),
                        Some(MemberSignature::Function(other_signature))
                            if other_signature
                                .parameters
                                .iter()
                                .map(|parameter| parameter.parameter_type.clone())
                                .collect::<Vec<_>>()
                                == parameters
                    )
            });
            if !paired {
                let span = self
                    .resolver
                    .declarations
                    .table
                    .symbol(*member)
                    .declarations
                    .first()
                    .map(|site| match &site.syntax {
                        SyntaxRef::Operator(operator) => operator.operator_keyword.clone(),
                        _ => 0..0,
                    })
                    .unwrap_or(0..0);
                let kind = SemanticErrorKind::OperatorRequiresPair {
                    operator: Self::operator_token(name).to_string(),
                    missing: Self::operator_token(partner).to_string(),
                };
                self.error(kind, span);
            }
        }
    }

    fn operator_token(name: &str) -> &'static str {
        match name {
            "op_Equality" => "==",
            "op_Inequality" => "!=",
            "op_LessThan" => "<",
            "op_GreaterThan" => ">",
            "op_LessThanOrEqual" => "<=",
            _ => ">=",
        }
    }

    // ------------------------------------------------- constructor chains

    /// A class that declares no constructor gets the implicit parameterless
    /// one, which calls `base()` (§15.11.5) — so that call is resolved here,
    /// keyed by the class.
    fn check_implicit_constructor(&mut self, symbol: SymbolId, span: Range<usize>) {
        let entry = self.resolver.declarations.table.symbol(symbol);
        if !matches!(entry.kind, SymbolKind::Class | SymbolKind::Record) || entry.is_static {
            return;
        }
        let declares_constructor = entry.members.iter().any(|&member| {
            let member = self.resolver.declarations.table.symbol(member);
            member.kind == SymbolKind::Constructor && !member.is_static
        });
        if declares_constructor {
            return;
        }
        if let Some(chain) =
            self.resolve_constructor_chain(symbol, ConstructorChainKind::Base, Vec::new(), &span)
        {
            self.constructor_chains.insert(symbol, chain);
        }
    }

    /// Resolves the constructor a constructor of `class` chains to: one of
    /// the base class's for `Base`, a sibling for `This`. `None` when there
    /// is nothing to call — a struct, a base that is not a source class —
    /// or when resolution failed (reported).
    fn resolve_constructor_chain(
        &mut self,
        class: SymbolId,
        kind: ConstructorChainKind,
        arguments: Vec<CallArgument<'ast>>,
        span: &Range<usize>,
    ) -> Option<ConstructorChain> {
        let entry = self.resolver.declarations.table.symbol(class);
        if !matches!(entry.kind, SymbolKind::Class | SymbolKind::Record) {
            return None;
        }
        let self_type = Type::Named {
            target: TypeTarget::Source(class),
            arguments: entry
                .type_parameters
                .iter()
                .map(|parameter| Type::TypeParameter(*parameter))
                .collect(),
        };
        let target_type = match kind {
            ConstructorChainKind::This => self_type,
            ConstructorChainKind::Base => {
                let base = self.system().base_of(&self_type)?;
                if !matches!(
                    &base,
                    Type::Named {
                        target: TypeTarget::Source(_),
                        ..
                    }
                ) {
                    // an external base (a behaviour, `object`): nothing of
                    // ours runs there
                    if !arguments.is_empty() {
                        self.error(SemanticErrorKind::NoMatchingOverload, span.clone());
                    }
                    return None;
                }
                base
            }
        };

        let constructors: Vec<MemberCandidate> = self
            .system()
            .members_named(&target_type, ".ctor")
            .into_iter()
            .filter(|candidate| {
                candidate.kind == SymbolKind::Constructor
                    && !candidate.is_static
                    && candidate.declaring_type == target_type
            })
            .collect();
        if constructors.is_empty() {
            if !arguments.is_empty() {
                let kind = SemanticErrorKind::NoMatchingBaseConstructor {
                    type_name: self.display(&target_type),
                };
                self.error(kind, span.clone());
                return None;
            }
            return Some(ConstructorChain {
                kind,
                target_type,
                call: None,
            });
        }

        let receiver_display = self.display(&target_type);
        let group = MethodGroup {
            candidates: constructors,
            explicit_arguments: Vec::new(),
            via_type: false,
            name: ".ctor",
            receiver: None,
            allow_extensions: false,
            receiver_display,
            span: span.clone(),
        };
        match self.attempt_call(&group, &arguments) {
            AttemptOutcome::Selected(selected) => {
                let call = self.resolved_call_of(&group, &selected, false);
                self.finish_call(&selected, &arguments);
                Some(ConstructorChain {
                    kind,
                    target_type,
                    call: Some(call),
                })
            }
            AttemptOutcome::Ambiguous => {
                self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                None
            }
            AttemptOutcome::NoMatch { .. } => {
                let kind = SemanticErrorKind::NoMatchingBaseConstructor {
                    type_name: self.display(&target_type),
                };
                self.error(kind, span.clone());
                None
            }
        }
    }

    // ------------------------------------------------- abstract / interface

    /// Is this source type declared `abstract`?
    fn is_abstract_type(&self, symbol: SymbolId) -> bool {
        self.resolver
            .declarations
            .table
            .symbol(symbol)
            .declarations
            .iter()
            .any(|site| {
                matches!(&site.syntax, SyntaxRef::Class(declaration)
                if declaration.modifiers.iter().any(|modifier| {
                    modifier.value == men_sharp_parser::ast::Modifier::Abstract
                }))
            })
    }

    /// A member declared without a body: an interface member, or one marked
    /// `abstract`.
    fn is_bodiless_member(&self, symbol: SymbolId) -> bool {
        let entry = self.resolver.declarations.table.symbol(symbol);
        let in_interface = entry.parent.is_some_and(|parent| {
            self.resolver.declarations.table.symbol(parent).kind == SymbolKind::Interface
        });
        in_interface
            || entry.declarations.iter().any(|site| {
                let modifiers: &[men_sharp_parser::ast::Spanned<
                    men_sharp_parser::ast::Modifier,
                >] = match &site.syntax {
                    SyntaxRef::Method(declaration) => declaration.modifiers,
                    SyntaxRef::Property(declaration) => declaration.modifiers,
                    SyntaxRef::Indexer(declaration) => declaration.modifiers,
                    _ => &[],
                };
                modifiers
                    .iter()
                    .any(|modifier| modifier.value == men_sharp_parser::ast::Modifier::Abstract)
            })
    }

    /// A concrete class or struct must implement every abstract member it
    /// inherits (CS0534) and every member of every interface it lists
    /// (CS0535) — otherwise a call dispatched to it would have nowhere to go.
    fn check_contracts(&mut self, symbol: SymbolId, span: Range<usize>) {
        let entry = self.resolver.declarations.table.symbol(symbol);
        if !matches!(
            entry.kind,
            SymbolKind::Class | SymbolKind::Record | SymbolKind::Struct | SymbolKind::RecordStruct
        ) || self.is_abstract_type(symbol)
        {
            return;
        }
        let self_type = Type::Named {
            target: TypeTarget::Source(symbol),
            arguments: entry
                .type_parameters
                .iter()
                .map(|parameter| Type::TypeParameter(*parameter))
                .collect(),
        };

        // (contract type, member) pairs to satisfy
        let mut required: Vec<(Type, SymbolId)> = Vec::new();
        let mut visited: std::collections::HashSet<Type> = std::collections::HashSet::new();
        let mut queue: Vec<Type> = vec![self_type.clone()];
        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            let Type::Named {
                target: TypeTarget::Source(current_symbol),
                ..
            } = &current
            else {
                continue;
            };
            let current_entry = self.resolver.declarations.table.symbol(*current_symbol);
            let is_interface = current_entry.kind == SymbolKind::Interface;
            if current != self_type {
                for &member in &current_entry.members {
                    let member_entry = self.resolver.declarations.table.symbol(member);
                    let contract_member = matches!(
                        member_entry.kind,
                        SymbolKind::Method | SymbolKind::Property | SymbolKind::Indexer
                    ) && !member_entry.is_static
                        && (is_interface || self.is_bodiless_member(member));
                    if contract_member {
                        required.push((current.clone(), member));
                    }
                }
            }
            let system = self.system();
            queue.extend(system.interfaces_of(&current));
            if let Some(base) = system.base_of(&current)
                && matches!(
                    base,
                    Type::Named {
                        target: TypeTarget::Source(_),
                        ..
                    }
                )
            {
                queue.push(base);
            }
        }

        for (contract, member) in required {
            let name = self.resolver.declarations.table.symbol(member).name;
            let kind = self.resolver.declarations.table.symbol(member).kind;
            // the member as the contract instantiates it
            let wanted = self
                .system()
                .members_named(&contract, name)
                .into_iter()
                .find(|candidate| matches!(candidate.origin, MemberOrigin::Source(id) if id == member))
                .and_then(|candidate| candidate.signature);
            let implemented =
                self.implementation_of(&self_type, &contract, name, kind, wanted.as_ref());
            if !implemented {
                let kind = SemanticErrorKind::MissingImplementation {
                    type_name: self.display(&self_type),
                    member: format!("{}.{}", self.display(&contract), name),
                };
                self.error(kind, span.clone());
            }
        }
    }

    /// Does `ty` (or a base) carry a non-abstract member `name` of this kind
    /// and signature? Explicit interface implementations count.
    fn implementation_of(
        &self,
        ty: &Type,
        contract: &Type,
        name: &str,
        kind: SymbolKind,
        wanted: Option<&MemberSignature>,
    ) -> bool {
        let system = self.system();
        let mut candidates = system.members_named(ty, name);
        // explicit implementations are hidden from ordinary lookup — and
        // implement exactly the interface they name, no other contract
        let mut current = Some(ty.clone());
        while let Some(class_type) = current {
            let Type::Named {
                target: TypeTarget::Source(class),
                arguments,
            } = &class_type
            else {
                break;
            };
            let class_entry = self.resolver.declarations.table.symbol(*class);
            let bindings: Vec<(SymbolId, Type)> = class_entry
                .type_parameters
                .iter()
                .copied()
                .zip(arguments.iter().cloned())
                .collect();
            for &member in &class_entry.members {
                let member_entry = self.resolver.declarations.table.symbol(member);
                if member_entry.is_explicit_implementation && member_entry.name == name {
                    let substitute = |ty: Type| match ty {
                        Type::TypeParameter(parameter) => bindings
                            .iter()
                            .find(|(bound, _)| *bound == parameter)
                            .map(|(_, to)| to.clone())
                            .unwrap_or(Type::TypeParameter(parameter)),
                        other => other,
                    };
                    let implements_contract = self
                        .signatures
                        .explicit_interfaces
                        .get(&member)
                        .is_some_and(|interface| interface.clone().map(&substitute) == *contract);
                    if !implements_contract {
                        continue;
                    }
                    let signature = self
                        .signatures
                        .members
                        .get(&member)
                        .map(|signature| signature.map(&substitute));
                    candidates.push(MemberCandidate {
                        origin: MemberOrigin::Source(member),
                        kind: member_entry.kind,
                        is_static: member_entry.is_static,
                        accessibility: member_entry.accessibility,
                        arity: member_entry.arity,
                        signature,
                        declaring_type: class_type.clone(),
                    });
                }
            }
            current = system.base_of(&class_type).filter(|base| {
                matches!(
                    base,
                    Type::Named {
                        target: TypeTarget::Source(_),
                        ..
                    }
                )
            });
        }
        candidates.into_iter().any(|candidate| {
            let same_kind = match kind {
                SymbolKind::Method => candidate.kind == SymbolKind::Method,
                SymbolKind::Property => candidate.kind == SymbolKind::Property,
                SymbolKind::Indexer => candidate.kind == SymbolKind::Indexer,
                _ => false,
            };
            if !same_kind || candidate.is_static {
                return false;
            }
            let fits = match (wanted, &candidate.signature) {
                (Some(wanted), Some(found)) => wanted == found,
                (None, _) => true,
                _ => false,
            };
            if !fits {
                return false;
            }
            match candidate.origin {
                MemberOrigin::Source(id) => !self.is_bodiless_member(id),
                MemberOrigin::External { .. } => true,
            }
        })
    }

    /// `int x = 5`: the default is typed against its parameter and must be
    /// a constant — a literal, `default`, `null`, or a constant member —
    /// because every call site bakes it in (CS1736 otherwise).
    fn check_parameter_defaults(
        &mut self,
        parameters: &[&'ast men_sharp_parser::ast::Parameter<'ast, 'ast>],
        function: &FunctionSignature,
        is_static: bool,
    ) {
        for (parameter, signature) in parameters.iter().zip(&function.parameters) {
            let Some(value) = &parameter.default_value else {
                continue;
            };
            if !Self::is_constant_shaped(value) {
                self.error(SemanticErrorKind::UnsupportedExpression, value.span());
                continue;
            }
            let parameter_type = signature.parameter_type.clone();
            let probe = FunctionSignature {
                return_type: Type::Void,
                parameters: Vec::new(),
            };
            self.enter_body(&probe, &[], is_static, |checker| {
                let literal = Self::is_integer_literal(value);
                let ty = checker.check_expression_expecting(value, Some(&parameter_type));
                checker.require_convertible(&ty, &parameter_type, literal, value.span());
            });
        }
    }

    /// The shapes a constant expression can take here: literals (signed),
    /// `default`, `null`, and dotted names (enum members, `const` fields).
    fn is_constant_shaped(expression: &Expression<'ast, 'ast>) -> bool {
        match expression {
            Expression::Unary(unary) => unary.operand.as_ref().is_ok_and(Self::is_constant_shaped),
            Expression::Primary(primary) => {
                let chain_is_members = primary
                    .chain
                    .iter()
                    .all(|right| matches!(right, PrimaryRight::Member { .. }));
                chain_is_members
                    && matches!(
                        primary.left,
                        PrimaryLeft::Literal(_)
                            | PrimaryLeft::Default { .. }
                            | PrimaryLeft::Identifier { .. }
                            | PrimaryLeft::Predefined(_)
                    )
            }
            _ => false,
        }
    }

    fn check_member(&mut self, node: &MemberNode<'ast>) {
        let Some(symbol) = self
            .resolver
            .declarations
            .symbol_of(node.syntax.entity_id())
        else {
            return;
        };
        let member_signature = self.signatures.members.get(&symbol).cloned();

        match node.syntax {
            SyntaxRef::Method(method) => {
                let Some(MemberSignature::Function(function)) = member_signature else {
                    return;
                };
                self.type_stack.push(symbol);
                let names = method
                    .parameters
                    .as_ref()
                    .map(|list| list.parameters.iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                self.check_parameter_defaults(&names, &function, node.is_static);
                self.check_function_body(&method.body, &function, &names, node.is_static);
                self.type_stack.pop();
            }
            SyntaxRef::Constructor(constructor) => {
                let Some(MemberSignature::Function(function)) = member_signature else {
                    return;
                };
                let names = constructor
                    .parameters
                    .as_ref()
                    .map(|list| list.parameters.iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                self.check_parameter_defaults(&names, &function, node.is_static);
                let owner = self.type_stack.last().copied();
                self.enter_body(&function, &names, node.is_static, |checker| {
                    if !node.is_static
                        && let Some(class) = owner
                    {
                        let chain = match &constructor.initializer {
                            Some(initializer) => {
                                let kind = match initializer.kind.value {
                                    ConstructorInitializerKind::Base => ConstructorChainKind::Base,
                                    ConstructorInitializerKind::This => ConstructorChainKind::This,
                                };
                                let arguments = initializer
                                    .arguments
                                    .as_ref()
                                    .map(|list| checker.check_arguments(list.arguments))
                                    .unwrap_or_default();
                                checker.resolve_constructor_chain(
                                    class,
                                    kind,
                                    arguments,
                                    &initializer.span,
                                )
                            }
                            None => checker.resolve_constructor_chain(
                                class,
                                ConstructorChainKind::Base,
                                Vec::new(),
                                &constructor.name.span,
                            ),
                        };
                        if let Some(chain) = chain {
                            checker.constructor_chains.insert(symbol, chain);
                        }
                    }
                    checker.check_function_body_inner(&constructor.body);
                });
            }
            SyntaxRef::Destructor(destructor) => {
                let function = crate::types::FunctionSignature {
                    return_type: Type::Void,
                    parameters: Vec::new(),
                };
                self.enter_body(&function, &[], false, |checker| {
                    checker.check_function_body_inner(&destructor.body);
                });
            }
            SyntaxRef::Operator(operator) => {
                let Some(MemberSignature::Function(function)) = member_signature else {
                    return;
                };
                let names = operator
                    .parameters
                    .as_ref()
                    .map(|list| list.parameters.iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                self.check_function_body(&operator.body, &function, &names, true);
            }
            SyntaxRef::Property(property) => {
                let Some(MemberSignature::Property(property_type)) = member_signature else {
                    return;
                };
                self.check_accessors(&property.body, &property_type, &[], node.is_static);
                if let Some(InitializerValue::Expression(initializer)) = &property.initializer {
                    let function = crate::types::FunctionSignature {
                        return_type: property_type.clone(),
                        parameters: Vec::new(),
                    };
                    self.enter_body(&function, &[], node.is_static, |checker| {
                        let literal = Self::is_integer_literal(initializer);
                        let ty =
                            checker.check_expression_expecting(initializer, Some(&property_type));
                        checker.require_convertible(
                            &ty,
                            &property_type,
                            literal,
                            initializer.span(),
                        );
                    });
                }
            }
            SyntaxRef::Indexer(indexer) => {
                let Some(MemberSignature::Function(function)) = member_signature else {
                    return;
                };
                let names = indexer
                    .parameters
                    .as_ref()
                    .map(|list| list.parameters.iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                // accessors see the indexer parameters
                let element = function.return_type.clone();
                self.enter_body(&function, &names, false, |checker| {
                    checker.check_accessors_inner(&indexer.body, &element);
                });
            }
            SyntaxRef::Field { field, declarator } => {
                let Some(MemberSignature::Field(field_type)) = member_signature else {
                    return;
                };
                if let Some(InitializerValue::Expression(initializer)) = &declarator.initializer {
                    let function = crate::types::FunctionSignature {
                        return_type: field_type.clone(),
                        parameters: Vec::new(),
                    };
                    self.enter_body(&function, &[], node.is_static, |checker| {
                        let literal = Self::is_integer_literal(initializer);
                        let ty = checker.check_expression_expecting(initializer, Some(&field_type));
                        checker.require_convertible(&ty, &field_type, literal, initializer.span());
                    });
                }
                let _ = field;
            }
            SyntaxRef::EnumMember(member) => {
                if let Some(value) = &member.value {
                    let function = crate::types::FunctionSignature {
                        return_type: self.corlib("Int32"),
                        parameters: Vec::new(),
                    };
                    self.enter_body(&function, &[], true, |checker| {
                        checker.check_expression(value);
                    });
                }
            }
            _ => {}
        }
    }

    /// Sets up locals/this/return for one body, runs `body`, restores.
    fn enter_body(
        &mut self,
        function: &crate::types::FunctionSignature,
        parameter_syntax: &[&men_sharp_parser::ast::Parameter<'ast, 'ast>],
        is_static: bool,
        body: impl FnOnce(&mut Self),
    ) {
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_this = self.this_type.take();
        let saved_static = self.static_context;
        let saved_return = std::mem::replace(&mut self.return_type, function.return_type.clone());

        self.locals.push(HashMap::new());
        for (index, parameter) in parameter_syntax.iter().enumerate() {
            if let (Ok(name), Some(signature)) = (&parameter.name, function.parameters.get(index)) {
                self.declare_local(name.value, signature.parameter_type.clone());
            }
        }
        self.this_type = self.self_type();
        self.static_context = is_static;

        body(self);

        self.locals = saved_locals;
        self.this_type = saved_this;
        self.static_context = saved_static;
        self.return_type = saved_return;
    }

    fn check_function_body(
        &mut self,
        body: &'ast FunctionBody<'ast, 'ast>,
        function: &crate::types::FunctionSignature,
        parameter_syntax: &[&men_sharp_parser::ast::Parameter<'ast, 'ast>],
        is_static: bool,
    ) {
        self.enter_body(function, parameter_syntax, is_static, |checker| {
            checker.check_function_body_inner(body);
        });
    }

    fn check_function_body_inner(&mut self, body: &'ast FunctionBody<'ast, 'ast>) {
        match body {
            FunctionBody::Block(block) => self.check_block(block),
            FunctionBody::Expression {
                expression: Ok(expression),
                ..
            } => {
                let literal = Self::is_integer_literal(expression);
                if self.return_type != Type::Void {
                    let expected = self.return_type.clone();
                    let ty = self.check_expression_expecting(expression, Some(&expected));
                    self.require_convertible(&ty, &expected, literal, expression.span());
                } else {
                    self.check_expression(expression);
                }
            }
            _ => {}
        }
    }

    fn check_accessors(
        &mut self,
        body: &'ast FunctionBody<'ast, 'ast>,
        member_type: &Type,
        parameter_syntax: &[&men_sharp_parser::ast::Parameter<'ast, 'ast>],
        is_static: bool,
    ) {
        let function = crate::types::FunctionSignature {
            return_type: member_type.clone(),
            parameters: Vec::new(),
        };
        self.enter_body(&function, parameter_syntax, is_static, |checker| {
            checker.check_accessors_inner(body, member_type);
        });
    }

    fn check_accessors_inner(&mut self, body: &'ast FunctionBody<'ast, 'ast>, member_type: &Type) {
        match body {
            // an expression-bodied property is its getter
            FunctionBody::Expression {
                expression: Ok(expression),
                ..
            } => {
                let literal = Self::is_integer_literal(expression);
                let ty = self.check_expression_expecting(expression, Some(member_type));
                self.require_convertible(&ty, member_type, literal, expression.span());
            }
            FunctionBody::Accessors(list) => {
                for accessor in list.accessors {
                    use men_sharp_parser::ast::AccessorKind;
                    let is_setter = matches!(
                        accessor.kind.value,
                        AccessorKind::Set
                            | AccessorKind::Init
                            | AccessorKind::Add
                            | AccessorKind::Remove
                    );

                    self.locals.push(HashMap::new());
                    let saved_return = self.return_type.clone();
                    if is_setter {
                        self.declare_local("value", member_type.clone());
                        self.return_type = Type::Void;
                    } else {
                        self.return_type = member_type.clone();
                    }

                    match &accessor.body {
                        FunctionBody::Block(block) => self.check_block(block),
                        FunctionBody::Expression {
                            expression: Ok(expression),
                            ..
                        } => {
                            let ty = self.check_expression(expression);
                            if !is_setter {
                                self.require_convertible(
                                    &ty,
                                    member_type,
                                    false,
                                    expression.span(),
                                );
                            }
                        }
                        _ => {}
                    }

                    self.return_type = saved_return;
                    self.locals.pop();
                }
            }
            FunctionBody::Block(block) => self.check_block(block),
            _ => {}
        }
    }

    // ----------------------------------------------------------- statements

    fn check_block(&mut self, block: &'ast Block<'ast, 'ast>) {
        self.locals.push(HashMap::new());
        for statement in block.statements {
            self.check_statement(statement);
        }
        self.locals.pop();
    }

    fn check_statement(&mut self, statement: &'ast Statement<'ast, 'ast>) {
        match statement {
            Statement::Block(block) => self.check_block(block),
            Statement::LocalVariable(declaration) => self.check_local_declaration(declaration),
            Statement::Expression(statement) => {
                self.check_expression(&statement.expression);
            }
            Statement::If(statement) => {
                if let Ok(condition) = &statement.condition {
                    self.check_condition(condition);
                }
                if let Ok(then_branch) = statement.then_branch {
                    self.check_statement(then_branch);
                }
                if let Some(else_branch) = statement.else_branch {
                    self.check_statement(else_branch);
                }
            }
            Statement::While(statement) => {
                if let Ok(condition) = &statement.condition {
                    self.check_condition(condition);
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
            }
            Statement::DoWhile(statement) => {
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
                if let Ok(condition) = &statement.condition {
                    self.check_condition(condition);
                }
            }
            Statement::For(statement) => {
                self.locals.push(HashMap::new());
                match &statement.initializer {
                    Some(ForInitializer::Declaration(declaration)) => {
                        self.check_local_declaration(declaration)
                    }
                    Some(ForInitializer::Expressions(expressions)) => {
                        for expression in *expressions {
                            self.check_expression(expression);
                        }
                    }
                    None => {}
                }
                if let Some(condition) = &statement.condition {
                    self.check_condition(condition);
                }
                for incrementor in statement.incrementors {
                    self.check_expression(incrementor);
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
                self.locals.pop();
            }
            Statement::Foreach(statement) => {
                self.locals.push(HashMap::new());
                let element = match &statement.collection {
                    Ok(collection) => {
                        let collection_type = self.check_expression(collection);
                        self.element_type_of(
                            &collection_type,
                            collection.span(),
                            EntityID::from(statement),
                        )
                    }
                    Err(()) => Type::Error,
                };

                if let (Ok(variable_type), Ok(name)) = (&statement.variable_type, &statement.name) {
                    let declared = self.resolve_type(variable_type);
                    let ty = if matches!(declared, Type::Infer) {
                        element.clone()
                    } else {
                        self.require_convertible(&element, &declared, false, name.span.clone());
                        declared
                    };
                    self.declare_local(name.value, ty);
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
                self.locals.pop();
            }
            Statement::Switch(statement) => {
                let value = match &statement.value {
                    Ok(value) => self.check_expression(value),
                    Err(()) => Type::Error,
                };
                if let Ok(sections) = statement.sections {
                    for section in sections {
                        self.locals.push(HashMap::new());
                        for label in section.labels {
                            if let SwitchLabel::Case { pattern, guard, .. } = label {
                                if let Ok(pattern) = pattern {
                                    self.check_pattern(pattern, &value);
                                }
                                if let Some(guard) = guard {
                                    self.check_condition(guard);
                                }
                            }
                        }
                        for statement in section.statements {
                            self.check_statement(statement);
                        }
                        self.locals.pop();
                    }
                }
            }
            Statement::Try(statement) => {
                if let Ok(block) = &statement.block {
                    self.check_block(block);
                }
                for catch in statement.catches {
                    self.locals.push(HashMap::new());
                    let exception = catch
                        .exception_type
                        .as_ref()
                        .map(|exception| self.resolve_type(exception));
                    if let (Some(name), Some(ty)) = (&catch.name, exception) {
                        self.declare_local(name.value, ty);
                    }
                    if let Some(filter) = &catch.filter
                        && let Ok(condition) = &filter.condition
                    {
                        self.check_condition(condition);
                    }
                    if let Ok(block) = &catch.block {
                        self.check_block(block);
                    }
                    self.locals.pop();
                }
                if let Some(finally) = &statement.finally_clause
                    && let Ok(block) = &finally.block
                {
                    self.check_block(block);
                }
            }
            Statement::Using(statement) => {
                self.locals.push(HashMap::new());
                match &statement.resource {
                    Ok(UsingResource::Declaration(declaration)) => {
                        self.check_local_declaration(declaration)
                    }
                    Ok(UsingResource::Expression(expression)) => {
                        self.check_expression(expression);
                    }
                    Err(()) => {}
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
                self.locals.pop();
            }
            Statement::Lock(statement) => {
                if let Ok(target) = &statement.target {
                    self.check_expression(target);
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
            }
            Statement::Checked(statement) => {
                if let Ok(block) = &statement.block {
                    self.check_block(block);
                }
            }
            Statement::Unsafe(statement) => {
                if let Ok(block) = &statement.block {
                    self.check_block(block);
                }
            }
            Statement::Fixed(statement) => {
                self.locals.push(HashMap::new());
                if let Ok(declaration) = &statement.declaration {
                    self.check_local_declaration(declaration);
                }
                if let Ok(body) = statement.body {
                    self.check_statement(body);
                }
                self.locals.pop();
            }
            Statement::Return(statement) => {
                if self.lambda_probe_returns.is_some() {
                    let ty = statement
                        .value
                        .as_ref()
                        .map(|value| self.check_expression(value))
                        .unwrap_or(Type::Void);
                    if let Some(returns) = &mut self.lambda_probe_returns {
                        returns.push(ty);
                    }
                    return;
                }
                let expected = self.return_type.clone();
                match (&statement.value, expected == Type::Void) {
                    (Some(value), false) => {
                        let literal = Self::is_integer_literal(value);
                        let ty = self.check_expression_expecting(value, Some(&expected));
                        self.require_convertible(&ty, &expected, literal, value.span());
                    }
                    (Some(value), true) => {
                        self.check_expression(value);
                        self.error(
                            SemanticErrorKind::ReturnValueMismatch,
                            statement.span.clone(),
                        );
                    }
                    (None, false) => {
                        self.error(
                            SemanticErrorKind::ReturnValueMismatch,
                            statement.span.clone(),
                        );
                    }
                    (None, true) => {}
                }
            }
            Statement::Throw(statement) => {
                if let Some(value) = &statement.value {
                    self.check_expression(value);
                }
            }
            Statement::Yield(statement) => {
                if let Some(value) = &statement.value {
                    self.check_expression(value);
                }
                // iterators do not exist on Udon; a precise diagnostic comes with
                // the capability check phase
                self.error(
                    SemanticErrorKind::UnsupportedStatement,
                    statement.span.clone(),
                );
            }
            Statement::LocalFunction(function) => {
                // needs local signature resolution; scaffolding until then
                self.error(
                    SemanticErrorKind::UnsupportedStatement,
                    function.span.clone(),
                );
            }
            Statement::Labeled(statement) => {
                if let Ok(inner) = statement.statement {
                    self.check_statement(inner);
                }
            }
            Statement::Goto(statement) => {
                if let men_sharp_parser::ast::GotoTarget::Case {
                    value: Ok(value), ..
                } = &statement.target
                {
                    self.check_expression(value);
                }
            }
            Statement::Break(_) | Statement::Continue(_) | Statement::Empty { .. } => {}
        }
    }

    fn check_local_declaration(&mut self, declaration: &'ast LocalVariableDeclaration<'ast, 'ast>) {
        let declared = self.resolve_type(&declaration.variable_type);

        for declarator in declaration.declarators {
            let ty = match (&declared, &declarator.initializer) {
                (Type::Infer, Some(InitializerValue::Expression(initializer))) => {
                    let inferred = self.check_expression(initializer);
                    match inferred {
                        // `var x = null;` cannot pick a type
                        Type::Null | Type::Void => {
                            self.error(
                                SemanticErrorKind::TypeAnnotationNeeded,
                                declarator.name.span.clone(),
                            );
                            Type::Error
                        }
                        inferred => inferred,
                    }
                }
                (Type::Infer, _) => {
                    self.error(
                        SemanticErrorKind::TypeAnnotationNeeded,
                        declarator.name.span.clone(),
                    );
                    Type::Error
                }
                (declared, Some(InitializerValue::Expression(initializer))) => {
                    let literal = Self::is_integer_literal(initializer);
                    let declared = declared.clone();
                    let ty = self.check_expression_expecting(initializer, Some(&declared));
                    self.require_convertible(&ty, &declared, literal, initializer.span());
                    declared
                }
                (declared, _) => declared.clone(),
            };

            self.declare_local(declarator.name.value, ty);
        }
    }

    fn check_condition(&mut self, condition: &'ast Expression<'ast, 'ast>) {
        let ty = self.check_expression(condition);
        if !self.system().is_bool(&ty) {
            let kind = SemanticErrorKind::ConditionNotBoolean {
                found: self.display(&ty),
            };
            self.error(kind, condition.span());
        }
    }

    /// A pattern written as a bare dotted name (`Color.Red`), resolved as a
    /// member: records it on the type node and answers true. Quiet — a name
    /// that does not resolve that way leaves no trace, and the caller falls
    /// back to reading it as a type.
    fn bind_pattern_constant(
        &mut self,
        pattern_type: &'ast men_sharp_parser::ast::TypeRef<'ast, 'ast>,
    ) -> bool {
        use men_sharp_parser::ast::TypeRefBase;
        if !pattern_type.suffixes.is_empty() {
            return false;
        }
        let TypeRefBase::Name(name) = &pattern_type.base else {
            return false;
        };
        if name.global.is_some()
            || name.segments.len() < 2
            || name
                .segments
                .iter()
                .any(|segment| segment.generics.is_some())
        {
            return false;
        }

        let node = EntityID::from(pattern_type);
        let before = self.resolver.out.errors.len();
        let mut segments = name.segments.iter();
        let first = segments.next().expect("at least two segments");
        let mut meaning = match self.lookup_name(first.name.value, 0, &first.span) {
            Some(Resolution::Type { target, arguments }) => {
                Meaning::TypeName(Type::Named { target, arguments })
            }
            Some(resolution @ Resolution::Namespace { .. }) => Meaning::Namespace(resolution),
            Some(Resolution::TypeParameter(symbol)) => {
                Meaning::TypeName(Type::TypeParameter(symbol))
            }
            _ => return false,
        };
        let last = name.segments.len() - 1;
        for (index, segment) in segments.enumerate() {
            let record = (index + 1 == last).then_some(node);
            meaning = self.access_member(
                meaning,
                segment.name.value,
                Vec::new(),
                &segment.span,
                record,
            );
        }
        let bound = matches!(meaning, Meaning::Value(_)) && self.targets.contains_key(&node);
        if !bound {
            self.resolver.out.errors.truncate(before);
            self.targets.remove(&node);
        }
        bound
    }

    fn check_pattern(&mut self, pattern: &'ast Pattern<'ast, 'ast>, matched: &Type) {
        match pattern {
            Pattern::Discard(_) => {}
            Pattern::Declaration {
                pattern_type,
                designation,
                ..
            } => {
                // `case Color.Red:` parses as this pattern — a bare dotted
                // name could equally be a type, and only binding tells (C#'s
                // own rule). When it lands on a member, this is a constant
                // pattern; the member is recorded on the type node for the
                // code generator.
                if designation.is_none() && self.bind_pattern_constant(pattern_type) {
                    return;
                }
                let ty = self.resolve_type(pattern_type);
                if let Some(name) = designation {
                    self.declare_local(name.value, ty);
                }
            }
            Pattern::Var { designation, .. } => {
                if let Ok(VariableDesignation::Single(name)) = designation {
                    self.declare_local(name.value, matched.clone());
                }
            }
            Pattern::Constant(expression) => {
                self.check_expression(expression);
            }
            Pattern::Relational { value, .. } => {
                if let Ok(value) = value {
                    self.check_expression(value);
                }
            }
            Pattern::Not { pattern, .. } | Pattern::Parenthesized { pattern, .. } => {
                if let Ok(pattern) = pattern {
                    self.check_pattern(pattern, matched);
                }
            }
            Pattern::And { left, right, .. } | Pattern::Or { left, right, .. } => {
                self.check_pattern(left, matched);
                if let Ok(right) = right {
                    self.check_pattern(right, matched);
                }
            }
            Pattern::Property {
                pattern_type,
                subpatterns,
                designation,
                ..
            } => {
                let target = match pattern_type {
                    Some(pattern_type) => self.resolve_type(pattern_type),
                    None => matched.clone(),
                };
                for subpattern in *subpatterns {
                    let member_type = self
                        .system()
                        .members_named(&target, subpattern.name.value)
                        .into_iter()
                        .find_map(|candidate| match candidate.signature {
                            Some(MemberSignature::Field(ty))
                            | Some(MemberSignature::Property(ty)) => Some(ty),
                            _ => None,
                        })
                        .unwrap_or(Type::Error);
                    if let Ok(pattern) = &subpattern.pattern {
                        self.check_pattern(pattern, &member_type);
                    }
                }
                if let Some(name) = designation {
                    self.declare_local(name.value, target);
                }
            }
            // positional/list/slice matching needs Deconstruct and indexers
            _ => {
                self.error(SemanticErrorKind::UnsupportedExpression, pattern.span());
            }
        }
    }

    // ---------------------------------------------------------- expressions

    fn check_expression(&mut self, expression: &'ast Expression<'ast, 'ast>) -> Type {
        self.check_expression_expecting(expression, None)
    }

    /// `expected` is the target type the context supplies — what makes lambdas,
    /// `default`, and target-typed `new` well-typed where C# says they are.
    fn check_expression_expecting(
        &mut self,
        expression: &'ast Expression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Type {
        let ty = self.check_expression_inner(expression, expected);
        self.record(expression, ty)
    }

    fn check_expression_inner(
        &mut self,
        expression: &'ast Expression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Type {
        match expression {
            Expression::Primary(primary) => self.check_primary(primary, expected),
            Expression::Assignment(assignment) => {
                let target = self.check_expression(&assignment.target);
                let Ok(value) = &assignment.value else {
                    return target;
                };
                let literal = Self::is_integer_literal(value);
                let value_type = self.check_expression_expecting(value, Some(&target));

                match assignment.operator.value {
                    AssignmentOperator::Assign | AssignmentOperator::Coalesce => {
                        self.require_convertible(&value_type, &target, literal, value.span());
                    }
                    compound => {
                        let operator = match compound {
                            AssignmentOperator::Add => BinaryOperator::Add,
                            AssignmentOperator::Subtract => BinaryOperator::Subtract,
                            AssignmentOperator::Multiply => BinaryOperator::Multiply,
                            AssignmentOperator::Divide => BinaryOperator::Divide,
                            AssignmentOperator::Modulo => BinaryOperator::Modulo,
                            AssignmentOperator::BitwiseAnd => BinaryOperator::BitwiseAnd,
                            AssignmentOperator::BitwiseOr => BinaryOperator::BitwiseOr,
                            AssignmentOperator::BitwiseXor => BinaryOperator::BitwiseXor,
                            AssignmentOperator::LeftShift => BinaryOperator::LeftShift,
                            AssignmentOperator::RightShift => BinaryOperator::RightShift,
                            _ => BinaryOperator::UnsignedRightShift,
                        };
                        let result = self.binary_type(
                            operator,
                            target.clone(),
                            value_type,
                            literal,
                            assignment.span.clone(),
                            Some(EntityID::from(*assignment)),
                        );
                        // compound assignment narrows back implicitly (int += byte)
                        if !matches!(result, Type::Error) {
                            let compatible =
                                self.system().is_implicitly_convertible(&result, &target)
                                    || self.system().numeric_kind(&target).is_some();
                            if !compatible {
                                let kind = SemanticErrorKind::TypeMismatch {
                                    expected: self.display(&target),
                                    found: self.display(&result),
                                };
                                self.error(kind, assignment.span.clone());
                            }
                        }
                    }
                }
                target
            }
            Expression::Conditional(conditional) => {
                self.check_condition(&conditional.condition);
                let then_type = match &conditional.then_value {
                    Ok(value) => self.check_expression_expecting(value, expected),
                    Err(()) => Type::Error,
                };
                let else_type = match &conditional.else_value {
                    Ok(value) => self.check_expression_expecting(value, expected),
                    Err(()) => Type::Error,
                };

                let system = self.system();
                match best_common_type(&system, &[then_type.clone(), else_type.clone()]) {
                    Some(common) => common,
                    None if matches!(then_type, Type::Null) && matches!(else_type, Type::Null) => {
                        Type::Null
                    }
                    None => {
                        let kind = SemanticErrorKind::TypeMismatch {
                            expected: self.display(&then_type),
                            found: self.display(&else_type),
                        };
                        self.error(kind, conditional.span.clone());
                        Type::Error
                    }
                }
            }
            Expression::Binary(binary) => {
                let left = self.check_expression(&binary.left);
                let (right, literal) = match &binary.right {
                    Ok(right) => (
                        self.check_expression(right),
                        Self::is_integer_literal(right) || Self::is_integer_literal(&binary.left),
                    ),
                    Err(()) => (Type::Error, false),
                };
                self.binary_type(
                    binary.operator.value,
                    left,
                    right,
                    literal,
                    binary.span.clone(),
                    Some(EntityID::from(*binary)),
                )
            }
            Expression::Unary(unary) => {
                let operand = match &unary.operand {
                    Ok(operand) => self.check_expression(operand),
                    Err(()) => Type::Error,
                };
                self.unary_type(
                    unary.operator.value,
                    operand,
                    unary.span.clone(),
                    Some(EntityID::from(*unary)),
                )
            }
            Expression::Is(is) => {
                let value = self.check_expression(&is.value);
                if let Ok(pattern) = &is.pattern {
                    self.check_pattern(pattern, &value);
                }
                self.corlib("Boolean")
            }
            Expression::As(as_expression) => {
                self.check_expression(&as_expression.value);
                match &as_expression.target_type {
                    Ok(target) => self.resolve_type(target),
                    Err(()) => Type::Error,
                }
            }
            Expression::Cast(cast) => {
                if let Ok(value) = &cast.value {
                    self.check_expression(value);
                }
                // explicit conversions go unvalidated for now: a bad cast is a
                // runtime question more often than a static one
                self.resolve_type(&cast.target_type)
            }
            Expression::Throw(throw) => {
                if let Ok(value) = &throw.value {
                    self.check_expression(value);
                }
                // a throw expression has no value; Error converts everywhere,
                // matching C#'s "convertible to any type"
                Type::Error
            }
            Expression::Switch(switch) => {
                let value = self.check_expression(&switch.value);
                let mut arm_types: Vec<Type> = Vec::new();
                if let Ok(arms) = switch.arms {
                    for arm in arms {
                        self.locals.push(HashMap::new());
                        self.check_pattern(&arm.pattern, &value);
                        if let Some(guard) = &arm.guard {
                            self.check_condition(guard);
                        }
                        if let Ok(arm_value) = &arm.value {
                            arm_types.push(self.check_expression_expecting(arm_value, expected));
                        }
                        self.locals.pop();
                    }
                }
                if arm_types.is_empty() {
                    return Type::Error;
                }

                let system = self.system();
                match best_common_type(&system, &arm_types) {
                    Some(common) => common,
                    None if arm_types.iter().all(|arm| matches!(arm, Type::Null)) => Type::Null,
                    None => {
                        let first = arm_types[0].clone();
                        let clash = arm_types
                            .iter()
                            .find(|arm| {
                                best_common_type(&system, &[first.clone(), (*arm).clone()])
                                    .is_none()
                            })
                            .cloned()
                            .unwrap_or_else(|| first.clone());
                        let kind = SemanticErrorKind::TypeMismatch {
                            expected: self.display(&first),
                            found: self.display(&clash),
                        };
                        self.error(kind, switch.span.clone());
                        Type::Error
                    }
                }
            }
            Expression::Declaration(declaration) => {
                // deconstruction targets; not modelled yet
                self.error(
                    SemanticErrorKind::UnsupportedExpression,
                    declaration.span.clone(),
                );
                Type::Error
            }
            Expression::Lambda(lambda) => self.check_lambda(lambda, expected),
            Expression::AnonymousMethod(method) => {
                self.error(
                    SemanticErrorKind::UnsupportedExpression,
                    method.span.clone(),
                );
                Type::Error
            }
            Expression::Await(await_expression) => {
                if let Ok(value) = &await_expression.value {
                    self.check_expression(value);
                }
                self.error(
                    SemanticErrorKind::UnsupportedExpression,
                    await_expression.span.clone(),
                );
                Type::Error
            }
            Expression::Range(range) => {
                if let Some(start) = &range.start {
                    self.check_expression(start);
                }
                if let Some(end) = &range.end {
                    self.check_expression(end);
                }
                self.error(SemanticErrorKind::UnsupportedExpression, range.span.clone());
                Type::Error
            }
            Expression::With(with) => {
                let ty = self.check_expression(&with.value);
                self.error(SemanticErrorKind::UnsupportedExpression, with.span.clone());
                ty
            }
            Expression::Query(query) => {
                self.error(SemanticErrorKind::UnsupportedExpression, query.span.clone());
                Type::Error
            }
            Expression::Ref(reference) => {
                if let Ok(value) = &reference.value {
                    self.check_expression(value)
                } else {
                    Type::Error
                }
            }
        }
    }

    // ------------------------------------------------------------- primary

    fn check_primary(
        &mut self,
        primary: &'ast PrimaryExpression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Type {
        // the context's target type applies to the head only when nothing follows it
        let head_expected = if primary.chain.is_empty() {
            expected
        } else {
            None
        };
        let mut meaning = self.check_primary_left(&primary.left, head_expected);
        for right in primary.chain {
            meaning = self.apply_primary_right(meaning, right);
        }
        self.value_of(meaning, primary.span.clone(), expected)
    }

    fn value_of(
        &mut self,
        meaning: Meaning<'ast>,
        span: Range<usize>,
        expected: Option<&Type>,
    ) -> Type {
        match meaning {
            Meaning::Value(ty) => ty,
            Meaning::TypeName(_) => {
                self.error(SemanticErrorKind::TypeUsedAsValue, span);
                Type::Error
            }
            Meaning::Namespace(_) => {
                self.error(SemanticErrorKind::NamespaceUsedAsValue, span);
                Type::Error
            }
            Meaning::Group(group) => {
                // a method group converts to a delegate type
                if let Some(expected) = expected
                    && let Some(delegate) = self.delegate_signature(expected)
                {
                    let system = self.system();
                    let compatible =
                        group
                            .candidates
                            .iter()
                            .any(|candidate| match &candidate.signature {
                                Some(MemberSignature::Function(function)) => {
                                    function.parameters.len() == delegate.parameters.len()
                                        && function.parameters.iter().zip(&delegate.parameters).all(
                                            |(method, target)| {
                                                system.is_implicitly_convertible(
                                                    &target.parameter_type,
                                                    &method.parameter_type,
                                                )
                                            },
                                        )
                                        && (delegate.return_type == Type::Void
                                            || system.is_implicitly_convertible(
                                                &function.return_type,
                                                &delegate.return_type,
                                            ))
                                }
                                _ => false,
                            });
                    if compatible {
                        return expected.clone();
                    }
                    self.error(SemanticErrorKind::NoMatchingOverload, group.span);
                    return Type::Error;
                }

                // C# 10 natural function type for a unique non-generic candidate
                if group.candidates.len() == 1
                    && group.candidates[0].arity == 0
                    && let Some(MemberSignature::Function(function)) =
                        group.candidates[0].signature.clone()
                {
                    let parameters: Vec<Type> = function
                        .parameters
                        .iter()
                        .map(|parameter| parameter.parameter_type.clone())
                        .collect();
                    if let Some(ty) = self.func_or_action(&parameters, &function.return_type) {
                        return ty;
                    }
                }

                self.error(SemanticErrorKind::TypeAnnotationNeeded, group.span);
                Type::Error
            }
            Meaning::Error => Type::Error,
        }
    }

    fn check_primary_left(
        &mut self,
        left: &'ast PrimaryLeft<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Meaning<'ast> {
        match left {
            PrimaryLeft::Literal(literal) => self.check_literal(literal),
            PrimaryLeft::Identifier {
                name,
                generics,
                span,
            } => {
                let explicit_arguments = self.explicit_arguments(generics);

                // locals and parameters shadow everything
                if generics.is_none()
                    && let Some(ty) = self.local(name.value).cloned()
                {
                    self.targets
                        .insert(EntityID::from(left), ResolvedTarget::Local);
                    return Meaning::Value(ty);
                }

                // members of the enclosing type (inherited included)
                if let Some(this_type) = self.this_type.clone() {
                    let candidates = self.system().members_named(&this_type, name.value);
                    if !candidates.is_empty()
                        && let Some(meaning) = self.member_meaning(
                            candidates,
                            AccessContext {
                                receiver: Some(this_type.clone()),
                                via_type: false,
                                implicit_this: true,
                            },
                            explicit_arguments.clone(),
                            name.value,
                            span,
                            Some(EntityID::from(left)),
                        )
                    {
                        return meaning;
                    }
                }

                // otherwise a type or namespace name
                let arity = explicit_arguments.len() as u32;
                match self.lookup_name(name.value, arity, span) {
                    Some(Resolution::Type { target, arguments }) => {
                        let mut all = arguments;
                        all.extend(explicit_arguments);
                        Meaning::TypeName(Type::Named {
                            target,
                            arguments: all,
                        })
                    }
                    Some(Resolution::TypeParameter(symbol)) => {
                        Meaning::TypeName(Type::TypeParameter(symbol))
                    }
                    Some(resolution @ Resolution::Namespace { .. }) => {
                        Meaning::Namespace(resolution)
                    }
                    Some(Resolution::Error) => Meaning::Error,
                    None => {
                        self.error(SemanticErrorKind::UnknownIdentifier, span.clone());
                        Meaning::Error
                    }
                }
            }
            PrimaryLeft::Predefined(predefined) => {
                Meaning::TypeName(self.resolver.resolve_predefined(predefined))
            }
            PrimaryLeft::Global(_) => Meaning::Namespace(Resolution::Namespace {
                path: Vec::new(),
                symbol: Some(self.resolver.declarations.table.root()),
            }),
            PrimaryLeft::This(span) => match self.this_type.clone() {
                Some(ty) if !self.static_context => Meaning::Value(ty),
                Some(_) => {
                    self.error(
                        SemanticErrorKind::InstanceMemberInStaticContext,
                        span.clone(),
                    );
                    Meaning::Error
                }
                None => {
                    self.error(SemanticErrorKind::UnknownIdentifier, span.clone());
                    Meaning::Error
                }
            },
            PrimaryLeft::Base(span) => match self
                .this_type
                .clone()
                .and_then(|this_type| self.system().base_of(&this_type))
            {
                Some(base) if !self.static_context => Meaning::Value(base),
                _ => {
                    self.error(
                        SemanticErrorKind::InstanceMemberInStaticContext,
                        span.clone(),
                    );
                    Meaning::Error
                }
            },
            PrimaryLeft::Parenthesized { expression, .. } => {
                Meaning::Value(self.check_expression(expression))
            }
            PrimaryLeft::Tuple { elements, .. } => {
                let elements = elements
                    .iter()
                    .map(|element| TupleElement {
                        name: element.name.as_ref().map(|name| name.value.into()),
                        element: self.check_expression(&element.value),
                    })
                    .collect();
                Meaning::Value(Type::Tuple(elements))
            }
            PrimaryLeft::New(new_expression) => self.check_new(new_expression, expected),
            PrimaryLeft::Typeof { target_type, .. } => {
                // resolving it is what records the type, which is what code
                // generation needs: on Udon a `typeof` is a value built from
                // the named type, not a compile-time-only fact
                if let Ok(target_type) = target_type {
                    self.resolve_type(target_type);
                }
                Meaning::Value(self.corlib("Type"))
            }
            PrimaryLeft::Sizeof { .. } => Meaning::Value(self.corlib("Int32")),
            // nameof's operand may be a method group or type; C# only reads its
            // spelling, so it goes unchecked here
            PrimaryLeft::Nameof { .. } => Meaning::Value(self.corlib("String")),
            PrimaryLeft::Default {
                target_type, span, ..
            } => match (target_type, expected) {
                (Some(target_type), _) => {
                    let target = self.resolve_type(target_type);
                    Meaning::Value(target)
                }
                (None, Some(expected)) => {
                    // a bare `default` has no syntax of its own to hang the
                    // type on; the code generator reads it from here
                    self.expression_types
                        .insert(EntityID::from(left), expected.clone());
                    Meaning::Value(expected.clone())
                }
                (None, None) => {
                    self.error(SemanticErrorKind::TypeAnnotationNeeded, span.clone());
                    Meaning::Error
                }
            },
            PrimaryLeft::Checked { value, .. } => match value {
                Ok(value) => Meaning::Value(self.check_expression(value)),
                Err(()) => Meaning::Error,
            },
            PrimaryLeft::AnonymousObject { span, .. } | PrimaryLeft::Collection { span, .. } => {
                self.error(SemanticErrorKind::UnsupportedExpression, span.clone());
                Meaning::Error
            }
            PrimaryLeft::Stackalloc(stackalloc) => {
                self.error(
                    SemanticErrorKind::UnsupportedExpression,
                    stackalloc.span.clone(),
                );
                Meaning::Error
            }
        }
    }

    fn check_literal(&mut self, literal: &'ast LiteralExpression<'ast, 'ast>) -> Meaning<'ast> {
        let ty = match literal {
            LiteralExpression::Integer(text) => {
                let stripped = text.value.trim_end_matches(['u', 'U', 'l', 'L']);
                let suffix = &text.value[stripped.len()..].to_ascii_lowercase();
                let name = match (suffix.contains('u'), suffix.contains('l')) {
                    (true, true) => "UInt64",
                    (true, false) => "UInt32",
                    (false, true) => "Int64",
                    (false, false) => "Int32",
                };
                self.corlib(name)
            }
            LiteralExpression::Real(text) => {
                let name = match text.value.chars().last().map(|c| c.to_ascii_lowercase()) {
                    Some('f') => "Single",
                    Some('m') => "Decimal",
                    _ => "Double",
                };
                self.corlib(name)
            }
            LiteralExpression::Char(_) => self.corlib("Char"),
            LiteralExpression::String(_)
            | LiteralExpression::VerbatimString(_)
            | LiteralExpression::RawString(_) => self.corlib("String"),
            LiteralExpression::InterpolatedString(interpolated) => {
                for part in interpolated.parts {
                    if let InterpolationPart::Hole(hole) = part {
                        if let Ok(expression) = &hole.expression {
                            self.check_expression(expression);
                        }
                        if let Some(alignment) = &hole.alignment {
                            self.check_expression(alignment);
                        }
                    }
                }
                self.corlib("String")
            }
            LiteralExpression::True(_) | LiteralExpression::False(_) => self.corlib("Boolean"),
            LiteralExpression::Null(_) => Type::Null,
        };
        Meaning::Value(ty)
    }

    fn explicit_arguments(
        &mut self,
        generics: &Option<men_sharp_parser::ast::GenericsInfo<'ast, 'ast>>,
    ) -> Vec<Type> {
        generics
            .as_ref()
            .map(|generics| {
                generics
                    .types
                    .iter()
                    .map(|argument| self.resolve_type(argument))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Turns member candidates into a meaning: a value for a field/property, a
    /// group for methods. `None` when the candidates were only types (the caller
    /// falls through to type lookup).
    fn member_meaning(
        &mut self,
        candidates: Vec<MemberCandidate>,
        access: AccessContext,
        explicit_arguments: Vec<Type>,
        name: &'ast str,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Meaning<'ast>> {
        let AccessContext {
            receiver,
            via_type,
            implicit_this,
        } = access;
        let methods: Vec<MemberCandidate> = candidates
            .iter()
            .filter(|candidate| {
                matches!(candidate.kind, SymbolKind::Method | SymbolKind::Constructor)
            })
            .cloned()
            .collect();
        if !methods.is_empty() {
            let receiver_display = receiver
                .as_ref()
                .map(|ty| self.display(ty))
                .unwrap_or_default();
            return Some(Meaning::Group(MethodGroup {
                candidates: methods,
                explicit_arguments,
                via_type,
                name,
                allow_extensions: !via_type && !implicit_this && receiver.is_some(),
                receiver: receiver.clone(),
                receiver_display,
                span: span.clone(),
            }));
        }

        // nearest non-method candidate decides
        let first = candidates.first()?;
        if first.kind.is_type() {
            // a nested type name
            if let MemberOrigin::Source(symbol) = first.origin {
                return Some(Meaning::TypeName(Type::Named {
                    target: TypeTarget::Source(symbol),
                    arguments: explicit_arguments,
                }));
            }
            return None;
        }

        // static/instance agreement
        if via_type && !first.is_static && first.kind != SymbolKind::EnumMember {
            self.error(
                SemanticErrorKind::InstanceMemberInStaticContext,
                span.clone(),
            );
            return Some(Meaning::Error);
        }
        if !via_type
            && first.is_static
            && receiver.is_some()
            && self.receiver_is_instance(&receiver)
        {
            // static member through `this` is fine in C# only via plain name;
            // through an explicit instance it is CS0176
            if self.static_context {
                self.error(SemanticErrorKind::StaticMemberViaInstance, span.clone());
            }
        }
        // through the implicit `this` there must actually be a `this`
        if implicit_this && !first.is_static && self.static_context {
            self.error(
                SemanticErrorKind::InstanceMemberInStaticContext,
                span.clone(),
            );
            return Some(Meaning::Error);
        }

        let ty = match (&first.signature, first.kind) {
            (_, SymbolKind::EnumMember) => first.declaring_type.clone(),
            (Some(MemberSignature::Field(ty)), _)
            | (Some(MemberSignature::Property(ty)), _)
            | (Some(MemberSignature::Event(ty)), _) => ty.clone(),
            _ => {
                let _ = name;
                Type::Error
            }
        };
        if let Some(node) = node {
            self.targets.insert(
                node,
                ResolvedTarget::Member(ResolvedMember {
                    origin: first.origin.clone(),
                    kind: first.kind,
                    is_static: first.is_static,
                    declaring_type: first.declaring_type.clone(),
                    member_type: ty.clone(),
                }),
            );
        }
        Some(Meaning::Value(ty))
    }

    fn is_struct(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.resolver.external.type_info(*id).kind == ExternalTypeKind::Struct,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => matches!(
                self.resolver.declarations.table.symbol(*symbol).kind,
                SymbolKind::Struct | SymbolKind::RecordStruct
            ),
            _ => false,
        }
    }

    fn receiver_is_instance(&self, _receiver: &Option<Type>) -> bool {
        false // refined when expression shapes are tracked further
    }

    fn apply_primary_right(
        &mut self,
        meaning: Meaning<'ast>,
        right: &'ast PrimaryRight<'ast, 'ast>,
    ) -> Meaning<'ast> {
        match right {
            PrimaryRight::Member {
                name,
                generics,
                span,
                ..
            } => {
                let Ok(name) = name else {
                    return Meaning::Error;
                };
                let explicit_arguments = self.explicit_arguments(generics);
                self.access_member(
                    meaning,
                    name.value,
                    explicit_arguments,
                    span,
                    Some(EntityID::from(right)),
                )
            }
            PrimaryRight::Invocation { arguments, span } => self.invoke(
                meaning,
                arguments.arguments,
                span,
                Some(EntityID::from(right)),
            ),
            PrimaryRight::ElementAccess {
                arguments, span, ..
            } => {
                let receiver = self.value_of(meaning, span.clone(), None);
                self.index(
                    receiver,
                    arguments.arguments,
                    span,
                    Some(EntityID::from(right)),
                )
            }
            PrimaryRight::Postfix { operator, span } => {
                let ty = self.value_of(meaning, span.clone(), None);
                use men_sharp_parser::ast::PostfixOperator;
                match operator.value {
                    PostfixOperator::Increment | PostfixOperator::Decrement => {
                        let increment = operator.value == PostfixOperator::Increment;
                        Meaning::Value(self.postfix_step_type(
                            increment,
                            ty,
                            span,
                            EntityID::from(right),
                        ))
                    }
                    PostfixOperator::NullForgiving => Meaning::Value(match ty {
                        Type::Nullable(inner) => *inner,
                        other => other,
                    }),
                }
            }
        }
    }

    fn access_member(
        &mut self,
        meaning: Meaning<'ast>,
        name: &'ast str,
        explicit_arguments: Vec<Type>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        match meaning {
            Meaning::Namespace(resolution) => {
                let arity = explicit_arguments.len() as u32;
                match self.resolver.lookup_member(resolution, name, arity, span) {
                    Resolution::Type { target, arguments } => {
                        let mut all = arguments;
                        all.extend(explicit_arguments);
                        Meaning::TypeName(Type::Named {
                            target,
                            arguments: all,
                        })
                    }
                    resolution @ Resolution::Namespace { .. } => Meaning::Namespace(resolution),
                    _ => Meaning::Error,
                }
            }
            Meaning::TypeName(ty) => {
                let candidates = self.system().members_named(&ty, name);
                if candidates.is_empty() {
                    // maybe a nested type instead of a member
                    if let Some(nested) =
                        self.nested_type_of(&ty, name, explicit_arguments.len() as u32)
                    {
                        return Meaning::TypeName(Type::Named {
                            target: nested,
                            arguments: explicit_arguments,
                        });
                    }
                    let kind = SemanticErrorKind::UnknownMember {
                        type_name: self.display(&ty),
                    };
                    self.error(kind, span.clone());
                    return Meaning::Error;
                }
                self.member_meaning(
                    candidates,
                    AccessContext {
                        receiver: Some(ty),
                        via_type: true,
                        implicit_this: false,
                    },
                    explicit_arguments,
                    name,
                    span,
                    node,
                )
                .unwrap_or(Meaning::Error)
            }
            Meaning::Value(ty) => {
                // `?.` and `.` are treated alike for now
                let receiver = match ty {
                    Type::Nullable(inner) => *inner,
                    other => other,
                };
                if matches!(receiver, Type::Error) {
                    return Meaning::Error;
                }

                let candidates = self.system().members_named(&receiver, name);
                if candidates.is_empty() {
                    // no instance member: maybe an extension method
                    let probe = MethodGroup {
                        candidates: Vec::new(),
                        explicit_arguments,
                        via_type: false,
                        name,
                        receiver: Some(receiver.clone()),
                        allow_extensions: true,
                        receiver_display: self.display(&receiver),
                        span: span.clone(),
                    };
                    if self.extension_group(&probe).is_some() {
                        return Meaning::Group(probe);
                    }
                    let kind = SemanticErrorKind::UnknownMember {
                        type_name: self.display(&receiver),
                    };
                    self.error(kind, span.clone());
                    return Meaning::Error;
                }
                self.member_meaning(
                    candidates,
                    AccessContext {
                        receiver: Some(receiver),
                        via_type: false,
                        implicit_this: false,
                    },
                    explicit_arguments,
                    name,
                    span,
                    node,
                )
                .unwrap_or(Meaning::Error)
            }
            Meaning::Group(group) => {
                self.error(SemanticErrorKind::UnsupportedExpression, group.span);
                Meaning::Error
            }
            Meaning::Error => Meaning::Error,
        }
    }

    fn nested_type_of(&self, ty: &Type, name: &str, arity: u32) -> Option<TypeTarget> {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => self
                .resolver
                .declarations
                .table
                .symbol(*symbol)
                .members_named(name)
                .iter()
                .copied()
                .find(|&id| {
                    let entry = self.resolver.declarations.table.symbol(id);
                    entry.kind.is_type() && entry.arity == arity
                })
                .map(TypeTarget::Source),
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self
                .resolver
                .external
                .find_nested_type(*id, name, arity)
                .map(TypeTarget::External),
            _ => None,
        }
    }

    // ------------------------------------------------------------- invoking

    fn invoke(
        &mut self,
        meaning: Meaning<'ast>,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        match meaning {
            Meaning::Group(group) => {
                let call_arguments = self.check_arguments(arguments);
                Meaning::Value(self.resolve_call(group, call_arguments, span, node))
            }
            Meaning::Value(ty) => {
                // calling a value: a delegate invocation
                let invoke = self
                    .system()
                    .members_named(&ty, "Invoke")
                    .into_iter()
                    .find(|candidate| candidate.kind == SymbolKind::Method);
                match invoke {
                    Some(candidate) => {
                        let receiver_display = self.display(&ty);
                        let group = MethodGroup {
                            candidates: vec![candidate],
                            explicit_arguments: Vec::new(),
                            via_type: false,
                            name: "Invoke",
                            receiver: None,
                            allow_extensions: false,
                            receiver_display,
                            span: span.clone(),
                        };
                        let call_arguments = self.check_arguments(arguments);
                        Meaning::Value(self.resolve_call(group, call_arguments, span, node))
                    }
                    None => {
                        if !matches!(ty, Type::Error) {
                            let kind = SemanticErrorKind::NotCallable {
                                type_name: self.display(&ty),
                            };
                            self.error(kind, span.clone());
                        }
                        Meaning::Error
                    }
                }
            }
            Meaning::TypeName(ty) => {
                let kind = SemanticErrorKind::NotCallable {
                    type_name: self.display(&ty),
                };
                self.error(kind, span.clone());
                Meaning::Error
            }
            Meaning::Namespace(_) => {
                self.error(SemanticErrorKind::NamespaceUsedAsValue, span.clone());
                Meaning::Error
            }
            Meaning::Error => {
                // still check the arguments so their sub-errors surface
                self.check_arguments(arguments);
                Meaning::Error
            }
        }
    }

    fn check_arguments(
        &mut self,
        arguments: &'ast [Argument<'ast, 'ast>],
    ) -> Vec<CallArgument<'ast>> {
        arguments
            .iter()
            .map(|argument| self.check_argument_expression(argument))
            .collect()
    }

    fn check_argument_expression(
        &mut self,
        argument: &'ast Argument<'ast, 'ast>,
    ) -> CallArgument<'ast> {
        match &argument.value {
            ArgumentValue::Expression(expression) => {
                if let Expression::Lambda(lambda) = expression {
                    return CallArgument {
                        shape: ArgumentShape::Lambda(lambda),
                        name: argument.name.as_ref().map(|name| name.value),
                        expression: Some(expression),
                        modifier: argument.modifier.as_ref().map(|modifier| modifier.value),
                        is_integer_literal: false,
                        out_declaration: None,
                        span: argument.span.clone(),
                    };
                }
                CallArgument {
                    shape: ArgumentShape::Value(self.check_expression(expression)),
                    name: argument.name.as_ref().map(|name| name.value),
                    expression: Some(expression),
                    modifier: argument.modifier.as_ref().map(|modifier| modifier.value),
                    is_integer_literal: Self::is_integer_literal(expression),
                    out_declaration: None,
                    span: argument.span.clone(),
                }
            }
            ArgumentValue::Declaration {
                variable_type,
                name,
                ..
            } => {
                let declared = self.resolve_type(variable_type);
                let infer = matches!(declared, Type::Infer);
                if !infer {
                    self.declare_local(name.value, declared.clone());
                }
                CallArgument {
                    shape: ArgumentShape::Value(if infer { Type::Infer } else { declared }),
                    name: argument.name.as_ref().map(|name| name.value),
                    expression: None,
                    modifier: Some(ArgumentModifier::Out),
                    is_integer_literal: false,
                    out_declaration: Some((name.value, infer)),
                    span: argument.span.clone(),
                }
            }
            ArgumentValue::Missing => CallArgument {
                shape: ArgumentShape::Value(Type::Error),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                span: argument.span.clone(),
            },
        }
    }

    /// Overload resolution over a method group, §12.6.3 inference included; when
    /// instance candidates fail, extension methods in scope get their turn with the
    /// receiver as first argument (C# §12.8.10.3).
    fn resolve_call(
        &mut self,
        group: MethodGroup<'ast>,
        arguments: Vec<CallArgument<'ast>>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        let instance_failure = match self.attempt_call(&group, &arguments) {
            AttemptOutcome::Selected(selected) => {
                self.record_call(node, &group, &selected, false);
                return self.finish_call(&selected, &arguments);
            }
            AttemptOutcome::Ambiguous => {
                self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                return Type::Error;
            }
            AttemptOutcome::NoMatch { inference_failed } => inference_failed,
        };

        if group.allow_extensions
            && let Some(receiver) = group.receiver.clone()
            && let Some(extension_group) = self.extension_group(&group)
        {
            let mut extension_arguments = Vec::with_capacity(arguments.len() + 1);
            extension_arguments.push(CallArgument {
                shape: ArgumentShape::Value(receiver),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                span: span.clone(),
            });
            extension_arguments.extend(arguments);

            match self.attempt_call(&extension_group, &extension_arguments) {
                AttemptOutcome::Selected(selected) => {
                    self.record_call(node, &extension_group, &selected, true);
                    return self.finish_call(&selected, &extension_arguments);
                }
                AttemptOutcome::Ambiguous => {
                    self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                    return Type::Error;
                }
                AttemptOutcome::NoMatch { inference_failed } => {
                    self.report_call_failure(
                        &group,
                        &extension_arguments[1..],
                        instance_failure || inference_failed,
                        span,
                    );
                    return Type::Error;
                }
            }
        }

        self.report_call_failure(&group, &arguments, instance_failure, span);
        Type::Error
    }

    /// Remembers which overload a call node bound to, for the code generator.
    fn record_call(
        &mut self,
        node: Option<EntityID>,
        group: &MethodGroup<'ast>,
        selected: &SelectedOverload,
        is_extension: bool,
    ) {
        let Some(node) = node else {
            return;
        };
        let call = self.resolved_call_of(group, selected, is_extension);
        self.targets.insert(node, ResolvedTarget::Call(call));
    }

    fn resolved_call_of(
        &self,
        group: &MethodGroup<'ast>,
        selected: &SelectedOverload,
        is_extension: bool,
    ) -> ResolvedCall {
        let candidate = &group.candidates[selected.candidate];
        ResolvedCall {
            origin: candidate.origin.clone(),
            is_static: candidate.is_static,
            is_extension,
            declaring_type: candidate.declaring_type.clone(),
            signature: selected.signature.clone(),
            type_arguments: selected.type_arguments.clone(),
            parameter_of_argument: selected.parameter_of_argument.clone(),
            params_expansion: selected.params_expansion,
        }
    }

    fn report_call_failure(
        &mut self,
        group: &MethodGroup<'ast>,
        arguments: &[CallArgument<'ast>],
        inference_failed: bool,
        span: &Range<usize>,
    ) {
        let kind = if group.candidates.is_empty() {
            SemanticErrorKind::UnknownMember {
                type_name: group.receiver_display.clone(),
            }
        } else if inference_failed {
            SemanticErrorKind::CannotInferTypeArguments
        } else {
            SemanticErrorKind::NoMatchingOverload
        };
        self.error(kind, span.clone());

        for argument in arguments {
            if let (ArgumentShape::Lambda(_), Some(expression)) =
                (&argument.shape, argument.expression)
            {
                self.expression_types
                    .insert(EntityID::from(expression), Type::Error);
            }
        }
    }

    /// One pass of candidate filtering and betterness. Side-effect free apart from
    /// rolled-back lambda probes.
    /// Which parameter each written argument binds to (§12.6.4.2): a
    /// positional argument takes its own position, a named one the parameter
    /// of that name. `None` when the shape does not fit this candidate — an
    /// unknown name, a parameter given twice, or a positional argument after
    /// a named one that moved out of place (C# 7.2 allows positional
    /// arguments after named ones only while the named ones sit where they
    /// would have anyway).
    fn bind_argument_names(
        arguments: &[CallArgument<'ast>],
        parameters: &[crate::types::ParameterSignature],
    ) -> Option<Vec<usize>> {
        let mut used = vec![false; parameters.len()];
        let mut bound = Vec::with_capacity(arguments.len());
        let mut names_moved = false;
        for (position, argument) in arguments.iter().enumerate() {
            let index = match argument.name {
                None => {
                    if names_moved {
                        return None;
                    }
                    position
                }
                Some(name) => {
                    let index = parameters
                        .iter()
                        .position(|parameter| parameter.name.as_deref() == Some(name))?;
                    if index != position {
                        names_moved = true;
                    }
                    index
                }
            };
            if index >= used.len() || used[index] {
                return None;
            }
            used[index] = true;
            bound.push(index);
        }
        Some(bound)
    }

    fn attempt_call(
        &mut self,
        group: &MethodGroup<'ast>,
        arguments: &[CallArgument<'ast>],
    ) -> AttemptOutcome {
        let mut viable: Vec<Applicable> = Vec::new();
        let mut inference_failed = false;

        for (candidate_index, candidate) in group.candidates.iter().enumerate() {
            // normal form first; a candidate applicable that way is not also
            // considered in its expanded form (§12.6.4.2)
            match self.try_candidate(group, candidate, candidate_index, arguments, false) {
                Ok(applicable) => {
                    viable.push(applicable);
                    continue;
                }
                Err(failed) => inference_failed |= failed,
            }
            let takes_params = matches!(
                &candidate.signature,
                Some(MemberSignature::Function(function))
                    if function.parameters.last().is_some_and(|parameter| parameter.is_params)
            );
            if takes_params {
                match self.try_candidate(group, candidate, candidate_index, arguments, true) {
                    Ok(applicable) => viable.push(applicable),
                    Err(failed) => inference_failed |= failed,
                }
            }
        }

        // most exact matches wins; then normal form over expanded; then the
        // one that fills in the fewest defaults (§12.6.4.3)
        let rank = |applicable: &Applicable| {
            (
                applicable.exact,
                !applicable.expanded,
                usize::MAX - applicable.omitted,
            )
        };
        let Some(best) = viable.iter().map(rank).max() else {
            return AttemptOutcome::NoMatch { inference_failed };
        };
        let mut winners = viable
            .into_iter()
            .filter(|applicable| rank(applicable) == best);
        let selected = winners.next().unwrap().selected;
        if winners.next().is_some() {
            return AttemptOutcome::Ambiguous;
        }
        AttemptOutcome::Selected(selected)
    }

    /// One candidate against the arguments, in normal form or — with the
    /// trailing `params T[]` opened up into one `T` parameter per remaining
    /// argument — in expanded form. `Err(true)` when type inference is what
    /// failed (the diagnostic differs), `Err(false)` otherwise.
    fn try_candidate(
        &mut self,
        group: &MethodGroup<'ast>,
        candidate: &MemberCandidate,
        candidate_index: usize,
        arguments: &[CallArgument<'ast>],
        expanded: bool,
    ) -> Result<Applicable, bool> {
        use crate::types::{ParameterPassing, ParameterSignature};
        let Some(MemberSignature::Function(declared)) = &candidate.signature else {
            return Err(false);
        };
        if group.via_type && !candidate.is_static {
            return Err(false);
        }

        let fixed = declared.parameters.len().saturating_sub(1);
        let working: FunctionSignature = if expanded {
            let Some(last) = declared
                .parameters
                .last()
                .filter(|parameter| parameter.is_params)
            else {
                return Err(false);
            };
            let Type::Array { element, rank: 1 } = &last.parameter_type else {
                return Err(false);
            };
            let mut parameters = declared.parameters[..fixed].to_vec();
            for _ in 0..arguments.len().saturating_sub(fixed) {
                parameters.push(ParameterSignature {
                    passing: ParameterPassing::Value,
                    is_params: false,
                    parameter_type: (**element).clone(),
                    name: None,
                    default_value: None,
                });
            }
            FunctionSignature {
                return_type: declared.return_type.clone(),
                parameters,
            }
        } else {
            declared.clone()
        };

        if arguments.len() > working.parameters.len() {
            return Err(false);
        }
        // named arguments pick their parameter; the rest go by position
        let Some(parameter_of_argument) = Self::bind_argument_names(arguments, &working.parameters)
        else {
            return Err(false);
        };
        // whatever is left unbound must be optional (§12.6.4.2); a `params`
        // parameter left out is the expanded form's business, not a default
        let omitted = working
            .parameters
            .iter()
            .enumerate()
            .filter(|(index, _)| !parameter_of_argument.contains(index))
            .count();
        if working
            .parameters
            .iter()
            .enumerate()
            .any(|(index, parameter)| {
                !parameter_of_argument.contains(&index) && parameter.default_value.is_none()
            })
        {
            return Err(false);
        }
        let pairs: Vec<(usize, usize)> =
            parameter_of_argument.iter().copied().enumerate().collect();

        // the method's own generic parameters: explicit, inferred, or absent
        let (working, declared, type_arguments) = if candidate.arity > 0 {
            let keys = method_parameter_keys(&self.system(), candidate);
            let mut engine = Inference::new(keys.clone());

            if !group.explicit_arguments.is_empty() {
                if group.explicit_arguments.len() != keys.len() {
                    return Err(false);
                }
                for (key, ty) in keys.iter().zip(&group.explicit_arguments) {
                    engine.preset(*key, ty.clone());
                }
            } else {
                // phase 1: ordinary arguments contribute bounds
                for &(argument_index, parameter_index) in &pairs {
                    let argument = &arguments[argument_index];
                    let parameter = &working.parameters[parameter_index];
                    if let ArgumentShape::Value(ty) = &argument.shape {
                        let system = self.system();
                        engine.lower_bound(&system, &parameter.parameter_type, ty);
                    }
                }
                {
                    let system = self.system();
                    engine.fix_where_possible(&system);
                }

                // phase 2: lambda bodies, typed against now-concrete inputs,
                // feed their return types back
                for &(argument_index, parameter_index) in &pairs {
                    let argument = &arguments[argument_index];
                    let parameter = &working.parameters[parameter_index];
                    let ArgumentShape::Lambda(lambda) = &argument.shape else {
                        continue;
                    };
                    let parameter_type = engine.substitute(&parameter.parameter_type);
                    let Some(delegate) = self.delegate_signature(&parameter_type) else {
                        return Err(false);
                    };
                    if delegate
                        .parameters
                        .iter()
                        .any(|parameter| engine.has_unfixed(&parameter.parameter_type))
                    {
                        return Err(true);
                    }
                    if !Self::lambda_shape_matches(lambda, &delegate) {
                        return Err(false);
                    }
                    let Some(returned) = self.probe_lambda_return(lambda, &delegate) else {
                        return Err(false);
                    };
                    let system = self.system();
                    engine.lower_bound(&system, &delegate.return_type, &returned);
                }
                {
                    let system = self.system();
                    engine.fix_where_possible(&system);
                }
                if !engine.all_fixed() {
                    return Err(true);
                }
            }

            let type_arguments: Vec<Type> = keys
                .iter()
                .map(|key| {
                    engine.substitute(&match key {
                        InferenceKey::Source(symbol) => Type::TypeParameter(*symbol),
                        InferenceKey::External(index) => Type::ExternalMethodTypeParameter(*index),
                    })
                })
                .collect();
            let substitute = |signature: &FunctionSignature| match engine
                .substitute_signature(&MemberSignature::Function(signature.clone()))
            {
                MemberSignature::Function(function) => function,
                _ => unreachable!(),
            };
            (substitute(&working), substitute(declared), type_arguments)
        } else {
            if !group.explicit_arguments.is_empty() {
                return Err(false);
            }
            (working, declared.clone(), Vec::new())
        };

        // applicability
        let mut exact = 0usize;
        for &(argument_index, parameter_index) in &pairs {
            let argument = &arguments[argument_index];
            let parameter = &working.parameters[parameter_index];
            let modifier_ok = match parameter.passing {
                ParameterPassing::Ref => argument.modifier == Some(ArgumentModifier::Ref),
                ParameterPassing::Out => argument.modifier == Some(ArgumentModifier::Out),
                ParameterPassing::In => argument
                    .modifier
                    .map(|modifier| modifier == ArgumentModifier::In)
                    .unwrap_or(true),
                ParameterPassing::Value => argument.modifier.is_none(),
            };
            if !modifier_ok {
                return Err(false);
            }

            match &argument.shape {
                ArgumentShape::Value(ty) => {
                    if argument
                        .out_declaration
                        .map(|(_, infer)| infer)
                        .unwrap_or(false)
                    {
                        // `out var x` matches any out parameter
                        continue;
                    }
                    if *ty == parameter.parameter_type {
                        exact += 1;
                        continue;
                    }
                    // `ref`/`out` write through the reference, so the types
                    // must be identical — a conversion would leave the
                    // callee writing into a slot of the wrong type (§12.6.4.2)
                    if matches!(
                        parameter.passing,
                        ParameterPassing::Ref | ParameterPassing::Out
                    ) && !matches!(ty, Type::Error)
                    {
                        return Err(false);
                    }
                    let system = self.system();
                    let convertible = system
                        .is_implicitly_convertible(ty, &parameter.parameter_type)
                        || (argument.is_integer_literal
                            && system
                                .numeric_kind(&parameter.parameter_type)
                                .map(|kind| kind.is_integral())
                                .unwrap_or(false));
                    if !convertible {
                        return Err(false);
                    }
                }
                ArgumentShape::Lambda(lambda) => {
                    let Some(delegate) = self.delegate_signature(&parameter.parameter_type) else {
                        return Err(false);
                    };
                    if !Self::lambda_shape_matches(lambda, &delegate) {
                        return Err(false);
                    }
                }
            }
        }

        Ok(Applicable {
            selected: SelectedOverload {
                signature: declared,
                candidate: candidate_index,
                type_arguments,
                parameter_of_argument,
                params_expansion: expanded.then_some(fixed),
            },
            exact,
            omitted,
            expanded,
        })
    }

    /// The chosen overload's side effects: lambda bodies checked for real,
    /// `out var` locals bound, argument expressions typed.
    fn finish_call(
        &mut self,
        selected: &SelectedOverload,
        arguments: &[CallArgument<'ast>],
    ) -> Type {
        let signature = &selected.signature;
        for (argument_index, argument) in arguments.iter().enumerate() {
            let parameter_index = selected
                .parameter_of_argument
                .get(argument_index)
                .copied()
                .unwrap_or(argument_index);
            let expanded_element = selected
                .params_expansion
                .filter(|&fixed| parameter_index >= fixed)
                .and_then(|fixed| signature.parameters.get(fixed))
                .and_then(|parameter| match &parameter.parameter_type {
                    Type::Array { element, .. } => Some((**element).clone()),
                    _ => None,
                });
            let element_parameter;
            let parameter = match expanded_element {
                Some(element) => {
                    element_parameter = crate::types::ParameterSignature {
                        passing: crate::types::ParameterPassing::Value,
                        is_params: false,
                        parameter_type: element,
                        name: None,
                        default_value: None,
                    };
                    &element_parameter
                }
                None => match signature.parameters.get(parameter_index) {
                    Some(parameter) => parameter,
                    None => continue,
                },
            };
            match &argument.shape {
                ArgumentShape::Lambda(lambda) => {
                    if let Some(delegate) = self.delegate_signature(&parameter.parameter_type) {
                        self.check_lambda_against(lambda, &delegate);
                    }
                    if let Some(expression) = argument.expression {
                        self.expression_types
                            .insert(EntityID::from(expression), parameter.parameter_type.clone());
                    }
                }
                ArgumentShape::Value(_) => {
                    if let Some((name, true)) = argument.out_declaration {
                        self.declare_local(name, parameter.parameter_type.clone());
                    }
                }
            }
        }
        signature.return_type.clone()
    }

    /// The extension methods named like this group's member, gathered from every
    /// namespace scope and `using` import, nearest scope first.
    fn extension_group(&self, group: &MethodGroup<'ast>) -> Option<MethodGroup<'ast>> {
        let name = group.name;

        let mut search: Vec<(Vec<&'ast str>, Option<SymbolId>)> = Vec::new();
        for scope in self.scopes.iter().rev() {
            search.push((scope.path.clone(), scope.symbol));
            for using in &scope.usings {
                if let crate::resolve::ResolvedUsing::Namespace(path) = using {
                    let symbol = self.resolver.source_namespace_at(path);
                    search.push((path.clone(), symbol));
                }
            }
        }

        let mut candidates: Vec<MemberCandidate> = Vec::new();
        for (path, namespace_symbol) in &search {
            // source static classes declared in this namespace
            if let Some(namespace_symbol) = namespace_symbol {
                for &class in &self
                    .resolver
                    .declarations
                    .table
                    .symbol(*namespace_symbol)
                    .members
                {
                    if self.resolver.declarations.table.symbol(class).kind != SymbolKind::Class {
                        continue;
                    }
                    let class_type = Type::Named {
                        target: TypeTarget::Source(class),
                        arguments: Vec::new(),
                    };
                    for candidate in self.system().members_named(&class_type, name) {
                        let is_extension = match &candidate.origin {
                            MemberOrigin::Source(id) => {
                                self.resolver.declarations.table.symbol(*id).is_extension
                            }
                            MemberOrigin::External { member, .. } => member.is_extension,
                        };
                        if is_extension && candidate.declaring_type == class_type {
                            candidates.push(candidate);
                        }
                    }
                }
            }

            // external static classes with matching extension methods
            for owner in self.resolver.external.extension_method_owners(path, name) {
                let owner_type = Type::Named {
                    target: TypeTarget::External(owner),
                    arguments: Vec::new(),
                };
                for candidate in self.system().members_named(&owner_type, name) {
                    let is_extension = matches!(
                        &candidate.origin,
                        MemberOrigin::External { member, .. } if member.is_extension
                    );
                    if is_extension && candidate.declaring_type == owner_type {
                        candidates.push(candidate);
                    }
                }
            }
        }

        if candidates.is_empty() {
            return None;
        }
        Some(MethodGroup {
            candidates,
            explicit_arguments: group.explicit_arguments.clone(),
            via_type: true,
            name,
            receiver: None,
            allow_extensions: false,
            receiver_display: group.receiver_display.clone(),
            span: group.span.clone(),
        })
    }

    // -------------------------------------------------------------- lambdas

    /// The `Invoke` shape of a delegate type: what a lambda checks against.
    fn delegate_signature(&self, ty: &Type) -> Option<FunctionSignature> {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                arguments,
            } => {
                if self.resolver.declarations.table.symbol(*symbol).kind != SymbolKind::Delegate {
                    return None;
                }
                let MemberSignature::Function(function) =
                    self.signatures.members.get(symbol)?.clone()
                else {
                    return None;
                };
                let system = self.system();
                match system.instantiate_signature(
                    &MemberSignature::Function(function),
                    &TypeTarget::Source(*symbol),
                    arguments,
                ) {
                    MemberSignature::Function(function) => Some(function),
                    _ => None,
                }
            }
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => {
                if self.resolver.external.type_info(*id).kind != ExternalTypeKind::Delegate {
                    return None;
                }
                self.system()
                    .members_named(ty, "Invoke")
                    .into_iter()
                    .find_map(|candidate| match candidate.signature {
                        Some(MemberSignature::Function(function)) => Some(function),
                        _ => None,
                    })
            }
            _ => None,
        }
    }

    fn lambda_parameter_names(
        lambda: &'ast LambdaExpression<'ast, 'ast>,
    ) -> Vec<Option<&'ast str>> {
        match &lambda.parameters {
            LambdaParameters::Single(name) => vec![Some(name.value)],
            LambdaParameters::List(list) => list
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_ref().ok().map(|name| name.value))
                .collect(),
        }
    }

    fn lambda_shape_matches(
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) -> bool {
        Self::lambda_parameter_names(lambda).len() == delegate.parameters.len()
    }

    /// Types a lambda body with known parameter types to learn its return type,
    /// then rolls every diagnostic back — this is inference, not checking.
    fn probe_lambda_return(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) -> Option<Type> {
        let error_mark = self.resolver.out.errors.len();
        let saved_probe = self.lambda_probe_returns.take();
        let saved_return = std::mem::replace(&mut self.return_type, Type::Infer);

        let mut scope = HashMap::new();
        for (name, parameter) in Self::lambda_parameter_names(lambda)
            .into_iter()
            .zip(&delegate.parameters)
        {
            if let Some(name) = name {
                scope.insert(name, parameter.parameter_type.clone());
            }
        }
        self.locals.push(scope);

        let result = match &lambda.body {
            Ok(LambdaBody::Expression(expression)) => Some(self.check_expression(expression)),
            Ok(LambdaBody::Block(block)) => {
                self.lambda_probe_returns = Some(Vec::new());
                self.check_block(block);
                let returns = self.lambda_probe_returns.take().unwrap_or_default();
                if returns.is_empty() {
                    Some(Type::Void)
                } else {
                    let system = self.system();
                    best_common_type(&system, &returns)
                }
            }
            Err(()) => None,
        };

        self.locals.pop();
        self.return_type = saved_return;
        self.lambda_probe_returns = saved_probe;
        self.resolver.out.errors.truncate(error_mark);
        result
    }

    /// Checks a lambda against a concrete delegate signature, for real.
    fn check_lambda_against(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) {
        let names = Self::lambda_parameter_names(lambda);
        if names.len() != delegate.parameters.len() {
            self.error(
                SemanticErrorKind::LambdaParameterMismatch,
                lambda.parameters.span(),
            );
            return;
        }

        let mut scope = HashMap::new();
        for (name, parameter) in names.iter().zip(&delegate.parameters) {
            if let Some(name) = name {
                scope.insert(*name, parameter.parameter_type.clone());
            }
        }
        // explicitly written parameter types must agree with the delegate
        if let LambdaParameters::List(list) = &lambda.parameters {
            for (parameter, delegate_parameter) in list.parameters.iter().zip(&delegate.parameters)
            {
                if let Some(written) = &parameter.parameter_type {
                    let resolved = self.resolve_type(written);
                    if resolved != delegate_parameter.parameter_type
                        && !matches!(resolved, Type::Error)
                    {
                        let kind = SemanticErrorKind::TypeMismatch {
                            expected: self.display(&delegate_parameter.parameter_type),
                            found: self.display(&resolved),
                        };
                        self.error(kind, written.span.clone());
                    }
                }
            }
        }

        self.locals.push(scope);
        let saved_return = std::mem::replace(&mut self.return_type, delegate.return_type.clone());

        match &lambda.body {
            Ok(LambdaBody::Expression(expression)) => {
                if delegate.return_type == Type::Void {
                    self.check_expression(expression);
                } else {
                    let expected = delegate.return_type.clone();
                    let literal = Self::is_integer_literal(expression);
                    let ty = self.check_expression_expecting(expression, Some(&expected));
                    self.require_convertible(&ty, &expected, literal, expression.span());
                }
            }
            Ok(LambdaBody::Block(block)) => self.check_block(block),
            Err(()) => {}
        }

        self.return_type = saved_return;
        self.locals.pop();
    }

    /// A lambda in a non-argument position: against the context's expected type,
    /// or with its C# 10 natural `Func<>`/`Action<>` type.
    fn check_lambda(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Type {
        if let Some(expected) = expected {
            if let Some(delegate) = self.delegate_signature(expected) {
                self.check_lambda_against(lambda, &delegate);
                return expected.clone();
            }
            if !matches!(expected, Type::Error) {
                let kind = SemanticErrorKind::TypeMismatch {
                    expected: self.display(expected),
                    found: "lambda".to_string(),
                };
                self.error(kind, lambda.span.clone());
                return Type::Error;
            }
            return Type::Error;
        }

        // natural type: every parameter must carry a written type
        let Some(parameter_types) = self.lambda_written_parameter_types(lambda) else {
            self.error(SemanticErrorKind::TypeAnnotationNeeded, lambda.span.clone());
            return Type::Error;
        };
        let probe = FunctionSignature {
            return_type: Type::Void,
            parameters: parameter_types
                .iter()
                .map(|ty| crate::types::ParameterSignature {
                    passing: crate::types::ParameterPassing::Value,
                    is_params: false,
                    parameter_type: ty.clone(),
                    name: None,
                    default_value: None,
                })
                .collect(),
        };
        let Some(returned) = self.probe_lambda_return(lambda, &probe) else {
            return Type::Error;
        };
        let Some(delegate_type) = self.func_or_action(&parameter_types, &returned) else {
            self.error(SemanticErrorKind::TypeAnnotationNeeded, lambda.span.clone());
            return Type::Error;
        };

        self.check_lambda_against(
            lambda,
            &FunctionSignature {
                return_type: returned,
                parameters: probe.parameters,
            },
        );
        delegate_type
    }

    /// `Some` only when every parameter has an explicit type (zero parameters
    /// qualifies); `x => ...` has no natural type in C# either.
    fn lambda_written_parameter_types(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
    ) -> Option<Vec<Type>> {
        match &lambda.parameters {
            LambdaParameters::Single(_) => None,
            LambdaParameters::List(list) => {
                let mut types = Vec::with_capacity(list.parameters.len());
                for parameter in list.parameters {
                    let written = parameter.parameter_type.as_ref()?;
                    types.push(self.resolve_type(written));
                }
                Some(types)
            }
        }
    }

    /// `System.Func<..., R>` / `System.Action<...>` for a signature.
    fn func_or_action(&self, parameters: &[Type], returned: &Type) -> Option<Type> {
        let (name, arity, arguments) = if *returned == Type::Void {
            ("Action", parameters.len() as u32, parameters.to_vec())
        } else {
            let mut arguments = parameters.to_vec();
            arguments.push(returned.clone());
            ("Func", parameters.len() as u32 + 1, arguments)
        };

        let id = self.resolver.external.find_type(&["System"], name, arity)?;
        Some(Type::Named {
            target: TypeTarget::External(id),
            arguments,
        })
    }

    // ------------------------------------------------------------- new / []

    fn check_new(
        &mut self,
        new_expression: &'ast men_sharp_parser::ast::NewExpression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Meaning<'ast> {
        // `new[] { ... }`: the element type is the best common type of the elements
        if new_expression.created_type.is_none() && !new_expression.array_suffixes.is_empty() {
            use men_sharp_parser::ast::{CollectionElement, Initializer};
            let mut element_types = Vec::new();
            if let Some(Initializer::Collection { elements, .. }) = &new_expression.initializer {
                for element in *elements {
                    if let CollectionElement::Expression(expression) = element {
                        element_types.push(self.check_expression(expression));
                    }
                }
            }
            let system = self.system();
            let element = match best_common_type(&system, &element_types) {
                Some(element) => element,
                None => {
                    self.error(
                        SemanticErrorKind::TypeAnnotationNeeded,
                        new_expression.span.clone(),
                    );
                    Type::Error
                }
            };
            let ty = Type::Array {
                element: Box::new(element),
                rank: 1,
            };
            // `new[]` writes no type; the code generator reads it from here
            self.expression_types
                .insert(EntityID::from(new_expression), ty.clone());
            return Meaning::Value(ty);
        }

        // array creation
        if new_expression.is_array_creation() {
            let element = match &new_expression.created_type {
                Some(created_type) => self.resolve_type(created_type),
                None => Type::Error,
            };
            for size in new_expression.array_sizes {
                self.check_expression(size);
            }
            let ty = if new_expression.array_sizes.is_empty() {
                // `new int[] { ... }`: the written type already carries its ranks
                element
            } else {
                let inner = apply_suffixes(element, new_expression.array_suffixes);
                Type::Array {
                    element: Box::new(inner),
                    rank: new_expression.array_sizes.len() as u32,
                }
            };
            self.check_initializer(&new_expression.initializer, &ty);
            return Meaning::Value(ty);
        }

        let ty = match (&new_expression.created_type, expected) {
            (Some(created_type), _) => self.resolve_type(created_type),
            // target-typed `new(...)` takes the context's type
            (None, Some(expected)) => expected.clone(),
            (None, None) => {
                self.error(
                    SemanticErrorKind::TypeAnnotationNeeded,
                    new_expression.span.clone(),
                );
                return Meaning::Error;
            }
        };
        if matches!(ty, Type::Error) {
            return Meaning::Error;
        }

        // constructor overloads declared on the type itself
        if let Type::Named {
            target: TypeTarget::Source(class),
            ..
        } = &ty
            && self.is_abstract_type(*class)
        {
            let kind = SemanticErrorKind::CannotInstantiateAbstractType {
                type_name: self.display(&ty),
            };
            self.error(kind, new_expression.span.clone());
        }

        let constructors: Vec<MemberCandidate> = self
            .system()
            .members_named(&ty, ".ctor")
            .into_iter()
            .filter(|candidate| {
                candidate.kind == SymbolKind::Constructor
                    && !candidate.is_static
                    && candidate.declaring_type == ty
            })
            .collect();

        let arguments = new_expression
            .arguments
            .as_ref()
            .map(|list| self.check_arguments(list.arguments))
            .unwrap_or_default();

        // a struct always has a parameterless constructor (§16.4.9), whether
        // or not one is declared next to the others
        let implicit_struct_default = arguments.is_empty()
            && self.is_struct(&ty)
            && !constructors.iter().any(|candidate| {
                matches!(
                    &candidate.signature,
                    Some(MemberSignature::Function(function)) if function.parameters.is_empty()
                )
            });
        if constructors.is_empty() || implicit_struct_default {
            // the implicit parameterless constructor
            if !arguments.is_empty() {
                self.error(
                    SemanticErrorKind::NoMatchingOverload,
                    new_expression.span.clone(),
                );
            }
        } else {
            let receiver_display = self.display(&ty);
            let group = MethodGroup {
                candidates: constructors,
                explicit_arguments: Vec::new(),
                via_type: false,
                name: ".ctor",
                receiver: None,
                allow_extensions: false,
                receiver_display,
                span: new_expression.span.clone(),
            };
            self.resolve_call(
                group,
                arguments,
                &new_expression.span,
                Some(EntityID::from(new_expression)),
            );
        }

        self.check_initializer(&new_expression.initializer, &ty);
        Meaning::Value(ty)
    }

    fn check_initializer(
        &mut self,
        initializer: &'ast Option<men_sharp_parser::ast::Initializer<'ast, 'ast>>,
        ty: &Type,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer, InitializerTarget};
        match initializer {
            Some(Initializer::Object { elements, .. }) => {
                for element in *elements {
                    // `new D { [k] = v }` is `d[k] = v`: the indexer binds like
                    // any element access, recorded on the element
                    if let InitializerTarget::Index { arguments, span } = &element.target {
                        let call_arguments: Vec<CallArgument<'ast>> = arguments
                            .iter()
                            .map(|argument| self.plain_argument(argument))
                            .collect();
                        let element_type = match self.index_with(
                            ty.clone(),
                            call_arguments,
                            span,
                            Some(EntityID::from(element)),
                        ) {
                            Meaning::Value(element_type) => element_type,
                            _ => Type::Error,
                        };
                        if let Ok(InitializerValue::Expression(value)) = &element.value {
                            let literal = Self::is_integer_literal(value);
                            let value_type =
                                self.check_expression_expecting(value, Some(&element_type));
                            self.require_convertible(
                                &value_type,
                                &element_type,
                                literal,
                                value.span(),
                            );
                        }
                        continue;
                    }
                    if let InitializerTarget::Member(name) = &element.target {
                        // `new T { X = v }` is `t.X = v`: the member binds
                        // like any other write, recorded on the element for
                        // the code generator
                        let member = self
                            .system()
                            .members_named(ty, name.value)
                            .into_iter()
                            .find_map(|candidate| match &candidate.signature {
                                Some(MemberSignature::Field(member_type))
                                | Some(MemberSignature::Property(member_type))
                                    if !candidate.is_static =>
                                {
                                    Some((member_type.clone(), candidate))
                                }
                                _ => None,
                            });
                        let Some((member_type, candidate)) = member else {
                            let kind = SemanticErrorKind::UnknownMember {
                                type_name: self.display(ty),
                            };
                            self.error(kind, name.span.clone());
                            continue;
                        };
                        self.targets.insert(
                            EntityID::from(element),
                            ResolvedTarget::Member(ResolvedMember {
                                origin: candidate.origin,
                                kind: candidate.kind,
                                is_static: candidate.is_static,
                                declaring_type: candidate.declaring_type,
                                member_type: member_type.clone(),
                            }),
                        );
                        if let Ok(InitializerValue::Expression(value)) = &element.value {
                            let literal = Self::is_integer_literal(value);
                            let value_type =
                                self.check_expression_expecting(value, Some(&member_type));
                            self.require_convertible(
                                &value_type,
                                &member_type,
                                literal,
                                value.span(),
                            );
                        }
                    }
                }
            }
            Some(Initializer::Collection { elements, .. }) => {
                if let Type::Array { element, .. } = ty {
                    for item in *elements {
                        if let CollectionElement::Expression(expression) = item {
                            let value_type = self.check_expression(expression);
                            let literal = Self::is_integer_literal(expression);
                            self.require_convertible(
                                &value_type,
                                element,
                                literal,
                                expression.span(),
                            );
                        }
                    }
                    return;
                }
                if matches!(ty, Type::Error | Type::Dynamic) {
                    return;
                }
                // `new C { a, b }` is `Add(a); Add(b);` (§12.8.17.4): each
                // element is one overload resolution against the type's `Add`,
                // recorded on the element node for the code generator
                for item in *elements {
                    self.check_collection_element(item, ty);
                }
            }
            None => {}
        }
    }

    fn check_collection_element(
        &mut self,
        item: &'ast men_sharp_parser::ast::CollectionElement<'ast, 'ast>,
        collection: &Type,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};
        let (arguments, span): (Vec<CallArgument<'ast>>, Range<usize>) = match item {
            CollectionElement::Expression(expression) => {
                (vec![self.plain_argument(expression)], expression.span())
            }
            // `{ k, v }`: one `Add` with several arguments
            CollectionElement::Nested(Initializer::Collection { elements, span }) => {
                let mut arguments = Vec::with_capacity(elements.len());
                for inner in *elements {
                    match inner {
                        CollectionElement::Expression(expression) => {
                            arguments.push(self.plain_argument(expression));
                        }
                        CollectionElement::Nested(initializer) => {
                            self.error(
                                SemanticErrorKind::UnsupportedExpression,
                                initializer.span(),
                            );
                            return;
                        }
                    }
                }
                (arguments, span.clone())
            }
            CollectionElement::Nested(initializer) => {
                self.error(SemanticErrorKind::UnsupportedExpression, initializer.span());
                return;
            }
        };
        let candidates: Vec<MemberCandidate> = self
            .system()
            .members_named(collection, "Add")
            .into_iter()
            .filter(|candidate| candidate.kind == SymbolKind::Method && !candidate.is_static)
            .collect();
        if candidates.is_empty() {
            let kind = SemanticErrorKind::UnknownMember {
                type_name: self.display(collection),
            };
            self.error(kind, span);
            return;
        }
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: false,
            name: "Add",
            receiver: Some(collection.clone()),
            allow_extensions: false,
            receiver_display: self.display(collection),
            span: span.clone(),
        };
        self.resolve_call(group, arguments, &span, Some(EntityID::from(item)));
    }

    /// A by-value argument built from a bare expression (no `Argument` node:
    /// collection-initializer elements).
    fn plain_argument(&mut self, expression: &'ast Expression<'ast, 'ast>) -> CallArgument<'ast> {
        CallArgument {
            shape: ArgumentShape::Value(self.check_expression(expression)),
            name: None,
            expression: Some(expression),
            modifier: None,
            is_integer_literal: Self::is_integer_literal(expression),
            out_declaration: None,
            span: expression.span(),
        }
    }

    fn index(
        &mut self,
        receiver: Type,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        let call_arguments = self.check_arguments(arguments);
        self.index_with(receiver, call_arguments, span, node)
    }

    fn index_with(
        &mut self,
        receiver: Type,
        call_arguments: Vec<CallArgument<'ast>>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        match &receiver {
            Type::Array { element, .. } => {
                let int32 = self.corlib("Int32");
                for argument in &call_arguments {
                    self.require_convertible(
                        &argument.value_type(),
                        &int32,
                        argument.is_integer_literal,
                        argument.span.clone(),
                    );
                }
                Meaning::Value((**element).clone())
            }
            Type::Error => Meaning::Error,
            _ if self.system().is_string(&receiver) => Meaning::Value(self.corlib("Char")),
            _ => {
                // indexers: `this[]` from source, `Item` from metadata
                let mut candidates = self.system().members_named(&receiver, "this[]");
                candidates.extend(self.system().members_named(&receiver, "Item"));
                let indexers: Vec<MemberCandidate> = candidates
                    .into_iter()
                    .filter(|candidate| {
                        matches!(candidate.signature, Some(MemberSignature::Function(_)))
                    })
                    .collect();

                if indexers.is_empty() {
                    let kind = SemanticErrorKind::NotIndexable {
                        type_name: self.display(&receiver),
                    };
                    self.error(kind, span.clone());
                    return Meaning::Error;
                }

                let receiver_display = self.display(&receiver);
                let group = MethodGroup {
                    candidates: indexers,
                    explicit_arguments: Vec::new(),
                    via_type: false,
                    name: "this[]",
                    receiver: None,
                    allow_extensions: false,
                    receiver_display,
                    span: span.clone(),
                };
                Meaning::Value(self.resolve_call(group, call_arguments, span, node))
            }
        }
    }

    // ------------------------------------------------------------ operators

    fn unary_type(
        &mut self,
        operator: UnaryOperator,
        operand: Type,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        if matches!(operand, Type::Error | Type::Dynamic) {
            return operand;
        }

        let system = self.system();
        match operator {
            UnaryOperator::Not => {
                if system.is_bool(&operand) {
                    self.corlib("Boolean")
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.display(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::Plus | UnaryOperator::Minus => {
                if let Some(kind) = system.numeric_kind(&operand) {
                    // small operands promote to int
                    let promoted = match kind {
                        NumericKind::SByte
                        | NumericKind::Byte
                        | NumericKind::Int16
                        | NumericKind::UInt16
                        | NumericKind::Char => NumericKind::Int32,
                        other => other,
                    };
                    self.corlib(promoted.corlib_name())
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.display(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::BitwiseNot => {
                if system
                    .numeric_kind(&operand)
                    .map(|kind| kind.is_integral())
                    .unwrap_or(false)
                    || system.is_enum_type(&operand)
                {
                    operand
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.display(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::PreIncrement | UnaryOperator::PreDecrement => {
                if system.numeric_kind(&operand).is_some() || system.is_enum_type(&operand) {
                    operand
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.display(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::IndexFromEnd | UnaryOperator::AddressOf | UnaryOperator::Dereference => {
                self.error(SemanticErrorKind::UnsupportedExpression, span);
                Type::Error
            }
        }
    }

    fn binary_type(
        &mut self,
        operator: BinaryOperator,
        left: Type,
        right: Type,
        literal: bool,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        if matches!(left, Type::Error) || matches!(right, Type::Error) {
            return Type::Error;
        }
        if matches!(left, Type::Dynamic) || matches!(right, Type::Dynamic) {
            return Type::Dynamic;
        }

        let system = self.system();
        use BinaryOperator::*;

        match operator {
            LogicalAnd | LogicalOr => {
                if system.is_bool(&left) && system.is_bool(&right) {
                    return self.corlib("Boolean");
                }
            }
            Coalesce => {
                return match left {
                    Type::Null => right,
                    Type::Nullable(inner) => *inner,
                    other => other,
                };
            }
            Equal | NotEqual => {
                let comparable = matches!(left, Type::Null)
                    || matches!(right, Type::Null)
                    || (system.numeric_kind(&left).is_some()
                        && system.numeric_kind(&right).is_some())
                    || system.is_bool(&left) && system.is_bool(&right)
                    || left == right
                    || system.is_implicitly_convertible(&left, &right)
                    || system.is_implicitly_convertible(&right, &left);
                if comparable && !self.has_user_operator(operator, &left, &right) {
                    return self.corlib("Boolean");
                }
                if let Some(result) = self.user_defined_binary(operator, &left, &right, &span, node)
                {
                    return result;
                }
                if comparable {
                    return self.corlib("Boolean");
                }
            }
            Add if system.is_string(&left) || system.is_string(&right) => {
                // string concatenation accepts anything on the other side
                return self.corlib("String");
            }
            _ => {}
        }

        // numeric operands: the binary promotion rules
        if let (Some(left_kind), Some(right_kind)) =
            (system.numeric_kind(&left), system.numeric_kind(&right))
        {
            return self.numeric_binary(operator, left_kind, right_kind, span);
        }

        // enums
        if system.is_enum_type(&left) || system.is_enum_type(&right) {
            match operator {
                LessThan | GreaterThan | LessThanEqual | GreaterThanEqual => {
                    if left == right {
                        return self.corlib("Boolean");
                    }
                }
                BitwiseAnd | BitwiseOr | BitwiseXor if left == right => return left,
                Add | Subtract => {
                    if system.is_enum_type(&left) && system.numeric_kind(&right).is_some() {
                        return left;
                    }
                    if system.numeric_kind(&left).is_some() && system.is_enum_type(&right) {
                        return right;
                    }
                    if operator == Subtract && left == right {
                        return self.corlib("Int32");
                    }
                }
                _ => {}
            }
        }

        // user-defined operators (Vector3 + Vector3, a struct's own `+`, ...)
        if let Some(result) = self.user_defined_binary(operator, &left, &right, &span, node) {
            return result;
        }

        let _ = literal;
        let kind = SemanticErrorKind::InvalidOperator {
            left: self.display(&left),
            right: Some(self.display(&right)),
        };
        self.error(kind, span);
        Type::Error
    }

    fn numeric_binary(
        &mut self,
        operator: BinaryOperator,
        left: NumericKind,
        right: NumericKind,
        span: Range<usize>,
    ) -> Type {
        use BinaryOperator::*;
        use NumericKind::*;

        let promoted = if left == Decimal || right == Decimal {
            Decimal
        } else if left == Double || right == Double {
            Double
        } else if left == Single || right == Single {
            Single
        } else if left == UInt64 || right == UInt64 {
            UInt64
        } else if left == Int64 || right == Int64 {
            Int64
        } else if left == UInt32 || right == UInt32 {
            // uint with a signed operand goes to long
            let signed = |kind: NumericKind| matches!(kind, SByte | Int16 | Int32);
            if signed(left) || signed(right) {
                Int64
            } else {
                UInt32
            }
        } else {
            Int32
        };

        match operator {
            LessThan | GreaterThan | LessThanEqual | GreaterThanEqual | Equal | NotEqual => {
                self.corlib("Boolean")
            }
            LeftShift | RightShift | UnsignedRightShift => {
                // the left operand alone decides, promoted to at least int
                let shifted = match left {
                    SByte | Byte | Int16 | UInt16 | Char => Int32,
                    other => other,
                };
                self.corlib(shifted.corlib_name())
            }
            BitwiseAnd | BitwiseOr | BitwiseXor
                if matches!(promoted, Single | Double | Decimal) =>
            {
                let kind = SemanticErrorKind::InvalidOperator {
                    left: promoted.corlib_name().to_string(),
                    right: Some(promoted.corlib_name().to_string()),
                };
                self.error(kind, span);
                Type::Error
            }
            _ => self.corlib(promoted.corlib_name()),
        }
    }

    fn binary_operator_name(operator: BinaryOperator) -> Option<&'static str> {
        use BinaryOperator::*;
        Some(match operator {
            Add => "op_Addition",
            Subtract => "op_Subtraction",
            Multiply => "op_Multiply",
            Divide => "op_Division",
            Modulo => "op_Modulus",
            BitwiseAnd => "op_BitwiseAnd",
            BitwiseOr => "op_BitwiseOr",
            BitwiseXor => "op_ExclusiveOr",
            LeftShift => "op_LeftShift",
            RightShift => "op_RightShift",
            UnsignedRightShift => "op_UnsignedRightShift",
            Equal => "op_Equality",
            NotEqual => "op_Inequality",
            LessThan => "op_LessThan",
            GreaterThan => "op_GreaterThan",
            LessThanEqual => "op_LessThanOrEqual",
            GreaterThanEqual => "op_GreaterThanOrEqual",
            LogicalAnd | LogicalOr | Coalesce => return None,
        })
    }

    /// Whether either operand's type declares this operator at all — when
    /// it does, `==` on two references is the declared one, not identity.
    fn has_user_operator(&self, operator: BinaryOperator, left: &Type, right: &Type) -> bool {
        let Some(name) = Self::binary_operator_name(operator) else {
            return false;
        };
        !self
            .operator_candidates(name, &[left.clone(), right.clone()], 2)
            .is_empty()
    }

    /// The declared operators named `name` on the operand types (and their
    /// bases), with this many parameters.
    fn operator_candidates(
        &self,
        name: &str,
        operands: &[Type],
        parameter_count: usize,
    ) -> Vec<MemberCandidate> {
        let system = self.system();
        let mut out: Vec<MemberCandidate> = Vec::new();
        let mut seen: Vec<Type> = Vec::new();
        for operand in operands {
            let ty = match operand {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            if seen.contains(&ty) {
                continue;
            }
            seen.push(ty.clone());
            for candidate in system.members_named(&ty, name) {
                let takes = matches!(
                    &candidate.signature,
                    Some(MemberSignature::Function(function))
                        if function.parameters.len() == parameter_count
                );
                let duplicate = out.iter().any(|existing| {
                    matches!(
                        (&existing.origin, &candidate.origin),
                        (MemberOrigin::Source(a), MemberOrigin::Source(b)) if a == b
                    ) || (existing.declaring_type == candidate.declaring_type
                        && existing.signature == candidate.signature
                        && !matches!(candidate.origin, MemberOrigin::Source(_)))
                });
                // metadata spells an operator as a static `op_*` method
                if matches!(candidate.kind, SymbolKind::Operator | SymbolKind::Method)
                    && candidate.is_static
                    && takes
                    && !duplicate
                {
                    out.push(candidate);
                }
            }
        }
        out
    }

    /// Overload resolution among the declared operators named `name` for
    /// these operand types (§12.4.5); the chosen one is recorded on `node`
    /// for the code generator. `None` when none applies.
    fn resolve_user_operator(
        &mut self,
        name: &'static str,
        operands: &[Type],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let candidates = self.operator_candidates(name, operands, operands.len());
        if candidates.is_empty() {
            return None;
        }
        let arguments: Vec<CallArgument<'ast>> = operands
            .iter()
            .map(|ty| CallArgument {
                shape: ArgumentShape::Value(ty.clone()),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                span: span.clone(),
            })
            .collect();
        let receiver_display = self.display(&operands[0]);
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: true,
            name,
            receiver: None,
            allow_extensions: false,
            receiver_display,
            span: span.clone(),
        };
        match self.attempt_call(&group, &arguments) {
            AttemptOutcome::Selected(selected) => {
                self.record_call(node, &group, &selected, false);
                Some(selected.signature.return_type.clone())
            }
            AttemptOutcome::Ambiguous => {
                self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                Some(Type::Error)
            }
            AttemptOutcome::NoMatch { .. } => None,
        }
    }

    fn user_defined_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Type,
        right: &Type,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let name = Self::binary_operator_name(operator)?;
        self.resolve_user_operator(name, &[left.clone(), right.clone()], span, node)
    }

    fn user_defined_unary(
        &mut self,
        operator: UnaryOperator,
        operand: &Type,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let name = match operator {
            UnaryOperator::Plus => "op_UnaryPlus",
            UnaryOperator::Minus => "op_UnaryNegation",
            UnaryOperator::Not => "op_LogicalNot",
            UnaryOperator::BitwiseNot => "op_OnesComplement",
            UnaryOperator::PreIncrement => "op_Increment",
            UnaryOperator::PreDecrement => "op_Decrement",
            _ => return None,
        };
        self.resolve_user_operator(name, std::slice::from_ref(operand), span, node)
    }

    /// `x++` / `x--` on a type of the user's: its `op_Increment` /
    /// `op_Decrement`, recorded on the postfix node. Numeric and enum
    /// operands keep the built-in meaning.
    fn postfix_step_type(
        &mut self,
        increment: bool,
        operand: Type,
        span: &Range<usize>,
        node: EntityID,
    ) -> Type {
        let system = self.system();
        if matches!(operand, Type::Error | Type::Dynamic)
            || system.numeric_kind(&operand).is_some()
            || system.is_enum_type(&operand)
        {
            return operand;
        }
        let name = if increment {
            "op_Increment"
        } else {
            "op_Decrement"
        };
        match self.resolve_user_operator(name, std::slice::from_ref(&operand), span, Some(node)) {
            Some(result) => result,
            None => {
                let kind = SemanticErrorKind::InvalidOperator {
                    left: self.display(&operand),
                    right: None,
                };
                self.error(kind, span.clone());
                Type::Error
            }
        }
    }

    // -------------------------------------------------------------- foreach

    /// The element type a `foreach` over `collection` yields. Arrays and
    /// strings are walked by index; everything else goes through the
    /// enumerator pattern, which is resolved here and recorded under `node`
    /// for the code generator.
    fn element_type_of(&mut self, collection: &Type, span: Range<usize>, node: EntityID) -> Type {
        match collection {
            Type::Array { element, rank: 1 } => return (**element).clone(),
            Type::Error | Type::Dynamic => return Type::Error,
            _ if self.system().is_string(collection) => return self.corlib("Char"),
            _ => {}
        }

        // the pattern-based protocol: GetEnumerator(), then MoveNext()/Current
        if let Some(enumeration) = self.resolve_enumeration(collection, &span) {
            let element = enumeration.current.member_type.clone();
            self.enumerations.insert(node, enumeration);
            return element;
        }

        // IEnumerable<T> somewhere in the closure: typed, but with no members
        // for the code generator to call — it reports that itself
        if let Some(ienumerable) = self.resolver.external.find_type(
            &["System", "Collections", "Generic"],
            "IEnumerable",
            1,
        ) {
            let mut visited = std::collections::HashSet::new();
            let mut queue = vec![collection.clone()];
            while let Some(current) = queue.pop() {
                if !visited.insert(current.clone()) {
                    continue;
                }
                if let Type::Named {
                    target: TypeTarget::External(id),
                    arguments,
                } = &current
                    && *id == ienumerable
                    && arguments.len() == 1
                {
                    return arguments[0].clone();
                }
                queue.extend(self.system().interfaces_of(&current));
                if let Some(base) = self.system().base_of(&current) {
                    queue.push(base);
                }
            }
        }

        let kind = SemanticErrorKind::NotEnumerable {
            type_name: self.display(collection),
        };
        self.error(kind, span);
        Type::Error
    }

    /// Binds the enumerator pattern on `collection`: an accessible instance
    /// `GetEnumerator()` taking nothing, whose result has an instance
    /// `MoveNext()` returning `bool` and a readable `Current` property. Any
    /// piece missing means the pattern does not apply (`None`), not an error —
    /// the caller has the interface fallback and the diagnostic.
    fn resolve_enumeration(
        &mut self,
        collection: &Type,
        span: &Range<usize>,
    ) -> Option<ForeachEnumeration> {
        let (get_enumerator, enumerator_type) =
            self.resolve_parameterless_call(collection, "GetEnumerator", span)?;
        let (move_next, move_next_type) =
            self.resolve_parameterless_call(&enumerator_type, "MoveNext", span)?;
        if !self.system().is_system_type(&move_next_type, "Boolean") {
            return None;
        }
        let current = self
            .system()
            .members_named(&enumerator_type, "Current")
            .into_iter()
            .find(|candidate| candidate.kind == SymbolKind::Property && !candidate.is_static)?;
        let Some(MemberSignature::Property(member_type)) = current.signature.clone() else {
            return None;
        };
        Some(ForeachEnumeration {
            get_enumerator,
            enumerator_type,
            move_next,
            current: ResolvedMember {
                origin: current.origin,
                kind: current.kind,
                is_static: current.is_static,
                declaring_type: current.declaring_type,
                member_type,
            },
        })
    }

    /// Overload resolution for `receiver.name()` with no arguments and no
    /// syntax to hang an error on: the selected overload and its return type,
    /// or `None` when nothing fits.
    fn resolve_parameterless_call(
        &mut self,
        receiver: &Type,
        name: &'ast str,
        span: &Range<usize>,
    ) -> Option<(ResolvedCall, Type)> {
        let candidates: Vec<MemberCandidate> = self
            .system()
            .members_named(receiver, name)
            .into_iter()
            .filter(|candidate| candidate.kind == SymbolKind::Method && !candidate.is_static)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: false,
            name,
            receiver: Some(receiver.clone()),
            allow_extensions: false,
            receiver_display: self.display(receiver),
            span: span.clone(),
        };
        let AttemptOutcome::Selected(selected) = self.attempt_call(&group, &[]) else {
            return None;
        };
        let candidate = &group.candidates[selected.candidate];
        let call = ResolvedCall {
            origin: candidate.origin.clone(),
            is_static: candidate.is_static,
            is_extension: false,
            declaring_type: candidate.declaring_type.clone(),
            signature: selected.signature.clone(),
            type_arguments: selected.type_arguments.clone(),
            parameter_of_argument: selected.parameter_of_argument.clone(),
            params_expansion: selected.params_expansion,
        };
        let return_type = selected.signature.return_type.clone();
        Some((call, return_type))
    }
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
    }
}
