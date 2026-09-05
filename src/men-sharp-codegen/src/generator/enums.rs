//! `ToString` of a source enum — the member's name, as in C#.
//!
//! A source enum is an Int32 on the heap and nothing more: Udon has no
//! `System.Enum` extern that could look its name up, and a boxed one is a
//! boxed int. So each enum gets a function of its own, synthesized from
//! its declaration: `string (int value)` that compares the value with every
//! member's constant and returns the name of the first that matches, or
//! the number when none does — exactly what .NET prints. A `[Flags]` enum
//! is decomposed the way .NET's `Enum.ToString` does it: the members with
//! the largest values first, joined with ", " in ascending order, and the
//! whole value as a number when some bit belongs to no member.
//!
//! String concatenation, interpolation and `.ToString()` all come here when
//! the static type is the enum. Through an `object` the type is gone — a
//! boxed source enum prints as its number, which is documented.

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// `value.ToString()` for a source enum: a call into its name function.
    pub(super) fn enum_to_string(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        symbol: SymbolId,
        span: Range<usize>,
    ) -> DataId {
        let key = FunctionKey {
            symbol,
            role: Role::EnumToString,
            bindings: Vec::new(),
        };
        match self.call_function(ctx, &key, None, &[value], &[], span) {
            Some(text) => text,
            None => self.string_constant(""),
        }
    }

    /// The members of a source enum as (name, value), in declaration
    /// order; a value declared twice keeps its first name, as .NET's lookup
    /// does.
    fn enum_names(&mut self, symbol: SymbolId) -> Vec<(String, i32)> {
        let members: Vec<SymbolId> = self.declarations.table.symbol(symbol).members.to_vec();
        let mut names: Vec<(String, i32)> = Vec::new();
        for member in members {
            let entry = self.declarations.table.symbol(member);
            if entry.kind != SymbolKind::EnumMember {
                continue;
            }
            let name = entry.name.to_string();
            let Some(value) = self.enum_member_value(member) else {
                continue;
            };
            let value = value as i32;
            if names.iter().any(|(_, seen)| *seen == value) {
                continue;
            }
            names.push((name, value));
        }
        names
    }

    fn is_flags_enum(&self, symbol: SymbolId) -> bool {
        self.attribute_sections(symbol).iter().any(|sections| {
            sections
                .iter()
                .flat_map(|section| section.attributes)
                .any(|attribute| {
                    matches!(attribute_name(attribute), Some("Flags" | "FlagsAttribute"))
                })
        })
    }

    /// The body of `Role::EnumToString`.
    pub(super) fn emit_enum_to_string(&mut self, ctx: &mut Ctx<'ast>) {
        let value = self.functions[&ctx.key].parameters[0];
        let result = ctx.result.expect("ToString returns a string");
        let symbol = ctx.key.symbol;
        let names = self.enum_names(symbol);
        let span = 0..0;
        let end = self.fresh_label("enum_tostring_end");

        if self.is_flags_enum(symbol) {
            self.emit_flags_to_string(ctx, value, result, &names, end);
        } else {
            for (name, constant) in &names {
                let next = self.fresh_label("enum_tostring_next");
                let expected = self.int_constant(*constant);
                let equal = self.temp("SystemBoolean");
                self.call_extern(
                    ctx,
                    "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                    &[value, expected, equal],
                    span.clone(),
                );
                self.program.code.push(Op::Push(equal));
                self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
                let text = self.string_constant(name);
                self.copy(text, result);
                self.program.code.push(Op::Jump(Target::Label(end)));
                self.program.code.push(Op::Label(next));
            }
        }

        // no member has this value: the number
        self.call_extern(
            ctx,
            "SystemInt32.__ToString__SystemString",
            &[value, result],
            span,
        );
        self.program.code.push(Op::Label(end));
    }

    /// `[Flags]`: the names of the members whose bits are all set, largest
    /// value first, joined in ascending order; falls through to the caller's
    /// number when bits are left over.
    fn emit_flags_to_string(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: DataId,
        result: DataId,
        names: &[(String, i32)],
        end: LabelId,
    ) {
        let span = 0..0;
        let zero = self.int_constant(0);
        let equal = self.temp("SystemBoolean");

        // zero: the member that is zero, or "0"
        let not_zero = self.fresh_label("flags_not_zero");
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[value, zero, equal],
            span.clone(),
        );
        self.program.code.push(Op::Push(equal));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(not_zero)));
        let zero_name = names
            .iter()
            .find(|(_, constant)| *constant == 0)
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "0".to_string());
        let text = self.string_constant(&zero_name);
        self.copy(text, result);
        self.program.code.push(Op::Jump(Target::Label(end)));
        self.program.code.push(Op::Label(not_zero));

        let remaining = self.temp("SystemInt32");
        self.copy(value, remaining);
        let joined = self.temp("SystemString");
        let empty = self.string_constant("");
        self.copy(empty, joined);
        let separator = self.string_constant(", ");
        let masked = self.temp("SystemInt32");
        let mut descending: Vec<&(String, i32)> = names
            .iter()
            .filter(|(_, constant)| *constant != 0)
            .collect();
        // by unsigned value, as .NET sorts the underlying values
        descending.sort_by_key(|entry| std::cmp::Reverse(entry.1 as u32));
        for (name, constant) in descending {
            let skip = self.fresh_label("flags_skip");
            let bits = self.int_constant(*constant);
            self.call_extern(
                ctx,
                "SystemInt32.__op_LogicalAnd__SystemInt32_SystemInt32__SystemInt32",
                &[remaining, bits, masked],
                span.clone(),
            );
            self.call_extern(
                ctx,
                "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                &[masked, bits, equal],
                span.clone(),
            );
            self.program.code.push(Op::Push(equal));
            self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));
            // this name goes in front of what is joined so far
            let text = self.string_constant(name);
            let is_first = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                "SystemString.__op_Equality__SystemString_SystemString__SystemBoolean",
                &[joined, empty, is_first],
                span.clone(),
            );
            let first = self.fresh_label("flags_first");
            let appended = self.fresh_label("flags_appended");
            self.program.code.push(Op::Push(is_first));
            self.program
                .code
                .push(Op::JumpIfFalse(Target::Label(first)));
            self.copy(text, joined);
            self.program.code.push(Op::Jump(Target::Label(appended)));
            self.program.code.push(Op::Label(first));
            let with_separator = self.temp("SystemString");
            self.call_extern(
                ctx,
                "SystemString.__Concat__SystemString_SystemString__SystemString",
                &[text, separator, with_separator],
                span.clone(),
            );
            self.call_extern(
                ctx,
                "SystemString.__Concat__SystemString_SystemString__SystemString",
                &[with_separator, joined, joined],
                span.clone(),
            );
            self.program.code.push(Op::Label(appended));
            let cleared = self.int_constant(!*constant);
            self.call_extern(
                ctx,
                "SystemInt32.__op_LogicalAnd__SystemInt32_SystemInt32__SystemInt32",
                &[remaining, cleared, remaining],
                span.clone(),
            );
            self.program.code.push(Op::Label(skip));
        }

        // every bit accounted for: the names; otherwise the number
        let leftover = self.fresh_label("flags_leftover");
        self.call_extern(
            ctx,
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
            &[remaining, zero, equal],
            span,
        );
        self.program.code.push(Op::Push(equal));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(leftover)));
        self.copy(joined, result);
        self.program.code.push(Op::Jump(Target::Label(end)));
        self.program.code.push(Op::Label(leftover));
    }
}
