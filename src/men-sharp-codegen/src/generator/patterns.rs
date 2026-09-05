//! Pattern matching: `x is <pattern>`. Every pattern lowers to a boolean
//! slot, binding its pattern variables (in the enclosing scope, where the
//! checker declared them) on the path where it matched.

use men_sharp_parser::ast::{Pattern, RelationalOperator, VariableDesignation};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// `value is pattern`, as a bool slot.
    pub(super) fn lower_pattern(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        value_type: &Type,
        pattern: &'ast Pattern<'ast, 'ast>,
    ) -> Option<DataId> {
        let span = pattern.span();
        let value_type = self.substitute(value_type, &ctx.key.bindings);
        match pattern {
            Pattern::Discard(_) => Some(self.bool_constant(true)),
            Pattern::Var { designation, .. } => {
                match designation {
                    Ok(VariableDesignation::Single(name)) => {
                        let local = self.temp_for(&value_type);
                        self.copy(value, local);
                        self.bind_pattern_variable(ctx, name.value, local, value_type);
                    }
                    Ok(VariableDesignation::Discard(_)) => {}
                    // `var (a, b)`: a deconstruction that always matches
                    Ok(designation @ VariableDesignation::Parenthesized { .. }) => {
                        self.bind_designation(ctx, designation, value, &value_type, &None, span);
                    }
                    Err(()) => return None,
                }
                Some(self.bool_constant(true))
            }
            Pattern::Declaration {
                pattern_type,
                designation,
                ..
            } => {
                // `x is Color.Red`: a bare dotted name the checker bound to a
                // constant member rather than a type
                if designation.is_none()
                    && let Some(ResolvedTarget::Member(member)) = self
                        .bodies
                        .targets
                        .get(&EntityID::from(pattern_type))
                        .cloned()
                {
                    let Some((constant, constant_type)) = self.member_constant(&member) else {
                        self.error(
                            ctx,
                            Message::key("codegen.this_pattern_is_not_a_constant"),
                            span,
                        );
                        return None;
                    };
                    return self.pattern_equality(
                        ctx,
                        value,
                        &value_type,
                        constant,
                        &constant_type,
                        span,
                    );
                }
                let to = self
                    .bodies
                    .resolved_types
                    .get(&EntityID::from(pattern_type))
                    .cloned()
                    .map(|ty| self.substitute(&ty, &ctx.key.bindings))?;
                let result = self.lower_runtime_type_test(ctx, value, &to, span.clone())?;
                if let Some(name) = designation {
                    let local = self.temp_for(&to);
                    let skip = self.fresh_label("is_bind_skip");
                    self.program.code.push(Op::Push(result));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
                    self.copy(value, local);
                    self.program.code.push(Op::Label(skip));
                    self.bind_pattern_variable(ctx, name.value, local, to);
                }
                Some(result)
            }
            Pattern::Constant(expression) => {
                let constant_type = self.type_of(ctx, expression);
                let constant = self.lower_expression(ctx, expression)?;
                self.pattern_equality(ctx, value, &value_type, constant, &constant_type, span)
            }
            Pattern::Relational {
                operator,
                value: bound,
                ..
            } => {
                let bound_expression = bound.as_ref().ok()?;
                let bound_type = self.type_of(ctx, bound_expression);
                let bound = self.lower_expression(ctx, bound_expression)?;
                let operator = match operator.value {
                    RelationalOperator::LessThan => BinaryOperator::LessThan,
                    RelationalOperator::GreaterThan => BinaryOperator::GreaterThan,
                    RelationalOperator::LessThanEqual => BinaryOperator::LessThanEqual,
                    RelationalOperator::GreaterThanEqual => BinaryOperator::GreaterThanEqual,
                };
                self.pattern_comparison(ctx, operator, value, &value_type, bound, &bound_type, span)
            }
            Pattern::Not { pattern, .. } => {
                let inner = self.lower_pattern(ctx, value, &value_type, pattern.as_ref().ok()?)?;
                let out = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                    &[inner, out],
                    span,
                );
                Some(out)
            }
            Pattern::Parenthesized { pattern, .. } => {
                self.lower_pattern(ctx, value, &value_type, pattern.as_ref().ok()?)
            }
            Pattern::And { left, right, .. } | Pattern::Or { left, right, .. } => {
                let is_and = matches!(pattern, Pattern::And { .. });
                let result = self.temp("SystemBoolean");
                let end = self.fresh_label(if is_and {
                    "pattern_and_end"
                } else {
                    "pattern_or_end"
                });
                let first = self.lower_pattern(ctx, value, &value_type, left)?;
                self.copy(first, result);
                if is_and {
                    // false && _ → false
                    self.program.code.push(Op::Push(first));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                } else {
                    // true || _ → true
                    self.jump_if(first, end);
                }
                let second = self.lower_pattern(ctx, value, &value_type, right.as_ref().ok()?)?;
                self.copy(second, result);
                self.program.code.push(Op::Label(end));
                Some(result)
            }
            Pattern::Property {
                pattern_type,
                subpatterns,
                designation,
                ..
            } => {
                let result = self.temp("SystemBoolean");
                let no = self.bool_constant(false);
                self.copy(no, result);
                let end = self.fresh_label("property_pattern_end");

                // the subject: the value as the named type, or as it is —
                // and never null (`null is { }` is false)
                let (subject, subject_type) = match pattern_type {
                    Some(pattern_type) => {
                        let to = self
                            .bodies
                            .resolved_types
                            .get(&EntityID::from(pattern_type))
                            .cloned()
                            .map(|ty| self.substitute(&ty, &ctx.key.bindings))?;
                        let test = self.lower_runtime_type_test(ctx, value, &to, span.clone())?;
                        self.program.code.push(Op::Push(test));
                        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                        let typed = self.temp_for(&to);
                        self.copy(value, typed);
                        (typed, to)
                    }
                    None => {
                        if self.is_reference_type(&value_type)
                            || self.heap_type(&value_type) == "SystemObject"
                        {
                            let null = self.constant("SystemObject", "null", HeapInit::Null);
                            let is_null = self.temp("SystemBoolean");
                            self.call_extern(
                                ctx,
                                "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
                                &[value, null, is_null],
                                span.clone(),
                            );
                            self.jump_if(is_null, end);
                        }
                        (value, value_type.clone())
                    }
                };

                for subpattern in *subpatterns {
                    let Some(ResolvedTarget::Member(member)) = self
                        .bodies
                        .targets
                        .get(&EntityID::from(subpattern))
                        .cloned()
                    else {
                        self.error(
                            ctx,
                            Message::key("codegen.a0_is_not_a_field_or_property")
                                .arg("a0", subpattern.name.value),
                            subpattern.span.clone(),
                        );
                        return None;
                    };
                    let place = self.member_place(
                        ctx,
                        &member,
                        Some((subject, subject_type.clone())),
                        subpattern.span.clone(),
                    );
                    let (member_value, member_type) =
                        self.read_place(ctx, place, subpattern.span.clone())?;
                    let matched = self.lower_pattern(
                        ctx,
                        member_value,
                        &member_type,
                        subpattern.pattern.as_ref().ok()?,
                    )?;
                    self.program.code.push(Op::Push(matched));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                }

                if let Some(name) = designation {
                    let local = self.temp_for(&subject_type);
                    self.copy(subject, local);
                    self.bind_pattern_variable(ctx, name.value, local, subject_type);
                }
                let yes = self.bool_constant(true);
                self.copy(yes, result);
                self.program.code.push(Op::Label(end));
                Some(result)
            }
            // `(0, var y)` and `Point(0, var y)`: the parts, position by
            // position — a tuple's elements, or what `Deconstruct` writes
            Pattern::Positional {
                pattern_type,
                subpatterns,
                property_subpatterns: [],
                designation,
                ..
            } => {
                let result = self.temp("SystemBoolean");
                let no = self.bool_constant(false);
                self.copy(no, result);
                let end = self.fresh_label("positional_pattern_end");

                // `Point(...)`: the value has to be one first
                let (subject, subject_type) = match pattern_type {
                    Some(pattern_type) => {
                        let to = self
                            .bodies
                            .resolved_types
                            .get(&EntityID::from(pattern_type))
                            .cloned()
                            .map(|ty| self.substitute(&ty, &ctx.key.bindings))?;
                        let test = self.lower_runtime_type_test(ctx, value, &to, span.clone())?;
                        self.program.code.push(Op::Push(test));
                        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                        let typed = self.temp_for(&to);
                        self.copy(value, typed);
                        (typed, to)
                    }
                    None => (value, value_type.clone()),
                };

                let elements = Self::tuple_elements(&subject_type)
                    .map(<[men_sharp_semantics::TupleElement]>::to_vec)
                    .unwrap_or_default();
                let parts = self.deconstructed_parts(
                    ctx,
                    subject,
                    &subject_type,
                    EntityID::from(pattern),
                    subpatterns.len(),
                    span.clone(),
                )?;
                for (position, subpattern) in subpatterns.iter().enumerate() {
                    // `(x: 0, y: 1)`: a name picks the part by name
                    let index = match &subpattern.name {
                        Some(name) => {
                            men_sharp_semantics::tuple_element_index(&elements, name.value)?
                        }
                        None => position,
                    };
                    let (read, part) = parts.get(index)?.clone();
                    let matched = self.lower_pattern(ctx, read, &part, &subpattern.pattern)?;
                    self.program.code.push(Op::Push(matched));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                }

                if let Some(name) = designation {
                    let local = self.temp_for(&subject_type);
                    self.copy(subject, local);
                    self.bind_pattern_variable(ctx, name.value, local, subject_type);
                }
                let yes = self.bool_constant(true);
                self.copy(yes, result);
                self.program.code.push(Op::Label(end));
                Some(result)
            }
            // `[1, 2, ..]`, `[first, .. var rest, last]`: an array matched
            // by length, then position by position from both ends
            Pattern::List {
                elements,
                designation,
                ..
            } => {
                let Type::Array { element, rank: 1 } = &value_type else {
                    self.error(
                        ctx,
                        Message::key("codegen.a_list_pattern_matches_an_array"),
                        span,
                    );
                    return None;
                };
                let element_type = self.substitute(element, &ctx.key.bindings);
                let result = self.temp("SystemBoolean");
                let no = self.bool_constant(false);
                self.copy(no, result);
                let end = self.fresh_label("list_pattern_end");

                self.check_not_null(ctx, value, span.clone());
                let length = self.array_length(ctx, value, &value_type, span.clone());
                let slice = elements
                    .iter()
                    .position(|element| matches!(element, Pattern::Slice { .. }));
                let fixed = self.int_constant(elements.len() as i32 - i32::from(slice.is_some()));
                let condition = self.temp("SystemBoolean");
                let comparison = if slice.is_some() {
                    "op_GreaterThanOrEqual"
                } else {
                    "op_Equality"
                };
                self.call_extern(
                    ctx,
                    &format!("SystemInt32.__{comparison}__SystemInt32_SystemInt32__SystemBoolean"),
                    &[length, fixed, condition],
                    span.clone(),
                );
                self.program.code.push(Op::Push(condition));
                self.program.code.push(Op::JumpIfFalse(Target::Label(end)));

                let int32 = self.corlib_type("Int32");
                let after = slice.map_or(0, |at| elements.len() - at - 1);
                for (position, written) in elements.iter().enumerate() {
                    // before the slice the index counts from the front,
                    // after it from the back
                    let index = match slice {
                        Some(at) if position == at => {
                            let Pattern::Slice { pattern, .. } = written else {
                                continue;
                            };
                            let Some(rest) = pattern else {
                                continue;
                            };
                            let start = self.int_constant(at as i32);
                            let tail = self.int_constant(after as i32);
                            let taken = self.emit_binary_operator(
                                ctx,
                                BinaryOperator::Subtract,
                                (length, &int32),
                                (tail, &int32),
                                &int32,
                                span.clone(),
                                None,
                            )?;
                            let taken = self.emit_binary_operator(
                                ctx,
                                BinaryOperator::Subtract,
                                (taken, &int32),
                                (start, &int32),
                                &int32,
                                span.clone(),
                                None,
                            )?;
                            let part = self.copy_range(
                                ctx,
                                value,
                                &value_type,
                                start,
                                taken,
                                span.clone(),
                            );
                            let matched = self.lower_pattern(ctx, part, &value_type, rest)?;
                            self.program.code.push(Op::Push(matched));
                            self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                            continue;
                        }
                        Some(at) if position > at => {
                            let back = self.int_constant((elements.len() - position) as i32);
                            self.emit_binary_operator(
                                ctx,
                                BinaryOperator::Subtract,
                                (length, &int32),
                                (back, &int32),
                                &int32,
                                span.clone(),
                                None,
                            )?
                        }
                        _ => self.int_constant(position as i32),
                    };
                    let read =
                        self.array_get(ctx, value, index, &value_type, &element_type, span.clone());
                    let matched = self.lower_pattern(ctx, read, &element_type, written)?;
                    self.program.code.push(Op::Push(matched));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
                }

                if let Some(name) = designation {
                    let local = self.temp_for(&value_type);
                    self.copy(value, local);
                    self.bind_pattern_variable(ctx, name.value, local, value_type);
                }
                let yes = self.bool_constant(true);
                self.copy(yes, result);
                self.program.code.push(Op::Label(end));
                Some(result)
            }
            Pattern::Positional { .. } | Pattern::Slice { .. } => {
                self.error(
                    ctx,
                    Message::key("codegen.a_positional_pattern_works_on_a_tuple"),
                    span,
                );
                None
            }
        }
    }

    /// A pattern variable, visible from here on in the enclosing scope.
    fn bind_pattern_variable(
        &mut self,
        ctx: &mut Ctx<'ast>,
        name: &'ast str,
        slot: DataId,
        ty: Type,
    ) {
        self.bind_local(ctx, name, slot, ty);
    }

    pub(super) fn bool_constant(&mut self, value: bool) -> DataId {
        self.constant(
            "SystemBoolean",
            if value { "true" } else { "false" },
            HeapInit::Boolean(value),
        )
    }

    /// `value is <constant>`: equality with a constant, under the rules of
    /// the constant pattern — `null` is a reference test, a number compares
    /// as the value's own type, and an `object` (or other reference) subject
    /// compares by `object.Equals` (value equality for a boxed primitive or
    /// a string, false for anything else, null included).
    fn pattern_equality(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        value_type: &Type,
        constant: DataId,
        constant_type: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let out = self.temp("SystemBoolean");
        if matches!(constant_type, Type::Null) {
            if self.is_reference_type(value_type) || self.heap_type(value_type) == "SystemObject" {
                let null = self.constant("SystemObject", "null", HeapInit::Null);
                self.call_extern(
                    ctx,
                    "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
                    &[value, null, out],
                    span,
                );
                return Some(out);
            }
            // a non-nullable value is never null
            return Some(self.bool_constant(false));
        }
        let same_shape = self.extern_type_name(value_type).is_some()
            && self.extern_type_name(value_type) == self.extern_type_name(constant_type);
        let both_numeric =
            self.numeric_udon_type(value_type) && self.numeric_udon_type(constant_type);
        if same_shape || both_numeric {
            let converted = if same_shape {
                constant
            } else {
                self.convert(ctx, constant, constant_type, value_type, span.clone())
            };
            let boolean = self.corlib_type("Boolean");
            return self.emit_binary_operator(
                ctx,
                BinaryOperator::Equal,
                (value, value_type),
                (converted, value_type),
                &boolean,
                span,
                None,
            );
        }
        // `object` and friends: static Object.Equals — null-safe, value
        // equality for boxed primitives and strings
        self.call_extern(
            ctx,
            "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean",
            &[value, constant, out],
            span,
        );
        Some(out)
    }

    /// `value is > bound`: an ordering against a constant. On a subject that
    /// is not statically numeric (`object`), the value must first *be* the
    /// constant's type, as C# has it (`o is > 5` is false for a string).
    #[allow(clippy::too_many_arguments)]
    fn pattern_comparison(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: BinaryOperator,
        value: DataId,
        value_type: &Type,
        bound: DataId,
        bound_type: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let boolean = self.corlib_type("Boolean");
        if self.numeric_udon_type(value_type) {
            let converted =
                if self.extern_type_name(value_type) == self.extern_type_name(bound_type) {
                    bound
                } else {
                    self.convert(ctx, bound, bound_type, value_type, span.clone())
                };
            return self.emit_binary_operator(
                ctx,
                operator,
                (value, value_type),
                (converted, value_type),
                &boolean,
                span,
                None,
            );
        }
        if !self.numeric_udon_type(bound_type) {
            self.error(
                ctx,
                Message::key("codegen.a_relational_pattern_needs_a_numeric_constant"),
                span,
            );
            return None;
        }
        let result = self.temp("SystemBoolean");
        let no = self.bool_constant(false);
        self.copy(no, result);
        let end = self.fresh_label("relational_end");
        let test = self.lower_runtime_type_test(ctx, value, bound_type, span.clone())?;
        self.program.code.push(Op::Push(test));
        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
        let typed = self.temp_for(bound_type);
        self.copy(value, typed);
        if let Some(compared) = self.emit_binary_operator(
            ctx,
            operator,
            (typed, bound_type),
            (bound, bound_type),
            &boolean,
            span,
            None,
        ) {
            self.copy(compared, result);
        }
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// A type whose Udon slot holds a number.
    fn numeric_udon_type(&self, ty: &Type) -> bool {
        matches!(
            self.heap_type(ty).as_str(),
            "SystemInt32"
                | "SystemInt64"
                | "SystemUInt32"
                | "SystemUInt64"
                | "SystemInt16"
                | "SystemUInt16"
                | "SystemByte"
                | "SystemSByte"
                | "SystemSingle"
                | "SystemDouble"
                | "SystemDecimal"
                | "SystemChar"
        )
    }
}
