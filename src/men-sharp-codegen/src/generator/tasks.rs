//! `async`/`await`.
//!
//! An M# function keeps its whole state in static heap slots — its frame
//! (`Function::frame`). That makes suspending it cheap to describe: copy
//! the frame into an `object[]`, remember the code address to come back to,
//! and return. Resuming is the reverse: copy the snapshot back into the
//! frame and `JUMP_INDIRECT` to that address. No local is hoisted anywhere;
//! the locals already live in the heap.
//!
//! The *continuation* is an ordinary `Action` delegate whose thunk is a
//! **resume thunk** made for the function (element 1 the snapshot, element
//! 2 the address), so the mini-corlib's `Task` can hold it, queue it and
//! call it like any delegate — `corlib/Tasks.cs` is plain M#. To the frame
//! machinery the thunk is the edge `Action`-invoker → function, saved
//! around like any call, so a function resumed while another activation of
//! it is on the stack keeps that activation intact.
//!
//! An `async` function makes its `Task` (or `Task<T>`) on entry and puts it
//! in its result slot; that is what the caller receives whenever the
//! function first returns. `return x` completes the task with `x`, falling
//! off the end completes it with nothing, and an exception that leaves the
//! body faults it — an implicit `try` around the whole body, whose handler
//! calls `__Fail`. `async void` has no task: it returns nothing and its
//! exceptions unwind to whoever resumed it.
//!
//! `await e` is C#'s awaiter pattern, bound by the checker
//! (`BodyCheck::awaits`): `GetAwaiter()`, `IsCompleted`, and either
//! `GetResult()` straight away or `OnCompleted(continuation)` followed by a
//! return. Continuations are queued, never run inline; every event stub
//! drains the queue after its body (`Scheduler.__Drain`), and the
//! `_mensharpResume` event Udon raises for `SendCustomEventDelayed…` moves
//! timed ones into the queue first.

use men_sharp_parser::ast::{AwaitExpression, Modifier, YieldKind, YieldStatement};

use super::delegates::{DelegateShape, Thunk, ThunkKind};
use super::*;

/// The custom event a program raises on itself for timed continuations.
pub(super) const RESUME_EVENT: &str = "_mensharpResume";
const TASKS_PATH: [&str; 3] = ["System", "Threading", "Tasks"];
const SCHEDULER_PATH: [&str; 2] = ["MenSharp", "Scheduler"];
const ITERATOR_PATH: [&str; 3] = ["MenSharp", "Internal", "Iterator"];
const STEPPING_PATH: [&str; 4] = ["MenSharp", "Internal", "Iterators", "Stepping"];
const GET: &str = "SystemObjectArray.__Get__SystemInt32__SystemObject";
/// The exported variable a remote continuation is handed over in.
pub(super) const INCOMING_RESUME: &str = "__mensharp_resume";

/// The state of an iterator body (`yield return`) while it is being
/// compiled: the `Iterator<T>` it made on entry, its type and element type.
#[derive(Clone)]
pub(super) struct IteratorCtx {
    object: DataId,
    ty: Type,
    element: Type,
}

