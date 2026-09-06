//! User-defined structs: value semantics on a reference representation.
//!
//! A struct is laid out exactly like a class — an `object[]` with the type id
//! in slot 0 — because Udon's heap holds nothing else. What makes it a value
//! is *where the compiler copies*: every point at which C# transfers a value
//! into a new storage location (assignment from a variable, argument passing,
//! returning a field, storing into a field or element, boxing) clones the
//! array; member writes and method calls on a variable work in place, as
//! they do in C#. `default(S)` and `new S[n]` allocate fresh instances rather
//! than sharing one. Udon treats its own structs (`Vector3`) the same way:
//! boxed on the heap, re-boxed by every setter extern.
//!
//! `Equals`/`GetHashCode` are synthesized field-wise (what `ValueType` does
//! by reflection) unless the struct declares its own, so structs work as
//! dictionary keys.

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    pub(super) fn is_source_struct(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if matches!(
                self.declarations.table.symbol(*symbol).kind,
                SymbolKind::Struct | SymbolKind::RecordStruct
            )
        )
    }

    /// The struct's own type parameters bound to this instantiation's
    /// arguments.
    pub(super) fn struct_bindings(&self, ty: &Type) -> Vec<(SymbolId, Type)> {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = ty
        else {
            return Vec::new();
        };
        self.type_parameter_chain(*symbol)
            .into_iter()
            .zip(arguments.iter().cloned())
            .collect()
    }

    /// Every stored field of the struct as (element index, instantiated
    /// type), in layout order.
    pub(super) fn struct_field_types(&mut self, ty: &Type) -> Vec<(usize, Type)> {
        let Some(layout) = self.layout_of(ty) else {
            return Vec::new();
        };
        let bindings = self.struct_bindings(ty);
        let mut fields: Vec<(usize, Type)> = layout
            .slots
            .iter()
            .filter_map(|(member, &index)| {
                let ty = match self.signatures.members.get(member) {
                    Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => ty,
                    _ => return None,
                };
                Some((index, self.substitute(ty, &bindings)))
            })
            .collect();
        fields.sort_by_key(|(index, _)| *index);
        fields
    }

    /// `default(T)` as a value the caller may keep: for a struct a fresh
    /// zeroed instance (a shared one would be written through), for
    /// everything else the usual constant.
    pub(super) fn default_value_in(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        if self.is_source_struct(ty) {
            self.allocate_default_struct(ctx, ty, span)
        } else if Self::tuple_elements(ty).is_some() {
            self.default_tuple(ctx, ty, span)
        } else {
            self.default_value(ty)
        }
    }

    /// A new instance holding every field's default — nested structs
    /// included, so no slot of a struct is ever null.
    pub(super) fn allocate_default_struct(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let Some(layout) = self.layout_of(ty) else {
            return self.constant("SystemObject", "null", HeapInit::Null);
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
        for (index, field_type) in self.struct_field_types(ty) {
            if self.is_reference_type(&field_type) {
                continue; // already null
            }
            let value = self.default_value_in(ctx, &field_type, span.clone());
            let index = self.int_constant(index as i32);
            self.set_element(ctx, object, index, value, span.clone());
        }
        object
    }

    /// A copy of a struct value: a new array with the same elements, nested
    /// structs copied in turn (the outer copy alone would share them).
    pub(super) fn clone_struct(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let Some(layout) = self.layout_of(ty) else {
            return source;
        };
        let copy = self.temp("SystemObjectArray");
        let size = self.int_constant(layout.size as i32);
        self.call_extern(
            ctx,
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
            &[size, copy],
            span.clone(),
        );
        self.call_extern(
            ctx,
            "SystemArray.__Copy__SystemArray_SystemArray_SystemInt32__SystemVoid",
            &[source, copy, size],
            span.clone(),
        );
        for (index, field_type) in self.struct_field_types(ty) {
            if !self.is_source_struct(&field_type) {
                continue;
            }
            let index = self.int_constant(index as i32);
            let inner = self.get_element(ctx, copy, index, &field_type, span.clone());
            let inner_copy = self.clone_struct(ctx, inner, &field_type, span.clone());
            self.set_element(ctx, copy, index, inner_copy, span.clone());
        }
        copy
    }

    /// The value of an expression for a new storage location: what
    /// `lower_expression` gives, except that a struct read from somewhere
    /// that stays alive (a variable, field, element, property) is copied
    /// first — the language's rule for value types. A struct that no one
    /// else holds (`new`, a call's result, `default`) is taken as is.
    pub(super) fn owned_value(
        &mut self,
        ctx: &mut Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value = self.lower_expression(ctx, expression)?;
        let ty = self.type_of(ctx, expression);
        if Self::is_fresh_value(expression) {
            return Some(value);
        }
        // a value type living in an `object[]` — a struct or a tuple — is
        // copied into whatever holds it next
        if self.is_source_struct(&ty) {
            return Some(self.clone_struct(ctx, value, &ty, expression.span()));
        }
        if Self::tuple_elements(&ty).is_some() {
            return Some(self.clone_tuple(ctx, value, &ty, expression.span()));
        }
        Some(value)
    }

    /// [`Generator::owned_value`], converted to `target` when the written
    /// type differs — the implicit numeric conversion of an assignment, an
    /// argument or a return (`float f = 1;` copies an `Int32` into a
    /// `Single` slot otherwise, and an extern reading it throws).
    pub(super) fn owned_value_as(
        &mut self,
        ctx: &mut Ctx<'ast>,
        expression: &'ast Expression<'ast, 'ast>,
        target: &Type,
    ) -> Option<DataId> {
        let value = self.owned_value(ctx, expression)?;
        let from = self.type_of(ctx, expression);
        Some(self.convert(ctx, value, &from, target, expression.span()))
    }

    /// Does this expression produce a value nothing else refers to? Only the
    /// shapes that certainly do count; anything else is copied, which is
    /// never wrong, only slower.
    fn is_fresh_value(expression: &Expression<'ast, 'ast>) -> bool {
        let Expression::Primary(primary) = expression else {
            return false;
        };
        match primary.chain.last() {
            Some(PrimaryRight::Invocation { .. }) => true,
            Some(_) => false,
            None => matches!(
                primary.left,
                PrimaryLeft::New(_) | PrimaryLeft::Default { .. } | PrimaryLeft::Tuple { .. }
            ),
        }
    }

    /// A struct read back from a property, indexer or method is a copy, so
    /// writing a member of it changes nothing — C# makes that an error
    /// (CS1612) rather than letting the write vanish.
    pub(super) fn check_write_through_copy(
        &mut self,
        ctx: &Ctx<'ast>,
        piece: &Piece,
        step: &'ast PrimaryRight<'ast, 'ast>,
        receiver_was_array: bool,
    ) -> bool {
        let Piece::Value(_, ty) = piece else {
            return true;
        };
        if !self.is_source_struct(ty) {
            return true;
        }
        let copied = match step {
            PrimaryRight::Invocation { .. } => true,
            PrimaryRight::ElementAccess { .. } => !receiver_was_array,
            PrimaryRight::Member { .. } => matches!(
                self.bodies.targets.get(&EntityID::from(step)),
                Some(ResolvedTarget::Member(member)) if member.kind == SymbolKind::Property
            ),
            PrimaryRight::Postfix { .. } => false,
        };
        if copied {
            self.error(
                ctx,
                Message::key("codegen.cannot_modify_a_member_of_this_struct"),
                step.span(),
            );
        }
        !copied
    }

    // ------------------------------------------- Equals / GetHashCode

    /// Body of the synthesized `bool Equals(object other)`: other is not
    /// null, is an `object[]` of the same type id, and every field is equal
    /// (`object.Equals` — value equality for boxed primitives and strings,
    /// the synthesized version again for nested structs).
    pub(super) fn emit_struct_equals(&mut self, ctx: &mut Ctx<'ast>) {
        let this = ctx.this_slot.expect("Equals has a receiver");
        let other = self.functions[&ctx.key].parameters[1];
        let result = ctx.result.expect("Equals returns bool");
        let this_type = ctx.this_type.clone().expect("Equals has a receiver type");
        let span = 0..0;

        let end = self.fresh_label("equals_end");
        let no = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        let yes = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        self.copy(no, result);

        // other == null → false
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[other, null, is_null],
            span.clone(),
        );
        let not_null = self.fresh_label("equals_not_null");
        self.program.code.push(Op::Push(is_null));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(not_null)));
        self.program.code.push(Op::Jump(Target::Label(end)));
        self.program.code.push(Op::Label(not_null));

        // other is an object[] at all (an int, say, has no slot 0 to read)
        let this_runtime_type = self.temp("SystemType");
        let other_runtime_type = self.temp("SystemType");
        self.call_extern(
            ctx,
            "SystemObject.__GetType__SystemType",
            &[this, this_runtime_type],
            span.clone(),
        );
        self.call_extern(
            ctx,
            "SystemObject.__GetType__SystemType",
            &[other, other_runtime_type],
            span.clone(),
        );
        let same_shape = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemType.__op_Equality__SystemType_SystemType__SystemBoolean",
            &[this_runtime_type, other_runtime_type, same_shape],
            span.clone(),
        );
        self.program.code.push(Op::Push(same_shape));
        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));

        // same struct type: slot 0 carries the type id
        let int32 = self.corlib_type("Int32");
        let zero = self.int_constant(0);
        let this_id = self.get_element(ctx, this, zero, &int32, span.clone());
        let other_id = self.get_element(ctx, other, zero, &int32, span.clone());
        let same_type = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean",
            &[this_id, other_id, same_type],
            span.clone(),
        );
        self.program.code.push(Op::Push(same_type));
        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));

        // every field
        for (index, field_type) in self.struct_field_types(&this_type) {
            let index = self.int_constant(index as i32);
            let mine = self.get_element(ctx, this, index, &field_type, span.clone());
            let theirs = self.get_element(ctx, other, index, &field_type, span.clone());
            let equal = if self.is_source_struct(&field_type) {
                let Type::Named {
                    target: TypeTarget::Source(symbol),
                    ..
                } = &field_type
                else {
                    continue;
                };
                let key = FunctionKey {
                    symbol: *symbol,
                    role: Role::StructEquals,
                    bindings: self.struct_bindings(&field_type),
                };
                match self.call_function(ctx, &key, Some(mine), &[theirs], &[], span.clone()) {
                    Some(equal) => equal,
                    None => continue,
                }
            } else {
                let equal = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean",
                    &[mine, theirs, equal],
                    span.clone(),
                );
                equal
            };
            self.program.code.push(Op::Push(equal));
            self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
        }

        self.copy(yes, result);
        self.program.code.push(Op::Label(end));
    }

    /// Body of the synthesized `int GetHashCode()`: the fields' hash codes
    /// combined in order (`h = h * 31 + field`), null fields counting as 0,
    /// so equal values hash alike.
    pub(super) fn emit_struct_hash_code(&mut self, ctx: &mut Ctx<'ast>) {
        let this = ctx.this_slot.expect("GetHashCode has a receiver");
        let result = ctx.result.expect("GetHashCode returns int");
        let this_type = ctx
            .this_type
            .clone()
            .expect("GetHashCode has a receiver type");
        let span = 0..0;

        let accumulator = self.temp("SystemInt32");
        let seed = self.int_constant(17);
        let factor = self.int_constant(31);
        let zero = self.int_constant(0);
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        self.copy(seed, accumulator);

        for (index, field_type) in self.struct_field_types(&this_type) {
            let index = self.int_constant(index as i32);
            let value = self.get_element(ctx, this, index, &field_type, span.clone());
            let hash = self.temp("SystemInt32");
            if self.is_source_struct(&field_type) {
                let Type::Named {
                    target: TypeTarget::Source(symbol),
                    ..
                } = &field_type
                else {
                    continue;
                };
                let key = FunctionKey {
                    symbol: *symbol,
                    role: Role::StructHashCode,
                    bindings: self.struct_bindings(&field_type),
                };
                match self.call_function(ctx, &key, Some(value), &[], &[], span.clone()) {
                    Some(inner) => self.copy(inner, hash),
                    None => continue,
                }
            } else {
                let is_null = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
                    &[value, null, is_null],
                    span.clone(),
                );
                let compute = self.fresh_label("hash_field");
                let join = self.fresh_label("hash_join");
                self.program.code.push(Op::Push(is_null));
                self.program
                    .code
                    .push(Op::JumpIfFalse(Target::Label(compute)));
                self.copy(zero, hash);
                self.program.code.push(Op::Jump(Target::Label(join)));
                self.program.code.push(Op::Label(compute));
                self.call_extern(
                    ctx,
                    "SystemObject.__GetHashCode__SystemInt32",
                    &[value, hash],
                    span.clone(),
                );
                self.program.code.push(Op::Label(join));
            }
            self.call_extern(
                ctx,
                "SystemInt32.__op_Multiplication__SystemInt32_SystemInt32__SystemInt32",
                &[accumulator, factor, accumulator],
                span.clone(),
            );
            self.call_extern(
                ctx,
                "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
                &[accumulator, hash, accumulator],
                span.clone(),
            );
        }
        self.copy(accumulator, result);
    }
}
