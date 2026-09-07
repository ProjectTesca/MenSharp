//! What a type declares, checked without looking at a body: the walk
//! itself, contracts, constructor chains and paired operators.

use super::{
    AttemptOutcome, CallArgument, Checker, ConstructorChain, ConstructorChainKind, MethodGroup,
    Scope,
};
use crate::error::SemanticErrorKind;
use crate::semantics::collect::{DeclarationNode, MemberNode, NamespaceNode, TypeNode};
use crate::semantics::resolve::NamespaceScope;
use crate::symbol::{SymbolId, SymbolKind, SyntaxRef};
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{FunctionSignature, MemberSignature, Type, TypeTarget};
use men_sharp_parser::ast::{
    ConstructorInitializerKind, EntityID, Expression, FunctionBody, InitializerValue, Modifier,
    PrimaryLeft, PrimaryRight,
};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn walk_nodes(&mut self, nodes: &[DeclarationNode<'ast>]) {
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
            self.check_union_attribute(symbol);
            // `class C(int x)` — a C# 12 primary constructor on a class or
            // struct: its parameters would be captured as hidden fields, which
            // is not modelled; a record's is desugared by the parser
            if let Some(parameters) = &declaration.primary_constructor
                && !matches!(
                    declaration.kind.value,
                    men_sharp_parser::ast::ClassKind::Record
                        | men_sharp_parser::ast::ClassKind::RecordStruct
                )
            {
                self.error(
                    SemanticErrorKind::UnsupportedExpression,
                    parameters.span.clone(),
                );
            }
        }
        for nested in &node.nested {
            self.check_type_declaration(nested);
        }

        self.type_stack.pop();
    }

    // ------------------------------------------------------- exceptions

    /// `throw` takes a `System.Exception` — the mini-corlib's, which is what
    /// the name resolves to (CS0155 otherwise).
    pub(super) fn require_exception(&mut self, ty: &Type, span: Range<usize>) {
        if matches!(ty, Type::Error | Type::Null) {
            return;
        }
        let Some(exception) = self.lookup_type_path(&["System", "Exception"]) else {
            return;
        };
        if !self.system().is_implicitly_convertible(ty, &exception) {
            let kind = SemanticErrorKind::ThrowNeedsException {
                type_name: self.describe(ty),
            };
            self.error(kind, span);
        }
    }

    /// A source type by namespace path, as a type.
    fn lookup_type_path(&self, path: &[&str]) -> Option<Type> {
        let table = &self.resolver.declarations.table;
        let mut current = table.root();
        for segment in path {
            current = *table.symbol(current).members_named(segment).first()?;
        }
        if !table.symbol(current).kind.is_type() {
            return None;
        }
        Some(Type::Named {
            target: TypeTarget::Source(current),
            arguments: Vec::new(),
        })
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
        let self_type = self.open_type(class);
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
                    type_name: self.describe(&target_type),
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

        let receiver_display = self.describe(&target_type);
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
                    type_name: self.describe(&target_type),
                };
                self.error(kind, span.clone());
                None
            }
        }
    }

    // ------------------------------------------------- abstract / interface

    /// Is this source type declared `abstract`?
    pub(super) fn is_abstract_type(&self, symbol: SymbolId) -> bool {
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
        // an interface member with a body is a default implementation (C# 8):
        // no contract for the implementing type to satisfy
        let without_body = !entry.declarations.iter().any(|site| site.syntax.has_body());
        (in_interface && without_body)
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
        let self_type = self.open_type(symbol);

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
            if current != self_type {
                for &member in &current_entry.members {
                    let member_entry = self.resolver.declarations.table.symbol(member);
                    let contract_member = matches!(
                        member_entry.kind,
                        SymbolKind::Method | SymbolKind::Property | SymbolKind::Indexer
                    ) && !member_entry.is_static
                        && !member_entry.is_explicit_implementation
                        && self.is_bodiless_member(member);
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
                self.implementation_of(&self_type, &contract, member, name, kind, wanted.as_ref());
            if !implemented {
                let kind = SemanticErrorKind::MissingImplementation {
                    type_name: self.describe(&self_type),
                    member: format!("{}.{}", self.describe(&contract), name),
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
        contract_member: SymbolId,
        name: &str,
        kind: SymbolKind,
        wanted: Option<&MemberSignature>,
    ) -> bool {
        let system = self.system();
        // a generic method's type parameters are symbols of its own: the
        // contract's `Visit<TField>(ref TField)` and an implementation's
        // `Visit<F>(ref F)` compare equal once `F` is read as `TField`
        let contract_parameters = self
            .resolver
            .declarations
            .table
            .symbol(contract_member)
            .type_parameters
            .clone();
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
                (Some(wanted), Some(found)) => {
                    wanted == found
                        || (candidate.arity as usize == contract_parameters.len()
                            && candidate.arity > 0
                            && matches!(candidate.origin, MemberOrigin::Source(id)
                                if self.renamed_type_parameters(found, id, &contract_parameters)
                                    == *wanted))
                }
                (None, _) => true,
                _ => false,
            };
            if !fits {
                return false;
            }
            match candidate.origin {
                MemberOrigin::Source(id) => !self.is_bodiless_member(id),
                MemberOrigin::External { .. } | MemberOrigin::LocalFunction(_) => true,
            }
        })
    }

    /// `signature` with the method's own type parameters read as
    /// `parameters`, position by position.
    fn renamed_type_parameters(
        &self,
        signature: &MemberSignature,
        method: SymbolId,
        parameters: &[SymbolId],
    ) -> MemberSignature {
        let own = &self
            .resolver
            .declarations
            .table
            .symbol(method)
            .type_parameters;
        signature.map(&|ty| match ty {
            Type::TypeParameter(parameter) => own
                .iter()
                .position(|candidate| *candidate == parameter)
                .and_then(|position| parameters.get(position))
                .map(|renamed| Type::TypeParameter(*renamed))
                .unwrap_or(Type::TypeParameter(parameter)),
            other => other,
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
        let saved_member = self.current_member.replace(symbol);
        self.check_member_inner(node, symbol);
        self.current_member = saved_member;
    }

    fn check_member_inner(&mut self, node: &MemberNode<'ast>, symbol: SymbolId) {
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
                let is_async = Self::has_modifier(method, Modifier::Async);
                let body_signature = if is_async {
                    self.async_signature(&function, method.return_type.span.clone())
                } else {
                    Some(function.clone())
                };
                if let Some(body_signature) = body_signature {
                    let saved_async = std::mem::replace(&mut self.in_async, is_async);
                    let iterator = self.enter_iterator_body(&function.return_type);
                    self.check_function_body(&method.body, &body_signature, &names, node.is_static);
                    self.leave_iterator_body(
                        iterator,
                        EntityID::from(method),
                        method.name.span.clone(),
                    );
                    self.in_async = saved_async;
                }
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
                self.check_member_attributes(property.attributes, node.is_static);
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
                // the attributes are the declaration's: checked once, with
                // the first declarator, when `int a, b;` shares them
                if std::ptr::eq(&field.declarators[0], declarator) {
                    self.check_member_attributes(field.attributes, node.is_static);
                }
                match &declarator.initializer {
                    Some(InitializerValue::Expression(initializer)) => {
                        let function = crate::types::FunctionSignature {
                            return_type: field_type.clone(),
                            parameters: Vec::new(),
                        };
                        self.enter_body(&function, &[], node.is_static, |checker| {
                            let literal = Self::is_integer_literal(initializer);
                            let ty =
                                checker.check_expression_expecting(initializer, Some(&field_type));
                            checker.require_convertible(
                                &ty,
                                &field_type,
                                literal,
                                initializer.span(),
                            );
                        });
                    }
                    // `public int[] steps = { 1, 2 };`
                    Some(InitializerValue::Nested(initializer)) => {
                        let function = crate::types::FunctionSignature {
                            return_type: field_type.clone(),
                            parameters: Vec::new(),
                        };
                        self.enter_body(&function, &[], node.is_static, |checker| {
                            checker.check_array_initializer(initializer, &field_type);
                        });
                    }
                    None => {}
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
    pub(super) fn enter_body(
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

        self.locals.push(Scope::default());
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

    pub(super) fn check_function_body_inner(&mut self, body: &'ast FunctionBody<'ast, 'ast>) {
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

                    self.locals.push(Scope::default());
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
}
