//! Records: what the parser could not write down as ordinary members.
//!
//! A positional record's properties, constructor and `Deconstruct` are
//! synthesized as syntax by the parser (see `parser/record.rs`), so they
//! reach here as any other member would. Three things have no C# spelling
//! and are made here:
//!
//! - **value equality** — `Equals`, `GetHashCode` and `==`/`!=` compare
//!   every stored field, the runtime type included (a `Circle` never equals
//!   a `Square` with the same fields). The bodies are the ones a struct
//!   gets (`structs.rs`); a record class only adds the null handling `==`
//!   needs.
//! - **`ToString`** — `Point { X = 1, Y = 2 }`: the public instance fields
//!   and properties, base record's first, in declaration order, exactly
//!   as .NET prints them.
//! - **`with`** — a shallow copy of the object (nested structs copied,
//!   references shared) with the listed members assigned, through the same
//!   machinery an object initializer uses.

use super::*;
use men_sharp_semantics::symbol::Accessibility;

const REFERENCE_EQUALS: &str =
    "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean";
const CONCAT: &str = "SystemString.__Concat__SystemString_SystemString__SystemString";

impl<'a, 'ast> Generator<'a, 'ast> {
    /// A `record` or `record struct` of the user's.
    pub(super) fn is_source_record(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if matches!(
                self.declarations.table.symbol(*symbol).kind,
                SymbolKind::Record | SymbolKind::RecordStruct
            )
        )
    }

    fn is_record_class(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if self.declarations.table.symbol(*symbol).kind == SymbolKind::Record
        )
    }

    /// `a == b` / `a != b` on records: the synthesized `Equals`, with a
    /// null on either side handled first for a record class (`null == null`
    /// is true, `null == x` false, and `x.Equals(null)` is never called).
    pub(super) fn record_equality(
        &mut self,
        ctx: &mut Ctx<'ast>,
        left: DataId,
        right: DataId,
        ty: &Type,
        negate: bool,
        span: Range<usize>,
    ) -> Option<DataId> {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = ty
        else {
            return None;
        };
        let key = FunctionKey {
            symbol: *symbol,
            role: Role::StructEquals,
            bindings: self.struct_bindings(ty),
        };
        let result = self.temp("SystemBoolean");
        let end = self.fresh_label("record_eq_end");
        if self.is_record_class(ty) {
            let null = self.constant("SystemObject", "null", HeapInit::Null);
            let left_null = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                REFERENCE_EQUALS,
                &[left, null, left_null],
                span.clone(),
            );
            let compare = self.fresh_label("record_eq_compare");
            self.program.code.push(Op::Push(left_null));
            self.program
                .code
                .push(Op::JumpIfFalse(Target::Label(compare)));
            // left is null: equal exactly when right is too
            self.call_extern(ctx, REFERENCE_EQUALS, &[right, null, result], span.clone());
            self.program.code.push(Op::Jump(Target::Label(end)));
            self.program.code.push(Op::Label(compare));
        }
        let equal = self.call_function(ctx, &key, Some(left), &[right], &[], span.clone())?;
        self.copy(equal, result);
        self.program.code.push(Op::Label(end));
        if !negate {
            return Some(result);
        }
        let out = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean",
            &[result, out],
            span,
        );
        Some(out)
    }

    /// `value with { X = 1 }`: a copy, then the initializer on the copy.
    pub(super) fn lower_with(
        &mut self,
        ctx: &mut Ctx<'ast>,
        with: &'ast men_sharp_parser::ast::WithExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        use men_sharp_parser::ast::Initializer;
        let ty = self.type_of(ctx, &with.value);
        let ty = self.substitute(&ty, &ctx.key.bindings);
        let source = self.lower_expression(ctx, &with.value)?;
        let copy = self.clone_struct(ctx, source, &ty, with.span.clone());
        if let Ok(Initializer::Object { elements, .. }) = &with.initializer {
            self.apply_object_initializer(ctx, copy, &ty, elements, with.span.clone());
        }
        Some(copy)
    }

    /// Body of the synthesized `string ToString()`.
    pub(super) fn emit_record_to_string(&mut self, ctx: &mut Ctx<'ast>) {
        let this = ctx.this_slot.expect("ToString has a receiver");
        let result = ctx.result.expect("ToString returns a string");
        let this_type = ctx.this_type.clone().expect("ToString has a receiver type");
        let span = 0..0;

        // the record and the records above it, base first
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let mut chain = vec![this_type.clone()];
        while let Some(base) = system.base_of(chain.last().expect("one type at least")) {
            if !self.is_source_record(&base) {
                break;
            }
            chain.push(base);
        }
        chain.reverse();

        let Type::Named {
            target: TypeTarget::Source(record),
            ..
        } = &this_type
        else {
            return;
        };
        let name = self.declarations.table.symbol(*record).name.to_string();
        let text = self.temp("SystemString");
        let opening = self.string_constant(&format!("{name} {{ "));
        self.copy(opening, text);

        let mut first = true;
        for declaring in chain {
            let Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } = &declaring
            else {
                continue;
            };
            let members: Vec<SymbolId> = self.declarations.table.symbol(*symbol).members.to_vec();
            for member in members {
                let entry = self.declarations.table.symbol(member);
                if !matches!(entry.kind, SymbolKind::Field | SymbolKind::Property)
                    || entry.is_static
                    || entry.accessibility != Accessibility::Public
                    || system.is_hidden_positional(member)
                {
                    continue;
                }
                let member_name = entry.name.to_string();
                let kind = entry.kind;
                // the member as the receiver sees it: its type instantiated
                let candidate = system
                    .members_named(&declaring, &member_name)
                    .into_iter()
                    .find(|candidate| {
                        matches!(candidate.origin, MemberOrigin::Source(origin) if origin == member)
                    });
                let member_type = match candidate.and_then(|candidate| candidate.signature) {
                    Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => ty,
                    _ => continue,
                };
                let resolved = ResolvedMember {
                    origin: MemberOrigin::Source(member),
                    kind,
                    is_static: false,
                    declaring_type: declaring.clone(),
                    member_type,
                };
                let place = self.member_place(
                    ctx,
                    &resolved,
                    Some((this, this_type.clone())),
                    span.clone(),
                );
                let Some((value, value_type)) = self.read_place(ctx, place, span.clone()) else {
                    continue;
                };
                let shown = self.stringify(ctx, value, &value_type, span.clone());
                let separator = if first { "" } else { ", " };
                let label = self.string_constant(&format!("{separator}{member_name} = "));
                first = false;
                let with_label = self.temp("SystemString");
                self.call_extern(ctx, CONCAT, &[text, label, with_label], span.clone());
                self.call_extern(ctx, CONCAT, &[with_label, shown, text], span.clone());
            }
        }

        let closing = self.string_constant(if first { "}" } else { " }" });
        self.call_extern(ctx, CONCAT, &[text, closing, result], span);
    }
}
