//! Per-file declaration collection.
//!
//! [`collect_file`] walks one frozen syntax tree and lists what it declares:
//! namespaces, types, members and the `using` directives that will drive name
//! resolution. It touches no shared state, so the compiler driver runs it on every
//! file in parallel; the sequential merge into one [`crate::symbol::SymbolTable`]
//! happens afterwards in [`crate::merge`].
//!
//! Only declaration *shapes* are read (names, arities, modifiers). Written types —
//! return types, parameter types, base types — stay in the syntax tree, reachable
//! through each node's [`SyntaxRef`], because resolving them needs the merged table
//! to exist first.
//!
//! Parse holes are skipped quietly: a type whose name is missing has already produced
//! a parse error, and inventing a placeholder symbol would only cascade.

use std::ops::Range;

use men_sharp_parser::ast::{
    ClassDeclaration, CompilationUnit, DelegateDeclaration, EnumDeclaration, EventDeclaration,
    ExternAliasDirective, FieldDeclaration, GenericsDefine, GenericsParameter, Ident, Modifier,
    NamespaceDeclaration, NamespaceMember, ParameterModifier, Spanned, TypeDeclaration, TypeMember,
    UsingDirective,
};

use crate::symbol::{
    Accessibility, FileId, SymbolKind, SyntaxRef, conversion_operator_name, operator_name,
};

/// Everything one file declares, in source order.
#[derive(Debug)]
pub struct FileDeclarations<'ast> {
    pub file: FileId,
    /// A foreign (UdonSharp) file: merged after every other, and a type it
    /// declares under a name the user's code already declares is dropped
    /// without a word — the user's wins, as a library's would.
    pub foreign: bool,
    /// `using` directives at file scope, `global using` included.
    pub usings: Vec<&'ast UsingDirective<'ast, 'ast>>,
    pub extern_aliases: Vec<&'ast ExternAliasDirective<'ast>>,
    pub members: Vec<DeclarationNode<'ast>>,
}

#[derive(Debug)]
pub enum DeclarationNode<'ast> {
    Namespace(NamespaceNode<'ast>),
    Type(TypeNode<'ast>),
}

#[derive(Debug)]
pub struct NamespaceNode<'ast> {
    pub syntax: &'ast NamespaceDeclaration<'ast, 'ast>,
    /// `namespace A.B.C` — one identifier per dotted segment; empty when the name
    /// was a parse hole, in which case the members spill into the enclosing scope.
    pub name: &'ast [Ident<'ast>],
    pub usings: Vec<&'ast UsingDirective<'ast, 'ast>>,
    pub members: Vec<DeclarationNode<'ast>>,
}

#[derive(Debug)]
pub struct TypeNode<'ast> {
    pub syntax: SyntaxRef<'ast>,
    pub kind: SymbolKind,
    pub name: &'ast str,
    /// The name's span — where diagnostics about this declaration point.
    pub span: Range<usize>,
    pub arity: u32,
    pub is_partial: bool,
    pub is_static: bool,
    /// Written accessibility; `None` lets the merge pick the container's default.
    pub accessibility: Option<Accessibility>,
    pub type_parameters: Vec<&'ast GenericsParameter<'ast, 'ast>>,
    pub members: Vec<MemberNode<'ast>>,
    pub nested: Vec<TypeNode<'ast>>,
}

#[derive(Debug)]
pub struct MemberNode<'ast> {
    pub syntax: SyntaxRef<'ast>,
    pub kind: SymbolKind,
    pub name: &'ast str,
    pub span: Range<usize>,
    pub arity: u32,
    pub type_parameters: Vec<&'ast GenericsParameter<'ast, 'ast>>,
    pub is_partial: bool,
    pub is_explicit_implementation: bool,
    pub is_static: bool,
    pub accessibility: Option<Accessibility>,
    /// A method whose first parameter carries the `this` modifier.
    pub is_extension: bool,
}

pub fn collect_file<'ast>(
    file: FileId,
    unit: &'ast CompilationUnit<'ast, 'ast>,
) -> FileDeclarations<'ast> {
    FileDeclarations {
        file,
        foreign: false,
        usings: unit.usings.iter().collect(),
        extern_aliases: unit.extern_aliases.iter().collect(),
        members: collect_namespace_members(unit.members),
    }
}

fn collect_namespace_members<'ast>(
    members: &'ast [NamespaceMember<'ast, 'ast>],
) -> Vec<DeclarationNode<'ast>> {
    let mut nodes = Vec::with_capacity(members.len());

    for member in members {
        match member {
            NamespaceMember::Namespace(namespace) => {
                nodes.push(DeclarationNode::Namespace(NamespaceNode {
                    syntax: namespace,
                    name: namespace.name.unwrap_or(&[]),
                    usings: namespace.usings.iter().collect(),
                    members: collect_namespace_members(namespace.members),
                }));
            }
            NamespaceMember::Type(declaration) => {
                if let Some(node) = collect_type(declaration) {
                    nodes.push(DeclarationNode::Type(node));
                }
            }
        }
    }

    nodes
}

