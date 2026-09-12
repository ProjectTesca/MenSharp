//! Expressions: what one is worth, and what a name in one means.

use super::{AccessContext, Checker, Meaning, MethodGroup, ResolvedMember, ResolvedTarget, Scope};
use crate::error::SemanticErrorKind;
use crate::semantics::resolve::Resolution;
use crate::symbol::SymbolKind;
use crate::types::external::ExternalTypeKind;
use crate::types::infer::best_common_type;
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{MemberSignature, TupleElement, Type, TypeTarget};
use men_sharp_parser::ast::{
    AssignmentOperator, BinaryOperator, EntityID, Expression, InterpolationPart, LiteralExpression,
    MemberSeparator, Pattern, PrimaryExpression, PrimaryLeft, PrimaryRight,
};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn check_expression(&mut self, expression: &'ast Expression<'ast, 'ast>) -> Type {
        self.check_expression_expecting(expression, None)
    }

    /// `expected` is the target type the context supplies — what makes lambdas,
    /// `default`, and target-typed `new` well-typed where C# says they are.
    pub(super) fn check_expression_expecting(
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
                // `var (a, b) = t;`, `(c, d) = t;`, `(int e, string f) = t;`
                // — C# models every one of these as an assignment
                if assignment.operator.value == AssignmentOperator::Assign
                    && Self::is_deconstruction(&assignment.target)
                {
                    return self.check_deconstruction(assignment);
                }
                let target = self.check_expression(&assignment.target);
                let Ok(value) = &assignment.value else {
                    return target;
                };
                let literal = Self::is_integer_literal(value);
                let value_type = self.check_expression_expecting(value, Some(&target));

                match assignment.operator.value {
                    AssignmentOperator::Assign | AssignmentOperator::Coalesce => {
                        self.require_convertible(&value_type, &target, literal, value.span());
                        // `x ??= y` needs an `x` that can be null (CS8330):
                        // on a plain value type the assignment could never run
                        if assignment.operator.value == AssignmentOperator::Coalesce
                            && !matches!(target, Type::Error)
                            && !self.system().is_reference_type(&target)
                            && !matches!(target, Type::Nullable(_))
                            && self.system().nullable_of(&target).is_none()
                        {
                            let kind = SemanticErrorKind::InvalidOperator {
                                left: self.describe(&target),
                                right: None,
                            };
                            self.error(kind, assignment.span.clone());
                        }
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
                        let zero_literal = assignment
                            .value
                            .as_ref()
                            .ok()
                            .and_then(super::exhaustive::integer_literal_value)
                            == Some(0);
                        let result = self.binary_type(
                            operator,
                            target.clone(),
                            value_type,
                            literal,
                            zero_literal,
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
                                    expected: self.describe(&target),
                                    found: self.describe(&result),
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
                    // C# 9 target typing (§12.18): no natural type, but both
                    // arms convert to what the context wants — `int? x = c ?
                    // 1 : null`, `IShape s = c ? circle : square`
                    None if expected.is_some_and(|target| {
                        !matches!(target, Type::Error)
                            && [&then_type, &else_type]
                                .into_iter()
                                .zip([
                                    conditional.then_value.as_ref().ok(),
                                    conditional.else_value.as_ref().ok(),
                                ])
                                .all(|(arm, value)| {
                                    system.is_implicitly_convertible(arm, target)
                                        || (value.is_some_and(Self::is_integer_literal)
                                            && system.numeric_kind(target).is_some())
                                })
                    }) =>
                    {
                        expected.cloned().unwrap_or(Type::Error)
                    }
                    None => {
                        let kind = SemanticErrorKind::TypeMismatch {
                            expected: self.describe(&then_type),
                            found: self.describe(&else_type),
                        };
                        self.error(kind, conditional.span.clone());
                        Type::Error
                    }
                }
            }
            Expression::Binary(binary) => {
                let left = self.check_expression(&binary.left);
                let (right, literal, zero_literal) = match &binary.right {
                    Ok(right) => (
                        self.check_expression(right),
                        Self::is_integer_literal(right) || Self::is_integer_literal(&binary.left),
                        super::exhaustive::integer_literal_value(right) == Some(0)
                            || super::exhaustive::integer_literal_value(&binary.left) == Some(0),
                    ),
                    Err(()) => (Type::Error, false, false),
                };
                self.binary_type(
                    binary.operator.value,
                    left,
                    right,
                    literal,
                    zero_literal,
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
                match &is.pattern {
                    Ok(Pattern::Discard(span)) => {
                        self.error(SemanticErrorKind::DiscardIsNotAPattern, span.clone());
                    }
                    Ok(pattern) => self.check_pattern(pattern, &value),
                    Err(()) => {}
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
                    let ty = self.check_expression(value);
                    self.require_exception(&ty, value.span());
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
                        self.locals.push(Scope::default());
                        self.check_pattern(&arm.pattern, &value);
                        if let Some(guard) = &arm.guard {
                            self.check_condition(guard);
                        }
                        if let Ok(arm_value) = &arm.value {
                            arm_types.push(self.check_expression_expecting(arm_value, expected));
                        }
                        self.locals.pop();
                    }
                    self.check_switch_expression_exhaustive(
                        &value,
                        arms,
                        switch.switch_keyword.clone(),
                    );
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
                            expected: self.describe(&first),
                            found: self.describe(&clash),
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
            Expression::Await(await_expression) => self.check_await(await_expression),
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
            // `p with { X = 1 }`: a copy of a record (or struct) with the
            // listed members assigned — an object initializer on the copy
            Expression::With(with) => {
                use men_sharp_parser::ast::Initializer;
                let ty = self.check_expression(&with.value);
                let copyable = matches!(
                    &ty,
                    Type::Named { target: TypeTarget::Source(symbol), .. }
                    if matches!(
                        self.resolver.declarations.table.symbol(*symbol).kind,
                        SymbolKind::Record | SymbolKind::RecordStruct | SymbolKind::Struct
                    )
                );
                if !copyable {
                    if !matches!(ty, Type::Error) {
                        let kind = SemanticErrorKind::WithNeedsRecord {
                            type_name: self.describe(&ty),
                        };
                        self.error(kind, with.value.span());
                    }
                    return Type::Error;
                }
                match &with.initializer {
                    Ok(initializer @ Initializer::Object { .. }) => {
                        self.check_initializer_value(initializer, &ty);
                    }
                    Ok(other) => {
                        self.error(SemanticErrorKind::UnsupportedExpression, other.span());
                    }
                    Err(()) => {}
                }
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
        for (index, right) in primary.chain.iter().enumerate() {
            let invoked_next = matches!(
                primary.chain.get(index + 1),
                Some(PrimaryRight::Invocation { .. })
            );
            meaning = self.apply_primary_right(meaning, right, invoked_next);
        }
        let ty = self.value_of(
            meaning,
            primary.span.clone(),
            expected,
            Some(EntityID::from(primary)),
        );
        // `a?.b` with a value-typed `b` is a `b?`: null when `a` is
        let conditional = primary.chain.iter().any(|right| match right {
            PrimaryRight::Member { separator, .. } => {
                separator.value == MemberSeparator::NullConditionalDot
            }
            PrimaryRight::ElementAccess {
                null_conditional, ..
            } => *null_conditional,
            _ => false,
        });
        if conditional
            && matches!(ty, Type::Named { .. })
            && !self.system().is_reference_type(&ty)
            && self.system().nullable_of(&ty).is_none()
        {
            return Type::Nullable(Box::new(ty));
        }
        ty
    }

    /// The `T` of a `T?` that is a `Nullable<T>` — a value type's — in
    /// either spelling. `None` for a reference annotation (`string?`).
    pub(super) fn nullable_value(&self, ty: &Type) -> Option<Type> {
        let system = self.system();
        match ty {
            Type::Nullable(inner) => (matches!(**inner, Type::Named { .. })
                && !system.is_reference_type(inner))
            .then(|| (**inner).clone()),
            other => system.nullable_of(other),
        }
    }

    pub(super) fn value_of(
        &mut self,
        meaning: Meaning<'ast>,
        span: Range<usize>,
        expected: Option<&Type>,
        // the primary node, where a method group's conversion to a delegate
        // is recorded for the code generator
        node: Option<EntityID>,
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
                // a method group converts to a delegate type: to the
                // candidate whose parameters the delegate's convert to — the
                // one taking exactly the delegate's types when there is one
                if let Some(expected) = expected
                    && let Some(delegate) = self.delegate_signature(expected)
                {
                    let system = self.system();
                    let fits = |candidate: &MemberCandidate, exact: bool| match &candidate.signature
                    {
                        Some(MemberSignature::Function(function)) => {
                            candidate.arity as usize == group.explicit_arguments.len()
                                && function.parameters.len() == delegate.parameters.len()
                                && function.parameters.iter().zip(&delegate.parameters).all(
                                    |(method, target)| {
                                        if exact {
                                            method.parameter_type == target.parameter_type
                                        } else {
                                            system.is_implicitly_convertible(
                                                &target.parameter_type,
                                                &method.parameter_type,
                                            )
                                        }
                                    },
                                )
                                && (delegate.return_type == Type::Void
                                    || system.is_implicitly_convertible(
                                        &function.return_type,
                                        &delegate.return_type,
                                    ))
                        }
                        _ => false,
                    };
                    let chosen = group
                        .candidates
                        .iter()
                        .position(|candidate| fits(candidate, true))
                        .or_else(|| {
                            group
                                .candidates
                                .iter()
                                .position(|candidate| fits(candidate, false))
                        });
                    if let Some(index) = chosen {
                        self.record_group_conversion(&group, index, node);
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
                        self.record_group_conversion(&group, 0, node);
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
                    self.note_local_use(name.value, span);
                    self.targets
                        .insert(EntityID::from(left), ResolvedTarget::Local);
                    return Meaning::Value(ty);
                }

                // a local function shadows a member of the same name
                if generics.is_none()
                    && let Some(meaning) = self.local_function_group(name.value, span)
                {
                    return meaning;
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
                        // `using static T;`: T's static members by bare name
                        if let Some(meaning) = self.static_using_meaning(
                            name.value,
                            explicit_arguments,
                            span,
                            Some(EntityID::from(left)),
                        ) {
                            return meaning;
                        }
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
                let ty = Type::Tuple(elements);
                // the tuple is built where it is written, so the code
                // generator reads its type from this very node
                self.expression_types
                    .insert(EntityID::from(left), ty.clone());
                Meaning::Value(ty)
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
                    // without a suffix C# takes the first type the value
                    // fits: `int`, then `long`
                    (false, false) => {
                        let digits: String = stripped.chars().filter(|c| *c != '_').collect();
                        let value = if let Some(hex) = digits
                            .strip_prefix("0x")
                            .or_else(|| digits.strip_prefix("0X"))
                        {
                            i64::from_str_radix(hex, 16)
                        } else if let Some(bits) = digits
                            .strip_prefix("0b")
                            .or_else(|| digits.strip_prefix("0B"))
                        {
                            i64::from_str_radix(bits, 2)
                        } else {
                            digits.parse::<i64>()
                        };
                        match value {
                            Ok(value) if i32::try_from(value).is_err() => "Int64",
                            _ => "Int32",
                        }
                    }
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
    pub(super) fn member_meaning(
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
                .map(|ty| self.describe(ty))
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
            // a nested type name: `Outer<int>.Inner<string>` carries the
            // outer's arguments before its own
            if let MemberOrigin::Source(symbol) = first.origin {
                let mut arguments = match &first.declaring_type {
                    Type::Named { arguments, .. } => arguments.clone(),
                    _ => Vec::new(),
                };
                arguments.extend(explicit_arguments);
                return Some(Meaning::TypeName(Type::Named {
                    target: TypeTarget::Source(symbol),
                    arguments,
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

    pub(super) fn is_struct(&self, ty: &Type) -> bool {
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

    /// One step of a postfix chain. `invoked_next` says an argument list
    /// follows this step, which matters for a member access alone.
    fn apply_primary_right(
        &mut self,
        meaning: Meaning<'ast>,
        right: &'ast PrimaryRight<'ast, 'ast>,
        invoked_next: bool,
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
                let receiver = match &meaning {
                    Meaning::Value(ty) => Some(self.member_receiver(ty.clone())),
                    _ => None,
                };
                let node = EntityID::from(right);
                let accessed = self.access_member(
                    meaning,
                    name.value,
                    explicit_arguments.clone(),
                    span,
                    Some(node),
                );
                // `list.Count(n => n > 1)`: the member found is a property (or
                // a field) that cannot be called, so the invocation binds to
                // an extension method of that name instead, as C# does
                // (§12.8.10.2) — the recorded property access is withdrawn
                if invoked_next
                    && let (Some(receiver), Meaning::Value(ty)) = (receiver, &accessed)
                    && !matches!(ty, Type::Error)
                    && self.delegate_invoke_of(ty).is_none()
                {
                    let probe = MethodGroup {
                        candidates: Vec::new(),
                        explicit_arguments,
                        via_type: false,
                        name: name.value,
                        receiver: Some(receiver.clone()),
                        allow_extensions: true,
                        receiver_display: self.describe(&receiver),
                        span: span.clone(),
                    };
                    if self.extension_group(&probe).is_some() {
                        self.targets.remove(&node);
                        return Meaning::Group(probe);
                    }
                }
                accessed
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
                let receiver = self.value_of(meaning, span.clone(), None, None);
                self.index(
                    receiver,
                    arguments.arguments,
                    span,
                    Some(EntityID::from(right)),
                )
            }
            PrimaryRight::Postfix { operator, span } => {
                let ty = self.value_of(meaning, span.clone(), None, None);
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

    pub(super) fn access_member(
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
                        // the outer's arguments lead the nested type's
                        let mut arguments = match &ty {
                            Type::Named { arguments, .. } => arguments.clone(),
                            _ => Vec::new(),
                        };
                        arguments.extend(explicit_arguments);
                        return Meaning::TypeName(Type::Named {
                            target: nested,
                            arguments,
                        });
                    }
                    let kind = SemanticErrorKind::UnknownMember {
                        type_name: self.describe(&ty),
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
                let receiver = self.member_receiver(ty);
                if matches!(receiver, Type::Error) {
                    return Meaning::Error;
                }
                // `t.Item1`, `t.x`: a tuple's elements are not members of a
                // type Udon knows — they are positions, resolved here
                if let Type::Tuple(elements) = &receiver
                    && let Some(index) = crate::types::tuple_element_index(elements, name)
                {
                    return Meaning::Value(elements[index].element.clone());
                }

                // `op.Invoke(...)` on a delegate of the compilation's own
                if name == "Invoke"
                    && let Some(candidate) = self.source_delegate_invoke(&receiver)
                {
                    return Meaning::Group(MethodGroup {
                        candidates: vec![candidate],
                        explicit_arguments,
                        via_type: false,
                        name,
                        receiver: None,
                        allow_extensions: false,
                        receiver_display: self.describe(&receiver),
                        span: span.clone(),
                    });
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
                        receiver_display: self.describe(&receiver),
                        span: span.clone(),
                    };
                    if self.extension_group(&probe).is_some() {
                        return Meaning::Group(probe);
                    }
                    let kind = SemanticErrorKind::UnknownMember {
                        type_name: self.describe(&receiver),
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

    /// The type whose members a value of `ty` has: a reference annotation
    /// (`string?`) has the members of the type; a `Nullable<T>` has its own
    /// (`HasValue`, `Value`) — and the lookup roots add `T`'s after them.
    fn member_receiver(&self, ty: Type) -> Type {
        match ty {
            Type::Nullable(inner) if self.system().is_reference_type(&inner) => *inner,
            other => other,
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
}
