//! Delegates, lambdas and closures.
//!
//! Udon has no function values, but it has `JUMP_INDIRECT`: a jump to the
//! code address held in a heap variable — what every return already does.
//! So a delegate is an `object[]`: element 0 the code address of a *thunk*,
//! the rest what the thunk hands the target function before the arguments
//! proper — the receiver of a method; the `this` and the captured
//! variables of a lambda.
//!
//! A call of a delegate cannot copy its arguments into the target's own
//! parameter slots, the target being unknown until run time. Every call of
//! a delegate of one *shape* (parameter and return types) goes through that
//! shape's **invoker** instead: an ordinary function taking the delegate
//! and the arguments, whose body reads element 0 and jumps to it. The
//! thunk it lands in is made for one (target, shape) pair: it copies the
//! delegate's payload and the invoker's parameters into the target's
//! parameters, calls the target, copies the result into the invoker's
//! result slot and returns through the invoker's return address. To the
//! frame machinery a thunk is the edge invoker → target, with a save/restore
//! marker like any call site — recursion through a delegate is saved like
//! any other recursion.
//!
//! Element 1 of every delegate is its **owner**: the UdonBehaviour whose
//! program made it, and so whose code the address in element 0 is. A
//! delegate can travel to another behaviour's program — through a static
//! field, or a field of an object a static field reaches — and that
//! program cannot jump to a foreign address. So the invoker checks the
//! owner first: its own, and it jumps; another's, and it hands the call
//! over the only way there is between programs — the delegate and the
//! (boxed) arguments through `SetProgramVariable`, then the owner's
//! `__mensharpInvoke` event, then the result back through
//! `GetProgramVariable`. On the owner's side that event jumps to the
//! address in element 2, the **remote entry** of the delegate's shape,
//! which unboxes the arguments into the invoker's parameters and calls it
//! as any local call would. Element 3 on is the payload.
//!
//! A lambda is a function of its own (`Role::Lambda`), compiled from the
//! enclosing member's body with that member's generic bindings. What it
//! captures travels in **boxes**: a local or parameter that some lambda
//! names is declared as a one-element `object[]` in the enclosing body and
//! read and written through it there as well, so the lambda and the body
//! share one variable — and a lambda made in a loop iteration keeps that
//! iteration's box, as C# promises. The checker says which names those
//! are (`BodyCheck::captured_locals`, `BodyCheck::captures`).

use men_sharp_parser::ast::{LambdaBody, LambdaExpression, LambdaParameters, MethodDeclaration};
use men_sharp_semantics::{ExternalTypeKind, FunctionSignature, ParameterPassing};

use super::*;

/// A delegate's parameter and return types, concrete: what an invoker is
/// made for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct DelegateShape {
    /// Each parameter's type and whether it is `ref`/`out`.
    pub(super) parameters: Vec<(Type, bool)>,
    pub(super) returns: Type,
}

/// A lambda that became a function.
pub(super) struct LambdaInfo<'ast> {
    node: &'ast LambdaExpression<'ast, 'ast>,
    /// The captured variables in payload order, each with its type: the
    /// boxes the function receives before its own parameters.
    captures: Vec<(String, Type)>,
    pub(super) has_this: bool,
    parameters: Vec<Type>,
    returns: Type,
}

/// A local function that became a function.
pub(super) struct LocalFunctionInfo<'ast> {
    pub(super) node: &'ast MethodDeclaration<'ast, 'ast>,
    /// The captured variables in parameter order, each with its type: the
    /// boxes the function receives before its own parameters.
    captures: Vec<(String, Type)>,
    pub(super) has_this: bool,
    parameters: Vec<Type>,
    returns: Type,
}

/// A thunk waiting to be emitted: the entry into `target` from the invoker
/// of `shape`.
pub(super) struct Thunk {
    pub(super) label: LabelId,
    pub(super) target: FunctionKey,
    pub(super) shape: u32,
    pub(super) kind: ThunkKind,
}

pub(super) enum ThunkKind {
    /// A call of the target: `payload` is how many delegate elements after
    /// the header feed the target's leading parameters — its `this`, then
    /// a lambda's boxes.
    Call { payload: usize },
    /// The multicast thunk of the shape (`target` is its invoker).
    Multicast,
    /// Resumes a suspended `async` target from its snapshot (see `tasks`).
    Resume,
    /// The remote entry of the shape (`target` is its invoker): where the
    /// `__mensharpInvoke` event lands for a delegate of this shape.
    RemoteEntry,
}

const GET: &str = "SystemObjectArray.__Get__SystemInt32__SystemObject";
const NEW_OBJECT_ARRAY: &str = "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray";
const REFERENCE_EQUALS: &str =
    "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean";

/// The delegate's header: address, owner, remote entry — the payload
/// starts after it. Kept in step with `corlib/Delegates.cs`.
pub(super) const DELEGATE_OWNER: i32 = 1;
pub(super) const DELEGATE_REMOTE_ENTRY: i32 = 2;
pub(super) const DELEGATE_PAYLOAD: i32 = 3;

