//! `MenSharp.Internal.Comparers.Compare<T>` — `Comparer<T>.Default` for
//! the `T` at hand.
//!
//! Udon has no `IComparable` extern; what it has is one `CompareTo` per
//! type that orders itself (`SystemInt32.__CompareTo__SystemInt32__…`,
//! `SystemString.__CompareTo__SystemString__…`, and so on). The corlib
//! cannot pick between those from generic code, so `Compare<T>` is lowered
//! here, once per instantiation: that extern when `T` has one, the
//! class's `IComparable<T>.CompareTo` through the ordinary dispatch when a
//! source type implements the interface, and a compile error naming the
//! type when neither — sorting something that has no ordering is caught
//! when the program is built, not when the world is running. `null`
//! orders before everything, as in .NET.

use super::*;
use men_sharp_semantics::{FunctionSignature, ParameterPassing, ParameterSignature};

const COMPARERS_PATH: [&str; 3] = ["MenSharp", "Internal", "Comparers"];
const COMPARABLE_PATH: [&str; 2] = ["System", "IComparable"];

impl<'a, 'ast> Generator<'a, 'ast> {
    /// `Comparers.Compare(a, b)` lowered in place; `None` for any other call.
    pub(super) fn try_comparers_intrinsic(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        values: &[DataId],
        span: Range<usize>,
    ) -> Option<Piece> {
        let entry = self.declarations.table.symbol(symbol);
        let parent = entry.parent?;
        if entry.name != "Compare" || self.display_path(parent) != COMPARERS_PATH.join(".") {
            return None;
        }
        let element = self.substitute(call.type_arguments.first()?, &ctx.key.bindings);
        let a = *values.first()?;
        let b = *values.get(1)?;
        let result = self.compare_values(ctx, a, b, &element, span);
        Some(Piece::Value(result, self.corlib_type("Int32")))
    }

    /// `Comparer<T>.Default.Compare(a, b)` into a fresh Int32 slot.
    fn compare_values(
        &mut self,
        ctx: &mut Ctx<'ast>,
        a: DataId,
        b: DataId,
        element: &Type,
        span: Range<usize>,
    ) -> DataId {
        let out = self.temp("SystemInt32");
        if !self.type_system().is_reference_type(element) {
            self.compare_non_null(ctx, a, b, out, element, span);
            return out;
        }
        // null first: (null, null) = 0, (null, x) = -1, (x, null) = 1
        let a_is_null = self.fresh_label("compare_a_null");
        let b_is_null = self.fresh_label("compare_b_null");
        let both_null = self.fresh_label("compare_both_null");
        let end = self.fresh_label("compare_end");
        let a_null = self.is_null(ctx, a, span.clone());
        self.jump_if(a_null, a_is_null);
        let b_null = self.is_null(ctx, b, span.clone());
        self.jump_if(b_null, b_is_null);
        self.compare_non_null(ctx, a, b, out, element, span.clone());
        self.program.code.push(Op::Jump(Target::Label(end)));

        self.program.code.push(Op::Label(a_is_null));
        let b_null = self.is_null(ctx, b, span);
        self.jump_if(b_null, both_null);
        let minus_one = self.int_constant(-1);
        self.copy(minus_one, out);
        self.program.code.push(Op::Jump(Target::Label(end)));

        self.program.code.push(Op::Label(both_null));
        let zero = self.int_constant(0);
        self.copy(zero, out);
        self.program.code.push(Op::Jump(Target::Label(end)));

        self.program.code.push(Op::Label(b_is_null));
        let one = self.int_constant(1);
        self.copy(one, out);
        self.program.code.push(Op::Label(end));
        out
    }

    /// The comparison of two values known not to be null.
    fn compare_non_null(
        &mut self,
        ctx: &mut Ctx<'ast>,
        a: DataId,
        b: DataId,
        out: DataId,
        element: &Type,
        span: Range<usize>,
    ) {
        // a number, string, char, bool, ...: the type's own extern. A
        // source enum is its underlying int on the heap.
        let extern_name = if self.type_system().is_enum_type(element)
            && matches!(
                element,
                Type::Named {
                    target: TypeTarget::Source(_),
                    ..
                }
            ) {
            Some("SystemInt32".to_string())
        } else {
            self.extern_type_name(element)
        };
        if let Some(name) = extern_name {
            let signature = format!("{name}.__CompareTo__{name}__SystemInt32");
            if self.nodes.extern_node(&signature).is_some() {
                self.call_extern(ctx, &signature, &[a, b, out], span);
                return;
            }
        }

        // a source class or struct implementing `IComparable<T>`: its
        // `CompareTo`, through the interface's dispatch
        if let Some(call) = self.comparable_call(element) {
            let receiver = Some((a, element.clone()));
            match self.dispatch_call(
                ctx,
                &call,
                receiver,
                vec![b],
                Vec::new(),
                Vec::new(),
                span.clone(),
                false,
            ) {
                Piece::Value(result, _) => self.copy(result, out),
                _ => self.error(ctx, "internal: `CompareTo` produced no value", span),
            }
            return;
        }

        let display = self.display_type(element);
        let function = self.functions[&ctx.key].name.clone();
        self.error(
            ctx,
            format!(
                "`{display}` has no ordering: it is not a number, string, char or enum, and \
                 does not implement `IComparable<{display}>` — so `{function}` cannot order \
                 it. Implement `IComparable<{display}>` on the type, or order by a key \
                 (`OrderBy(x => x.Name)`, `Max(x => x.Score)`)"
            ),
            span,
        );
    }

    /// The call `a.CompareTo(b)` through `IComparable<T>`, when `element`
    /// implements it.
    fn comparable_call(&mut self, element: &Type) -> Option<ResolvedCall> {
        if !matches!(
            element,
            Type::Named {
                target: TypeTarget::Source(_),
                ..
            }
        ) {
            return None;
        }
        let interface_symbol = self.find_symbol(&COMPARABLE_PATH)?;
        let interface = Type::Named {
            target: TypeTarget::Source(interface_symbol),
            arguments: vec![element.clone()],
        };
        if !self.implements(element, &interface) {
            return None;
        }
        let compare_to = self
            .declarations
            .table
            .symbol(interface_symbol)
            .members
            .iter()
            .copied()
            .find(|&member| {
                let entry = self.declarations.table.symbol(member);
                entry.kind == SymbolKind::Method && entry.name == "CompareTo"
            })?;
        Some(ResolvedCall {
            origin: MemberOrigin::Source(compare_to),
            is_static: false,
            is_extension: false,
            declaring_type: interface,
            signature: FunctionSignature {
                return_type: self.corlib_type("Int32"),
                parameters: vec![ParameterSignature {
                    passing: ParameterPassing::Value,
                    is_params: false,
                    parameter_type: element.clone(),
                    name: None,
                    default_value: None,
                }],
            },
            type_arguments: Vec::new(),
            parameter_of_argument: vec![0],
            params_expansion: None,
        })
    }
}
