//! Signature resolution: written type references become [`Type`]s.
//!
//! This phase runs after the declaration merge and before any expression is looked
//! at. For every declaration-level `TypeRef` in a file — base types, field types,
//! method returns and parameters, property/event/indexer/operator/delegate
//! signatures, constraint bounds — it resolves the name through C#'s scope rules
//! and records the result in side tables keyed by [`EntityID`] and [`SymbolId`].
//!
//! Lookup order for an unqualified name follows the C# spec in simplified form:
//! type parameters of the enclosing method and types, then nested types of the
//! enclosing types, then each enclosing namespace scope from innermost out — first
//! the namespace's own members (source before referenced assemblies), then that
//! scope's `using` directives (aliases, `using static` types, imported namespaces,
//! which must agree or the name is ambiguous). Nested types inherited from base
//! classes are not searched yet; finding them needs resolved base types, which are
//! this phase's own output.
//!
//! `using` targets resolve before the containing scope's own usings are attached,
//! which gives C#'s rule that using directives do not see their siblings.
//!
//! [`resolve_file`] is one file's pure function — the driver fans files out across
//! threads and merges the returned [`Signatures`] — while [`resolve_signatures`]
//! is the sequential whole-compilation convenience.

use std::collections::HashMap;
use std::ops::Range;

use men_sharp_parser::ast::{
    BaseTypeList, ConstraintBound, EntityID, NameSegment, NameType, ParameterList,
    ParameterModifier, PredefinedType, Spanned, TypeParameterConstraint, TypeRef, TypeRefBase,
    TypeSuffix, UsingDirective,
};

use crate::{
    collect::{DeclarationNode, MemberNode, NamespaceNode, TypeNode},
    error::{SemanticError, SemanticErrorKind},
    external::ExternalTypes,
    merge::Declarations,
    symbol::{FileId, SymbolId, SymbolKind, SyntaxRef},
    types::{
        FunctionSignature, MemberSignature, ParameterPassing, ParameterSignature, TupleElement,
        Type, TypeTarget,
    },
};

/// The output of signature resolution, mergeable across files.
#[derive(Debug, Default)]
pub struct Signatures {
    /// Every resolved `TypeRef` node, keyed by node identity.
    pub type_of: HashMap<EntityID, Type>,
    /// Base types (and enum underlying types) per type symbol, in written order.
    pub base_types: HashMap<SymbolId, Vec<Type>>,
    /// Field/property/event types and callable signatures per member symbol.
    /// Delegates appear here too, keyed by the delegate's type symbol.
    pub members: HashMap<SymbolId, MemberSignature>,
    /// Resolved constraint bounds per type-parameter symbol (`where T : Bound`).
    pub constraints: HashMap<SymbolId, Vec<Type>>,
    pub errors: Vec<SemanticError>,
}

impl Signatures {
    pub fn merge(&mut self, other: Signatures) {
        self.type_of.extend(other.type_of);
        self.base_types.extend(other.base_types);
        self.members.extend(other.members);
        self.constraints.extend(other.constraints);
        self.errors.extend(other.errors);
    }
}

/// Resolves every file sequentially. The parallel driver calls [`resolve_file`]
/// per file instead and merges.
pub fn resolve_signatures(
    declarations: &Declarations<'_>,
    external: &dyn ExternalTypes,
) -> Signatures {
    let mut all = Signatures::default();
    for index in 0..declarations.files.len() {
        all.merge(resolve_file(declarations, external, index));
    }
    all.errors
        .sort_by_key(|error| (error.file, error.span.start, error.span.end));
    all
}

