//! Rectangular arrays: `int[,]`, `string[,,]`.
//!
//! Udon has no type for a multi-dimensional array — only `T[]` exists on
//! its heap — so one is built out of what it does have: an `object[]` in
//! the same shape a struct or a tuple takes, whose slots are
//!
//! | slot     | holds                                                  |
//! |----------|--------------------------------------------------------|
//! | 0        | the type id (so `is int[,]` and casts still work)      |
//! | 1        | the elements, one flat `T[]` in row-major order        |
//! | 2 + k    | the length of dimension `k`                            |
//!
//! `a[i, j]` is `data[i * len1 + j]` after every index is checked against
//! its own dimension — an index past the end of a row would otherwise land
//! in the next row instead of throwing, as C# promises. `foreach` walks the
//! flat array, which is exactly the order C# visits a rectangular array in.
//! `Length`, `Rank`, `GetLength`, `GetUpperBound`, `GetLowerBound` and
//! `Clone` are the members supported; anything else on `System.Array` is a
//! compile error rather than an extern quietly called on the wrong shape.
//! A rectangular array cannot be handed to something typed `System.Array`
//! for the same reason.

use men_sharp_parser::ast::{Argument, ArgumentValue, CollectionElement, Initializer};

use super::*;

const DATA_SLOT: i32 = 1;
const FIRST_LENGTH_SLOT: i32 = 2;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// The rank of a rectangular array type; `None` for anything else,
    /// one-dimensional arrays included.
    pub(super) fn rectangular_rank(ty: &Type) -> Option<u32> {
        match ty {
            Type::Array { rank, .. } if *rank > 1 => Some(*rank),
            _ => None,
        }
    }

    /// The flat `T[]` that holds the elements.
    pub(super) fn rectangular_data_type(ty: &Type) -> Type {
        match ty {
            Type::Array { element, .. } => Type::Array {
                element: element.clone(),
                rank: 1,
            },
            other => other.clone(),
        }
    }

    fn rectangular_element_type(&self, ctx: &Ctx<'ast>, ty: &Type) -> Type {
        match ty {
            Type::Array { element, .. } => self.substitute(element, &ctx.key.bindings),
            other => other.clone(),
        }
    }

    /// The type id in slot 0, allocated on first use like a tuple's.
    fn rectangular_type_id(&mut self, ty: &Type) -> Option<i32> {
        self.layout_of(ty).map(|layout| layout.type_id)
    }

    pub(super) fn rectangular_data(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let data_type = Self::rectangular_data_type(ty);
        let slot = self.int_constant(DATA_SLOT);
        self.get_element(ctx, object, slot, &data_type, span)
    }

    /// The length of dimension `dimension`, a compile-time one.
    fn rectangular_length(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        dimension: u32,
        span: Range<usize>,
    ) -> DataId {
        let int32 = self.corlib_type("Int32");
        let slot = self.int_constant(FIRST_LENGTH_SLOT + dimension as i32);
        self.get_element(ctx, object, slot, &int32, span)
    }

    /// A fresh array with these dimension lengths, its elements at their
    /// defaults (what `new T[n]` gives a flat array).
    pub(super) fn new_rectangular(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        lengths: &[DataId],
        span: Range<usize>,
    ) -> DataId {
        let object = self.temp("SystemObjectArray");
        let size = self.int_constant(FIRST_LENGTH_SLOT + lengths.len() as i32);
        self.call_extern(
            ctx,
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
            &[size, object],
            span.clone(),
        );
        if let Some(type_id) = self.rectangular_type_id(ty) {
            let zero = self.int_constant(0);
            let id = self.int_constant(type_id);
            self.set_element(ctx, object, zero, id, span.clone());
        }
        // total = len0 * len1 * ...
        let total = self.temp("SystemInt32");
        let one = self.int_constant(1);
        self.copy(one, total);
        for (dimension, length) in lengths.iter().enumerate() {
            let zero = self.int_constant(0);
            let negative = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean",
                &[*length, zero, negative],
                span.clone(),
            );
            let fine = self.fresh_label("rect_length_ok");
            self.program.code.push(Op::Push(negative));
            self.program.code.push(Op::JumpIfFalse(Target::Label(fine)));
            // C# throws OverflowException for a negative length
            self.throw_new(ctx, &["System", "OverflowException"], None, span.clone());
            self.program.code.push(Op::Label(fine));
            self.call_extern(
                ctx,
                "SystemInt32.__op_Multiplication__SystemInt32_SystemInt32__SystemInt32",
                &[total, *length, total],
                span.clone(),
            );
            let slot = self.int_constant(FIRST_LENGTH_SLOT + dimension as i32);
            self.set_element(ctx, object, slot, *length, span.clone());
        }
        let data_type = Self::rectangular_data_type(ty);
        let data = self.allocate_array(ctx, &data_type, total, span.clone());
        let slot = self.int_constant(DATA_SLOT);
        self.set_element(ctx, object, slot, data, span);
        object
    }

    /// `new int[2, 3]`, with an optional `{ { ... }, { ... } }` after it.
    pub(super) fn lower_new_rectangular(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        sizes: &'ast [Expression<'ast, 'ast>],
        initializer: &'ast Option<Initializer<'ast, 'ast>>,
        span: Range<usize>,
    ) -> Piece {
        let int32 = self.corlib_type("Int32");
        let mut lengths = Vec::with_capacity(sizes.len());
        for size in sizes {
            let Some(length) = self.owned_value_as(ctx, size, &int32) else {
                return Piece::Error;
            };
            lengths.push(length);
        }
        let object = self.new_rectangular(ctx, ty, &lengths, span.clone());
        if let Some(Initializer::Collection { elements, .. }) = initializer {
            let data = self.rectangular_data(ctx, object, ty, span.clone());
            let data_type = Self::rectangular_data_type(ty);
            let mut position = 0;
            self.fill_rectangular(ctx, data, &data_type, elements, &mut position, span);
        }
        Piece::Value(object, ty.clone())
    }

    /// `{ { 1, 2 }, { 3, 4 } }` as a whole array: the dimension lengths are
    /// read off the nesting (the checker saw to it that the rows agree).
    pub(super) fn lower_rectangular_shorthand(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        initializer: &'ast Initializer<'ast, 'ast>,
        span: Range<usize>,
    ) -> Option<DataId> {
        let rank = Self::rectangular_rank(ty)?;
        let elements = match initializer {
            Initializer::Collection { elements, .. } => &**elements,
            Initializer::Object { .. } => &[][..],
        };
        let mut lengths = Vec::with_capacity(rank as usize);
        let mut level: &[CollectionElement<'ast, 'ast>] = elements;
        for _ in 0..rank {
            lengths.push(self.int_constant(level.len() as i32));
            level = match level.first() {
                Some(CollectionElement::Nested(Initializer::Collection { elements, .. })) => {
                    elements
                }
                _ => &[],
            };
        }
        let object = self.new_rectangular(ctx, ty, &lengths, span.clone());
        let data = self.rectangular_data(ctx, object, ty, span.clone());
        let data_type = Self::rectangular_data_type(ty);
        let mut position = 0;
        self.fill_rectangular(ctx, data, &data_type, elements, &mut position, span);
        Some(object)
    }

    /// Writes the leaves of a nested initializer into the flat array in
    /// the order they are written — which is row-major order.
    fn fill_rectangular(
        &mut self,
        ctx: &mut Ctx<'ast>,
        data: DataId,
        data_type: &Type,
        elements: &'ast [CollectionElement<'ast, 'ast>],
        position: &mut i32,
        span: Range<usize>,
    ) {
        let element_type = self.rectangular_element_type(ctx, data_type);
        for element in elements {
            match element {
                CollectionElement::Nested(Initializer::Collection { elements, .. }) => {
                    self.fill_rectangular(ctx, data, data_type, elements, position, span.clone());
                }
                CollectionElement::Nested(Initializer::Object { .. }) => {}
                CollectionElement::Expression(expression) => {
                    if let Some(value) = self.owned_value_as(ctx, expression, &element_type) {
                        let index = self.int_constant(*position);
                        self.array_set(ctx, data, index, value, data_type, span.clone());
                    }
                    *position += 1;
                }
            }
        }
    }

    /// `a[i, j]` as a place: every index checked against its dimension,
    /// then the flat element.
    pub(super) fn rectangular_element_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        ty: &Type,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
    ) -> Place {
        let Some(rank) = Self::rectangular_rank(ty) else {
            return Place::Error;
        };
        if arguments.len() != rank as usize {
            self.error(
                ctx,
                Message::key("codegen.a_rank_dimensional_array_takes_rank_indices")
                    .arg("rank", rank),
                span,
            );
            return Place::Error;
        }
        self.check_not_null(ctx, object, span.clone());
        let int32 = self.corlib_type("Int32");
        let flat = self.temp("SystemInt32");
        for (dimension, argument) in arguments.iter().enumerate() {
            let ArgumentValue::Expression(expression) = &argument.value else {
                return Place::Error;
            };
            let Some(index) = self.owned_value_as(ctx, expression, &int32) else {
                return Place::Error;
            };
            let length = self.rectangular_length(ctx, object, dimension as u32, span.clone());
            self.check_index_in_range(ctx, index, length, span.clone());
            if dimension == 0 {
                self.copy(index, flat);
            } else {
                // flat = flat * len_k + index_k
                self.call_extern(
                    ctx,
                    "SystemInt32.__op_Multiplication__SystemInt32_SystemInt32__SystemInt32",
                    &[flat, length, flat],
                    span.clone(),
                );
                self.call_extern(
                    ctx,
                    "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
                    &[flat, index, flat],
                    span.clone(),
                );
            }
        }
        let data = self.rectangular_data(ctx, object, ty, span);
        Place::Element {
            array: data,
            index: flat,
            element: self.rectangular_element_type(ctx, ty),
            array_type: Self::rectangular_data_type(ty),
        }
    }

    /// `a.Length`, `a.Rank`: the properties `System.Array` has that make
    /// sense here. `None` after an error for any other member.
    pub(super) fn rectangular_member(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        ty: &Type,
        name: &str,
        span: Range<usize>,
    ) -> Piece {
        let Some(rank) = Self::rectangular_rank(ty) else {
            return Piece::Error;
        };
        let int32 = self.corlib_type("Int32");
        match name {
            "Length" => {
                self.check_not_null(ctx, object, span.clone());
                let data = self.rectangular_data(ctx, object, ty, span.clone());
                let data_type = Self::rectangular_data_type(ty);
                let length = self.array_length(ctx, data, &data_type, span);
                Piece::Value(length, int32)
            }
            "Rank" => Piece::Value(self.int_constant(rank as i32), int32),
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.name_is_not_available_on_a_rectangular")
                        .arg("name", name),
                    span,
                );
                Piece::Error
            }
        }
    }

    /// `a.GetLength(k)` and friends.
    pub(super) fn rectangular_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        object: DataId,
        ty: &Type,
        name: &str,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
    ) -> Piece {
        let Some(rank) = Self::rectangular_rank(ty) else {
            return Piece::Error;
        };
        let int32 = self.corlib_type("Int32");
        match (name, arguments) {
            ("GetLength" | "GetUpperBound" | "GetLowerBound", [dimension]) => {
                let ArgumentValue::Expression(expression) = &dimension.value else {
                    return Piece::Error;
                };
                let Some(dimension) = self.owned_value_as(ctx, expression, &int32) else {
                    return Piece::Error;
                };
                self.check_not_null(ctx, object, span.clone());
                let rank_value = self.int_constant(rank as i32);
                self.check_index_in_range(ctx, dimension, rank_value, span.clone());
                if name == "GetLowerBound" {
                    return Piece::Value(self.int_constant(0), int32);
                }
                let first = self.int_constant(FIRST_LENGTH_SLOT);
                let slot = self.temp("SystemInt32");
                self.call_extern(
                    ctx,
                    "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
                    &[dimension, first, slot],
                    span.clone(),
                );
                let length = self.get_element(ctx, object, slot, &int32, span.clone());
                if name == "GetLength" {
                    return Piece::Value(length, int32);
                }
                let one = self.int_constant(1);
                let bound = self.temp("SystemInt32");
                self.call_extern(
                    ctx,
                    "SystemInt32.__op_Subtraction__SystemInt32_SystemInt32__SystemInt32",
                    &[length, one, bound],
                    span,
                );
                Piece::Value(bound, int32)
            }
            ("Clone", []) => {
                self.check_not_null(ctx, object, span.clone());
                let lengths: Vec<DataId> = (0..rank)
                    .map(|dimension| self.rectangular_length(ctx, object, dimension, span.clone()))
                    .collect();
                let copy = self.new_rectangular(ctx, ty, &lengths, span.clone());
                let source = self.rectangular_data(ctx, object, ty, span.clone());
                let target = self.rectangular_data(ctx, copy, ty, span.clone());
                let data_type = Self::rectangular_data_type(ty);
                let total = self.array_length(ctx, source, &data_type, span.clone());
                self.call_extern(
                    ctx,
                    "SystemArray.__Copy__SystemArray_SystemArray_SystemInt32__SystemVoid",
                    &[source, target, total],
                    span,
                );
                // `Clone()` is typed `object` in C#; the value is the array
                Piece::Value(copy, self.corlib_type("Object"))
            }
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.name_is_not_available_on_a_rectangular")
                        .arg("name", name),
                    span,
                );
                Piece::Error
            }
        }
    }

    /// `value is int[,]`: an `object[]` of the right size whose slot 0 is
    /// this shape's type id — the test a tuple gets.
    pub(super) fn rectangular_type_test(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let rank = Self::rectangular_rank(ty)?;
        let type_id = self.rectangular_type_id(ty)?;
        let result = self.temp("SystemBoolean");
        let no = self.bool_constant(false);
        self.copy(no, result);
        let end = self.fresh_label("rect_test_end");
        self.check_object_array_of(
            ctx,
            value,
            FIRST_LENGTH_SLOT as usize + rank as usize,
            end,
            span.clone(),
        );
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
}
