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

    pub(super) fn exception_state(&mut self) -> ExceptionState {
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

    // ------------------------------------------------------ source sites

    /// `File.cs:line:column` for a span.
    fn source_position(&mut self, file: FileId, offset: usize) -> String {
        match self.source_location(file, offset) {
            Some((name, line, column)) => format!("{name}:{line}:{column}"),
            None => "?".into(),
        }
    }

    /// File name, line and column (1-based) for a span, the file named as
    /// the driver saw it (from `Assets/` on when the path reaches that far).
    fn source_location(&mut self, file: FileId, offset: usize) -> Option<(String, u32, u32)> {
        let source = self.declarations.sources.get(file.0 as usize).cloned()?;
        let starts = self.line_starts.entry(file).or_insert_with(|| {
            let mut starts = vec![0];
            starts.extend(
                source
                    .text
                    .bytes()
                    .enumerate()
                    .filter(|(_, byte)| *byte == b'\n')
                    .map(|(index, _)| index + 1),
            );
            starts
        });
        let line = starts.partition_point(|&start| start <= offset);
        let line_start = starts[line.saturating_sub(1)];
        let column = source.text[line_start..offset.min(source.text.len())]
            .chars()
            .count()
            + 1;
        let name: &str = &source.name;
        let name = name.find("Assets/").map_or(name, |at| &name[at..]);
        Some((name.to_string(), line as u32, column as u32))
    }

    /// The function a frame line names: the user's path, or the stub's
    /// mangled name.
    fn frame_function_name(&self, ctx: &Ctx<'_>) -> String {
        match ctx.key.role {
            Role::Method | Role::Getter | Role::Setter | Role::Constructor => {
                let mut path = self.display_path(ctx.key.symbol);
                if let Some(class) = path.strip_suffix("..ctor") {
                    let name = class.rsplit('.').next().unwrap_or(class).to_string();
                    path = format!("{class}.{name}");
                }
                path
            }
            Role::DefaultConstructor => {
                let class = self.display_path(ctx.key.symbol);
                let name = class.rsplit('.').next().unwrap_or(&class).to_string();
                format!("{class}.{name}")
            }
            Role::UnhandledException => "the unhandled exception report".into(),
            Role::Lambda(_) => format!("a lambda in {}", self.display_path(ctx.key.symbol)),
            Role::LocalFunction(_) => {
                let name = self
                    .local_functions
                    .get(&ctx.key)
                    .map(|info| info.node.name.value)
                    .unwrap_or("a local function");
                format!("{name} in {}", self.display_path(ctx.key.symbol))
            }
            _ => self
                .functions
                .get(&ctx.key)
                .map(|function| function.name.clone())
                .unwrap_or_default(),
        }
    }

    /// An [`Op::Source`] for the code that follows, unless the previous one
    /// already says the same.
    pub(super) fn emit_source_mark(&mut self, ctx: &Ctx<'_>, span: &Range<usize>) {
        if *span == (0..0) {
            return;
        }
        let key = (ctx.file, span.start, ctx.key.clone());
        if self.last_source_mark.as_ref() == Some(&key) {
            return;
        }
        let Some((file, line, column)) = self.source_location(ctx.file, span.start) else {
            return;
        };
        let function = self.frame_function_name(ctx);
        self.push_source_mark(men_sharp_asm::SourceMark {
            file,
            line,
            column,
            function,
            kind: men_sharp_asm::SourceMarkKind::Position,
        });
        self.last_source_mark = Some(key);
    }

    /// The mark every function starts with: from here to the first
    /// statement the code is the compiler's, and a halt in it must not be
    /// attributed to the function that happens to precede it in the
    /// program. Functions without a source (dispatchers, the unhandled
    /// exception reporter) keep this as their only mark.
    pub(super) fn emit_function_start_mark(&mut self, ctx: &Ctx<'_>) {
        let function = self.frame_function_name(ctx);
        self.push_source_mark(men_sharp_asm::SourceMark {
            file: String::new(),
            line: 0,
            column: 0,
            function,
            kind: men_sharp_asm::SourceMarkKind::FunctionStart,
        });
        self.last_source_mark = None;
    }

    /// The mark before the compiler's own halt: the VM report that follows
    /// is expected, and the unhandled exception was already reported.
    pub(super) fn emit_halt_mark(&mut self, ctx: &Ctx<'_>) {
        let function = self.frame_function_name(ctx);
        self.push_source_mark(men_sharp_asm::SourceMark {
            file: String::new(),
            line: 0,
            column: 0,
            function,
            kind: men_sharp_asm::SourceMarkKind::Halt,
        });
        self.last_source_mark = None;
    }

    fn push_source_mark(&mut self, mark: men_sharp_asm::SourceMark) {
        let index = self.program.source_marks.len() as u32;
        self.program.source_marks.push(mark);
        self.program.code.push(Op::Source(index));
    }

    /// Heap slots 0 and 1: a program id and the program name. The VM's
    /// halt report dumps the heap from address 0, so these are what the
    /// Unity side reads to find the program (and its sidecar) the report
    /// is about — the same convention UdonSharp uses.
    pub(super) fn emit_program_identity(&mut self, entry_path: &[&str]) {
        let name = entry_path.join(".");
        // FNV-1a over the name: stable across builds, distinct across
        // programs — and what `GetComponent<T>()` looks for (see `components`)
        let id = super::components::program_id_of(&name);
        self.program.add_data(DataSymbol {
            name: "__program_id".into(),
            udon_type: "SystemInt64".into(),
            init: HeapInit::Int64(id),
            export: false,
            sync: None,
        });
        self.program.add_data(DataSymbol {
            name: "__program_name".into(),
            udon_type: "SystemString".into(),
            init: HeapInit::Str(name),
            export: false,
            sync: None,
        });
        self.program.program_id = Some(id);
    }

    /// `Game.Door.Open in Assets/MenSharp/Door.cs:42:13` — a frame line.
    fn site_string(&mut self, ctx: &Ctx<'ast>, span: &Range<usize>) -> String {
        let mut path = self.display_path(ctx.key.symbol);
        if let Some(class) = path.strip_suffix("..ctor") {
            // the constructor of `Game.Door` reads better as `Game.Door.Door`
            let name = class.rsplit('.').next().unwrap_or(class).to_string();
            path = format!("{class}.{name}");
        }
        let position = self.source_position(ctx.file, span.start);
        format!("{path} in {position}")
    }

    /// The slot index of one of `System.Exception`'s compiler-written
    /// fields (`__type`, `__site`, `__trace`).
    fn exception_field(&mut self, name: &str) -> Option<DataId> {
        let exception_type = self.exception_type()?;
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = &exception_type
        else {
            return None;
        };
        let field = *self
            .declarations
            .table
            .symbol(*symbol)
            .members_named(name)
            .first()?;
        let layout = self.layout_of(&exception_type)?;
        let index = *layout.slots.get(&field)?;
        Some(self.int_constant(index as i32))
    }

    /// A freshly constructed object of an exception class: its type name,
    /// for `ToString()` and the unhandled-exception report.
    pub(super) fn stamp_exception_type(&mut self, ctx: &Ctx<'ast>, object: DataId, ty: &Type) {
        let Some(exception_type) = self.exception_type() else {
            return;
        };
        if !self.is_subtype(ty, &exception_type) {
            return;
        }
        let Some(index) = self.exception_field("__type") else {
            return;
        };
        let name = self.string_constant(&self.display_type(ty));
        self.set_element(ctx, object, index, name, 0..0);
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
    /// pending — after adding this call site to its stack trace, when the
    /// caller is a function of the user's (a dispatch stub is no frame).
    pub(super) fn emit_pending_check(&mut self, ctx: &Ctx<'ast>, span: &Range<usize>) {
        let state = self.exception_state();
        let ok = self.fresh_label("no_exception");
        self.program.code.push(Op::Push(state.pending));
        self.program.code.push(Op::JumpIfFalse(Target::Label(ok)));
        let is_frame = matches!(
            ctx.key.role,
            Role::Method
                | Role::Getter
                | Role::Setter
                | Role::Constructor
                | Role::DefaultConstructor
                | Role::Lambda(_)
                | Role::LocalFunction(_)
        ) && *span != (0..0);
        if is_frame && let Some(index) = self.exception_field("__trace") {
            let string = self.corlib_type("String");
            let trace = self.get_element(ctx, state.exception, index, &string, 0..0);
            let site = self.site_string(ctx, span);
            let line = self.string_constant(&format!("\n   at {site}"));
            let extended = self.temp("SystemString");
            self.call_extern(
                ctx,
                "SystemString.__Concat__SystemString_SystemString__SystemString",
                &[trace, line, extended],
                0..0,
            );
            self.set_element(ctx, state.exception, index, extended, 0..0);
        }
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

    /// Raises `exception` from the current position. A fresh throw
    /// (`throw e`) records the site and starts the trace over, as .NET
    /// does; a rethrow (`throw;`, or unwinding on through a `finally`)
    /// keeps what the exception has.
    pub(super) fn emit_throw(
        &mut self,
        ctx: &Ctx<'ast>,
        exception: DataId,
        span: Range<usize>,
        fresh: bool,
    ) {
        if fresh
            && let (Some(site_index), Some(trace_index)) = (
                self.exception_field("__site"),
                self.exception_field("__trace"),
            )
        {
            let site = self.site_string(ctx, &span);
            let site = self.string_constant(&site);
            self.set_element(ctx, exception, site_index, site, 0..0);
            let none = self.constant("SystemString", "null", HeapInit::Null);
            self.set_element(ctx, exception, trace_index, none, 0..0);
        }
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
        self.emit_throw(ctx, object, span, true);
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
        self.stamp_exception_type(ctx, object, ty);
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
                    self.emit_throw(ctx, exception, statement.span.clone(), true);
                }
            }
            None => match ctx.caught.last().copied() {
                Some(exception) => self.emit_throw(ctx, exception, statement.span.clone(), false),
                None => self.error(
                    ctx,
                    Message::key("codegen.throw_outside_a_catch_block"),
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
        self.emit_throw(ctx, exception, throw.span.clone(), true);
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
            finally: Some(FinallyAction::Block(finally)),
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
        self.emit_throw(ctx, saved, statement.span.clone(), false);
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
                self.bind_local(ctx, name.value, local, caught_type);
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
        self.emit_throw(ctx, saved, statement.span.clone(), false);
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
                finally: Some(action),
                ..
            } = &ctx.loop_stack[index]
            else {
                continue;
            };
            let action = action.clone();
            let outer = ctx.loop_stack.split_off(index);
            self.emit_finally_action(ctx, &action);
            ctx.loop_stack.extend(outer);
        }
    }

    /// One region's way out: the block as written, or the `Dispose()` a
    /// `foreach` owes its enumerator.
    pub(super) fn emit_finally_action(
        &mut self,
        ctx: &mut Ctx<'ast>,
        action: &FinallyAction<'ast>,
    ) {
        match action {
            FinallyAction::Block(block) => self.lower_block(ctx, block),
            FinallyAction::Dispose(disposal) => {
                self.emit_call(
                    ctx,
                    &disposal.call,
                    Some((disposal.enumerator, disposal.enumerator_type.clone())),
                    &[],
                    0..0,
                    false,
                );
            }
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
        self.check_index_in_range(ctx, index, length, span);
    }

    /// `0 <= index < length`, or `IndexOutOfRangeException`.
    pub(super) fn check_index_in_range(
        &mut self,
        ctx: &mut Ctx<'ast>,
        index: DataId,
        length: DataId,
        span: Range<usize>,
    ) {
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

    /// Body of the unhandled-exception function: the exception's
    /// `ToString()` — type, message and stack trace — to the error log,
    /// then the halt.
    pub(super) fn emit_unhandled_exception_body(&mut self, ctx: &mut Ctx<'ast>) {
        let state = self.exception_state();
        // the calls below check the flag themselves
        let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        self.copy(cleared, state.pending);
        let exception = self.temp("SystemObject");
        self.copy(state.exception, exception);

        let object = self.corlib_type("Object");
        let text = self.object_to_string(ctx, exception, &object, 0..0);
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