/// Resolves one file's declaration-level types. Pure: reads shared state, returns
/// its own tables.
pub fn resolve_file(
    declarations: &Declarations<'_>,
    external: &dyn ExternalTypes,
    file_index: usize,
) -> Signatures {
    let file = &declarations.files[file_index];

    let mut resolver = Resolver {
        declarations,
        external,
        file: file.file,
        out: Signatures::default(),
    };

    // file scope: every file sees its own usings plus all `global using`s
    let mut scopes = vec![NamespaceScope {
        path: Vec::new(),
        symbol: Some(declarations.table.root()),
        usings: Vec::new(),
    }];

    let mut root_usings = Vec::new();
    for (_, using) in declarations.global_usings() {
        if let Some(resolved) = resolver.resolve_using(using, &scopes) {
            root_usings.push(resolved);
        }
    }
    for using in &file.usings {
        if using.global.is_none()
            && let Some(resolved) = resolver.resolve_using(using, &scopes)
        {
            root_usings.push(resolved);
        }
    }
    scopes[0].usings = root_usings;

    let mut type_stack = Vec::new();
    resolver.walk_nodes(&file.members, &mut scopes, &mut type_stack);

    resolver.out
}

// ---------------------------------------------------------------------------

/// One enclosing namespace scope during the walk.
pub(crate) struct NamespaceScope<'ast> {
    /// Full dotted path from the root.
    pub(crate) path: Vec<&'ast str>,
    /// The source symbol of this namespace, when the table has one.
    pub(crate) symbol: Option<SymbolId>,
    pub(crate) usings: Vec<ResolvedUsing<'ast>>,
}

pub(crate) enum ResolvedUsing<'ast> {
    /// `using A.B;`
    Namespace(Vec<&'ast str>),
    /// `using static T;` — brings T's nested types (and later, members) into scope.
    Static(Resolution<'ast>),
    /// `using X = ...;`
    Alias {
        name: &'ast str,
        target: Resolution<'ast>,
    },
}

/// What a (partial) name has resolved to while walking segments.
#[derive(Clone)]
pub(crate) enum Resolution<'ast> {
    Namespace {
        path: Vec<&'ast str>,
        symbol: Option<SymbolId>,
    },
    Type {
        target: TypeTarget,
        arguments: Vec<Type>,
    },
    TypeParameter(SymbolId),
    /// Already reported; stay quiet downstream.
    Error,
}

pub(crate) struct Resolver<'a, 'ast> {
    pub(crate) declarations: &'a Declarations<'ast>,
    pub(crate) external: &'a dyn ExternalTypes,
    pub(crate) file: FileId,
    pub(crate) out: Signatures,
}

