//! Operators, built in and user defined.

use super::{ArgumentShape, AttemptOutcome, CallArgument, Checker, MethodGroup};
use crate::error::SemanticErrorKind;
use crate::symbol::SymbolKind;
use crate::types::conversions::NumericKind;
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{MemberSignature, Type};
use men_sharp_parser::ast::{BinaryOperator, EntityID, UnaryOperator};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn unary_type(
        &mut self,
        operator: UnaryOperator,
        operand: Type,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        if matches!(operand, Type::Error | Type::Dynamic) {
            return operand;
        }
        // a lifted operator (§12.4.8): `-x` on an `int?` is an `int?`
        if let Some(inner) = self.nullable_value(&operand) {
            let underlying = self.unary_type(operator, inner, span, node);
            return match underlying {
                Type::Error => Type::Error,
                underlying => Type::Nullable(Box::new(underlying)),
            };
        }

        let system = self.system();
        match operator {
            UnaryOperator::Not => {
                if system.is_bool(&operand) {
                    self.corlib("Boolean")
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.describe(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::Plus | UnaryOperator::Minus => {
                if let Some(kind) = system.numeric_kind(&operand) {
                    let promoted = match (operator, kind) {
                        (UnaryOperator::Minus, NumericKind::UInt32) => NumericKind::Int64,
                        (UnaryOperator::Minus, NumericKind::UInt64) => {
                            self.error(
                                SemanticErrorKind::InvalidOperator {
                                    left: self.describe(&operand),
                                    right: None,
                                },
                                span,
                            );
                            return Type::Error;
                        }
                        _ => kind.unary_promoted(),
                    };
                    self.corlib(promoted.corlib_name())
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.describe(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::BitwiseNot => {
                if system
                    .numeric_kind(&operand)
                    .map(|kind| kind.is_integral())
                    .unwrap_or(false)
                    || system.is_enum_type(&operand)
                {
                    if let Some(kind) = system.numeric_kind(&operand) {
                        self.corlib(kind.unary_promoted().corlib_name())
                    } else {
                        operand
                    }
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.describe(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::PreIncrement | UnaryOperator::PreDecrement => {
                if system.numeric_kind(&operand).is_some() || system.is_enum_type(&operand) {
                    operand
                } else if let Some(result) =
                    self.user_defined_unary(operator, &operand, &span, node)
                {
                    result
                } else {
                    let kind = SemanticErrorKind::InvalidOperator {
                        left: self.describe(&operand),
                        right: None,
                    };
                    self.error(kind, span);
                    Type::Error
                }
            }
            UnaryOperator::IndexFromEnd | UnaryOperator::AddressOf | UnaryOperator::Dereference => {
                self.error(SemanticErrorKind::UnsupportedExpression, span);
                Type::Error
            }
        }
    }

    /// `literal`: one operand is an integer literal; `zero_literal`: one is
    /// the literal `0`, which converts implicitly to any enum (§10.2.4).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn binary_type(
        &mut self,
        operator: BinaryOperator,
        left: Type,
        right: Type,
        literal: bool,
        zero_literal: bool,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        if matches!(left, Type::Error) || matches!(right, Type::Error) {
            return Type::Error;
        }
        if matches!(left, Type::Dynamic) || matches!(right, Type::Dynamic) {
            return Type::Dynamic;
        }

        let system = self.system();
        use BinaryOperator::*;

        // lifted operators (§12.4.8): on `T?` operands the operator of `T`
        // applies to the values; arithmetic gives a `T?` (null when either
        // side is), comparisons a bool (false when either side is null,
        // except that `==` holds between two nulls)
        let concatenation =
            operator == Add && (system.is_string(&left) || system.is_string(&right));
        if !matches!(operator, Coalesce | LogicalAnd | LogicalOr) && !concatenation {
            let lifted_left = self.nullable_value(&left);
            let lifted_right = self.nullable_value(&right);
            if lifted_left.is_some() || lifted_right.is_some() {
                let inner_left = lifted_left.unwrap_or_else(|| left.clone());
                let inner_right = lifted_right.unwrap_or_else(|| right.clone());
                if matches!(operator, Equal | NotEqual)
                    && (matches!(inner_left, Type::Null) || matches!(inner_right, Type::Null))
                {
                    return self.corlib("Boolean");
                }
                let underlying = self.binary_type(
                    operator,
                    inner_left,
                    inner_right,
                    literal,
                    zero_literal,
                    span,
                    node,
                );
                return match (operator, underlying) {
                    (_, Type::Error) => Type::Error,
                    (
                        Equal | NotEqual | LessThan | GreaterThan | LessThanEqual
                        | GreaterThanEqual,
                        _,
                    ) => self.corlib("Boolean"),
                    (_, underlying) => Type::Nullable(Box::new(underlying)),
                };
            }
        }

        match operator {
            LogicalAnd | LogicalOr => {
                if system.is_bool(&left) && system.is_bool(&right) {
                    return self.corlib("Boolean");
                }
            }
            // `a & b`, `a | b`, `a ^ b` on bools (§12.13.4): both sides
            // evaluated, unlike `&&`/`||` — Udon's `op_LogicalAnd` and
            // friends on Boolean are exactly these
            BitwiseAnd | BitwiseOr | BitwiseXor
                if system.is_bool(&left) && system.is_bool(&right) =>
            {
                return self.corlib("Boolean");
            }
            Coalesce => {
                return match left {
                    Type::Null => right,
                    Type::Nullable(inner) => {
                        // `x ?? fallback`: the underlying type when the
                        // fallback fits it, else the fallback's own type
                        // when `x` fits that
                        if system.is_implicitly_convertible(&right, &inner)
                            || (literal && system.numeric_kind(&inner).is_some())
                        {
                            *inner
                        } else if system
                            .is_implicitly_convertible(&Type::Nullable(inner.clone()), &right)
                        {
                            right
                        } else {
                            let kind = SemanticErrorKind::TypeMismatch {
                                expected: self.describe(&inner),
                                found: self.describe(&right),
                            };
                            self.error(kind, span);
                            Type::Error
                        }
                    }
                    other => other,
                };
            }
            Equal | NotEqual => {
                if let (Some(l), Some(r)) =
                    (system.numeric_kind(&left), system.numeric_kind(&right))
                {
                    return self.numeric_binary(operator, l, r, span, node);
                }

                let comparable = matches!(left, Type::Null)
                    || matches!(right, Type::Null)
                    || (system.numeric_kind(&left).is_some()
                        && system.numeric_kind(&right).is_some())
                    || system.is_bool(&left) && system.is_bool(&right)
                    || left == right
                    || system.is_implicitly_convertible(&left, &right)
                    || system.is_implicitly_convertible(&right, &left);
                // delegates compare by what they call (`Delegate.op_Equality`
                // is not an extern, and Multicast's would tie with it)
                let delegate = self.delegate_signature(&left).is_some()
                    || self.delegate_signature(&right).is_some();
                if comparable && (delegate || !self.has_user_operator(operator, &left, &right)) {
                    return self.corlib("Boolean");
                }
                if let Some(result) = self.user_defined_binary(operator, &left, &right, &span, node)
                {
                    return result;
                }
                if comparable {
                    return self.corlib("Boolean");
                }
            }
            Add if system.is_string(&left) || system.is_string(&right) => {
                // string concatenation accepts anything on the other side
                return self.corlib("String");
            }
            // `Delegate.Combine` / `Remove`: one delegate type on both sides,
            // or `null` on one
            Add | Subtract
                if self.delegate_signature(&left).is_some()
                    || self.delegate_signature(&right).is_some() =>
            {
                let (delegate, other) = if self.delegate_signature(&left).is_some() {
                    (left.clone(), &right)
                } else {
                    (right.clone(), &left)
                };
                if *other == delegate || matches!(other, Type::Null) {
                    return delegate;
                }
            }
            _ => {}
        }

        // numeric operands: the binary promotion rules
        if let (Some(left_kind), Some(right_kind)) =
            (system.numeric_kind(&left), system.numeric_kind(&right))
        {
            return self.numeric_binary(operator, left_kind, right_kind, span, node);
        }

        // enums
        if system.is_enum_type(&left) || system.is_enum_type(&right) {
            // `(flags & Flag.A) != 0`, `flags == 0`: the literal 0 stands
            // for the enum's zero on the other side
            let against_zero = zero_literal
                && (system.is_enum_type(&left) != system.is_enum_type(&right))
                && (system.numeric_kind(&left).is_some() || system.numeric_kind(&right).is_some());
            let enum_side = if system.is_enum_type(&left) {
                left.clone()
            } else {
                right.clone()
            };
            match operator {
                Equal | NotEqual if against_zero => return self.corlib("Boolean"),
                LessThan | GreaterThan | LessThanEqual | GreaterThanEqual => {
                    if left == right || against_zero {
                        return self.corlib("Boolean");
                    }
                }
                BitwiseAnd | BitwiseOr | BitwiseXor if left == right => return left,
                BitwiseAnd | BitwiseOr | BitwiseXor if against_zero => return enum_side,
                Add | Subtract => {
                    if system.is_enum_type(&left) && system.numeric_kind(&right).is_some() {
                        return left;
                    }
                    if system.numeric_kind(&left).is_some() && system.is_enum_type(&right) {
                        return right;
                    }
                    if operator == Subtract && left == right {
                        return self.corlib("Int32");
                    }
                }
                _ => {}
            }
        }

        // user-defined operators (Vector3 + Vector3, a struct's own `+`, ...)
        if let Some(result) = self.user_defined_binary(operator, &left, &right, &span, node) {
            return result;
        }

        let _ = literal;
        let kind = SemanticErrorKind::InvalidOperator {
            left: self.describe(&left),
            right: Some(self.describe(&right)),
        };
        self.error(kind, span);
        Type::Error
    }

    fn numeric_binary(
        &mut self,
        operator: BinaryOperator,
        left: NumericKind,
        right: NumericKind,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        use BinaryOperator::*;
        use NumericKind::*;

        let shift = matches!(operator, LeftShift | RightShift | UnsignedRightShift);
        let promoted = if shift {
            (left.is_integral() && right.unary_promoted() == Int32).then_some(left.unary_promoted())
        } else {
            left.binary_promoted(right)
        };
        let valid_operator = matches!(
            operator,
            Add | Subtract
                | Multiply
                | Divide
                | Modulo
                | LessThan
                | GreaterThan
                | LessThanEqual
                | GreaterThanEqual
                | Equal
                | NotEqual
        ) || (matches!(operator, BitwiseAnd | BitwiseOr | BitwiseXor)
            && left.is_integral()
            && right.is_integral())
            || shift;
        let Some(promoted) = promoted.filter(|_| valid_operator) else {
            self.error(
                SemanticErrorKind::InvalidOperator {
                    left: left.corlib_name().to_string(),
                    right: Some(right.corlib_name().to_string()),
                },
                span,
            );
            return Type::Error;
        };
        if let Some(node) = node {
            let left = self.corlib(promoted.corlib_name());
            let right = if shift {
                self.corlib("Int32")
            } else {
                left.clone()
            };
            self.numeric_promotions.insert(node, (left, right));
        }
        if matches!(
            operator,
            LessThan | GreaterThan | LessThanEqual | GreaterThanEqual | Equal | NotEqual
        ) {
            self.corlib("Boolean")
        } else {
            self.corlib(promoted.corlib_name())
        }
    }

    fn binary_operator_name(operator: BinaryOperator) -> Option<&'static str> {
        use BinaryOperator::*;
        Some(match operator {
            Add => "op_Addition",
            Subtract => "op_Subtraction",
            Multiply => "op_Multiply",
            Divide => "op_Division",
            Modulo => "op_Modulus",
            BitwiseAnd => "op_BitwiseAnd",
            BitwiseOr => "op_BitwiseOr",
            BitwiseXor => "op_ExclusiveOr",
            LeftShift => "op_LeftShift",
            RightShift => "op_RightShift",
            UnsignedRightShift => "op_UnsignedRightShift",
            Equal => "op_Equality",
            NotEqual => "op_Inequality",
            LessThan => "op_LessThan",
            GreaterThan => "op_GreaterThan",
            LessThanEqual => "op_LessThanOrEqual",
            GreaterThanEqual => "op_GreaterThanOrEqual",
            LogicalAnd | LogicalOr | Coalesce => return None,
        })
    }

    /// Whether either operand's type declares this operator at all — when
    /// it does, `==` on two references is the declared one, not identity.
    fn has_user_operator(&self, operator: BinaryOperator, left: &Type, right: &Type) -> bool {
        let Some(name) = Self::binary_operator_name(operator) else {
            return false;
        };
        !self
            .operator_candidates(name, &[left.clone(), right.clone()], 2)
            .is_empty()
    }

    /// The declared operators named `name` on the operand types (and their
    /// bases), with this many parameters.
    fn operator_candidates(
        &self,
        name: &str,
        operands: &[Type],
        parameter_count: usize,
    ) -> Vec<MemberCandidate> {
        let system = self.system();
        let mut out: Vec<MemberCandidate> = Vec::new();
        let mut seen: Vec<Type> = Vec::new();
        for operand in operands {
            let ty = match operand {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            if seen.contains(&ty) {
                continue;
            }
            seen.push(ty.clone());
            for candidate in system.members_named(&ty, name) {
                let takes = matches!(
                    &candidate.signature,
                    Some(MemberSignature::Function(function))
                        if function.parameters.len() == parameter_count
                );
                let duplicate = out.iter().any(|existing| {
                    matches!(
                        (&existing.origin, &candidate.origin),
                        (MemberOrigin::Source(a), MemberOrigin::Source(b)) if a == b
                    ) || (existing.declaring_type == candidate.declaring_type
                        && existing.signature == candidate.signature
                        && !matches!(candidate.origin, MemberOrigin::Source(_)))
                });
                // metadata spells an operator as a static `op_*` method
                if matches!(candidate.kind, SymbolKind::Operator | SymbolKind::Method)
                    && candidate.is_static
                    && takes
                    && !duplicate
                {
                    out.push(candidate);
                }
            }
        }
        out
    }

    /// Overload resolution among the declared operators named `name` for
    /// these operand types (§12.4.5); the chosen one is recorded on `node`
    /// for the code generator. `None` when none applies.
    fn resolve_user_operator(
        &mut self,
        name: &'static str,
        operands: &[Type],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let candidates = self.operator_candidates(name, operands, operands.len());
        if candidates.is_empty() {
            return None;
        }
        let arguments: Vec<CallArgument<'ast>> = operands
            .iter()
            .map(|ty| CallArgument {
                shape: ArgumentShape::Value(ty.clone()),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                is_receiver: false,
                span: span.clone(),
            })
            .collect();
        let receiver_display = self.describe(&operands[0]);
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: true,
            name,
            receiver: None,
            allow_extensions: false,
            receiver_display,
            span: span.clone(),
        };
        match self.attempt_call(&group, &arguments) {
            AttemptOutcome::Selected(selected) => {
                self.record_call(node, &group, &selected, false);
                Some(selected.signature.return_type.clone())
            }
            AttemptOutcome::Ambiguous => {
                self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                Some(Type::Error)
            }
            AttemptOutcome::NoMatch { .. } => None,
        }
    }

    fn user_defined_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Type,
        right: &Type,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let name = Self::binary_operator_name(operator)?;
        self.resolve_user_operator(name, &[left.clone(), right.clone()], span, node)
    }

    fn user_defined_unary(
        &mut self,
        operator: UnaryOperator,
        operand: &Type,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Type> {
        let name = match operator {
            UnaryOperator::Plus => "op_UnaryPlus",
            UnaryOperator::Minus => "op_UnaryNegation",
            UnaryOperator::Not => "op_LogicalNot",
            UnaryOperator::BitwiseNot => "op_OnesComplement",
            UnaryOperator::PreIncrement => "op_Increment",
            UnaryOperator::PreDecrement => "op_Decrement",
            _ => return None,
        };
        self.resolve_user_operator(name, std::slice::from_ref(operand), span, node)
    }

    /// `x++` / `x--` on a type of the user's: its `op_Increment` /
    /// `op_Decrement`, recorded on the postfix node. Numeric and enum
    /// operands keep the built-in meaning.
    pub(super) fn postfix_step_type(
        &mut self,
        increment: bool,
        operand: Type,
        span: &Range<usize>,
        node: EntityID,
    ) -> Type {
        if let Some(inner) = self.nullable_value(&operand) {
            let underlying = self.postfix_step_type(increment, inner, span, node);
            return match underlying {
                Type::Error => Type::Error,
                underlying => Type::Nullable(Box::new(underlying)),
            };
        }
        let system = self.system();
        if matches!(operand, Type::Error | Type::Dynamic)
            || system.numeric_kind(&operand).is_some()
            || system.is_enum_type(&operand)
        {
            return operand;
        }
        let name = if increment {
            "op_Increment"
        } else {
            "op_Decrement"
        };
        match self.resolve_user_operator(name, std::slice::from_ref(&operand), span, Some(node)) {
            Some(result) => result,
            None => {
                let kind = SemanticErrorKind::InvalidOperator {
                    left: self.describe(&operand),
                    right: None,
                };
                self.error(kind, span.clone());
                Type::Error
            }
        }
    }
}
