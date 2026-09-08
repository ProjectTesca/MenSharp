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
            scope.locals.insert(name, LocalVariable { ty, order });
        }
    }

    /// A parameter bound straight into a scope of its own, stamped like
    /// any other declaration.
    pub(super) fn new_local(&mut self, ty: Type) -> LocalVariable {
        self.local_order += 1;
        LocalVariable {
            ty,
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