/// The event a program raises on another to run one of that program's
/// delegates, and the slots it fills first / reads afterwards.
pub(super) const REMOTE_INVOKE_EVENT: &str = "__mensharpInvoke";
const REMOTE_INVOKE_DELEGATE: &str = "__invoke_delegate";
const REMOTE_INVOKE_ARGUMENTS: &str = "__invoke_args";
const REMOTE_INVOKE_RESULT: &str = "__invoke_result";

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------ types

    pub(super) fn is_delegate_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => self.declarations.table.symbol(*symbol).kind == SymbolKind::Delegate,
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.external.type_info(*id).kind == ExternalTypeKind::Delegate,
            Type::Nullable(inner) => self.is_delegate_type(inner),
            _ => false,
        }
    }

    /// `object[]` as a type: what a delegate and a box are stored as.
    fn object_array_type(&self) -> Type {
        Type::Array {
            element: Box::new(self.corlib_type("Object")),
            rank: 1,
        }
    }

    /// The shape of a delegate type — its `Invoke` — with `bindings`
    /// applied. Every delegate value of a type is made and called through
    /// this one function, so the shape is canonical.
    pub(super) fn delegate_shape(
        &self,
        ty: &Type,
        bindings: &[(SymbolId, Type)],
    ) -> Option<DelegateShape> {
        let ty = match self.substitute(ty, bindings) {
            Type::Nullable(inner) => *inner,
            other => other,
        };
        let signature = match &ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                arguments,
            } => {
                if self.declarations.table.symbol(*symbol).kind != SymbolKind::Delegate {
                    return None;
                }
                let MemberSignature::Function(signature) = self.signatures.members.get(symbol)?
                else {
                    return None;
                };
                // the delegate's own type parameters bound to its arguments
                let own: Vec<(SymbolId, Type)> = self
                    .type_parameter_chain(*symbol)
                    .into_iter()
                    .zip(arguments.iter().cloned())
                    .collect();
                self.substitute_signature(signature, &own)
            }
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => {
                if self.external.type_info(*id).kind != ExternalTypeKind::Delegate {
                    return None;
                }
                let system = men_sharp_semantics::TypeSystem {
                    declarations: self.declarations,
                    signatures: self.signatures,
                    external: self.external,
                };
                system
                    .members_named(&ty, "Invoke")
                    .into_iter()
                    .find_map(|candidate| match candidate.signature {
                        Some(MemberSignature::Function(function)) => Some(function),
                        _ => None,
                    })?
            }
            _ => return None,
        };
        Some(Self::shape_of_signature(&signature))
    }

    fn shape_of_signature(signature: &FunctionSignature) -> DelegateShape {
        DelegateShape {
            parameters: signature
                .parameters
                .iter()
                .map(|parameter| {
                    (
                        parameter.parameter_type.clone(),
                        matches!(
                            parameter.passing,
                            ParameterPassing::Ref | ParameterPassing::Out
                        ),
                    )
                })
                .collect(),
            returns: signature.return_type.clone(),
        }
    }

    // --------------------------------------------------------- invokers

    /// The invoker of a shape, made on first use.
    pub(super) fn ensure_invoker(&mut self, shape: &DelegateShape) -> (FunctionKey, u32) {
        if let Some(key) = self.invokers.get(shape) {
            let Role::DelegateInvoker(index) = key.role else {
                unreachable!("an invoker has the invoker role");
            };
            return (key.clone(), index);
        }
        let index = self.delegate_shapes.len() as u32;
        self.delegate_shapes.push(shape.clone());
        let key = FunctionKey {
            symbol: self.entry.expect("the entry class is known"),
            role: Role::DelegateInvoker(index),
            bindings: Vec::new(),
        };
        self.invokers.insert(shape.clone(), key.clone());
        self.ensure_function(&key);
        // a delegate of any shape may be handed to this program by another
        // to run: the event's dispatch function, once
        let remote = self.remote_dispatch_key();
        self.ensure_function(&remote);
        (key, index)
    }

    /// The function the `__mensharpInvoke` event calls: jumps to the remote
    /// entry the delegate names, which returns on its behalf.
    pub(super) fn remote_dispatch_key(&self) -> FunctionKey {
        FunctionKey {
            symbol: self.entry.expect("the entry class is known"),
            role: Role::DelegateRemote,
            bindings: Vec::new(),
        }
    }

    /// Whether any delegate shape was met: the program then exports the
    /// `__mensharpInvoke` event.
    pub(super) fn has_delegates(&self) -> bool {
        !self.delegate_shapes.is_empty()
    }

    /// The slots the remote protocol goes through, made on first use.
    fn remote_invoke_slots(&mut self) -> (DataId, DataId, DataId) {
        if let Some(slots) = self.remote_invoke_slots {
            return slots;
        }
        let slot = |generator: &mut Self, name: &str, udon_type: &str| {
            generator.program.add_data(DataSymbol {
                name: name.into(),
                udon_type: udon_type.into(),
                init: HeapInit::Null,
                // written by another program's `SetProgramVariable`, which
                // reaches any symbol: not a public variable
                export: false,
                sync: None,
            })
        };
        let slots = (
            slot(self, REMOTE_INVOKE_DELEGATE, "SystemObjectArray"),
            slot(self, REMOTE_INVOKE_ARGUMENTS, "SystemObjectArray"),
            slot(self, REMOTE_INVOKE_RESULT, "SystemObject"),
        );
        self.remote_invoke_slots = Some(slots);
        slots
    }

    /// The code address of a shape's remote entry, as the constant every
    /// delegate of the shape carries in element 2.
    fn remote_entry_address(&mut self, shape: u32) -> DataId {
        let label = match self.remote_entries.get(&shape) {
            Some(label) => *label,
            None => {
                let label = self.program.add_label(format!("remote_entry_{shape}"));
                self.remote_entries.insert(shape, label);
                let invoker = self.invokers[&self.delegate_shapes[shape as usize]].clone();
                self.thunk_queue.push_back(Thunk {
                    label,
                    target: invoker,
                    shape,
                    kind: ThunkKind::RemoteEntry,
                });
                label
            }
        };
        self.thunk_address(label)
    }

    /// An invoker's parameters: the delegate, then the shape's.
    pub(super) fn invoker_shape(&self, index: u32) -> (Vec<Type>, Type) {
        let shape = &self.delegate_shapes[index as usize];
        let mut parameters = vec![self.object_array_type()];
        parameters.extend(shape.parameters.iter().map(|(ty, _)| ty.clone()));
        (parameters, shape.returns.clone())
    }

    /// A lambda function's parameters: one box per captured variable, then
    /// the lambda's own.
    pub(super) fn lambda_shape(&self, key: &FunctionKey) -> (Vec<Type>, Type) {
        let Some(info) = self.lambdas.get(key) else {
            return (Vec::new(), Type::Void);
        };
        let mut parameters: Vec<Type> = info
            .captures
            .iter()
            .map(|_| self.object_array_type())
            .collect();
        parameters.extend(info.parameters.iter().cloned());
        (parameters, info.returns.clone())
    }

    /// The body of an invoker: a null delegate throws; one of this program's
    /// own jumps to the address in element 0 — the thunk returns on the
    /// invoker's behalf; another program's is handed to that program (see
    /// the module note).
    pub(super) fn emit_invoker_body(&mut self, ctx: &mut Ctx<'ast>) {
        let Role::DelegateInvoker(index) = ctx.key.role else {
            unreachable!("an invoker has the invoker role");
        };
        let shape = self.delegate_shapes[index as usize].clone();
        let (parameters, result, return_slot) = {
            let function = &self.functions[&ctx.key];
            (
                function.parameters.clone(),
                function.result,
                function.return_slot,
            )
        };
        let delegate = parameters[0];
        self.check_not_null(ctx, delegate, 0..0);

        let owner = self.temp(BEHAVIOUR_HEAP_TYPE);
        let owner_index = self.int_constant(DELEGATE_OWNER);
        self.call_extern(ctx, GET, &[delegate, owner_index, owner], 0..0);
        let own = self.self_behaviour_slot();
        let mine = self.temp("SystemBoolean");
        self.call_extern(ctx, REFERENCE_EQUALS, &[owner, own, mine], 0..0);
        let remote = self.fresh_label("delegate_remote");
        self.program.code.push(Op::Push(mine));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(remote)));
        let address = self.temp("SystemUInt32");
        let zero = self.int_constant(0);
        self.call_extern(ctx, GET, &[delegate, zero, address], 0..0);
        self.program.code.push(Op::JumpIndirect(address));

        // another program's delegate: the arguments boxed into an array,
        // the delegate and the array written to the owner, its event
        // raised, the result read back
        self.program.code.push(Op::Label(remote));
        if shape.parameters.iter().any(|(_, by_ref)| *by_ref) {
            let message = self.string_constant(
                "a delegate of another behaviour cannot be invoked with a `ref` or `out` \
                 argument",
            );
            self.throw_new(
                ctx,
                &["System", "NotSupportedException"],
                Some(message),
                0..0,
            );
            self.program.code.push(Op::JumpIndirect(return_slot));
            return;
        }
        let count = self.int_constant(shape.parameters.len() as i32);
        let arguments = self.temp("SystemObjectArray");
        self.call_extern(ctx, NEW_OBJECT_ARRAY, &[count, arguments], 0..0);
        for position in 0..shape.parameters.len() {
            let at = self.int_constant(position as i32);
            self.set_element(ctx, arguments, at, parameters[1 + position], 0..0);
        }
        self.set_program_variable(ctx, owner, REMOTE_INVOKE_DELEGATE, delegate, 0..0);
        self.set_program_variable(ctx, owner, REMOTE_INVOKE_ARGUMENTS, arguments, 0..0);
        let event = self.string_constant(REMOTE_INVOKE_EVENT);
        self.call_extern(
            ctx,
            &format!("{BEHAVIOUR_EXTERN_TYPE}.__SendCustomEvent__SystemString__SystemVoid"),
            &[owner, event],
            0..0,
        );
        if let Some(result) = result {
            let returns = shape.returns.clone();
            let value = self.get_program_variable(ctx, owner, REMOTE_INVOKE_RESULT, &returns, 0..0);
            self.copy(value, result);
        }
        self.program.code.push(Op::JumpIndirect(return_slot));
    }

    /// The body of the remote dispatch function: jump to the remote entry
    /// the delegate handed over names; the entry returns on its behalf.
    pub(super) fn emit_remote_dispatch_body(&mut self, ctx: &mut Ctx<'ast>) {
        let (delegate, _, _) = self.remote_invoke_slots();
        self.check_not_null(ctx, delegate, 0..0);
        let entry = self.temp("SystemUInt32");
        let at = self.int_constant(DELEGATE_REMOTE_ENTRY);
        self.call_extern(ctx, GET, &[delegate, at, entry], 0..0);
        self.program.code.push(Op::JumpIndirect(entry));
    }

    /// A shape's remote entry: the delegate and the boxed arguments another
    /// program wrote into the invoker's parameters, the invoker called, its
    /// result put where that program reads it, and the dispatch function
    /// returned from.
    fn emit_remote_entry(&mut self, label: LabelId, shape_index: u32) {
        let shape = self.delegate_shapes[shape_index as usize].clone();
        let invoker = self.invokers[&shape].clone();
        let dispatch = self.remote_dispatch_key();
        let saved_frame = self.enter_frame(Some(dispatch.clone()));
        let ctx = self.dispatcher_ctx(&dispatch);
        let (invoker_label, invoker_parameters, invoker_result, invoker_return) = {
            let function = &self.functions[&invoker];
            (
                function.label,
                function.parameters.clone(),
                function.result,
                function.return_slot,
            )
        };
        let dispatch_return = self.functions[&dispatch].return_slot;
        let (delegate, arguments, result) = self.remote_invoke_slots();

        self.program.code.push(Op::Label(label));
        // the dispatch → invoker edge, saved around like any call site
        let marker = self.frame_markers.len() as u32;
        self.frame_markers.push((dispatch.clone(), invoker.clone()));
        self.call_edges
            .entry(dispatch.clone())
            .or_default()
            .insert(invoker.clone());
        self.program.code.push(Op::SaveFrame(marker));
        self.copy(delegate, invoker_parameters[0]);
        for position in 0..shape.parameters.len() {
            let at = self.int_constant(position as i32);
            self.call_extern(
                &ctx,
                GET,
                &[arguments, at, invoker_parameters[1 + position]],
                0..0,
            );
        }
        let continuation = self
            .program
            .add_label(format!("remote_ret_{}", self.temp_counter));
        self.temp_counter += 1;
        let back = self.code_address_constant(
            format!("__remoteaddr_{}", self.temp_counter),
            Some(continuation),
        );
        self.copy(back, invoker_return);
        self.program
            .code
            .push(Op::Jump(Target::Label(invoker_label)));
        self.program.code.push(Op::Label(continuation));
        self.program.code.push(Op::RestoreFrame(marker));
        if let Some(invoker_result) = invoker_result {
            self.copy(invoker_result, result);
        }
        self.program.code.push(Op::JumpIndirect(dispatch_return));
        self.leave_frame(saved_frame);
    }

    // ------------------------------------------------------------ thunks

    fn ensure_thunk(&mut self, target: &FunctionKey, shape: u32, payload: usize) -> LabelId {
        if let Some(label) = self.thunks.get(&(target.clone(), shape)) {
            return *label;
        }
        let name = format!("thunk_{}_{shape}", self.functions[target].name);
        let label = self.program.add_label(name);
        self.thunks.insert((target.clone(), shape), label);
        self.thunk_queue.push_back(Thunk {
            label,
            target: target.clone(),
            shape,
            kind: ThunkKind::Call { payload },
        });
        label
    }

    /// Emits every thunk registered since the last time. A thunk needs its
    /// invoker and target to exist, which they do from registration on;
    /// it schedules nothing new itself.
    pub(super) fn emit_thunks(&mut self) {
        timescope::scope!("emit thunks");
        while let Some(thunk) = self.thunk_queue.pop_front() {
            match thunk.kind {
                ThunkKind::Call { payload } => self.emit_thunk(thunk, payload),
                ThunkKind::Multicast => self.emit_multicast_thunk(thunk.label, thunk.shape),
                ThunkKind::Resume => self.emit_resume_thunk(thunk.label, &thunk.target),
                ThunkKind::RemoteEntry => self.emit_remote_entry(thunk.label, thunk.shape),
            }
        }
    }

    fn emit_thunk(&mut self, thunk: Thunk, payload: usize) {
        let shape = self.delegate_shapes[thunk.shape as usize].clone();
        let invoker = self.invokers[&shape].clone();
        let ctx = self.dispatcher_ctx(&invoker);
        let (invoker_parameters, invoker_result, invoker_return) = {
            let function = &self.functions[&invoker];
            (
                function.parameters.clone(),
                function.result,
                function.return_slot,
            )
        };
        let (target_label, target_parameters, target_result, target_return) = {
            let function = &self.functions[&thunk.target];
            (
                function.label,
                function.parameters.clone(),
                function.result,
                function.return_slot,
            )
        };

        self.program.code.push(Op::Label(thunk.label));
        if target_parameters.len() != payload + shape.parameters.len() {
            self.errors.push(CodegenError {
                message: format!(
                    "internal: the delegate target `{}` takes {} parameters, the delegate \
                     supplies {}",
                    self.functions[&thunk.target].name,
                    target_parameters.len(),
                    payload + shape.parameters.len()
                )
                .into(),
                file: ctx.file,
                span: 0..0,
            });
            self.program
                .code
                .push(Op::Jump(Target::Address(HALT_ADDRESS)));
            return;
        }

        // the invoker → target edge: saved around the call when the target
        // can come back through the invoker, like any other call site
        let marker = self.frame_markers.len() as u32;
        self.frame_markers
            .push((invoker.clone(), thunk.target.clone()));
        self.call_edges
            .entry(invoker.clone())
            .or_default()
            .insert(thunk.target.clone());
        self.program.code.push(Op::SaveFrame(marker));

        // the payload: receiver / `this` and boxes, straight into the
        // target's leading parameters
        let delegate = invoker_parameters[0];
        for (index, &slot) in target_parameters.iter().enumerate().take(payload) {
            let element = self.int_constant(index as i32 + DELEGATE_PAYLOAD);
            self.call_extern(&ctx, GET, &[delegate, element, slot], 0..0);
        }
        // the arguments the invoker received
        for index in 0..shape.parameters.len() {
            self.copy(
                invoker_parameters[1 + index],
                target_parameters[payload + index],
            );
        }

        let continuation = self
            .program
            .add_label(format!("thunk_ret_{}", self.temp_counter));
        self.temp_counter += 1;
        let address = self.code_address_constant(
            format!("__thunkaddr_{}", self.temp_counter),
            Some(continuation),
        );
        self.copy(address, target_return);
        self.program
            .code
            .push(Op::Jump(Target::Label(target_label)));
        self.program.code.push(Op::Label(continuation));

        // `ref`/`out` parameters go back to the invoker's slots, which the
        // caller copies home — before the restore rewinds the target's
        for (index, (_, by_ref)) in shape.parameters.iter().enumerate() {
            if *by_ref {
                self.copy(
                    target_parameters[payload + index],
                    invoker_parameters[1 + index],
                );
            }
        }
        self.program.code.push(Op::RestoreFrame(marker));
        if let (Some(result), Some(out)) = (target_result, invoker_result) {
            self.copy(result, out);
        }
        self.program.code.push(Op::JumpIndirect(invoker_return));
    }

    /// A fresh delegate object: the thunk's address, this program as its
    /// owner, the shape's remote entry, then the payload.
    pub(super) fn make_delegate(
        &mut self,
        ctx: &mut Ctx<'ast>,
        thunk: LabelId,
        shape: u32,
        payload: &[DataId],
        span: Range<usize>,
    ) -> DataId {
        let size = self.int_constant(payload.len() as i32 + DELEGATE_PAYLOAD);
        let delegate = self.temp("SystemObjectArray");
        self.call_extern(ctx, NEW_OBJECT_ARRAY, &[size, delegate], span.clone());
        let address = self.thunk_address(thunk);
        let zero = self.int_constant(0);
        self.set_element(ctx, delegate, zero, address, span.clone());
        let owner = self.self_behaviour_slot();
        let owner_index = self.int_constant(DELEGATE_OWNER);
        self.set_element(ctx, delegate, owner_index, owner, span.clone());
        let remote = self.remote_entry_address(shape);
        let remote_index = self.int_constant(DELEGATE_REMOTE_ENTRY);
        self.set_element(ctx, delegate, remote_index, remote, span.clone());
        for (index, value) in payload.iter().enumerate() {
            let element = self.int_constant(index as i32 + DELEGATE_PAYLOAD);
            self.set_element(ctx, delegate, element, *value, span.clone());
        }
        delegate
    }

    // ----------------------------------------------------------- lambdas

    /// A lambda expression: its function (scheduled), and a delegate to it
    /// carrying `this` and the boxes of what it captures.
    pub(super) fn lower_lambda(
        &mut self,
        ctx: &mut Ctx<'ast>,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        expression: &'ast Expression<'ast, 'ast>,
    ) -> Option<DataId> {
        let span = lambda.span.clone();
        let delegate_type = self.type_of(ctx, expression);
        let Some(shape) = self.delegate_shape(&delegate_type, &[]) else {
            self.error(
                ctx,
                Message::key("codegen.this_lambda_has_no_delegate_type_to"),
                span,
            );
            return None;
        };
        let key = FunctionKey {
            symbol: ctx.key.symbol,
            role: Role::Lambda(EntityID::from(lambda)),
            bindings: ctx.key.bindings.clone(),
        };

        // what it captures: `this` when the enclosing body has one, then
        // every named variable — each of which the body declared boxed
        let mut payload: Vec<DataId> = Vec::new();
        let mut captures: Vec<(String, Type)> = Vec::new();
        payload.extend(ctx.this_slot);
        let names: Vec<String> = self
            .bodies
            .captures
            .get(&EntityID::from(lambda))
            .cloned()
            .unwrap_or_default();
        for name in &names {
            let found = ctx
                .locals
                .iter()
                .rev()
                .find_map(|scope| scope.get_key_value(name.as_str()));
            let Some((key_name, local)) = found else {
                self.error(
                    ctx,
                    format!("internal: the captured variable `{name}` has no slot"),
                    span,
                );
                return None;
            };
            if !local.boxed {
                self.error(
                    ctx,
                    format!("internal: the captured variable `{name}` was not boxed"),
                    span,
                );
                return None;
            }
            captures.push((key_name.clone(), local.ty.clone()));
            payload.push(local.slot);
        }

        if !self.lambdas.contains_key(&key) {
            self.lambdas.insert(
                key.clone(),
                LambdaInfo {
                    node: lambda,
                    captures,
                    has_this: ctx.this_slot.is_some(),
                    parameters: shape.parameters.iter().map(|(ty, _)| ty.clone()).collect(),
                    returns: shape.returns.clone(),
                },
            );
        }
        self.ensure_function(&key);
        let (_, index) = self.ensure_invoker(&shape);
        let thunk = self.ensure_thunk(&key, index, payload.len());
        Some(self.make_delegate(ctx, thunk, index, &payload, span))
    }

    /// The body of a lambda function: bind the boxes and the parameters,
    /// then the lambda's expression or block.
    pub(super) fn emit_lambda_body(&mut self, ctx: &mut Ctx<'ast>, key: &FunctionKey) {
        let (node, captures, has_this, parameters, returns) = {
            let info = &self.lambdas[key];
            (
                info.node,
                info.captures.clone(),
                info.has_this,
                info.parameters.clone(),
                info.returns.clone(),
            )
        };
        let slots = self.functions[key].parameters.clone();
        let mut next = usize::from(has_this);
        for (name, ty) in captures {
            ctx.locals[0].insert(
                name,
                Local {
                    slot: slots[next],
                    ty,
                    boxed: true,
                },
            );
            next += 1;
        }
        let names: Vec<Option<&'ast str>> = match &node.parameters {
            LambdaParameters::Single(name) => vec![Some(name.value)],
            LambdaParameters::List(list) => list
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_ref().ok().map(|name| name.value))
                .collect(),
        };
        for (index, name) in names.into_iter().enumerate() {
            if let (Some(name), Some(&slot), Some(ty)) =
                (name, slots.get(next + index), parameters.get(index))
            {
                self.bind_local(ctx, name, slot, ty.clone());
            }
        }
        let is_async = Self::has_async_modifier(node.modifiers);
        if is_async {
            self.begin_async(ctx, &returns, node.span.clone());
        }
        // an async lambda's expression is what its task carries
        let returns = match &ctx.async_state {
            Some(state) => state.inner.clone(),
            None => returns,
        };
        match &node.body {
            Ok(LambdaBody::Expression(expression)) => {
                if returns == Type::Void {
                    self.lower_expression(ctx, expression);
                } else if let Some(value) = self.owned_value_as(ctx, expression, &returns) {
                    if is_async {
                        self.complete_async(ctx, Some(value), expression.span());
                    } else if let Some(result) = ctx.result {
                        self.copy(value, result);
                    }
                }
            }
            Ok(LambdaBody::Block(block)) => self.lower_block(ctx, block),
            Err(()) => {}
        }
        if is_async {
            self.end_async(ctx, node.span.clone());
        }
    }

    // --------------------------------------------------- local functions

    /// The function a local function of this body compiles to.
    pub(super) fn local_function_key(&self, ctx: &Ctx<'ast>, id: EntityID) -> FunctionKey {
        FunctionKey {
            symbol: ctx.key.symbol,
            role: Role::LocalFunction(id),
            bindings: ctx.key.bindings.clone(),
        }
    }

    /// Registers every local function written directly in a block, before
    /// the block is lowered: one may be called from above its declaration,
    /// and from a lambda written above it.
    pub(super) fn register_local_functions(
        &mut self,
        ctx: &Ctx<'ast>,
        block: &'ast Block<'ast, 'ast>,
    ) {
        for statement in block.statements {
            let Statement::LocalFunction(node) = statement else {
                continue;
            };
            let id = EntityID::from(node);
            let key = self.local_function_key(ctx, id);
            if self.local_functions.contains_key(&key) {
                continue;
            }
            let Some(declaration) = self.bodies.local_functions.get(&id) else {
                // the checker refused it (a generic one) and reported why
                continue;
            };
            let signature = declaration.signature.clone();
            let is_static = declaration.is_static;
            // the type of a captured variable comes from the checker: the
            // box of one declared further down the block does not exist yet
            let types = self.bodies.capture_types.get(&ctx.key.symbol);
            let captures: Vec<(String, Type)> = self
                .bodies
                .captures
                .get(&id)
                .map(|names| {
                    names
                        .iter()
                        .map(|name| {
                            let ty = types
                                .and_then(|types| types.get(name))
                                .cloned()
                                .unwrap_or(Type::Error);
                            (name.clone(), self.substitute(&ty, &ctx.key.bindings))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let parameters = signature
                .parameters
                .iter()
                .map(|parameter| self.substitute(&parameter.parameter_type, &ctx.key.bindings))
                .collect();
            let returns = self.substitute(&signature.return_type, &ctx.key.bindings);
            self.local_functions.insert(
                key,
                LocalFunctionInfo {
                    node,
                    captures,
                    has_this: !is_static && ctx.this_slot.is_some(),
                    parameters,
                    returns,
                },
            );
        }
    }

    /// A local function's parameters: one box per captured variable, then
    /// the ones it declares.
    pub(super) fn local_function_shape(&self, key: &FunctionKey) -> (Vec<Type>, Type) {
        let Some(info) = self.local_functions.get(key) else {
            return (Vec::new(), Type::Void);
        };
        let mut parameters: Vec<Type> = info
            .captures
            .iter()
            .map(|_| self.object_array_type())
            .collect();
        parameters.extend(info.parameters.iter().cloned());
        (parameters, info.returns.clone())
    }

    /// The boxes a call hands a local function, `this` first when it has
    /// one: what the enclosing body holds for every name it captures.
    pub(super) fn local_function_payload(
        &mut self,
        ctx: &mut Ctx<'ast>,
        key: &FunctionKey,
        span: Range<usize>,
    ) -> Option<Vec<DataId>> {
        let Some(info) = self.local_functions.get(key) else {
            self.error(ctx, "internal: this local function has no function", span);
            return None;
        };
        let has_this = info.has_this;
        let function = info.node.name.value.to_string();
        let names: Vec<String> = info.captures.iter().map(|(name, _)| name.clone()).collect();
        let mut payload: Vec<DataId> = Vec::new();
        if has_this {
            let Some(this) = ctx.this_slot else {
                self.error(
                    ctx,
                    Message::key("codegen.function_uses_the_object_the_enclosing_method")
                        .arg("function", function),
                    span,
                );
                return None;
            };
            payload.push(this);
        }
        for name in names {
            let found = ctx
                .locals
                .iter()
                .rev()
                .find_map(|scope| scope.get(name.as_str()))
                .filter(|local| local.boxed)
                .map(|local| local.slot);
            let Some(slot) = found else {
                self.error(
                    ctx,
                    Message::key("codegen.function_uses_name_of_the_enclosing_method")
                        .arg("function", function)
                        .arg("name", name),
                    span,
                );
                return None;
            };
            payload.push(slot);
        }
        Some(payload)
    }

    /// The body of a local function: bind the boxes and the parameters,
    /// then its own body.
    pub(super) fn emit_local_function_body(&mut self, ctx: &mut Ctx<'ast>, key: &FunctionKey) {
        let (node, captures, has_this, parameters) = {
            let info = &self.local_functions[key];
            (
                info.node,
                info.captures.clone(),
                info.has_this,
                info.parameters.clone(),
            )
        };
        let slots = self.functions[key].parameters.clone();
        let mut next = usize::from(has_this);
        for (name, ty) in captures {
            ctx.locals[0].insert(
                name,
                Local {
                    slot: slots[next],
                    ty,
                    boxed: true,
                },
            );
            next += 1;
        }
        self.bind_parameters(
            ctx,
            node.parameters.as_ref().ok().map(|list| list.parameters),
            &slots[next..],
            &parameters,
        );
        let is_async = Self::has_async_modifier(node.modifiers);
        let is_iterator = self.bodies.iterators.contains(&EntityID::from(node));
        let returns = self.local_functions[key].returns.clone();
        if is_async {
            self.begin_async(ctx, &returns, node.name.span.clone());
        } else if is_iterator {
            self.begin_iterator(ctx, &returns, node.name.span.clone());
        }
        self.emit_function_body(ctx, &node.body);
        if is_async {
            self.end_async(ctx, node.name.span.clone());
        } else if is_iterator {
            self.end_iterator(ctx, node.name.span.clone());
        }
    }

    // ----------------------------------------------------- method groups

    /// `Func<int, int> f = Twice;` — a delegate to a method of the
    /// compilation's own, with the receiver as its payload.
    pub(super) fn method_group_delegate(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: Option<(DataId, Type)>,
        non_virtual: bool,
        delegate_type: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let Some(shape) = self.delegate_shape(delegate_type, &[]) else {
            self.error(
                ctx,
                Message::key("codegen.a_method_can_only_be_used_as"),
                span,
            );
            return None;
        };
        // `Action a = Local;` — the payload is what a call would have
        // passed: the `this` it was written under, then its boxes
        if let MemberOrigin::LocalFunction(id) = call.origin {
            let key = self.local_function_key(ctx, id);
            let payload = self.local_function_payload(ctx, &key, span.clone())?;
            self.ensure_function(&key);
            let (_, index) = self.ensure_invoker(&shape);
            let thunk = self.ensure_thunk(&key, index, payload.len());
            return Some(self.make_delegate(ctx, thunk, index, &payload, span));
        }
        let MemberOrigin::Source(symbol) = call.origin else {
            self.error(
                ctx,
                Message::key("codegen.an_engine_method_cannot_become_a_delegate"),
                span,
            );
            return None;
        };
        if let Some((_, receiver_type)) = &receiver
            && self.is_program_reference(receiver_type)
        {
            self.error(
                ctx,
                Message::key("codegen.a_method_of_another_behaviour_cannot_become"),
                span,
            );
            return None;
        }
        let (this, key) =
            self.source_call_target(ctx, call, symbol, &receiver, non_virtual, span.clone())?;
        self.ensure_function(&key);
        if self.function_has_this(&key) != this.is_some() {
            let member = self.declarations.table.symbol(symbol).name;
            self.error(
                ctx,
                Message::key("codegen.member_needs_an_instance_to_become_a").arg("member", member),
                span,
            );
            return None;
        }
        let payload: Vec<DataId> = this.into_iter().collect();
        let (_, index) = self.ensure_invoker(&shape);
        let thunk = self.ensure_thunk(&key, index, payload.len());
        Some(self.make_delegate(ctx, thunk, index, &payload, span))
    }

    // ------------------------------------------------------------- calls

    /// `f(1)` / `f.Invoke(1)`: a call of the delegate's shape's invoker
    /// with the delegate as its first argument.
    pub(super) fn invoke_delegate(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: Option<(DataId, Type)>,
        values: Vec<DataId>,
        source_by_ref: Vec<(usize, Place)>,
        span: Range<usize>,
    ) -> Piece {
        let Some((delegate, delegate_type)) = receiver else {
            self.error(
                ctx,
                "internal: a delegate call without a delegate value",
                span,
            );
            return Piece::Error;
        };
        // the shape from the delegate's type, exactly as its values were
        // made; the call's own signature only when the type gives none
        let shape = match self.delegate_shape(&delegate_type, &ctx.key.bindings) {
            Some(shape) => shape,
            None => {
                let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
                Self::shape_of_signature(&signature)
            }
        };
        let returns = shape.returns.clone();
        let (invoker, _) = self.ensure_invoker(&shape);
        let mut arguments = vec![delegate];
        arguments.extend(values);
        let by_ref: Vec<(usize, Place)> = source_by_ref
            .into_iter()
            .map(|(index, place)| (index + 1, place))
            .collect();
        match self.call_function(ctx, &invoker, None, &arguments, &by_ref, span) {
            Some(result) => Piece::Value(result, returns),
            None if returns == Type::Void => Piece::Void,
            None => Piece::Error,
        }
    }

    // ------------------------------------------------------------- boxes

    /// Declares a local or parameter in the current scope: in a box when a
    /// lambda of this body captures it, in its own slot otherwise.
    pub(super) fn bind_local(
        &mut self,
        ctx: &mut Ctx<'ast>,
        name: &'ast str,
        slot: DataId,
        ty: Type,
    ) {
        let boxed = ctx.boxed.iter().any(|captured| captured == name);
        let local = if boxed {
            let cell = self.new_box(ctx, slot);
            Local {
                slot: cell,
                ty,
                boxed: true,
            }
        } else {
            Local {
                slot,
                ty,
                boxed: false,
            }
        };
        ctx.locals
            .last_mut()
            .expect("a scope is open")
            .insert(name.to_string(), local);
    }

    /// A fresh one-element `object[]` holding `value`.
    fn new_box(&mut self, ctx: &Ctx<'ast>, value: DataId) -> DataId {
        let one = self.int_constant(1);
        let cell = self.temp("SystemObjectArray");
        self.call_extern(ctx, NEW_OBJECT_ARRAY, &[one, cell], 0..0);
        let zero = self.int_constant(0);
        self.set_element(ctx, cell, zero, value, 0..0);
        cell
    }

    /// The assignable place of a local: its slot, or element 0 of its box.
    pub(super) fn local_place(&mut self, local: &Local) -> Place {
        if local.boxed {
            Place::Field {
                object: local.slot,
                index: self.int_constant(0),
                ty: local.ty.clone(),
            }
        } else {
            Place::Slot(local.slot, local.ty.clone())
        }
    }

    /// The current value of a local, read out of its box when it has one.
    pub(super) fn read_local(
        &mut self,
        ctx: &Ctx<'ast>,
        local: &Local,
        span: Range<usize>,
    ) -> (DataId, Type) {
        if local.boxed {
            let zero = self.int_constant(0);
            let value = self.get_element(ctx, local.slot, zero, &local.ty, span);
            (value, local.ty.clone())
        } else {
            (local.slot, local.ty.clone())
        }
    }
}

// ------------------------------------------------------------- multicast

const LENGTH: &str = "SystemObjectArray.__get_Length__SystemInt32";
const LESS_THAN: &str = "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean";
const ADD: &str = "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32";
const NOT: &str = "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean";

impl<'a, 'ast> Generator<'a, 'ast> {
    /// A field-like event (`public event Action Name;`), as opposed to one
    /// with `add`/`remove` accessors.
    pub(super) fn is_field_like_event(&self, symbol: SymbolId) -> bool {
        self.declarations
            .table
            .symbol(symbol)
            .declarations
            .first()
            .is_some_and(|site| match site.syntax {
                SyntaxRef::Event { event, .. } => event.accessors.is_none(),
                _ => false,
            })
    }

    /// `a + b`, `a - b`, `a == b`, `a != b` on delegates: `Delegate.Combine`,
    /// `Remove` and `Equals`, as the corlib's helpers over the delegate
    /// representation, handed the shape's multicast address.
    pub(super) fn delegate_operator(
        &mut self,
        ctx: &mut Ctx<'ast>,
        operator: BinaryOperator,
        left: (DataId, &Type),
        right: (DataId, &Type),
        span: Range<usize>,
    ) -> Option<DataId> {
        let delegate_type = if self.is_delegate_type(left.1) {
            left.1.clone()
        } else {
            right.1.clone()
        };
        let Some(shape) = self.delegate_shape(&delegate_type, &ctx.key.bindings) else {
            self.error(
                ctx,
                "internal: the delegate type of this operator has no shape",
                span,
            );
            return None;
        };
        let helper = match operator {
            BinaryOperator::Add => "Combine",
            BinaryOperator::Subtract => "Remove",
            BinaryOperator::Equal | BinaryOperator::NotEqual => "AreEqual",
            _ => {
                self.error(
                    ctx,
                    Message::key("codegen.this_operator_is_not_defined_on_delegates"),
                    span,
                );
                return None;
            }
        };
        let multicast = self.multicast_address(&shape);
        let arguments: Vec<DataId> = if helper == "AreEqual" {
            vec![left.0, right.0, multicast]
        } else {
            // a multicast delegate made here is this program's, of the
            // shape's remote entry, like any delegate made here
            let (_, index) = self.ensure_invoker(&shape);
            let owner = self.self_behaviour_slot();
            let remote = self.remote_entry_address(index);
            vec![left.0, right.0, multicast, owner, remote]
        };
        let result = self.call_delegates_helper(ctx, helper, &arguments, span.clone())?;
        if operator == BinaryOperator::NotEqual {
            let negated = self.temp("SystemBoolean");
            self.call_extern(ctx, NOT, &[result, negated], span);
            return Some(negated);
        }
        Some(result)
    }

    fn call_delegates_helper(
        &mut self,
        ctx: &mut Ctx<'ast>,
        name: &str,
        arguments: &[DataId],
        span: Range<usize>,
    ) -> Option<DataId> {
        let helper = self
            .find_symbol(&["MenSharp", "Internal", "Delegates"])
            .and_then(|class| {
                self.declarations
                    .table
                    .symbol(class)
                    .members_named(name)
                    .first()
                    .copied()
            });
        let Some(symbol) = helper else {
            self.error(
                ctx,
                format!(
                    "internal: the corlib helper `MenSharp.Internal.Delegates.{name}` is missing"
                ),
                span,
            );
            return None;
        };
        let key = FunctionKey {
            symbol,
            role: Role::Method,
            bindings: Vec::new(),
        };
        self.call_function(ctx, &key, None, arguments, &[], span)
    }

    /// The code address of a shape's multicast thunk, as the constant a
    /// multicast delegate of that shape carries in element 0.
    fn multicast_address(&mut self, shape: &DelegateShape) -> DataId {
        let (_, index) = self.ensure_invoker(shape);
        let label = match self.multicast_thunks.get(&index) {
            Some(label) => *label,
            None => {
                let label = self.program.add_label(format!("thunk_multicast_{index}"));
                self.multicast_thunks.insert(index, label);
                self.thunk_queue.push_back(Thunk {
                    label,
                    target: self.invokers[shape].clone(),
                    shape: index,
                    kind: ThunkKind::Multicast,
                });
                label
            }
        };
        self.thunk_address(label)
    }

    fn thunk_address(&mut self, thunk: LabelId) -> DataId {
        if let Some(address) = self.thunk_addresses.get(&thunk) {
            return *address;
        }
        let address =
            self.code_address_constant(format!("__delegate_target_{}", thunk.0), Some(thunk));
        self.thunk_addresses.insert(thunk, address);
        address
    }

    /// The multicast thunk of a shape: calls the invoker once per delegate
    /// in the list (element 1), the same arguments each time, and returns
    /// with the last one's result — stopping at the first exception. The
    /// loop's temps belong to the invoker's frame, and the inner call is
    /// the invoker calling itself, so the ordinary frame save around it
    /// keeps the loop state of the outer activation.
    fn emit_multicast_thunk(&mut self, label: LabelId, shape_index: u32) {
        let shape = self.delegate_shapes[shape_index as usize].clone();
        let invoker = self.invokers[&shape].clone();
        let saved_frame = self.enter_frame(Some(invoker.clone()));
        let mut ctx = self.dispatcher_ctx(&invoker);
        let (invoker_label, invoker_parameters, invoker_return) = {
            let function = &self.functions[&invoker];
            (
                function.label,
                function.parameters.clone(),
                function.return_slot,
            )
        };
        let pending = self.exception_state().pending;

        self.program.code.push(Op::Label(label));
        let delegate = invoker_parameters[0];
        let list = self.temp("SystemObjectArray");
        let list_index = self.int_constant(DELEGATE_PAYLOAD);
        self.call_extern(&ctx, GET, &[delegate, list_index, list], 0..0);
        let one = self.int_constant(1);
        let count = self.temp("SystemInt32");
        self.call_extern(&ctx, LENGTH, &[list, count], 0..0);
        let index = self.temp("SystemInt32");
        let zero = self.int_constant(0);
        self.copy(zero, index);

        let head = self.fresh_label("multicast_head");
        let next = self.fresh_label("multicast_next");
        let done = self.fresh_label("multicast_done");
        self.program.code.push(Op::Label(head));
        let more = self.temp("SystemBoolean");
        self.call_extern(&ctx, LESS_THAN, &[index, count, more], 0..0);
        self.program.code.push(Op::Push(more));
        self.program.code.push(Op::JumpIfFalse(Target::Label(done)));
        let inner = self.temp("SystemObjectArray");
        self.call_extern(&ctx, GET, &[list, index, inner], 0..0);

        // the invoker calling itself with the inner delegate: the frame
        // save keeps this activation's delegate, arguments and loop state
        let marker = self.frame_markers.len() as u32;
        self.frame_markers.push((invoker.clone(), invoker.clone()));
        self.call_edges
            .entry(invoker.clone())
            .or_default()
            .insert(invoker.clone());
        self.program.code.push(Op::SaveFrame(marker));
        self.copy(inner, delegate);
        let continuation = self
            .program
            .add_label(format!("multicast_ret_{}", self.temp_counter));
        self.temp_counter += 1;
        let address = self.code_address_constant(
            format!("__multicastaddr_{}", self.temp_counter),
            Some(continuation),
        );
        self.copy(address, invoker_return);
        self.program
            .code
            .push(Op::Jump(Target::Label(invoker_label)));
        self.program.code.push(Op::Label(continuation));
        self.program.code.push(Op::RestoreFrame(marker));

        // an exception in one target ends the whole call; the caller
        // unwinds it
        self.program.code.push(Op::Push(pending));
        self.program.code.push(Op::JumpIfFalse(Target::Label(next)));
        self.program.code.push(Op::Jump(Target::Label(done)));
        self.program.code.push(Op::Label(next));
        self.call_extern(&ctx, ADD, &[index, one, index], 0..0);
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(done));
        self.program.code.push(Op::JumpIndirect(invoker_return));
        let _ = &mut ctx;
        self.leave_frame(saved_frame);
    }
}