fn collect_type<'ast>(declaration: &'ast TypeDeclaration<'ast, 'ast>) -> Option<TypeNode<'ast>> {
    match declaration {
        TypeDeclaration::Class(class) => collect_class(class),
        TypeDeclaration::Enum(declaration) => collect_enum(declaration),
        TypeDeclaration::Delegate(declaration) => collect_delegate(declaration),
    }
}

fn collect_class<'ast>(class: &'ast ClassDeclaration<'ast, 'ast>) -> Option<TypeNode<'ast>> {
    let name = class.name.as_ref().ok()?;

    let mut members = Vec::new();
    let mut nested = Vec::new();
    for member in class.members.unwrap_or(&[]) {
        collect_type_member(member, &mut members, &mut nested);
    }

    Some(TypeNode {
        syntax: SyntaxRef::Class(class),
        kind: class.kind.value.into(),
        name: name.value,
        span: name.span.clone(),
        arity: arity_of(&class.generics),
        is_partial: is_partial(class.modifiers),
        is_static: is_static(class.modifiers),
        accessibility: accessibility_of(class.modifiers),
        type_parameters: type_parameters_of(&class.generics),
        members,
        nested,
    })
}

fn collect_enum<'ast>(declaration: &'ast EnumDeclaration<'ast, 'ast>) -> Option<TypeNode<'ast>> {
    let name = declaration.name.as_ref().ok()?;

    let members = declaration
        .members
        .unwrap_or(&[])
        .iter()
        .map(|member| MemberNode {
            syntax: SyntaxRef::EnumMember(member),
            kind: SymbolKind::EnumMember,
            name: member.name.value,
            span: member.name.span.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: false,
            // an enum member is a public constant of its enum
            is_static: true,
            accessibility: None,
            is_extension: false,
        })
        .collect();

    Some(TypeNode {
        syntax: SyntaxRef::Enum(declaration),
        kind: SymbolKind::Enum,
        name: name.value,
        span: name.span.clone(),
        arity: 0,
        is_partial: false,
        is_static: false,
        accessibility: accessibility_of(declaration.modifiers),
        type_parameters: Vec::new(),
        members,
        nested: Vec::new(),
    })
}

fn collect_delegate<'ast>(
    declaration: &'ast DelegateDeclaration<'ast, 'ast>,
) -> Option<TypeNode<'ast>> {
    let name = declaration.name.as_ref().ok()?;

    Some(TypeNode {
        syntax: SyntaxRef::Delegate(declaration),
        kind: SymbolKind::Delegate,
        name: name.value,
        span: name.span.clone(),
        arity: arity_of(&declaration.generics),
        is_partial: false,
        is_static: false,
        accessibility: accessibility_of(declaration.modifiers),
        type_parameters: type_parameters_of(&declaration.generics),
        members: Vec::new(),
        nested: Vec::new(),
    })
}

fn collect_type_member<'ast>(
    member: &'ast TypeMember<'ast, 'ast>,
    members: &mut Vec<MemberNode<'ast>>,
    nested: &mut Vec<TypeNode<'ast>>,
) {
    match member {
        TypeMember::Field(field) => collect_field(field, members),
        TypeMember::Method(method) => members.push(MemberNode {
            syntax: SyntaxRef::Method(method),
            kind: SymbolKind::Method,
            name: method.name.value,
            span: method.name.span.clone(),
            arity: arity_of(&method.generics),
            type_parameters: type_parameters_of(&method.generics),
            is_partial: is_partial(method.modifiers),
            is_explicit_implementation: method.explicit_interface.is_some(),
            is_static: is_static(method.modifiers),
            accessibility: accessibility_of(method.modifiers),
            is_extension: method
                .parameters
                .as_ref()
                .ok()
                .and_then(|list| list.parameters.first())
                .map(|parameter| {
                    parameter
                        .modifiers
                        .iter()
                        .any(|modifier| modifier.value == ParameterModifier::This)
                })
                .unwrap_or(false),
        }),
        TypeMember::Property(property) => members.push(MemberNode {
            syntax: SyntaxRef::Property(property),
            kind: SymbolKind::Property,
            name: property.name.value,
            span: property.name.span.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: is_partial(property.modifiers),
            is_explicit_implementation: property.explicit_interface.is_some(),
            is_static: is_static(property.modifiers),
            accessibility: accessibility_of(property.modifiers),
            is_extension: false,
        }),
        TypeMember::Indexer(indexer) => members.push(MemberNode {
            syntax: SyntaxRef::Indexer(indexer),
            kind: SymbolKind::Indexer,
            name: "this[]",
            span: indexer.this_keyword.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: indexer.explicit_interface.is_some(),
            is_static: false,
            accessibility: accessibility_of(indexer.modifiers),
            is_extension: false,
        }),
        TypeMember::Event(event) => collect_event(event, members),
        TypeMember::Constructor(constructor) => members.push(MemberNode {
            syntax: SyntaxRef::Constructor(constructor),
            kind: SymbolKind::Constructor,
            name: ".ctor",
            span: constructor.name.span.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: false,
            is_static: is_static(constructor.modifiers),
            accessibility: accessibility_of(constructor.modifiers),
            is_extension: false,
        }),
        TypeMember::Destructor(destructor) => members.push(MemberNode {
            syntax: SyntaxRef::Destructor(destructor),
            kind: SymbolKind::Destructor,
            name: "Finalize",
            span: destructor.tilde.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: false,
            is_static: false,
            accessibility: None,
            is_extension: false,
        }),
        TypeMember::Operator(operator) => {
            let parameter_count = operator
                .parameters
                .as_ref()
                .map(|list| list.parameters.len())
                .unwrap_or(2);

            let name = match &operator.conversion {
                Some(conversion) => conversion_operator_name(conversion.value),
                None => match &operator.symbol {
                    Some(symbol) => operator_name(symbol.value, parameter_count),
                    // the parser reported the missing symbol already
                    None => return,
                },
            };

            members.push(MemberNode {
                syntax: SyntaxRef::Operator(operator),
                kind: SymbolKind::Operator,
                name,
                span: operator.operator_keyword.clone(),
                arity: 0,
                type_parameters: Vec::new(),
                is_partial: false,
                is_explicit_implementation: false,
                // overloaded operators are always static in C#
                is_static: true,
                accessibility: accessibility_of(operator.modifiers),
                is_extension: false,
            });
        }
        TypeMember::NestedType(declaration) => {
            if let Some(node) = collect_type(declaration) {
                nested.push(node);
            }
        }
    }
}

