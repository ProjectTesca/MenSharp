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
        // before the statements: a local function may be called from above
        // its own declaration
        self.register_local_functions(ctx, block);
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
            // registered by the block, compiled when something calls it
            Statement::LocalFunction(_) => {}
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
            Statement::Yield(statement) => self.lower_yield(ctx, statement),
            Statement::Return(ReturnStatement { value, span, .. }) => {
                let mut returned = None;
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
                    // converted to the declared return type — or, in an
                    // async body, to what its task carries
                    let (_, return_type) = self.function_shape(&ctx.key);
                    let return_type = match &ctx.async_state {
                        Some(state) => state.inner.clone(),
                        None => return_type,
                    };
                    returned = lowered.map(|slot| {
                        let from = self.type_of(ctx, value);
                        self.convert(ctx, slot, &from, &return_type, span.clone())
                    });
                    if ctx.async_state.is_none()
                        && let (Some(value), Some(result)) = (returned, ctx.result)
                    {
                        self.copy(value, result);
                    }
                }
                // leaving every `try` region: their `finally` blocks first
                self.emit_finally_copies(ctx, 0);
                // an async body's `return` completes its task
                if ctx.async_state.is_some() {
                    self.complete_async(ctx, returned, span.clone());
                }
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
                        Message::key("codegen.break_outside_a_loop_or_switch"),
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
                    None => self.error(
                        ctx,
                        Message::key("codegen.continue_outside_a_loop"),
                        statement.span.clone(),
                    ),
                }
            }
            other => {
                self.error(
                    ctx,
                    Message::key("codegen.this_statement_is_not_supported_by_the"),
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
                Some(InitializerValue::Nested(_)) | None => None,
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
            // `int[] x = { 1, 2 };`
            if let Some(InitializerValue::Nested(nested)) = &declarator.initializer
                && let Some(array) = self.lower_array_shorthand(ctx, &ty, nested, nested.span())
            {
                self.copy(array, slot);
            }
            self.bind_local(ctx, declarator.name.value, slot, ty);
        }
    }

    fn lower_if(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast IfStatement<'ast, 'ast>) {
        let condition = match &statement.condition {
            Ok(condition) => self.lower_expression(ctx, condition),
            Err(()) => None,
        };
        // a constant condition — `typeof(T) == typeof(int)`, `Reflect
        // .IsArray<T>()` — settles the branch here, and the other branch is
        // not lowered at all: generic code gets to name, in each arm, what
        // exists only for some `T`, and only the arm for this `T` has to
        // compile
        if let Some(value) = condition
            && let Some(taken) = self.constant_boolean(value)
        {
            if taken {
                if let Ok(then_branch) = &statement.then_branch {
                    self.lower_statement(ctx, then_branch);
                }
            } else if let Some(else_branch) = &statement.else_branch {
                self.lower_statement(ctx, else_branch);
            }
            return;
        }
        let else_label = self.fresh_label("if_else");
        let end_label = self.fresh_label("if_end");
        if let Some(value) = condition {
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
        } else if let Type::Array { element, rank } = &collection_type
            && *rank > 1
        {
            // a rectangular array is walked in row-major order — the order
            // its flat data already is in
            let element = (**element).clone();
            self.check_not_null(ctx, value, span.clone());
            let data = self.rectangular_data(ctx, value, &collection_type, span);
            let data_type = Self::rectangular_data_type(&collection_type);
            self.lower_foreach_over_array(ctx, statement, data, data_type, element);
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
                Message::key("codegen.foreach_over_this_type_is_not_supported"),
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
        self.bind_designation(
            ctx,
            name,
            variable,
            &element_type,
            &None,
            statement.span.clone(),
        );

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

        // §13.9.5: an enumerator that can be disposed is disposed however
        // the loop is left — the end, a `break`, a `return`, an exception.
        // That is what runs an iterator's pending `finally` blocks.
        let disposal = enumeration
            .dispose
            .clone()
            .filter(|call| !self.dispose_does_nothing(call, &enumerator_type))
            .map(|call| {
                FinallyAction::Dispose(Box::new(Disposal {
                    call,
                    enumerator,
                    enumerator_type: enumerator_type.clone(),
                }))
            });
        let dispose_handler = self.fresh_label("foreach_dispose");
        if let Some(action) = disposal.clone() {
            ctx.loop_stack.push(BreakFrame::Try {
                handler: dispose_handler,
                finally: Some(action),
            });
        }

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
        self.bind_designation(ctx, name, variable, &element_type, &None, span.clone());

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

        if let Some(action) = disposal {
            // the region ends here, before the disposal itself runs: an
            // exception out of `Dispose` unwinds past this loop, not into
            // its own handler
            ctx.loop_stack.pop();
            let done = self.fresh_label("foreach_disposed");
            self.emit_finally_action(ctx, &action);
            self.program.code.push(Op::Jump(Target::Label(done)));

            // left by an exception: dispose, then keep unwinding
            self.program.code.push(Op::Label(dispose_handler));
            let state = self.exception_state();
            let saved = self.temp("SystemObject");
            self.copy(state.exception, saved);
            let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
            self.copy(cleared, state.pending);
            self.emit_finally_action(ctx, &action);
            self.emit_throw(ctx, saved, span.clone(), false);
            self.program.code.push(Op::Label(done));
        }
        ctx.locals.pop();
    }

    /// A `Dispose()` that provably does nothing: a source method with an
    /// empty body on a sealed type, which is every collection's enumerator.
    /// Skipping it keeps a `foreach` over a list exactly the size it was.
    fn dispose_does_nothing(&self, call: &ResolvedCall, enumerator_type: &Type) -> bool {
        let MemberOrigin::Source(symbol) = call.origin else {
            return false;
        };
        if self.is_interface_member(symbol) || !self.is_final_type(enumerator_type) {
            return false;
        }
        let declarations = &self.declarations.table.symbol(symbol).declarations;
        !declarations.is_empty()
            && declarations.iter().all(|site| {
                matches!(&site.syntax, SyntaxRef::Method(method)
                    if matches!(&method.body, FunctionBody::Block(block)
                        if block.statements.is_empty()))
            })
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
                // a method group where a value is wanted: the checker
                // recorded the method it converts to — a delegate to it
                piece @ (Piece::Pending { .. } | Piece::Base { .. }) => {
                    // `this` as a value inside a behaviour: the program has
                    // no object of its own, but it does have an identity —
                    // its UdonBehaviour, which is what a program reference
                    // is at run time. `(IUdonEventReceiver)this`, `door =
                    // this`, `list.Add(this)` all want that
                    if primary.chain.is_empty()
                        && matches!(primary.left, PrimaryLeft::This(_))
                        && self.is_entry_member(ctx.key.symbol)
                    {
                        let ty = self.type_of(ctx, expression);
                        let slot = self.self_behaviour_slot();
                        let out = self.temp_for(&ty);
                        self.copy(slot, out);
                        return Some(out);
                    }
                    let conversion = self.bodies.targets.get(&EntityID::from(*primary)).cloned();
                    match conversion {
                        Some(ResolvedTarget::Call(call)) => {
                            let ty = self.type_of(ctx, expression);
                            let non_virtual = matches!(piece, Piece::Base { .. });
                            self.method_group_delegate(
                                ctx,
                                &call,
                                piece.receiver(),
                                non_virtual,
                                &ty,
                                expression.span(),
                            )
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            Expression::Lambda(lambda) => self.lower_lambda(ctx, lambda, expression),
            Expression::AnonymousMethod(method) => {
                self.error(
                    ctx,
                    Message::key("codegen.anonymous_methods_delegate_are_not_supported_by"),
                    method.span.clone(),
                );
                None
            }
            Expression::Await(await_expression) => self.lower_await(ctx, await_expression),
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
                // each arm converted to the expression's type: `c ? 1 : 2.5`
                // is a double, `c ? 1 : null` an `int?`
                if let Ok(value) = &conditional.then_value
                    && let Some(slot) = self.owned_value_as(ctx, value, &ty)
                {
                    self.copy(slot, result);
                }
                self.program.code.push(Op::Jump(Target::Label(end_label)));
                self.program.code.push(Op::Label(else_label));
                if let Ok(value) = &conditional.else_value
                    && let Some(slot) = self.owned_value_as(ctx, value, &ty)
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
            Expression::With(with) => self.lower_with(ctx, with),
            Expression::Throw(throw) => self.lower_throw_expression(ctx, throw),
            Expression::As(as_expression) => self.lower_as(ctx, as_expression, expression),
            other => {
                self.error(
                    ctx,
                    Message::key("codegen.this_expression_is_not_supported_by_the"),
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
        if let Some(converted) = self.convert_nullable(ctx, source, from, to, span.clone()) {
            return converted;
        }
        // a rectangular array is an `object[]` of the compiler's own shape:
        // `System.Array`'s externs would read that shape, not the elements
        if Self::rectangular_rank(from).is_some()
            && self
                .extern_type_name(to)
                .is_some_and(|name| name == "SystemArray")
        {
            self.error(
                ctx,
                Message::key("codegen.a_rectangular_array_cannot_be_used_as"),
                span,
            );
            return source;
        }
        // `DataToken t = 1;` — a conversion operator from metadata is an
        // extern like any other
        if let Some(converted) = self.convert_by_operator(ctx, source, from, to, &span) {
            return converted;
        }
        // `Total(numbers)` where `numbers` is an `int[]`: an array becomes a
        // sequence by being wrapped in one
        if let Some(wrapped) = self.sequence_of_array(ctx, source, from, to, &span) {
            return wrapped;
        }
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
                    "SystemSByte" => Some("ToSByte"),
                    "SystemInt16" => Some("ToInt16"),
                    "SystemUInt16" => Some("ToUInt16"),
                    "SystemUInt32" => Some("ToUInt32"),
                    "SystemUInt64" => Some("ToUInt64"),
                    "SystemChar" => Some("ToChar"),
                    "SystemDecimal" => Some("ToDecimal"),
                    _ => None,
                };
                if let Some(method) = method {
                    // `(int)3.7` is 3 in C#: a fraction is dropped, where
                    // `Convert.ToInt32` would round it to the nearest even
                    let (source, from_name) = if matches!(
                        (from_name.as_str(), method),
                        (
                            "SystemSingle" | "SystemDouble",
                            "ToInt32"
                                | "ToInt64"
                                | "ToByte"
                                | "ToSByte"
                                | "ToInt16"
                                | "ToUInt16"
                                | "ToUInt32"
                                | "ToUInt64"
                                | "ToChar"
                        )
                    ) {
                        let double = if from_name == "SystemSingle" {
                            let widened = self.temp("SystemDouble");
                            self.call_extern(
                                ctx,
                                "SystemConvert.__ToDouble__SystemSingle__SystemDouble",
                                &[source, widened],
                                span.clone(),
                            );
                            widened
                        } else {
                            source
                        };
                        let truncated = self.temp("SystemDouble");
                        self.call_extern(
                            ctx,
                            "SystemMath.__Truncate__SystemDouble__SystemDouble",
                            &[double, truncated],
                            span.clone(),
                        );
                        (truncated, "SystemDouble".to_string())
                    } else {
                        (source, from_name.clone())
                    };
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
                    // a number that Udon cannot convert: copying the slot as
                    // it is would leave a value of the wrong type in it
                    if self.nodes.has_signature(&format!(
                        "SystemConvert.__ToInt32__{from_name}__SystemInt32"
                    )) {
                        let (from_display, to_display) =
                            (self.describe_type(from), self.describe_type(to));
                        self.error(
                            ctx,
                            Message::key("codegen.udon_has_no_conversion_from_from_display")
                                .arg("from_display", from_display)
                                .arg("to_display", to_display)
                                .arg("signature", signature),
                            span,
                        );
                    }
                }
                source
            }
            // reference casts and identity: values are untyped objects
            _ => source,
        }
    }

    /// The `op_Implicit` extern that turns `from` into `to`, applied — with
    /// whatever standard conversion the operator's parameter needs first.
    fn convert_by_operator(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        from: &Type,
        to: &Type,
        span: &Range<usize>,
    ) -> Option<DataId> {
        if from == to {
            return None;
        }
        let operator = self.type_system().implicit_conversion_operator(from, to)?;
        let owner = self.extern_type_name(&operator.declaring_type)?;
        let parameter = self.extern_type_name(&operator.parameter_type)?;
        let result = self.extern_type_name(&operator.return_type)?;
        let signature = format!("{owner}.__op_Implicit__{parameter}__{result}");
        if !self.nodes.has_signature(&signature) {
            return None;
        }
        let source = if operator.parameter_type == *from {
            source
        } else {
            self.convert(ctx, source, from, &operator.parameter_type, span.clone())
        };
        let out = self.temp(&result);
        self.call_extern(ctx, &signature, &[source, out], span.clone());
        Some(out)
    }

    /// The type a bare `typeof(X)` operand names (parentheses allowed),
    /// as this instantiation sees it — `None` for any other expression.
    fn typeof_operand(
        &self,
        ctx: &Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Option<Type> {
        let Expression::Primary(primary) = expression else {
            return None;
        };
        if !primary.chain.is_empty() {
            return None;
        }
        match &primary.left {
            PrimaryLeft::Parenthesized { expression, .. } => self.typeof_operand(ctx, expression),
            PrimaryLeft::Typeof {
                target_type: Ok(target_type),
                ..
            } => {
                let ty = self
                    .bodies
                    .resolved_types
                    .get(&EntityID::from(target_type))?;
                Some(self.substitute(ty, &ctx.key.bindings))
            }
            _ => None,
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
            let left = self.lower_expression(ctx, &binary.left)?;
            // a constant left side decides: `Reflect.IsArray<T>() && ...`
            // is the right side or nothing, so what the right side names
            // only has to exist for the `T` that gets there
            if let Some(known) = self.constant_boolean(left) {
                let decided = match binary.operator.value {
                    LogicalAnd => !known,
                    _ => known,
                };
                if decided {
                    return Some(left);
                }
                return self.lower_expression(ctx, binary.right.as_ref().ok()?);
            }
            let result = self.temp("SystemBoolean");
            let short_label = self.fresh_label("logic_short");
            let end_label = self.fresh_label("logic_end");
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

        if binary.operator.value == Coalesce {
            return self.lower_coalesce(ctx, binary, whole);
        }

        // `typeof(T) == typeof(int)`: both sides are types the compiler
        // holds, so the comparison is a constant — and one that needs no
        // `System.Type` on the VM, which a type of the compilation's own has
        // none of
        if matches!(binary.operator.value, Equal | NotEqual)
            && let Some(left_type) = self.typeof_operand(ctx, &binary.left)
            && let Ok(right_expression) = &binary.right
            && let Some(right_type) = self.typeof_operand(ctx, right_expression)
        {
            let same = left_type == right_type;
            let answer = if binary.operator.value == Equal {
                same
            } else {
                !same
            };
            return Some(self.bool_constant(answer));
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

        // `x + 1`, `x == null`, `a < b` on a `T?`: lifted over null — but
        // `"n=" + x` is string concatenation, which prints a null as ""
        let concatenation = operator == Add
            && self.extern_type_name(result_type).as_deref() == Some("SystemString");
        if !concatenation
            && (self.nullable_inner(left.1).is_some() || self.nullable_inner(right.1).is_some())
        {
            return self.lift_binary(ctx, operator, left, right, result_type, span, node);
        }
        // `a == b` on tuples: element by element, as C# defines it
        if matches!(operator, Equal | NotEqual)
            && (Self::tuple_elements(left.1).is_some() || Self::tuple_elements(right.1).is_some())
        {
            return self.tuple_equality(ctx, operator == NotEqual, left, right, span);
        }
        // `a + b`, `a - b`, `a == b` on delegates: the corlib helpers
        if self.is_delegate_type(left.1) || self.is_delegate_type(right.1) {
            return self.delegate_operator(ctx, operator, left, right, span);
        }
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

        // binary numeric promotion (§12.4.7): `long == int`, `float * int`
        // — both operands are brought to the wider type first, as an extern
        // takes two slots of exactly its own type
        let (left, right) =
            self.promote_numeric_operands(ctx, operator, left, right, result_type, span.clone());
        let left = (left.0, &left.1);
        let right = (right.0, &right.1);

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
                    Message::key("codegen.this_operator_is_not_supported_by_the"),
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

        // a record compares by value: its synthesized `Equals`
        if matches!(operator, Equal | NotEqual) && self.is_source_record(&operand_type) {
            return self.record_equality(
                ctx,
                left.0,
                right.0,
                &operand_type,
                operator == NotEqual,
                span,
            );
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
                    Message::key("codegen.operator_name_is_not_available_on_udon")
                        .arg("name", name),
                    span,
                );
                None
            }
        }
    }

    /// The rank of a numeric type in binary promotion; `None` for anything
    /// that is not a primitive number.
    fn numeric_rank(&self, ty: &Type) -> Option<u8> {
        Some(match self.extern_type_name(ty)?.as_str() {
            "SystemByte" | "SystemSByte" | "SystemInt16" | "SystemUInt16" | "SystemChar" => 0,
            "SystemInt32" => 1,
            "SystemUInt32" => 2,
            "SystemInt64" => 3,
            "SystemUInt64" => 4,
            "SystemSingle" => 5,
            "SystemDouble" => 6,
            _ => return None,
        })
    }

    /// Both operands of a numeric operator converted to the promoted type:
    /// the result type for arithmetic, the wider operand for comparisons.
    /// A shift keeps its `int` count. Operands that are not both numeric
    /// come back as they were.
    fn promote_numeric_operands(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: BinaryOperator,
        left: (DataId, &Type),
        right: (DataId, &Type),
        result_type: &Type,
        span: Range<usize>,
    ) -> ((DataId, Type), (DataId, Type)) {
        use BinaryOperator::*;
        let as_is = ((left.0, left.1.clone()), (right.0, right.1.clone()));
        let (Some(left_rank), Some(right_rank)) =
            (self.numeric_rank(left.1), self.numeric_rank(right.1))
        else {
            return as_is;
        };
        if matches!(operator, LeftShift | RightShift) {
            return as_is;
        }
        // one type on both sides: its own operator applies (Udon has
        // `char == char`, and no promotion is needed)
        if self.extern_type_name(left.1) == self.extern_type_name(right.1) {
            return as_is;
        }
        let comparison = matches!(
            operator,
            Equal | NotEqual | LessThan | GreaterThan | LessThanEqual | GreaterThanEqual
        );
        let target = if !comparison && self.numeric_rank(result_type).is_some() {
            result_type.clone()
        } else if left_rank >= right_rank {
            left.1.clone()
        } else {
            right.1.clone()
        };
        // a small type (byte, short, char) computes as an int
        let target = if self.numeric_rank(&target) == Some(0) {
            self.corlib_type("Int32")
        } else {
            target
        };
        let promoted_left = self.convert(ctx, left.0, left.1, &target, span.clone());
        let promoted_right = self.convert(ctx, right.0, right.1, &target, span);
        ((promoted_left, target.clone()), (promoted_right, target))
    }

    /// A value as a `string`, via the type's own `ToString` extern.
    pub(super) fn stringify(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let ty = &self.substitute(ty, &ctx.key.bindings);
        // `(1, a)`: a tuple has no type at run time, so it is printed here,
        // where its shape is known
        if Self::tuple_elements(ty).is_some() {
            return self.tuple_to_string(ctx, slot, ty, span);
        }
        // a source enum: its member's name
        if let Some(symbol) = self.source_enum(ty) {
            return self.enum_to_string(ctx, slot, symbol, span);
        }
        // a rectangular array prints as its type name, as any array does
        if Self::rectangular_rank(ty).is_some() {
            return self.string_constant(&self.display_type(ty));
        }
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

    /// [`Generator::stringify`] with an interpolation hole's format
    /// specifier applied: `T.ToString(format)`, which Udon exposes for the
    /// numeric types and most engine structs. A type without one is an
    /// error rather than a string that quietly ignores the format.
    pub(super) fn stringify_as(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        ty: &Type,
        format: Option<&str>,
        span: Range<usize>,
    ) -> DataId {
        let Some(format) = format else {
            return self.stringify(ctx, slot, ty, span);
        };
        let ty = &self.substitute(ty, &ctx.key.bindings);
        if let Some(name) = self.extern_type_name(ty) {
            let signature = format!("{name}.__ToString__SystemString__SystemString");
            if self.nodes.has_signature(&signature) {
                let text = self.string_constant(format);
                let out = self.temp("SystemString");
                self.call_extern(ctx, &signature, &[slot, text, out], span);
                return out;
            }
        }
        self.error(
            ctx,
            Message::key("codegen.format_udon_has_no_tostring_string_for")
                .arg("format", format)
                .arg("a0", self.describe_type(ty)),
            span.clone(),
        );
        self.stringify(ctx, slot, ty, span)
    }

    /// `{x,8}` / `{x,-8}`: the text padded to that field width, right- and
    /// left-aligned as C# reads the sign.
    fn align_to_width(
        &mut self,
        ctx: &mut Ctx<'ast>,
        text: DataId,
        hole: &'ast men_sharp_parser::ast::InterpolationHole<'ast, 'ast>,
    ) -> DataId {
        let Some(alignment) = &hole.alignment else {
            return text;
        };
        let span = alignment.span();
        let Some(width) = Self::constant_integer(alignment) else {
            self.error(
                ctx,
                Message::key("codegen.the_alignment_of_an_interpolated_hole_must"),
                span,
            );
            return text;
        };
        if width == 0 {
            return text;
        }
        // a null string pads to spaces in C#; `PadLeft` on null would throw
        let value = self.temp("SystemString");
        self.copy(text, value);
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[value, null, is_null],
            span.clone(),
        );
        let filled = self.fresh_label("pad_value");
        self.program.code.push(Op::Push(is_null));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(filled)));
        let empty = self.string_constant("");
        self.copy(empty, value);
        self.program.code.push(Op::Label(filled));

        let signature = if width > 0 {
            "SystemString.__PadLeft__SystemInt32__SystemString"
        } else {
            "SystemString.__PadRight__SystemInt32__SystemString"
        };
        let count = self.int_constant(width.abs());
        let out = self.temp("SystemString");
        self.call_extern(ctx, signature, &[value, count, out], span);
        out
    }

    /// An integer literal, with an optional sign: what C# accepts as an
    /// interpolation alignment (a constant expression).
    fn constant_integer(expression: &Expression<'ast, 'ast>) -> Option<i32> {
        match expression {
            Expression::Unary(unary) => {
                let inner = unary.operand.as_ref().ok()?;
                let value = Self::constant_integer(inner)?;
                match unary.operator.value {
                    UnaryOperator::Minus => Some(-value),
                    UnaryOperator::Plus => Some(value),
                    _ => None,
                }
            }
            Expression::Primary(primary) if primary.chain.is_empty() => {
                let PrimaryLeft::Literal(LiteralExpression::Integer(text)) = &primary.left else {
                    return None;
                };
                text.value.replace('_', "").parse().ok()
            }
            _ => None,
        }
    }

    /// `!x`, `-x`, `+x` on an evaluated operand: the user's operator when
    /// the checker bound one, else the type's own extern.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_unary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: UnaryOperator,
        operand: DataId,
        operand_type: &Type,
        result_type: &Type,
        span: Range<usize>,
        node: Option<EntityID>,
    ) -> Option<DataId> {
        if let Some(result) =
            self.user_operator_call(ctx, node, &[(operand, operand_type)], span.clone())
        {
            return result;
        }
        match operator {
            UnaryOperator::Not => {
                let out = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
                    &[operand, out],
                    span,
                );
                Some(out)
            }
            UnaryOperator::Minus => {
                let name = self.extern_type_name(result_type)?;
                let out = self.temp_for(result_type);
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
                        Message::key("codegen.unary_is_not_available_on_udon_for"),
                        span,
                    );
                    return None;
                };
                self.call_extern(ctx, signature, &[operand, out], span);
                Some(out)
            }
            UnaryOperator::Plus => Some(operand),
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.this_operator_is_not_supported_by_the"),
                    span,
                );
                None
            }
        }
    }

    fn lower_unary(
        &mut self,
        ctx: &mut Ctx<'ast>,
        unary: &'ast men_sharp_parser::ast::UnaryExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let operand_expression = unary.operand.as_ref().ok()?;
        // (`!`, `-`, `+` bind their user operators in apply_unary)
        if matches!(unary.operator.value, UnaryOperator::BitwiseNot)
            && self.bodies.targets.contains_key(&EntityID::from(unary))
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
            UnaryOperator::Not | UnaryOperator::Minus | UnaryOperator::Plus => {
                let operand = self.lower_expression(ctx, operand_expression)?;
                let operand_type = self.type_of(ctx, operand_expression);
                let result_type = self.type_of(ctx, whole);
                // `-x` on an `int?`: lifted over null
                if let Some(inner) = self.nullable_inner(&operand_type) {
                    return self.lift_unary(
                        ctx,
                        unary.operator.value,
                        operand,
                        &inner,
                        unary.span.clone(),
                        Some(EntityID::from(unary)),
                    );
                }
                self.apply_unary(
                    ctx,
                    unary.operator.value,
                    operand,
                    &operand_type,
                    &result_type,
                    unary.span.clone(),
                    Some(EntityID::from(unary)),
                )
            }
            UnaryOperator::PreIncrement | UnaryOperator::PreDecrement => {
                let operator = if unary.operator.value == UnaryOperator::PreIncrement {
                    BinaryOperator::Add
                } else {
                    BinaryOperator::Subtract
                };
                // the place is found once: `a[Next()]++` moves once
                let place = self.lower_place(ctx, operand_expression);
                let (value, ty) = self.read_place(ctx, place.clone(), unary.span.clone())?;
                let one = self.unit_step(ctx, &ty, unary.span.clone());
                let updated = self.emit_binary_operator(
                    ctx,
                    operator,
                    (value, &ty),
                    (one, &ty),
                    &ty,
                    unary.span.clone(),
                    Some(EntityID::from(unary)),
                )?;
                self.write_place(ctx, place, updated, unary.span.clone());
                Some(updated)
            }
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.this_operator_is_not_supported_by_the"),
                    unary.span.clone(),
                );
                None
            }
        }
    }

    /// The `1` that `++`/`--` adds, as the operand's own type: a `long`
    /// steps by an Int64, a `float` by a Single — an Int32 constant handed
    /// to their externs would halt the VM. A source enum is its Int32.
    fn unit_step(&mut self, ctx: &mut Ctx<'ast>, ty: &Type, span: Range<usize>) -> DataId {
        let one = self.int_constant(1);
        let int = self.corlib_type("Int32");
        if *ty == int || self.type_system().is_enum_type(ty) {
            return one;
        }
        self.convert(ctx, one, &int, ty, span)
    }

    fn lower_assignment(
        &mut self,
        ctx: &mut Ctx<'ast>,
        assignment: &'ast men_sharp_parser::ast::AssignmentExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value_expression = assignment.value.as_ref().ok()?;
        // `var (a, b) = t;` and friends are assignments whose target is a
        // shape, not a place
        if assignment.operator.value == AssignmentOperator::Assign
            && Self::is_deconstruction_target(&assignment.target)
        {
            return self.lower_deconstruction(ctx, assignment);
        }
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
        if assignment.operator.value == AssignmentOperator::Coalesce {
            return self.lower_coalesce_assignment(ctx, assignment);
        }
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
                    Message::key("codegen.this_compound_assignment_is_not_supported_by"),
                    assignment.span.clone(),
                );
                return None;
            }
        };

        // §12.21.4, in this order: find the place once, so the receiver and
        // any index expression run once; read what it holds *now*, into a
        // slot of our own, since evaluating the right side may write to the
        // place itself; then the right side; then combine and store back.
        let place = self.lower_place(ctx, &assignment.target);
        let (current, target_type) =
            self.read_place(ctx, place.clone(), assignment.span.clone())?;
        let previous = self.temp_for(&target_type);
        self.copy(current, previous);

        let value = self.owned_value(ctx, value_expression)?;
        let value_type = self.type_of(ctx, value_expression);
        let final_value = self.emit_binary_operator(
            ctx,
            operator,
            (previous, &target_type),
            (value, &value_type),
            &target_type,
            assignment.span.clone(),
            Some(EntityID::from(assignment)),
        )?;

        self.write_place(ctx, place, final_value, assignment.span.clone());
        Some(final_value)
    }

    /// `a ??= b`: `b` runs only when `a` is null, and the place is found
    /// once, so index expressions on the way to it run once.
    fn lower_coalesce_assignment(
        &mut self,
        ctx: &mut Ctx<'ast>,
        assignment: &'ast men_sharp_parser::ast::AssignmentExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value_expression = assignment.value.as_ref().ok()?;
        let span = assignment.span.clone();
        let place = self.lower_place(ctx, &assignment.target);
        let target_type = place_type(&place)?;
        let (current, _) = self.read_place(ctx, place.clone(), span.clone())?;
        let result = self.temp_for(&target_type);
        self.copy(current, result);

        let assign = self.fresh_label("coalesce_assign");
        let end = self.fresh_label("coalesce_assign_end");
        let is_null = self.is_null(ctx, current, span.clone());
        self.jump_if(is_null, assign);
        self.program.code.push(Op::Jump(Target::Label(end)));
        self.program.code.push(Op::Label(assign));
        if let Some(value) = self.owned_value_as(ctx, value_expression, &target_type) {
            self.write_place(ctx, place, value, span);
            self.copy(value, result);
        }
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    // --------------------------------------------------------------- places

    pub(super) fn lower_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Place {
        let Expression::Primary(primary) = expression else {
            self.error(
                ctx,
                Message::key("codegen.this_expression_cannot_be_assigned_to"),
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
            PrimaryRight::Member { span, name, .. } => {
                // a tuple's elements are positions, not members
                if let Ok(name) = name
                    && let Some(place) = self.tuple_element_place(ctx, &receiver, name.value, span)
                {
                    return place;
                }
                match self.bodies.targets.get(&EntityID::from(last)) {
                    Some(ResolvedTarget::Member(member)) => {
                        let member = member.clone();
                        self.member_place(ctx, &member, receiver, span.clone())
                    }
                    _ => {
                        self.error(
                            ctx,
                            Message::key("codegen.this_member_cannot_be_assigned_to"),
                            span.clone(),
                        );
                        Place::Error
                    }
                }
            }
            PrimaryRight::ElementAccess {
                arguments, span, ..
            } => self.element_place(ctx, receiver, arguments.arguments, span.clone(), last),
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.this_expression_cannot_be_assigned_to"),
                    last.span(),
                );
                Place::Error
            }
        }
    }

    fn left_place(&mut self, ctx: &mut Ctx<'ast>, left: &'ast PrimaryLeft<'ast, 'ast>) -> Place {
        match left {
            PrimaryLeft::Identifier { name, span, .. } => {
                match self.bodies.targets.get(&EntityID::from(left)) {
                    Some(ResolvedTarget::Local) => match ctx.lookup(name.value) {
                        Some(local) => self.local_place(&local),
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
                        self.error(
                            ctx,
                            Message::key("codegen.this_name_cannot_be_assigned_to"),
                            span.clone(),
                        );
                        Place::Error
                    }
                }
            }
            other => {
                self.error(
                    ctx,
                    Message::key("codegen.this_expression_cannot_be_assigned_to"),
                    other.span(),
                );
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
            Message::key("codegen.name_is_an_instance_member_with_no").arg("name", name),
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
                            Message::key("codegen.a0_is_only_available_on_the_behaviour")
                                .arg("a0", self.declarations.table.symbol(symbol).name),
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
                if matches!(member.kind, SymbolKind::Event) && !self.is_field_like_event(symbol) {
                    self.error(
                        ctx,
                        Message::key("codegen.an_event_with_add_remove_accessors_is"),
                        span,
                    );
                    return Place::Error;
                }
                if self.is_entry_member(symbol)
                    && matches!(
                        member.kind,
                        SymbolKind::Field | SymbolKind::Event | SymbolKind::Property
                    )
                    && (member.kind != SymbolKind::Property || self.is_auto_property(symbol))
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
                    SymbolKind::Field | SymbolKind::Event if member.is_static => {
                        let slot = self.ensure_static(symbol, false);
                        Place::Slot(slot, member_type)
                    }
                    SymbolKind::Field | SymbolKind::Event => {
                        let Some(layout) = self.layout_of(&declaring) else {
                            self.error(
                                ctx,
                                Message::key("codegen.no_layout_for_this_receiver"),
                                span,
                            );
                            return Place::Error;
                        };
                        let Some(&index) = layout.slots.get(&symbol) else {
                            self.error(
                                ctx,
                                Message::key("codegen.field_is_missing_from_the_object_layout"),
                                span,
                            );
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
                            Message::key("codegen.this_member_kind_is_not_supported_by"),
                            span,
                        );
                        Place::Error
                    }
                }
            }
            MemberOrigin::External {
                member: external, ..
            } => {
                // `x.HasValue` / `x.Value` on a `T?`
                if let Some(place) =
                    self.try_nullable_member(ctx, &receiver, &external.name, span.clone())
                {
                    return place;
                }
                let Some(owner) = self.extern_type_name(&declaring) else {
                    self.error(
                        ctx,
                        Message::key("codegen.this_type_is_not_available_on_udon"),
                        span,
                    );
                    return Place::Error;
                };
                Place::ExternalProperty {
                    receiver: receiver.map(|(slot, _)| slot),
                    owner,
                    name: external.name.clone(),
                    ty: member_type,
                }
            }
            // a local function is a call target, never a place
            MemberOrigin::LocalFunction(_) => {
                self.error(
                    ctx,
                    Message::key("codegen.a_local_function_is_not_a_value"),
                    span,
                );
                Place::Error
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
        if Self::rectangular_rank(&ty).is_some() {
            return self.rectangular_element_place(ctx, slot, &ty, arguments, span);
        }
        // `a[1..2] = x`: a slice is a fresh array, not a place (CS0131)
        if let Some(Expression::Range(range)) = Self::single_index_expression(arguments) {
            self.error(
                ctx,
                Message::key("codegen.a_slice_cannot_be_assigned_to_a"),
                range.span.clone(),
            );
            return Place::Error;
        }
        if let Type::Array { element, rank: 1 } = &ty {
            let index = Self::single_index_expression(arguments)
                .and_then(|expression| self.index_value(ctx, slot, &ty, expression, span.clone()));
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
        // `text[i] = c`: C# has no setter on `string` either. A read never
        // reaches here — it is lowered where the element access is read.
        if self
            .extern_type_name(&ty)
            .is_some_and(|name| name == "SystemString")
        {
            self.error(
                ctx,
                Message::key("codegen.a_string_cannot_be_written_through_text"),
                span,
            );
            return Place::Error;
        }
        // an indexer
        match self.bodies.targets.get(&EntityID::from(node)) {
            Some(ResolvedTarget::Call(call)) => {
                let call = call.clone();
                let MemberOrigin::Source(symbol) = call.origin else {
                    return self.external_indexer_place(ctx, &call, slot, arguments, span);
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
                self.error(
                    ctx,
                    Message::key("codegen.this_element_access_cannot_be_compiled_yet"),
                    span,
                );
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
            Place::Slot(slot, ty)
            | Place::SelfReference { slot, ty, .. }
            | Place::ReadOnly { slot, ty, .. } => Some((slot, ty)),
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
            Place::ExternalIndexer {
                receiver,
                owner,
                name,
                indices,
                ty,
            } => {
                let return_name = self.extern_type_name(&ty)?;
                let parts = self.extern_index_types(ctx, &indices, &span)?;
                let signature = format!("{owner}.__get_{name}__{}__{return_name}", parts.join("_"));
                let out = self.temp_for(&ty);
                let mut arguments = vec![receiver];
                arguments.extend(indices.iter().map(|(slot, _)| *slot));
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
                Message::key("codegen.name_is_read_only_it_is_what").arg("name", name),
                span,
            ),
            Place::ReadOnly { what, .. } => self.error(
                ctx,
                Message::key("codegen.what_cannot_be_assigned_to").arg("what", what),
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
            Place::ExternalIndexer {
                receiver,
                owner,
                name,
                indices,
                ty,
            } => {
                let Some(value_name) = self.extern_type_name(&ty) else {
                    return;
                };
                let Some(mut parts) = self.extern_index_types(ctx, &indices, &span) else {
                    return;
                };
                parts.push(value_name);
                let signature = format!("{owner}.__set_{name}__{}__SystemVoid", parts.join("_"));
                let mut arguments = vec![receiver];
                arguments.extend(indices.iter().map(|(slot, _)| *slot));
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

    /// The single `[...]` argument, when it is written as one expression.
    fn single_index_expression(
        arguments: &'ast [Argument<'ast, 'ast>],
    ) -> Option<&'ast Expression<'ast, 'ast>> {
        let [argument] = arguments else {
            return None;
        };
        match &argument.value {
            ArgumentValue::Expression(expression) => Some(expression),
            _ => None,
        }
    }

    /// `^k` as written in an index position, if that is what this is.
    fn index_from_end(
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Option<&'ast men_sharp_parser::ast::UnaryExpression<'ast, 'ast>> {
        match expression {
            Expression::Unary(unary)
                if unary.operator.value == men_sharp_parser::ast::UnaryOperator::IndexFromEnd =>
            {
                Some(unary)
            }
            _ => None,
        }
    }

    /// How long the thing being indexed is: an array's length, a string's.
    fn indexable_length(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: DataId,
        receiver_type: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        if matches!(receiver_type, Type::Array { rank: 1, .. }) {
            return Some(self.array_length(ctx, receiver, receiver_type, span));
        }
        if self
            .extern_type_name(receiver_type)
            .is_some_and(|name| name == "SystemString")
        {
            let length = self.temp("SystemInt32");
            self.call_extern(
                ctx,
                "SystemString.__get_Length__SystemInt32",
                &[receiver, length],
                span,
            );
            return Some(length);
        }
        self.error(ctx, "internal: this receiver has no length", span);
        None
    }

    /// One index as written: `^k` counts back from the end, anything else
    /// is the index itself.
    pub(super) fn index_value(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: DataId,
        receiver_type: &Type,
        expression: &'ast Expression<'ast, 'ast>,
        span: Range<usize>,
    ) -> Option<DataId> {
        let Some(unary) = Self::index_from_end(expression) else {
            return self.lower_expression(ctx, expression);
        };
        let operand = unary.operand.as_ref().ok()?;
        let from_end = self.lower_expression(ctx, operand)?;
        let length = self.indexable_length(ctx, receiver, receiver_type, span.clone())?;
        let int32 = self.corlib_type("Int32");
        self.emit_binary_operator(
            ctx,
            BinaryOperator::Subtract,
            (length, &int32),
            (from_end, &int32),
            &int32,
            span,
            None,
        )
    }

    /// `a[i..j]` — a new array or string holding the range. The endpoints
    /// are checked here: the externs would throw, and that halts the VM,
    /// where C# throws `ArgumentOutOfRangeException`.
    fn lower_slice(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: DataId,
        receiver_type: &Type,
        range: &'ast men_sharp_parser::ast::RangeExpression<'ast, 'ast>,
        span: Range<usize>,
    ) -> Option<DataId> {
        let is_string = self
            .extern_type_name(receiver_type)
            .is_some_and(|name| name == "SystemString");
        if is_string {
            self.check_not_null(ctx, receiver, span.clone());
        }
        let length = self.indexable_length(ctx, receiver, receiver_type, span.clone())?;
        let int32 = self.corlib_type("Int32");

        // each endpoint: written, counted from the end, or left out
        let mut endpoint = |generator: &mut Self,
                            written: &'ast Option<Expression<'ast, 'ast>>,
                            default: DataId|
         -> Option<DataId> {
            let Some(expression) = written else {
                return Some(default);
            };
            generator.index_value(ctx, receiver, receiver_type, expression, span.clone())
        };
        let zero = self.int_constant(0);
        let start = endpoint(self, &range.start, zero)?;
        let end = endpoint(self, &range.end, length)?;

        // `0 <= start <= end <= length` — an empty slice at either end is
        // fine, which is why this is not the array bounds check
        let ok = self.fresh_label("slice_ok");
        let fail = self.fresh_label("slice_fail");
        let flag = self.temp("SystemBoolean");
        let zero = self.int_constant(0);
        for (name, left, right) in [
            ("op_GreaterThanOrEqual", start, zero),
            ("op_LessThanOrEqual", end, length),
            ("op_LessThanOrEqual", start, end),
        ] {
            let signature = format!("SystemInt32.__{name}__SystemInt32_SystemInt32__SystemBoolean");
            self.call_extern(ctx, &signature, &[left, right, flag], span.clone());
            self.program.code.push(Op::Push(flag));
            self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));
        }
        self.program.code.push(Op::Jump(Target::Label(ok)));
        self.program.code.push(Op::Label(fail));
        self.throw_new(
            ctx,
            &["System", "ArgumentOutOfRangeException"],
            None,
            span.clone(),
        );
        self.program.code.push(Op::Label(ok));

        let taken = self.emit_binary_operator(
            ctx,
            BinaryOperator::Subtract,
            (end, &int32),
            (start, &int32),
            &int32,
            span.clone(),
            None,
        )?;

        if is_string {
            let out = self.temp("SystemString");
            self.call_extern(
                ctx,
                "SystemString.__Substring__SystemInt32_SystemInt32__SystemString",
                &[receiver, start, taken, out],
                span,
            );
            return Some(out);
        }
        Some(self.copy_range(ctx, receiver, receiver_type, start, taken, span))
    }

    /// `taken` elements from `start`, in an array of their own.
    pub(super) fn copy_range(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        array_type: &Type,
        start: DataId,
        taken: DataId,
        span: Range<usize>,
    ) -> DataId {
        let out = self.allocate_array(ctx, array_type, taken, span.clone());
        let zero = self.int_constant(0);
        self.call_extern(
            ctx,
            "SystemArray.__Copy__SystemArray_SystemInt32_SystemArray_SystemInt32_SystemInt32__SystemVoid",
            &[array, start, out, zero, taken],
            span,
        );
        out
    }

    /// `text[index]` — Udon exposes no `String.get_Chars`, so the character
    /// comes out of a one-element `ToCharArray(index, 1)`. The bounds are
    /// checked here: the extern would throw too, but that halts the VM,
    /// while this throws the `IndexOutOfRangeException` C# promises.
    pub(super) fn string_char_at(
        &mut self,
        ctx: &mut Ctx<'ast>,
        text: DataId,
        index: DataId,
        span: Range<usize>,
    ) -> DataId {
        self.check_not_null(ctx, text, span.clone());
        let length = self.temp("SystemInt32");
        self.call_extern(
            ctx,
            "SystemString.__get_Length__SystemInt32",
            &[text, length],
            span.clone(),
        );
        self.check_index_in_range(ctx, index, length, span.clone());
        let one = self.int_constant(1);
        let chars = self.temp("SystemCharArray");
        self.call_extern(
            ctx,
            "SystemString.__ToCharArray__SystemInt32_SystemInt32__SystemCharArray",
            &[text, index, one, chars],
            span.clone(),
        );
        let zero = self.int_constant(0);
        let char_type = self.corlib_type("Char");
        let array_type = Type::Array {
            element: Box::new(char_type.clone()),
            rank: 1,
        };
        self.array_get(ctx, chars, zero, &array_type, &char_type, span)
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
            // ... and once here too, for the same reason
            let place = self.place_upto(ctx, primary, primary.chain.len() - 1);
            let Some((value, ty)) = self.read_place(ctx, place.clone(), span.clone()) else {
                return Piece::Error;
            };
            let old = self.temp_for(&ty);
            self.copy(value, old);
            let one = self.unit_step(ctx, &ty, span.clone());
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
                self.write_place(ctx, place, updated, span.clone());
            }
            return Piece::Value(old, ty);
        }

        let mut piece = self.lower_left(ctx, &primary.left);
        // `a?.b`, `a?[i]`: a null `a` makes the whole chain null (or does
        // nothing, for a call), instead of dereferencing it
        let mut null_label: Option<LabelId> = None;
        for right in primary.chain {
            let conditional = match right {
                PrimaryRight::Member { separator, .. } => {
                    separator.value == men_sharp_parser::ast::MemberSeparator::NullConditionalDot
                }
                PrimaryRight::ElementAccess {
                    null_conditional, ..
                } => *null_conditional,
                _ => false,
            };
            if conditional && let Piece::Value(slot, _) = &piece {
                let slot = *slot;
                let label = *null_label.get_or_insert_with(|| self.fresh_label("chain_null"));
                let null = self.constant("SystemObject", "null", HeapInit::Null);
                let is_null = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
                    &[slot, null, is_null],
                    right.span(),
                );
                self.jump_if(is_null, label);
            }
            piece = self.apply_right(ctx, piece, right);
        }
        let Some(null_label) = null_label else {
            return piece;
        };
        match piece {
            Piece::Value(slot, ty) => {
                // a value-typed result becomes a `T?`: null when the chain was
                let ty = if matches!(ty, Type::Named { .. })
                    && !self.is_reference_type(&ty)
                    && self.nullable_inner(&ty).is_none()
                {
                    Type::Nullable(Box::new(ty))
                } else {
                    ty
                };
                let result = self.temp_for(&ty);
                let end = self.fresh_label("chain_end");
                self.copy(slot, result);
                self.program.code.push(Op::Jump(Target::Label(end)));
                self.program.code.push(Op::Label(null_label));
                let null = self.constant("SystemObject", "null", HeapInit::Null);
                self.copy(null, result);
                self.program.code.push(Op::Label(end));
                Piece::Value(result, ty)
            }
            other => {
                self.program.code.push(Op::Label(null_label));
                other
            }
        }
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
                        self.error(
                            ctx,
                            Message::key("codegen.this_nameof_operand_is_not_supported"),
                            span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            PrimaryLeft::Identifier { name, span, .. } => {
                match self.bodies.targets.get(&EntityID::from(left)) {
                    Some(ResolvedTarget::Local) => match ctx.lookup(name.value) {
                        Some(local) => {
                            let (slot, ty) = self.read_local(ctx, &local, span.clone());
                            Piece::Value(slot, ty)
                        }
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
                    self.error(
                        ctx,
                        Message::key("codegen.this_is_unavailable_here"),
                        span.clone(),
                    );
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
            PrimaryLeft::Tuple { elements, span } => {
                let ty = self.type_of_node(ctx, EntityID::from(left));
                match self.lower_tuple(ctx, elements, &ty, span.clone()) {
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
                    self.error(
                        ctx,
                        Message::key("codegen.base_is_unavailable_here"),
                        span.clone(),
                    );
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
                            Message::key("codegen.typeof_only_works_for_types_udon_knows"),
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
                            Message::key("codegen.the_type_of_this_default_could_not"),
                            span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            other => {
                self.error(
                    ctx,
                    Message::key("codegen.this_expression_is_not_supported_by_the"),
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
                if let PrimaryRight::Member { name: Ok(name), .. } = right
                    && let Some(place) = self.tuple_element_place(ctx, &receiver, name.value, span)
                {
                    return match self.read_place(ctx, place, span.clone()) {
                        Some((slot, ty)) => Piece::Value(slot, ty),
                        None => Piece::Error,
                    };
                }
                // `grid.Length`, `grid.Rank`: `System.Array`'s members on
                // a rectangular array, which Udon's `Array` externs cannot
                // read (the object is an `object[]` of another shape)
                if let Some((slot, ty)) = &receiver
                    && Self::rectangular_rank(ty).is_some()
                    && let PrimaryRight::Member { name: Ok(name), .. } = right
                    && matches!(name.value, "Length" | "LongLength" | "Rank")
                {
                    let (slot, ty) = (*slot, ty.clone());
                    return self.rectangular_member(ctx, slot, &ty, name.value, span.clone());
                }
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
                        self.error(
                            ctx,
                            Message::key("codegen.this_call_could_not_be_resolved"),
                            span.clone(),
                        );
                        Piece::Error
                    }
                }
            }
            PrimaryRight::ElementAccess { span, .. } => {
                let receiver = piece.receiver();
                // `a[1..^1]`: a slice, made here — Udon has no `Range` value
                if let Some((slot, ty)) = &receiver {
                    let PrimaryRight::ElementAccess { arguments, .. } = right else {
                        unreachable!()
                    };
                    if let Some(Expression::Range(range)) =
                        Self::single_index_expression(arguments.arguments)
                    {
                        let (slot, ty) = (*slot, ty.clone());
                        return match self.lower_slice(ctx, slot, &ty, range, span.clone()) {
                            Some(value) => Piece::Value(value, ty),
                            None => Piece::Error,
                        };
                    }
                }
                // string indexing is special-cased by the checker
                if let Some((slot, ty)) = &receiver
                    && self
                        .extern_type_name(ty)
                        .is_some_and(|name| name == "SystemString")
                {
                    let PrimaryRight::ElementAccess { arguments, .. } = right else {
                        unreachable!()
                    };
                    let slot = *slot;
                    let ty = ty.clone();
                    let index =
                        Self::single_index_expression(arguments.arguments).and_then(|expression| {
                            self.index_value(ctx, slot, &ty, expression, span.clone())
                        });
                    let Some(index) = index else {
                        return Piece::Error;
                    };
                    let value = self.string_char_at(ctx, slot, index, span.clone());
                    return Piece::Value(value, self.corlib_type("Char"));
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
                    let one = self.unit_step(ctx, &ty, span.clone());
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

    pub(super) fn emit_call(
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
        // `grid.GetLength(0)`: `System.Array`'s methods on a rectangular
        // array are lowered by hand — see `rectangular`
        if let Some((slot, ty)) = &receiver
            && Self::rectangular_rank(ty).is_some()
            && let MemberOrigin::External { member, .. } = &call.origin
        {
            let (slot, ty) = (*slot, ty.clone());
            let name = member.name.clone();
            return self.rectangular_call(ctx, slot, &ty, &name, arguments, span);
        }
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
                        let reported = self.errors.len();
                        let lowered = match target {
                            Some(target) => self.owned_value_as(ctx, expression, &target),
                            None => self.owned_value(ctx, expression),
                        };
                        match lowered {
                            Some(value) => ordered[slot] = Some(value),
                            None => {
                                self.ensure_error_reported(
                                    ctx,
                                    reported,
                                    expression.span(),
                                    "this argument",
                                );
                                return Piece::Error;
                            }
                        }
                    }
                    _ => {
                        self.error(
                            ctx,
                            Message::key("codegen.this_argument_form_is_not_supported_by"),
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
            (Some(DefaultArgument::Source), MemberOrigin::LocalFunction(id)) => {
                let key = self.local_function_key(ctx, *id);
                let node = self.local_functions.get(&key).map(|info| info.node)?;
                let Some(expression) = node
                    .parameters
                    .as_ref()
                    .ok()
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
                        Message::key("codegen.this_optional_parameter_s_default_has_no"),
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
    pub(super) fn dispatch_call(
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
            let Some((slot, receiver_type)) = &receiver else {
                self.error(ctx, "internal: extension call without a receiver", span);
                return Piece::Error;
            };
            // the receiver is the first argument, converted like one: an
            // array passed to `this IEnumerable<T>` becomes a sequence here
            let (slot, receiver_type) = (*slot, receiver_type.clone());
            let value = match call.signature.parameters.first() {
                Some(parameter) => {
                    let parameter_type =
                        self.substitute(&parameter.parameter_type, &ctx.key.bindings);
                    self.convert(ctx, slot, &receiver_type, &parameter_type, span.clone())
                }
                None => slot,
            };
            values.insert(0, value);
        }

        let return_type = self.substitute(&call.signature.return_type, &ctx.key.bindings);
        match &call.origin {
            MemberOrigin::Source(symbol) => {
                let symbol = *symbol;
                // `op(1)` / `op.Invoke(1)` on a delegate of the compilation's
                // own: the delegate's symbol stands for its `Invoke`
                if self.declarations.table.symbol(symbol).kind == SymbolKind::Delegate {
                    return self.invoke_delegate(ctx, call, receiver, values, source_by_ref, span);
                }
                // the corlib's program-search intrinsics are lowered in place
                if let Some(piece) =
                    self.try_program_intrinsic(ctx, call, symbol, &values, span.clone())
                {
                    return piece;
                }
                // ... and so is `Comparer<T>.Default`
                if let Some(piece) =
                    self.try_comparers_intrinsic(ctx, call, symbol, &values, span.clone())
                {
                    return piece;
                }
                // ... and static reflection, answered for the type at hand
                if let Some(piece) = self.try_reflect_intrinsic(
                    ctx,
                    call,
                    symbol,
                    &values,
                    &source_by_ref,
                    span.clone(),
                ) {
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
                let Some((this, key)) = self.source_call_target(
                    ctx,
                    call,
                    symbol,
                    &receiver,
                    non_virtual,
                    span.clone(),
                ) else {
                    return Piece::Error;
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
            // a local function of this body: no receiver, no dispatch — the
            // boxes of what it captures go in front of its arguments
            MemberOrigin::LocalFunction(id) => {
                let key = self.local_function_key(ctx, *id);
                let Some(payload) = self.local_function_payload(ctx, &key, span.clone()) else {
                    return Piece::Error;
                };
                let this = self
                    .function_has_this(&key)
                    .then_some(())
                    .and(payload.first().copied());
                let leading = payload.len();
                let mut arguments: Vec<DataId> = payload[usize::from(this.is_some())..].to_vec();
                arguments.extend(values);
                let by_ref: Vec<(usize, Place)> = source_by_ref
                    .into_iter()
                    .map(|(index, place)| (index + leading - usize::from(this.is_some()), place))
                    .collect();
                match self.call_function(ctx, &key, this, &arguments, &by_ref, span) {
                    Some(result) => Piece::Value(result, return_type),
                    None if return_type == Type::Void => Piece::Void,
                    None => Piece::Error,
                }
            }
            MemberOrigin::External { member, owner } => {
                let member_name = member.name.clone();
                // `f(1)` / `f.Invoke(1)` on a `Func`/`Action`/...: no extern,
                // the delegate is the compiler's own
                if member_name == "Invoke"
                    && self.external.type_info(*owner).kind
                        == men_sharp_semantics::ExternalTypeKind::Delegate
                {
                    if !write_backs.is_empty() {
                        self.error(
                            ctx,
                            Message::key("codegen.ref_out_parameters_of_an_external_delegate"),
                            span,
                        );
                        return Piece::Error;
                    }
                    return self.invoke_delegate(ctx, call, receiver, values, Vec::new(), span);
                }
                // `x.GetValueOrDefault()`, `x.ToString()`, ... on a `T?`
                if let Some(piece) = self.try_nullable_call(
                    ctx,
                    *owner,
                    &member_name,
                    &receiver,
                    &values,
                    &return_type,
                    span.clone(),
                ) {
                    return piece;
                }
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
                        // `rank.ToString()` on a source enum: its name
                        if member_name == "ToString"
                            && values.is_empty()
                            && let Some(symbol) = self.source_enum(&receiver_type)
                        {
                            let text = self.enum_to_string(ctx, slot, symbol, span);
                            return Piece::Value(text, self.corlib_type("String"));
                        }
                        // a tuple has no type at run time: its `Equals`,
                        // `GetHashCode` and `ToString` are lowered here,
                        // where the shape is known — which is what lets one
                        // be a dictionary key
                        if Self::tuple_elements(&receiver_type).is_some()
                            && let Some(piece) = self.tuple_object_member(
                                ctx,
                                &member_name,
                                (slot, &receiver_type),
                                &values,
                                span.clone(),
                            )
                        {
                            return piece;
                        }
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
                                Message::key("codegen.this_type_argument_has_no_system_type"),
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

    /// The function instance a call into source binds to, and the `this`
    /// it takes: the most derived override on a behaviour, the direct
    /// implementation on a sealed receiver, the dispatch stub otherwise.
    /// `None` after reporting an instance member with no instance.
    pub(super) fn source_call_target(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        receiver: &Option<(DataId, Type)>,
        non_virtual: bool,
        span: Range<usize>,
    ) -> Option<(Option<DataId>, FunctionKey)> {
        if !call.is_extension
            && self.has_no_instance_to_read_from(ctx, symbol, call.is_static, receiver)
        {
            self.no_instance_error(ctx, symbol, span);
            return None;
        }
        let mut this = if call.is_static || call.is_extension {
            None
        } else {
            receiver.as_ref().map(|(slot, _)| *slot).or(ctx.this_slot)
        };
        if !call.is_static
            && !call.is_extension
            && let Some((slot, receiver_type)) = receiver
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
                        this = Some(self.clone_struct(ctx, slot, struct_type, span.clone()));
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
        Some((this, key))
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
                            Message::key("codegen.this_is_read_only_so_it_cannot"),
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
                self.bind_local(ctx, name.value, slot, parameter_type.clone());
                // a captured `out` variable lives in its box: the extern
                // writes the slot, which is then written home
                let place = ctx.lookup(name.value).map(|local| self.local_place(&local));
                match place {
                    Some(Place::Slot(..)) | None => Some((slot, None)),
                    Some(place) => Some((slot, Some(place))),
                }
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
                            Message::key("codegen.this_is_read_only_so_it_cannot"),
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
                self.bind_local(ctx, name.value, slot, parameter_type.clone());
                let place = ctx
                    .lookup(name.value)
                    .map(|local| self.local_place(&local))
                    .unwrap_or(Place::Error);
                Some((slot, place))
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
            // `new int[2, 3]` — a rectangular array, which Udon has no type
            // for. `new int[2][]` is a jagged one: an array of arrays, and
            // those it does have
            let element = new_expression
                .created_type
                .as_ref()
                .and_then(|type_ref| {
                    self.bodies
                        .resolved_types
                        .get(&EntityID::from(type_ref))
                        .cloned()
                })
                .map(|ty| {
                    let ty = men_sharp_semantics::apply_suffixes(ty, new_expression.array_suffixes);
                    self.substitute(&ty, &ctx.key.bindings)
                });
            let Some(element) = element else {
                self.error(
                    ctx,
                    Message::key("codegen.could_not_resolve_the_array_element_type"),
                    span,
                );
                return Piece::Error;
            };
            // `new int[2, 3]`: a rectangular array, built out of an `object[]`
            if new_expression.array_sizes.len() > 1 {
                let array_type = Type::Array {
                    element: Box::new(element),
                    rank: new_expression.array_sizes.len() as u32,
                };
                return self.lower_new_rectangular(
                    ctx,
                    &array_type,
                    new_expression.array_sizes,
                    &new_expression.initializer,
                    span,
                );
            }
            let array_type = Type::Array {
                element: Box::new(element),
                rank: 1,
            };
            let Some(size) = self.lower_expression(ctx, &new_expression.array_sizes[0]) else {
                return Piece::Error;
            };
            let slot = self.allocate_array(ctx, &array_type, size, span.clone());
            // element initializers
            if let Some(elements) = Self::collection_elements(&new_expression.initializer) {
                self.fill_array_initializer(ctx, slot, &array_type, elements, span);
            }
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
                Message::key("codegen.target_typed_new_is_not_supported_by"),
                span,
            );
            return Piece::Error;
        };

        // `new int[] { 1, 2 }` / `new[] { 1, 2 }`: no written size — it is
        // the element count
        if let Type::Array { rank: 1, .. } = &created {
            let elements = Self::collection_elements(&new_expression.initializer);
            let slot = self.allocate_written_array(ctx, &created, elements, span.clone());
            if let Some(elements) = elements {
                self.fill_array_initializer(ctx, slot, &created, elements, span);
            }
            return Piece::Value(slot, created);
        }
        // `new int[,] { { 1, 2 }, { 3, 4 } }`: the nesting gives the lengths
        if Self::rectangular_rank(&created).is_some() {
            return match &new_expression.initializer {
                Some(initializer) => {
                    match self.lower_rectangular_shorthand(ctx, &created, initializer, span) {
                        Some(slot) => Piece::Value(slot, created),
                        None => Piece::Error,
                    }
                }
                None => {
                    self.error(
                        ctx,
                        Message::key("codegen.a_rectangular_array_needs_its_lengths_or"),
                        span,
                    );
                    Piece::Error
                }
            };
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
                Message::key("codegen.a_behaviour_cannot_be_constructed_with_new"),
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
                    self.error(
                        ctx,
                        Message::key("codegen.this_type_is_not_available_on_udon"),
                        span,
                    );
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
                                Message::key(
                                    "codegen.a_constructor_parameter_type_is_not_available",
                                ),
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
                    Message::key("codegen.constructing_this_type_is_not_supported_by"),
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

    /// The Udon names of an indexer's index types, in order: what the
    /// `__get_Item`/`__set_Item` extern is named after.
    fn extern_index_types(
        &mut self,
        ctx: &Ctx<'ast>,
        indices: &[(DataId, Type)],
        span: &Range<usize>,
    ) -> Option<Vec<String>> {
        let mut parts = Vec::with_capacity(indices.len());
        for (_, ty) in indices {
            match self.extern_type_name(ty) {
                Some(part) => parts.push(part),
                None => {
                    self.error(
                        ctx,
                        Message::key("codegen.an_index_type_of_this_indexer_cannot"),
                        span.clone(),
                    );
                    return None;
                }
            }
        }
        Some(parts)
    }

    /// `list[0]`, `dictionary[key]`, `vector[1]` on a type from metadata:
    /// the place is a pair of `__get_Item`/`__set_Item` externs, whose names
    /// spell out the index types.
    fn external_indexer_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: DataId,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
    ) -> Place {
        let MemberOrigin::External { member, .. } = &call.origin else {
            self.error(
                ctx,
                Message::key("codegen.this_element_access_cannot_be_compiled_yet"),
                span,
            );
            return Place::Error;
        };
        let name = member.name.replace('.', "");
        let declaring = self.substitute(&call.declaring_type, &ctx.key.bindings);
        let Some(owner) = self.extern_type_name(&declaring) else {
            self.error(
                ctx,
                Message::key("codegen.this_indexer_s_declaring_type_cannot_be"),
                span,
            );
            return Place::Error;
        };
        let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
        let mut indices: Vec<(DataId, Type)> = Vec::new();
        for (position, argument) in arguments.iter().enumerate() {
            let parameter = call
                .parameter_of_argument
                .get(position)
                .and_then(|index| signature.parameters.get(*index));
            let Some(parameter) = parameter else {
                self.error(ctx, "internal: an index was left unbound", span);
                return Place::Error;
            };
            let ArgumentValue::Expression(expression) = &argument.value else {
                self.error(
                    ctx,
                    Message::key("codegen.this_index_is_not_supported_here"),
                    span,
                );
                return Place::Error;
            };
            let target = parameter.parameter_type.clone();
            let Some(value) = self.owned_value_as(ctx, expression, &target) else {
                return Place::Error;
            };
            indices.push((value, target));
        }
        Place::ExternalIndexer {
            receiver,
            owner,
            name,
            indices,
            ty: self.substitute(&signature.return_type, &ctx.key.bindings),
        }
    }

    /// The `{ ... }` elements of an initializer, when it is a collection one.
    fn collection_elements(
        initializer: &'ast Option<men_sharp_parser::ast::Initializer<'ast, 'ast>>,
    ) -> Option<&'ast [men_sharp_parser::ast::CollectionElement<'ast, 'ast>]> {
        match initializer {
            Some(men_sharp_parser::ast::Initializer::Collection { elements, .. }) => Some(elements),
            _ => None,
        }
    }

    /// An array as long as the elements written for it.
    fn allocate_written_array(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array_type: &Type,
        elements: Option<&'ast [men_sharp_parser::ast::CollectionElement<'ast, 'ast>]>,
        span: Range<usize>,
    ) -> DataId {
        use men_sharp_parser::ast::CollectionElement;
        let count = elements.map_or(0, |elements| {
            elements
                .iter()
                .filter(|element| matches!(element, CollectionElement::Expression(_)))
                .count()
        });
        let size = self.int_constant(count as i32);
        self.allocate_array(ctx, array_type, size, span)
    }

    /// `int[] a = { 1, 2 };` — C# reads the braces as `new int[] { 1, 2 }`,
    /// and the declared type says what to make.
    pub(super) fn lower_array_shorthand(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array_type: &Type,
        initializer: &'ast men_sharp_parser::ast::Initializer<'ast, 'ast>,
        span: Range<usize>,
    ) -> Option<DataId> {
        if Self::rectangular_rank(array_type).is_some() {
            return self.lower_rectangular_shorthand(ctx, array_type, initializer, span);
        }
        if !matches!(array_type, Type::Array { rank: 1, .. }) {
            // the checker said why; nothing sensible to build
            return None;
        }
        let elements = match initializer {
            men_sharp_parser::ast::Initializer::Collection { elements, .. } => Some(&**elements),
            men_sharp_parser::ast::Initializer::Object { .. } => None,
        };
        let slot = self.allocate_written_array(ctx, array_type, elements, span.clone());
        if let Some(elements) = elements {
            self.fill_array_initializer(ctx, slot, array_type, elements, span);
        }
        Some(slot)
    }

    /// `{ 1, 2, 3 }` into an array that is already the right size.
    pub(super) fn fill_array_initializer(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        array_type: &Type,
        elements: &'ast [men_sharp_parser::ast::CollectionElement<'ast, 'ast>],
        span: Range<usize>,
    ) {
        use men_sharp_parser::ast::CollectionElement;
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
    pub(super) fn apply_object_initializer(
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
                                    Message::key(
                                        "codegen.external_indexers_are_not_supported_by_the",
                                    ),
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
                        Message::key("codegen.a_nested_initializer_inside_an_object_initializer"),
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
                let suffix = raw[trimmed.len()..].to_ascii_lowercase();
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
                // the suffix picks the type, exactly as the checker read it;
                // without one, C# takes the first of `int`, `long` that fits
                match (parsed, suffix.contains('u'), suffix.contains('l')) {
                    (Ok(value), false, false) if i32::try_from(value).is_ok() => {
                        Piece::Value(self.int_constant(value as i32), self.corlib_type("Int32"))
                    }
                    (Ok(value), false, _) => {
                        let slot = self.constant(
                            "SystemInt64",
                            &value.to_string(),
                            HeapInit::Int64(value),
                        );
                        Piece::Value(slot, self.corlib_type("Int64"))
                    }
                    (Ok(value), true, false) if u32::try_from(value).is_ok() => {
                        let slot = self.constant(
                            "SystemUInt32",
                            &value.to_string(),
                            HeapInit::UInt32(value as u32),
                        );
                        Piece::Value(slot, self.corlib_type("UInt32"))
                    }
                    (Ok(_), true, _) => {
                        self.error(
                            ctx,
                            Message::key("codegen.ulong_literals_are_not_supported_by_the"),
                            text.span.clone(),
                        );
                        Piece::Error
                    }
                    _ => {
                        self.error(
                            ctx,
                            Message::key("codegen.this_integer_literal_does_not_fit_in"),
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
                            let Some(slot) = self.lower_expression(ctx, expression) else {
                                continue;
                            };
                            // `{x:F2}` — the format is text, not code, and
                            // escapes in it were written in the literal
                            let format = hole.format.as_ref().map(|text| {
                                if interpolated.is_verbatim || interpolated.is_raw {
                                    text.value.to_string()
                                } else {
                                    unescape(text.value)
                                }
                            });
                            let text = self.stringify_as(
                                ctx,
                                slot,
                                &ty,
                                format.as_deref(),
                                hole.span.clone(),
                            );
                            // `{x,-8}` — padded to the field width
                            self.align_to_width(ctx, text, hole)
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
                    Message::key("codegen.this_literal_is_not_supported_by_the"),
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
pub(super) fn place_type(place: &Place) -> Option<Type> {
    match place {
        Place::Slot(_, ty)
        | Place::SelfReference { ty, .. }
        | Place::ReadOnly { ty, .. }
        | Place::Field { ty, .. }
        | Place::Accessor { ty, .. }
        | Place::ProgramVariable { ty, .. }
        | Place::ProgramAccessor { ty, .. }
        | Place::ExternalProperty { ty, .. }
        | Place::ExternalIndexer { ty, .. } => Some(ty.clone()),
        Place::Element { element, .. } => Some(element.clone()),
        Place::Error => None,
    }
}
