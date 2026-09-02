//! Exceptions: `throw`, `try`/`catch`/`finally`, and the runtime checks
//! that turn what would be an extern's (uncatchable) crash into a thrown
//! M# exception — array bounds, null receivers, integer division by zero,
//! failed casts.
//!
//! # How unwinding works
//!
//! Udon has no stack to unwind and M# functions keep their state in static
//! heap slots (saved and restored around re-entrant calls at the call
//! site), so an exception must leave a function the way a `return` does.
//! `throw` stores the exception in `__exception`, raises `__exception_pending`
//! and jumps to the innermost `try` handler of the *current* function — or,
//! with none, returns. Every call is followed by a check of the flag: still
//! pending, the caller continues the same way. The frame restore that
//! follows a call thus runs as usual, and a handler is always a label the
//! compiler knows.
//!
//! A `catch` handler clears the flag, tests the exception's type against
//! each clause (the same type tests `is` uses), binds and runs the first
//! that fits, and rethrows when none does. `try`/`finally` is the classic
//! nesting: the `finally` block is emitted once on the normal exit, once
//! on the exceptional one (followed by a rethrow), and once before every
//! `return`/`break`/`continue` that leaves the region.
//!
//! What an extern throws (a Unity API given null, say) never reaches this:
//! the VM halts the behaviour. The checks below cover what the compiler
//! itself can see coming.

