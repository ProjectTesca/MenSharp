//! Statement and expression lowering.

use men_sharp_parser::ast::{
    ExpressionStatement, IfStatement, LocalVariableDeclaration, NewExpression, PostfixOperator,
    ReturnStatement, WhileStatement,
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
                ctx.loop_stack.push((continue_label, break_label));
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
                ctx.loop_stack.push((continue_label, break_label));
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
            Statement::Return(ReturnStatement { value, span, .. }) => {
                if let Some(value) = value {
                    let lowered = self.lower_expression(ctx, value);
                    match (lowered, ctx.result) {
                        (Some(value), Some(result)) => self.copy(value, result),
                        (Some(_), None) => {}
                        (None, _) => {
                            let _ = span;
                        }
                    }
                }
                self.program.code.push(Op::JumpIndirect(ctx.return_slot));
            }
            Statement::Break(statement) => match ctx.loop_stack.last() {
                Some(&(_, break_label)) => {
                    self.program.code.push(Op::Jump(Target::Label(break_label)))
                }
                None => self.error(ctx, "`break` outside a loop", statement.span.clone()),
            },
            Statement::Continue(statement) => match ctx.loop_stack.last() {
                Some(&(continue_label, _)) => self
                    .program
                    .code
                    .push(Op::Jump(Target::Label(continue_label))),
                None => self.error(ctx, "`continue` outside a loop", statement.span.clone()),
            },
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
                _ => None,
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
                && let Some(lowered) = self.lower_expression(ctx, value)
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
        ctx.loop_stack.push((head, break_label));
        if let Ok(body) = &statement.body {
            self.lower_statement(ctx, body);
        }
        ctx.loop_stack.pop();
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(break_label));
    }

    /// `foreach` over an array lowers to an index loop.
    fn lower_foreach(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast men_sharp_parser::ast::ForeachStatement<'ast, 'ast>,
    ) {
        let Ok(collection) = &statement.collection else {
            return;
        };
        let collection_type = self.type_of(ctx, collection);
        let Type::Array { element, rank: 1 } = &collection_type else {
            self.error(
                ctx,
                "`foreach` over anything but an array is not supported by the Udon backend yet",
                statement.span.clone(),
            );
            return;
        };
        let element_type = (**element).clone();
        let Some(array) = self.lower_expression(ctx, collection) else {
            return;
        };
        let Ok(name) = &statement.name else {
            return;
        };

        ctx.locals.push(HashMap::new());
        let length = self.array_length(ctx, array, &collection_type, statement.span.clone());
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
            &collection_type,
            &element_type,
            statement.span.clone(),
        );
        ctx.locals
            .last_mut()
            .expect("scope")
            .insert(name.value, (variable, element_type.clone()));

        ctx.loop_stack.push((continue_label, break_label));
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

    fn fresh_label(&mut self, prefix: &str) -> LabelId {
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
                Some(self.convert(ctx, source, &from, &to, expression.span()))
            }
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
    fn convert(
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
        )
    }

    pub(super) fn emit_binary_operator(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: BinaryOperator,
        left: (DataId, &Type),
        right: (DataId, &Type),
        result_type: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        use BinaryOperator::*;

        let system_is = |ty: &Type, name: &str| {
            self.extern_type_name(ty)
                .is_some_and(|mangled| mangled == name)
        };

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
        let out = self.temp_for(result_type);
        let result_name = self
            .extern_type_name(result_type)
            .unwrap_or_else(|| "SystemBoolean".into());

        let operand_name = self.extern_type_name(&operand_type);
        let candidates: Vec<String> = match operand_name {
            Some(operand_name) => vec![format!(
                "{operand_name}.__{name}__{operand_name}_{operand_name}__{result_name}"
            )],
            // reference comparison (`x == null`, object identity)
            None if matches!(operator, Equal | NotEqual) => vec![format!(
                "SystemObject.__{name}__SystemObject_SystemObject__SystemBoolean"
            )],
            None => Vec::new(),
        };
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
                let signature = format!("{name}.__op_UnaryMinus__{name}__{name}");
                self.call_extern(ctx, &signature, &[operand, out], unary.span.clone());
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
        let value = self.lower_expression(ctx, value_expression)?;
        let value_type = self.type_of(ctx, value_expression);

        let final_value = if assignment.operator.value == AssignmentOperator::Assign {
            value
        } else {
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
            piece = self.apply_right(ctx, piece, right);
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

    fn member_place(
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
                    let slot = self.ensure_static(symbol, true);
                    return Place::Slot(slot, member_type);
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

    fn read_place(
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
                let key = FunctionKey {
                    symbol,
                    role: Role::Getter,
                    bindings,
                };
                let result = self.call_function(ctx, &key, receiver, &indices, span)?;
                Some((result, ty))
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

    fn write_place(
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
            } => self.array_set(ctx, array, index, value, &array_type, span),
            Place::Accessor {
                receiver,
                symbol,
                bindings,
                mut indices,
                ..
            } => {
                let key = FunctionKey {
                    symbol,
                    role: Role::Setter,
                    bindings,
                };
                indices.push(value);
                self.call_function(ctx, &key, receiver, &indices, span);
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
                let signature = format!("{owner}.__set_{name}__{value_name}__SystemVoid");
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
        if let Some(PrimaryRight::Postfix { operator, span }) = primary.chain.last()
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
            if let Some(updated) =
                self.emit_binary_operator(ctx, op, (value, &ty), (one, &ty), &ty, span.clone())
            {
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
                    .map(|ty| self.substitute(&ty, &ctx.key.bindings));
                match ty {
                    Some(ty) => {
                        let slot = self.default_value(&ty);
                        Piece::Value(slot, ty)
                    }
                    None => {
                        self.error(
                            ctx,
                            "target-typed `default` is not supported by the Udon backend yet",
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
        match self.extern_type_name(ty).as_deref() {
            Some("SystemInt32") => self.int_constant(0),
            Some("SystemBoolean") => {
                self.constant("SystemBoolean", "false", HeapInit::Boolean(false))
            }
            Some("SystemSingle") => self.constant("SystemSingle", "0", HeapInit::Single(0.0)),
            Some("SystemDouble") => self.constant("SystemDouble", "0", HeapInit::Double(0.0)),
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
        // evaluate arguments left to right
        let mut values: Vec<DataId> = Vec::new();
        for argument in arguments {
            if argument.modifier.is_some() {
                self.error(
                    ctx,
                    "`ref`/`out` arguments are not supported by the Udon backend yet",
                    argument.span.clone(),
                );
                return Piece::Error;
            }
            match &argument.value {
                ArgumentValue::Expression(expression) => {
                    match self.lower_expression(ctx, expression) {
                        Some(value) => values.push(value),
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
            }
        }
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
                if !call.is_extension
                    && self.has_no_instance_to_read_from(ctx, symbol, call.is_static, &receiver)
                {
                    self.no_instance_error(ctx, symbol, span);
                    return Piece::Error;
                }
                let this = if call.is_static || call.is_extension {
                    None
                } else {
                    receiver.as_ref().map(|(slot, _)| *slot).or(ctx.this_slot)
                };
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
                    self.dispatcher_for(ctx, call, symbol, &call.declaring_type)
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
                match self.call_function(ctx, &key, this, &values, span) {
                    Some(result) => Piece::Value(result, return_type),
                    None if return_type == Type::Void => Piece::Void,
                    None => Piece::Error,
                }
            }
            MemberOrigin::External { member, .. } => {
                let member_name = member.name.clone();
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
                let result = if return_type == Type::Void {
                    None
                } else {
                    Some(self.temp_for(&return_type))
                };
                pushed.extend(result);
                self.call_extern(ctx, &signature, &pushed, span);
                match result {
                    Some(result) => Piece::Value(result, return_type),
                    None => Piece::Void,
                }
            }
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
            .map(|ty| self.substitute(&ty, &ctx.key.bindings));
        let Some(created) = created else {
            self.error(
                ctx,
                "target-typed `new(...)` is not supported by the Udon backend yet",
                span,
            );
            return Piece::Error;
        };

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

            // run the constructor
            let arguments = new_expression
                .arguments
                .as_ref()
                .map(|list| list.arguments)
                .unwrap_or(&[]);
            match target {
                Some(ResolvedTarget::Call(call)) => {
                    let mut values = Vec::new();
                    for argument in arguments {
                        if let ArgumentValue::Expression(expression) = &argument.value {
                            match self.lower_expression(ctx, expression) {
                                Some(value) => values.push(value),
                                None => return Piece::Error,
                            }
                        }
                    }
                    if let MemberOrigin::Source(ctor) = call.origin {
                        let key = FunctionKey {
                            symbol: ctor,
                            role: Role::Constructor,
                            bindings: self.bindings_for(ctx, ctor, &call.declaring_type, &[]),
                        };
                        self.call_function(ctx, &key, Some(object), &values, span.clone());
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
                        self.call_function(ctx, &key, Some(object), &[], span.clone());
                    }
                }
            }

            self.apply_object_initializer(ctx, object, &created, new_expression, span);
            return Piece::Value(object, created);
        }

        // external type: extern constructor
        match target {
            Some(ResolvedTarget::Call(call)) => {
                let mut values = Vec::new();
                for argument in new_expression
                    .arguments
                    .as_ref()
                    .map(|list| list.arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Expression(expression) = &argument.value {
                        match self.lower_expression(ctx, expression) {
                            Some(value) => values.push(value),
                            None => return Piece::Error,
                        }
                    }
                }
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
                self.call_extern(ctx, &extern_signature, &pushed, span);
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
        self.call_extern(ctx, &signature, &[size, slot], span);
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
        for (position, element) in elements.iter().enumerate() {
            if let CollectionElement::Expression(expression) = element
                && let Some(value) = self.lower_expression(ctx, expression)
            {
                let index = self.int_constant(position as i32);
                self.array_set(ctx, array, index, value, array_type, span.clone());
            }
        }
    }

    fn apply_object_initializer(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        created: &Type,
        new_expression: &'ast NewExpression<'ast, 'ast>,
        span: Range<usize>,
    ) {
        use men_sharp_parser::ast::{Initializer, InitializerTarget};
        let Some(initializer) = &new_expression.initializer else {
            return;
        };
        let Initializer::Object { elements, .. } = initializer else {
            self.error(
                ctx,
                "collection initializers are not supported by the Udon backend yet",
                span,
            );
            return;
        };
        let Type::Named {
            target: TypeTarget::Source(class),
            ..
        } = created
        else {
            self.error(
                ctx,
                "object initializers on external types are not supported yet",
                span,
            );
            return;
        };
        let Some(layout) = self.layout_of(created) else {
            return;
        };
        for element in *elements {
            let InitializerTarget::Member(name) = &element.target else {
                continue;
            };
            let member = self
                .declarations
                .table
                .symbol(*class)
                .members_named(name.value)
                .first()
                .copied();
            let Some(member) = member else {
                continue;
            };
            let Some(&index) = layout.slots.get(&member) else {
                self.error(
                    ctx,
                    "object initializers may only set fields and auto-properties for now",
                    name.span.clone(),
                );
                continue;
            };
            if let Ok(InitializerValue::Expression(value)) = &element.value
                && let Some(lowered) = self.lower_expression(ctx, value)
            {
                let index = self.int_constant(index as i32);
                self.set_element(ctx, object, index, lowered, span.clone());
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
    fn corlib_type(&self, name: &str) -> Type {
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