impl<'ast> Resolver<'_, 'ast> {
    pub(crate) fn error(&mut self, kind: SemanticErrorKind, span: Range<usize>) {
        self.out.errors.push(SemanticError {
            kind,
            file: self.file,
            span,
        });
    }

    // ---------------------------------------------------------------- walking

    fn walk_nodes(
        &mut self,
        nodes: &[DeclarationNode<'ast>],
        scopes: &mut Vec<NamespaceScope<'ast>>,
        type_stack: &mut Vec<SymbolId>,
    ) {
        for node in nodes {
            match node {
                DeclarationNode::Namespace(namespace) => {
                    self.walk_namespace(namespace, scopes, type_stack)
                }
                DeclarationNode::Type(type_node) => {
                    self.resolve_type_declaration(type_node, scopes, type_stack)
                }
            }
        }
    }

    fn walk_namespace(
        &mut self,
        node: &NamespaceNode<'ast>,
        scopes: &mut Vec<NamespaceScope<'ast>>,
        type_stack: &mut Vec<SymbolId>,
    ) {
        let pushed = node.name.len();

        for segment in node.name {
            let previous = scopes.last().unwrap();
            let mut path = previous.path.clone();
            path.push(segment.value);

            let symbol = previous
                .symbol
                .and_then(|symbol| self.source_namespace_in(symbol, segment.value));

            scopes.push(NamespaceScope {
                path,
                symbol,
                usings: Vec::new(),
            });
        }

        // the namespace's own usings resolve in its scope (its own list still
        // empty, so siblings stay invisible to each other) and then belong to it
        let mut usings = Vec::new();
        for using in &node.usings {
            if let Some(resolved) = self.resolve_using(using, scopes) {
                usings.push(resolved);
            }
        }
        if let Some(scope) = scopes.last_mut() {
            scope.usings = usings;
        }

        self.walk_nodes(&node.members, scopes, type_stack);

        scopes.truncate(scopes.len() - pushed);
    }

    fn resolve_type_declaration(
        &mut self,
        node: &TypeNode<'ast>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &mut Vec<SymbolId>,
    ) {
        let Some(symbol) = self.declarations.symbol_of(node.syntax.entity_id()) else {
            return;
        };
        type_stack.push(symbol);

        match node.syntax {
            SyntaxRef::Class(class) => {
                let bases = self.resolve_base_list(&class.base_types, scopes, type_stack);
                self.out.base_types.entry(symbol).or_default().extend(bases);
                self.resolve_constraints(symbol, class.constraints, scopes, type_stack);
            }
            SyntaxRef::Enum(declaration) => {
                let bases =
                    self.resolve_base_list(&declaration.underlying_type, scopes, type_stack);
                self.out.base_types.entry(symbol).or_default().extend(bases);
            }
            SyntaxRef::Delegate(declaration) => {
                let return_type = match &declaration.return_type {
                    Ok(type_ref) => self.resolve_type_ref(type_ref, scopes, type_stack),
                    Err(()) => Type::Error,
                };
                let parameters = match &declaration.parameters {
                    Ok(list) => self.resolve_parameters(list, scopes, type_stack),
                    Err(()) => Vec::new(),
                };
                self.out.members.insert(
                    symbol,
                    MemberSignature::Function(FunctionSignature {
                        return_type,
                        parameters,
                    }),
                );
                self.resolve_constraints(symbol, declaration.constraints, scopes, type_stack);
            }
            _ => {}
        }

        for member in &node.members {
            self.resolve_member(member, scopes, type_stack);
        }
        for nested in &node.nested {
            self.resolve_type_declaration(nested, scopes, type_stack);
        }

        type_stack.pop();
    }

    fn resolve_member(
        &mut self,
        node: &MemberNode<'ast>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &mut Vec<SymbolId>,
    ) {
        let Some(symbol) = self.declarations.symbol_of(node.syntax.entity_id()) else {
            return;
        };

        let signature = match node.syntax {
            SyntaxRef::Field { field, .. } => {
                MemberSignature::Field(self.resolve_type_ref(&field.field_type, scopes, type_stack))
            }
            SyntaxRef::Property(property) => MemberSignature::Property(self.resolve_type_ref(
                &property.property_type,
                scopes,
                type_stack,
            )),
            SyntaxRef::Event { event, .. } => {
                MemberSignature::Event(self.resolve_type_ref(&event.event_type, scopes, type_stack))
            }
            SyntaxRef::Method(method) => {
                // the method's own type parameters are in scope inside its signature
                type_stack.push(symbol);
                let return_type = self.resolve_type_ref(&method.return_type, scopes, type_stack);
                let parameters = match &method.parameters {
                    Ok(list) => self.resolve_parameters(list, scopes, type_stack),
                    Err(()) => Vec::new(),
                };
                self.resolve_constraints(symbol, method.constraints, scopes, type_stack);
                type_stack.pop();

                MemberSignature::Function(FunctionSignature {
                    return_type,
                    parameters,
                })
            }
            SyntaxRef::Indexer(indexer) => {
                let return_type = self.resolve_type_ref(&indexer.element_type, scopes, type_stack);
                let parameters = match &indexer.parameters {
                    Ok(list) => self.resolve_parameters(list, scopes, type_stack),
                    Err(()) => Vec::new(),
                };
                MemberSignature::Function(FunctionSignature {
                    return_type,
                    parameters,
                })
            }
            SyntaxRef::Constructor(constructor) => {
                let parameters = match &constructor.parameters {
                    Ok(list) => self.resolve_parameters(list, scopes, type_stack),
                    Err(()) => Vec::new(),
                };
                MemberSignature::Function(FunctionSignature {
                    return_type: Type::Void,
                    parameters,
                })
            }
            SyntaxRef::Destructor(_) => MemberSignature::Function(FunctionSignature {
                return_type: Type::Void,
                parameters: Vec::new(),
            }),
            SyntaxRef::Operator(operator) => {
                let return_type = self.resolve_type_ref(&operator.result_type, scopes, type_stack);
                let parameters = match &operator.parameters {
                    Ok(list) => self.resolve_parameters(list, scopes, type_stack),
                    Err(()) => Vec::new(),
                };
                MemberSignature::Function(FunctionSignature {
                    return_type,
                    parameters,
                })
            }
            // enum members carry their enum's type implicitly
            _ => return,
        };

        self.out.members.insert(symbol, signature);
    }

    fn resolve_base_list(
        &mut self,
        list: &Option<BaseTypeList<'ast, 'ast>>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Vec<Type> {
        list.as_ref()
            .map(|list| {
                list.types
                    .iter()
                    .map(|base| self.resolve_type_ref(base, scopes, type_stack))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn resolve_constraints(
        &mut self,
        owner: SymbolId,
        constraints: &[TypeParameterConstraint<'ast, 'ast>],
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) {
        for constraint in constraints {
            let Ok(target) = &constraint.target else {
                continue;
            };
            let Some(&parameter) = self
                .declarations
                .table
                .symbol(owner)
                .type_parameters
                .iter()
                .find(|&&id| self.declarations.table.symbol(id).name == target.value)
            else {
                continue;
            };

            let mut bounds = Vec::new();
            for bound in constraint.bounds {
                if let ConstraintBound::Type(type_ref) = bound {
                    bounds.push(self.resolve_type_ref(type_ref, scopes, type_stack));
                }
            }
            self.out
                .constraints
                .entry(parameter)
                .or_default()
                .extend(bounds);
        }
    }

    fn resolve_parameters(
        &mut self,
        list: &ParameterList<'ast, 'ast>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Vec<ParameterSignature> {
        list.parameters
            .iter()
            .map(|parameter| {
                let parameter_type = match &parameter.parameter_type {
                    Some(type_ref) => self.resolve_type_ref(type_ref, scopes, type_stack),
                    None => Type::Error,
                };

                let mut passing = ParameterPassing::Value;
                let mut is_params = false;
                for modifier in parameter.modifiers {
                    match modifier.value {
                        ParameterModifier::Ref => passing = ParameterPassing::Ref,
                        ParameterModifier::Out => passing = ParameterPassing::Out,
                        ParameterModifier::In => passing = ParameterPassing::In,
                        ParameterModifier::Params => is_params = true,
                        _ => {}
                    }
                }

                ParameterSignature {
                    passing,
                    is_params,
                    parameter_type,
                }
            })
            .collect()
    }

    // ----------------------------------------------------------- type shapes

    /// The heart of the phase: one written `TypeRef` to one [`Type`], recorded in
    /// the `type_of` side table.
    pub(crate) fn resolve_type_ref(
        &mut self,
        node: &TypeRef<'ast, 'ast>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Type {
        let base = match &node.base {
            TypeRefBase::Predefined(predefined) => self.resolve_predefined(predefined),
            TypeRefBase::Var(_) => Type::Infer,
            TypeRefBase::Ref {
                readonly_keyword,
                element,
                ..
            } => Type::ByRef {
                readonly: readonly_keyword.is_some(),
                element: Box::new(self.resolve_type_ref(element, scopes, type_stack)),
            },
            TypeRefBase::Tuple(tuple) => Type::Tuple(
                tuple
                    .elements
                    .iter()
                    .map(|element| TupleElement {
                        name: element.name.as_ref().map(|name| name.value.into()),
                        element: self.resolve_type_ref(&element.element_type, scopes, type_stack),
                    })
                    .collect(),
            ),
            TypeRefBase::Name(name) => self.resolve_name_type(name, scopes, type_stack),
        };

        let resolved = apply_suffixes(base, node.suffixes);
        self.out
            .type_of
            .insert(EntityID::from(node), resolved.clone());
        resolved
    }

    pub(crate) fn resolve_predefined(&mut self, predefined: &Spanned<PredefinedType>) -> Type {
        let name = match predefined.value {
            PredefinedType::Void => return Type::Void,
            PredefinedType::Dynamic => return Type::Dynamic,
            PredefinedType::Bool => "Boolean",
            PredefinedType::Byte => "Byte",
            PredefinedType::Sbyte => "SByte",
            PredefinedType::Short => "Int16",
            PredefinedType::Ushort => "UInt16",
            PredefinedType::Int => "Int32",
            PredefinedType::Uint => "UInt32",
            PredefinedType::Long => "Int64",
            PredefinedType::Ulong => "UInt64",
            PredefinedType::Char => "Char",
            PredefinedType::Float => "Single",
            PredefinedType::Double => "Double",
            PredefinedType::Decimal => "Decimal",
            PredefinedType::String => "String",
            PredefinedType::Object => "Object",
            PredefinedType::Nint => "IntPtr",
            PredefinedType::Nuint => "UIntPtr",
        };

        match self.external.find_type(&["System"], name, 0) {
            Some(id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => {
                // no core library was referenced; every predefined type is missing
                self.error(
                    SemanticErrorKind::UnresolvedTypeName,
                    predefined.span.clone(),
                );
                Type::Error
            }
        }
    }

    pub(crate) fn resolve_name_type(
        &mut self,
        name: &NameType<'ast, 'ast>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Type {
        let mut arguments = Vec::new();
        let mut segments = name.segments.iter();

        let mut current = if name.global.is_some() {
            Resolution::Namespace {
                path: Vec::new(),
                symbol: Some(self.declarations.table.root()),
            }
        } else {
            let Some(first) = segments.next() else {
                return Type::Error;
            };
            self.resolve_arguments(first, &mut arguments, scopes, type_stack);
            self.lookup_unqualified(
                first.name.value,
                segment_arity(first),
                &first.span,
                scopes,
                type_stack,
            )
        };

        for segment in segments {
            self.resolve_arguments(segment, &mut arguments, scopes, type_stack);
            current = self.lookup_member(
                current,
                segment.name.value,
                segment_arity(segment),
                &segment.span,
            );
        }

        match current {
            // an alias target may carry its own arguments (`using L = List<int>`);
            // they precede any written after it
            Resolution::Type {
                target,
                arguments: mut alias_arguments,
            } => {
                alias_arguments.extend(arguments);
                Type::Named {
                    target,
                    arguments: alias_arguments,
                }
            }
            Resolution::TypeParameter(symbol) => Type::TypeParameter(symbol),
            Resolution::Namespace { .. } => {
                self.error(SemanticErrorKind::NamespaceUsedAsType, name.span.clone());
                Type::Error
            }
            Resolution::Error => Type::Error,
        }
    }

    fn resolve_arguments(
        &mut self,
        segment: &NameSegment<'ast, 'ast>,
        arguments: &mut Vec<Type>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) {
        if let Some(generics) = &segment.generics {
            for argument in generics.types {
                arguments.push(self.resolve_type_ref(argument, scopes, type_stack));
            }
        }
    }

    // ---------------------------------------------------------------- lookup

    pub(crate) fn lookup_unqualified(
        &mut self,
        name: &'ast str,
        arity: u32,
        span: &Range<usize>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Resolution<'ast> {
        match self.try_lookup_unqualified(name, arity, span, scopes, type_stack) {
            Some(resolution) => resolution,
            None => {
                self.error(SemanticErrorKind::UnresolvedTypeName, span.clone());
                Resolution::Error
            }
        }
    }

    /// [`Self::lookup_unqualified`] without the not-found diagnostic, for callers
    /// (the body checker) that have a better message to give.
    pub(crate) fn try_lookup_unqualified(
        &mut self,
        name: &'ast str,
        arity: u32,
        span: &Range<usize>,
        scopes: &[NamespaceScope<'ast>],
        type_stack: &[SymbolId],
    ) -> Option<Resolution<'ast>> {
        // 1. type parameters of the enclosing method and types, innermost first
        if arity == 0 {
            for &owner in type_stack.iter().rev() {
                for &parameter in &self.declarations.table.symbol(owner).type_parameters {
                    if self.declarations.table.symbol(parameter).name == name {
                        return Some(Resolution::TypeParameter(parameter));
                    }
                }
            }
        }

        // 2. nested types of the enclosing types, innermost first
        for &owner in type_stack.iter().rev() {
            if let Some(id) = self.source_type_in(owner, name, arity) {
                return Some(Resolution::Type {
                    target: TypeTarget::Source(id),
                    arguments: Vec::new(),
                });
            }
        }

        // 3. namespace scopes, innermost first: own members, then this scope's usings
        for scope in scopes.iter().rev() {
            if let Some(symbol) = scope.symbol {
                if let Some(id) = self.source_type_in(symbol, name, arity) {
                    return Some(Resolution::Type {
                        target: TypeTarget::Source(id),
                        arguments: Vec::new(),
                    });
                }
                if arity == 0
                    && let Some(child) = self.source_namespace_in(symbol, name)
                {
                    let mut path = scope.path.clone();
                    path.push(name);
                    return Some(Resolution::Namespace {
                        path,
                        symbol: Some(child),
                    });
                }
            }

            if let Some(id) = self.external.find_type(&scope.path, name, arity) {
                return Some(Resolution::Type {
                    target: TypeTarget::External(id),
                    arguments: Vec::new(),
                });
            }
            if arity == 0 {
                let mut path = scope.path.clone();
                path.push(name);
                if self.external.namespace_exists(&path) {
                    return Some(Resolution::Namespace { path, symbol: None });
                }
            }

            if let Some(resolution) = self.lookup_in_usings(scope, name, arity, span) {
                return Some(resolution);
            }
        }

        None
    }

    /// This scope's `using` directives: aliases, `using static` nested types, and
    /// imported namespaces (which must agree on what the name means).
    fn lookup_in_usings(
        &mut self,
        scope: &NamespaceScope<'ast>,
        name: &'ast str,
        arity: u32,
        span: &Range<usize>,
    ) -> Option<Resolution<'ast>> {
        if arity == 0 {
            for using in &scope.usings {
                if let ResolvedUsing::Alias {
                    name: alias,
                    target,
                } = using
                    && *alias == name
                {
                    return Some(target.clone());
                }
            }
        }

        let mut found: Option<Resolution<'ast>> = None;
        let mut ambiguous = false;
        let mut offer = |candidate: Resolution<'ast>| match (&found, &candidate) {
            (None, _) => found = Some(candidate),
            (
                Some(Resolution::Type { target: first, .. }),
                Resolution::Type { target: second, .. },
            ) if first == second => {}
            _ => ambiguous = true,
        };

        for using in &scope.usings {
            match using {
                ResolvedUsing::Namespace(path) => {
                    // a source declaration and a referenced type with the same
                    // fully-qualified name are not ambiguous: the source one
                    // wins, as in C# (CS0436) — that is also how the shipped
                    // mini-corlib shadows types Udon does not expose
                    let source = self
                        .source_namespace_at(path)
                        .and_then(|symbol| self.source_type_in(symbol, name, arity));
                    if let Some(id) = source {
                        offer(Resolution::Type {
                            target: TypeTarget::Source(id),
                            arguments: Vec::new(),
                        });
                    } else if let Some(id) = self.external.find_type(path, name, arity) {
                        offer(Resolution::Type {
                            target: TypeTarget::External(id),
                            arguments: Vec::new(),
                        });
                    }
                }
                ResolvedUsing::Static(target) => {
                    if let Some(nested) = self.nested_type_of(target, name, arity) {
                        offer(nested);
                    }
                }
                ResolvedUsing::Alias { .. } => {}
            }
        }

        if ambiguous {
            self.error(SemanticErrorKind::AmbiguousTypeName, span.clone());
            return Some(Resolution::Error);
        }
        found
    }

    pub(crate) fn lookup_member(
        &mut self,
        current: Resolution<'ast>,
        name: &'ast str,
        arity: u32,
        span: &Range<usize>,
    ) -> Resolution<'ast> {
        match current {
            Resolution::Namespace { path, symbol } => {
                if let Some(symbol) = symbol {
                    if let Some(id) = self.source_type_in(symbol, name, arity) {
                        return Resolution::Type {
                            target: TypeTarget::Source(id),
                            arguments: Vec::new(),
                        };
                    }
                    if arity == 0
                        && let Some(child) = self.source_namespace_in(symbol, name)
                    {
                        let mut path = path;
                        path.push(name);
                        return Resolution::Namespace {
                            path,
                            symbol: Some(child),
                        };
                    }
                }

                if let Some(id) = self.external.find_type(&path, name, arity) {
                    return Resolution::Type {
                        target: TypeTarget::External(id),
                        arguments: Vec::new(),
                    };
                }
                if arity == 0 {
                    let mut path = path;
                    path.push(name);
                    if self.external.namespace_exists(&path) {
                        return Resolution::Namespace { path, symbol: None };
                    }
                }

                self.error(SemanticErrorKind::UnresolvedTypeName, span.clone());
                Resolution::Error
            }
            Resolution::Type { .. } => match self.nested_type_of(&current, name, arity) {
                Some(resolution) => resolution,
                None => {
                    self.error(SemanticErrorKind::UnresolvedTypeName, span.clone());
                    Resolution::Error
                }
            },
            Resolution::TypeParameter(_) => {
                self.error(SemanticErrorKind::UnresolvedTypeName, span.clone());
                Resolution::Error
            }
            Resolution::Error => Resolution::Error,
        }
    }

    /// A type nested in a resolved type.
    fn nested_type_of(
        &self,
        parent: &Resolution<'ast>,
        name: &str,
        arity: u32,
    ) -> Option<Resolution<'ast>> {
        match parent {
            Resolution::Type {
                target: TypeTarget::Source(symbol),
                arguments,
            } => self
                .source_type_in(*symbol, name, arity)
                .map(|id| Resolution::Type {
                    target: TypeTarget::Source(id),
                    // outer arguments stay with the nested type (`Outer<int>.Inner`)
                    arguments: arguments.clone(),
                }),
            Resolution::Type {
                target: TypeTarget::External(id),
                arguments,
            } => self
                .external
                .find_nested_type(*id, name, arity)
                .map(|nested| Resolution::Type {
                    target: TypeTarget::External(nested),
                    arguments: arguments.clone(),
                }),
            _ => None,
        }
    }

    /// A type member of a source container (namespace or type) by name and arity.
    fn source_type_in(&self, container: SymbolId, name: &str, arity: u32) -> Option<SymbolId> {
        self.declarations
            .table
            .symbol(container)
            .members_named(name)
            .iter()
            .copied()
            .find(|&id| {
                let symbol = self.declarations.table.symbol(id);
                symbol.kind.is_type() && symbol.arity == arity
            })
    }

    fn source_namespace_in(&self, container: SymbolId, name: &str) -> Option<SymbolId> {
        self.declarations
            .table
            .symbol(container)
            .members_named(name)
            .iter()
            .copied()
            .find(|&id| self.declarations.table.symbol(id).kind == SymbolKind::Namespace)
    }

    /// Walks the source namespace tree along a path.
    pub(crate) fn source_namespace_at(&self, path: &[&str]) -> Option<SymbolId> {
        let mut current = self.declarations.table.root();
        for segment in path {
            current = self.source_namespace_in(current, segment)?;
        }
        Some(current)
    }

    // ---------------------------------------------------------------- usings

    /// Resolves one `using` directive against the current scopes. `None` means the
    /// directive was broken and already reported (here or by the parser).
    pub(crate) fn resolve_using(
        &mut self,
        using: &UsingDirective<'ast, 'ast>,
        scopes: &[NamespaceScope<'ast>],
    ) -> Option<ResolvedUsing<'ast>> {
        let Ok(target) = &using.target else {
            return None;
        };

        let TypeRefBase::Name(name) = &target.base else {
            self.error(
                SemanticErrorKind::UnresolvedUsingTarget,
                target.span.clone(),
            );
            return None;
        };
        let path: Vec<&'ast str> = name
            .segments
            .iter()
            .map(|segment| segment.name.value)
            .collect();
        let is_plain = name.global.is_none()
            && target.suffixes.is_empty()
            && name
                .segments
                .iter()
                .all(|segment| segment.generics.is_none());

        if using.static_keyword.is_none() && using.alias.is_none() {
            // `using A.B.C;` — must be a namespace
            let exists = is_plain
                && (self.source_namespace_at(&path).is_some()
                    || self.external.namespace_exists(&path));
            if !exists {
                self.error(
                    SemanticErrorKind::UnresolvedUsingTarget,
                    target.span.clone(),
                );
                return None;
            }
            return Some(ResolvedUsing::Namespace(path));
        }

        // `using X = ...;` may alias a namespace; `using static T;` never does
        let resolution = if using.alias.is_some() && is_plain {
            if let Some(symbol) = self.source_namespace_at(&path) {
                Some(Resolution::Namespace {
                    path,
                    symbol: Some(symbol),
                })
            } else if self.external.namespace_exists(&path) {
                Some(Resolution::Namespace { path, symbol: None })
            } else {
                None
            }
        } else {
            None
        };

        let resolution = match resolution {
            Some(resolution) => resolution,
            None => match self.resolve_type_ref(target, scopes, &[]) {
                Type::Named { target, arguments } => Resolution::Type { target, arguments },
                Type::Error => return None,
                _ => {
                    self.error(
                        SemanticErrorKind::UnresolvedUsingTarget,
                        target.span.clone(),
                    );
                    return None;
                }
            },
        };

        Some(match (&using.alias, &using.static_keyword) {
            (Some(alias), _) => ResolvedUsing::Alias {
                name: alias.value,
                target: resolution,
            },
            (None, Some(_)) => ResolvedUsing::Static(resolution),
            (None, None) => unreachable!("handled above"),
        })
    }
}

fn segment_arity(segment: &NameSegment) -> u32 {
    segment
        .generics
        .as_ref()
        .map(|generics| {
            if generics.is_unbound() {
                generics.arity as u32
            } else {
                generics.types.len() as u32
            }
        })
        .unwrap_or(0)
}

/// Applies `?`, `[]` and `*` in C#'s reading order: `?` and `*` wrap what precedes
/// them; a run of rank specifiers reads outermost-first (`int[][,]` is a `[]` array
/// of `[,]` arrays).
pub(crate) fn apply_suffixes(base: Type, suffixes: &[TypeSuffix]) -> Type {
    let mut current = base;
    let mut index = 0;

    while index < suffixes.len() {
        match &suffixes[index] {
            TypeSuffix::Nullable { .. } => {
                current = Type::Nullable(Box::new(current));
                index += 1;
            }
            TypeSuffix::Pointer { .. } => {
                current = Type::Pointer(Box::new(current));
                index += 1;
            }
            TypeSuffix::Array { .. } => {
                let run_start = index;
                while index < suffixes.len() && matches!(suffixes[index], TypeSuffix::Array { .. })
                {
                    index += 1;
                }
                for suffix in suffixes[run_start..index].iter().rev() {
                    if let TypeSuffix::Array { rank, .. } = suffix {
                        current = Type::Array {
                            element: Box::new(current),
                            rank: *rank as u32,
                        };
                    }
                }
            }
        }
    }

    current
}