use men_sharp_parser::ast::{ThrowExpression, ThrowStatement, TryStatement};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------- state

    fn exception_state(&mut self) -> ExceptionState {
        if let Some(state) = self.exception_state {
            return state;
        }
        let exception = self.program.add_data(DataSymbol {
            name: "__exception".into(),
            udon_type: "SystemObject".into(),
            init: HeapInit::Null,
            export: false,
            sync: None,
        });
        let pending = self.program.add_data(DataSymbol {
            name: "__exception_pending".into(),
            udon_type: "SystemBoolean".into(),
            init: HeapInit::Boolean(false),
            export: false,
            sync: None,
        });
        let state = ExceptionState { exception, pending };
        self.exception_state = Some(state);
        state
    }

    /// The function every uncaught exception ends in.
    pub(super) fn unhandled_key(&self) -> FunctionKey {
        FunctionKey {
            symbol: self.declarations.table.root(),
            role: Role::UnhandledException,
            bindings: Vec::new(),
        }
    }

    /// `System.Exception` from the mini-corlib.
    fn exception_type(&self) -> Option<Type> {
        let symbol = self.find_symbol(&["System", "Exception"])?;
        Some(Type::Named {
            target: TypeTarget::Source(symbol),
            arguments: Vec::new(),
        })
    }

    // ---------------------------------------------------------- unwinding

    /// Where an exception raised at the current position goes: the
    /// innermost `try` region's handler, or out of the function.
    fn unwind_target(ctx: &Ctx<'ast>) -> Option<LabelId> {
        ctx.loop_stack.iter().rev().find_map(|frame| match frame {
            BreakFrame::Try { handler, .. } => Some(*handler),
            _ => None,
        })
    }

    fn emit_unwind(&mut self, ctx: &Ctx<'ast>) {
        match Self::unwind_target(ctx) {
            Some(handler) => self.program.code.push(Op::Jump(Target::Label(handler))),
            None => self.program.code.push(Op::JumpIndirect(ctx.return_slot)),
        }
    }

    /// After a call: continue unwinding when the callee left an exception
    /// pending.
    pub(super) fn emit_pending_check(&mut self, ctx: &Ctx<'ast>) {
        let state = self.exception_state();
        let ok = self.fresh_label("no_exception");
        self.program.code.push(Op::Push(state.pending));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        self.emit_unwind(ctx);
        self.program.code.push(Op::Label(ok));
    }

    /// In an entry stub: the body (or the static initializer) returned;
    /// with an exception pending, report it and halt.
    pub(super) fn emit_unhandled_check(&mut self, name: &str) {
        let state = self.exception_state();
        let ok = self
            .program
            .add_label(format!("event_{name}__no_exception"));
        self.program.code.push(Op::Push(state.pending));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        let key = self.unhandled_key();
        self.ensure_function(&key);
        let function = &self.functions[&key];
        let (label, return_slot) = (function.label, function.return_slot);
        let halt = self.code_address_constant(format!("__halt_after_{name}"), None);
        self.copy(halt, return_slot);
        self.program.code.push(Op::Jump(Target::Label(label)));
        self.program.code.push(Op::Label(ok));
    }

    /// Raises `exception` from the current position.
    pub(super) fn emit_throw(&mut self, ctx: &Ctx<'ast>, exception: DataId, span: Range<usize>) {
        let _ = span;
        let state = self.exception_state();
        self.copy(exception, state.exception);
        let raised = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        self.copy(raised, state.pending);
        self.emit_unwind(ctx);
    }

    /// `throw new T(message)` for a mini-corlib exception type at `path` —
    /// what the compiler's own checks raise.
    pub(super) fn throw_new(
        &mut self,
        ctx: &mut Ctx<'ast>,
        path: &[&str],
        message: Option<DataId>,
        span: Range<usize>,
    ) {
        let Some(symbol) = self.find_symbol(path) else {
            self.error(
                ctx,
                format!(
                    "internal: the corlib exception `{}` is missing",
                    path.join(".")
                ),
                span,
            );
            return;
        };
        let ty = Type::Named {
            target: TypeTarget::Source(symbol),
            arguments: Vec::new(),
        };
        let Some(object) = self.allocate_object(ctx, &ty, span.clone()) else {
            return;
        };
        let string = self.corlib_type("String");
        let wanted = usize::from(message.is_some());
        let constructor = self
            .declarations
            .table
            .symbol(symbol)
            .members_named(".ctor")
            .iter()
            .copied()
            .find(|&member| {
                matches!(
                    self.signatures.members.get(&member),
                    Some(MemberSignature::Function(function))
                        if function.parameters.len() == wanted
                            && function
                                .parameters
                                .first()
                                .is_none_or(|parameter| parameter.parameter_type == string)
                )
            });
        let Some(constructor) = constructor else {
            self.error(
                ctx,
                format!("internal: `{}` has no fitting constructor", path.join(".")),
                span,
            );
            return;
        };
        let key = FunctionKey {
            symbol: constructor,
            role: Role::Constructor,
            bindings: Vec::new(),
        };
        let arguments: Vec<DataId> = message.into_iter().collect();
        self.call_function(ctx, &key, Some(object), &arguments, &[], span.clone());
        self.emit_throw(ctx, object, span);
    }

    /// A fresh object of class `ty`: the `object[]` with its type id in
    /// slot 0, constructor not yet run.
    pub(super) fn allocate_object(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let layout = self.layout_of(ty)?;
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
        self.set_element(ctx, object, zero, type_id, span);
        Some(object)
    }

    // ------------------------------------------------------------ statements

    pub(super) fn lower_throw_statement(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast ThrowStatement<'ast, 'ast>,
    ) {
        match &statement.value {
            Some(value) => {
                if let Some(exception) = self.lower_expression(ctx, value) {
                    self.emit_throw(ctx, exception, statement.span.clone());
                }
            }
            None => match ctx.caught.last().copied() {
                Some(exception) => self.emit_throw(ctx, exception, statement.span.clone()),
                None => self.error(
                    ctx,
                    "`throw;` outside a `catch` block",
                    statement.span.clone(),
                ),
            },
        }
    }

    /// `x ?? throw new ...`: never yields; the slot it "returns" is dead.
    pub(super) fn lower_throw_expression(
        &mut self,
        ctx: &mut Ctx<'ast>,
        throw: &'ast ThrowExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value = throw.value.as_ref().ok()?;
        let exception = self.lower_expression(ctx, value)?;
        self.emit_throw(ctx, exception, throw.span.clone());
        Some(self.temp("SystemObject"))
    }

    pub(super) fn lower_try(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast TryStatement<'ast, 'ast>,
    ) {
        let Some(finally) = statement
            .finally_clause
            .as_ref()
            .and_then(|clause| clause.block.as_ref().ok())
        else {
            self.lower_try_catch(ctx, statement);
            return;
        };

        // `try {} catch {} finally {}` is `try { try {} catch {} } finally {}`
        let handler = self.fresh_label("finally_handler");
        let end = self.fresh_label("finally_end");
        ctx.loop_stack.push(BreakFrame::Try {
            handler,
            finally: Some(finally),
        });
        self.lower_try_catch(ctx, statement);
        ctx.loop_stack.pop();
        // completed normally
        self.lower_block(ctx, finally);
        self.program.code.push(Op::Jump(Target::Label(end)));

        // left by an exception: run the block, then keep unwinding
        self.program.code.push(Op::Label(handler));
        let state = self.exception_state();
        let saved = self.temp("SystemObject");
        self.copy(state.exception, saved);
        let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        self.copy(cleared, state.pending);
        self.lower_block(ctx, finally);
        self.emit_throw(ctx, saved, statement.span.clone());
        self.program.code.push(Op::Label(end));
    }

    fn lower_try_catch(&mut self, ctx: &mut Ctx<'ast>, statement: &'ast TryStatement<'ast, 'ast>) {
        let Ok(block) = &statement.block else {
            return;
        };
        if statement.catches.is_empty() {
            self.lower_block(ctx, block);
            return;
        }

        let handler = self.fresh_label("catch_handler");
        let end = self.fresh_label("try_end");
        ctx.loop_stack.push(BreakFrame::Try {
            handler,
            finally: None,
        });
        self.lower_block(ctx, block);
        ctx.loop_stack.pop();
        self.program.code.push(Op::Jump(Target::Label(end)));

        self.program.code.push(Op::Label(handler));
        let state = self.exception_state();
        let saved = self.temp("SystemObject");
        self.copy(state.exception, saved);
        let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        self.copy(cleared, state.pending);

        let object = self.corlib_type("Object");
        for clause in statement.catches {
            let next = self.fresh_label("catch_next");
            let caught_type = match &clause.exception_type {
                Some(type_ref) => {
                    let Some(ty) = self
                        .bodies
                        .resolved_types
                        .get(&EntityID::from(type_ref))
                        .cloned()
                        .map(|ty| self.substitute(&ty, &ctx.key.bindings))
                    else {
                        continue;
                    };
                    let Some(fits) =
                        self.lower_runtime_type_test(ctx, saved, &ty, clause.span.clone())
                    else {
                        continue;
                    };
                    self.program.code.push(Op::Push(fits));
                    self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
                    ty
                }
                None => object.clone(),
            };
            ctx.locals.push(HashMap::new());
            if let Some(name) = &clause.name {
                let local = self.temp_for(&caught_type);
                self.copy(saved, local);
                ctx.locals
                    .last_mut()
                    .expect("a scope is open")
                    .insert(name.value, (local, caught_type));
            }
            if let Some(filter) = &clause.filter
                && let Ok(condition) = &filter.condition
                && let Some(passes) = self.lower_expression(ctx, condition)
            {
                self.program.code.push(Op::Push(passes));
                self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
            }
            ctx.caught.push(saved);
            if let Ok(body) = &clause.block {
                self.lower_block(ctx, body);
            }
            ctx.caught.pop();
            ctx.locals.pop();
            self.program.code.push(Op::Jump(Target::Label(end)));
            self.program.code.push(Op::Label(next));
        }

        // no clause took it
        self.emit_throw(ctx, saved, statement.span.clone());
        self.program.code.push(Op::Label(end));
    }

    /// The `finally` blocks of every `try` region above stack index
    /// `above`, innermost first — what a `return`, `break` or `continue`
    /// leaving those regions runs on its way out. Each block is lowered
    /// with its own region already popped, so an exception inside it
    /// unwinds further out.
    pub(super) fn emit_finally_copies(&mut self, ctx: &mut Ctx<'ast>, above: usize) {
        let mut index = ctx.loop_stack.len();
        while index > above {
            index -= 1;
            let BreakFrame::Try {
                finally: Some(block),
                ..
            } = ctx.loop_stack[index]
            else {
                continue;
            };
            let outer = ctx.loop_stack.split_off(index);
            self.lower_block(ctx, block);
            ctx.loop_stack.extend(outer);
        }
    }

    // -------------------------------------------------------------- checks

    /// A read or write of `array[index]`: null and bounds, as C# would
    /// throw them (the extern would throw too, but that halts the VM).
    pub(super) fn check_array_access(
        &mut self,
        ctx: &mut Ctx<'ast>,
        array: DataId,
        index: DataId,
        array_type: &Type,
        span: Range<usize>,
    ) {
        self.check_not_null(ctx, array, span.clone());
        let length = self.array_length(ctx, array, array_type, span.clone());
        let ok = self.fresh_label("index_ok");
        let fail = self.fresh_label("index_fail");
        let condition = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean",
            &[index, length, condition],
            span.clone(),
        );
        self.program.code.push(Op::Push(condition));
        self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));
        let zero = self.int_constant(0);
        self.call_extern(
            ctx,
            "SystemInt32.__op_GreaterThanOrEqual__SystemInt32_SystemInt32__SystemBoolean",
            &[index, zero, condition],
            span.clone(),
        );
        self.program.code.push(Op::Push(condition));
        self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));
        self.program.code.push(Op::Jump(Target::Label(ok)));
        self.program.code.push(Op::Label(fail));
        self.throw_new(ctx, &["System", "IndexOutOfRangeException"], None, span);
        self.program.code.push(Op::Label(ok));
    }

    /// A member access or call on a reference that may be null.
    pub(super) fn check_not_null(&mut self, ctx: &mut Ctx<'ast>, slot: DataId, span: Range<usize>) {
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[slot, null, is_null],
            span.clone(),
        );
        let ok = self.fresh_label("not_null");
        self.program.code.push(Op::Push(is_null));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        self.throw_new(ctx, &["System", "NullReferenceException"], None, span);
        self.program.code.push(Op::Label(ok));
    }

    /// Integer `/` and `%`: a zero divisor throws DivideByZeroException
    /// rather than crashing the extern. A non-zero constant divisor needs
    /// no check.
    pub(super) fn check_divisor(
        &mut self,
        ctx: &mut Ctx<'ast>,
        divisor: DataId,
        divisor_type: &Type,
        span: Range<usize>,
    ) {
        if self.heap_type(divisor_type) != "SystemInt32" {
            return;
        }
        if let HeapInit::Int32(value) = self.program.data[divisor.0].init
            && value != 0
        {
            return;
        }
        let zero = self.int_constant(0);
        let is_zero = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[divisor, zero, is_zero],
            span.clone(),
        );
        let ok = self.fresh_label("divisor_ok");
        self.program.code.push(Op::Push(is_zero));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        self.throw_new(ctx, &["System", "DivideByZeroException"], None, span);
        self.program.code.push(Op::Label(ok));
    }

    // ----------------------------------------------------------- unhandled

    /// Body of the unhandled-exception function: `<type or ToString>:
    /// <Message>` to the error log, then the halt.
    pub(super) fn emit_unhandled_exception_body(&mut self, ctx: &mut Ctx<'ast>) {
        let state = self.exception_state();
        // the calls below check the flag themselves
        let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        self.copy(cleared, state.pending);
        let exception = self.temp("SystemObject");
        self.copy(state.exception, exception);

        let object = self.corlib_type("Object");
        let name = self.object_to_string(ctx, exception, &object, 0..0);
        let mut text = name;
        if let Some(exception_type) = self.exception_type()
            && let Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } = &exception_type
            && let Some(&message) = self
                .declarations
                .table
                .symbol(*symbol)
                .members_named("Message")
                .first()
        {
            let getter = self.dispatcher_for(ctx, message, &exception_type, &[], Role::Getter);
            if let Some(message_text) =
                self.call_function(ctx, &getter, Some(exception), &[], &[], 0..0)
            {
                let separator = self.string_constant(": ");
                let joined = self.temp("SystemString");
                self.call_extern(
                    ctx,
                    "SystemString.__Concat__SystemString_SystemString__SystemString",
                    &[text, separator, joined],
                    0..0,
                );
                let full = self.temp("SystemString");
                self.call_extern(
                    ctx,
                    "SystemString.__Concat__SystemString_SystemString__SystemString",
                    &[joined, message_text, full],
                    0..0,
                );
                text = full;
            }
        }
        let prefix = self.string_constant("Unhandled exception: ");
        let report = self.temp("SystemString");
        self.call_extern(
            ctx,
            "SystemString.__Concat__SystemString_SystemString__SystemString",
            &[prefix, text, report],
            0..0,
        );
        self.emit_halt(ctx, report, 0..0);
    }
}
