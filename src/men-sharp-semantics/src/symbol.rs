//! Symbols and the symbol table.
//!
//! A symbol is what a name in the source refers to, which is not the same thing as a
//! syntax node: a `partial` type declared in three files is one symbol with three
//! declaration sites, and an open namespace is one symbol no matter how many files
//! reopen it. Later phases (name resolution, type resolution) look names up here and
//! follow [`DeclarationSite::syntax`] back into the syntax tree when they need the
//! written-out signature.
//!
//! The table borrows the frozen syntax trees (`'ast`): symbol names are slices of the
//! source text and declaration sites are typed references into the arenas. Nothing is
//! copied, so the trees must stay alive for as long as the table does — the compiler
//! driver owns both and hands the table out by reference.

use std::{collections::HashMap, ops::Range};

use men_sharp_parser::ast::{
    ClassDeclaration, ClassKind, ConstructorDeclaration, ConversionKind, DelegateDeclaration,
    DestructorDeclaration, EntityID, EnumDeclaration, EnumMember, EventDeclaration,
    FieldDeclaration, GenericsParameter, IndexerDeclaration, MethodDeclaration,
    NamespaceDeclaration, OperatorDeclaration, OperatorSymbol, PropertyDeclaration,
    VariableDeclarator,
};

/// Index of a source file within one compilation, in the order the files were given
/// to the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Index into a [`SymbolTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Namespace,
    Class,
    Struct,
    Interface,
    Record,
    RecordStruct,
    Enum,
    Delegate,
    Field,
    Method,
    Property,
    Indexer,
    Event,
    Constructor,
    Destructor,
    Operator,
    EnumMember,
    TypeParameter,
}

impl SymbolKind {
    pub fn is_type(&self) -> bool {
        matches!(
            self,
            SymbolKind::Class
                | SymbolKind::Struct
                | SymbolKind::Interface
                | SymbolKind::Record
                | SymbolKind::RecordStruct
                | SymbolKind::Enum
                | SymbolKind::Delegate
        )
    }

    /// Member kinds that C# lets share one name inside one container.
    pub fn is_overloadable(&self) -> bool {
        matches!(
            self,
            SymbolKind::Method
                | SymbolKind::Constructor
                | SymbolKind::Indexer
                | SymbolKind::Operator
        )
    }
}

/// C# accessibility, source and metadata alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Accessibility {
    Public,
    Internal,
    Protected,
    /// `protected internal`.
    ProtectedInternal,
    /// `private protected`.
    PrivateProtected,
    Private,
    /// C# 11 `file`.
    File,
}

impl From<ClassKind> for SymbolKind {
    fn from(kind: ClassKind) -> Self {
        match kind {
            ClassKind::Class => SymbolKind::Class,
            ClassKind::Struct => SymbolKind::Struct,
            ClassKind::Interface => SymbolKind::Interface,
            ClassKind::Record => SymbolKind::Record,
            ClassKind::RecordStruct => SymbolKind::RecordStruct,
        }
    }
}