/// The state of an `async` body while it is being compiled.
#[derive(Clone)]
pub(super) struct AsyncCtx {
    /// The task the function returns, made on entry — `None` for `async void`.
    task: Option<(DataId, Type)>,
    /// What `return` completes the task with: `Void` for `Task` and `void`.
    pub(super) inner: Type,
    /// The implicit handler that faults the task, when there is one.
    fail: Option<LabelId>,
}

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------- types

    fn task_symbol(&self, arity: usize) -> Option<SymbolId> {
        let namespace = self.find_symbol(&TASKS_PATH)?;
        self.declarations
            .table
            .symbol(namespace)
            .members_named("Task")
            .iter()
            .copied()
            .find(|symbol| {
                self.declarations
                    .table
                    .symbol(*symbol)
                    .type_parameters
                    .len()
                    == arity
            })
    }

    /// The mini-corlib's `Task` or `Task<T>`.
    pub(super) fn is_task_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                arguments,
            } => self.task_symbol(arguments.len()) == Some(*symbol),
            Type::Nullable(inner) => self.is_task_type(inner),
            _ => false,
        }
    }

    /// What an `async` body's `return` hands its task: `T` of `Task<T>`,
    /// nothing otherwise.
    fn async_inner(&self, declared: &Type) -> Type {
        match declared {
            Type::Named { arguments, .. } if self.is_task_type(declared) => {
                arguments.first().cloned().unwrap_or(Type::Void)
            }
            _ => Type::Void,
        }
    }

    pub(super) fn has_async_modifier(
        modifiers: &[men_sharp_parser::ast::Spanned<Modifier>],
    ) -> bool {
        modifiers
            .iter()
            .any(|modifier| modifier.value == Modifier::Async)
    }

    // ------------------------------------------------------------ bodies

    /// Opens an `async` body: makes its task, puts it in the result slot,
    /// and opens the implicit `try` that faults the task on an exception.
    pub(super) fn begin_async(&mut self, ctx: &mut Ctx<'ast>, declared: &Type, span: Range<usize>) {
        let declared = self.substitute(declared, &ctx.key.bindings);
        let inner = self.async_inner(&declared);
        let task = if self.is_task_type(&declared) {
            match self.new_corlib_object(ctx, &declared, span) {
                Some(object) => {
                    if let Some(result) = ctx.result {
                        self.copy(object, result);
                    }
                    Some((object, declared.clone()))
                }
                None => None,
            }
        } else {
            None
        };
        let fail = task.is_some().then(|| {
            let label = self.fresh_label("async_fail");
            ctx.loop_stack.push(BreakFrame::Try {
                handler: label,
                finally: None,
            });
            label
        });
        ctx.async_state = Some(AsyncCtx { task, inner, fail });
    }

    /// Closes an `async` body: falling off the end completes the task, and
    /// the implicit handler faults it with whatever exception got out.
    pub(super) fn end_async(&mut self, ctx: &mut Ctx<'ast>, span: Range<usize>) {
        let Some(state) = ctx.async_state.clone() else {
            return;
        };
        let Some((task, task_type)) = state.task.clone() else {
            ctx.async_state = None;
            return;
        };
        ctx.loop_stack.pop();
        self.complete_async(ctx, None, span.clone());
        self.program.code.push(Op::JumpIndirect(ctx.return_slot));

        if let Some(fail) = state.fail {
            self.program.code.push(Op::Label(fail));
            let exception = self.exception_state();
            let saved = self.temp("SystemObject");
            self.copy(exception.exception, saved);
            let cleared = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
            self.copy(cleared, exception.pending);
            self.call_corlib_member(ctx, Some((task, task_type)), "__Fail", &[saved], span);
        }
        ctx.async_state = None;
    }

    /// `return value;` (or the end of the body) in an `async` body:
    /// completes the task, with the value when it carries one.
    pub(super) fn complete_async(
        &mut self,
        ctx: &mut Ctx<'ast>,
        value: Option<DataId>,
        span: Range<usize>,
    ) {
        let Some(state) = ctx.async_state.clone() else {
            return;
        };
        let Some((task, task_type)) = state.task else {
            return;
        };
        match value {
            Some(value) if state.inner != Type::Void => {
                self.call_corlib_member(
                    ctx,
                    Some((task, task_type)),
                    "__SetResult",
                    &[value],
                    span,
                );
            }
            _ => {
                self.call_corlib_member(ctx, Some((task, task_type)), "__Complete", &[], span);
            }
        }
    }

    // ------------------------------------------------------------- await

    pub(super) fn lower_await(
        &mut self,
        ctx: &mut Ctx<'ast>,
        node: &'ast AwaitExpression<'ast, 'ast>,
    ) -> Option<DataId> {
        let span = node.span.clone();
        let Ok(value) = &node.value else {
            return None;
        };
        let Some(resolved) = self.bodies.awaits.get(&EntityID::from(node)).cloned() else {
            self.error(ctx, "internal: this `await` was not resolved", span);
            return None;
        };
        if ctx.async_state.is_none() {
            self.error(ctx, "internal: `await` outside an async body", span);
            return None;
        }
        let awaitable = self.lower_expression(ctx, value)?;
        let awaitable_type = self.type_of(ctx, value);

        let awaiter = self.call_resolved(
            ctx,
            &resolved.get_awaiter,
            (awaitable, awaitable_type),
            &[],
            span.clone(),
        )?;
        let awaiter_type = self.substitute(&resolved.awaiter_type, &ctx.key.bindings);
        let completed = self.read_resolved_property(
            ctx,
            &resolved.is_completed,
            (awaiter, awaiter_type.clone()),
            span.clone(),
        )?;

        let suspend = self.fresh_label("await_suspend");
        let resume = self.fresh_label("await_resume");
        let done = self.fresh_label("await_done");
        self.program.code.push(Op::Push(completed));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(suspend)));
        self.program.code.push(Op::Jump(Target::Label(done)));

        // not done yet: hand the awaiter a continuation and return
        self.program.code.push(Op::Label(suspend));
        let continuation = self.continuation_to(ctx, resume, span.clone());
        self.call_resolved(
            ctx,
            &resolved.on_completed,
            (awaiter, awaiter_type.clone()),
            &[continuation],
            span.clone(),
        );
        self.program.code.push(Op::JumpIndirect(ctx.return_slot));

        // the resume thunk lands here with the frame restored
        self.program.code.push(Op::Label(resume));
        self.program.code.push(Op::Label(done));
        self.call_resolved(
            ctx,
            &resolved.get_result,
            (awaiter, awaiter_type),
            &[],
            span,
        )
    }

    /// The continuation that resumes this function at `resume`: the frame
    /// copied into a snapshot (whose contents are decided once the whole
    /// body is compiled, see resolve_frame_markers) and the address, in an
    /// `Action` delegate to the function's resume thunk.
    fn continuation_to(
        &mut self,
        ctx: &mut Ctx<'ast>,
        resume: LabelId,
        span: Range<usize>,
    ) -> DataId {
        let snapshot = self.scratch_slot("SystemObjectArray");
        let marker = self.async_snapshots.len() as u32;
        self.async_snapshots.push((ctx.key.clone(), snapshot));
        self.program.code.push(Op::SnapshotFrame(marker));
        let address =
            self.code_address_constant(format!("__resume_{}", self.temp_counter), Some(resume));
        self.temp_counter += 1;
        let thunk = self.resume_thunk(&ctx.key);
        self.make_delegate(ctx, thunk, &[snapshot, address], span)
    }

    // ---------------------------------------------------------- iterators

    /// `T` of the `IEnumerable<T>`/`IEnumerator<T>` an iterator is declared
    /// to return.
    fn iterator_element_of(&self, declared: &Type) -> Type {
        match declared {
            Type::Named { arguments, .. } if arguments.len() == 1 => arguments[0].clone(),
            _ => self.corlib_type("Object"),
        }
    }

    /// Opens an iterator body: makes its `Iterator<T>`, hands it the
    /// continuation that starts the body, and returns it — the body itself
    /// runs from the first `MoveNext` on.
    pub(super) fn begin_iterator(
        &mut self,
        ctx: &mut Ctx<'ast>,
        declared: &Type,
        span: Range<usize>,
    ) {
        let declared = self.substitute(declared, &ctx.key.bindings);
        let element = self.iterator_element_of(&declared);
        let Some(class) = self.find_symbol(&ITERATOR_PATH) else {
            self.error(
                ctx,
                "internal: the mini-corlib's Iterator<T> is missing",
                span,
            );
            return;
        };
        let ty = Type::Named {
            target: TypeTarget::Source(class),
            arguments: vec![element.clone()],
        };
        let Some(object) = self.new_corlib_object(ctx, &ty, span.clone()) else {
            return;
        };
        if let Some(result) = ctx.result {
            self.copy(object, result);
        }
        let body = self.fresh_label("iterator_body");
        let entry = self.continuation_to(ctx, body, span.clone());
        self.call_corlib_member(ctx, Some((object, ty.clone())), "__Start", &[entry], span);
        self.program.code.push(Op::JumpIndirect(ctx.return_slot));

        self.program.code.push(Op::Label(body));
        self.reload_stepping(ctx, object);
        ctx.iterator_state = Some(IteratorCtx {
            object,
            ty,
            element,
        });
    }

    /// Closes an iterator body: falling off the end finishes it.
    pub(super) fn end_iterator(&mut self, ctx: &mut Ctx<'ast>, span: Range<usize>) {
        let Some(state) = ctx.iterator_state.take() else {
            return;
        };
        self.call_corlib_member(ctx, Some((state.object, state.ty)), "__Finish", &[], span);
    }

    /// `yield return value;` / `yield break;`.
    pub(super) fn lower_yield(
        &mut self,
        ctx: &mut Ctx<'ast>,
        statement: &'ast YieldStatement<'ast, 'ast>,
    ) {
        let span = statement.span.clone();
        let Some(state) = ctx.iterator_state.clone() else {
            self.error(ctx, "internal: `yield` outside an iterator body", span);
            return;
        };
        match (statement.kind.value, &statement.value) {
            (YieldKind::Return, Some(value)) => {
                let Some(value) = self.owned_value_as(ctx, value, &state.element) else {
                    return;
                };
                let resume = self.fresh_label("yield_resume");
                let continuation = self.continuation_to(ctx, resume, span.clone());
                self.call_corlib_member(
                    ctx,
                    Some((state.object, state.ty.clone())),
                    "__Yield",
                    &[value, continuation],
                    span.clone(),
                );
                self.program.code.push(Op::JumpIndirect(ctx.return_slot));
                self.program.code.push(Op::Label(resume));
                self.reload_stepping(ctx, state.object);
                self.emit_disposing_exit(ctx, &state, span);
            }
            (YieldKind::Break, _) => {
                self.emit_finally_copies(ctx, 0);
                self.call_corlib_member(ctx, Some((state.object, state.ty)), "__Finish", &[], span);
                self.program.code.push(Op::JumpIndirect(ctx.return_slot));
            }
            (YieldKind::Return, None) => {}
        }
    }

    /// Right after a resume: `Dispose` resumes a suspended body only to
    /// unwind it, so the `finally` blocks this `yield` sits inside run and
    /// the iteration ends there instead of carrying on.
    fn emit_disposing_exit(
        &mut self,
        ctx: &mut Ctx<'ast>,
        state: &IteratorCtx,
        span: Range<usize>,
    ) {
        let Some(field) = self
            .find_symbol(&STEPPING_PATH[..3])
            .and_then(|_| self.find_symbol(&["MenSharp", "Internal", "Iterators", "Disposing"]))
        else {
            self.error(
                ctx,
                "internal: the mini-corlib's Iterators.Disposing is missing",
                span,
            );
            return;
        };
        let disposing = self.ensure_static(field, false);
        let carry_on = self.fresh_label("yield_continue");
        self.program.code.push(Op::Push(disposing));
        self.program
            .code
            .push(Op::JumpIfFalse(Target::Label(carry_on)));
        self.emit_finally_copies(ctx, 0);
        self.call_corlib_member(
            ctx,
            Some((state.object, state.ty.clone())),
            "__Finish",
            &[],
            span,
        );
        self.program.code.push(Op::JumpIndirect(ctx.return_slot));
        self.program.code.push(Op::Label(carry_on));
    }

    /// After a resume: the iterator driving this step may be a copy made by
    /// a second enumeration — take it from where `MoveNext` left it.
    fn reload_stepping(&mut self, ctx: &mut Ctx<'ast>, object: DataId) {
        let Some(field) = self.find_symbol(&STEPPING_PATH) else {
            self.error(
                ctx,
                "internal: the mini-corlib's Iterators.Stepping is missing",
                0..0,
            );
            return;
        };
        let slot = self.ensure_static(field, false);
        self.copy(slot, object);
    }

    /// Calls a resolved instance method on `receiver`.
    fn call_resolved(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: (DataId, Type),
        arguments: &[DataId],
        span: Range<usize>,
    ) -> Option<DataId> {
        let MemberOrigin::Source(symbol) = call.origin else {
            self.error(
                ctx,
                Message::key("codegen.an_awaiter_from_the_engine_cannot_be"),
                span,
            );
            return None;
        };
        let receiver = Some(receiver);
        let (this, key) =
            self.source_call_target(ctx, call, symbol, &receiver, false, span.clone())?;
        self.call_function(ctx, &key, this, arguments, &[], span)
    }

    /// Reads a resolved instance property of `receiver`.
    fn read_resolved_property(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: &ResolvedMember,
        receiver: (DataId, Type),
        span: Range<usize>,
    ) -> Option<DataId> {
        let MemberOrigin::Source(symbol) = member.origin else {
            self.error(ctx, "internal: an awaiter property from the engine", span);
            return None;
        };
        let bindings = self.bindings_for(ctx, symbol, &member.declaring_type, &[]);
        let ty = self.substitute(&member.member_type, &ctx.key.bindings);
        let place = Place::Accessor {
            receiver: Some(receiver.0),
            symbol,
            bindings,
            indices: Vec::new(),
            ty,
        };
        self.read_place(ctx, place, span).map(|(slot, _)| slot)
    }

    /// Calls a method of the mini-corlib by name on `receiver` (its class
    /// or any base), or a static one of `class` when there is no receiver.
    fn call_corlib_member(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: Option<(DataId, Type)>,
        name: &str,
        arguments: &[DataId],
        span: Range<usize>,
    ) -> Option<DataId> {
        let Some((_, receiver_type)) = &receiver else {
            self.error(ctx, format!("internal: `{name}` needs a receiver"), span);
            return None;
        };
        let receiver_type = self.substitute(receiver_type, &ctx.key.bindings);
        let found = self
            .type_system()
            .members_named(&receiver_type, name)
            .into_iter()
            .find_map(
                |candidate| match (&candidate.origin, &candidate.signature) {
                    (MemberOrigin::Source(symbol), Some(MemberSignature::Function(signature)))
                        if signature.parameters.len() == arguments.len() =>
                    {
                        Some((*symbol, candidate.declaring_type.clone()))
                    }
                    _ => None,
                },
            );
        let Some((symbol, declaring_type)) = found else {
            self.error(
                ctx,
                format!(
                    "internal: the mini-corlib's `{}` has no `{name}`",
                    self.describe_type(&receiver_type)
                ),
                span,
            );
            return None;
        };
        let bindings = self.bindings_for(ctx, symbol, &declaring_type, &[]);
        let key = FunctionKey {
            symbol,
            role: Role::Method,
            bindings,
        };
        let this = receiver.map(|(slot, _)| slot);
        self.call_function(ctx, &key, this, arguments, &[], span)
    }

    /// `new T()` for a mini-corlib class `ty`: allocated, constructor run.
    fn new_corlib_object(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        self.new_corlib_object_with(ctx, ty, &[], span)
    }

    /// ... with constructor arguments.
    pub(super) fn new_corlib_object_with(
        &mut self,
        ctx: &mut Ctx<'ast>,
        ty: &Type,
        arguments: &[DataId],
        span: Range<usize>,
    ) -> Option<DataId> {
        let Type::Named {
            target: TypeTarget::Source(class),
            ..
        } = ty
        else {
            return None;
        };
        let object = self.allocate_object(ctx, ty, span.clone())?;
        let constructor = self
            .declarations
            .table
            .symbol(*class)
            .members_named(".ctor")
            .iter()
            .copied()
            .find(|&member| {
                matches!(
                    self.signatures.members.get(&member),
                    Some(MemberSignature::Function(function))
                        if function.parameters.len() == arguments.len()
                )
            });
        let Some(constructor) = constructor else {
            self.error(
                ctx,
                format!(
                    "internal: `{}` has no constructor taking {} argument(s)",
                    self.describe_type(ty),
                    arguments.len()
                ),
                span,
            );
            return None;
        };
        let bindings = self.bindings_for(ctx, constructor, ty, &[]);
        let key = FunctionKey {
            symbol: constructor,
            role: Role::Constructor,
            bindings,
        };
        self.call_function(ctx, &key, Some(object), arguments, &[], span);
        Some(object)
    }

    /// `T[]` where `IEnumerable<T>` is wanted: the array in an
    /// `ArrayEnumerable<T>`, which is what makes it a sequence at run time.
    /// `None` when this is not that conversion.
    pub(super) fn sequence_of_array(
        &mut self,
        ctx: &mut Ctx<'ast>,
        source: DataId,
        from: &Type,
        to: &Type,
        span: &Range<usize>,
    ) -> Option<DataId> {
        if !self
            .type_system()
            .is_source_type_path(to, &["System", "Collections", "Generic", "IEnumerable"])
        {
            return None;
        }
        // a string enumerates as its characters, which on Udon means the
        // array `ToCharArray` hands back
        let (items, element) = if self.type_system().is_string(from) {
            let characters = self.temp("SystemCharArray");
            self.call_extern(
                ctx,
                "SystemString.__ToCharArray__SystemCharArray",
                &[source, characters],
                span.clone(),
            );
            (characters, self.corlib_type("Char"))
        } else if let Type::Array { element, rank: 1 } = from {
            (source, (**element).clone())
        } else {
            return None;
        };
        let class = self.find_symbol(&["MenSharp", "Internal", "ArrayEnumerable"])?;
        let ty = Type::Named {
            target: TypeTarget::Source(class),
            arguments: vec![element],
        };
        self.new_corlib_object_with(ctx, &ty, &[items], span.clone())
    }

    // ------------------------------------------------------ resume thunks

    fn action_shape() -> DelegateShape {
        DelegateShape {
            parameters: Vec::new(),
            returns: Type::Void,
        }
    }

    /// The thunk that resumes `target` from a continuation delegate:
    /// registered once per function, emitted with the other thunks.
    fn resume_thunk(&mut self, target: &FunctionKey) -> LabelId {
        if let Some(label) = self.resume_thunks.get(target) {
            return *label;
        }
        let (_, shape) = self.ensure_invoker(&Self::action_shape());
        let label = self
            .program
            .add_label(format!("resume_{}", self.functions[target].name));
        self.resume_thunks.insert(target.clone(), label);
        self.thunk_queue.push_back(Thunk {
            label,
            target: target.clone(),
            shape,
            kind: ThunkKind::Resume,
        });
        label
    }

    /// The body of a resume thunk: take the snapshot and the address out of
    /// the delegate, put the snapshot back into the target's frame, point
    /// its return at the thunk's tail and jump to the address. The target
    /// comes back here when it suspends again or finishes; the thunk then
    /// returns on the invoker's behalf.
    pub(super) fn emit_resume_thunk(&mut self, label: LabelId, target: &FunctionKey) {
        let shape = Self::action_shape();
        let invoker = self.invokers[&shape].clone();
        let saved_frame = self.enter_frame(Some(invoker.clone()));
        let ctx = self.dispatcher_ctx(&invoker);
        let (invoker_parameters, invoker_return) = {
            let function = &self.functions[&invoker];
            (function.parameters.clone(), function.return_slot)
        };
        let target_return = self.functions[target].return_slot;

        self.program.code.push(Op::Label(label));
        // the invoker → target edge: saved around when the target may be
        // active underneath (see resolve_frame_markers)
        let marker = self.frame_markers.len() as u32;
        self.frame_markers.push((invoker.clone(), target.clone()));
        self.call_edges
            .entry(invoker.clone())
            .or_default()
            .insert(target.clone());
        self.program.code.push(Op::SaveFrame(marker));

        let delegate = invoker_parameters[0];
        let one = self.int_constant(1);
        let two = self.int_constant(2);
        let snapshot = self.temp("SystemObjectArray");
        self.call_extern(&ctx, GET, &[delegate, one, snapshot], 0..0);
        let address = self.temp("SystemUInt32");
        self.call_extern(&ctx, GET, &[delegate, two, address], 0..0);
        let restore = self.async_snapshots.len() as u32;
        self.async_snapshots.push((target.clone(), snapshot));
        self.program.code.push(Op::RestoreSnapshot(restore));

        let continuation = self
            .program
            .add_label(format!("resume_ret_{}", self.temp_counter));
        self.temp_counter += 1;
        let back = self.code_address_constant(
            format!("__resumeaddr_{}", self.temp_counter),
            Some(continuation),
        );
        self.copy(back, target_return);
        self.program.code.push(Op::JumpIndirect(address));
        self.program.code.push(Op::Label(continuation));
        self.program.code.push(Op::RestoreFrame(marker));
        self.program.code.push(Op::JumpIndirect(invoker_return));
        self.leave_frame(saved_frame);
    }

    // ------------------------------------------------------- scheduler

    /// The program's own UdonBehaviour: the receiver of
    /// `SendCustomEventDelayed…` on itself.
    pub(super) fn self_behaviour_slot(&mut self) -> DataId {
        if let Some(slot) = self.self_behaviour {
            return slot;
        }
        let declared = self.marker.and_then(|marker| {
            self.declarations
                .table
                .symbol(marker)
                .members_named("udonBehaviour")
                .first()
                .copied()
        });
        let slot = declared
            .and_then(|member| self.self_reference_slot(member))
            .unwrap_or_else(|| {
                self.program.add_data(DataSymbol {
                    name: "__this_udonBehaviour".into(),
                    udon_type: BEHAVIOUR_HEAP_TYPE.into(),
                    init: HeapInit::SelfReference,
                    export: false,
                    sync: None,
                })
            });
        self.self_behaviour = Some(slot);
        slot
    }

    /// The exported slot a *remote* continuation arrives in: another
    /// program writes it with `SetProgramVariable` and then raises
    /// [`RESUME_EVENT`], which is how a task of ours that another behaviour
    /// was holding gets its continuation back into our own queue.
    pub(super) fn incoming_resume_slot(&mut self) -> DataId {
        if let Some(slot) = self.incoming_resume {
            return slot;
        }
        let slot = self.program.add_data(DataSymbol {
            name: INCOMING_RESUME.into(),
            udon_type: "SystemObjectArray".into(),
            init: HeapInit::Null,
            // not exported, like every other variable one program writes on
            // another (`__0_x__param` and friends): `SetProgramVariable`
            // reaches any symbol, and a public one would also be a public
            // variable, which this is not
            export: false,
            sync: None,
        });
        self.incoming_resume = Some(slot);
        slot
    }

    /// `MenSharp.Scheduler.<name>`, scheduled for compilation.
    pub(super) fn scheduler_key(&mut self, name: &str) -> Option<FunctionKey> {
        let scheduler = self.find_symbol(&SCHEDULER_PATH)?;
        let symbol = self
            .declarations
            .table
            .symbol(scheduler)
            .members_named(name)
            .first()
            .copied()?;
        let key = FunctionKey {
            symbol,
            role: Role::Method,
            bindings: Vec::new(),
        };
        self.ensure_function(&key);
        Some(key)
    }

    /// In an entry stub: calls `MenSharp.Scheduler.<name>()` and reports an
    /// exception it left pending.
    pub(super) fn emit_scheduler_call(&mut self, name: &str, stub: &str) {
        let Some(key) = self.scheduler_key(name) else {
            return;
        };
        let (label, return_slot) = {
            let function = &self.functions[&key];
            (function.label, function.return_slot)
        };
        let done = self.program.add_label(format!("event_{stub}__{name}_done"));
        let back = self.code_address_constant(format!("__ret_{stub}_{name}"), Some(done));
        self.copy(back, return_slot);
        self.program.code.push(Op::Jump(Target::Label(label)));
        self.program.code.push(Op::Label(done));
        self.emit_unhandled_check(&format!("{stub}__{name}"));
    }
}
