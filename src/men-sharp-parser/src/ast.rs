//! The MenSharp syntax tree.
//!
//! Every node lives in a [`bumpalo::Bump`] arena: sequences are `&'allocator [T]` slices
//! rather than `Vec`, and recursive positions are `&'allocator T`. Nothing in here owns a
//! heap allocation of its own, so dropping the arena frees the whole tree at once and
//! building it costs a bump-pointer increment per node.
//!
//! Two lifetimes run through the tree:
//! - `'input` borrows the source text, so identifiers and literals are `&str` slices of it
//!   rather than copies.
//! - `'allocator` borrows the arena.
//!
//! Recoverable holes are encoded in the types themselves: `Option<T>` is *syntactically
//! optional*, `Result<T, ()>` is *required but missing*, with the corresponding
//! [`crate::error::ParseError`] already recorded. The parser therefore always returns a
//! tree, never bails out, and a downstream pass can keep working around the holes.

use std::{any::TypeId, ops::Range};

/// Marker for every syntax node, so that [`EntityID`] can key later compiler phases
/// (name resolution, type inference, ...) on node identity without touching the node itself.
pub trait AST {}

/// A stable identity for a node: its type plus its address in the arena.
///
/// Sound only while the arena is alive, which is exactly as long as the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityID((TypeId, usize));

impl<T: AST + Sized> From<&T> for EntityID {
    fn from(value: &T) -> Self {
        Self((typeid::of::<T>(), value as *const T as usize))
    }
}

macro_rules! impl_ast {
    (plain: $($ty:ty),* $(,)?) => {
        $( impl AST for $ty {} )*
    };
    (input: $($ty:ident),* $(,)?) => {
        $( impl<'input> AST for $ty<'input> {} )*
    };
    (arena: $($ty:ident),* $(,)?) => {
        $( impl<'input, 'allocator> AST for $ty<'input, 'allocator> {} )*
    };
}

impl<T> AST for Spanned<T> {}

impl_ast!(plain:
    Modifier,
    ParameterModifier,
    ArgumentModifier,
    AttributeTarget,
    ClassKind,
    AccessorKind,
    Variance,
    PredefinedType,
    MemberSeparator,
    BinaryOperator,
    AssignmentOperator,
    UnaryOperator,
    PostfixOperator,
    OperatorSymbol,
    ConversionKind,
    ConstructorInitializerKind,
    RelationalOperator,
    OrderDirection,
    CheckedKind,
    YieldKind,
    TypeSuffix,
    BreakStatement,
    ContinueStatement,
);

impl_ast!(input:
    ExternAliasDirective,
);

impl_ast!(arena:
    CompilationUnit,
    NamespaceMember,
    UsingDirective,
    UsingResource,
    ForInitializer,
    GotoTarget,
    NamespaceDeclaration,
    Documents,
    AttributeSection,
    Attribute,
    TypeDeclaration,
    ClassDeclaration,
    EnumDeclaration,
    EnumMember,
    DelegateDeclaration,
    BaseTypeList,
    TypeMember,
    FieldDeclaration,
    VariableDeclarator,
    MethodDeclaration,
    PropertyDeclaration,
    IndexerDeclaration,
    EventDeclaration,
    ConstructorDeclaration,
    ConstructorInitializer,
    DestructorDeclaration,
    OperatorDeclaration,
    AccessorList,
    Accessor,
    FunctionBody,
    ParameterList,
    Parameter,
    GenericsDefine,
    GenericsParameter,
    GenericsInfo,
    TypeParameterConstraint,
    ConstraintBound,
    TypeRef,
    TypeRefBase,
    NameType,
    NameSegment,
    TupleType,
    TupleTypeElement,
    Block,
    Statement,
    LocalVariableDeclaration,
    ExpressionStatement,
    IfStatement,
    WhileStatement,
    DoWhileStatement,
    ForStatement,
    ForeachStatement,
    SwitchStatement,
    SwitchSection,
    SwitchLabel,
    TryStatement,
    CatchClause,
    CatchFilter,
    FinallyClause,
    UsingStatement,
    LockStatement,
    CheckedStatement,
    UnsafeStatement,
    FixedStatement,
    StackallocExpression,
    ReturnStatement,
    ThrowStatement,
    YieldStatement,
    GotoStatement,
    LabeledStatement,
    Expression,
    AssignmentExpression,
    ConditionalExpression,
    BinaryExpression,
    IsExpression,
    AsExpression,
    UnaryExpression,
    CastExpression,
    AwaitExpression,
    ThrowExpression,
    RefExpression,
    DeclarationExpression,
    CollectionExpressionElement,
    RangeExpression,
    LambdaExpression,
    AnonymousMethodExpression,
    LambdaParameters,
    LambdaBody,
    SwitchExpression,
    SwitchExpressionArm,
    WithExpression,
    QueryExpression,
    QueryBody,
    QueryClause,
    FromClause,
    LetClause,
    WhereQueryClause,
    JoinClause,
    OrderByClause,
    Ordering,
    SelectOrGroupClause,
    QueryContinuation,
    LiteralExpression,
    InterpolatedString,
    InterpolationPart,
    InterpolationHole,
    PrimaryExpression,
    PrimaryLeft,
    PrimaryRight,
    NewExpression,
    ArgumentList,
    Argument,
    ArgumentValue,
    Initializer,
    ObjectInitializerElement,
    InitializerTarget,
    InitializerValue,
    CollectionElement,
    TupleElement,
    Pattern,
    PropertySubpattern,
    PositionalSubpattern,
    VariableDesignation,
);

// ============================================================================
// spans
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Range<usize>) -> Self {
        Self { value, span }
    }

    pub fn map<V, F: FnOnce(T) -> V>(self, f: F) -> Spanned<V> {
        Spanned::new(f(self.value), self.span)
    }
}

pub trait WithSpan: Sized {
    fn with_span(self, span: Range<usize>) -> Spanned<Self>;
}

impl<T: Sized> WithSpan for T {
    fn with_span(self, span: Range<usize>) -> Spanned<Self> {
        Spanned::new(self, span)
    }
}

/// An identifier, borrowed from the source.
pub type Ident<'input> = Spanned<&'input str>;

/// The raw text of a literal token, borrowed from the source. Escape sequences and
/// interpolation holes are left untouched; decoding them is a later pass.
pub type LiteralText<'input> = Spanned<&'input str>;

/// Joins two spans into the range that covers both.
pub fn merge_span(left: &Range<usize>, right: &Range<usize>) -> Range<usize> {
    left.start.min(right.start)..left.end.max(right.end)
}

// ============================================================================
// compilation unit
// ============================================================================

#[derive(Debug)]
pub struct CompilationUnit<'input, 'allocator> {
    pub extern_aliases: &'allocator [ExternAliasDirective<'input>],
    pub usings: &'allocator [UsingDirective<'input, 'allocator>],
    /// Attributes with an `assembly:` or `module:` target.
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub members: &'allocator [NamespaceMember<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum NamespaceMember<'input, 'allocator> {
    Namespace(NamespaceDeclaration<'input, 'allocator>),
    /// Arena boxed: a type declaration dwarfs a namespace one, and this enum is stored
    /// in a slice, so inlining it would pad every namespace entry to the larger size.
    Type(&'allocator TypeDeclaration<'input, 'allocator>),
}

impl NamespaceMember<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            NamespaceMember::Namespace(namespace) => namespace.span.clone(),
            NamespaceMember::Type(type_declaration) => type_declaration.span(),
        }
    }
}

