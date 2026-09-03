//! Statement and expression lowering.

use men_sharp_parser::ast::{
    ExpressionStatement, IfStatement, LocalVariableDeclaration, NewExpression, PostfixOperator,
    ReturnStatement, SwitchLabel, SwitchStatement, WhileStatement,
};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    // ---------------------------------------------------------- statements

    pub(super) fn lower_block(&mut self, ctx: &mut Ctx<'ast>, block: &'ast Block<'ast, 'ast>) {
        ctx.locals.push(HashMap::new());
        for statement in block.statements {
            self.lower_statement(ctx, statement);
        }
        ctx.locals.pop();
    }

    fn lower_statement(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast Statement<'ast, 'ast>) {
        self.emit_source_mark(ctx, &statement.span());
        match statement {
            Statement::Block(block) => self.lower_block(ctx, block),
            Statement::Empty { .. } => {}
            Statement::LocalVariable(declaration) => self.lower_local(ctx, declaration),
            Statement::Expression(ExpressionStatement { expression, .. }) => {
                self.lower_expression(ctx, expression);
            }
            Statement::If(statement) => self.lower_if(ctx, statement),
            Statement::While(statement) => self.lower_while(ctx, statement),
            Statement::DoWhile(statement) => {
                let body_label = self.fresh_label("do_body");
                let continue_label = self.fresh_label("do_continue");
                let break_label = self.fresh_label("do_break");
                self.program.code.push(Op::Label(body_label));
                ctx.loop_stack.push(BreakFrame::Loop {
                    continue_target: continue_label,
                    break_target: break_label,
                });
                if let Ok(body) = &statement.body {
                    self.lower_statement(ctx, body);
                }
                ctx.loop_stack.pop();
                self.program.code.push(Op::Label(continue_label));
                if let Ok(condition) = &statement.condition
                    && let Some(value) = self.lower_expression(ctx, condition)
                {
                    self.program.code.push(Op::Push(value));
                    self.program
                        .code
                        .push(Op::JumpIfFalse(Target::Label(break_label)));
                    self.program.code.push(Op::Jump(Target::Label(body_label)));
                }
                self.program.code.push(Op::Label(break_label));
            }
            Statement::For(statement) => {
                ctx.locals.push(HashMap::new());
                match &statement.initializer {
                    Some(ForInitializer::Declaration(declaration)) => {
                        self.lower_local(ctx, declaration)
                    }
                    Some(ForInitializer::Expressions(expressions)) => {
                        for expression in *expressions {
                            self.lower_expression(ctx, expression);
                        }
                    }
                    None => {}
                }
                let head = self.fresh_label("for_head");
                let continue_label = self.fresh_label("for_continue");
                let break_label = self.fresh_label("for_break");
                self.program.code.push(Op::Label(head));
                if let Some(condition) = &statement.condition
                    && let Some(value) = self.lower_expression(ctx, condition)
                {
                    self.program.code.push(Op::Push(value));
                    self.program
                        .code
                        .push(Op::JumpIfFalse(Target::Label(break_label)));
                }
                ctx.loop_stack.push(BreakFrame::Loop {
                    continue_target: continue_label,
                    break_target: break_label,
                });
                if let Ok(body) = &statement.body {
                    self.lower_statement(ctx, body);
                }
                ctx.loop_stack.pop();
                self.program.code.push(Op::Label(continue_label));
                for incrementor in statement.incrementors {
                    self.lower_expression(ctx, incrementor);
                }
                self.program.code.push(Op::Jump(Target::Label(head)));
                self.program.code.push(Op::Label(break_label));
                ctx.locals.pop();
            }
            Statement::Foreach(statement) => self.lower_foreach(ctx, statement),
            Statement::Switch(statement) => self.lower_switch(ctx, statement),
            Statement::Try(statement) => self.lower_try(ctx, statement),
            Statement::Throw(statement) => self.lower_throw_statement(ctx, statement),
            Statement::Return(ReturnStatement { value, span, .. }) => {
                if let Some(value) = value {
                    // a returned local or parameter is not copied: its slot
                    // is dead once the function returns. A field or element
                    // stays alive, so a struct read from one is copied
                    let is_local = match value {
                        Expression::Primary(primary) if primary.chain.is_empty() => {
                            match &primary.left {
                                PrimaryLeft::Identifier { name, .. } => {
                                    ctx.lookup(name.value).is_some()
                                }
                                _ => false,
                            }
                        }
                        _ => false,
                    };
                    let lowered = if is_local {
                        self.lower_expression(ctx, value)
                    } else {
                        self.owned_value(ctx, value)
                    };
                    // converted to the declared return type
                    let (_, return_type) = self.function_shape(&ctx.key);
                    let lowered = lowered.map(|slot| {
                        let from = self.type_of(ctx, value);
                        self.convert(ctx, slot, &from, &return_type, span.clone())
                    });
                    match (lowered, ctx.result) {
                        (Some(value), Some(result)) => self.copy(value, result),
                        (Some(_), None) => {}
                        (None, _) => {
                            let _ = span;
                        }
                    }
                }
                // leaving every `try` region: their `finally` blocks first
                self.emit_finally_copies(ctx, 0);
                self.program.code.push(Op::JumpIndirect(ctx.return_slot));
            }
            Statement::Break(statement) => {
                let frame =
                    ctx.loop_stack.iter().enumerate().rev().find_map(
                        |(index, frame)| match frame {
                            BreakFrame::Loop { break_target, .. }
                            | BreakFrame::Switch { break_target } => Some((index, *break_target)),
                            BreakFrame::Try { .. } => None,
                        },
                    );
                match frame {
                    Some((index, target)) => {
                        self.emit_finally_copies(ctx, index + 1);
                        self.program.code.push(Op::Jump(Target::Label(target)));
                    }
                    None => self.error(
                        ctx,
                        "`break` outside a loop or `switch`",
                        statement.span.clone(),
                    ),
                }
            }
            // `continue` skips over `switch` frames to the enclosing loop
            Statement::Continue(statement) => {
                let frame =
                    ctx.loop_stack.iter().enumerate().rev().find_map(
                        |(index, frame)| match frame {
                            BreakFrame::Loop {
                                continue_target, ..
                            } => Some((index, *continue_target)),
                            BreakFrame::Switch { .. } | BreakFrame::Try { .. } => None,
                        },
                    );
                match frame {
                    Some((index, target)) => {
                        self.emit_finally_copies(ctx, index + 1);
                        self.program.code.push(Op::Jump(Target::Label(target)));
                    }
                    None => self.error(ctx, "`continue` outside a loop", statement.span.clone()),
                }
            }
            other => {
                self.error(
                    ctx,
                    "this statement is not supported by the Udon backend yet",
                    other.span(),
                );
            }
        }
    }

    fn lower_local(
        &mut self,
        ctx: &mut Ctx<'ast>,
        declaration: &'ast LocalVariableDeclaration<'ast, 'ast>,
    ) {
        let declared = self
            .bodies
            .resolved_types
            .get(&EntityID::from(&declaration.variable_type))
            .cloned();
        for declarator in declaration.declarators {
            let initializer = match &declarator.initializer {
                Some(InitializerValue::Expression(value)) => Some(value),
                Some(InitializerValue::Nested(nested)) => {
                    // `int[] x = { 1, 2 };` — dropping it silently would
                    // leave x null with no complaint
                    self.error(
                        ctx,
                        "the array-initializer shorthand is not supported by the Udon \
                         backend yet: write `= new T[] { ... }`",
                        nested.span(),
                    );
                    None
                }
                None => None,
            };
            // `var` declarations resolve to the initializer's type
            let ty = match &declared {
                Some(ty) if !matches!(ty, Type::Infer) => ty.clone(),
                _ => initializer
                    .map(|value| self.type_of_unsubstituted(value))
                    .unwrap_or(Type::Error),
            };
            let ty = self.substitute(&ty, &ctx.key.bindings);
            let slot = self.temp_for(&ty);
            if let Some(value) = initializer
                && let Some(lowered) = self.owned_value_as(ctx, value, &ty)
            {
                self.copy(lowered, slot);
            }
            ctx.locals
                .last_mut()
                .expect("a scope is open")
                .insert(declarator.name.value, (slot, ty));
        }
    }

    fn lower_if(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast IfStatement<'ast, 'ast>) {
        let else_label = self.fresh_label("if_else");
        let end_label = self.fresh_label("if_end");
        if let Ok(condition) = &statement.condition
            && let Some(value) = self.lower_expression(ctx, condition)
        {
            self.program.code.push(Op::Push(value));
            self.program
                .code
                .push(Op::JumpIfFalse(Target::Label(else_label)));
        }
        if let Ok(then_branch) = &statement.then_branch {
            self.lower_statement(ctx, then_branch);
        }
        self.program.code.push(Op::Jump(Target::Label(end_label)));
        self.program.code.push(Op::Label(else_label));
        if let Some(else_branch) = &statement.else_branch {
            self.lower_statement(ctx, else_branch);
        }
        self.program.code.push(Op::Label(end_label));
    }

    fn lower_while(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast WhileStatement<'ast, 'ast>) {
        let head = self.fresh_label("while_head");
        let break_label = self.fresh_label("while_break");
        self.program.code.push(Op::Label(head));
        if let Ok(condition) = &statement.condition
            && let Some(value) = self.lower_expression(ctx, condition)
        {
            self.program.code.push(Op::Push(value));
            self.program
                .code
                .push(Op::JumpIfFalse(Target::Label(break_label)));
        }
        ctx.loop_stack.push(BreakFrame::Loop {
            continue_target: head,
            break_target: break_label,
        });
        if let Ok(body) = &statement.body {
            self.lower_statement(ctx, body);
        }
        ctx.loop_stack.pop();
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(break_label));
    }

    /// `foreach`: arrays are walked by index, strings as their character
    /// array, everything else through the enumerator pattern the checker
    /// bound (`GetEnumerator()` once, `MoveNext()`/`Current` per iteration).
    fn lower_foreach(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast men_sharp_parser::ast::ForeachStatement<'ast, 'ast>,
    ) {
        let Ok(collection) = &statement.collection else {
            return;
        };
        let collection_type = self.type_of(ctx, collection);
        let Some(value) = self.lower_expression(ctx, collection) else {
            return;
        };
        let span = statement.span.clone();
        if let Type::Array { element, rank: 1 } = &collection_type {
            let element = (**element).clone();
            self.lower_foreach_over_array(ctx, statement, value, collection_type, element);
        } else if self.heap_type(&collection_type) == "SystemString" {
            // Udon exposes no indexer on `string`; its character array is
            // one extern away and the loop is an ordinary array walk
            let chars = self.temp("SystemCharArray");
            self.call_extern(
                ctx,
                "SystemString.__ToCharArray__SystemCharArray",
                &[value, chars],
                span,
            );
            let char_type = self.corlib_type("Char");
            let array_type = Type::Array {
                element: Box::new(char_type.clone()),
                rank: 1,
            };
            self.lower_foreach_over_array(ctx, statement, chars, array_type, char_type);
        } else if let Some(enumeration) = self
            .bodies
            .enumerations
            .get(&EntityID::from(statement))
            .cloned()
        {
            self.lower_foreach_by_enumerator(ctx, statement, value, collection_type, &enumeration);
        } else {
            self.error(
                ctx,
                "`foreach` over this type is not supported by the Udon backend: it has no \
                 `GetEnumerator()` the compiler can call",
                span,
            );
        }
    }

    fn lower_foreach_over_array(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast men_sharp_parser::ast::ForeachStatement<'ast, 'ast>,
        array: DataId,
        array_type: Type,
        element_type: Type,
    ) {
        let Ok(name) = &statement.name else {
            return;
        };

        ctx.locals.push(HashMap::new());
        let length = self.array_length(ctx, array, &array_type, statement.span.clone());
        let index = self.temp("SystemInt32");
        let zero = self.int_constant(0);
        let one = self.int_constant(1);
        self.copy(zero, index);

        let head = self.fresh_label("foreach_head");
        let continue_label = self.fresh_label("foreach_continue");
        let break_label = self.fresh_label("foreach_break");
        let condition = self.temp("SystemBoolean");
        self.program.code.push(Op::Label(head));
        self.call_extern(
            ctx,
            "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean",
            &[index, length, condition],
            statement.span.clone(),
        );
        self.program.code.push(Op::Push(condition));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(break_label)));

        let variable = self.array_get(
            ctx,
            array,
            index,
            &array_type,
            &element_type,
            statement.span.clone(),
        );
        ctx.locals
            .last_mut()
            .expect("scope")
            .insert(name.value, (variable, element_type.clone()));

        ctx.loop_stack.push(BreakFrame::Loop {
            continue_target: continue_label,
            break_target: break_label,
        });
        if let Ok(body) = &statement.body {
            self.lower_statement(ctx, body);
        }
        ctx.loop_stack.pop();
        self.program.code.push(Op::Label(continue_label));
        self.call_extern(
            ctx,
            "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
            &[index, one, index],
            statement.span.clone(),
        );
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(break_label));
        ctx.locals.pop();
    }

    /// The written-out form of the pattern:
    /// `var e = c.GetEnumerator(); while (e.MoveNext()) { var x = e.Current; ... }`
    /// — every member call goes through the same paths a hand-written one
    /// would, so source enumerators, generic ones and externs all work alike.
    fn lower_foreach_by_enumerator(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast men_sharp_parser::ast::ForeachStatement<'ast, 'ast>,
        collection: DataId,
        collection_type: Type,
        enumeration: &ForeachEnumeration,
    ) {
        let Ok(name) = &statement.name else {
            return;
        };
        let span = statement.span.clone();

        let Piece::Value(enumerator, enumerator_type) = self.emit_call(
            ctx,
            &enumeration.get_enumerator,
            Some((collection, collection_type)),
            &[],
            span.clone(),
            false,
        ) else {
            return;
        };

        ctx.locals.push(HashMap::new());
        let head = self.fresh_label("foreach_head");
        let break_label = self.fresh_label("foreach_break");
        self.program.code.push(Op::Label(head));
        let Piece::Value(condition, _) = self.emit_call(
            ctx,
            &enumeration.move_next,
            Some((enumerator, enumerator_type.clone())),
            &[],
            span.clone(),
            false,
        ) else {
            ctx.locals.pop();
            return;
        };
        self.program.code.push(Op::Push(condition));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(break_label)));

        let place = self.member_place(
            ctx,
            &enumeration.current,
            Some((enumerator, enumerator_type)),
            span.clone(),
        );
        let Some((current, element_type)) = self.read_place(ctx, place, span.clone()) else {
            ctx.locals.pop();
            return;
        };
        // the iteration variable is a fresh local each time round: a copy,
        // so the body cannot reach into the enumerator's own slot
        let variable = self.temp_for(&element_type);
        self.copy(current, variable);
        ctx.locals
            .last_mut()
            .expect("scope")
            .insert(name.value, (variable, element_type));

        // `continue` goes straight back to `MoveNext()`
        ctx.loop_stack.push(BreakFrame::Loop {
            continue_target: head,
            break_target: break_label,
        });
        if let Ok(body) = &statement.body {
            self.lower_statement(ctx, body);
        }
        ctx.loop_stack.pop();
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(break_label));
        ctx.locals.pop();
    }

    /// `switch` over constants: compare the value against every case label in
    /// order, jump to the matching section, `default` (or the end) otherwise.
    /// `switch` statement: sections are tried in order, each label a
    /// pattern (constants included) plus its `when` guard; the first match
    /// runs its section, `default` runs when none matched wherever it was
    /// written. A section's pattern variables live in the section's scope,
    /// so its labels and body are lowered together.
    fn lower_switch(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast SwitchStatement<'ast, 'ast>) {
        let Ok(value_expression) = &statement.value else {
            return;
        };
        let value_type = self.type_of(ctx, value_expression);
        let Some(value) = self.lower_expression(ctx, value_expression) else {
            return;
        };
        let Ok(sections) = statement.sections else {
            return;
        };

        let end = self.fresh_label("switch_end");
        let no_match = self.fresh_label("switch_no_match");
        let mut default_body: Option<LabelId> = None;

        for (index, section) in sections.iter().enumerate() {
            let body = self.fresh_label(&format!("switch_body_{index}"));
            let next = self.fresh_label(&format!("switch_try_{}", index + 1));
            ctx.locals.push(HashMap::new());
            for label in section.labels {
                match label {
                    SwitchLabel::Default { .. } => default_body = Some(body),
                    SwitchLabel::Case {
                        pattern: Ok(pattern),
                        guard,
                        span,
                        ..
                    } => {
                        let next_label = self.fresh_label("case_next");
                        let Some(matched) = self.lower_pattern(ctx, value, &value_type, pattern)
                        else {
                            continue;
                        };
                        self.program.code.push(Op::Push(matched));
                        self.program
                            .code
                            .push(Op::JumpIfFalse(Target::Label(next_label)));
                        if let Some(guard) = guard
                            && let Some(condition) = self.lower_expression(ctx, guard)
                        {
                            self.program.code.push(Op::Push(condition));
                            self.program
                                .code
                                .push(Op::JumpIfFalse(Target::Label(next_label)));
                        }
                        let _ = span;
                        self.program.code.push(Op::Jump(Target::Label(body)));
                        self.program.code.push(Op::Label(next_label));
                    }
                    SwitchLabel::Case {
                        pattern: Err(()), ..
                    } => {}
                }
            }
            self.program.code.push(Op::Jump(Target::Label(next)));

            self.program.code.push(Op::Label(body));
            ctx.loop_stack
                .push(BreakFrame::Switch { break_target: end });
            for statement in section.statements {
                self.lower_statement(ctx, statement);
            }
            ctx.loop_stack.pop();
            ctx.locals.pop();
            // C# forbids falling through, so this jump is what Roslyn already
            // guaranteed the section ends with
            self.program.code.push(Op::Jump(Target::Label(end)));
            self.program.code.push(Op::Label(next));
        }

        self.program.code.push(Op::Label(no_match));
        self.program
            .code
            .push(Op::Jump(Target::Label(default_body.unwrap_or(end))));
        self.program.code.push(Op::Label(end));
    }

    /// `value switch { pattern when guard => result, ... }`: the first arm
    /// whose pattern (and guard) matches yields the value; none matching is
    /// C#'s SwitchExpressionException, so the program halts there.
    fn lower_switch_expression(
        &mut self,
        ctx: &mut Ctx<'ast>,
        switch: &'ast men_sharp_parser::ast::SwitchExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value_type = self.type_of(ctx, &switch.value);
        let value = self.lower_expression(ctx, &switch.value)?;
        let result_type = self.type_of(ctx, whole);
        let result = self.temp_for(&result_type);
        let end = self.fresh_label("switch_expression_end");
        let arms = switch.arms.ok()?;

        for arm in arms {
            let next = self.fresh_label("arm_next");
            ctx.locals.push(HashMap::new());
            let matched = self.lower_pattern(ctx, value, &value_type, &arm.pattern);
            if let Some(matched) = matched {
                self.program.code.push(Op::Push(matched));
                self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
                if let Some(guard) = &arm.guard
                    && let Some(condition) = self.lower_expression(ctx, guard)
                {
                    self.program.code.push(Op::Push(condition));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
                }
                if let Ok(arm_value) = &arm.value
                    && let Some(produced) = self.owned_value(ctx, arm_value)
                {
                    let arm_type = self.type_of(ctx, arm_value);
                    let converted =
                        self.convert(ctx, produced, &arm_type, &result_type, arm.span.clone());
                    self.copy(converted, result);
                }
                self.program.code.push(Op::Jump(Target::Label(end)));
            }
            ctx.locals.pop();
            self.program.code.push(Op::Label(next));
        }

        self.throw_new(
            ctx,
            &[
                "System",
                "Runtime",
                "CompilerServices",
                "SwitchExpressionException",
            ],
            None,
            switch.span.clone(),
        );
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    pub(super) fn fresh_label(&mut self, prefix: &str) -> LabelId {
        self.temp_counter += 1;
        self.program
            .add_label(format!("{prefix}_{}", self.temp_counter))
    }

    fn type_of_unsubstituted(&self, expression: &Expression) -> Type {
        self.bodies
            .expression_types
            .get(&EntityID::from(expression))
            .cloned()
            .unwrap_or(Type::Error)
    }

    // ---------------------------------------------------------- expressions

    pub(super) fn lower_expression(
        &mut self,
        ctx: &mut Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        match expression {
            Expression::Primary(primary) => match self.lower_primary(ctx, primary) {
                Piece::Value(slot, _) => Some(slot),
                _ => None,
            },
            Expression::Binary(binary) => self.lower_binary(ctx, binary, expression),
            Expression::Unary(unary) => self.lower_unary(ctx, unary, expression),
            Expression::Assignment(assignment) => self.lower_assignment(ctx, assignment),
            Expression::Conditional(conditional) => {
                let ty = self.type_of(ctx, expression);
                let result = self.temp_for(&ty);
                let else_label = self.fresh_label("cond_else");
                let end_label = self.fresh_label("cond_end");
                if let Some(condition) = self.lower_expression(ctx, &conditional.condition) {
                    self.program.code.push(Op::Push(condition));
                    self.program
                        .code
                        .push(Op::JumpIfFalse(Target::Label(else_label)));
                }
                if let Ok(value) = &conditional.then_value
                    && let Some(slot) = self.lower_expression(ctx, value)
                {
                    self.copy(slot, result);
                }
                self.program.code.push(Op::Jump(Target::Label(end_label)));
                self.program.code.push(Op::Label(else_label));
                if let Ok(value) = &conditional.else_value
                    && let Some(slot) = self.lower_expression(ctx, value)
                {
                    self.copy(slot, result);
                }
                self.program.code.push(Op::Label(end_label));
                Some(result)
            }
            Expression::Cast(cast) => {
                let value = cast.value.as_ref().ok()?;
                let source = self.lower_expression(ctx, value)?;
                let from = self.type_of(ctx, value);
                let to = self.type_of(ctx, expression);
                let converted = self.convert(ctx, source, &from, &to, expression.span());
                Some(self.checked_cast(ctx, converted, &from, &to, expression.span()))
            }
            Expression::Is(is) => self.lower_is(ctx, is),
            Expression::Switch(switch) => self.lower_switch_expression(ctx, switch, expression),
            Expression::Throw(throw) => self.lower_throw_expression(ctx, throw),
            Expression::As(as_expression) => self.lower_as(ctx, as_expression, expression),
            other => {
                self.error(
                    ctx,
                    "this expression is not supported by the Udon backend yet",
                    other.span(),
                );
                None
            }
        }
    }

    /// A best-effort conversion between representable types. Same Udon type:
    /// plain copy semantics (no code at all — the caller uses the source
    /// slot). Numeric changes go through `SystemConvert`.
    pub(super) fn convert(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        from: &Type,
        to: &Type,
        span: Range<usize>,
    ) -> DataId {
        let from_name = self.extern_type_name(from);
        let to_name = self.extern_type_name(to);
        match (from_name, to_name) {
            (Some(from_name), Some(to_name)) if from_name != to_name => {
                let method = match to_name.as_str() {
                    "SystemInt32" => Some("ToInt32"),
                    "SystemInt64" => Some("ToInt64"),
                    "SystemSingle" => Some("ToSingle"),
                    "SystemDouble" => Some("ToDouble"),
                    "SystemByte" => Some("ToByte"),
                    "SystemUInt32" => Some("ToUInt32"),
                    _ => None,
                };
                if let Some(method) = method {
                    let signature = format!("SystemConvert.__{method}__{from_name}__{to_name}");
                    if self.nodes.has_signature(&signature) {
                        let out = self.temp(&to_name);
                        self.call_extern(ctx, &signature, &[source, out], span);
                        return out;
                    }
                    // `(int)keyCode`: no per-enum Convert extern exists, but
                    // the boxed value converts fine as an object
                    if self.external_enum(from).is_some() {
                        let signature =
                            format!("SystemConvert.__{method}__SystemObject__{to_name}");
                        if self.nodes.has_signature(&signature) {
                            let out = self.temp(&to_name);
                            self.call_extern(ctx, &signature, &[source, out], span);
                            return out;
                        }
                    }
                }
                source
            }
            // reference casts and identity: values are untyped objects
            _ => source,
        }
    }

    fn lower_binary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        binary: &'ast men_sharp_parser::ast::BinaryExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        use BinaryOperator::*;

        if matches!(binary.operator.value, LogicalAnd | LogicalOr) {
            let result = self.temp("SystemBoolean");
            let short_label = self.fresh_label("logic_short");
            let end_label = self.fresh_label("logic_end");
            let left = self.lower_expression(ctx, &binary.left)?;
            self.copy(left, result);
            self.program.code.push(Op::Push(left));
            match binary.operator.value {
                // false && _ → false
                LogicalAnd => self
                    .program
                    .code
                    .push(Op::JumpIfFalse(Target::Label(short_label))),
                // true || _ → true: invert by testing and falling through
                _ => {
                    let negated = self.temp("SystemBoolean");
                    // pushed `left` above is consumed here
                    self.program.code.pop();
                    self.call_extern(
                        ctx,
                        "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                        &[left, negated],
                        binary.span.clone(),
                    );
                    self.program.code.push(Op::Push(negated));
                    self.program
                        .code
                        .push(Op::JumpIfFalse(Target::Label(short_label)));
                }
            }
            if let Ok(right) = &binary.right
                && let Some(right) = self.lower_expression(ctx, right)
            {
                self.copy(right, result);
            }
            self.program.code.push(Op::Jump(Target::Label(end_label)));
            self.program.code.push(Op::Label(short_label));
            self.program.code.push(Op::Label(end_label));
            return Some(result);
        }

        let left_type = self.type_of(ctx, &binary.left);
        let left = self.lower_expression(ctx, &binary.left)?;
        let right_expression = binary.right.as_ref().ok()?;
        let right_type = self.type_of(ctx, right_expression);
        let right = self.lower_expression(ctx, right_expression)?;
        let result_type = self.type_of(ctx, whole);

        self.emit_binary_operator(
            ctx,
            binary.operator.value,
            (left, &left_type),
            (right, &right_type),
            &result_type,
            binary.span.clone(),
            Some(EntityID::from(binary)),
        )
    }

    /// A user-declared operator the checker bound this node to (a struct's
    /// own `+`, a class's `==`): a call to it, with struct operands copied
    /// as by-value parameters are. `None` when the node has none — the
    /// built-in or extern operator applies.
    fn user_operator_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        node: Option<EntityID>,
        operands: &[(DataId, &Type)],
        span: Range<usize>,
    ) -> Option<Option<DataId>> {
        let call = match self.bodies.targets.get(&node?) {
            Some(ResolvedTarget::Call(call)) => call.clone(),
            _ => return None,
        };
        let MemberOrigin::Source(symbol) = call.origin else {
            return None;
        };
        if self.declarations.table.symbol(symbol).kind != SymbolKind::Operator {
            return None;
        }
        let key = FunctionKey {
            symbol,
            role: Role::Method,
            bindings: self.bindings_for(ctx, symbol, &call.declaring_type, &[]),
        };
        let count = call.signature.parameters.len().min(operands.len());
        let mut values = Vec::with_capacity(count);
        for (slot, ty) in &operands[..count] {
            let ty = self.substitute(ty, &ctx.key.bindings);
            values.push(if self.is_source_struct(&ty) {
                self.clone_struct(ctx, *slot, &ty, span.clone())
            } else {
                *slot
            });
        }
        Some(self.call_function(ctx, &key, None, &values, &[], span))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_binary_operator(
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

        if let Some(result) = self.user_operator_call(ctx, node, &[left, right], span.clone()) {
            return result;
        }
        if matches!(operator, Divide | Modulo) {
            self.check_divisor(ctx, right.0, right.1, span.clone());
        }

        let system_is = |ty: &Type, name: &str| {
            self.extern_type_name(ty)
                .is_some_and(|mangled| mangled == name)
        };

        // a value type compared with `null`: never equal (C# folds this too)
        if matches!(operator, Equal | NotEqual) {
            let value_type = match (left.1, right.1) {
                (Type::Null, ty) | (ty, Type::Null) => ty,
                _ => &Type::Error,
            };
            if !matches!(value_type, Type::Null | Type::Error | Type::Nullable(_))
                && !self.is_reference_type(value_type)
                && self.heap_type(value_type) != "SystemObject"
            {
                return Some(self.constant(
                    "SystemBoolean",
                    if operator == Equal { "false" } else { "true" },
                    HeapInit::Boolean(operator != Equal),
                ));
            }
        }

        // string concatenation
        if operator == Add && system_is(result_type, "SystemString") {
            let out = self.temp("SystemString");
            let stringified = [
                self.stringify(ctx, left.0, left.1, span.clone()),
                self.stringify(ctx, right.0, right.1, span.clone()),
            ];
            self.call_extern(
                ctx,
                "SystemString.__Concat__SystemString_SystemString__SystemString",
                &[stringified[0], stringified[1], out],
                span,
            );
            return Some(out);
        }

        let name = match operator {
            Add => "op_Addition",
            Subtract => "op_Subtraction",
            Multiply => "op_Multiplication",
            Divide => "op_Division",
            Modulo => "op_Remainder",
            LessThan => "op_LessThan",
            GreaterThan => "op_GreaterThan",
            LessThanEqual => "op_LessThanOrEqual",
            GreaterThanEqual => "op_GreaterThanOrEqual",
            Equal => "op_Equality",
            NotEqual => "op_Inequality",
            BitwiseAnd => "op_LogicalAnd",
            BitwiseOr => "op_LogicalOr",
            BitwiseXor => "op_LogicalXor",
            LeftShift => "op_LeftShift",
            RightShift => "op_RightShift",
            _ => {
                self.error(
                    ctx,
                    "this operator is not supported by the Udon backend yet",
                    span,
                );
                return None;
            }
        };

        // the operand type carries the operator; comparisons return bool
        let operand_type = if matches!(
            operator,
            LessThan | GreaterThan | LessThanEqual | GreaterThanEqual | Equal | NotEqual
        ) {
            left.1.clone()
        } else {
            result_type.clone()
        };

        // an external enum is a boxed value: `==`/`!=` go through
        // Object.Equals (value equality for same-type enums, where
        // op_Equality would compare boxes), orderings through the
        // underlying Int32
        if self.external_enum(&operand_type).is_some() {
            match operator {
                Equal | NotEqual => {
                    let equal = self.temp("SystemBoolean");
                    self.call_extern(
                        ctx,
                        "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean",
                        &[left.0, right.0, equal],
                        span.clone(),
                    );
                    if operator == Equal {
                        return Some(equal);
                    }
                    let out = self.temp("SystemBoolean");
                    self.call_extern(
                        ctx,
                        "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                        &[equal, out],
                        span,
                    );
                    return Some(out);
                }
                LessThan | GreaterThan | LessThanEqual | GreaterThanEqual => {
                    let mut unboxed = [left.0, right.0];
                    for slot in &mut unboxed {
                        let int = self.temp("SystemInt32");
                        self.call_extern(
                            ctx,
                            "SystemConvert.__ToInt32__SystemObject__SystemInt32",
                            &[*slot, int],
                            span.clone(),
                        );
                        *slot = int;
                    }
                    let out = self.temp("SystemBoolean");
                    self.call_extern(
                        ctx,
                        &format!("SystemInt32.__{name}__SystemInt32_SystemInt32__SystemBoolean"),
                        &[unboxed[0], unboxed[1], out],
                        span,
                    );
                    return Some(out);
                }
                _ => {}
            }
        }

        let out = self.temp_for(result_type);
        let mut result_name = self
            .extern_type_name(result_type)
            .unwrap_or_else(|| "SystemBoolean".into());
        // a source enum result (`Flags.A | Flags.B`) lives in an Int32 slot
        if self.source_enum(result_type).is_some() {
            result_name = "SystemInt32".into();
        }

        // Udon names a primitive's operators its own way (`op_Multiplication`,
        // `op_Remainder`); an engine type's keep their .NET metadata names
        // (`Vector3.op_Multiply`) — so both spellings are tried
        let dotnet_name = match operator {
            Multiply => "op_Multiply",
            Modulo => "op_Modulus",
            _ => name,
        };
        let names: Vec<&str> = if dotnet_name == name {
            vec![name]
        } else {
            vec![name, dotnet_name]
        };
        let mut candidates: Vec<String> = Vec::new();
        // the operands as written (`Vector3 * float`), declared on either
        // side's type or a base of it
        if let (Some(left_name), Some(right_name)) = (
            self.extern_type_name(left.1),
            self.extern_type_name(right.1),
        ) {
            let mut owners = self.external_chain(left.1);
            for owner in self.external_chain(right.1) {
                if !owners.contains(&owner) {
                    owners.push(owner);
                }
            }
            for owner in &owners {
                for spelled in &names {
                    candidates.push(format!(
                        "{owner}.__{spelled}__{left_name}_{right_name}__{result_name}"
                    ));
                }
            }
        }
        // an operator can be declared on a base type — `Rigidbody == null`
        // binds to UnityEngine.Object's — so the whole chain is a candidate,
        // nearest first, exactly as C# overload resolution would look
        for owner in self.external_chain(&operand_type) {
            for spelled in &names {
                candidates.push(format!(
                    "{owner}.__{spelled}__{owner}_{owner}__{result_name}"
                ));
            }
        }
        // and reference types with no operator at all still compare by
        // identity, which is what `x == null` needs. Value types must not fall
        // through to this: two equal structs are different objects once boxed
        if matches!(operator, Equal | NotEqual) && self.is_reference_type(&operand_type) {
            candidates.push(format!(
                "SystemObject.__{name}__SystemObject_SystemObject__SystemBoolean"
            ));
        }
        // a source enum compares (and, for flags, combines) as its
        // underlying Int32 — its slots hold plain ints
        if self.source_enum(&operand_type).is_some() {
            candidates.push(format!(
                "SystemInt32.__{name}__SystemInt32_SystemInt32__{result_name}"
            ));
        }
        let signature = candidates
            .into_iter()
            .find(|signature| self.nodes.has_signature(signature));
        match signature {
            Some(signature) => {
                self.call_extern(ctx, &signature, &[left.0, right.0, out], span);
                Some(out)
            }
            None => {
                self.error(
                    ctx,
                    format!("operator `{name}` is not available on Udon for these operand types"),
                    span,
                );
                None
            }
        }
    }

    /// A value as a `string`, via the type's own `ToString` extern.
    fn stringify(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let ty = &self.substitute(ty, &ctx.key.bindings);
        // a value that may be an object of the user's: its own `ToString`
        if self.has_type_id(ty)
            || (self.heap_type(ty) == "SystemObject"
                && matches!(
                    ty,
                    Type::Named {
                        target: TypeTarget::External(_),
                        ..
                    }
                ))
        {
            return self.object_to_string(ctx, slot, ty, span);
        }
        if let Some(name) = self.extern_type_name(ty) {
            if name == "SystemString" {
                return slot;
            }
            let signature = format!("{name}.__ToString__SystemString");
            if self.nodes.has_signature(&signature) {
                let out = self.temp("SystemString");
                self.call_extern(ctx, &signature, &[slot, out], span);
                return out;
            }
        }
        let out = self.temp("SystemString");
        self.call_extern(
            ctx,
            "SystemConvert.__ToString__SystemObject__SystemString",
            &[slot, out],
            span,
        );
        out
    }

    fn lower_unary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        unary: &'ast men_sharp_parser::ast::UnaryExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let operand_expression = unary.operand.as_ref().ok()?;
        if matches!(
            unary.operator.value,
            UnaryOperator::Not
                | UnaryOperator::Minus
                | UnaryOperator::Plus
                | UnaryOperator::BitwiseNot
        ) && self.bodies.targets.contains_key(&EntityID::from(unary))
        {
            let operand = self.lower_expression(ctx, operand_expression)?;
            let operand_type = self.type_of(ctx, operand_expression);
            if let Some(result) = self.user_operator_call(
                ctx,
                Some(EntityID::from(unary)),
                &[(operand, &operand_type)],
                unary.span.clone(),
            ) {
                return result;
            }
        }
        match unary.operator.value {
            UnaryOperator::Not => {
                let operand = self.lower_expression(ctx, operand_expression)?;
                let out = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                    &[operand, out],
                    unary.span.clone(),
                );
                Some(out)
            }
            UnaryOperator::Minus => {
                let operand = self.lower_expression(ctx, operand_expression)?;
                let ty = self.type_of(ctx, whole);
                let name = self.extern_type_name(&ty)?;
                let out = self.temp_for(&ty);
                // Udon's spelling for primitives, .NET's for engine types
                let candidates = [
                    format!("{name}.__op_UnaryMinus__{name}__{name}"),
                    format!("{name}.__op_UnaryNegation__{name}__{name}"),
                ];
                let Some(signature) = candidates
                    .iter()
                    .find(|signature| self.nodes.has_signature(signature))
                else {
                    self.error(
                        ctx,
                        "unary `-` is not available on Udon for this operand type",
                        unary.span.clone(),
                    );
                    return None;
                };
                self.call_extern(ctx, signature, &[operand, out], unary.span.clone());
                Some(out)
            }
            UnaryOperator::Plus => self.lower_expression(ctx, operand_expression),
            UnaryOperator::PreIncrement | UnaryOperator::PreDecrement => {
                let one = self.int_constant(1);
                let operator = if unary.operator.value == UnaryOperator::PreIncrement {
                    BinaryOperator::Add
                } else {
                    BinaryOperator::Subtract
                };
                let place = self.lower_place(ctx, operand_expression);
                let (value, ty) = self.read_place(ctx, place, unary.span.clone())?;
                let updated = self.emit_binary_operator(
                    ctx,
                    operator,
                    (value, &ty),
                    (one, &ty),
                    &ty,
                    unary.span.clone(),
                    Some(EntityID::from(unary)),
                )?;
                let place = self.lower_place(ctx, operand_expression);
                self.write_place(ctx, place, updated, unary.span.clone());
                Some(updated)
            }
            _ => {
                self.error(
                    ctx,
                    "this operator is not supported by the Udon backend yet",
                    unary.span.clone(),
                );
                None
            }
        }
    }

    fn lower_assignment(
        &mut self,
        ctx: &mut Ctx<'ast>,
        assignment: &'ast men_sharp_parser::ast::AssignmentExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value_expression = assignment.value.as_ref().ok()?;
        if assignment.operator.value == AssignmentOperator::Assign {
            // the target first (its index expressions run before the value,
            // as in C#), then the value converted to the target's type
            let place = self.lower_place(ctx, &assignment.target);
            let value = match place_type(&place) {
                Some(target) => self.owned_value_as(ctx, value_expression, &target)?,
                None => self.owned_value(ctx, value_expression)?,
            };
            self.write_place(ctx, place, value, assignment.span.clone());
            return Some(value);
        }
        let value = self.owned_value(ctx, value_expression)?;
        let value_type = self.type_of(ctx, value_expression);

        let final_value = {
            let operator = match assignment.operator.value {
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
                _ => {
                    self.error(
                        ctx,
                        "this compound assignment is not supported by the Udon backend yet",
                        assignment.span.clone(),
                    );
                    return None;
                }
            };
            let place = self.lower_place(ctx, &assignment.target);
            let (current, target_type) = self.read_place(ctx, place, assignment.span.clone())?;
            self.emit_binary_operator(
                ctx,
                operator,
                (current, &target_type),
                (value, &value_type),
                &target_type,
                assignment.span.clone(),
                Some(EntityID::from(assignment)),
            )?
        };

        let place = self.lower_place(ctx, &assignment.target);
        self.write_place(ctx, place, final_value, assignment.span.clone());
        Some(final_value)
    }

    // --------------------------------------------------------------- places

    fn lower_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Place {
        let Expression::Primary(primary) = expression else {
            self.error(
                ctx,
                "this expression cannot be assigned to",
                expression.span(),
            );
            return Place::Error;
        };
        self.primary_place(ctx, primary)
    }

    fn primary_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        primary: &'ast PrimaryExpression<'ast, 'ast>,
    ) -> Place {
        self.place_upto(ctx, primary, primary.chain.len())
    }

    /// The assignable place produced by `primary.left` plus the first `count`
    /// chain elements.
    fn place_upto(
        &mut self,
        ctx: &mut Ctx<'ast>,
        primary: &'ast PrimaryExpression<'ast, 'ast>,
        count: usize,
    ) -> Place {
        if count == 0 {
            return self.left_place(ctx, &primary.left);
        }
        let mut piece = self.lower_left(ctx, &primary.left);
        for right in &primary.chain[..count - 1] {
            let receiver_was_array = matches!(&piece, Piece::Value(_, Type::Array { .. }));
            piece = self.apply_right(ctx, piece, right);
            if !self.check_write_through_copy(ctx, &piece, right, receiver_was_array) {
                return Place::Error;
            }
        }
        let last = &primary.chain[count - 1];
        let receiver = piece.receiver();
        match last {
            PrimaryRight::Member { span, .. } => {
                match self.bodies.targets.get(&EntityID::from(last)) {
                    Some(ResolvedTarget::Member(member)) => {
                        let member = member.clone();
                        self.member_place(ctx, &member, receiver, span.clone())
                    }
                    _ => {
                        self.error(ctx, "this member cannot be assigned to", span.clone());
                        Place::Error
                    }
                }
            }
            PrimaryRight::ElementAccess {
                arguments, span, ..
            } => self.element_place(ctx, receiver, arguments.arguments, span.clone(), last),
            _ => {
                self.error(ctx, "this expression cannot be assigned to", last.span());
                Place::Error
            }
        }
    }

    fn left_place(&mut self, ctx: &mut Ctx<'ast>, left: &'ast PrimaryLeft<'ast, 'ast>) -> Place {
        match left {
            PrimaryLeft::Identifier { name, span, .. } => {
                match self.bodies.targets.get(&EntityID::from(left)) {
                    Some(ResolvedTarget::Local) => match ctx.lookup(name.value) {
                        Some((slot, ty)) => Place::Slot(slot, ty),
                        None => Place::Error,
                    },
                    Some(ResolvedTarget::Member(member)) => {
                        let member = member.clone();
                        let receiver = ctx
                            .this_slot
                            .zip(ctx.this_type.clone())
                            .filter(|_| !member.is_static);
                        self.member_place(ctx, &member, receiver, span.clone())
                    }
                    _ => {
                        self.error(ctx, "this name cannot be assigned to", span.clone());
                        Place::Error
                    }
                }
            }
            other => {
                self.error(ctx, "this expression cannot be assigned to", other.span());
                Place::Error
            }
        }
    }

    /// Inside a behaviour there is no `this`: its own (and inherited) members
    /// live in named heap slots, and everything else needs an explicit
    /// receiver. This catches an instance member that has neither, so it is
    /// reported instead of quietly lowering to a missing slot.
    fn has_no_instance_to_read_from(
        &self,
        ctx: &Ctx<'ast>,
        symbol: SymbolId,
        is_static: bool,
        receiver: &Option<(DataId, Type)>,
    ) -> bool {
        self.entry_class.is_some()
            && !is_static
            && receiver.is_none()
            && ctx.this_slot.is_none()
            && !self.is_entry_member(symbol)
    }

    fn no_instance_error(&mut self, ctx: &Ctx<'ast>, symbol: SymbolId, span: Range<usize>) {
        let name = self.declarations.table.symbol(symbol).name;
        self.error(
            ctx,
            format!("`{name}` is an instance member with no object to reach it through"),
            span,
        );
    }

    pub(super) fn member_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: &ResolvedMember,
        receiver: Option<(DataId, Type)>,
        span: Range<usize>,
    ) -> Place {
        let member_type = self.substitute(&member.member_type, &ctx.key.bindings);
        let declaring = self.substitute(&member.declaring_type, &ctx.key.bindings);
        match &member.origin {
            MemberOrigin::Source(symbol) => {
                let symbol = *symbol;
                // `gameObject`/`transform`: a slot Udon fills with what this
                // program is attached to. Only *this* program's — Udon
                // resolves the reference against the behaviour that owns the
                // heap, so there is no way to ask another one
                if let Some(slot) = self.self_reference_slot(symbol) {
                    if receiver.is_some() {
                        self.error(
                            ctx,
                            format!(
                                "`{}` is only available on the behaviour itself, \
                                 not through another object",
                                self.declarations.table.symbol(symbol).name
                            ),
                            span,
                        );
                        return Place::Error;
                    }
                    return Place::SelfReference {
                        slot,
                        ty: member_type,
                        name: self.declarations.table.symbol(symbol).name.to_string(),
                    };
                }
                // fields (and auto-property stores) of the behaviour entry
                // class are the program's named, exported heap slots
                if self.is_entry_member(symbol)
                    && matches!(member.kind, SymbolKind::Field | SymbolKind::Property)
                    && (member.kind == SymbolKind::Field || self.is_auto_property(symbol))
                {
                    let export = self.is_public_variable(symbol);
                    let slot = self.ensure_static(symbol, export);
                    return Place::Slot(slot, member_type);
                }
                // a member of *another* behaviour: two programs share no
                // memory, so the only way across is Udon's by-name access
                if let Some((slot, receiver_type)) = &receiver
                    && self.is_program_reference(receiver_type)
                {
                    let (slot, receiver_type) = (*slot, receiver_type.clone());
                    return self.program_member_place(
                        ctx,
                        member,
                        symbol,
                        slot,
                        &receiver_type,
                        span,
                    );
                }
                if self.has_no_instance_to_read_from(ctx, symbol, member.is_static, &receiver) {
                    self.no_instance_error(ctx, symbol, span);
                    return Place::Error;
                }
                match member.kind {
                    SymbolKind::Field if member.is_static => {
                        let slot = self.ensure_static(symbol, false);
                        Place::Slot(slot, member_type)
                    }
                    SymbolKind::Field => {
                        let Some(layout) = self.layout_of(&declaring) else {
                            self.error(ctx, "no layout for this receiver", span);
                            return Place::Error;
                        };
                        let Some(&index) = layout.slots.get(&symbol) else {
                            self.error(ctx, "field is missing from the object layout", span);
                            return Place::Error;
                        };
                        if let Some((slot, receiver_type)) = &receiver
                            && Some(*slot) != ctx.this_slot
                            && self.has_type_id(receiver_type)
                        {
                            self.check_not_null(ctx, *slot, span.clone());
                        }
                        let object = receiver.map(|(slot, _)| slot).or(ctx.this_slot);
                        match object {
                            Some(object) => Place::Field {
                                object,
                                index: self.int_constant(index as i32),
                                ty: member_type,
                            },
                            None => Place::Error,
                        }
                    }
                    SymbolKind::Property => {
                        // auto-properties store like fields
                        if self.is_auto_property(symbol)
                            && !member.is_static
                            && let Some(layout) = self.layout_of(&declaring)
                            && let Some(&index) = layout.slots.get(&symbol)
                            && let Some(object) =
                                receiver.as_ref().map(|(slot, _)| *slot).or(ctx.this_slot)
                        {
                            if let Some((slot, receiver_type)) = &receiver
                                && Some(*slot) != ctx.this_slot
                                && self.has_type_id(receiver_type)
                            {
                                self.check_not_null(ctx, *slot, span.clone());
                            }
                            return Place::Field {
                                object,
                                index: self.int_constant(index as i32),
                                ty: member_type,
                            };
                        }
                        // same reasoning as for methods: on the single
                        // behaviour instance the override is known here
                        let symbol = if !member.is_static
                            && self.is_virtual(symbol)
                            && self.is_entry_member(symbol)
                        {
                            self.entry_override(symbol)
                        } else {
                            symbol
                        };
                        let bindings = self.bindings_for(ctx, symbol, &member.declaring_type, &[]);
                        Place::Accessor {
                            receiver: receiver
                                .map(|(slot, _)| slot)
                                .or(ctx.this_slot.filter(|_| !member.is_static)),
                            symbol,
                            bindings,
                            indices: Vec::new(),
                            ty: member_type,
                        }
                    }
                    _ => {
                        self.error(
                            ctx,
                            "this member kind is not supported by the Udon backend yet",
                            span,
                        );
                        Place::Error
                    }
                }
            }
            MemberOrigin::External {
                member: external, ..
            } => {
                let Some(owner) = self.extern_type_name(&declaring) else {
                    self.error(ctx, "this type is not available on Udon", span);
                    return Place::Error;
                };
                Place::ExternalProperty {
                    receiver: receiver.map(|(slot, _)| slot),
                    owner,
                    name: external.name.clone(),
                    ty: member_type,
                }
            }
        }
    }

    fn element_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: Option<(DataId, Type)>,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
        node: &'ast PrimaryRight<'ast, 'ast>,
    ) -> Place {
        let Some((slot, ty)) = receiver else {
            return Place::Error;
        };
        if let Type::Array { element, rank: 1 } = &ty {
            let index = arguments
                .first()
                .and_then(|argument| match &argument.value {
                    ArgumentValue::Expression(expression) => self.lower_expression(ctx, expression),
                    _ => None,
                });
            return match index {
                Some(index) => Place::Element {
                    array: slot,
                    index,
                    element: self.substitute(element, &ctx.key.bindings),
                    array_type: ty.clone(),
                },
                None => Place::Error,
            };
        }
        // an indexer
        match self.bodies.targets.get(&EntityID::from(node)) {
            Some(ResolvedTarget::Call(call)) => {
                let call = call.clone();
                let MemberOrigin::Source(symbol) = call.origin else {
                    self.error(
                        ctx,
                        "external indexers are not supported by the Udon backend yet",
                        span,
                    );
                    return Place::Error;
                };
                let mut indices = Vec::new();
                for argument in arguments {
                    if let ArgumentValue::Expression(expression) = &argument.value
                        && let Some(value) = self.lower_expression(ctx, expression)
                    {
                        indices.push(value);
                    }
                }
                let bindings =
                    self.bindings_for(ctx, symbol, &call.declaring_type, &call.type_arguments);
                let ty = self.substitute(&call.signature.return_type, &ctx.key.bindings);
                Place::Accessor {
                    receiver: Some(slot),
                    symbol,
                    bindings,
                    indices,
                    ty,
                }
            }
            _ => {
                self.error(ctx, "this element access cannot be compiled yet", span);
                Place::Error
            }
        }
    }

    /// The getter/setter to call for a property or indexer: the accessor
    /// itself, or — when it is virtual, abstract or an interface member and
    /// there is an instance to dispatch on — the stub that picks the
    /// override by the receiver's type id.
    fn accessor_key(
        &mut self,
        ctx: &Ctx<'ast>,
        has_receiver: bool,
        symbol: SymbolId,
        bindings: Vec<(SymbolId, Type)>,
        role: Role,
    ) -> FunctionKey {
        if has_receiver && self.is_virtual(symbol) && !self.is_entry_member(symbol) {
            let declaring = self.declaring_type_of(symbol, &bindings);
            return self.dispatcher_for(ctx, symbol, &declaring, &[], role);
        }
        if self.is_entry_member(symbol) && self.is_virtual(symbol) {
            // one behaviour instance: its most derived accessor is known
            return FunctionKey {
                symbol: self.entry_override(symbol),
                role,
                bindings,
            };
        }
        FunctionKey {
            symbol,
            role,
            bindings,
        }
    }

    pub(super) fn read_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        place: Place,
        span: Range<usize>,
    ) -> Option<(DataId, Type)> {
        match place {
            Place::Slot(slot, ty) | Place::SelfReference { slot, ty, .. } => Some((slot, ty)),
            Place::Field { object, index, ty } => {
                let value = self.get_element(ctx, object, index, &ty, span);
                Some((value, ty))
            }
            Place::Element {
                array,
                index,
                element,
                array_type,
            } => {
                self.check_array_access(ctx, array, index, &array_type, span.clone());
                let value = self.array_get(ctx, array, index, &array_type, &element, span);
                Some((value, element))
            }
            Place::Accessor {
                receiver,
                symbol,
                bindings,
                indices,
                ty,
            } => {
                let key =
                    self.accessor_key(ctx, receiver.is_some(), symbol, bindings, Role::Getter);
                let result = self.call_function(ctx, &key, receiver, &indices, &[], span)?;
                Some((result, ty))
            }
            Place::ProgramVariable { receiver, name, ty } => {
                let out = self.get_program_variable(ctx, receiver, &name, &ty, span);
                Some((out, ty))
            }
            Place::ProgramAccessor {
                receiver,
                getter,
                name,
                ty,
                ..
            } => {
                let out = self.read_program_accessor(ctx, receiver, getter, &name, &ty, span)?;
                Some((out, ty))
            }
            Place::ExternalProperty {
                receiver,
                owner,
                name,
                ty,
            } => {
                let return_name = self.extern_type_name(&ty)?;
                let signature = format!("{owner}.__get_{name}__{return_name}");
                let out = self.temp_for(&ty);
                let mut arguments = Vec::new();
                arguments.extend(receiver);
                arguments.push(out);
                self.call_extern(ctx, &signature, &arguments, span);
                Some((out, ty))
            }
            Place::Error => None,
        }
    }

    pub(super) fn write_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        place: Place,
        value: DataId,
        span: Range<usize>,
    ) {
        match place {
            Place::Slot(slot, _) => self.copy(value, slot),
            Place::SelfReference { name, .. } => self.error(
                ctx,
                format!("`{name}` is read-only: it is what this behaviour is attached to"),
                span,
            ),
            Place::Field { object, index, .. } => self.set_element(ctx, object, index, value, span),
            Place::Element {
                array,
                index,
                array_type,
                ..
            } => {
                self.check_array_access(ctx, array, index, &array_type, span.clone());
                self.array_set(ctx, array, index, value, &array_type, span)
            }
            Place::Accessor {
                receiver,
                symbol,
                bindings,
                mut indices,
                ..
            } => {
                let key =
                    self.accessor_key(ctx, receiver.is_some(), symbol, bindings, Role::Setter);
                indices.push(value);
                self.call_function(ctx, &key, receiver, &indices, &[], span);
            }
            Place::ProgramVariable { receiver, name, .. } => {
                self.set_program_variable(ctx, receiver, &name, value, span);
            }
            Place::ProgramAccessor {
                receiver,
                setter,
                name,
                ..
            } => {
                self.write_program_accessor(ctx, receiver, setter, &name, value, span);
            }
            Place::ExternalProperty {
                receiver,
                owner,
                name,
                ty,
            } => {
                let Some(value_name) = self.extern_type_name(&ty) else {
                    return;
                };
                // a property setter is `__set_X__T__SystemVoid`; the SDK spells
                // a struct *field* setter (`Vector3.x`) without the return part
                let property = format!("{owner}.__set_{name}__{value_name}__SystemVoid");
                let field = format!("{owner}.__set_{name}__{value_name}");
                let signature =
                    if !self.nodes.has_signature(&property) && self.nodes.has_signature(&field) {
                        field
                    } else {
                        property
                    };
                let mut arguments = Vec::new();
                arguments.extend(receiver);
                arguments.push(value);
                self.call_extern(ctx, &signature, &arguments, span);
            }
            Place::Error => {}
        }
    }

    // --------------------------------------------------------------- arrays

    pub(super) fn array_length(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        array_type: &Type,
        span: Range<usize>,
    ) -> DataId {
        let owner = self.heap_type(array_type);
        let out = self.temp("SystemInt32");
        let candidates = [
            format!("{owner}.__get_Length__SystemInt32"),
            "SystemArray.__get_Length__SystemInt32".to_string(),
        ];
        let signature = candidates
            .iter()
            .find(|signature| self.nodes.has_signature(signature))
            .cloned()
            .unwrap_or_else(|| candidates[1].clone());
        self.call_extern(ctx, &signature, &[array, out], span);
        out
    }

    pub(super) fn array_get(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        index: DataId,
        array_type: &Type,
        element: &Type,
        span: Range<usize>,
    ) -> DataId {
        let owner = self.heap_type(array_type);
        let element_name = self
            .extern_type_name(element)
            .unwrap_or_else(|| "SystemObject".into());
        let out = self.temp_for(element);
        let candidates = [
            format!("{owner}.__Get__SystemInt32__{element_name}"),
            "SystemObjectArray.__Get__SystemInt32__SystemObject".to_string(),
        ];
        let signature = candidates
            .iter()
            .find(|signature| self.nodes.has_signature(signature))
            .cloned()
            .unwrap_or_else(|| candidates[1].clone());
        self.call_extern(ctx, &signature, &[array, index, out], span);
        out
    }

    pub(super) fn array_set(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        index: DataId,
        value: DataId,
        array_type: &Type,
        span: Range<usize>,
    ) {
        let owner = self.heap_type(array_type);
        let element_name = match array_type {
            Type::Array { element, rank: 1 } => self
                .extern_type_name(element)
                .unwrap_or_else(|| "SystemObject".into()),
            _ => "SystemObject".into(),
        };
        let candidates = [
            format!("{owner}.__Set__SystemInt32_{element_name}__SystemVoid"),
            "SystemObjectArray.__Set__SystemInt32_SystemObject__SystemVoid".to_string(),
        ];
        let signature = candidates
            .iter()
            .find(|signature| self.nodes.has_signature(signature))
            .cloned()
            .unwrap_or_else(|| candidates[1].clone());
        self.call_extern(ctx, &signature, &[array, index, value], span);
    }

    // ------------------------------------------------------------- primary

    fn lower_primary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        primary: &'ast PrimaryExpression<'ast, 'ast>,
    ) -> Piece {
        // `x++` / `obj.field--`: read–modify–write through the *place*, not
        // the value copy the ordinary walk would produce
        if let Some(last) = primary.chain.last()
            && let PrimaryRight::Postfix { operator, span } = last
            && matches!(
                operator.value,
                PostfixOperator::Increment | PostfixOperator::Decrement
            )
        {
            let place = self.place_upto(ctx, primary, primary.chain.len() - 1);
            let Some((value, ty)) = self.read_place(ctx, place, span.clone()) else {
                return Piece::Error;
            };
            let old = self.temp_for(&ty);
            self.copy(value, old);
            let one = self.int_constant(1);
            let op = if operator.value == PostfixOperator::Increment {
                BinaryOperator::Add
            } else {
                BinaryOperator::Subtract
            };
            if let Some(updated) = self.emit_binary_operator(
                ctx,
                op,
                (value, &ty),
                (one, &ty),
                &ty,
                span.clone(),
                Some(EntityID::from(last)),
            ) {
                let place = self.place_upto(ctx, primary, primary.chain.len() - 1);
                self.write_place(ctx, place, updated, span.clone());
            }
            return Piece::Value(old, ty);
        }

        let mut piece = self.lower_left(ctx, &primary.left);
        for right in primary.chain {
            piece = self.apply_right(ctx, piece, right);
        }
        piece
    }

    fn lower_left(&mut self, ctx: &mut Ctx<'ast>, left: &'ast PrimaryLeft<'ast, 'ast>) -> Piece {
        match left {
            PrimaryLeft::Literal(literal) => self.lower_literal(ctx, literal),
            // `nameof(Hit)`, `nameof(Other.Ping)`: the spelling of the last
            // name — a constant, as in C#
            PrimaryLeft::Nameof { value, span, .. } => {
                match value.as_ref().ok().and_then(nameof_text) {
                    Some(text) => {
                        let slot = self.string_constant(text);
                        Piece::Value(slot, self.corlib_type("String"))
                    }
                    None => {
                        self.error(ctx, "this `nameof` operand is not supported", span.clone());
                        Piece::Error
                    }
                }
            }
            PrimaryLeft::Identifier { name, span, .. } => {
                match self.bodies.targets.get(&EntityID::from(left)) {
                    Some(ResolvedTarget::Local) => match ctx.lookup(name.value) {
                        Some((slot, ty)) => Piece::Value(slot, ty),
                        None => {
                            self.error(ctx, "internal: local without a slot", span.clone());
                            Piece::Error
                        }
                    },
                    Some(ResolvedTarget::Member(member)) => {
                        let member = member.clone();
                        if matches!(member.kind, SymbolKind::Method) {
                            let receiver = ctx
                                .this_slot
                                .zip(ctx.this_type.clone())
                                .filter(|_| !member.is_static);
                            return Piece::Pending { receiver };
                        }
                        // enum members and external consts are baked values
                        if let Some((slot, ty)) = self.member_constant(&member) {
                            return Piece::Value(slot, ty);
                        }
                        let receiver = ctx
                            .this_slot
                            .zip(ctx.this_type.clone())
                            .filter(|_| !member.is_static);
                        let place = self.member_place(ctx, &member, receiver, span.clone());
                        match self.read_place(ctx, place, span.clone()) {
                            Some((slot, ty)) => Piece::Value(slot, ty),
                            None => Piece::Error,
                        }
                    }
                    _ => Piece::Pending { receiver: None },
                }
            }
            PrimaryLeft::This(span) => match (ctx.this_slot, ctx.this_type.clone()) {
                (Some(slot), Some(ty)) => Piece::Value(slot, ty),
                // inside a behaviour method `this` is the program itself: it
                // has no slot, but `this.field` / `this.Method()` still work
                // because the member targets resolve without a receiver value
                _ if self.is_entry_member(ctx.key.symbol) => Piece::Pending { receiver: None },
                _ => {
                    self.error(ctx, "`this` is unavailable here", span.clone());
                    Piece::Error
                }
            },
            PrimaryLeft::Parenthesized { expression, .. } => {
                let ty = self.type_of(ctx, expression);
                match self.lower_expression(ctx, expression) {
                    Some(slot) => Piece::Value(slot, ty),
                    None => Piece::Error,
                }
            }
            PrimaryLeft::Base(span) => match (ctx.this_slot, ctx.this_type.clone()) {
                (Some(slot), Some(ty)) => Piece::Base {
                    receiver: Some((slot, ty)),
                },
                // inside a behaviour method there is no `this` value, exactly
                // as for `this` itself — `base.gameObject` still resolves,
                // because the member it names carries its own storage
                _ if self.is_entry_member(ctx.key.symbol) => Piece::Base { receiver: None },
                _ => {
                    self.error(ctx, "`base` is unavailable here", span.clone());
                    Piece::Error
                }
            },
            PrimaryLeft::New(new_expression) => self.lower_new(ctx, new_expression),
            PrimaryLeft::Typeof {
                target_type, span, ..
            } => {
                let ty = target_type
                    .as_ref()
                    .ok()
                    .and_then(|type_ref| {
                        self.bodies
                            .resolved_types
                            .get(&EntityID::from(type_ref))
                            .cloned()
                    })
                    .map(|ty| self.substitute(&ty, &ctx.key.bindings));
                match ty.as_ref().and_then(|ty| self.type_constant(ty)) {
                    Some(slot) => Piece::Value(slot, self.system_type()),
                    None => {
                        self.error(
                            ctx,
                            "`typeof` only works for types Udon knows; a user-defined type \
                             has no `System.Type` on the VM",
                            span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            PrimaryLeft::Predefined(_) | PrimaryLeft::Global(_) => {
                Piece::Pending { receiver: None }
            }
            PrimaryLeft::Default {
                target_type, span, ..
            } => {
                let ty = target_type
                    .as_ref()
                    .and_then(|type_ref| {
                        self.bodies
                            .resolved_types
                            .get(&EntityID::from(type_ref))
                            .cloned()
                    })
                    // a bare `default` takes the type the checker gave the
                    // context, recorded on this very node
                    .or_else(|| {
                        self.bodies
                            .expression_types
                            .get(&EntityID::from(left))
                            .cloned()
                    })
                    .map(|ty| self.substitute(&ty, &ctx.key.bindings));
                match ty {
                    Some(ty) => {
                        let slot = self.default_value_in(ctx, &ty, span.clone());
                        Piece::Value(slot, ty)
                    }
                    None => {
                        self.error(
                            ctx,
                            "the type of this `default` could not be determined",
                            span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            other => {
                self.error(
                    ctx,
                    "this expression is not supported by the Udon backend yet",
                    other.span(),
                );
                Piece::Error
            }
        }
    }

    pub(super) fn default_value(&mut self, ty: &Type) -> DataId {
        // a source enum is its underlying Int32
        if self.source_enum(ty).is_some() {
            return self.int_constant(0);
        }
        // an external enum's default is the real boxed zero
        if let Some(id) = self.external_enum(ty) {
            let dotnet_type = self.external.display_name(id);
            let udon_type = self.heap_type(ty);
            return self.constant(
                &udon_type,
                &format!("{dotnet_type}#0"),
                HeapInit::EnumValue {
                    dotnet_type,
                    value: 0,
                },
            );
        }
        match self.extern_type_name(ty).as_deref() {
            Some("SystemInt32") => self.int_constant(0),
            Some("SystemInt64") => self.constant("SystemInt64", "0", HeapInit::Int64(0)),
            Some("SystemUInt32") => self.constant("SystemUInt32", "0", HeapInit::UInt32(0)),
            Some("SystemChar") => self.constant("SystemChar", "0", HeapInit::Char('\0')),
            Some("SystemBoolean") => {
                self.constant("SystemBoolean", "false", HeapInit::Boolean(false))
            }
            Some("SystemSingle") => self.constant("SystemSingle", "0", HeapInit::Single(0.0)),
            Some("SystemDouble") => self.constant("SystemDouble", "0", HeapInit::Double(0.0)),
            // any other value type (`Vector3`, `Color`, ...): a slot declared
            // with the struct's own type and no value — the Udon heap
            // initialises such a slot to `default(T)`, whereas a null
            // *object* copied into it would be a null, not a zeroed struct
            Some(udon_type) if !self.is_reference_type(ty) && udon_type != "SystemObject" => {
                let udon_type = udon_type.to_string();
                self.constant(&udon_type, "default", HeapInit::Null)
            }
            _ => self.constant("SystemObject", "null", HeapInit::Null),
        }
    }

    fn apply_right(
        &mut self,
        ctx: &mut Ctx<'ast>,
        piece: Piece,
        right: &'ast PrimaryRight<'ast, 'ast>,
    ) -> Piece {
        match right {
            PrimaryRight::Member { span, .. } => {
                // `base.M` keeps its static binding across the method group,
                // so the invocation that follows can suppress dispatch
                let from_base = matches!(piece, Piece::Base { .. });
                let receiver = piece.receiver();
                // a method group carries the base binding to the invocation;
                // note the checker records no target for most method groups,
                // so the fallback below must carry it too
                let group = |receiver| {
                    if from_base {
                        Piece::Base { receiver }
                    } else {
                        Piece::Pending { receiver }
                    }
                };
                match self.bodies.targets.get(&EntityID::from(right)) {
                    Some(ResolvedTarget::Member(member)) => {
                        let member = member.clone();
                        if matches!(member.kind, SymbolKind::Method) {
                            return group(receiver);
                        }
                        // enum members and external consts are baked values
                        if let Some((slot, ty)) = self.member_constant(&member) {
                            return Piece::Value(slot, ty);
                        }
                        let place = self.member_place(ctx, &member, receiver, span.clone());
                        match self.read_place(ctx, place, span.clone()) {
                            Some((slot, ty)) => Piece::Value(slot, ty),
                            None => Piece::Error,
                        }
                    }
                    // method group or namespace/type segment
                    _ => group(receiver),
                }
            }
            PrimaryRight::Invocation { arguments, span } => {
                let non_virtual = matches!(piece, Piece::Base { .. });
                let receiver = piece.receiver();
                match self.bodies.targets.get(&EntityID::from(right)) {
                    Some(ResolvedTarget::Call(call)) => {
                        let call = call.clone();
                        self.emit_call(
                            ctx,
                            &call,
                            receiver,
                            arguments.arguments,
                            span.clone(),
                            non_virtual,
                        )
                    }
                    _ => {
                        self.error(ctx, "this call could not be resolved", span.clone());
                        Piece::Error
                    }
                }
            }
            PrimaryRight::ElementAccess { span, .. } => {
                let receiver = piece.receiver();
                // string indexing is special-cased by the checker
                if let Some((slot, ty)) = &receiver
                    && self
                        .extern_type_name(ty)
                        .is_some_and(|name| name == "SystemString")
                {
                    let PrimaryRight::ElementAccess { arguments, .. } = right else {
                        unreachable!()
                    };
                    let index =
                        arguments
                            .arguments
                            .first()
                            .and_then(|argument| match &argument.value {
                                ArgumentValue::Expression(expression) => {
                                    self.lower_expression(ctx, expression)
                                }
                                _ => None,
                            });
                    let Some(index) = index else {
                        return Piece::Error;
                    };
                    let out = self.temp("SystemChar");
                    let char_type = ty.clone();
                    self.call_extern(
                        ctx,
                        "SystemString.__get_Chars__SystemInt32__SystemChar",
                        &[*slot, index, out],
                        span.clone(),
                    );
                    let _ = char_type;
                    return Piece::Value(out, Type::Error);
                }
                let PrimaryRight::ElementAccess { arguments, .. } = right else {
                    unreachable!()
                };
                let place =
                    self.element_place(ctx, receiver, arguments.arguments, span.clone(), right);
                match self.read_place(ctx, place, span.clone()) {
                    Some((slot, ty)) => Piece::Value(slot, ty),
                    None => Piece::Error,
                }
            }
            PrimaryRight::Postfix { operator, span } => match operator.value {
                PostfixOperator::Increment | PostfixOperator::Decrement => {
                    // the piece was already evaluated as a value; `x++` as an
                    // expression statement is the common case, so re-derive
                    // the place from the value slot when it was a simple slot
                    let Piece::Value(slot, ty) = piece else {
                        return Piece::Error;
                    };
                    let old = self.temp_for(&ty);
                    self.copy(slot, old);
                    let one = self.int_constant(1);
                    let op = if operator.value == PostfixOperator::Increment {
                        BinaryOperator::Add
                    } else {
                        BinaryOperator::Subtract
                    };
                    if let Some(updated) = self.emit_binary_operator(
                        ctx,
                        op,
                        (slot, &ty),
                        (one, &ty),
                        &ty,
                        span.clone(),
                        Some(EntityID::from(right)),
                    ) {
                        self.copy(updated, slot);
                    }
                    Piece::Value(old, ty)
                }
                PostfixOperator::NullForgiving => piece,
            },
        }
    }

    // ---------------------------------------------------------------- calls

    fn emit_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: Option<(DataId, Type)>,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
        // `base.M()`: bind to the resolved implementation instead of
        // dispatching, so an override calling its base does not re-enter itself
        non_virtual: bool,
    ) -> Piece {
        // `ref`/`out` slots standing in for a field, element or property: the
        // extern writes the slot, and afterwards the slot is written home
        let mut write_backs: Vec<(Place, DataId)> = Vec::new();
        // for calls into source: the callee's `ref`/`out` parameter slots and
        // where each is copied back to (argument index, place)
        let mut source_by_ref: Vec<(usize, Place)> = Vec::new();
        let parameter_offset = usize::from(call.is_extension);
        // named arguments may sit in any order: each is evaluated where it is
        // written and lands in its parameter's position
        let slot_of: Vec<usize> = (0..arguments.len())
            .map(|index| {
                call.parameter_of_argument
                    .get(index + parameter_offset)
                    .copied()
                    .unwrap_or(index + parameter_offset)
                    - parameter_offset
            })
            .collect();
        // one slot per parameter: the ones no argument fills take defaults
        let mut ordered: Vec<Option<DataId>> = vec![
            None;
            call.signature
                .parameters
                .len()
                .saturating_sub(parameter_offset)
                .max(arguments.len())
        ];
        for (index, argument) in arguments.iter().enumerate() {
            let slot = slot_of[index];
            match argument.modifier.as_ref().map(|modifier| modifier.value) {
                Some(modifier @ (ArgumentModifier::Ref | ArgumentModifier::Out)) => {
                    let Some(parameter) = call.signature.parameters.get(slot + parameter_offset)
                    else {
                        self.error(
                            ctx,
                            "internal: argument without a matching parameter",
                            argument.span.clone(),
                        );
                        return Piece::Error;
                    };
                    let parameter_type =
                        self.substitute(&parameter.parameter_type, &ctx.key.bindings);
                    if matches!(call.origin, MemberOrigin::External { .. }) {
                        // an extern takes every parameter by heap address, so
                        // a variable's own slot *is* the reference
                        match self.by_ref_argument(ctx, argument, modifier, &parameter_type) {
                            Some((reference, write_back)) => {
                                ordered[slot] = Some(reference);
                                if let Some(place) = write_back {
                                    write_backs.push((place, reference));
                                }
                            }
                            None => return Piece::Error,
                        }
                    } else {
                        // a source method writes its own parameter slot; the
                        // call copies it back into the argument's place after
                        match self.source_by_ref_argument(ctx, argument, modifier, &parameter_type)
                        {
                            Some((value, place)) => {
                                ordered[slot] = Some(value);
                                source_by_ref.push((slot, place));
                            }
                            None => return Piece::Error,
                        }
                    }
                }
                Some(ArgumentModifier::In) | None => match &argument.value {
                    ArgumentValue::Expression(expression) => {
                        // converted to the parameter's type (a `params`
                        // element goes to the element type)
                        let target = call.signature.parameters.get(slot + parameter_offset).map(
                            |parameter| {
                                let ty =
                                    self.substitute(&parameter.parameter_type, &ctx.key.bindings);
                                let written = self.type_of(ctx, expression);
                                match (&ty, parameter.is_params) {
                                    (Type::Array { element, .. }, true)
                                        if !matches!(written, Type::Array { .. }) =>
                                    {
                                        (**element).clone()
                                    }
                                    _ => ty,
                                }
                            },
                        );
                        let lowered = match target {
                            Some(target) => self.owned_value_as(ctx, expression, &target),
                            None => self.owned_value(ctx, expression),
                        };
                        match lowered {
                            Some(value) => ordered[slot] = Some(value),
                            None => return Piece::Error,
                        }
                    }
                    _ => {
                        self.error(
                            ctx,
                            "this argument form is not supported by the Udon backend yet",
                            argument.span.clone(),
                        );
                        return Piece::Error;
                    }
                },
            }
        }
        // the expanded form of `params`: the trailing arguments become one array
        if !self.pack_params_arguments(ctx, call, &mut ordered, parameter_offset, &span) {
            return Piece::Error;
        }
        // optional parameters the call left out take their declared default
        let mut values = Vec::with_capacity(ordered.len());
        for (slot, value) in ordered.into_iter().enumerate() {
            let value = match value {
                Some(value) => value,
                None => match self.default_argument(ctx, call, slot + parameter_offset, &span) {
                    Some(value) => value,
                    None => return Piece::Error,
                },
            };
            values.push(value);
        }
        self.dispatch_call(
            ctx,
            call,
            receiver,
            values,
            write_backs,
            source_by_ref,
            span,
            non_virtual,
        )
    }

    /// By-value arguments of a constructor call, evaluated in written order
    /// and returned in parameter order (named arguments may reorder them).
    pub(super) fn constructor_arguments(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        arguments: &'ast [Argument<'ast, 'ast>],
    ) -> Option<Vec<DataId>> {
        let mut ordered: Vec<Option<DataId>> = vec![None; call.signature.parameters.len()];
        for (index, argument) in arguments.iter().enumerate() {
            let ArgumentValue::Expression(expression) = &argument.value else {
                continue;
            };
            let slot = call
                .parameter_of_argument
                .get(index)
                .copied()
                .unwrap_or(index);
            let target = call.signature.parameters.get(slot).map(|parameter| {
                let ty = self.substitute(&parameter.parameter_type, &ctx.key.bindings);
                let written = self.type_of(ctx, expression);
                match (&ty, parameter.is_params) {
                    (Type::Array { element, .. }, true)
                        if !matches!(written, Type::Array { .. }) =>
                    {
                        (**element).clone()
                    }
                    _ => ty,
                }
            });
            let value = match target {
                Some(target) => self.owned_value_as(ctx, expression, &target)?,
                None => self.owned_value(ctx, expression)?,
            };
            if slot < ordered.len() {
                ordered[slot] = Some(value);
            }
        }
        let span = arguments
            .first()
            .map(|argument| argument.span.clone())
            .unwrap_or(0..0);
        if !self.pack_params_arguments(ctx, call, &mut ordered, 0, &span) {
            return None;
        }
        let mut values = Vec::with_capacity(ordered.len());
        for (slot, value) in ordered.into_iter().enumerate() {
            values.push(match value {
                Some(value) => value,
                None => self.default_argument(ctx, call, slot, &span)?,
            });
        }
        Some(values)
    }

    /// When the call bound in the expanded form of a `params` parameter,
    /// gathers the arguments beyond the ordinary parameters into a fresh
    /// array — `new T[] { a, b, c }` — that takes the parameter's place, so
    /// the callee sees exactly what a written-out array would give it.
    fn pack_params_arguments(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        ordered: &mut Vec<Option<DataId>>,
        parameter_offset: usize,
        span: &Range<usize>,
    ) -> bool {
        let Some(fixed) = call.params_expansion else {
            return true;
        };
        let Some(array_parameter) = call.signature.parameters.get(fixed) else {
            self.error(
                ctx,
                "internal: params expansion without a params parameter",
                span.clone(),
            );
            return false;
        };
        let array_type = self.substitute(&array_parameter.parameter_type, &ctx.key.bindings);
        let first = fixed.saturating_sub(parameter_offset);
        let elements: Vec<DataId> = ordered
            .get(first..)
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .collect();
        let size = self.int_constant(elements.len() as i32);
        let array = self.allocate_array(ctx, &array_type, size, span.clone());
        for (position, element) in elements.into_iter().enumerate() {
            let index = self.int_constant(position as i32);
            self.array_set(ctx, array, index, element, &array_type, span.clone());
        }
        ordered.truncate(first);
        ordered.push(Some(array));
        true
    }

    /// The value of an optional parameter the call did not supply: the
    /// declaration's own `= expression` for a source method (a constant, so
    /// it lowers the same at any call site), a metadata constant, `null` or
    /// `default` for an extern.
    fn default_argument(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        parameter_index: usize,
        span: &Range<usize>,
    ) -> Option<DataId> {
        use men_sharp_semantics::DefaultArgument;
        let Some(parameter) = call.signature.parameters.get(parameter_index) else {
            self.error(ctx, "internal: an argument was left unbound", span.clone());
            return None;
        };
        let parameter_type = self.substitute(&parameter.parameter_type, &ctx.key.bindings);
        let default = parameter.default_value.clone();
        match (default, &call.origin) {
            (Some(DefaultArgument::Source), MemberOrigin::Source(symbol)) => {
                let site = self
                    .declarations
                    .table
                    .symbol(*symbol)
                    .declarations
                    .first()?;
                let parameters = match &site.syntax {
                    SyntaxRef::Method(declaration) => declaration.parameters.as_ref().ok(),
                    SyntaxRef::Constructor(declaration) => declaration.parameters.as_ref().ok(),
                    _ => None,
                };
                let Some(expression) = parameters
                    .and_then(|list| list.parameters.get(parameter_index))
                    .and_then(|parameter| parameter.default_value.as_ref())
                else {
                    self.error(
                        ctx,
                        "internal: optional parameter without a default",
                        span.clone(),
                    );
                    return None;
                };
                let value = self.lower_expression(ctx, expression)?;
                let written_type = self.type_of(ctx, expression);
                Some(self.convert(ctx, value, &written_type, &parameter_type, span.clone()))
            }
            (Some(DefaultArgument::Constant(constant)), _) => {
                let value = self.typed_constant(&constant, &parameter_type);
                if value.is_none() {
                    self.error(
                        ctx,
                        "this optional parameter's default has no Udon representation",
                        span.clone(),
                    );
                }
                value
            }
            (Some(DefaultArgument::Null), _) => {
                Some(self.constant("SystemObject", "null", HeapInit::Null))
            }
            (Some(DefaultArgument::Default), _) => {
                Some(self.default_value_in(ctx, &parameter_type, span.clone()))
            }
            _ => {
                self.error(ctx, "internal: an argument was left unbound", span.clone());
                None
            }
        }
    }

    /// The second half of a call: arguments already evaluated (`values`, one
    /// slot each, `ref`/`out` stand-ins included), bind the resolved member and
    /// emit it. Collection initializers and `foreach` come in here directly,
    /// since their `Add`/`MoveNext` calls have no argument syntax.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: Option<(DataId, Type)>,
        mut values: Vec<DataId>,
        write_backs: Vec<(Place, DataId)>,
        source_by_ref: Vec<(usize, Place)>,
        span: Range<usize>,
        non_virtual: bool,
    ) -> Piece {
        let parameter_offset = usize::from(call.is_extension);
        if call.is_extension {
            let Some((slot, _)) = &receiver else {
                self.error(ctx, "internal: extension call without a receiver", span);
                return Piece::Error;
            };
            values.insert(0, *slot);
        }

        let return_type = self.substitute(&call.signature.return_type, &ctx.key.bindings);
        match &call.origin {
            MemberOrigin::Source(symbol) => {
                let symbol = *symbol;
                // the corlib's program-search intrinsics are lowered in place
                if let Some(piece) =
                    self.try_program_intrinsic(ctx, call, symbol, &values, span.clone())
                {
                    return piece;
                }
                // calling into *another* behaviour: Udon has no cross-program
                // call, only "raise this event by name" — so that is what a
                // method call becomes
                if let Some((slot, receiver_type)) = &receiver
                    && self.is_program_reference(receiver_type)
                {
                    let slot = *slot;
                    // `door.SendCustomEvent(...)`: MenSharpBehaviour's own
                    // members are Udon's operations on the other program
                    if let Some(piece) =
                        self.try_marker_member_call(ctx, call, symbol, slot, &values, span.clone())
                    {
                        return piece;
                    }
                    return self.cross_program_call(
                        ctx,
                        call,
                        symbol,
                        slot,
                        &values,
                        &source_by_ref,
                        span,
                    );
                }
                if !call.is_extension
                    && self.has_no_instance_to_read_from(ctx, symbol, call.is_static, &receiver)
                {
                    self.no_instance_error(ctx, symbol, span);
                    return Piece::Error;
                }
                let mut this = if call.is_static || call.is_extension {
                    None
                } else {
                    receiver.as_ref().map(|(slot, _)| *slot).or(ctx.this_slot)
                };
                if !call.is_static
                    && !call.is_extension
                    && let Some((slot, receiver_type)) = &receiver
                    && Some(*slot) != ctx.this_slot
                    && self.has_type_id(receiver_type)
                    && !self.is_source_struct(receiver_type)
                {
                    self.check_not_null(ctx, *slot, span.clone());
                }
                // a behaviour needs no dispatcher: there is one instance, so
                // the most derived override is known here
                let symbol = if !call.is_static
                    && !non_virtual
                    && self.is_virtual(symbol)
                    && self.is_entry_member(symbol)
                {
                    self.entry_override(symbol)
                } else {
                    symbol
                };
                let key = if !call.is_static
                    && !non_virtual
                    && self.is_virtual(symbol)
                    && !self.is_entry_member(symbol)
                {
                    // a struct or sealed receiver has no subtypes: bind the
                    // implementation directly (the "static dispatch" a
                    // monomorphized `T : IShape` allows); otherwise the stub
                    // that compares type ids at runtime
                    let receiver_type = receiver
                        .as_ref()
                        .map(|(_, ty)| self.substitute(ty, &ctx.key.bindings));
                    let bindings =
                        self.bindings_for(ctx, symbol, &call.declaring_type, &call.type_arguments);
                    let direct = receiver_type.as_ref().and_then(|receiver_type| {
                        self.direct_implementation(receiver_type, symbol, &bindings, Role::Method)
                    });
                    match direct {
                        Some(key) => {
                            // a default interface body runs on a *boxed* copy
                            // of a struct (§18.6.9): its writes stay in the box
                            if self.is_interface_member(key.symbol)
                                && let (Some(slot), Some(struct_type)) = (this, &receiver_type)
                                && self.is_source_struct(struct_type)
                            {
                                this =
                                    Some(self.clone_struct(ctx, slot, struct_type, span.clone()));
                            }
                            key
                        }
                        None => self.dispatcher_for(
                            ctx,
                            symbol,
                            &call.declaring_type,
                            &call.type_arguments,
                            Role::Method,
                        ),
                    }
                } else {
                    FunctionKey {
                        symbol,
                        role: Role::Method,
                        bindings: self.bindings_for(
                            ctx,
                            symbol,
                            &call.declaring_type,
                            &call.type_arguments,
                        ),
                    }
                };
                // the inserted extension receiver shifts every argument right
                let by_ref: Vec<(usize, Place)> = source_by_ref
                    .into_iter()
                    .map(|(index, place)| (index + parameter_offset, place))
                    .collect();
                match self.call_function(ctx, &key, this, &values, &by_ref, span) {
                    Some(result) => Piece::Value(result, return_type),
                    None if return_type == Type::Void => Piece::Void,
                    None => Piece::Error,
                }
            }
            MemberOrigin::External { member, .. } => {
                let member_name = member.name.clone();
                // `Equals`/`GetHashCode`/`ToString` on `object` or on a type
                // of the user's: the override the runtime type selects — the
                // reference-identity extern only when nothing overrides
                if !call.is_static
                    && matches!(member_name.as_str(), "Equals" | "GetHashCode" | "ToString")
                {
                    let receiver_value = match &receiver {
                        Some((slot, receiver_type)) => Some((*slot, receiver_type.clone())),
                        None => ctx.this_slot.zip(ctx.this_type.clone()),
                    };
                    if let Some((slot, receiver_type)) = receiver_value {
                        let receiver_type = self.substitute(&receiver_type, &ctx.key.bindings);
                        if Some(slot) != ctx.this_slot
                            && self.has_type_id(&receiver_type)
                            && !self.is_source_struct(&receiver_type)
                        {
                            self.check_not_null(ctx, slot, span.clone());
                        }
                        if let Some(piece) = self.object_member_call(
                            ctx,
                            &member_name,
                            (slot, receiver_type),
                            &values,
                            &return_type,
                            span.clone(),
                        ) {
                            return piece;
                        }
                    }
                }
                // `GetComponent<Door>()`: a program is found by asking, not
                // by engine type — see `components`
                if let Some(piece) =
                    self.try_get_component(ctx, call, &receiver, &values, span.clone())
                {
                    return piece;
                }
                let Some(signature) = self.external_signature(ctx, call, &span) else {
                    return Piece::Error;
                };
                let mut pushed = Vec::new();
                if !call.is_static && !member_name.starts_with(".ctor") {
                    match &receiver {
                        Some((slot, _)) => pushed.push(*slot),
                        None => {
                            if let Some(this) = ctx.this_slot {
                                pushed.push(this);
                            }
                        }
                    }
                }
                pushed.extend(values.iter().copied());
                // a generic extern takes its type argument as a value, after
                // the ordinary parameters and before the result
                for argument in &call.type_arguments {
                    let argument = self.substitute(argument, &ctx.key.bindings);
                    match self.type_constant(&argument) {
                        Some(slot) => pushed.push(slot),
                        None => {
                            self.error(
                                ctx,
                                "this type argument has no `System.Type` Udon can name",
                                span,
                            );
                            return Piece::Error;
                        }
                    }
                }
                let result = if return_type == Type::Void {
                    None
                } else {
                    Some(self.temp_for(&return_type))
                };
                pushed.extend(result);
                self.call_extern(ctx, &signature, &pushed, span.clone());
                for (place, slot) in write_backs {
                    self.write_place(ctx, place, slot, span.clone());
                }
                match result {
                    Some(result) => Piece::Value(result, return_type),
                    None => Piece::Void,
                }
            }
        }
    }

    /// The heap slot to push for a `ref`/`out` argument of an extern call, and
    /// the place to copy the slot back into afterwards when it is a stand-in.
    ///
    /// An extern takes every parameter by heap address, so a plain variable's
    /// own slot is the reference — the extern writes straight into it. A
    /// location without a slot of its own (a field, an array element, a
    /// property) gets a temporary instead: holding the current value for
    /// `ref`, left for the extern to fill for `out`, written home either way.
    fn by_ref_argument(
        &mut self,
        ctx: &mut Ctx<'ast>,
        argument: &'ast Argument<'ast, 'ast>,
        modifier: ArgumentModifier,
        parameter_type: &Type,
    ) -> Option<(DataId, Option<Place>)> {
        match &argument.value {
            ArgumentValue::Expression(expression) => {
                let place = self.lower_place(ctx, expression);
                match place {
                    Place::Slot(slot, _) => Some((slot, None)),
                    Place::SelfReference { .. } => {
                        self.error(
                            ctx,
                            "this is read-only, so it cannot be a `ref`/`out` argument",
                            argument.span.clone(),
                        );
                        None
                    }
                    // lower_place already reported what was wrong
                    Place::Error => None,
                    other => {
                        let slot = match modifier {
                            // the callee may read before writing, so the
                            // current value has to be there first
                            ArgumentModifier::Ref => {
                                let (value, _) =
                                    self.read_place(ctx, other.clone(), argument.span.clone())?;
                                value
                            }
                            _ => self.temp_for(parameter_type),
                        };
                        Some((slot, Some(other)))
                    }
                }
            }
            // `out var x` / `out RaycastHit x`: the call site declares the
            // variable, and its slot is the reference
            ArgumentValue::Declaration { name, .. } => {
                let slot = self.temp_for(parameter_type);
                ctx.locals
                    .last_mut()
                    .expect("a scope is open")
                    .insert(name.value, (slot, parameter_type.clone()));
                Some((slot, None))
            }
            ArgumentValue::Missing => None,
        }
    }

    /// A `ref`/`out` argument of a call into *source*: the value passed in
    /// (the current one for `ref`; a placeholder for `out`) and the place the
    /// callee's parameter slot is copied back into afterwards.
    fn source_by_ref_argument(
        &mut self,
        ctx: &mut Ctx<'ast>,
        argument: &'ast Argument<'ast, 'ast>,
        modifier: ArgumentModifier,
        parameter_type: &Type,
    ) -> Option<(DataId, Place)> {
        match &argument.value {
            ArgumentValue::Expression(expression) => {
                let place = self.lower_place(ctx, expression);
                match place {
                    Place::SelfReference { .. } => {
                        self.error(
                            ctx,
                            "this is read-only, so it cannot be a `ref`/`out` argument",
                            argument.span.clone(),
                        );
                        None
                    }
                    Place::Error => None,
                    place => {
                        let value = match (&place, modifier) {
                            (Place::Slot(slot, _), _) => *slot,
                            (_, ArgumentModifier::Ref) => {
                                self.read_place(ctx, place.clone(), argument.span.clone())?
                                    .0
                            }
                            _ => self.temp_for(parameter_type),
                        };
                        Some((value, place))
                    }
                }
            }
            // `out var x`: the call site declares the variable; the call
            // writes it through the place like any other
            ArgumentValue::Declaration { name, .. } => {
                let slot = self.temp_for(parameter_type);
                ctx.locals
                    .last_mut()
                    .expect("a scope is open")
                    .insert(name.value, (slot, parameter_type.clone()));
                Some((slot, Place::Slot(slot, parameter_type.clone())))
            }
            ArgumentValue::Missing => None,
        }
    }

    // ------------------------------------------------------------------ new

    fn lower_new(
        &mut self,
        ctx: &mut Ctx<'ast>,
        new_expression: &'ast NewExpression<'ast, 'ast>,
    ) -> Piece {
        let span = new_expression.span.clone();

        // array creation
        if !new_expression.array_sizes.is_empty() {
            if new_expression.array_sizes.len() > 1 || !new_expression.array_suffixes.is_empty() {
                self.error(
                    ctx,
                    "multi-dimensional arrays are not supported by the Udon backend yet",
                    span,
                );
                return Piece::Error;
            }
            let element = new_expression
                .created_type
                .as_ref()
                .and_then(|type_ref| {
                    self.bodies
                        .resolved_types
                        .get(&EntityID::from(type_ref))
                        .cloned()
                })
                .map(|ty| self.substitute(&ty, &ctx.key.bindings));
            let Some(element) = element else {
                self.error(ctx, "could not resolve the array element type", span);
                return Piece::Error;
            };
            let array_type = Type::Array {
                element: Box::new(element),
                rank: 1,
            };
            let Some(size) = self.lower_expression(ctx, &new_expression.array_sizes[0]) else {
                return Piece::Error;
            };
            let slot = self.allocate_array(ctx, &array_type, size, span.clone());
            // element initializers
            self.fill_array_initializer(ctx, slot, &array_type, new_expression, span);
            return Piece::Value(slot, array_type);
        }

        // object creation
        let created = new_expression
            .created_type
            .as_ref()
            .and_then(|type_ref| {
                self.bodies
                    .resolved_types
                    .get(&EntityID::from(type_ref))
                    .cloned()
            })
            // `new[] { ... }` writes no type; the checker recorded the
            // best-common-type array on the node itself
            .or_else(|| {
                self.bodies
                    .expression_types
                    .get(&EntityID::from(new_expression))
                    .cloned()
            })
            .map(|ty| self.substitute(&ty, &ctx.key.bindings));
        let Some(created) = created else {
            self.error(
                ctx,
                "target-typed `new(...)` is not supported by the Udon backend yet",
                span,
            );
            return Piece::Error;
        };

        // `new int[] { 1, 2 }` / `new[] { 1, 2 }`: no written size — it is
        // the element count
        if let Type::Array { rank: 1, .. } = &created {
            use men_sharp_parser::ast::{CollectionElement, Initializer};
            let count = match &new_expression.initializer {
                Some(Initializer::Collection { elements, .. }) => elements
                    .iter()
                    .filter(|element| matches!(element, CollectionElement::Expression(_)))
                    .count(),
                _ => 0,
            };
            let size = self.int_constant(count as i32);
            let slot = self.allocate_array(ctx, &created, size, span.clone());
            self.fill_array_initializer(ctx, slot, &created, new_expression, span);
            return Piece::Value(slot, created);
        }

        let target = self
            .bodies
            .targets
            .get(&EntityID::from(new_expression))
            .cloned();

        if let Type::Named {
            target: TypeTarget::Source(created_symbol),
            ..
        } = &created
            && self.entry_class == Some(*created_symbol)
        {
            self.error(
                ctx,
                "a behaviour cannot be constructed with `new` — Unity creates it when the component is added",
                span,
            );
            return Piece::Error;
        }

        if self.is_source_class(&created) {
            let Some(layout) = self.layout_of(&created) else {
                return Piece::Error;
            };
            let object = self.temp("SystemObjectArray");
            let size = self.int_constant(layout.size as i32);
            self.call_extern(
                ctx,
                "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
                &[size, object],
                span.clone(),
            );
            let zero = self.int_constant(0);
            let type_id = self.int_constant(layout.type_id);
            self.set_element(ctx, object, zero, type_id, span.clone());
            self.stamp_exception_type(ctx, object, &created);

            // run the constructor
            let arguments = new_expression
                .arguments
                .as_ref()
                .map(|list| list.arguments)
                .unwrap_or(&[]);
            match target {
                Some(ResolvedTarget::Call(call)) => {
                    let Some(values) = self.constructor_arguments(ctx, &call, arguments) else {
                        return Piece::Error;
                    };
                    if let MemberOrigin::Source(ctor) = call.origin {
                        let key = FunctionKey {
                            symbol: ctor,
                            role: Role::Constructor,
                            bindings: self.bindings_for(ctx, ctor, &call.declaring_type, &[]),
                        };
                        self.call_function(ctx, &key, Some(object), &values, &[], span.clone());
                    }
                }
                _ => {
                    // no declared constructors: the synthesized default one
                    if let Type::Named {
                        target: TypeTarget::Source(class),
                        arguments: class_arguments,
                    } = &created
                    {
                        let parameters = &self.declarations.table.symbol(*class).type_parameters;
                        let bindings = parameters
                            .iter()
                            .copied()
                            .zip(class_arguments.iter().cloned())
                            .collect();
                        let key = FunctionKey {
                            symbol: *class,
                            role: Role::DefaultConstructor,
                            bindings,
                        };
                        self.call_function(ctx, &key, Some(object), &[], &[], span.clone());
                    }
                }
            }

            self.apply_initializer(ctx, object, &created, new_expression, span);
            return Piece::Value(object, created);
        }

        // external type: extern constructor
        match target {
            Some(ResolvedTarget::Call(call)) => {
                let arguments = new_expression
                    .arguments
                    .as_ref()
                    .map(|list| list.arguments)
                    .unwrap_or(&[]);
                let Some(values) = self.constructor_arguments(ctx, &call, arguments) else {
                    return Piece::Error;
                };
                let Some(owner) = self.extern_type_name(&created) else {
                    self.error(ctx, "this type is not available on Udon", span);
                    return Piece::Error;
                };
                let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
                let mut parts = Vec::new();
                for parameter in &signature.parameters {
                    match self.extern_type_name(&parameter.parameter_type) {
                        Some(part) => parts.push(part),
                        None => {
                            self.error(
                                ctx,
                                "a constructor parameter type is not available on Udon",
                                span,
                            );
                            return Piece::Error;
                        }
                    }
                }
                // constructors always carry a parameter section, even empty
                let extern_signature = format!("{owner}.__ctor__{}__{owner}", parts.join("_"));
                let out = self.temp_for(&created);
                let mut pushed = values;
                pushed.push(out);
                self.call_extern(ctx, &extern_signature, &pushed, span.clone());
                self.apply_initializer(ctx, out, &created, new_expression, span);
                Piece::Value(out, created)
            }
            // `new Vector3()`: the parameterless struct constructor is
            // `default` — copied into a slot of its own, since the initializer
            // (or later writes) mutate it in place
            _ if !self.is_reference_type(&created)
                && new_expression
                    .arguments
                    .as_ref()
                    .is_none_or(|list| list.arguments.is_empty()) =>
            {
                let default = self.default_value(&created);
                let out = self.temp_for(&created);
                self.copy(default, out);
                self.apply_initializer(ctx, out, &created, new_expression, span);
                Piece::Value(out, created)
            }
            _ => {
                self.error(
                    ctx,
                    "constructing this type is not supported by the Udon backend yet",
                    span,
                );
                Piece::Error
            }
        }
    }

    pub(super) fn allocate_array(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array_type: &Type,
        size: DataId,
        span: Range<usize>,
    ) -> DataId {
        let owner = self.heap_type(array_type);
        let slot = self.temp(&owner);
        let candidates = [
            format!("{owner}.__ctor__SystemInt32__{owner}"),
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray".to_string(),
        ];
        let signature = candidates
            .iter()
            .find(|signature| self.nodes.has_signature(signature))
            .cloned()
            .unwrap_or_else(|| candidates[1].clone());
        self.call_extern(ctx, &signature, &[size, slot], span.clone());

        // `new S[n]` holds n default values in C#, and a struct's default is
        // an instance — so fill the array, one fresh instance per element
        if let Type::Array { element, rank: 1 } = array_type
            && self.is_source_struct(element)
        {
            let element = (**element).clone();
            let index = self.temp("SystemInt32");
            let zero = self.int_constant(0);
            let one = self.int_constant(1);
            self.copy(zero, index);
            let head = self.fresh_label("fill_head");
            let done = self.fresh_label("fill_done");
            let condition = self.temp("SystemBoolean");
            self.program.code.push(Op::Label(head));
            self.call_extern(
                ctx,
                "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean",
                &[index, size, condition],
                span.clone(),
            );
            self.program.code.push(Op::Push(condition));
            self.program.code.push(Op::JumpIfFalse(Target::Label(done)));
            let value = self.allocate_default_struct(ctx, &element, span.clone());
            self.array_set(ctx, slot, index, value, array_type, span.clone());
            self.call_extern(
                ctx,
                "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
                &[index, one, index],
                span.clone(),
            );
            self.program.code.push(Op::Jump(Target::Label(head)));
            self.program.code.push(Op::Label(done));
        }
        slot
    }

    fn fill_array_initializer(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        array_type: &Type,
        new_expression: &'ast NewExpression<'ast, 'ast>,
        span: Range<usize>,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};
        let Some(Initializer::Collection { elements, .. }) = &new_expression.initializer else {
            return;
        };
        let element_type = match array_type {
            Type::Array { element, .. } => Some((**element).clone()),
            _ => None,
        };
        for (position, element) in elements.iter().enumerate() {
            if let CollectionElement::Expression(expression) = element {
                let value = match &element_type {
                    Some(target) => self.owned_value_as(ctx, expression, target),
                    None => self.owned_value(ctx, expression),
                };
                if let Some(value) = value {
                    let index = self.int_constant(position as i32);
                    self.array_set(ctx, array, index, value, array_type, span.clone());
                }
            }
        }
    }

    /// `new T { ... }` after construction: an object initializer writes
    /// fields, a collection initializer is one `Add` call per element.
    fn apply_initializer(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        created: &Type,
        new_expression: &'ast NewExpression<'ast, 'ast>,
        span: Range<usize>,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};
        match &new_expression.initializer {
            None => {}
            Some(Initializer::Object { elements, .. }) => {
                self.apply_object_initializer(ctx, object, created, elements, span);
            }
            Some(Initializer::Collection { elements, .. }) => {
                for item in *elements {
                    let Some(ResolvedTarget::Call(add)) =
                        self.bodies.targets.get(&EntityID::from(item)).cloned()
                    else {
                        // the checker already said why
                        continue;
                    };
                    // `a` is `Add(a)`; `{ k, v }` is `Add(k, v)`
                    let expressions: Vec<&'ast Expression<'ast, 'ast>> = match item {
                        CollectionElement::Expression(expression) => vec![expression],
                        CollectionElement::Nested(Initializer::Collection { elements, .. }) => {
                            elements
                                .iter()
                                .filter_map(|element| match element {
                                    CollectionElement::Expression(expression) => Some(expression),
                                    CollectionElement::Nested(_) => None,
                                })
                                .collect()
                        }
                        CollectionElement::Nested(Initializer::Object { .. }) => continue,
                    };
                    let mut values = Vec::with_capacity(expressions.len());
                    for expression in expressions {
                        match self.owned_value(ctx, expression) {
                            Some(value) => values.push(value),
                            None => return,
                        }
                    }
                    self.dispatch_call(
                        ctx,
                        &add,
                        Some((object, created.clone())),
                        values,
                        Vec::new(),
                        Vec::new(),
                        span.clone(),
                        false,
                    );
                }
            }
        }
    }

    /// `new T { X = v, ... }`: each element is the write `t.X = v` — fields,
    /// properties with setters, external properties (`__set_X` externs, which
    /// for a struct like `Vector3` write the heap value back in place) all go
    /// through the same place machinery an assignment uses.
    /// `new T { X = v, [k] = v, ... }`: each element is the write `t.X = v`
    /// or `t[k] = v` — fields, properties with setters, external properties
    /// (`__set_X` externs, which for a struct like `Vector3` write the heap
    /// value back in place) and indexers all go through the same place
    /// machinery an assignment uses.
    fn apply_object_initializer(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        created: &Type,
        elements: &'ast [men_sharp_parser::ast::ObjectInitializerElement<'ast, 'ast>],
        span: Range<usize>,
    ) {
        use men_sharp_parser::ast::InitializerTarget;
        for element in elements {
            let target = self.bodies.targets.get(&EntityID::from(element)).cloned();
            match &element.value {
                Ok(InitializerValue::Expression(value)) => {
                    let place = match (&element.target, target) {
                        (InitializerTarget::Member(_), Some(ResolvedTarget::Member(member))) => {
                            self.member_place(
                                ctx,
                                &member,
                                Some((object, created.clone())),
                                span.clone(),
                            )
                        }
                        (
                            InitializerTarget::Index { arguments, .. },
                            Some(ResolvedTarget::Call(call)),
                        ) => {
                            let MemberOrigin::Source(symbol) = call.origin else {
                                self.error(
                                    ctx,
                                    "external indexers are not supported by the Udon backend yet",
                                    element.span.clone(),
                                );
                                continue;
                            };
                            let mut indices = Vec::with_capacity(arguments.len());
                            for argument in arguments.iter() {
                                let Some(index) = self.lower_expression(ctx, argument) else {
                                    continue;
                                };
                                indices.push(index);
                            }
                            let bindings = self.bindings_for(
                                ctx,
                                symbol,
                                &call.declaring_type,
                                &call.type_arguments,
                            );
                            let ty =
                                self.substitute(&call.signature.return_type, &ctx.key.bindings);
                            Place::Accessor {
                                receiver: Some(object),
                                symbol,
                                bindings,
                                indices,
                                ty,
                            }
                        }
                        // the checker already said why
                        _ => continue,
                    };
                    let lowered = match place_type(&place) {
                        Some(target) => self.owned_value_as(ctx, value, &target),
                        None => self.owned_value(ctx, value),
                    };
                    let Some(lowered) = lowered else {
                        continue;
                    };
                    self.write_place(ctx, place, lowered, span.clone());
                }
                Ok(InitializerValue::Nested(nested)) => {
                    self.error(
                        ctx,
                        "a nested initializer inside an object initializer is not supported \
                         by the Udon backend yet: assign the member a `new` expression",
                        nested.span(),
                    );
                }
                Err(()) => {}
            }
        }
    }

    // ------------------------------------------------------------- literals

    fn lower_literal(
        &mut self,
        ctx: &mut Ctx<'ast>,
        literal: &'ast LiteralExpression<'ast, 'ast>,
    ) -> Piece {
        match literal {
            LiteralExpression::Integer(text) => {
                let raw: String = text.value.chars().filter(|c| *c != '_').collect();
                let trimmed = raw.trim_end_matches(['u', 'U', 'l', 'L']);
                let parsed = if let Some(hex) = trimmed
                    .strip_prefix("0x")
                    .or_else(|| trimmed.strip_prefix("0X"))
                {
                    i64::from_str_radix(hex, 16)
                } else if let Some(bin) = trimmed
                    .strip_prefix("0b")
                    .or_else(|| trimmed.strip_prefix("0B"))
                {
                    i64::from_str_radix(bin, 2)
                } else {
                    trimmed.parse::<i64>()
                };
                match parsed {
                    Ok(value) if i32::try_from(value).is_ok() => {
                        let slot = self.int_constant(value as i32);
                        let ty = self.corlib_type("Int32");
                        Piece::Value(slot, ty)
                    }
                    _ => {
                        self.error(
                            ctx,
                            "integer literals outside `int` range are not supported yet",
                            text.span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            LiteralExpression::Real(text) => {
                let raw: String = text.value.chars().filter(|c| *c != '_').collect();
                if let Some(single) = raw.strip_suffix(['f', 'F']) {
                    match single.parse::<f32>() {
                        Ok(value) => {
                            let slot = self.constant(
                                "SystemSingle",
                                &format!("{value:?}"),
                                HeapInit::Single(value),
                            );
                            Piece::Value(slot, self.corlib_type("Single"))
                        }
                        Err(_) => Piece::Error,
                    }
                } else {
                    let trimmed = raw.trim_end_matches(['d', 'D', 'm', 'M']);
                    match trimmed.parse::<f64>() {
                        Ok(value) => {
                            let slot = self.constant(
                                "SystemDouble",
                                &format!("{value:?}"),
                                HeapInit::Double(value),
                            );
                            Piece::Value(slot, self.corlib_type("Double"))
                        }
                        Err(_) => Piece::Error,
                    }
                }
            }
            LiteralExpression::String(text) => {
                let inner = text
                    .value
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .unwrap_or(text.value);
                let value = unescape(inner);
                let slot = self.constant("SystemString", &value, HeapInit::Str(value.clone()));
                Piece::Value(slot, self.corlib_type("String"))
            }
            LiteralExpression::VerbatimString(text) => {
                let inner = text
                    .value
                    .strip_prefix("@\"")
                    .and_then(|rest| rest.strip_suffix('"'))
                    .unwrap_or(text.value);
                let value = inner.replace("\"\"", "\"");
                let slot = self.constant("SystemString", &value, HeapInit::Str(value.clone()));
                Piece::Value(slot, self.corlib_type("String"))
            }
            LiteralExpression::Char(text) => {
                let inner = text
                    .value
                    .strip_prefix('\'')
                    .and_then(|rest| rest.strip_suffix('\''))
                    .unwrap_or(text.value);
                let value = unescape(inner).chars().next().unwrap_or('\0');
                let slot = self.constant("SystemChar", &value.to_string(), HeapInit::Char(value));
                Piece::Value(slot, self.corlib_type("Char"))
            }
            LiteralExpression::True(_) => {
                let slot = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
                Piece::Value(slot, self.corlib_type("Boolean"))
            }
            LiteralExpression::False(_) => {
                let slot = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
                Piece::Value(slot, self.corlib_type("Boolean"))
            }
            LiteralExpression::Null(_) => {
                let slot = self.constant("SystemObject", "null", HeapInit::Null);
                Piece::Value(slot, Type::Null)
            }
            LiteralExpression::InterpolatedString(interpolated) => {
                let mut current: Option<DataId> = None;
                for part in interpolated.parts {
                    let piece_slot = match part {
                        InterpolationPart::Text(text) => {
                            let value = if interpolated.is_verbatim || interpolated.is_raw {
                                text.value.to_string()
                            } else {
                                unescape(text.value)
                            };
                            let value = value.replace("{{", "{").replace("}}", "}");
                            if value.is_empty() {
                                continue;
                            }
                            self.constant("SystemString", &value, HeapInit::Str(value.clone()))
                        }
                        InterpolationPart::Hole(hole) => {
                            let Ok(expression) = &hole.expression else {
                                continue;
                            };
                            let ty = self.type_of(ctx, expression);
                            match self.lower_expression(ctx, expression) {
                                Some(slot) => {
                                    self.stringify(ctx, slot, &ty, interpolated.span.clone())
                                }
                                None => continue,
                            }
                        }
                    };
                    current = Some(match current {
                        None => piece_slot,
                        Some(previous) => {
                            let out = self.temp("SystemString");
                            self.call_extern(
                                ctx,
                                "SystemString.__Concat__SystemString_SystemString__SystemString",
                                &[previous, piece_slot, out],
                                interpolated.span.clone(),
                            );
                            out
                        }
                    });
                }
                let slot = current.unwrap_or_else(|| {
                    self.constant("SystemString", "", HeapInit::Str(String::new()))
                });
                Piece::Value(slot, self.corlib_type("String"))
            }
            other => {
                self.error(
                    ctx,
                    "this literal is not supported by the Udon backend yet",
                    other.span(),
                );
                Piece::Error
            }
        }
    }

    /// The corlib type as the semantic model sees it (external `System.X`),
    /// used to type literal slots.
    pub(super) fn corlib_type(&self, name: &str) -> Type {
        match self.external.find_type(&["System"], name, 0) {
            Some(id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => Type::Error,
        }
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(decoded) = char::from_u32(code)
                {
                    out.push(decoded);
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// What `nameof(x.y.z)` spells: the last name of its operand.
fn nameof_text<'a>(expression: &'a Expression<'a, 'a>) -> Option<&'a str> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if let Some(last) = primary.chain.last() {
        return match last {
            PrimaryRight::Member { name, .. } => name.as_ref().ok().map(|name| name.value),
            _ => None,
        };
    }
    match &primary.left {
        PrimaryLeft::Identifier { name, .. } => Some(name.value),
        _ => None,
    }
}

/// The type of what a place holds, for converting a value written to it.
fn place_type(place: &Place) -> Option<Type> {
    match place {
        Place::Slot(_, ty)
        | Place::SelfReference { ty, .. }
        | Place::Field { ty, .. }
        | Place::Accessor { ty, .. }
        | Place::ProgramVariable { ty, .. }
        | Place::ProgramAccessor { ty, .. }
        | Place::ExternalProperty { ty, .. } => Some(ty.clone()),
        Place::Element { element, .. } => Some(element.clone()),
        Place::Error => None,
    }
}
