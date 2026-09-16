//! The checker's plumbing: errors, scopes, locals and the small
//! questions the rest of the walk keeps asking.

use super::{Checker, LocalVariable};
use crate::error::SemanticErrorKind;
use crate::semantics::resolve::Resolution;
use crate::symbol::{SymbolId, SyntaxRef};
use crate::types::lookup::TypeSystem;
use crate::types::{Type, TypeTarget};
use men_sharp_diagnostics::{Edit, Hint, Message};
use men_sharp_parser::ast::{
    BinaryOperator, EntityID, Expression, LiteralExpression, PrimaryLeft, UnaryOperator,
};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn system(&self) -> TypeSystem<'a, 'ast> {
        TypeSystem {
            declarations: self.resolver.declarations,
            signatures: self.signatures,
            external: self.resolver.external,
        }
    }

    pub(super) fn error(&mut self, kind: SemanticErrorKind, span: Range<usize>) {
        self.resolver.error(kind, span);
    }

    pub(super) fn error_with_hint(
        &mut self,
        kind: SemanticErrorKind,
        span: Range<usize>,
        hint: Option<Hint>,
    ) {
        self.resolver
            .error_with_hints(kind, span, hint.into_iter().collect());
    }

    /// `async` added to the member being checked, when the `await` is in
    /// the member's own body (in a lambda, the lambda would need it).
    pub(super) fn async_hint(&self) -> Option<Hint> {
        if !self.lambda_stack.is_empty() {
            return None;
        }
        let member = self.current_member?;
        let entry = self.resolver.declarations.table.symbol(member);
        let return_type = entry
            .declarations
            .iter()
            .find_map(|site| match &site.syntax {
                SyntaxRef::Method(declaration) => Some(declaration.return_type.span.start),
                _ => None,
            })?;
        Some(Hint::edit(
            Message::key("hint.add_async"),
            Edit::insert(self.resolver.file.0, return_type, "async "),
        ))
    }

    /// `(int)` in front of a value that needs an explicit numeric conversion.
    fn cast_hint(&self, from: &Type, to: &Type, span: &Range<usize>) -> Option<Hint> {
        let system = self.system();
        let keyword = system.numeric_kind(to)?.keyword();
        system.numeric_kind(from)?;
        // the whole expression is wrapped: `(int)(a * b)`, not `(int)a * b`
        Some(Hint::edit(
            Message::key("hint.explicit_cast"),
            Edit::wrap(
                self.resolver.file.0,
                span.clone(),
                format!("({keyword})("),
                ")",
            ),
        ))
    }

    /// A type as a person reads it in a message (see `TypeSystem::describe`).
    pub(super) fn describe(&self, ty: &Type) -> String {
        self.system().describe(ty)
    }

    pub(super) fn resolve_type(
        &mut self,
        node: &men_sharp_parser::ast::TypeRef<'ast, 'ast>,
    ) -> Type {
        let Checker {
            resolver,
            scopes,
            type_stack,
            ..
        } = self;
        resolver.resolve_type_ref(node, scopes, type_stack)
    }

    pub(super) fn lookup_name(
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

    pub(super) fn resolve_using(
        &mut self,
        using: &men_sharp_parser::ast::UsingDirective<'ast, 'ast>,
    ) -> Option<crate::semantics::resolve::ResolvedUsing<'ast>> {
        let Checker {
            resolver, scopes, ..
        } = self;
        resolver.resolve_using(using, scopes)
    }

    pub(super) fn record(&mut self, expression: &Expression<'ast, 'ast>, ty: Type) -> Type {
        self.expression_types
            .insert(EntityID::from(expression), ty.clone());
        ty
    }

    pub(super) fn corlib(&self, name: &str) -> Type {
        match self.resolver.external.find_type(&["System"], name, 0) {
            Some(id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => Type::Error,
        }
    }

    pub(super) fn declare_local(&mut self, name: &'ast str, ty: Type) {
        // a name declared inside a lambda or local function belongs to it:
        // no caller has to hand it over (see `close_captures`)
        if let Some((entity, _)) = self.lambda_stack.last() {
            self.declared_names
                .entry(*entity)
                .or_default()
                .insert(name.to_string());
        }
        self.local_order += 1;
        let order = self.local_order;
        if let Some(scope) = self.locals.last_mut() {
            scope.locals.insert(
                name,
                LocalVariable {
                    ty,
                    order,
                    integer_constant: None,
                },
            );
        }
    }

    /// A parameter bound straight into a scope of its own, stamped like
    /// any other declaration.
    pub(super) fn new_local(&mut self, ty: Type) -> LocalVariable {
        self.local_order += 1;
        LocalVariable {
            ty,
            integer_constant: None,
            order: self.local_order,
        }
    }

    pub(super) fn local(&self, name: &str) -> Option<&Type> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.locals.get(name))
            .map(|local| &local.ty)
    }

    /// A local or parameter was named: when its declaration lies outside a
    /// lambda being checked, that lambda — and every lambda between —
    /// captures it.
    pub(super) fn note_local_use(&mut self, name: &str, span: &Range<usize>) {
        let Some(depth) = self
            .locals
            .iter()
            .rposition(|scope| scope.locals.contains_key(name))
        else {
            return;
        };
        let variable = &self.locals[depth].locals[name];
        let ty = variable.ty.clone();
        let order = variable.order;
        let mut captured = false;
        let mut in_static = false;
        let mut declared_later = false;
        for (lambda, base) in &self.lambda_stack {
            if depth < *base {
                self.captures
                    .entry(*lambda)
                    .or_default()
                    .insert(name.to_string());
                captured = true;
                in_static |= self.static_local_functions.contains(lambda);
                // a local function is written where it is: the variables
                // below it are not yet variables (CS0841)
                declared_later |= self
                    .local_function_order
                    .get(lambda)
                    .is_some_and(|written| order > *written);
            }
        }
        if in_static {
            let kind = SemanticErrorKind::StaticLocalFunctionCapture {
                name: name.to_string(),
            };
            let hint = Hint::text(Message::key("hint.remove_static").arg("name", name));
            self.error_with_hint(kind, span.clone(), Some(hint));
        }
        if declared_later {
            let kind = SemanticErrorKind::LocalUsedBeforeDeclaration {
                name: name.to_string(),
            };
            self.error(kind, span.clone());
        }
        if captured && let Some(member) = self.current_member {
            self.captured_locals
                .entry(member)
                .or_default()
                .insert(name.to_string());
            self.capture_types
                .entry(member)
                .or_default()
                .insert(name.to_string(), ty);
        }
    }

    /// A source type applied to its own generic parameters — the enclosing
    /// types' first, as a nested type's arguments always are.
    pub(super) fn open_type(&self, symbol: SymbolId) -> Type {
        let arguments = self
            .system()
            .source_type_parameters(symbol)
            .into_iter()
            .map(Type::TypeParameter)
            .collect();
        Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        }
    }

    /// `this` inside the innermost enclosing type: its own generic parameters
    /// applied to itself.
    pub(super) fn self_type(&self) -> Option<Type> {
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

    pub(super) fn require_convertible(
        &mut self,
        from: &Type,
        to: &Type,
        literal: bool,
        span: Range<usize>,
    ) {
        let ok = self.system().is_implicitly_convertible(from, to)
            || (literal && self.integer_literal_fits(to));
        if !ok {
            let kind = SemanticErrorKind::TypeMismatch {
                expected: self.describe(to),
                found: self.describe(from),
            };
            let hint = self.cast_hint(from, to, &span);
            self.error_with_hint(kind, span, hint);
        }
    }

    /// Integral constant expressions used by operator overload resolution.
    /// Keep values only for const locals, never for ordinary variables whose
    /// initializer happens to be a literal.
    pub(super) fn integer_constant(&self, expression: &Expression<'ast, 'ast>) -> Option<i128> {
        self.integer_constant_inner(expression, 64)
    }

    fn integer_constant_inner(
        &self,
        expression: &Expression<'ast, 'ast>,
        depth: usize,
    ) -> Option<i128> {
        use crate::types::conversions::NumericKind::*;
        if depth == 0 {
            return None;
        }
        let Some(ty) = self.expression_types.get(&EntityID::from(expression)) else {
            return super::exhaustive::integer_literal_value(expression).map(i128::from);
        };
        let kind = self.system().numeric_kind(ty)?;
        if !kind.is_integral() {
            return None;
        }
        let value = match expression {
            Expression::Primary(primary)
                if self
                    .targets
                    .get(
                        &primary
                            .chain
                            .last()
                            .map(EntityID::from)
                            .unwrap_or_else(|| EntityID::from(&primary.left)),
                    )
                    .is_some_and(|t| matches!(t, super::ResolvedTarget::Member(_))) =>
            {
                let node = primary
                    .chain
                    .last()
                    .map(EntityID::from)
                    .unwrap_or_else(|| EntityID::from(&primary.left));
                let super::ResolvedTarget::Member(member) = self.targets.get(&node)? else {
                    return None;
                };
                match &member.origin {
                    crate::types::lookup::MemberOrigin::External { member, .. } => {
                        match member.constant.as_ref()? {
                            crate::ExternalConstant::Int(v) => i128::from(*v),
                            crate::ExternalConstant::UInt(v) => i128::from(*v),
                            _ => return None,
                        }
                    }
                    crate::types::lookup::MemberOrigin::Source(id) => {
                        let symbol = self.resolver.declarations.table.symbol(*id);
                        let value = symbol.declarations.iter().find_map(|site| {
                            if let crate::symbol::SyntaxRef::Field { field, declarator } =
                                site.syntax
                                && field
                                    .modifiers
                                    .iter()
                                    .any(|m| m.value == men_sharp_parser::ast::Modifier::Const)
                                && let Some(men_sharp_parser::ast::InitializerValue::Expression(
                                    value,
                                )) = &declarator.initializer
                            {
                                return Some(value);
                            }
                            None
                        })?;
                        self.integer_constant_inner(value, depth - 1)?
                    }
                    _ => return None,
                }
            }
            Expression::Primary(primary) if primary.chain.is_empty() => match &primary.left {
                PrimaryLeft::Parenthesized { expression, .. } => {
                    self.integer_constant_inner(expression, depth - 1)?
                }
                PrimaryLeft::Identifier { name, .. } => {
                    self.locals
                        .iter()
                        .rev()
                        .find_map(|scope| scope.locals.get(name.value))?
                        .integer_constant?
                }
                _ => i128::from(super::exhaustive::integer_literal_value(expression)?),
            },
            Expression::Cast(cast) => {
                self.integer_constant_inner(cast.value.as_ref().ok()?, depth - 1)?
            }
            Expression::Unary(unary) => {
                let value = self.integer_constant_inner(unary.operand.as_ref().ok()?, depth - 1)?;
                match unary.operator.value {
                    UnaryOperator::Plus => value,
                    UnaryOperator::Minus => value.checked_neg()?,
                    UnaryOperator::BitwiseNot => !value,
                    _ => return None,
                }
            }
            Expression::Binary(binary) => {
                let left = self.integer_constant_inner(&binary.left, depth - 1)?;
                let right = self.integer_constant_inner(binary.right.as_ref().ok()?, depth - 1)?;
                match binary.operator.value {
                    BinaryOperator::Add => left.checked_add(right)?,
                    BinaryOperator::Subtract => left.checked_sub(right)?,
                    BinaryOperator::Multiply => left.checked_mul(right)?,
                    BinaryOperator::Divide => left.checked_div(right)?,
                    BinaryOperator::Modulo => left.checked_rem(right)?,
                    BinaryOperator::BitwiseAnd => left & right,
                    BinaryOperator::BitwiseOr => left | right,
                    BinaryOperator::BitwiseXor => left ^ right,
                    BinaryOperator::LeftShift
                    | BinaryOperator::RightShift
                    | BinaryOperator::UnsignedRightShift => {
                        let bits = if matches!(kind, Int64 | UInt64) {
                            64
                        } else {
                            32
                        };
                        let count = (right as u32) & (bits - 1);
                        let shifted = match binary.operator.value {
                            BinaryOperator::LeftShift => left << count,
                            BinaryOperator::UnsignedRightShift if bits == 32 => {
                                i128::from((left as u32) >> count)
                            }
                            BinaryOperator::UnsignedRightShift => {
                                i128::from((left as u64) >> count)
                            }
                            _ => left >> count,
                        };
                        match kind {
                            Int32 => i128::from(shifted as i32),
                            UInt32 => i128::from(shifted as u32),
                            Int64 => i128::from(shifted as i64),
                            UInt64 => i128::from(shifted as u64),
                            _ => return None,
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        let fits = match kind {
            SByte => i8::try_from(value).is_ok(),
            Byte => u8::try_from(value).is_ok(),
            Int16 => i16::try_from(value).is_ok(),
            UInt16 | Char => u16::try_from(value).is_ok(),
            Int32 => i32::try_from(value).is_ok(),
            UInt32 => u32::try_from(value).is_ok(),
            Int64 => i64::try_from(value).is_ok(),
            UInt64 => u64::try_from(value).is_ok(),
            _ => false,
        };
        fits.then_some(value)
    }

    /// The constant-expression allowance, without evaluating: an integer
    /// constant expression may sit in any integral slot (the C# LSP checks the
    /// actual range).
    fn integer_literal_fits(&self, to: &Type) -> bool {
        self.system()
            .numeric_kind(to)
            .map(|kind| kind.is_integral())
            .unwrap_or(false)
    }

    /// An integer constant expression built from literals alone (§12.23):
    /// `5`, `-5`, `(3 - 5)`, `1 << 4`, `20 / 3`. These convert implicitly to
    /// any integral type the value fits, which is what lets `sbyte x = 3 - 5;`
    /// compile. Named constants are not folded here (yet).
    pub(super) fn is_integer_literal(expression: &Expression) -> bool {
        match expression {
            Expression::Primary(primary) => {
                primary.chain.is_empty()
                    && match &primary.left {
                        PrimaryLeft::Literal(LiteralExpression::Integer(_)) => true,
                        PrimaryLeft::Parenthesized { expression, .. } => {
                            Self::is_integer_literal(expression)
                        }
                        _ => false,
                    }
            }
            Expression::Unary(unary) => {
                matches!(
                    unary.operator.value,
                    UnaryOperator::Minus | UnaryOperator::Plus | UnaryOperator::BitwiseNot
                ) && unary
                    .operand
                    .as_ref()
                    .map(|operand| Self::is_integer_literal(operand))
                    .unwrap_or(false)
            }
            Expression::Binary(binary) => {
                matches!(
                    binary.operator.value,
                    BinaryOperator::Add
                        | BinaryOperator::Subtract
                        | BinaryOperator::Multiply
                        | BinaryOperator::Divide
                        | BinaryOperator::Modulo
                        | BinaryOperator::LeftShift
                        | BinaryOperator::RightShift
                        | BinaryOperator::BitwiseAnd
                        | BinaryOperator::BitwiseOr
                        | BinaryOperator::BitwiseXor
                ) && Self::is_integer_literal(&binary.left)
                    && binary
                        .right
                        .as_ref()
                        .map(|right| Self::is_integer_literal(right))
                        .unwrap_or(false)
            }
            _ => false,
        }
    }
}
