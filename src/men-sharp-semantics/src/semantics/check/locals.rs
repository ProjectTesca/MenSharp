//! Deconstruction, local functions and what a lambda captures.

use super::{
    Checker, LocalFunctionEntry, LocalFunctionSignature, Meaning, MethodGroup, ResolvedCall,
    ResolvedTarget, Scope,
};
use crate::error::SemanticErrorKind;
use crate::symbol::{Accessibility, SymbolKind};
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{FunctionSignature, MemberSignature, ParameterPassing, Type};
use men_sharp_parser::ast::{
    Block, EntityID, Expression, MethodDeclaration, Modifier, PrimaryLeft, Statement,
    VariableDesignation,
};
use std::collections::BTreeSet;
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    /// Is this assignment target a deconstruction rather than a place?
    pub(super) fn is_deconstruction(target: &'ast Expression<'ast, 'ast>) -> bool {
        match target {
            Expression::Declaration(_) => true,
            Expression::Primary(primary) => {
                primary.chain.is_empty() && matches!(primary.left, PrimaryLeft::Tuple { .. })
            }
            _ => false,
        }
    }

    pub(super) fn check_deconstruction(
        &mut self,
        assignment: &'ast men_sharp_parser::ast::AssignmentExpression<'ast, 'ast>,
    ) -> Type {
        let Ok(value) = &assignment.value else {
            return Type::Error;
        };
        let value_type = self.check_expression(value);
        self.bind_deconstruction(&assignment.target, &value_type);
        value_type
    }

    /// One side of a deconstruction against the type it takes apart.
    fn bind_deconstruction(&mut self, target: &'ast Expression<'ast, 'ast>, value: &Type) {
        match target {
            // `var (a, b)`, `int x` — a variable written where a value goes
            Expression::Declaration(declaration) => {
                let declared = self.resolve_type(&declaration.variable_type);
                self.bind_designation(
                    &declaration.designation,
                    &declared,
                    value,
                    &declaration.span,
                );
            }
            // `(x, y)` — each element assigns or declares on its own
            Expression::Primary(primary) => {
                let PrimaryLeft::Tuple { elements, span } = &primary.left else {
                    self.error(SemanticErrorKind::UnsupportedExpression, target.span());
                    return;
                };
                let node = EntityID::from(&primary.left);
                let Some(types) = self.tuple_parts(value, elements.len(), node, span) else {
                    return;
                };
                for (element, element_type) in elements.iter().zip(types) {
                    match &element.value {
                        Expression::Declaration(_) | Expression::Primary(_)
                            if Self::is_deconstruction(&element.value) =>
                        {
                            self.bind_deconstruction(&element.value, &element_type);
                        }
                        // `_` on its own is a discard, not a variable
                        value_target if Self::is_discard(value_target) => {}
                        value_target => {
                            let ty = self.check_expression(value_target);
                            self.require_convertible(
                                &element_type,
                                &ty,
                                false,
                                value_target.span(),
                            );
                        }
                    }
                }
            }
            other => {
                self.error(SemanticErrorKind::UnsupportedExpression, other.span());
            }
        }
    }

    /// `(a, (b, c))` and friends: the names a deconstruction declares.
    pub(super) fn bind_designation(
        &mut self,
        designation: &'ast VariableDesignation<'ast, 'ast>,
        declared: &Type,
        value: &Type,
        span: &Range<usize>,
    ) {
        match designation {
            VariableDesignation::Single(name) => {
                let ty = if matches!(declared, Type::Infer) {
                    value.clone()
                } else {
                    self.require_convertible(value, declared, false, name.span.clone());
                    declared.clone()
                };
                if matches!(ty, Type::Void | Type::Null) {
                    self.error(SemanticErrorKind::TypeAnnotationNeeded, name.span.clone());
                }
                self.declare_local(name.value, ty);
            }
            VariableDesignation::Discard(_) => {}
            VariableDesignation::Parenthesized { elements, span } => {
                let node = EntityID::from(designation);
                let Some(types) = self.tuple_parts(value, elements.len(), node, span) else {
                    return;
                };
                for (element, element_type) in elements.iter().zip(types) {
                    self.bind_designation(element, declared, &element_type, span);
                }
            }
        }
        let _ = span;
    }

    /// The parts a value of this type comes apart into: a tuple's elements,
    /// or what its `Deconstruct` writes. The call, when there is one, is
    /// recorded on `node` for the code generator.
    pub(super) fn tuple_parts(
        &mut self,
        value: &Type,
        wanted: usize,
        node: EntityID,
        span: &Range<usize>,
    ) -> Option<Vec<Type>> {
        if matches!(value, Type::Error) {
            return None;
        }
        if let Type::Tuple(elements) = value
            && elements.len() == wanted
        {
            return Some(
                elements
                    .iter()
                    .map(|element| element.element.clone())
                    .collect(),
            );
        }
        if let Some(parts) = self.deconstruct_parts(value, wanted, node) {
            return Some(parts);
        }
        let kind = SemanticErrorKind::TypeMismatch {
            expected: format!(
                "a tuple of {wanted} elements, or a `Deconstruct` with {wanted} `out` parameters"
            ),
            found: self.describe(value),
        };
        self.error(kind, span.clone());
        None
    }

    /// `void Deconstruct(out A a, out B b)` on the type, as C# §12.7 looks
    /// for it: the one whose `out` parameters match how many parts the
    /// target wants.
    fn deconstruct_parts(
        &mut self,
        value: &Type,
        wanted: usize,
        node: EntityID,
    ) -> Option<Vec<Type>> {
        let candidates = self.system().members_named(value, "Deconstruct");
        for candidate in candidates {
            if candidate.is_static || candidate.kind != SymbolKind::Method {
                continue;
            }
            let Some(MemberSignature::Function(signature)) = &candidate.signature else {
                continue;
            };
            if signature.parameters.len() != wanted
                || signature.return_type != Type::Void
                || !signature
                    .parameters
                    .iter()
                    .all(|parameter| parameter.passing == ParameterPassing::Out)
            {
                continue;
            }
            let parts: Vec<Type> = signature
                .parameters
                .iter()
                .map(|parameter| parameter.parameter_type.clone())
                .collect();
            let call = ResolvedCall {
                origin: candidate.origin.clone(),
                is_static: false,
                is_extension: false,
                declaring_type: candidate.declaring_type.clone(),
                signature: signature.clone(),
                type_arguments: Vec::new(),
                parameter_of_argument: (0..wanted).collect(),
                params_expansion: None,
            };
            self.targets.insert(node, ResolvedTarget::Call(call));
            return Some(parts);
        }
        None
    }

    /// A bare `_`: a discard, wherever a value could have been named.
    fn is_discard(expression: &Expression<'ast, 'ast>) -> bool {
        let Expression::Primary(primary) = expression else {
            return false;
        };
        primary.chain.is_empty()
            && matches!(
                &primary.left,
                PrimaryLeft::Identifier { name, generics: None, .. } if name.value == "_"
            )
    }

    /// A local function in scope, innermost first.
    pub(super) fn local_function(&self, name: &str) -> Option<&LocalFunctionEntry<'ast>> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.functions.get(name))
    }

    /// The names a freshly built scope declares belong to the lambda or
    /// local function that is about to open it.
    pub(super) fn note_declared(&mut self, scope: &Scope<'ast>) {
        if let Some((entity, _)) = self.lambda_stack.last() {
            let entity = *entity;
            let names: Vec<String> = scope.locals.keys().map(|name| name.to_string()).collect();
            self.declared_names.entry(entity).or_default().extend(names);
        }
    }

    /// Every local function written in a block, declared before the block's
    /// statements are walked: a local function may be called from anywhere
    /// in its block, its own body included (C# §13.6.4).
    pub(super) fn declare_local_functions(
        &mut self,
        block: &'ast Block<'ast, 'ast>,
    ) -> Vec<&'ast MethodDeclaration<'ast, 'ast>> {
        let mut declared = Vec::new();
        for statement in block.statements {
            let Statement::LocalFunction(function) = statement else {
                continue;
            };
            let id = EntityID::from(function);
            let is_static = Self::has_modifier(function, Modifier::Static);
            // a generic local function would need one compiled instance per
            // set of type arguments, which the Udon backend has no way to
            // pick at a call site
            let signature = if function.generics.is_some() {
                self.error(
                    SemanticErrorKind::GenericLocalFunction,
                    function.name.span.clone(),
                );
                None
            } else {
                let signature = self.local_function_signature(function);
                self.local_functions.insert(
                    id,
                    LocalFunctionSignature {
                        signature: signature.clone(),
                        is_static,
                    },
                );
                declared.push(function);
                Some(signature)
            };
            if is_static {
                self.static_local_functions.insert(id);
            }
            if let Some(scope) = self.locals.last_mut() {
                scope.functions.insert(
                    function.name.value,
                    LocalFunctionEntry {
                        node: function,
                        signature,
                    },
                );
            }
        }
        declared
    }

    pub(super) fn has_modifier(
        function: &MethodDeclaration<'ast, 'ast>,
        modifier: Modifier,
    ) -> bool {
        function
            .modifiers
            .iter()
            .any(|written| written.value == modifier)
    }

    fn local_function_signature(
        &mut self,
        function: &'ast MethodDeclaration<'ast, 'ast>,
    ) -> FunctionSignature {
        use men_sharp_parser::ast::ParameterModifier;

        let return_type = self.resolve_type(&function.return_type);
        let mut parameters = Vec::new();
        if let Ok(list) = &function.parameters {
            for parameter in list.parameters {
                let parameter_type = match &parameter.parameter_type {
                    Some(type_ref) => self.resolve_type(type_ref),
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
                parameters.push(crate::types::ParameterSignature {
                    passing,
                    is_params,
                    parameter_type,
                    name: parameter
                        .name
                        .as_ref()
                        .ok()
                        .map(|name| name.value.to_string()),
                    default_value: parameter
                        .default_value
                        .as_ref()
                        .map(|_| crate::types::DefaultArgument::Source),
                });
            }
        }
        FunctionSignature {
            return_type,
            parameters,
        }
    }

    /// A local function's body, checked once its whole block is known — so
    /// it may use a variable the block declares below it, as C# allows.
    pub(super) fn check_local_function_body(
        &mut self,
        function: &'ast MethodDeclaration<'ast, 'ast>,
    ) {
        let id = EntityID::from(function);
        let Some(declaration) = self.local_functions.get(&id).cloned() else {
            return;
        };
        let is_async = Self::has_modifier(function, Modifier::Async);
        let signature = if is_async {
            match self.async_signature(&declaration.signature, function.return_type.span.clone()) {
                Some(signature) => signature,
                None => return,
            }
        } else {
            declaration.signature
        };

        let saved_return = std::mem::replace(&mut self.return_type, signature.return_type.clone());
        let saved_static = self.static_context;
        let saved_async = std::mem::replace(&mut self.in_async, is_async);
        let iterator = self.enter_iterator_body(&signature.return_type);
        // a `return` in here is this function's, not that of a lambda whose
        // return type is being probed around it
        let saved_probe = self.lambda_probe_returns.take();
        self.static_context |= declaration.is_static;

        self.lambda_stack.push((id, self.locals.len()));
        let mut scope = Scope::default();
        if let Ok(list) = &function.parameters {
            for (parameter, signature) in list.parameters.iter().zip(&signature.parameters) {
                if let Ok(name) = &parameter.name {
                    let local = self.new_local(signature.parameter_type.clone());
                    scope.locals.insert(name.value, local);
                }
            }
        }
        self.note_declared(&scope);
        self.locals.push(scope);

        self.check_function_body_inner(&function.body);

        self.locals.pop();
        self.lambda_stack.pop();
        self.lambda_probe_returns = saved_probe;
        self.leave_iterator_body(iterator, id, function.name.span.clone());
        self.static_context = saved_static;
        self.in_async = saved_async;
        self.return_type = saved_return;
    }

    /// A name that is a local function: the call binds to it directly, and
    /// whatever lambda or local function names it takes on its captures.
    pub(super) fn local_function_group(
        &mut self,
        name: &'ast str,
        span: &Range<usize>,
    ) -> Option<Meaning<'ast>> {
        let entry = self.local_function(name)?;
        let id = EntityID::from(entry.node);
        let signature = entry.signature.clone();
        if let Some((caller, _)) = self.lambda_stack.last() {
            let caller = *caller;
            self.local_calls.entry(caller).or_default().insert(id);
        }
        // a declaration the checker refused: its error is already out, and
        // a call to it is no reason for a second one
        let Some(signature) = signature else {
            return Some(Meaning::Error);
        };
        Some(Meaning::Group(MethodGroup {
            candidates: vec![MemberCandidate {
                origin: MemberOrigin::LocalFunction(id),
                kind: SymbolKind::Method,
                // it takes no receiver: what it needs of the enclosing body
                // travels as captures, not as `this`
                is_static: true,
                accessibility: Accessibility::Private,
                arity: 0,
                signature: Some(MemberSignature::Function(signature)),
                declaring_type: self.this_type.clone().unwrap_or(Type::Error),
            }],
            explicit_arguments: Vec::new(),
            via_type: false,
            name,
            receiver: None,
            allow_extensions: false,
            receiver_display: name.to_string(),
            span: span.clone(),
        }))
    }

    /// Capture sets travel along calls: a lambda or local function that
    /// calls a local function has to hand it every box it captures, so it
    /// captures them too — minus the ones it declares itself. Repeated
    /// until nothing changes, so a chain (and mutual recursion) settles.
    pub(super) fn close_captures(&mut self) {
        loop {
            let mut changed = false;
            let edges: Vec<(EntityID, Vec<EntityID>)> = self
                .local_calls
                .iter()
                .map(|(caller, callees)| (*caller, callees.iter().copied().collect()))
                .collect();
            for (caller, callees) in edges {
                let own = self
                    .declared_names
                    .get(&caller)
                    .cloned()
                    .unwrap_or_default();
                let mut wanted: BTreeSet<String> = BTreeSet::new();
                for callee in callees {
                    if let Some(names) = self.captures.get(&callee) {
                        wanted.extend(names.iter().filter(|name| !own.contains(*name)).cloned());
                    }
                }
                if wanted.is_empty() {
                    continue;
                }
                let captures = self.captures.entry(caller).or_default();
                for name in wanted {
                    changed |= captures.insert(name);
                }
            }
            if !changed {
                return;
            }
        }
    }
}