/// `extern alias Foo;`
#[derive(Debug)]
pub struct ExternAliasDirective<'input> {
    pub extern_keyword: Range<usize>,
    pub alias_keyword: Option<Range<usize>>,
    pub name: Result<Ident<'input>, ()>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct UsingDirective<'input, 'allocator> {
    pub global: Option<Range<usize>>,
    pub using_keyword: Range<usize>,
    pub static_keyword: Option<Range<usize>>,
    /// `using Alias = Target;`
    pub alias: Option<Ident<'input>>,
    pub target: Result<TypeRef<'input, 'allocator>, ()>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct NamespaceDeclaration<'input, 'allocator> {
    pub namespace_keyword: Range<usize>,
    pub extern_aliases: &'allocator [ExternAliasDirective<'input>],
    /// `namespace A.B.C` -- one identifier per dotted segment.
    pub name: Result<&'allocator [Ident<'input>], ()>,
    pub is_file_scoped: bool,
    pub usings: &'allocator [UsingDirective<'input, 'allocator>],
    pub members: &'allocator [NamespaceMember<'input, 'allocator>],
    pub span: Range<usize>,
}

/// The `///` and `/** */` comments attached to a declaration, in source order,
/// with their comment markers still attached.
#[derive(Debug)]
pub struct Documents<'input, 'allocator> {
    pub documents: &'allocator [LiteralText<'input>],
    pub span: Range<usize>,
}

// ============================================================================
// attributes and modifiers
// ============================================================================

#[derive(Debug)]
pub struct AttributeSection<'input, 'allocator> {
    pub target: Option<Spanned<AttributeTarget>>,
    pub attributes: &'allocator [Attribute<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Attribute<'input, 'allocator> {
    pub name: TypeRef<'input, 'allocator>,
    pub arguments: Option<ArgumentList<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttributeTarget {
    Assembly,
    Module,
    Field,
    Event,
    Method,
    Param,
    Property,
    Return,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Public,
    Private,
    Protected,
    Internal,
    File,
    Static,
    Readonly,
    Const,
    Volatile,
    Extern,
    Unsafe,
    Abstract,
    Virtual,
    Override,
    Sealed,
    New,
    Async,
    Partial,
    Required,
    Ref,
    Fixed,
}

// ============================================================================
// type declarations
// ============================================================================

#[derive(Debug)]
pub enum TypeDeclaration<'input, 'allocator> {
    Class(ClassDeclaration<'input, 'allocator>),
    Enum(EnumDeclaration<'input, 'allocator>),
    Delegate(DelegateDeclaration<'input, 'allocator>),
}

impl TypeDeclaration<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            TypeDeclaration::Class(declaration) => declaration.span.clone(),
            TypeDeclaration::Enum(declaration) => declaration.span.clone(),
            TypeDeclaration::Delegate(declaration) => declaration.span.clone(),
        }
    }
}

/// `class`, `struct`, `interface` and `record` share one shape.
#[derive(Debug)]
pub struct ClassDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub kind: Spanned<ClassKind>,
    pub name: Result<Ident<'input>, ()>,
    pub generics: Option<GenericsDefine<'input, 'allocator>>,
    /// A record's primary constructor.
    pub primary_constructor: Option<ParameterList<'input, 'allocator>>,
    pub base_types: Option<BaseTypeList<'input, 'allocator>>,
    pub constraints: &'allocator [TypeParameterConstraint<'input, 'allocator>],
    pub members: Result<&'allocator [TypeMember<'input, 'allocator>], ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassKind {
    Class,
    Struct,
    Interface,
    Record,
    RecordStruct,
}

#[derive(Debug)]
pub struct BaseTypeList<'input, 'allocator> {
    pub colon: Range<usize>,
    pub types: &'allocator [TypeRef<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct EnumDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub enum_keyword: Range<usize>,
    pub name: Result<Ident<'input>, ()>,
    pub underlying_type: Option<BaseTypeList<'input, 'allocator>>,
    pub members: Result<&'allocator [EnumMember<'input, 'allocator>], ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct EnumMember<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub name: Ident<'input>,
    pub value: Option<Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct DelegateDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub delegate_keyword: Range<usize>,
    pub return_type: Result<TypeRef<'input, 'allocator>, ()>,
    pub name: Result<Ident<'input>, ()>,
    pub generics: Option<GenericsDefine<'input, 'allocator>>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub constraints: &'allocator [TypeParameterConstraint<'input, 'allocator>],
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

// ============================================================================
// type members
// ============================================================================

#[derive(Debug)]
pub enum TypeMember<'input, 'allocator> {
    Field(FieldDeclaration<'input, 'allocator>),
    Method(MethodDeclaration<'input, 'allocator>),
    Property(PropertyDeclaration<'input, 'allocator>),
    Indexer(IndexerDeclaration<'input, 'allocator>),
    Event(EventDeclaration<'input, 'allocator>),
    Constructor(ConstructorDeclaration<'input, 'allocator>),
    Destructor(DestructorDeclaration<'input, 'allocator>),
    Operator(OperatorDeclaration<'input, 'allocator>),
    NestedType(TypeDeclaration<'input, 'allocator>),
}

impl TypeMember<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            TypeMember::Field(member) => member.span.clone(),
            TypeMember::Method(member) => member.span.clone(),
            TypeMember::Property(member) => member.span.clone(),
            TypeMember::Indexer(member) => member.span.clone(),
            TypeMember::Event(member) => member.span.clone(),
            TypeMember::Constructor(member) => member.span.clone(),
            TypeMember::Destructor(member) => member.span.clone(),
            TypeMember::Operator(member) => member.span.clone(),
            TypeMember::NestedType(member) => member.span(),
        }
    }
}