/// A typed reference from a symbol back to the syntax that declared it.
///
/// This is what makes the table useful to the next phase: resolving a method's
/// signature needs the written parameter and return types, and those live in the
/// syntax tree, not here.
#[derive(Debug, Clone, Copy)]
pub enum SyntaxRef<'ast> {
    Namespace(&'ast NamespaceDeclaration<'ast, 'ast>),
    Class(&'ast ClassDeclaration<'ast, 'ast>),
    Enum(&'ast EnumDeclaration<'ast, 'ast>),
    Delegate(&'ast DelegateDeclaration<'ast, 'ast>),
    /// One field declaration can declare several names; the site points at its own
    /// declarator.
    Field {
        field: &'ast FieldDeclaration<'ast, 'ast>,
        declarator: &'ast VariableDeclarator<'ast, 'ast>,
    },
    Method(&'ast MethodDeclaration<'ast, 'ast>),
    Property(&'ast PropertyDeclaration<'ast, 'ast>),
    Indexer(&'ast IndexerDeclaration<'ast, 'ast>),
    Event {
        event: &'ast EventDeclaration<'ast, 'ast>,
        declarator: &'ast VariableDeclarator<'ast, 'ast>,
    },
    Constructor(&'ast ConstructorDeclaration<'ast, 'ast>),
    Destructor(&'ast DestructorDeclaration<'ast, 'ast>),
    Operator(&'ast OperatorDeclaration<'ast, 'ast>),
    EnumMember(&'ast EnumMember<'ast, 'ast>),
    TypeParameter(&'ast GenericsParameter<'ast, 'ast>),
}

impl SyntaxRef<'_> {
    /// The identity of the declaring node, for keying phase side-tables.
    ///
    /// For multi-declarator declarations this is the declarator's identity, so each
    /// declared name keys separately.
    pub fn entity_id(&self) -> EntityID {
        match self {
            SyntaxRef::Namespace(node) => EntityID::from(*node),
            SyntaxRef::Class(node) => EntityID::from(*node),
            SyntaxRef::Enum(node) => EntityID::from(*node),
            SyntaxRef::Delegate(node) => EntityID::from(*node),
            SyntaxRef::Field { declarator, .. } => EntityID::from(*declarator),
            SyntaxRef::Method(node) => EntityID::from(*node),
            SyntaxRef::Property(node) => EntityID::from(*node),
            SyntaxRef::Indexer(node) => EntityID::from(*node),
            SyntaxRef::Event { declarator, .. } => EntityID::from(*declarator),
            SyntaxRef::Constructor(node) => EntityID::from(*node),
            SyntaxRef::Destructor(node) => EntityID::from(*node),
            SyntaxRef::Operator(node) => EntityID::from(*node),
            SyntaxRef::EnumMember(node) => EntityID::from(*node),
            SyntaxRef::TypeParameter(node) => EntityID::from(*node),
        }
    }
}

/// One place in one file where a symbol was declared.
#[derive(Debug, Clone, Copy)]
pub struct DeclarationSite<'ast> {
    pub file: FileId,
    /// The declared name's span (or the declaration's span when the name is a parse
    /// hole), which is where diagnostics about this symbol should point.
    pub span_start: usize,
    pub span_end: usize,
    pub syntax: SyntaxRef<'ast>,
}

impl DeclarationSite<'_> {
    pub fn span(&self) -> Range<usize> {
        self.span_start..self.span_end
    }
}

#[derive(Debug)]
pub struct Symbol<'ast> {
    pub kind: SymbolKind,
    /// The declared name. Members that C# does not let the user name get their
    /// metadata names: `.ctor`, `Finalize`, `this[]`, `op_Addition`, ...
    pub name: &'ast str,
    /// Generic parameter count. `Foo` and `Foo<T>` are different symbols.
    pub arity: u32,
    /// `None` only for the root namespace.
    pub parent: Option<SymbolId>,
    /// Every declaration in source order (file order, then position). More than one
    /// for namespaces and `partial` types.
    pub declarations: Vec<DeclarationSite<'ast>>,
    /// Whether every declaration carried the `partial` modifier.
    pub is_partial: bool,
    pub is_static: bool,
    /// Written accessibility, or the container's default (members `private`,
    /// interface members `public`, top-level types `internal`, ...).
    pub accessibility: Accessibility,
    /// `out`/`in` on a type-parameter symbol; `Invariant` everywhere else.
    pub variance: crate::types::TypeVariance,
    /// `void IFoo.Bar()` — excluded from ordinary member lookup and name-clash rules.
    pub is_explicit_implementation: bool,
    /// Nested symbols in declaration order: namespace members, type members and
    /// nested types. Type parameters are kept apart in [`Self::type_parameters`]
    /// because they live in a different lookup scope.
    pub members: Vec<SymbolId>,
    pub type_parameters: Vec<SymbolId>,
    member_map: HashMap<&'ast str, Vec<SymbolId>>,
}

impl<'ast> Symbol<'ast> {
    /// All members with the given name: overloads, or a type family differing in arity.
    pub fn members_named(&self, name: &str) -> &[SymbolId] {
        self.member_map.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn member_names(&self) -> impl Iterator<Item = (&'ast str, &[SymbolId])> {
        self.member_map
            .iter()
            .map(|(name, ids)| (*name, ids.as_slice()))
    }
}

/// The merged symbol table of one compilation.
#[derive(Debug)]
pub struct SymbolTable<'ast> {
    symbols: Vec<Symbol<'ast>>,
    root: SymbolId,
}

impl<'ast> SymbolTable<'ast> {
    pub(crate) fn new() -> Self {
        let root = Symbol {
            kind: SymbolKind::Namespace,
            name: "",
            arity: 0,
            parent: None,
            declarations: Vec::new(),
            is_partial: false,
            is_static: false,
            accessibility: Accessibility::Public,
            variance: crate::types::TypeVariance::Invariant,
            is_explicit_implementation: false,
            members: Vec::new(),
            type_parameters: Vec::new(),
            member_map: HashMap::new(),
        };

        Self {
            symbols: vec![root],
            root: SymbolId(0),
        }
    }

    /// The global namespace.
    pub fn root(&self) -> SymbolId {
        self.root
    }

    pub fn symbol(&self, id: SymbolId) -> &Symbol<'ast> {
        &self.symbols[id.0 as usize]
    }

    pub(crate) fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol<'ast> {
        &mut self.symbols[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (SymbolId, &Symbol<'ast>)> {
        self.symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (SymbolId(index as u32), symbol))
    }

    /// Adds a member symbol under `parent` and indexes it by name.
    pub(crate) fn add_member(&mut self, parent: SymbolId, mut symbol: Symbol<'ast>) -> SymbolId {
        symbol.parent = Some(parent);
        let id = SymbolId(self.symbols.len() as u32);
        let name = symbol.name;
        self.symbols.push(symbol);

        let parent = self.symbol_mut(parent);
        parent.members.push(id);
        parent.member_map.entry(name).or_default().push(id);
        id
    }

    /// Adds a type parameter symbol under `parent`, outside the member namespace.
    pub(crate) fn add_type_parameter(
        &mut self,
        parent: SymbolId,
        mut symbol: Symbol<'ast>,
    ) -> SymbolId {
        symbol.parent = Some(parent);
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(symbol);
        self.symbol_mut(parent).type_parameters.push(id);
        id
    }

    /// `A.B.C` for namespaces and `A.B.List`1` for generic types, mirroring metadata
    /// names. Debug and diagnostics only; resolution never goes through strings.
    pub fn fully_qualified_name(&self, id: SymbolId) -> String {
        let mut segments = Vec::new();
        let mut current = Some(id);

        while let Some(id) = current {
            let symbol = self.symbol(id);
            if symbol.parent.is_some() {
                segments.push(match symbol.arity {
                    0 => symbol.name.to_string(),
                    arity => format!("{}`{}", symbol.name, arity),
                });
            }
            current = symbol.parent;
        }

        segments.reverse();
        segments.join(".")
    }
}

/// A fresh symbol with no members yet.
pub(crate) fn new_symbol<'ast>(
    kind: SymbolKind,
    name: &'ast str,
    arity: u32,
    site: DeclarationSite<'ast>,
) -> Symbol<'ast> {
    Symbol {
        kind,
        name,
        arity,
        parent: None,
        declarations: vec![site],
        is_partial: false,
        is_static: false,
        accessibility: Accessibility::Public,
        variance: crate::types::TypeVariance::Invariant,
        is_explicit_implementation: false,
        members: Vec::new(),
        type_parameters: Vec::new(),
        member_map: HashMap::new(),
    }
}

/// The metadata name of an overloaded operator, unary and binary `+`/`-` told apart
/// by parameter count.
pub fn operator_name(symbol: OperatorSymbol, parameter_count: usize) -> &'static str {
    match symbol {
        OperatorSymbol::Plus if parameter_count == 1 => "op_UnaryPlus",
        OperatorSymbol::Plus => "op_Addition",
        OperatorSymbol::Minus if parameter_count == 1 => "op_UnaryNegation",
        OperatorSymbol::Minus => "op_Subtraction",
        OperatorSymbol::Not => "op_LogicalNot",
        OperatorSymbol::BitwiseNot => "op_OnesComplement",
        OperatorSymbol::Increment => "op_Increment",
        OperatorSymbol::Decrement => "op_Decrement",
        OperatorSymbol::True => "op_True",
        OperatorSymbol::False => "op_False",
        OperatorSymbol::Multiply => "op_Multiply",
        OperatorSymbol::Divide => "op_Division",
        OperatorSymbol::Modulo => "op_Modulus",
        OperatorSymbol::BitwiseAnd => "op_BitwiseAnd",
        OperatorSymbol::BitwiseOr => "op_BitwiseOr",
        OperatorSymbol::BitwiseXor => "op_ExclusiveOr",
        OperatorSymbol::LeftShift => "op_LeftShift",
        OperatorSymbol::RightShift => "op_RightShift",
        OperatorSymbol::UnsignedRightShift => "op_UnsignedRightShift",
        OperatorSymbol::Equal => "op_Equality",
        OperatorSymbol::NotEqual => "op_Inequality",
        OperatorSymbol::LessThan => "op_LessThan",
        OperatorSymbol::GreaterThan => "op_GreaterThan",
        OperatorSymbol::LessThanEqual => "op_LessThanOrEqual",
        OperatorSymbol::GreaterThanEqual => "op_GreaterThanOrEqual",
    }
}

pub fn conversion_operator_name(kind: ConversionKind) -> &'static str {
    match kind {
        ConversionKind::Implicit => "op_Implicit",
        ConversionKind::Explicit => "op_Explicit",
    }
}
