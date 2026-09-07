//! Statements and patterns.

use super::{Checker, Meaning, MethodGroup, Scope};
use crate::error::SemanticErrorKind;
use crate::semantics::resolve::Resolution;
use crate::symbol::SymbolKind;
use crate::types::lookup::MemberCandidate;
use crate::types::{Type, TypeTarget};
use men_sharp_parser::ast::{
    Block, EntityID, Expression, ForInitializer, InitializerValue, LocalVariableDeclaration,
    Pattern, Statement, SwitchLabel, UsingResource, VariableDesignation,
};

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn check_block(&mut self, block: &'ast Block<'ast, 'ast>) {
        self.locals.push(Scope::default());
        let functions = self.declare_local_functions(block);
        for statement in block.statements {
            self.check_statement(statement);
        }
        // last of all: a local function may use any variable of its block,
        // wherever in the block that variable is written
        for function in functions {
            self.check_local_function_body(function);
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
                self.locals.push(Scope::default());
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
                self.locals.push(Scope::default());
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
                    let span = name.span();
                    self.bind_designation(name, &declared, &element, &span);
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
                        self.locals.push(Scope::default());
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
                    self.check_switch_statement_exhaustive(
                        &value,
                        sections,
                        statement.switch_keyword.clone(),
                    );
                }
            }
            Statement::Try(statement) => {
                let guarded = !statement.catches.is_empty();
                if guarded {
                    self.guarded_depth += 1;
                }
                if let Ok(block) = &statement.block {
                    self.check_block(block);
                }
                if guarded {
                    self.guarded_depth -= 1;
                }
                for catch in statement.catches {
                    self.locals.push(Scope::default());
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
                        self.catch_depth += 1;
                        self.catch_depth_for_yield += 1;
                        self.check_block(block);
                        self.catch_depth_for_yield -= 1;
                        self.catch_depth -= 1;
                    }
                    self.locals.pop();
                }
                if let Some(finally) = &statement.finally_clause
                    && let Ok(block) = &finally.block
                {
                    self.finally_depth += 1;
                    self.check_block(block);
                    self.finally_depth -= 1;
                }
            }
            Statement::Using(statement) => {
                self.locals.push(Scope::default());
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
                self.locals.push(Scope::default());
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
                if statement.value.is_some() {
                    self.value_return_seen = true;
                }
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
            Statement::Throw(statement) => match &statement.value {
                Some(value) => {
                    let ty = self.check_expression(value);
                    self.require_exception(&ty, value.span());
                }
                None => {
                    if self.catch_depth == 0 {
                        self.error(
                            SemanticErrorKind::RethrowOutsideCatch,
                            statement.span.clone(),
                        );
                    }
                }
            },
            Statement::Yield(statement) => self.check_yield(statement),
            Statement::LocalFunction(function) => {
                // where it stands among the block's variables: it may use
                // the ones above it, and its body is checked knowing that
                self.local_function_order
                    .insert(EntityID::from(function), self.local_order);
                // declared and checked by the block it belongs to; reaching
                // one from anywhere else means it is not in a block at all
                let declared = self
                    .local_function(function.name.value)
                    .is_some_and(|entry| EntityID::from(entry.node) == EntityID::from(function));
                if !declared {
                    self.error(
                        SemanticErrorKind::UnsupportedStatement,
                        function.span.clone(),
                    );
                }
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
                // `int[] a = { 1, 2 };` — the shorthand for `new int[] { ... }`
                (declared, Some(InitializerValue::Nested(initializer))) => {
                    let declared = declared.clone();
                    self.check_array_initializer(initializer, &declared);
                    declared
                }
                (declared, _) => declared.clone(),
            };

            self.declare_local(declarator.name.value, ty);
        }
    }

    pub(super) fn check_condition(&mut self, condition: &'ast Expression<'ast, 'ast>) {
        let ty = self.check_expression(condition);
        if !self.system().is_bool(&ty) {
            let kind = SemanticErrorKind::ConditionNotBoolean {
                found: self.describe(&ty),
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

    /// The attributes written on a field or property, bound where they can
    /// be: one whose name resolves to a class of the compilation's own is
    /// checked as the construction `new A(args)` it stands for, and the
    /// code generator builds that object for `Reflect.VisitFields`. Every
    /// other spelling — the engine's, the SDK's, the ones only the Unity
    /// side declares — is left alone, quietly: those are read by spelling
    /// where they matter, and are not this phase's business.
    pub(super) fn check_member_attributes(
        &mut self,
        sections: &'ast [men_sharp_parser::ast::AttributeSection<'ast, 'ast>],
        is_static: bool,
    ) {
        for section in sections {
            for attribute in section.attributes {
                let Some(ty) = self.attribute_class(&attribute.name) else {
                    continue;
                };
                let node = EntityID::from(attribute);
                let function = crate::types::FunctionSignature {
                    return_type: Type::Void,
                    parameters: Vec::new(),
                };
                let class = ty.clone();
                self.enter_body(&function, &[], is_static, |checker| {
                    let constructors: Vec<MemberCandidate> = checker
                        .system()
                        .members_named(&class, ".ctor")
                        .into_iter()
                        .filter(|candidate| {
                            candidate.kind == SymbolKind::Constructor
                                && !candidate.is_static
                                && candidate.declaring_type == class
                        })
                        .collect();
                    let arguments = attribute
                        .arguments
                        .as_ref()
                        .map(|list| checker.check_arguments(list.arguments))
                        .unwrap_or_default();
                    if constructors.is_empty() {
                        // the implicit parameterless constructor
                        if !arguments.is_empty() {
                            checker.error(
                                SemanticErrorKind::NoMatchingOverload,
                                attribute.span.clone(),
                            );
                        }
                        return;
                    }
                    let receiver_display = checker.describe(&class);
                    let group = MethodGroup {
                        candidates: constructors,
                        explicit_arguments: Vec::new(),
                        via_type: false,
                        name: ".ctor",
                        receiver: None,
                        allow_extensions: false,
                        receiver_display,
                        span: attribute.span.clone(),
                    };
                    checker.resolve_call(group, arguments, &attribute.span, Some(node));
                });
                self.attribute_types.insert(node, ty);
            }
        }
    }

    /// The class an attribute's name denotes, when it is one of the
    /// compilation's own: `[JsonName]` finds `JsonName` or, as C# does,
    /// `JsonNameAttribute`. Anything else is `None`, and leaves no error
    /// behind — an unknown spelling is not a mistake here.
    fn attribute_class(
        &mut self,
        name: &'ast men_sharp_parser::ast::TypeRef<'ast, 'ast>,
    ) -> Option<Type> {
        let before = self.resolver.out.errors.len();
        let resolved = self.resolve_type(name);
        let is_source_class = |ty: &Type| {
            matches!(ty, Type::Named { target: TypeTarget::Source(symbol), .. }
                if self.resolver.declarations.table.symbol(*symbol).kind == SymbolKind::Class)
        };
        if is_source_class(&resolved) {
            return Some(resolved);
        }
        self.resolver.out.errors.truncate(before);
        // `[JsonName]` for `class JsonNameAttribute`: a simple name with the
        // conventional suffix. The spelling has to live as long as the
        // syntax it stands in for; a handful of bytes per attribute, kept.
        let men_sharp_parser::ast::TypeRefBase::Name(written) = &name.base else {
            return None;
        };
        let [segment] = written.segments else {
            return None;
        };
        if segment.generics.is_some() || !name.suffixes.is_empty() {
            return None;
        }
        let suffixed: &'ast str =
            Box::leak(format!("{}Attribute", segment.name.value).into_boxed_str());
        let resolution = self.lookup_name(suffixed, 0, &segment.span);
        self.resolver.out.errors.truncate(before);
        match resolution {
            Some(Resolution::Type { target, arguments }) => {
                let ty = Type::Named { target, arguments };
                is_source_class(&ty).then_some(ty)
            }
            _ => None,
        }
    }

    pub(super) fn check_pattern(&mut self, pattern: &'ast Pattern<'ast, 'ast>, matched: &Type) {
        self.pattern_inputs
            .insert(EntityID::from(pattern), matched.clone());
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
            Pattern::Var { designation, .. } => match designation {
                Ok(VariableDesignation::Single(name)) => {
                    self.declare_local(name.value, matched.clone());
                }
                Ok(VariableDesignation::Discard(_)) | Err(()) => {}
                // `var (a, b)`: a deconstruction that always matches
                Ok(designation @ VariableDesignation::Parenthesized { .. }) => {
                    self.bind_designation(designation, &Type::Infer, matched, &pattern.span());
                }
            },
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
                    // `{ Radius: > 3 }` reads the member like `value.Radius`
                    // would; the binding is recorded on the subpattern node
                    let meaning = self.access_member(
                        Meaning::Value(target.clone()),
                        subpattern.name.value,
                        Vec::new(),
                        &subpattern.name.span,
                        Some(EntityID::from(subpattern)),
                    );
                    let member_type =
                        self.value_of(meaning, subpattern.name.span.clone(), None, None);
                    if let Ok(pattern) = &subpattern.pattern {
                        self.check_pattern(pattern, &member_type);
                    }
                }
                if let Some(name) = designation {
                    self.declare_local(name.value, target);
                }
            }
            // `(0, var y)` and `Point(0, var y)`: taken apart, position by
            // position — a tuple's elements, or what `Deconstruct` writes
            Pattern::Positional {
                pattern_type,
                subpatterns,
                property_subpatterns: [],
                designation,
                span,
            } => {
                let node = EntityID::from(pattern);
                let matched = &match pattern_type {
                    Some(pattern_type) => self.resolve_type(pattern_type),
                    None => matched.clone(),
                };
                if let Some(types) = self.tuple_parts(matched, subpatterns.len(), node, span) {
                    for (subpattern, element) in subpatterns.iter().zip(&types) {
                        // `(x: 0, y: 1)`: a name picks the element instead
                        let element = match (&subpattern.name, matched) {
                            (Some(name), Type::Tuple(elements)) => {
                                match crate::types::tuple_element_index(elements, name.value) {
                                    Some(index) => elements[index].element.clone(),
                                    None => {
                                        let kind = SemanticErrorKind::UnknownMember {
                                            type_name: self.describe(matched),
                                        };
                                        self.error(kind, name.span.clone());
                                        Type::Error
                                    }
                                }
                            }
                            _ => element.clone(),
                        };
                        self.check_pattern(&subpattern.pattern, &element);
                    }
                }
                if let Some(name) = designation {
                    self.declare_local(name.value, matched.clone());
                }
            }
            // `[1, 2, ..]`: an array matched by length and position
            Pattern::List {
                elements,
                designation,
                ..
            } => {
                let element = match matched {
                    Type::Array { element, rank: 1 } => (**element).clone(),
                    Type::Error => Type::Error,
                    other => {
                        let kind = SemanticErrorKind::NotIndexable {
                            type_name: self.describe(other),
                        };
                        self.error(kind, pattern.span());
                        Type::Error
                    }
                };
                let mut slices = 0;
                for written in *elements {
                    match written {
                        // `..` and `.. var rest`: the rest is the same kind
                        // of collection, and only one may appear
                        Pattern::Slice { pattern, span, .. } => {
                            slices += 1;
                            if slices > 1 {
                                self.error(SemanticErrorKind::UnsupportedExpression, span.clone());
                            }
                            if let Some(pattern) = pattern {
                                self.check_pattern(pattern, matched);
                            }
                        }
                        written => self.check_pattern(written, &element),
                    }
                }
                if let Some(name) = designation {
                    self.declare_local(name.value, matched.clone());
                }
            }
            // a positional pattern on anything else needs `Deconstruct`, and
            // a slice pattern belongs inside a list one
            _ => {
                self.error(SemanticErrorKind::UnsupportedExpression, pattern.span());
            }
        }
    }
}
