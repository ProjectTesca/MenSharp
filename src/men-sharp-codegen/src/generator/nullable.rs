//! `Nullable<T>` — `int?`, `Vector3?`, `MyStruct?`.
//!
//! .NET boxes a `T?` as the `T` itself or as null, and that is the whole
//! representation here: a `T?` lives in a `SystemObject` slot holding the
//! boxed value or null. `HasValue` is a null test, `Value` a null test plus
//! a copy into a `T`-typed slot (the heap unboxes on that copy), and the
//! lifted operators — `x + 1`, `a < b`, `x == null`, `-x`, `x++` on a `T?`
//! — test for null, run `T`'s own operator on the unboxed values, and box
//! the result. `??` and `?.` are the same null test in expression form.
//!
//! `string?` and other reference annotations are not this: they are the
//! type itself, and everything here leaves them alone.

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// The `T` of a `T?` that is a `Nullable<T>` (a value type's), in either
    /// spelling. `None` for `string?` and other reference annotations.
    pub(super) fn nullable_inner(&self, ty: &Type) -> Option<Type> {
        let inner = match ty {
            Type::Nullable(inner) => (**inner).clone(),
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } if arguments.len() == 1
                && self.external.find_type(&["System"], "Nullable", 1) == Some(*id) =>
            {
                arguments[0].clone()
            }
            _ => return None,
        };
        (matches!(inner, Type::Named { .. }) && !self.is_reference_type(&inner)).then_some(inner)
    }

    /// Is this `System.Nullable`, the external type whose members `x.Value`
    /// and friends bind to?
    pub(super) fn is_nullable_owner(&self, id: men_sharp_semantics::ExternalTypeId) -> bool {
        self.external.find_type(&["System"], "Nullable", 1) == Some(id)
    }

    /// `ReferenceEquals(slot, null)`, as a bool slot.
    pub(super) fn is_null(&mut self, ctx: &Ctx<'ast>, slot: DataId, span: Range<usize>) -> DataId {
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let out = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[slot, null, out],
            span,
        );
        out
    }

    /// The value inside a non-null `T?`: a `T`-typed slot the boxed value
    /// is copied into (the heap unboxes on the copy). A struct of the
    /// user's is cloned, as `Value` returns a copy.
    pub(super) fn unbox_nullable(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        inner: &Type,
        span: Range<usize>,
    ) -> DataId {
        if self.is_source_struct(inner) {
            return self.clone_struct(ctx, slot, inner, span);
        }
        let typed = self.temp_for(inner);
        self.copy(slot, typed);
        typed
    }

    /// `x.Value`: the value, or `InvalidOperationException` on a null.
    pub(super) fn nullable_value(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        inner: &Type,
        span: Range<usize>,
    ) -> DataId {
        let is_null = self.is_null(ctx, slot, span.clone());
        let ok = self.fresh_label("nullable_has_value");
        self.program.code.push(Op::Push(is_null));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        let message = self.string_constant("Nullable object must have a value.");
        self.throw_new(
            ctx,
            &["System", "InvalidOperationException"],
            Some(message),
            span.clone(),
        );
        self.program.code.push(Op::Label(ok));
        self.unbox_nullable(ctx, slot, inner, span)
    }

    /// `x.HasValue` / `x.Value` on a `T?`: computed, read-only places.
    pub(super) fn try_nullable_member(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: &Option<(DataId, Type)>,
        name: &str,
        span: Range<usize>,
    ) -> Option<Place> {
        let (slot, receiver_type) = receiver.as_ref()?;
        let inner = self.nullable_inner(receiver_type)?;
        let (slot, ty) = match name {
            "HasValue" => {
                let is_null = self.is_null(ctx, *slot, span.clone());
                let out = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                    &[is_null, out],
                    span,
                );
                (out, self.corlib_type("Boolean"))
            }
            "Value" => (self.nullable_value(ctx, *slot, &inner, span), inner),
            _ => return None,
        };
        Some(Place::ReadOnly {
            slot,
            ty,
            what: name.to_string(),
        })
    }

    /// `x.GetValueOrDefault()`, `x.GetValueOrDefault(fallback)`, and the
    /// `object` members on a `T?` receiver — `ToString()` of a null is
    /// `""`, `GetHashCode()` 0, `Equals(null)` true, as `Nullable<T>` has
    /// them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_nullable_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        owner: men_sharp_semantics::ExternalTypeId,
        name: &str,
        receiver: &Option<(DataId, Type)>,
        values: &[DataId],
        return_type: &Type,
        span: Range<usize>,
    ) -> Option<Piece> {
        if !self.is_nullable_owner(owner) {
            return None;
        }
        let (slot, receiver_type) = receiver.as_ref()?;
        let slot = *slot;
        let inner = self.nullable_inner(receiver_type)?;
        match name {
            "GetValueOrDefault" => {
                let result = self.temp_for(&inner);
                let fallback = match values.first() {
                    Some(&fallback) => fallback,
                    None => self.default_value_in(ctx, &inner, span.clone()),
                };
                let is_null = self.is_null(ctx, slot, span.clone());
                let use_fallback = self.fresh_label("nullable_default");
                let end = self.fresh_label("nullable_default_end");
                self.jump_if(is_null, use_fallback);
                let value = self.unbox_nullable(ctx, slot, &inner, span.clone());
                self.copy(value, result);
                self.program.code.push(Op::Jump(Target::Label(end)));
                self.program.code.push(Op::Label(use_fallback));
                self.copy(fallback, result);
                self.program.code.push(Op::Label(end));
                Some(Piece::Value(result, inner))
            }
            "ToString" => {
                // `Convert.ToString(object)`: "" for null, the value's own
                // `ToString` otherwise
                let out = self.temp("SystemString");
                self.call_extern(
                    ctx,
                    "SystemConvert.__ToString__SystemObject__SystemString",
                    &[slot, out],
                    span,
                );
                Some(Piece::Value(out, self.corlib_type("String")))
            }
            "Equals" => {
                let other = *values.first()?;
                let out = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean",
                    &[slot, other, out],
                    span,
                );
                Some(Piece::Value(out, self.corlib_type("Boolean")))
            }
            "GetHashCode" => {
                let result = self.temp("SystemInt32");
                let zero = self.int_constant(0);
                self.copy(zero, result);
                let is_null = self.is_null(ctx, slot, span.clone());
                let end = self.fresh_label("nullable_hash_end");
                self.jump_if(is_null, end);
                let value = self.unbox_nullable(ctx, slot, &inner, span.clone());
                let hashed = self.temp("SystemInt32");
                self.call_extern(
                    ctx,
                    "SystemObject.__GetHashCode__SystemInt32",
                    &[value, hashed],
                    span,
                );
                self.copy(hashed, result);
                self.program.code.push(Op::Label(end));
                Some(Piece::Value(result, return_type.clone()))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------- conversions

    /// Conversions to and from a `T?`, or `None` when neither side is one.
    pub(super) fn convert_nullable(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        from: &Type,
        to: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let from_inner = self.nullable_inner(from);
        let to_inner = self.nullable_inner(to);
        match (from_inner, to_inner) {
            // `int? x = 5`, `long? y = 5`: the value converts to `T`, and
            // the copy into the object slot boxes it
            (None, Some(to_inner)) => {
                // `(int?)boxed`: an object is the boxed value or null already
                if matches!(from, Type::Null)
                    || self.is_reference_type(from)
                    || self.heap_type(from) == "SystemObject"
                {
                    return Some(source);
                }
                Some(self.convert(ctx, source, from, &to_inner, span))
            }
            // `long? y = intNullable`: null stays null, else the value
            // converts
            (Some(from_inner), Some(to_inner)) => {
                if from_inner == to_inner {
                    return Some(source);
                }
                let result = self.temp("SystemObject");
                let null = self.constant("SystemObject", "null", HeapInit::Null);
                self.copy(null, result);
                let is_null = self.is_null(ctx, source, span.clone());
                let end = self.fresh_label("nullable_convert_end");
                self.jump_if(is_null, end);
                let value = self.unbox_nullable(ctx, source, &from_inner, span.clone());
                let converted = self.convert(ctx, value, &from_inner, &to_inner, span);
                self.copy(converted, result);
                self.program.code.push(Op::Label(end));
                Some(result)
            }
            // `(int)x`: the value, or InvalidOperationException; `(object)x`
            // and `(Nullable<int>)x` are the reference itself
            (Some(from_inner), None) => {
                if self.is_reference_type(to)
                    || self.heap_type(to) == "SystemObject"
                    || matches!(to, Type::Nullable(_))
                {
                    return Some(source);
                }
                let value = self.nullable_value(ctx, source, &from_inner, span.clone());
                Some(self.convert(ctx, value, &from_inner, to, span))
            }
            (None, None) => None,
        }
    }

    // --------------------------------------------------------- operators

    /// A lifted binary operator: null in → null out (or false, for an
    /// ordering; equal, for two nulls), else `T`'s operator on the values.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lift_binary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: BinaryOperator,
        left: (DataId, &Type),
        right: (DataId, &Type),
        result_type: &Type,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Option<DataId> {
        use BinaryOperator::*;
        let left_inner = self.nullable_inner(left.1);
        let right_inner = self.nullable_inner(right.1);
        let comparison = matches!(
            operator,
            Equal | NotEqual | LessThan | GreaterThan | LessThanEqual | GreaterThanEqual
        );

        // `x == null` / `x != null`: a null test and nothing else
        if matches!(operator, Equal | NotEqual)
            && (matches!(left.1, Type::Null) || matches!(right.1, Type::Null))
        {
            let value = if matches!(left.1, Type::Null) {
                right.0
            } else {
                left.0
            };
            let is_null = self.is_null(ctx, value, span.clone());
            if operator == Equal {
                return Some(is_null);
            }
            let out = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                &[is_null, out],
                span,
            );
            return Some(out);
        }

        let underlying_result = if comparison {
            self.corlib_type("Boolean")
        } else {
            match self.nullable_inner(result_type) {
                Some(inner) => inner,
                None => result_type.clone(),
            }
        };
        let result = if comparison {
            self.temp("SystemBoolean")
        } else {
            self.temp("SystemObject")
        };
        let null_case = self.fresh_label("lifted_null");
        let end = self.fresh_label("lifted_end");
        // the nullable operands tested, then unboxed
        for (slot, inner) in [(left.0, &left_inner), (right.0, &right_inner)] {
            if inner.is_some() {
                let is_null = self.is_null(ctx, slot, span.clone());
                self.jump_if(is_null, null_case);
            }
        }
        let left_value = match &left_inner {
            Some(inner) => (
                self.unbox_nullable(ctx, left.0, inner, span.clone()),
                inner.clone(),
            ),
            None => (left.0, left.1.clone()),
        };
        let right_value = match &right_inner {
            Some(inner) => (
                self.unbox_nullable(ctx, right.0, inner, span.clone()),
                inner.clone(),
            ),
            None => (right.0, right.1.clone()),
        };
        let value = self.emit_binary_operator(
            ctx,
            operator,
            (left_value.0, &left_value.1),
            (right_value.0, &right_value.1),
            &underlying_result,
            span.clone(),
            node,
        )?;
        self.copy(value, result);
        self.program.code.push(Op::Jump(Target::Label(end)));

        self.program.code.push(Op::Label(null_case));
        match operator {
            // two nulls are equal; a null and a value are not
            Equal | NotEqual => {
                let both = self.temp("SystemBoolean");
                let left_null = self.is_null(ctx, left.0, span.clone());
                let right_null = self.is_null(ctx, right.0, span.clone());
                self.call_extern(
                    ctx,
                    "SystemBoolean.__op_LogicalAnd__SystemBoolean_SystemBoolean__SystemBoolean",
                    &[left_null, right_null, both],
                    span.clone(),
                );
                if operator == Equal {
                    self.copy(both, result);
                } else {
                    self.call_extern(
                        ctx,
                        "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                        &[both, result],
                        span,
                    );
                }
            }
            LessThan | GreaterThan | LessThanEqual | GreaterThanEqual => {
                let no = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
                self.copy(no, result);
            }
            _ => {
                let null = self.constant("SystemObject", "null", HeapInit::Null);
                self.copy(null, result);
            }
        }
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// A lifted unary operator: null in, null out.
    pub(super) fn lift_unary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: UnaryOperator,
        operand: DataId,
        inner: &Type,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Option<DataId> {
        let result = self.temp("SystemObject");
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        self.copy(null, result);
        let end = self.fresh_label("lifted_unary_end");
        let is_null = self.is_null(ctx, operand, span.clone());
        self.jump_if(is_null, end);
        let value = self.unbox_nullable(ctx, operand, inner, span.clone());
        let applied = self.apply_unary(ctx, operator, value, inner, inner, span, node)?;
        self.copy(applied, result);
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// `left ?? right`: `left` unless it is null, then `right` — each
    /// converted to the expression's type.
    pub(super) fn lower_coalesce(
        &mut self,
        ctx: &mut Ctx<'ast>,
        binary: &'ast men_sharp_parser::ast::BinaryExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let result_type = self.type_of(ctx, whole);
        let result = self.temp_for(&result_type);
        let left_type = self.type_of(ctx, &binary.left);
        let left = self.lower_expression(ctx, &binary.left)?;
        let use_right = self.fresh_label("coalesce_right");
        let end = self.fresh_label("coalesce_end");
        let is_null = self.is_null(ctx, left, binary.span.clone());
        self.jump_if(is_null, use_right);
        // a `T?` left with a `T` result: the value, known non-null here
        let value = match (
            self.nullable_inner(&left_type),
            self.nullable_inner(&result_type),
        ) {
            (Some(inner), None) => {
                let unboxed = self.unbox_nullable(ctx, left, &inner, binary.span.clone());
                self.convert(ctx, unboxed, &inner, &result_type, binary.span.clone())
            }
            _ => self.convert(ctx, left, &left_type, &result_type, binary.span.clone()),
        };
        self.copy(value, result);
        self.program.code.push(Op::Jump(Target::Label(end)));
        self.program.code.push(Op::Label(use_right));
        let right = binary.right.as_ref().ok()?;
        if let Some(value) = self.owned_value_as(ctx, right, &result_type) {
            self.copy(value, result);
        }
        self.program.code.push(Op::Label(end));
        Some(result)
    }
}
