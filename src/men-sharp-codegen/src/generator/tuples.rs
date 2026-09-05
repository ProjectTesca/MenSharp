//! Tuples: `(int, string)`, deconstruction and positional patterns.
//!
//! Udon has no `ValueTuple`, so a tuple is an `object[]`: element `i` at
//! index `i + 1`, and index 0 left null on purpose. A struct of the user's
//! keeps its type id there, and the runtime's object dispatcher reads that
//! slot to find whose `Equals`/`ToString` to call — a null says "not one of
//! those", which is exactly what a tuple is: it has no type at run time.
//! `Equals`, `GetHashCode` and `ToString` are instead lowered element by
//! element wherever the tuple's type is known, which is everywhere but a
//! tuple that has been put in an `object`.
//!
//! Tuples are value types, so a tuple read out of a variable, field or
//! element is copied before it lands anywhere new, exactly like a struct.
//! Deconstruction (`var (a, b) = t;`) never builds the tuple's copy: it
//! reads the elements straight into the variables.

use men_sharp_parser::ast::{TupleElement as WrittenElement, VariableDesignation};
use men_sharp_semantics::{TupleElement, tuple_element_index};

use super::expressions::place_type;
use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    pub(super) fn tuple_elements(ty: &Type) -> Option<&[TupleElement]> {
        match ty {
            Type::Tuple(elements) => Some(elements),
            _ => None,
        }
    }

    /// Where element `index` sits: index 0 is the marker slot.
    pub(super) fn tuple_slot(&mut self, index: usize) -> DataId {
        self.int_constant(index as i32 + 1)
    }

    /// A fresh `object[]` holding the values.
    fn new_tuple(
        &mut self,
        ctx: &mut Ctx<'ast>,
        values: &[DataId],
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let object = self.temp("SystemObjectArray");
        let size = self.int_constant(values.len() as i32 + 1);
        self.call_extern(
            ctx,
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
            &[size, object],
            span.clone(),
        );
        // slot 0 is the shape's type id, the same way a struct carries one
        if let Some(type_id) = self.tuple_type_id(ty) {
            let zero = self.int_constant(0);
            let id = self.int_constant(type_id);
            self.set_element(ctx, object, zero, id, span.clone());
        }
        for (index, value) in values.iter().enumerate() {
            let at = self.tuple_slot(index);
            self.set_element(ctx, object, at, *value, span.clone());
        }
        object
    }

    /// `(1, "a")` — the written elements, converted to the tuple's own.
    pub(super) fn lower_tuple(
        &mut self,
        ctx: &mut Ctx<'ast>,
        written: &'ast [WrittenElement<'ast, 'ast>],
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let elements = Self::tuple_elements(ty).map(<[TupleElement]>::to_vec);
        let mut values = Vec::with_capacity(written.len());
        for (index, element) in written.iter().enumerate() {
            let value = match elements.as_ref().and_then(|elements| elements.get(index)) {
                Some(element_type) => {
                    let target = self.substitute(&element_type.element, &ctx.key.bindings);
                    self.owned_value_as(ctx, &element.value, &target)?
                }
                None => self.owned_value(ctx, &element.value)?,
            };
            values.push(value);
        }
        Some(self.new_tuple(ctx, &values, ty, span))
    }

    /// A copy of a tuple value: its own array, with nested tuples and
    /// structs copied in turn — the outer copy alone would share them.
    pub(super) fn clone_tuple(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let Some(elements) = Self::tuple_elements(ty).map(<[TupleElement]>::to_vec) else {
            return source;
        };
        let mut values = Vec::with_capacity(elements.len());
        for (index, element) in elements.iter().enumerate() {
            let element_type = self.substitute(&element.element, &ctx.key.bindings);
            let at = self.tuple_slot(index);
            let value = self.get_element(ctx, source, at, &element_type, span.clone());
            values.push(self.owned_copy(ctx, value, &element_type, span.clone()));
        }
        self.new_tuple(ctx, &values, ty, span)
    }

    /// A value about to be stored somewhere new: a value type that lives in
    /// an `object[]` is copied, anything else is itself.
    pub(super) fn owned_copy(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        if Self::tuple_elements(ty).is_some() {
            return self.clone_tuple(ctx, value, ty, span);
        }
        if self.is_source_struct(ty) {
            return self.clone_struct(ctx, value, ty, span);
        }
        value
    }

    /// `default((int, string))`: every element at its own default.
    pub(super) fn default_tuple(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let elements = Self::tuple_elements(ty)
            .map(<[TupleElement]>::to_vec)
            .unwrap_or_default();
        let mut values = Vec::with_capacity(elements.len());
        for element in &elements {
            let element_type = self.substitute(&element.element, &ctx.key.bindings);
            values.push(self.default_value_in(ctx, &element_type, span.clone()));
        }
        self.new_tuple(ctx, &values, ty, span)
    }

    /// `a == b` on tuples: every element compared with its own `==`, and
    /// the whole false as soon as one is (C# §12.12.15).
    pub(super) fn tuple_equality(
        &mut self,
        ctx: &mut Ctx<'ast>,
        negated: bool,
        left: (DataId, &Type),
        right: (DataId, &Type),
        span: Range<usize>,
    ) -> Option<DataId> {
        let (Some(left_elements), Some(right_elements)) = (
            Self::tuple_elements(left.1).map(<[TupleElement]>::to_vec),
            Self::tuple_elements(right.1).map(<[TupleElement]>::to_vec),
        ) else {
            self.error(
                ctx,
                "a tuple can only be compared with a tuple of the same shape",
                span,
            );
            return None;
        };
        if left_elements.len() != right_elements.len() {
            self.error(
                ctx,
                "a tuple can only be compared with a tuple of the same shape",
                span,
            );
            return None;
        }
        let result = self.temp("SystemBoolean");
        let same = self.bool_constant(!negated);
        let differs = self.bool_constant(negated);
        self.copy(differs, result);
        let end = self.fresh_label("tuple_equality_end");
        for (index, (left_element, right_element)) in
            left_elements.iter().zip(&right_elements).enumerate()
        {
            let left_type = self.substitute(&left_element.element, &ctx.key.bindings);
            let right_type = self.substitute(&right_element.element, &ctx.key.bindings);
            let at = self.tuple_slot(index);
            let left_value = self.get_element(ctx, left.0, at, &left_type, span.clone());
            let at = self.tuple_slot(index);
            let right_value = self.get_element(ctx, right.0, at, &right_type, span.clone());
            let boolean = self.corlib_type("Boolean");
            let equal = self.emit_binary_operator(
                ctx,
                men_sharp_parser::ast::BinaryOperator::Equal,
                (left_value, &left_type),
                (right_value, &right_type),
                &boolean,
                span.clone(),
                None,
            )?;
            self.program.code.push(Op::Push(equal));
            self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
        }
        self.copy(same, result);
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// `(1, a)` — what `ToString` gives in C#, built here because nothing
    /// at run time knows a tuple from any other `object[]`.
    pub(super) fn tuple_to_string(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let elements = Self::tuple_elements(ty)
            .map(<[TupleElement]>::to_vec)
            .unwrap_or_default();
        let mut text = self.string_constant("(");
        for (index, element) in elements.iter().enumerate() {
            if index > 0 {
                let comma = self.string_constant(", ");
                text = self.concat_strings(ctx, text, comma, span.clone());
            }
            let element_type = self.substitute(&element.element, &ctx.key.bindings);
            let at = self.tuple_slot(index);
            let read = self.get_element(ctx, value, at, &element_type, span.clone());
            let piece = self.stringify(ctx, read, &element_type, span.clone());
            text = self.concat_strings(ctx, text, piece, span.clone());
        }
        let close = self.string_constant(")");
        self.concat_strings(ctx, text, close, span)
    }

    fn concat_strings(
        &mut self,
        ctx: &mut Ctx<'ast>,
        left: DataId,
        right: DataId,
        span: Range<usize>,
    ) -> DataId {
        let out = self.temp("SystemString");
        self.call_extern(
            ctx,
            "SystemString.__Concat__SystemString_SystemString__SystemString",
            &[left, right, out],
            span,
        );
        out
    }

    /// `x is (int, int)` at run time: an `object[]` of the right length
    /// carrying this shape's type id.
    pub(super) fn tuple_type_test(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let elements = Self::tuple_elements(ty)?.len();
        let type_id = self.tuple_type_id(ty)?;
        let result = self.temp("SystemBoolean");
        let no = self.bool_constant(false);
        self.copy(no, result);
        let end = self.fresh_label("tuple_test_end");
        self.check_object_array_of(ctx, value, elements + 1, end, span.clone());
        let zero = self.int_constant(0);
        let int32 = self.corlib_type("Int32");
        let their_id = self.get_element(ctx, value, zero, &int32, span.clone());
        let wanted = self.int_constant(type_id);
        let same = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[their_id, wanted, same],
            span,
        );
        self.program.code.push(Op::Push(same));
        self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
        let yes = self.bool_constant(true);
        self.copy(yes, result);
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// The type id a tuple of this shape carries in slot 0 — what makes a
    /// boxed one recognisable to the object dispatcher.
    fn tuple_type_id(&mut self, ty: &Type) -> Option<i32> {
        self.layout_of(ty).map(|layout| layout.type_id)
    }

    /// The body of a [`Role::TupleMember`]: the dispatcher lands here with
    /// a tuple of one shape, and the elements do the rest.
    pub(super) fn emit_tuple_member_body(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: ObjectMember,
        shape: u32,
    ) {
        let Some(ty) = self.type_order.get(shape as usize).cloned() else {
            return;
        };
        let this = ctx.this_slot.expect("a tuple member has a receiver");
        let Some(result) = ctx.result else {
            return;
        };
        let span = 0..0;
        let value = match member {
            ObjectMember::ToString => Some(self.tuple_to_string(ctx, this, &ty, span)),
            ObjectMember::GetHashCode => self.tuple_hash_code(ctx, this, &ty, span),
            ObjectMember::Equals => {
                let other = self.functions[&ctx.key].parameters[1];
                self.tuple_equals_object(ctx, this, other, &ty, span)
            }
        };
        if let Some(value) = value {
            self.copy(value, result);
        }
    }

    /// `t.Equals(o)`: true when `o` is a tuple of the same shape — its type
    /// id says so — and every element matches.
    fn tuple_equals_object(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        other: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let elements = Self::tuple_elements(ty)?.len();
        let result = self.temp("SystemBoolean");
        let no = self.bool_constant(false);
        self.copy(no, result);
        let end = self.fresh_label("tuple_equals_end");
        self.check_object_array_of(ctx, other, elements + 1, end, span.clone());
        // the shapes have to agree, not just the lengths
        if let Some(type_id) = self.tuple_type_id(ty) {
            let zero = self.int_constant(0);
            let int32 = self.corlib_type("Int32");
            let their_id = self.get_element(ctx, other, zero, &int32, span.clone());
            let wanted = self.int_constant(type_id);
            let same = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                &[their_id, wanted, same],
                span.clone(),
            );
            self.program.code.push(Op::Push(same));
            self.program.code.push(Op::JumpIfFalse(Target::Label(end)));
        }
        let equal = self.tuple_equality(ctx, false, (value, ty), (other, ty), span)?;
        self.copy(equal, result);
        self.program.code.push(Op::Label(end));
        Some(result)
    }

    /// `Equals`, `GetHashCode` or `ToString` on a value known to be a tuple.
    pub(super) fn tuple_object_member(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: &str,
        receiver: (DataId, &Type),
        arguments: &[DataId],
        span: Range<usize>,
    ) -> Option<Piece> {
        let (slot, ty) = receiver;
        match member {
            "ToString" => {
                let text = self.tuple_to_string(ctx, slot, ty, span);
                Some(Piece::Value(text, self.corlib_type("String")))
            }
            "GetHashCode" => {
                let hash = self.tuple_hash_code(ctx, slot, ty, span)?;
                Some(Piece::Value(hash, self.corlib_type("Int32")))
            }
            // `a.Equals(b)`: `b` arrives as an `object`, so its shape is
            // checked at run time before the elements are compared
            "Equals" => {
                let other = *arguments.first()?;
                let elements = Self::tuple_elements(ty)?.len();
                let result = self.temp("SystemBoolean");
                let no = self.bool_constant(false);
                self.copy(no, result);
                let end = self.fresh_label("tuple_equals_end");
                self.check_object_array_of(ctx, other, elements + 1, end, span.clone());
                let equal = self.tuple_equality(ctx, false, (slot, ty), (other, ty), span)?;
                self.copy(equal, result);
                self.program.code.push(Op::Label(end));
                Some(Piece::Value(result, self.corlib_type("Boolean")))
            }
            _ => None,
        }
    }

    /// Jumps to `fail` unless the value is an `object[]` of that length —
    /// what a tuple of the shape in hand looks like at run time.
    pub(super) fn check_object_array_of(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        length: usize,
        fail: LabelId,
        span: Range<usize>,
    ) {
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let condition = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[value, null, condition],
            span.clone(),
        );
        self.jump_if(condition, fail);

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

        let array = self.temp("SystemObjectArray");
        self.copy(value, array);
        let array_type = Type::Array {
            element: Box::new(self.corlib_type("Object")),
            rank: 1,
        };
        let actual = self.array_length(ctx, array, &array_type, span.clone());
        let wanted = self.int_constant(length as i32);
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[actual, wanted, condition],
            span,
        );
        self.program.code.push(Op::Push(condition));
        self.program.code.push(Op::JumpIfFalse(Target::Label(fail)));
    }

    /// The hash of one value of a known type: a nested tuple's own, a
    /// struct's field-wise one, and otherwise what the object has (null
    /// hashing to zero, as the struct hash does).
    fn value_hash_code(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        if Self::tuple_elements(ty).is_some() {
            return self.tuple_hash_code(ctx, value, ty, span);
        }
        let hash = self.temp("SystemInt32");
        if let Some(Piece::Value(result, _)) = self.object_member_call(
            ctx,
            "GetHashCode",
            (value, ty.clone()),
            &[],
            &self.corlib_type("Int32"),
            span.clone(),
        ) {
            self.copy(result, hash);
            return Some(hash);
        }
        let null = self.constant("SystemObject", "null", HeapInit::Null);
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[value, null, is_null],
            span.clone(),
        );
        let compute = self.fresh_label("tuple_hash_element");
        let join = self.fresh_label("tuple_hash_join");
        self.program.code.push(Op::Push(is_null));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(compute)));
        let zero = self.int_constant(0);
        self.copy(zero, hash);
        self.program.code.push(Op::Jump(Target::Label(join)));
        self.program.code.push(Op::Label(compute));
        self.call_extern(
            ctx,
            "SystemObject.__GetHashCode__SystemInt32",
            &[value, hash],
            span,
        );
        self.program.code.push(Op::Label(join));
        Some(hash)
    }

    /// A tuple's hash, mixed from its elements' — so a tuple works as a
    /// dictionary key, which is most of what people build them for.
    pub(super) fn tuple_hash_code(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let elements = Self::tuple_elements(ty).map(<[TupleElement]>::to_vec)?;
        let int32 = self.corlib_type("Int32");
        let mut hash = self.int_constant(17);
        for (index, element) in elements.iter().enumerate() {
            let element_type = self.substitute(&element.element, &ctx.key.bindings);
            let at = self.tuple_slot(index);
            let read = self.get_element(ctx, value, at, &element_type, span.clone());
            let element_hash = self.value_hash_code(ctx, read, &element_type, span.clone())?;
            let factor = self.int_constant(31);
            let scaled = self.emit_binary_operator(
                ctx,
                men_sharp_parser::ast::BinaryOperator::Multiply,
                (hash, &int32),
                (factor, &int32),
                &int32,
                span.clone(),
                None,
            )?;
            hash = self.emit_binary_operator(
                ctx,
                men_sharp_parser::ast::BinaryOperator::Add,
                (scaled, &int32),
                (element_hash, &int32),
                &int32,
                span.clone(),
                None,
            )?;
        }
        Some(hash)
    }

    // ------------------------------------------------------ deconstruction

    /// `var (a, b) = t;`, `(c, d) = t;`, `(int e, string f) = t;` — the
    /// elements go straight into the variables; the tuple itself is never
    /// copied, since nothing keeps it.
    pub(super) fn lower_deconstruction(
        &mut self,
        ctx: &mut Ctx<'ast>,
        assignment: &'ast men_sharp_parser::ast::AssignmentExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let value = assignment.value.as_ref().ok()?;
        let ty = self.type_of(ctx, value);
        let slot = self.lower_expression(ctx, value)?;
        self.bind_deconstruction(ctx, &assignment.target, slot, &ty, assignment.span.clone());
        Some(slot)
    }

    fn bind_deconstruction(
        &mut self,
        ctx: &mut Ctx<'ast>,
        target: &'ast Expression<'ast, 'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) {
        match target {
            Expression::Declaration(declaration) => {
                let declared = self
                    .bodies
                    .resolved_types
                    .get(&EntityID::from(&declaration.variable_type))
                    .cloned()
                    .map(|ty| self.substitute(&ty, &ctx.key.bindings));
                self.bind_designation(ctx, &declaration.designation, value, ty, &declared, span);
            }
            Expression::Primary(primary) => {
                let PrimaryLeft::Tuple { elements, .. } = &primary.left else {
                    self.error(ctx, "internal: not a deconstruction target", span);
                    return;
                };
                let node = EntityID::from(&primary.left);
                let Some(parts) =
                    self.deconstructed_parts(ctx, value, ty, node, elements.len(), span.clone())
                else {
                    return;
                };
                for (index, element) in elements.iter().enumerate() {
                    let (read, part) = parts[index].clone();
                    match &element.value {
                        target @ (Expression::Declaration(_) | Expression::Primary(_))
                            if Self::is_deconstruction_target(target) =>
                        {
                            self.bind_deconstruction(ctx, target, read, &part, span.clone());
                        }
                        target if Self::is_discard_target(target) => {}
                        target => {
                            let place = self.lower_place(ctx, target);
                            let written = match place_type(&place) {
                                Some(into) => self.convert(ctx, read, &part, &into, span.clone()),
                                None => read,
                            };
                            let owned = self.owned_copy(ctx, written, &part, span.clone());
                            self.write_place(ctx, place, owned, span.clone());
                        }
                    }
                }
            }
            other => {
                self.error(ctx, "internal: not a deconstruction target", other.span());
            }
        }
    }

    /// The names one side of a deconstruction declares, bound to the parts.
    pub(super) fn bind_designation(
        &mut self,
        ctx: &mut Ctx<'ast>,
        designation: &'ast VariableDesignation<'ast, 'ast>,
        value: DataId,
        ty: &Type,
        declared: &Option<Type>,
        span: Range<usize>,
    ) {
        match designation {
            VariableDesignation::Single(name) => {
                let into = match declared {
                    Some(declared) if !matches!(declared, Type::Infer) => declared.clone(),
                    _ => ty.clone(),
                };
                let converted = self.convert(ctx, value, ty, &into, span.clone());
                let owned = self.owned_copy(ctx, converted, &into, span.clone());
                let slot = self.temp_for(&into);
                self.copy(owned, slot);
                self.bind_local(ctx, name.value, slot, into);
            }
            VariableDesignation::Discard(_) => {}
            VariableDesignation::Parenthesized { elements, .. } => {
                let node = EntityID::from(designation);
                let Some(parts) =
                    self.deconstructed_parts(ctx, value, ty, node, elements.len(), span.clone())
                else {
                    return;
                };
                for (index, element) in elements.iter().enumerate() {
                    let (read, part) = parts[index].clone();
                    self.bind_designation(ctx, element, read, &part, declared, span.clone());
                }
            }
        }
    }

    fn tuple_parts_of(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        wanted: usize,
        span: &Range<usize>,
    ) -> Option<Vec<Type>> {
        let elements = Self::tuple_elements(ty)?;
        if elements.len() != wanted {
            self.error(ctx, "internal: this tuple has another shape", span.clone());
            return None;
        }
        Some(
            elements
                .iter()
                .map(|element| self.substitute(&element.element, &ctx.key.bindings))
                .collect(),
        )
    }

    /// The parts a value comes apart into: a tuple's elements read out of
    /// its array, or the `out` parameters its `Deconstruct` writes — the
    /// checker recorded which on `node`.
    pub(super) fn deconstructed_parts(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        node: EntityID,
        wanted: usize,
        span: Range<usize>,
    ) -> Option<Vec<(DataId, Type)>> {
        if Self::tuple_elements(ty).is_some() {
            let parts = self.tuple_parts_of(ctx, ty, wanted, &span)?;
            return Some(
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(index, part)| {
                        let at = self.tuple_slot(index);
                        let read = self.get_element(ctx, value, at, &part, span.clone());
                        (read, part)
                    })
                    .collect(),
            );
        }
        let Some(ResolvedTarget::Call(call)) = self.bodies.targets.get(&node).cloned() else {
            self.error(ctx, "internal: nothing to take this value apart", span);
            return None;
        };
        let MemberOrigin::Source(symbol) = call.origin else {
            self.error(
                ctx,
                "a `Deconstruct` from the engine cannot be called on Udon yet: write the                  elements out by hand",
                span,
            );
            return None;
        };
        let receiver = Some((value, self.substitute(ty, &ctx.key.bindings)));
        let (this, key) =
            self.source_call_target(ctx, &call, symbol, &receiver, false, span.clone())?;
        // one temp per `out` parameter: passed in, written home after
        let mut parts: Vec<(DataId, Type)> = Vec::with_capacity(wanted);
        for parameter in &call.signature.parameters {
            let part = self.substitute(&parameter.parameter_type, &ctx.key.bindings);
            let slot = self.temp_for(&part);
            parts.push((slot, part));
        }
        let values: Vec<DataId> = parts.iter().map(|(slot, _)| *slot).collect();
        let by_ref: Vec<(usize, Place)> = parts
            .iter()
            .enumerate()
            .map(|(index, (slot, part))| (index, Place::Slot(*slot, part.clone())))
            .collect();
        // `Deconstruct` returns void, so there is no result to take
        self.call_function(ctx, &key, this, &values, &by_ref, span);
        Some(parts)
    }

    /// The shapes the checker reads as a deconstruction, not a place.
    pub(super) fn is_deconstruction_target(target: &Expression<'ast, 'ast>) -> bool {
        match target {
            Expression::Declaration(_) => true,
            Expression::Primary(primary) => {
                primary.chain.is_empty() && matches!(primary.left, PrimaryLeft::Tuple { .. })
            }
            _ => false,
        }
    }

    /// A bare `_` — a discard, which takes no slot.
    fn is_discard_target(target: &Expression<'ast, 'ast>) -> bool {
        let Expression::Primary(primary) = target else {
            return false;
        };
        primary.chain.is_empty()
            && matches!(
                &primary.left,
                PrimaryLeft::Identifier { name, generics: None, .. } if name.value == "_"
            )
    }

    /// `t.Item1`, `t.x`: the element at that position.
    pub(super) fn tuple_element_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: &Option<(DataId, Type)>,
        name: &str,
        span: &Range<usize>,
    ) -> Option<Place> {
        let (slot, ty) = receiver.as_ref()?;
        let elements = Self::tuple_elements(ty)?;
        let index = tuple_element_index(elements, name)?;
        let element = self.substitute(&elements[index].element, &ctx.key.bindings);
        let _ = span;
        Some(Place::Field {
            object: *slot,
            index: self.tuple_slot(index),
            ty: element,
        })
    }
}
