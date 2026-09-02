//! Runtime type identity: the type test behind casts, `is` and `as`; the
//! `object`-receiver dispatch of `Equals`/`GetHashCode`/`ToString`; and the
//! one way this backend stops a program — the halt every future `throw`
//! goes through as well.
//!
//! Every M# object is an `object[]` whose slot 0 holds its type id, so "is
//! this value a `T`" is: an `object[]`, non-empty, an `Int32` in slot 0, and
//! that id one of the instantiated subtypes of `T`. The subtype set is only
//! complete once the whole program is compiled, so the test is a synthesized
//! function emitted after the fixpoint, like a dispatcher.

use men_sharp_parser::ast::{AsExpression, IsExpression, Pattern};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------------ halt

    /// Stops the program the way an unhandled exception does on Udon: logs
    /// `message` as an error, then makes an extern throw. The VM reports the
    /// exception and halts the behaviour, so nothing after this runs. This
    /// is the one halt path; `throw`, once supported, has to use it too.
    pub(super) fn emit_halt(&mut self, ctx: &Ctx<'ast>, message: DataId, span: Range<usize>) {
        self.call_extern(
            ctx,
            "UnityEngineDebug.__LogError__SystemObject__SystemVoid",
            &[message],
            span.clone(),
        );
        // `int.Parse` on a sentence is a guaranteed FormatException, and
        // its text repeats the message in the VM's own report
        let sink = self.temp("SystemInt32");
        self.call_extern(
            ctx,
            "SystemInt32.__Parse__SystemString__SystemInt32",
            &[message, sink],
            span,
        );
    }

    // ------------------------------------------------------------ type tests

    /// Types whose values carry a type id: classes, structs and interfaces
    /// of the user's — except behaviours, which are program references.
    pub(super) fn has_type_id(&self, ty: &Type) -> bool {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = ty
        else {
            return false;
        };
        let kind = self.declarations.table.symbol(*symbol).kind;
        matches!(
            kind,
            SymbolKind::Class
                | SymbolKind::Record
                | SymbolKind::Struct
                | SymbolKind::RecordStruct
                | SymbolKind::Interface
        ) && self.behaviour_in_type(ty).is_none()
    }

    /// The synthesized `bool (object)` test for `target`, registered like a
    /// dispatcher so its body is emitted once every subtype is known.
    fn type_test_for(&mut self, ctx: &Ctx<'ast>, target: &Type) -> Option<FunctionKey> {
        let target = self.substitute(target, &ctx.key.bindings);
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = &target
        else {
            return None;
        };
        let parameters = &self.declarations.table.symbol(*symbol).type_parameters;
        let bindings: Vec<(SymbolId, Type)> = parameters
            .iter()
            .copied()
            .zip(arguments.iter().cloned())
            .collect();
        let key = FunctionKey {
            symbol: *symbol,
            role: Role::TypeTest,
            bindings,
        };
        if !self.dispatchers.contains_key(&key) {
            self.ensure_function(&key);
            self.queue.retain(|queued| queued != &key);
            self.dispatchers.insert(
                key.clone(),
                Dispatcher {
                    name: String::new(),
                    target: Role::Method,
                    receiver: target,
                    emitted_for: Vec::new(),
                },
            );
        }
        Some(key)
    }

    /// Jumps to `fail` unless `value` is an M# object; otherwise leaves the
    /// value in an `object[]` slot and its type id in an `Int32` slot.
    /// Every check comes before the copy into the typed slot, which would
    /// itself fail on a value of another shape.
    fn emit_object_guard(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        fail: LabelId,
    ) -> (DataId, DataId) {
        let span = 0..0;
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let condition = self.temp("SystemBoolean");

        // null → fail
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[value, null, condition],
            span.clone(),
        );
        self.jump_if(condition, fail);

        // not an object[] → fail
        let runtime_type = self.temp("SystemType");
        self.call_extern(
            ctx,
            "SystemObject.__GetType__SystemType",
            &[value, runtime_type],
            span.clone(),
        );
        let object_array = self.object_array_type_constant();
        self.call_extern(
            ctx,
            "SystemType.__op_Equality__SystemType_SystemType__SystemBoolean",
            &[runtime_type, object_array, condition],
            span.clone(),
        );
        self.program.code.push(Op::Push(condition));
        self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));

        // empty → fail
        let object = self.temp("SystemObjectArray");
        self.copy(value, object);
        let object_array_type = Type::Array {
            element: Box::new(self.corlib_type("Object")),
            rank: 1,
        };
        let length = self.array_length(ctx, object, &object_array_type, span.clone());
        let zero = self.int_constant(0);
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[length, zero, condition],
            span.clone(),
        );
        self.jump_if(condition, fail);

        // slot 0 not an Int32 (a user's own object[] holding anything else) → fail
        let object_type = self.corlib_type("Object");
        let first = self.get_element(ctx, object, zero, &object_type, span.clone());
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[first, null, condition],
            span.clone(),
        );
        self.jump_if(condition, fail);
        self.call_extern(
            ctx,
            "SystemObject.__GetType__SystemType",
            &[first, runtime_type],
            span.clone(),
        );
        let int32 = self.corlib_type("Int32");
        let Some(int32_type) = self.type_constant(&int32) else {
            return (object, first);
        };
        self.call_extern(
            ctx,
            "SystemType.__op_Equality__SystemType_SystemType__SystemBoolean",
            &[runtime_type, int32_type, condition],
            span,
        );
        self.program.code.push(Op::Push(condition));
        self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));

        let type_id = self.temp("SystemInt32");
        self.copy(first, type_id);
        (object, type_id)
    }

    /// `if condition goto label` — Udon only has jump-if-false.
    fn jump_if(&mut self, condition: DataId, label: LabelId) {
        let fall_through = self.fresh_label("not");
        self.program.code.push(Op::Push(condition));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(fall_through)));
        self.program.code.push(Op::Jump(Target::Label(label)));
        self.program.code.push(Op::Label(fall_through));
    }

    /// Body of a [`Role::TypeTest`] function: the guard, then the id against
    /// every instantiated subtype of the target.
    pub(super) fn emit_type_test_body(&mut self, key: &FunctionKey) {
        let function = &self.functions[key];
        let label = function.label;
        let value = function.parameters[0];
        let return_slot = function.return_slot;
        let result = function.result.expect("a type test returns bool");
        let target = self.dispatchers[key].receiver.clone();

        self.program.code.push(Op::Label(label));
        self.current_frame = Some(key.clone());
        let mut ctx = self.dispatcher_ctx(key);

        let no = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        let yes = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        self.copy(no, result);
        let fail = self.fresh_label("is_no");
        let (_, type_id) = self.emit_object_guard(&mut ctx, value, fail);

        let subtypes: Vec<i32> = self
            .type_order
            .iter()
            .filter(|ty| self.is_subtype(ty, &target) && self.behaviour_in_type(ty).is_none())
            .filter_map(|ty| self.layouts.get(ty).map(|layout| layout.type_id))
            .collect();
        let condition = self.temp("SystemBoolean");
        for id in subtypes {
            let id_constant = self.int_constant(id);
            let skip = self.fresh_label("is_skip");
            self.call_extern(
                &ctx,
                "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                &[type_id, id_constant, condition],
                0..0,
            );
            self.program.code.push(Op::Push(condition));
            self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
            self.copy(yes, result);
            self.program.code.push(Op::JumpIndirect(return_slot));
            self.program.code.push(Op::Label(skip));
        }

        self.program.code.push(Op::Label(fail));
        self.program.code.push(Op::JumpIndirect(return_slot));
        self.current_frame = None;
    }

    /// `value is T` as a bool slot, for the shapes this backend can decide:
    /// a type with a type id (through its test), or an external value type
    /// or string (by exact runtime type). `None` after reporting otherwise.
    fn lower_runtime_type_test(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        from: &Type,
        to: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let to = self.substitute(to, &ctx.key.bindings);
        if self.has_type_id(&to) {
            let test = self.type_test_for(ctx, &to)?;
            return self.call_function(ctx, &test, None, &[value], &[], span);
        }
        let exact_external = matches!(
            &to,
            Type::Named {
                target: TypeTarget::External(_),
                ..
            }
        ) && (!self.is_reference_type(&to)
            || self.heap_type(&to) == "SystemString");
        if !exact_external {
            self.error(
                ctx,
                format!(
                    "`is`/`as` with `{}` is not supported by the Udon backend yet: only the \
                     program's own classes, structs and interfaces, value types and string \
                     can be tested at runtime",
                    self.display_type(&to)
                ),
                span,
            );
            return None;
        }
        let Some(wanted) = self.type_constant(&to) else {
            self.error(ctx, "this type has no `System.Type` Udon can name", span);
            return None;
        };
        // null → false; else exact runtime type
        let result = self.temp("SystemBoolean");
        let no = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        self.copy(no, result);
        let end = self.fresh_label("is_end");
        if self.is_reference_type(from) || self.heap_type(from) == "SystemObject" {
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
        let runtime_type = self.temp("SystemType");
        self.call_extern(
            ctx,
            "SystemObject.__GetType__SystemType",
            &[value, runtime_type],
            span.clone(),
        );
        self.call_extern(
            ctx,
            "SystemType.__op_Equality__SystemType_SystemType__SystemBoolean",
            &[runtime_type, wanted, result],
            span,
        );
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// `x is T` / `x is T t`: the test, and on success the value bound to
    /// the pattern variable in the enclosing scope (where the checker
    /// declared it).
    pub(super) fn lower_is(
        &mut self,
        ctx: &mut Ctx<'ast>,
        is: &'ast IsExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let Ok(Pattern::Declaration {
            pattern_type,
            designation,
            ..
        }) = &is.pattern
        else {
            self.error(
                ctx,
                "only type patterns (`x is T`, `x is T t`) are supported by the Udon backend yet",
                is.span.clone(),
            );
            return None;
        };
        let to = self
            .bodies
            .resolved_types
            .get(&EntityID::from(pattern_type))
            .cloned()
            .map(|ty| self.substitute(&ty, &ctx.key.bindings))?;
        let from = self.type_of(ctx, &is.value);
        let value = self.lower_expression(ctx, &is.value)?;
        let result = self.lower_runtime_type_test(ctx, value, &from, &to, is.span.clone())?;
        if let Some(name) = designation {
            let local = self.temp_for(&to);
            let skip = self.fresh_label("is_bind_skip");
            self.program.code.push(Op::Push(result));
            self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
            self.copy(value, local);
            self.program.code.push(Op::Label(skip));
            ctx.locals
                .last_mut()
                .expect("a scope is open")
                .insert(name.value, (local, to));
        }
        Some(result)
    }

    /// `x as T`: the value when it is a `T`, null otherwise.
    pub(super) fn lower_as(
        &mut self,
        ctx: &mut Ctx<'ast>,
        as_expression: &'ast AsExpression<'ast, 'ast>,
        whole: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let to = self.type_of(ctx, whole);
        let from = self.type_of(ctx, &as_expression.value);
        let value = self.lower_expression(ctx, &as_expression.value)?;
        let test =
            self.lower_runtime_type_test(ctx, value, &from, &to, as_expression.span.clone())?;
        let udon_type = self.heap_type(&to);
        let null = self.constant(&udon_type, "null", HeapInit::Null);
        let result = self.temp_for(&to);
        self.copy(null, result);
        let skip = self.fresh_label("as_skip");
        self.program.code.push(Op::Push(test));
        self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
        self.copy(value, result);
        self.program.code.push(Op::Label(skip));
        Some(result)
    }

    /// `(T)x` where `T` carries a type id and `x` is not statically a `T`:
    /// the value passes when null or a `T`; anything else halts the program
    /// (C#'s InvalidCastException) instead of being read as a `T` later.
    pub(super) fn checked_cast(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        from: &Type,
        to: &Type,
        span: Range<usize>,
    ) -> DataId {
        let to = self.substitute(to, &ctx.key.bindings);
        if !self.has_type_id(&to) || matches!(from, Type::Null) || self.is_subtype(from, &to) {
            return source;
        }
        let Some(test) = self.type_test_for(ctx, &to) else {
            return source;
        };
        let done = self.fresh_label("cast_ok");
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[source, null, is_null],
            span.clone(),
        );
        self.jump_if(is_null, done);
        if let Some(ok) = self.call_function(ctx, &test, None, &[source], &[], span.clone()) {
            self.program.code.push(Op::Push(ok));
            let fail = self.fresh_label("cast_fail");
            self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));
            self.program.code.push(Op::Jump(Target::Label(done)));
            self.program.code.push(Op::Label(fail));
            let message = self.string_constant(&format!(
                "InvalidCastException: the object is not a `{}`",
                self.display_type(&to)
            ));
            self.emit_halt(ctx, message, span);
        }
        self.program.code.push(Op::Label(done));
        source
    }

    // ------------------------------------- object members through `object`

    /// The stub behind `name` (`Equals`, `GetHashCode`, `ToString`) on a
    /// receiver whose runtime type is open.
    fn object_dispatcher_for(&mut self, member: ObjectMember) -> FunctionKey {
        let key = FunctionKey {
            symbol: self.declarations.table.root(),
            role: Role::ObjectDispatcher(member),
            bindings: Vec::new(),
        };
        if !self.dispatchers.contains_key(&key) {
            self.ensure_function(&key);
            self.queue.retain(|queued| queued != &key);
            self.dispatchers.insert(
                key.clone(),
                Dispatcher {
                    name: member.name().to_string(),
                    target: Role::Method,
                    receiver: self.corlib_type("Object"),
                    emitted_for: Vec::new(),
                },
            );
        }
        key
    }

    /// What `name` runs on an object of exactly `ty`: the user's override
    /// (declared on it or inherited), or the synthesized field-wise version
    /// for a struct. `None` for a class that inherits `System.Object`'s.
    pub(super) fn object_member_implementation(
        &mut self,
        ty: &Type,
        name: &str,
    ) -> Option<FunctionKey> {
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let object = self.corlib_type("Object");
        for candidate in system.members_named(ty, name) {
            let MemberOrigin::Source(symbol) = candidate.origin else {
                continue;
            };
            if candidate.kind != SymbolKind::Method || candidate.is_static {
                continue;
            }
            let Some(MemberSignature::Function(signature)) = &candidate.signature else {
                continue;
            };
            let fits = match name {
                "Equals" => {
                    signature.parameters.len() == 1
                        && signature.parameters[0].parameter_type == object
                }
                _ => signature.parameters.is_empty(),
            };
            if !fits || self.is_bodiless(symbol) {
                continue;
            }
            let bindings = match &candidate.declaring_type {
                Type::Named {
                    target: TypeTarget::Source(class),
                    arguments,
                } => {
                    let parameters = &self.declarations.table.symbol(*class).type_parameters;
                    parameters
                        .iter()
                        .copied()
                        .zip(arguments.iter().cloned())
                        .collect()
                }
                _ => Vec::new(),
            };
            return Some(FunctionKey {
                symbol,
                role: Role::Method,
                bindings,
            });
        }
        if self.is_source_struct(ty) && name != "ToString" {
            let Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } = ty
            else {
                return None;
            };
            return Some(FunctionKey {
                symbol: *symbol,
                role: match name {
                    "Equals" => Role::StructEquals,
                    _ => Role::StructHashCode,
                },
                bindings: self.struct_bindings(ty),
            });
        }
        None
    }

    /// Schedules the implementation on every instantiated type for an
    /// object dispatcher; true when that queued new work.
    pub(super) fn ensure_object_dispatcher_impls(&mut self, key: &FunctionKey, name: &str) -> bool {
        let mut changed = false;
        let candidates: Vec<Type> = self.type_order.clone();
        for ty in candidates {
            if self.dispatchers[key].emitted_for.contains(&ty) {
                continue;
            }
            if self.emitted_dispatchers.contains(key) {
                let message = format!(
                    "internal: `{}` was instantiated after the `object.{name}` dispatch was emitted",
                    self.display_type(&ty)
                );
                let ctx = self.dispatcher_ctx(key);
                self.error(&ctx, message, 0..0);
            }
            if self.behaviour_in_type(&ty).is_none()
                && let Some(implementation) = self.object_member_implementation(&ty, name)
            {
                self.ensure_function(&implementation);
                changed = true;
            }
            self.dispatchers
                .get_mut(key)
                .expect("dispatcher exists")
                .emitted_for
                .push(ty);
        }
        changed
    }

    /// Body of a [`Role::ObjectDispatcher`]: guard, then per type id the
    /// user's implementation — a struct without `ToString` gets its type
    /// name, as .NET would print — and for everything else (a boxed int, a
    /// string, a Unity object, a class without an override) the
    /// `System.Object` extern.
    pub(super) fn emit_object_dispatcher_body(&mut self, key: &FunctionKey) {
        let function = &self.functions[key];
        let label = function.label;
        let parameters = function.parameters.clone();
        let receiver = parameters[0];
        let return_slot = function.return_slot;
        let result = function.result.expect("object members return a value");
        let name = self.dispatchers[key].name.clone();

        self.program.code.push(Op::Label(label));
        self.current_frame = Some(key.clone());
        let mut ctx = self.dispatcher_ctx(key);

        let fallback = self.fresh_label("object_fallback");
        let (object, type_id) = self.emit_object_guard(&mut ctx, receiver, fallback);

        let targets: Vec<(Type, i32)> = self
            .type_order
            .iter()
            .filter(|ty| self.behaviour_in_type(ty).is_none())
            .filter_map(|ty| {
                self.layouts
                    .get(ty)
                    .map(|layout| (ty.clone(), layout.type_id))
            })
            .collect();
        let condition = self.temp("SystemBoolean");
        for (ty, id) in targets {
            let implementation = self.object_member_implementation(&ty, &name);
            if implementation.is_none() && name != "ToString" {
                // reference identity: what the extern below does
                continue;
            }
            let id_constant = self.int_constant(id);
            let skip = self.fresh_label("object_skip");
            self.call_extern(
                &ctx,
                "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                &[type_id, id_constant, condition],
                0..0,
            );
            self.program.code.push(Op::Push(condition));
            self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
            match implementation {
                Some(implementation) => {
                    let arguments: Vec<DataId> = parameters[1..].to_vec();
                    let value = self.call_function(
                        &mut ctx,
                        &implementation,
                        Some(object),
                        &arguments,
                        &[],
                        0..0,
                    );
                    if let Some(value) = value {
                        self.copy(value, result);
                    }
                }
                None => {
                    let type_name = self.string_constant(&self.display_type(&ty));
                    self.copy(type_name, result);
                }
            }
            self.program.code.push(Op::JumpIndirect(return_slot));
            self.program.code.push(Op::Label(skip));
        }

        self.program.code.push(Op::Label(fallback));
        let mut pushed = parameters.clone();
        pushed.push(result);
        let signature = match name.as_str() {
            "Equals" => "SystemObject.__Equals__SystemObject__SystemBoolean",
            "GetHashCode" => "SystemObject.__GetHashCode__SystemInt32",
            _ => "SystemObject.__ToString__SystemString",
        };
        self.call_extern(&ctx, signature, &pushed, 0..0);
        self.program.code.push(Op::JumpIndirect(return_slot));
        self.current_frame = None;
    }

    /// Whether a value of static type `ty` may at runtime be an object of
    /// any of several types: `object`, an interface, a non-sealed class.
    fn is_open_object_type(&self, ty: &Type) -> bool {
        if self.heap_type(ty) == "SystemObject"
            && matches!(
                ty,
                Type::Named {
                    target: TypeTarget::External(_),
                    ..
                }
            )
        {
            return true;
        }
        self.has_type_id(ty) && !self.is_final_type(ty)
    }

    /// `Equals` / `GetHashCode` / `ToString` on a receiver of the user's
    /// types or of `object`: the override the runtime type selects — through
    /// the object dispatcher when the type is open, directly when it is a
    /// struct or sealed class. `None` when the `System.Object` extern is the
    /// right thing (a class without an override, an external receiver).
    pub(super) fn object_member_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        name: &str,
        receiver: (DataId, Type),
        values: &[DataId],
        return_type: &Type,
        span: Range<usize>,
    ) -> Option<Piece> {
        let (slot, ty) = receiver;
        let member = ObjectMember::of(name)?;
        if self.is_open_object_type(&ty) {
            let key = self.object_dispatcher_for(member);
            let mut arguments = vec![slot];
            arguments.extend(values.iter().copied());
            let result = self.call_function(ctx, &key, None, &arguments, &[], span)?;
            return Some(Piece::Value(result, return_type.clone()));
        }
        if !self.has_type_id(&ty) {
            return None;
        }
        match self.object_member_implementation(&ty, name) {
            Some(key) => {
                let result = self.call_function(ctx, &key, Some(slot), values, &[], span)?;
                Some(Piece::Value(result, return_type.clone()))
            }
            None if name == "ToString" => {
                let type_name = self.string_constant(&self.display_type(&ty));
                Some(Piece::Value(type_name, return_type.clone()))
            }
            None => None,
        }
    }

    /// `ToString()` of a value in string concatenation and interpolation:
    /// null gives the empty string (as `string.Concat` does), anything else
    /// its own `ToString`.
    pub(super) fn object_to_string(
        &mut self,
        ctx: &mut Ctx<'ast>,
        slot: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let out = self.temp("SystemString");
        let empty = self.string_constant("");
        self.copy(empty, out);
        let end = self.fresh_label("tostring_end");
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[slot, null, is_null],
            span.clone(),
        );
        self.jump_if(is_null, end);
        let string = self.corlib_type("String");
        match self.object_member_call(
            ctx,
            "ToString",
            (slot, ty.clone()),
            &[],
            &string,
            span.clone(),
        ) {
            Some(Piece::Value(value, _)) => self.copy(value, out),
            _ => {
                let fallback = self.temp("SystemString");
                self.call_extern(
                    ctx,
                    "SystemObject.__ToString__SystemString",
                    &[slot, fallback],
                    span,
                );
                self.copy(fallback, out);
            }
        }
        self.program.code.push(Op::Label(end));
        out
    }
}