fn collect_field<'ast>(
    field: &'ast FieldDeclaration<'ast, 'ast>,
    members: &mut Vec<MemberNode<'ast>>,
) {
    for declarator in field.declarators {
        members.push(MemberNode {
            syntax: SyntaxRef::Field { field, declarator },
            kind: SymbolKind::Field,
            name: declarator.name.value,
            span: declarator.name.span.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: false,
            // a `const` field is implicitly static
            is_static: is_static(field.modifiers)
                || field
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.value == Modifier::Const),
            accessibility: accessibility_of(field.modifiers),
            is_extension: false,
        });
    }
}

fn collect_event<'ast>(
    event: &'ast EventDeclaration<'ast, 'ast>,
    members: &mut Vec<MemberNode<'ast>>,
) {
    for declarator in event.declarators {
        members.push(MemberNode {
            syntax: SyntaxRef::Event { event, declarator },
            kind: SymbolKind::Event,
            name: declarator.name.value,
            span: declarator.name.span.clone(),
            arity: 0,
            type_parameters: Vec::new(),
            is_partial: false,
            is_explicit_implementation: event.explicit_interface.is_some(),
            is_static: is_static(event.modifiers),
            accessibility: accessibility_of(event.modifiers),
            is_extension: false,
        });
    }
}

fn arity_of(generics: &Option<GenericsDefine>) -> u32 {
    generics
        .as_ref()
        .map(|define| define.parameters.len() as u32)
        .unwrap_or(0)
}

fn type_parameters_of<'ast>(
    generics: &'ast Option<GenericsDefine<'ast, 'ast>>,
) -> Vec<&'ast GenericsParameter<'ast, 'ast>> {
    generics
        .as_ref()
        .map(|define| define.parameters.iter().collect())
        .unwrap_or_default()
}

fn is_partial(modifiers: &[Spanned<Modifier>]) -> bool {
    modifiers
        .iter()
        .any(|modifier| modifier.value == Modifier::Partial)
}

fn is_static(modifiers: &[Spanned<Modifier>]) -> bool {
    modifiers
        .iter()
        .any(|modifier| modifier.value == Modifier::Static)
}

/// The written accessibility, combining the two-keyword forms.
fn accessibility_of(modifiers: &[Spanned<Modifier>]) -> Option<Accessibility> {
    let has = |wanted: Modifier| modifiers.iter().any(|modifier| modifier.value == wanted);

    if has(Modifier::Public) {
        Some(Accessibility::Public)
    } else if has(Modifier::Protected) && has(Modifier::Internal) {
        Some(Accessibility::ProtectedInternal)
    } else if has(Modifier::Protected) && has(Modifier::Private) {
        Some(Accessibility::PrivateProtected)
    } else if has(Modifier::Protected) {
        Some(Accessibility::Protected)
    } else if has(Modifier::Internal) {
        Some(Accessibility::Internal)
    } else if has(Modifier::File) {
        Some(Accessibility::File)
    } else if has(Modifier::Private) {
        Some(Accessibility::Private)
    } else {
        None
    }
}