#[derive(Debug)]
pub struct FieldDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub field_type: TypeRef<'input, 'allocator>,
    pub declarators: &'allocator [VariableDeclarator<'input, 'allocator>],
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct VariableDeclarator<'input, 'allocator> {
    pub name: Ident<'input>,
    pub initializer: Option<InitializerValue<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct MethodDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub return_type: TypeRef<'input, 'allocator>,
    /// `void IFoo.Bar()` -- the interface part of an explicit implementation.
    pub explicit_interface: Option<NameType<'input, 'allocator>>,
    pub name: Ident<'input>,
    pub generics: Option<GenericsDefine<'input, 'allocator>>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub constraints: &'allocator [TypeParameterConstraint<'input, 'allocator>],
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct PropertyDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub property_type: TypeRef<'input, 'allocator>,
    pub explicit_interface: Option<NameType<'input, 'allocator>>,
    pub name: Ident<'input>,
    pub body: FunctionBody<'input, 'allocator>,
    /// `public int X { get; set; } = 1;`
    pub initializer: Option<InitializerValue<'input, 'allocator>>,
    /// Synthesized from a record's positional parameter (see the parser's
    /// `record` module). C# does not declare one when a base type already
    /// has a member of that name, which only name binding can tell.
    pub positional: bool,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct IndexerDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub element_type: TypeRef<'input, 'allocator>,
    pub explicit_interface: Option<NameType<'input, 'allocator>>,
    pub this_keyword: Range<usize>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct EventDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub event_keyword: Range<usize>,
    pub event_type: TypeRef<'input, 'allocator>,
    pub explicit_interface: Option<NameType<'input, 'allocator>>,
    /// Field-like events may declare several names at once; an event with
    /// `add`/`remove` accessors always has exactly one.
    pub declarators: &'allocator [VariableDeclarator<'input, 'allocator>],
    pub accessors: Option<AccessorList<'input, 'allocator>>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ConstructorDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub name: Ident<'input>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub initializer: Option<ConstructorInitializer<'input, 'allocator>>,
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ConstructorInitializer<'input, 'allocator> {
    pub colon: Range<usize>,
    pub kind: Spanned<ConstructorInitializerKind>,
    pub arguments: Result<ArgumentList<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstructorInitializerKind {
    Base,
    This,
}

#[derive(Debug)]
pub struct DestructorDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub tilde: Range<usize>,
    pub name: Result<Ident<'input>, ()>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct OperatorDeclaration<'input, 'allocator> {
    pub documents: Documents<'input, 'allocator>,
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    /// Present for `implicit operator T(...)` / `explicit operator T(...)`.
    pub conversion: Option<Spanned<ConversionKind>>,
    pub operator_keyword: Range<usize>,
    /// For a conversion operator this is the type written after `operator`.
    pub result_type: TypeRef<'input, 'allocator>,
    /// Absent for conversion operators.
    pub symbol: Option<Spanned<OperatorSymbol>>,
    pub parameters: Result<ParameterList<'input, 'allocator>, ()>,
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversionKind {
    Implicit,
    Explicit,
}

/// The operators C# allows to be overloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorSymbol {
    Plus,
    Minus,
    Not,
    BitwiseNot,
    Increment,
    Decrement,
    True,
    False,
    Multiply,
    Divide,
    Modulo,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    LeftShift,
    RightShift,
    UnsignedRightShift,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessThanEqual,
    GreaterThanEqual,
}

#[derive(Debug)]
pub struct AccessorList<'input, 'allocator> {
    pub accessors: &'allocator [Accessor<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Accessor<'input, 'allocator> {
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub kind: Spanned<AccessorKind>,
    pub body: FunctionBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessorKind {
    Get,
    Set,
    Init,
    Add,
    Remove,
}

/// What follows a callable's signature.
#[derive(Debug)]
pub enum FunctionBody<'input, 'allocator> {
    Block(Block<'input, 'allocator>),
    /// `{ get; set; }` -- properties, indexers and events only.
    Accessors(AccessorList<'input, 'allocator>),
    /// `=> expression;`
    Expression {
        fat_arrow: Range<usize>,
        expression: Result<Expression<'input, 'allocator>, ()>,
        semicolon: Option<Range<usize>>,
        span: Range<usize>,
    },
    /// `;` -- abstract, `extern`, an interface method, or an auto-accessor.
    None {
        semicolon: Range<usize>,
    },
    /// Neither a body nor a `;` was found; an error has been recorded.
    Missing,
}

impl FunctionBody<'_, '_> {
    pub fn span(&self) -> Option<Range<usize>> {
        match self {
            FunctionBody::Block(block) => Some(block.span.clone()),
            FunctionBody::Accessors(accessors) => Some(accessors.span.clone()),
            FunctionBody::Expression { span, .. } => Some(span.clone()),
            FunctionBody::None { semicolon } => Some(semicolon.clone()),
            FunctionBody::Missing => None,
        }
    }
}

// ============================================================================
// parameters
// ============================================================================

#[derive(Debug)]
pub struct ParameterList<'input, 'allocator> {
    pub parameters: &'allocator [Parameter<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Parameter<'input, 'allocator> {
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub modifiers: &'allocator [Spanned<ParameterModifier>],
    /// A lambda may leave its parameter types to inference.
    pub parameter_type: Option<TypeRef<'input, 'allocator>>,
    pub name: Result<Ident<'input>, ()>,
    pub default_value: Option<Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParameterModifier {
    Ref,
    Out,
    In,
    Params,
    This,
    Scoped,
    Readonly,
}

// ============================================================================
// generics
// ============================================================================

#[derive(Debug)]
pub struct GenericsDefine<'input, 'allocator> {
    pub parameters: &'allocator [GenericsParameter<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct GenericsParameter<'input, 'allocator> {
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    pub variance: Option<Spanned<Variance>>,
    pub name: Ident<'input>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variance {
    In,
    Out,
}

/// A generic argument list, `<int, string>`.
#[derive(Debug, Clone)]
pub struct GenericsInfo<'input, 'allocator> {
    pub types: &'allocator [TypeRef<'input, 'allocator>],
    /// `typeof(List<>)` and `typeof(Dictionary<,>)` name a type by arity only,
    /// so `types` is empty while `arity` is 1 and 2 respectively.
    pub arity: usize,
    pub span: Range<usize>,
}

impl GenericsInfo<'_, '_> {
    pub fn is_unbound(&self) -> bool {
        self.types.is_empty() && self.arity > 0
    }
}

/// `where T : class, IFoo, new()`
#[derive(Debug)]
pub struct TypeParameterConstraint<'input, 'allocator> {
    pub where_keyword: Range<usize>,
    pub target: Result<Ident<'input>, ()>,
    pub bounds: &'allocator [ConstraintBound<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum ConstraintBound<'input, 'allocator> {
    Class {
        nullable: Option<Range<usize>>,
        span: Range<usize>,
    },
    Struct {
        span: Range<usize>,
    },
    NotNull {
        span: Range<usize>,
    },
    Unmanaged {
        span: Range<usize>,
    },
    New {
        span: Range<usize>,
    },
    Type(TypeRef<'input, 'allocator>),
}

// ============================================================================
// types
// ============================================================================

#[derive(Debug, Clone)]
pub struct TypeRef<'input, 'allocator> {
    pub base: TypeRefBase<'input, 'allocator>,
    /// `?`, `[]`, `[,]` and `*`, in written order, so `int[][,]?` keeps its shape.
    pub suffixes: &'allocator [TypeSuffix],
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub enum TypeRefBase<'input, 'allocator> {
    Name(NameType<'input, 'allocator>),
    Predefined(Spanned<PredefinedType>),
    Tuple(TupleType<'input, 'allocator>),
    /// `var`, only valid in a local declaration.
    Var(Range<usize>),
    /// `ref T` in a return type or local.
    Ref {
        ref_keyword: Range<usize>,
        readonly_keyword: Option<Range<usize>>,
        element: &'allocator TypeRef<'input, 'allocator>,
        span: Range<usize>,
    },
}

/// A possibly qualified, possibly generic name: `global::System.Collections.Generic.List<int>`.
#[derive(Debug, Clone)]
pub struct NameType<'input, 'allocator> {
    pub global: Option<Range<usize>>,
    pub segments: &'allocator [NameSegment<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct NameSegment<'input, 'allocator> {
    pub name: Ident<'input>,
    pub generics: Option<GenericsInfo<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeSuffix {
    Nullable {
        span: Range<usize>,
    },
    /// `[]` has rank 1, `[,]` rank 2.
    Array {
        rank: usize,
        span: Range<usize>,
    },
    Pointer {
        span: Range<usize>,
    },
}

impl TypeSuffix {
    pub fn span(&self) -> Range<usize> {
        match self {
            TypeSuffix::Nullable { span }
            | TypeSuffix::Array { span, .. }
            | TypeSuffix::Pointer { span } => span.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TupleType<'input, 'allocator> {
    pub elements: &'allocator [TupleTypeElement<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct TupleTypeElement<'input, 'allocator> {
    pub element_type: TypeRef<'input, 'allocator>,
    pub name: Option<Ident<'input>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredefinedType {
    Bool,
    Byte,
    Sbyte,
    Short,
    Ushort,
    Int,
    Uint,
    Long,
    Ulong,
    Char,
    Float,
    Double,
    Decimal,
    String,
    Object,
    Void,
    Nint,
    Nuint,
    Dynamic,
}

// ============================================================================
// statements
// ============================================================================

#[derive(Debug)]
pub struct Block<'input, 'allocator> {
    pub statements: &'allocator [Statement<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum Statement<'input, 'allocator> {
    Block(Block<'input, 'allocator>),
    LocalVariable(LocalVariableDeclaration<'input, 'allocator>),
    LocalFunction(MethodDeclaration<'input, 'allocator>),
    Expression(ExpressionStatement<'input, 'allocator>),
    If(IfStatement<'input, 'allocator>),
    While(WhileStatement<'input, 'allocator>),
    DoWhile(DoWhileStatement<'input, 'allocator>),
    For(ForStatement<'input, 'allocator>),
    Foreach(ForeachStatement<'input, 'allocator>),
    Switch(SwitchStatement<'input, 'allocator>),
    Try(TryStatement<'input, 'allocator>),
    Using(UsingStatement<'input, 'allocator>),
    Lock(LockStatement<'input, 'allocator>),
    Checked(CheckedStatement<'input, 'allocator>),
    Unsafe(UnsafeStatement<'input, 'allocator>),
    Fixed(FixedStatement<'input, 'allocator>),
    Return(ReturnStatement<'input, 'allocator>),
    Throw(ThrowStatement<'input, 'allocator>),
    Yield(YieldStatement<'input, 'allocator>),
    Break(BreakStatement),
    Continue(ContinueStatement),
    Goto(GotoStatement<'input, 'allocator>),
    Labeled(LabeledStatement<'input, 'allocator>),
    Empty { semicolon: Range<usize> },
}

impl Statement<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            Statement::Block(statement) => statement.span.clone(),
            Statement::LocalVariable(statement) => statement.span.clone(),
            Statement::LocalFunction(statement) => statement.span.clone(),
            Statement::Expression(statement) => statement.span.clone(),
            Statement::If(statement) => statement.span.clone(),
            Statement::While(statement) => statement.span.clone(),
            Statement::DoWhile(statement) => statement.span.clone(),
            Statement::For(statement) => statement.span.clone(),
            Statement::Foreach(statement) => statement.span.clone(),
            Statement::Switch(statement) => statement.span.clone(),
            Statement::Try(statement) => statement.span.clone(),
            Statement::Using(statement) => statement.span.clone(),
            Statement::Lock(statement) => statement.span.clone(),
            Statement::Checked(statement) => statement.span.clone(),
            Statement::Unsafe(statement) => statement.span.clone(),
            Statement::Fixed(statement) => statement.span.clone(),
            Statement::Return(statement) => statement.span.clone(),
            Statement::Throw(statement) => statement.span.clone(),
            Statement::Yield(statement) => statement.span.clone(),
            Statement::Break(statement) => statement.span.clone(),
            Statement::Continue(statement) => statement.span.clone(),
            Statement::Goto(statement) => statement.span.clone(),
            Statement::Labeled(statement) => statement.span.clone(),
            Statement::Empty { semicolon } => semicolon.clone(),
        }
    }
}

#[derive(Debug)]
pub struct LocalVariableDeclaration<'input, 'allocator> {
    pub attributes: &'allocator [AttributeSection<'input, 'allocator>],
    /// `const int x = 1;`
    pub const_keyword: Option<Range<usize>>,
    /// `using var stream = ...;`
    pub using_keyword: Option<Range<usize>>,
    pub variable_type: TypeRef<'input, 'allocator>,
    pub declarators: &'allocator [VariableDeclarator<'input, 'allocator>],
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ExpressionStatement<'input, 'allocator> {
    pub expression: Expression<'input, 'allocator>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct IfStatement<'input, 'allocator> {
    pub if_keyword: Range<usize>,
    pub condition: Result<Expression<'input, 'allocator>, ()>,
    pub then_branch: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub else_keyword: Option<Range<usize>>,
    pub else_branch: Option<&'allocator Statement<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct WhileStatement<'input, 'allocator> {
    pub while_keyword: Range<usize>,
    pub condition: Result<Expression<'input, 'allocator>, ()>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct DoWhileStatement<'input, 'allocator> {
    pub do_keyword: Range<usize>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub while_keyword: Option<Range<usize>>,
    pub condition: Result<Expression<'input, 'allocator>, ()>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ForStatement<'input, 'allocator> {
    pub for_keyword: Range<usize>,
    pub initializer: Option<ForInitializer<'input, 'allocator>>,
    pub condition: Option<Expression<'input, 'allocator>>,
    pub incrementors: &'allocator [Expression<'input, 'allocator>],
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum ForInitializer<'input, 'allocator> {
    Declaration(LocalVariableDeclaration<'input, 'allocator>),
    Expressions(&'allocator [Expression<'input, 'allocator>]),
}

#[derive(Debug)]
pub struct ForeachStatement<'input, 'allocator> {
    pub foreach_keyword: Range<usize>,
    pub variable_type: Result<TypeRef<'input, 'allocator>, ()>,
    /// `foreach (var x in ...)`, and the deconstructing
    /// `foreach (var (a, b) in ...)`.
    pub name: Result<VariableDesignation<'input, 'allocator>, ()>,
    pub in_keyword: Option<Range<usize>>,
    pub collection: Result<Expression<'input, 'allocator>, ()>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct SwitchStatement<'input, 'allocator> {
    pub switch_keyword: Range<usize>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub sections: Result<&'allocator [SwitchSection<'input, 'allocator>], ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct SwitchSection<'input, 'allocator> {
    pub labels: &'allocator [SwitchLabel<'input, 'allocator>],
    pub statements: &'allocator [Statement<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum SwitchLabel<'input, 'allocator> {
    Case {
        case_keyword: Range<usize>,
        pattern: Result<Pattern<'input, 'allocator>, ()>,
        guard: Option<Expression<'input, 'allocator>>,
        span: Range<usize>,
    },
    Default {
        default_keyword: Range<usize>,
        span: Range<usize>,
    },
}

#[derive(Debug)]
pub struct TryStatement<'input, 'allocator> {
    pub try_keyword: Range<usize>,
    pub block: Result<Block<'input, 'allocator>, ()>,
    pub catches: &'allocator [CatchClause<'input, 'allocator>],
    pub finally_clause: Option<FinallyClause<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct CatchClause<'input, 'allocator> {
    pub catch_keyword: Range<usize>,
    pub exception_type: Option<TypeRef<'input, 'allocator>>,
    pub name: Option<Ident<'input>>,
    pub filter: Option<CatchFilter<'input, 'allocator>>,
    pub block: Result<Block<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct CatchFilter<'input, 'allocator> {
    pub when_keyword: Range<usize>,
    pub condition: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct FinallyClause<'input, 'allocator> {
    pub finally_keyword: Range<usize>,
    pub block: Result<Block<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct UsingStatement<'input, 'allocator> {
    pub using_keyword: Range<usize>,
    pub resource: Result<UsingResource<'input, 'allocator>, ()>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum UsingResource<'input, 'allocator> {
    Declaration(LocalVariableDeclaration<'input, 'allocator>),
    Expression(Expression<'input, 'allocator>),
}

#[derive(Debug)]
pub struct LockStatement<'input, 'allocator> {
    pub lock_keyword: Range<usize>,
    pub target: Result<Expression<'input, 'allocator>, ()>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct CheckedStatement<'input, 'allocator> {
    pub kind: Spanned<CheckedKind>,
    pub block: Result<Block<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `unsafe { ... }`
#[derive(Debug)]
pub struct UnsafeStatement<'input, 'allocator> {
    pub unsafe_keyword: Range<usize>,
    pub block: Result<Block<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `fixed (int* p = &array[0]) { ... }`
#[derive(Debug)]
pub struct FixedStatement<'input, 'allocator> {
    pub fixed_keyword: Range<usize>,
    pub declaration: Result<LocalVariableDeclaration<'input, 'allocator>, ()>,
    pub body: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckedKind {
    Checked,
    Unchecked,
}

#[derive(Debug)]
pub struct ReturnStatement<'input, 'allocator> {
    pub return_keyword: Range<usize>,
    pub value: Option<Expression<'input, 'allocator>>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ThrowStatement<'input, 'allocator> {
    pub throw_keyword: Range<usize>,
    /// Absent for a bare `throw;` inside a `catch`.
    pub value: Option<Expression<'input, 'allocator>>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct YieldStatement<'input, 'allocator> {
    pub yield_keyword: Range<usize>,
    pub kind: Spanned<YieldKind>,
    pub value: Option<Expression<'input, 'allocator>>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YieldKind {
    Return,
    Break,
}

#[derive(Debug)]
pub struct BreakStatement {
    pub break_keyword: Range<usize>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ContinueStatement {
    pub continue_keyword: Range<usize>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct GotoStatement<'input, 'allocator> {
    pub goto_keyword: Range<usize>,
    pub target: GotoTarget<'input, 'allocator>,
    pub semicolon: Option<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum GotoTarget<'input, 'allocator> {
    Label(Result<Ident<'input>, ()>),
    Case {
        case_keyword: Range<usize>,
        value: Result<Expression<'input, 'allocator>, ()>,
    },
    Default {
        default_keyword: Range<usize>,
    },
}

#[derive(Debug)]
pub struct LabeledStatement<'input, 'allocator> {
    pub label: Ident<'input>,
    pub colon: Range<usize>,
    pub statement: Result<&'allocator Statement<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

// ============================================================================
// expressions
// ============================================================================

/// Every variant is arena-boxed, keeping `Expression` two words wide however deep the
/// tree gets. Precedence is encoded in the shape of the tree, built by precedence
/// climbing in the parser rather than by one struct per level.
#[derive(Debug)]
pub enum Expression<'input, 'allocator> {
    Assignment(&'allocator AssignmentExpression<'input, 'allocator>),
    Lambda(&'allocator LambdaExpression<'input, 'allocator>),
    AnonymousMethod(&'allocator AnonymousMethodExpression<'input, 'allocator>),
    Conditional(&'allocator ConditionalExpression<'input, 'allocator>),
    Binary(&'allocator BinaryExpression<'input, 'allocator>),
    Is(&'allocator IsExpression<'input, 'allocator>),
    As(&'allocator AsExpression<'input, 'allocator>),
    Range(&'allocator RangeExpression<'input, 'allocator>),
    Unary(&'allocator UnaryExpression<'input, 'allocator>),
    Cast(&'allocator CastExpression<'input, 'allocator>),
    Await(&'allocator AwaitExpression<'input, 'allocator>),
    Throw(&'allocator ThrowExpression<'input, 'allocator>),
    Switch(&'allocator SwitchExpression<'input, 'allocator>),
    With(&'allocator WithExpression<'input, 'allocator>),
    Query(&'allocator QueryExpression<'input, 'allocator>),
    Ref(&'allocator RefExpression<'input, 'allocator>),
    Declaration(&'allocator DeclarationExpression<'input, 'allocator>),
    Primary(&'allocator PrimaryExpression<'input, 'allocator>),
}

impl Expression<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            Expression::Assignment(expression) => expression.span.clone(),
            Expression::AnonymousMethod(expression) => expression.span.clone(),
            Expression::Lambda(expression) => expression.span.clone(),
            Expression::Conditional(expression) => expression.span.clone(),
            Expression::Binary(expression) => expression.span.clone(),
            Expression::Is(expression) => expression.span.clone(),
            Expression::As(expression) => expression.span.clone(),
            Expression::Range(expression) => expression.span.clone(),
            Expression::Unary(expression) => expression.span.clone(),
            Expression::Cast(expression) => expression.span.clone(),
            Expression::Await(expression) => expression.span.clone(),
            Expression::Throw(expression) => expression.span.clone(),
            Expression::Switch(expression) => expression.span.clone(),
            Expression::With(expression) => expression.span.clone(),
            Expression::Query(expression) => expression.span.clone(),
            Expression::Ref(expression) => expression.span.clone(),
            Expression::Declaration(expression) => expression.span.clone(),
            Expression::Primary(expression) => expression.span.clone(),
        }
    }
}

#[derive(Debug)]
pub struct AssignmentExpression<'input, 'allocator> {
    pub target: Expression<'input, 'allocator>,
    pub operator: Spanned<AssignmentOperator>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    LeftShift,
    RightShift,
    UnsignedRightShift,
    Coalesce,
}

#[derive(Debug)]
pub struct ConditionalExpression<'input, 'allocator> {
    pub condition: Expression<'input, 'allocator>,
    pub question_mark: Range<usize>,
    pub then_value: Result<Expression<'input, 'allocator>, ()>,
    pub colon: Option<Range<usize>>,
    pub else_value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct BinaryExpression<'input, 'allocator> {
    pub left: Expression<'input, 'allocator>,
    pub operator: Spanned<BinaryOperator>,
    pub right: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    Multiply,
    Divide,
    Modulo,
    Add,
    Subtract,
    LeftShift,
    RightShift,
    UnsignedRightShift,
    LessThan,
    GreaterThan,
    LessThanEqual,
    GreaterThanEqual,
    Equal,
    NotEqual,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
    LogicalAnd,
    LogicalOr,
    Coalesce,
}

#[derive(Debug)]
pub struct IsExpression<'input, 'allocator> {
    pub value: Expression<'input, 'allocator>,
    pub is_keyword: Range<usize>,
    pub pattern: Result<Pattern<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct AsExpression<'input, 'allocator> {
    pub value: Expression<'input, 'allocator>,
    pub as_keyword: Range<usize>,
    pub target_type: Result<TypeRef<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `a..b`, `..b`, `a..` and `..`.
#[derive(Debug)]
pub struct RangeExpression<'input, 'allocator> {
    pub start: Option<Expression<'input, 'allocator>>,
    pub dot_dot: Range<usize>,
    pub end: Option<Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct UnaryExpression<'input, 'allocator> {
    pub operator: Spanned<UnaryOperator>,
    pub operand: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    Plus,
    Minus,
    Not,
    BitwiseNot,
    PreIncrement,
    PreDecrement,
    /// `^i` -- index counted from the end.
    IndexFromEnd,
    /// `&x`
    AddressOf,
    /// `*p`
    Dereference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostfixOperator {
    Increment,
    Decrement,
    /// `x!` -- the null-forgiving operator.
    NullForgiving,
}

#[derive(Debug)]
pub struct CastExpression<'input, 'allocator> {
    pub target_type: TypeRef<'input, 'allocator>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct AwaitExpression<'input, 'allocator> {
    pub await_keyword: Range<usize>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `ref x`, as in `ref int y = ref x;` and `return ref field;`.
#[derive(Debug)]
pub struct RefExpression<'input, 'allocator> {
    pub ref_keyword: Range<usize>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// A variable declared where an expression is expected.
///
/// This is how C# models deconstruction: `var (a, b) = t` is an ordinary assignment whose
/// left side is `var (a, b)`, and `(int a, string b) = t` is an assignment to a tuple of
/// two of these.
#[derive(Debug)]
pub struct DeclarationExpression<'input, 'allocator> {
    pub variable_type: TypeRef<'input, 'allocator>,
    pub designation: VariableDesignation<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ThrowExpression<'input, 'allocator> {
    pub throw_keyword: Range<usize>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct LambdaExpression<'input, 'allocator> {
    pub modifiers: &'allocator [Spanned<Modifier>],
    /// A lambda may declare an explicit return type: `int () => 0`.
    pub return_type: Option<TypeRef<'input, 'allocator>>,
    pub parameters: LambdaParameters<'input, 'allocator>,
    pub fat_arrow: Range<usize>,
    pub body: Result<LambdaBody<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `delegate(int x) { ... }` and `delegate { ... }` -- the pre-lambda anonymous method
/// form. Superseded by lambdas in practice, but still valid C#.
#[derive(Debug)]
pub struct AnonymousMethodExpression<'input, 'allocator> {
    /// Only `async` is allowed here.
    pub modifiers: &'allocator [Spanned<Modifier>],
    pub delegate_keyword: Range<usize>,
    /// `delegate { ... }` leaves the list out entirely, which makes the method
    /// convertible to any delegate type whose parameters it ignores.
    pub parameters: Option<ParameterList<'input, 'allocator>>,
    pub body: Result<Block<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum LambdaParameters<'input, 'allocator> {
    /// `x => ...`
    Single(Ident<'input>),
    /// `(x, y) => ...` and `(int x) => ...`
    List(ParameterList<'input, 'allocator>),
}

impl LambdaParameters<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            LambdaParameters::Single(name) => name.span.clone(),
            LambdaParameters::List(list) => list.span.clone(),
        }
    }
}

#[derive(Debug)]
pub enum LambdaBody<'input, 'allocator> {
    Expression(Expression<'input, 'allocator>),
    Block(Block<'input, 'allocator>),
}

#[derive(Debug)]
pub struct SwitchExpression<'input, 'allocator> {
    pub value: Expression<'input, 'allocator>,
    pub switch_keyword: Range<usize>,
    pub arms: Result<&'allocator [SwitchExpressionArm<'input, 'allocator>], ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct SwitchExpressionArm<'input, 'allocator> {
    pub pattern: Pattern<'input, 'allocator>,
    pub guard: Option<Expression<'input, 'allocator>>,
    pub fat_arrow: Option<Range<usize>>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct WithExpression<'input, 'allocator> {
    pub value: Expression<'input, 'allocator>,
    pub with_keyword: Range<usize>,
    pub initializer: Result<Initializer<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// query expressions
// ---------------------------------------------------------------------------

/// `from x in xs where x > 0 orderby x descending select x * 2`
///
/// The tree mirrors the written clauses rather than the method chain they desugar to;
/// rewriting `select` into `Select(...)` is a later pass's decision.
#[derive(Debug)]
pub struct QueryExpression<'input, 'allocator> {
    pub from: FromClause<'input, 'allocator>,
    pub body: &'allocator QueryBody<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct QueryBody<'input, 'allocator> {
    pub clauses: &'allocator [QueryClause<'input, 'allocator>],
    pub select_or_group: Result<SelectOrGroupClause<'input, 'allocator>, ()>,
    /// `into g ...` -- feeds the result of this body into another one.
    pub continuation: Option<QueryContinuation<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum QueryClause<'input, 'allocator> {
    From(FromClause<'input, 'allocator>),
    Let(LetClause<'input, 'allocator>),
    Where(WhereQueryClause<'input, 'allocator>),
    Join(JoinClause<'input, 'allocator>),
    OrderBy(OrderByClause<'input, 'allocator>),
}

impl QueryClause<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            QueryClause::From(clause) => clause.span.clone(),
            QueryClause::Let(clause) => clause.span.clone(),
            QueryClause::Where(clause) => clause.span.clone(),
            QueryClause::Join(clause) => clause.span.clone(),
            QueryClause::OrderBy(clause) => clause.span.clone(),
        }
    }
}

/// `from int x in xs`
#[derive(Debug)]
pub struct FromClause<'input, 'allocator> {
    pub from_keyword: Range<usize>,
    pub element_type: Option<TypeRef<'input, 'allocator>>,
    pub name: Ident<'input>,
    pub in_keyword: Range<usize>,
    pub source: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `let y = x * 2`
#[derive(Debug)]
pub struct LetClause<'input, 'allocator> {
    pub let_keyword: Range<usize>,
    pub name: Result<Ident<'input>, ()>,
    pub equal: Option<Range<usize>>,
    pub value: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `where x > 0`
#[derive(Debug)]
pub struct WhereQueryClause<'input, 'allocator> {
    pub where_keyword: Range<usize>,
    pub condition: Result<Expression<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

/// `join y in ys on x.Id equals y.Id into g`
#[derive(Debug)]
pub struct JoinClause<'input, 'allocator> {
    pub join_keyword: Range<usize>,
    pub element_type: Option<TypeRef<'input, 'allocator>>,
    pub name: Result<Ident<'input>, ()>,
    pub in_keyword: Option<Range<usize>>,
    pub source: Result<Expression<'input, 'allocator>, ()>,
    pub on_keyword: Option<Range<usize>>,
    pub left_key: Result<Expression<'input, 'allocator>, ()>,
    pub equals_keyword: Option<Range<usize>>,
    pub right_key: Result<Expression<'input, 'allocator>, ()>,
    /// The group join form, `join ... into g`.
    pub into: Option<Ident<'input>>,
    pub span: Range<usize>,
}

/// `orderby a, b descending`
#[derive(Debug)]
pub struct OrderByClause<'input, 'allocator> {
    pub orderby_keyword: Range<usize>,
    pub orderings: &'allocator [Ordering<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Ordering<'input, 'allocator> {
    pub key: Expression<'input, 'allocator>,
    pub direction: Option<Spanned<OrderDirection>>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    Ascending,
    Descending,
}

#[derive(Debug)]
pub enum SelectOrGroupClause<'input, 'allocator> {
    Select {
        select_keyword: Range<usize>,
        value: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Group {
        group_keyword: Range<usize>,
        value: Result<Expression<'input, 'allocator>, ()>,
        by_keyword: Option<Range<usize>>,
        key: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
}

#[derive(Debug)]
pub struct QueryContinuation<'input, 'allocator> {
    pub into_keyword: Range<usize>,
    pub name: Result<Ident<'input>, ()>,
    pub body: &'allocator QueryBody<'input, 'allocator>,
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// primary expressions
// ---------------------------------------------------------------------------

/// A primary expression is a head followed by a flat chain of postfix operations,
/// which is exactly how `a.b?.c(1)[2]++` reads.
#[derive(Debug)]
pub struct PrimaryExpression<'input, 'allocator> {
    pub left: PrimaryLeft<'input, 'allocator>,
    pub chain: &'allocator [PrimaryRight<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum PrimaryLeft<'input, 'allocator> {
    Literal(LiteralExpression<'input, 'allocator>),
    Identifier {
        name: Ident<'input>,
        generics: Option<GenericsInfo<'input, 'allocator>>,
        span: Range<usize>,
    },
    /// `int.Parse(...)`, `string.Empty`
    Predefined(Spanned<PredefinedType>),
    /// `global::System`
    Global(Range<usize>),
    This(Range<usize>),
    Base(Range<usize>),
    Parenthesized {
        expression: Expression<'input, 'allocator>,
        span: Range<usize>,
    },
    /// `(a, b)` -- a tuple literal, or a deconstruction target.
    Tuple {
        elements: &'allocator [TupleElement<'input, 'allocator>],
        span: Range<usize>,
    },
    New(NewExpression<'input, 'allocator>),
    /// `new { A = 1 }`
    AnonymousObject {
        new_keyword: Range<usize>,
        initializers: &'allocator [ObjectInitializerElement<'input, 'allocator>],
        span: Range<usize>,
    },
    Typeof {
        typeof_keyword: Range<usize>,
        target_type: Result<TypeRef<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Sizeof {
        sizeof_keyword: Range<usize>,
        target_type: Result<TypeRef<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Nameof {
        nameof_keyword: Range<usize>,
        value: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Default {
        default_keyword: Range<usize>,
        target_type: Option<TypeRef<'input, 'allocator>>,
        span: Range<usize>,
    },
    Checked {
        kind: Spanned<CheckedKind>,
        value: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Stackalloc(StackallocExpression<'input, 'allocator>),
    /// `[1, 2, ..rest]`
    Collection {
        elements: &'allocator [CollectionExpressionElement<'input, 'allocator>],
        span: Range<usize>,
    },
}

impl PrimaryLeft<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            PrimaryLeft::Literal(literal) => literal.span(),
            PrimaryLeft::Identifier { span, .. } => span.clone(),
            PrimaryLeft::Predefined(predefined) => predefined.span.clone(),
            PrimaryLeft::Global(span) => span.clone(),
            PrimaryLeft::This(span) => span.clone(),
            PrimaryLeft::Base(span) => span.clone(),
            PrimaryLeft::Parenthesized { span, .. } => span.clone(),
            PrimaryLeft::Tuple { span, .. } => span.clone(),
            PrimaryLeft::New(new_expression) => new_expression.span.clone(),
            PrimaryLeft::AnonymousObject { span, .. } => span.clone(),
            PrimaryLeft::Typeof { span, .. } => span.clone(),
            PrimaryLeft::Sizeof { span, .. } => span.clone(),
            PrimaryLeft::Nameof { span, .. } => span.clone(),
            PrimaryLeft::Default { span, .. } => span.clone(),
            PrimaryLeft::Checked { span, .. } => span.clone(),
            PrimaryLeft::Stackalloc(stackalloc) => stackalloc.span.clone(),
            PrimaryLeft::Collection { span, .. } => span.clone(),
        }
    }
}

#[derive(Debug)]
pub enum CollectionExpressionElement<'input, 'allocator> {
    Expression(Expression<'input, 'allocator>),
    /// `..rest`
    Spread {
        dot_dot: Range<usize>,
        value: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
}

/// `stackalloc int[10]`, `stackalloc int[] { 1, 2 }` and `stackalloc[] { 1, 2 }`.
#[derive(Debug)]
pub struct StackallocExpression<'input, 'allocator> {
    pub stackalloc_keyword: Range<usize>,
    /// Absent in the target typed `stackalloc[] { ... }` form.
    pub element_type: Option<TypeRef<'input, 'allocator>>,
    pub size: Option<Expression<'input, 'allocator>>,
    pub initializer: Option<Initializer<'input, 'allocator>>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum PrimaryRight<'input, 'allocator> {
    Member {
        separator: Spanned<MemberSeparator>,
        name: Result<Ident<'input>, ()>,
        generics: Option<GenericsInfo<'input, 'allocator>>,
        span: Range<usize>,
    },
    Invocation {
        arguments: ArgumentList<'input, 'allocator>,
        span: Range<usize>,
    },
    ElementAccess {
        /// `?[` rather than `[`.
        null_conditional: bool,
        arguments: ArgumentList<'input, 'allocator>,
        span: Range<usize>,
    },
    Postfix {
        operator: Spanned<PostfixOperator>,
        span: Range<usize>,
    },
}

impl PrimaryRight<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            PrimaryRight::Member { span, .. } => span.clone(),
            PrimaryRight::Invocation { span, .. } => span.clone(),
            PrimaryRight::ElementAccess { span, .. } => span.clone(),
            PrimaryRight::Postfix { span, .. } => span.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberSeparator {
    /// `.`
    Dot,
    /// `?.`
    NullConditionalDot,
    /// `::`
    DoubleColon,
    /// `->` -- pointer member access.
    Arrow,
}

#[derive(Debug)]
pub enum LiteralExpression<'input, 'allocator> {
    Integer(LiteralText<'input>),
    Real(LiteralText<'input>),
    Char(LiteralText<'input>),
    String(LiteralText<'input>),
    VerbatimString(LiteralText<'input>),
    RawString(LiteralText<'input>),
    InterpolatedString(&'allocator InterpolatedString<'input, 'allocator>),
    True(Range<usize>),
    False(Range<usize>),
    Null(Range<usize>),
}

impl LiteralExpression<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            LiteralExpression::Integer(literal)
            | LiteralExpression::Real(literal)
            | LiteralExpression::Char(literal)
            | LiteralExpression::String(literal)
            | LiteralExpression::VerbatimString(literal)
            | LiteralExpression::RawString(literal) => literal.span.clone(),
            LiteralExpression::InterpolatedString(literal) => literal.span.clone(),
            LiteralExpression::True(span)
            | LiteralExpression::False(span)
            | LiteralExpression::Null(span) => span.clone(),
        }
    }
}

/// An interpolated string, split into the text around its holes and the holes themselves.
///
/// The holes hold real expressions, not text, because the compiler has to evaluate them:
/// a hole can read a variable, call a method, assign, or `await`, and lowering the string
/// to concatenation needs a tree to emit code from.
#[derive(Debug)]
pub struct InterpolatedString<'input, 'allocator> {
    pub parts: &'allocator [InterpolationPart<'input, 'allocator>],
    /// A verbatim form such as `$@"..."`. Text parts keep a backslash as written rather
    /// than as the start of an escape.
    pub is_verbatim: bool,
    /// A raw form. Also unescaped, and a hole opens on as many braces as there are `$`.
    pub is_raw: bool,
    /// The whole token, delimiters included.
    pub text: LiteralText<'input>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum InterpolationPart<'input, 'allocator> {
    /// Literal text between holes, exactly as written -- doubled braces and escape
    /// sequences are left for a later pass to decode.
    Text(LiteralText<'input>),
    Hole(InterpolationHole<'input, 'allocator>),
}

impl InterpolationPart<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            InterpolationPart::Text(text) => text.span.clone(),
            InterpolationPart::Hole(hole) => hole.span.clone(),
        }
    }
}

/// `{ expression [, alignment] [: format] }`
///
/// The three parts are separated by the first `,` and the first `:` that are not nested
/// inside brackets. That is why C# needs parentheses around a conditional in a hole: in
/// `{a ? b : c}` the `:` ends the expression. This parser splits the same way, so a file
/// that compiles here compiles in C# too.
#[derive(Debug)]
pub struct InterpolationHole<'input, 'allocator> {
    pub expression: Result<Expression<'input, 'allocator>, ()>,
    pub comma: Option<Range<usize>>,
    /// A constant expression giving the field width, as in `{x,-5}`.
    pub alignment: Option<Expression<'input, 'allocator>>,
    pub colon: Option<Range<usize>>,
    /// A format string rather than code, as in `{x:F2}`, so it stays text.
    pub format: Option<LiteralText<'input>>,
    pub span: Range<usize>,
}

/// `new T(...) { ... }`, `new(...)`, `new int[n]`, `new[] { ... }`.
#[derive(Debug)]
pub struct NewExpression<'input, 'allocator> {
    pub new_keyword: Range<usize>,
    /// Absent for target-typed `new(...)` and for `new[] { ... }`.
    pub created_type: Option<TypeRef<'input, 'allocator>>,
    /// Non-empty makes this an array creation: `new int[a, b]`.
    pub array_sizes: &'allocator [Expression<'input, 'allocator>],
    /// Extra `[]` written after the sized rank, as in `new int[n][]`.
    pub array_suffixes: &'allocator [TypeSuffix],
    pub arguments: Option<ArgumentList<'input, 'allocator>>,
    pub initializer: Option<Initializer<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl NewExpression<'_, '_> {
    pub fn is_array_creation(&self) -> bool {
        !self.array_sizes.is_empty()
            || self
                .created_type
                .as_ref()
                .map(|created_type| {
                    created_type
                        .suffixes
                        .iter()
                        .any(|suffix| matches!(suffix, TypeSuffix::Array { .. }))
                })
                .unwrap_or(false)
    }
}

#[derive(Debug)]
pub struct ArgumentList<'input, 'allocator> {
    pub arguments: &'allocator [Argument<'input, 'allocator>],
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Argument<'input, 'allocator> {
    /// `Method(name: value)`
    pub name: Option<Ident<'input>>,
    pub modifier: Option<Spanned<ArgumentModifier>>,
    pub value: ArgumentValue<'input, 'allocator>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArgumentModifier {
    Ref,
    Out,
    In,
}

#[derive(Debug)]
pub enum ArgumentValue<'input, 'allocator> {
    Expression(Expression<'input, 'allocator>),
    /// `out int x` / `out var x` -- declares the variable at the call site.
    Declaration {
        variable_type: TypeRef<'input, 'allocator>,
        name: Ident<'input>,
        span: Range<usize>,
    },
    Missing,
}

#[derive(Debug)]
pub struct TupleElement<'input, 'allocator> {
    pub name: Option<Ident<'input>>,
    pub value: Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// initializers
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Initializer<'input, 'allocator> {
    /// `{ A = 1, [k] = v }`
    Object {
        elements: &'allocator [ObjectInitializerElement<'input, 'allocator>],
        span: Range<usize>,
    },
    /// `{ 1, 2, 3 }` and the nested form `{ { 1, 2 }, { 3, 4 } }`.
    Collection {
        elements: &'allocator [CollectionElement<'input, 'allocator>],
        span: Range<usize>,
    },
}

impl Initializer<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            Initializer::Object { span, .. } => span.clone(),
            Initializer::Collection { span, .. } => span.clone(),
        }
    }
}

#[derive(Debug)]
pub struct ObjectInitializerElement<'input, 'allocator> {
    pub target: InitializerTarget<'input, 'allocator>,
    pub equal: Option<Range<usize>>,
    pub value: Result<InitializerValue<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub enum InitializerTarget<'input, 'allocator> {
    Member(Ident<'input>),
    /// `[key] = value`
    Index {
        arguments: &'allocator [Expression<'input, 'allocator>],
        span: Range<usize>,
    },
}

#[derive(Debug)]
pub enum InitializerValue<'input, 'allocator> {
    Expression(Expression<'input, 'allocator>),
    Nested(Initializer<'input, 'allocator>),
}

impl InitializerValue<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            InitializerValue::Expression(expression) => expression.span(),
            InitializerValue::Nested(initializer) => initializer.span(),
        }
    }
}

#[derive(Debug)]
pub enum CollectionElement<'input, 'allocator> {
    Expression(Expression<'input, 'allocator>),
    Nested(Initializer<'input, 'allocator>),
}

// ============================================================================
// patterns
// ============================================================================

#[derive(Debug)]
pub enum Pattern<'input, 'allocator> {
    /// `_`
    Discard(Range<usize>),
    /// `string`, `string s`
    Declaration {
        pattern_type: TypeRef<'input, 'allocator>,
        designation: Option<Ident<'input>>,
        span: Range<usize>,
    },
    /// `var x` and the deconstructing `var (a, b)`
    Var {
        var_keyword: Range<usize>,
        designation: Result<VariableDesignation<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    /// `42`, `null`, `Color.Red`
    Constant(Expression<'input, 'allocator>),
    /// `> 0`, `<= 10`
    Relational {
        operator: Spanned<RelationalOperator>,
        value: Result<Expression<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Not {
        not_keyword: Range<usize>,
        pattern: Result<&'allocator Pattern<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    And {
        left: &'allocator Pattern<'input, 'allocator>,
        and_keyword: Range<usize>,
        right: Result<&'allocator Pattern<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Or {
        left: &'allocator Pattern<'input, 'allocator>,
        or_keyword: Range<usize>,
        right: Result<&'allocator Pattern<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    Parenthesized {
        pattern: Result<&'allocator Pattern<'input, 'allocator>, ()>,
        span: Range<usize>,
    },
    /// `Point(0, var y)` and the bare tuple form `(0, 1)`.
    ///
    /// C# allows a property part and a name after it, as in `Point(0, 0) { Z: 1 } p`.
    Positional {
        pattern_type: Option<TypeRef<'input, 'allocator>>,
        subpatterns: &'allocator [PositionalSubpattern<'input, 'allocator>],
        property_subpatterns: &'allocator [PropertySubpattern<'input, 'allocator>],
        designation: Option<Ident<'input>>,
        span: Range<usize>,
    },
    /// `[1, 2, ..]`
    List {
        elements: &'allocator [Pattern<'input, 'allocator>],
        designation: Option<Ident<'input>>,
        span: Range<usize>,
    },
    /// `..` and `..var rest`, only valid inside a list pattern.
    Slice {
        dot_dot: Range<usize>,
        pattern: Option<&'allocator Pattern<'input, 'allocator>>,
        span: Range<usize>,
    },
    /// `Point { X: 0, Y: var y } p`
    Property {
        pattern_type: Option<TypeRef<'input, 'allocator>>,
        subpatterns: &'allocator [PropertySubpattern<'input, 'allocator>],
        designation: Option<Ident<'input>>,
        span: Range<usize>,
    },
}

impl Pattern<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            Pattern::Discard(span) => span.clone(),
            Pattern::Declaration { span, .. } => span.clone(),
            Pattern::Var { span, .. } => span.clone(),
            Pattern::Constant(expression) => expression.span(),
            Pattern::Relational { span, .. } => span.clone(),
            Pattern::Not { span, .. } => span.clone(),
            Pattern::And { span, .. } => span.clone(),
            Pattern::Or { span, .. } => span.clone(),
            Pattern::Parenthesized { span, .. } => span.clone(),
            Pattern::Positional { span, .. } => span.clone(),
            Pattern::List { span, .. } => span.clone(),
            Pattern::Slice { span, .. } => span.clone(),
            Pattern::Property { span, .. } => span.clone(),
        }
    }
}

/// One element of a positional pattern, optionally naming the member it matches.
#[derive(Debug)]
pub struct PositionalSubpattern<'input, 'allocator> {
    pub name: Option<Ident<'input>>,
    pub pattern: Pattern<'input, 'allocator>,
    pub span: Range<usize>,
}

/// What a `var` pattern or a deconstruction binds to.
#[derive(Debug)]
pub enum VariableDesignation<'input, 'allocator> {
    Single(Ident<'input>),
    /// `_`
    Discard(Range<usize>),
    /// `(a, (b, c))`
    Parenthesized {
        elements: &'allocator [VariableDesignation<'input, 'allocator>],
        span: Range<usize>,
    },
}

impl VariableDesignation<'_, '_> {
    pub fn span(&self) -> Range<usize> {
        match self {
            VariableDesignation::Single(name) => name.span.clone(),
            VariableDesignation::Discard(span) => span.clone(),
            VariableDesignation::Parenthesized { span, .. } => span.clone(),
        }
    }
}

#[derive(Debug)]
pub struct PropertySubpattern<'input, 'allocator> {
    pub name: Ident<'input>,
    pub colon: Option<Range<usize>>,
    pub pattern: Result<Pattern<'input, 'allocator>, ()>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelationalOperator {
    LessThan,
    GreaterThan,
    LessThanEqual,
    GreaterThanEqual,
}
