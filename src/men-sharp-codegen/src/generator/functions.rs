//! Function instances: scheduling, frames, bodies, calls and virtual dispatch.

use men_sharp_parser::ast::{AccessorKind, Modifier};
use men_sharp_semantics::{FunctionSignature, ParameterPassing};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// The instantiated parameter/return shape for a function instance:
    /// parameter types (setter value last) and the return type.
    pub(super) fn function_shape(&self, key: &FunctionKey) -> (Vec<Type>, Type) {
        let member = self.signatures.members.get(&key.symbol);
        // a dispatch stub has the shape of what it dispatches to
        let role = match key.role {
            Role::GetterDispatcher => Role::Getter,
            Role::SetterDispatcher => Role::Setter,
            other => other,
        };
        let (parameters, return_type) = match (role, member) {
            (
                Role::Method | Role::Constructor | Role::Dispatcher,
                Some(MemberSignature::Function(signature)),
            ) => (
                signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.parameter_type.clone())
                    .collect(),
                signature.return_type.clone(),
            ),
            (Role::Getter, Some(MemberSignature::Property(ty))) => (Vec::new(), ty.clone()),
            (Role::Setter, Some(MemberSignature::Property(ty))) => (vec![ty.clone()], Type::Void),
            // indexers carry a callable signature; the getter takes the index
            // parameters, the setter appends `value`
            (Role::Getter, Some(MemberSignature::Function(signature))) => (
                signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.parameter_type.clone())
                    .collect(),
                signature.return_type.clone(),
            ),
            (Role::Setter, Some(MemberSignature::Function(signature))) => {
                let mut parameters: Vec<Type> = signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.parameter_type.clone())
                    .collect();
                parameters.push(signature.return_type.clone());
                (parameters, Type::Void)
            }
            (Role::DefaultConstructor, _) => (Vec::new(), Type::Void),
            (Role::StructEquals, _) => (
                vec![self.corlib_type("Object")],
                self.corlib_type("Boolean"),
            ),
            (Role::StructHashCode, _) => (Vec::new(), self.corlib_type("Int32")),
            (Role::TypeTest, _) => (
                vec![self.corlib_type("Object")],
                self.corlib_type("Boolean"),
            ),
            (Role::UnhandledException, _) => (Vec::new(), Type::Void),
            // the receiver is an ordinary `object` parameter, not `this`: it
            // may be anything, including a boxed int no `object[]` slot
            // could hold
            (Role::ObjectDispatcher(member), _) => {
                let object = self.corlib_type("Object");
                match member {
                    ObjectMember::Equals => {
                        (vec![object.clone(), object], self.corlib_type("Boolean"))
                    }
                    ObjectMember::GetHashCode => (vec![object], self.corlib_type("Int32")),
                    ObjectMember::ToString => (vec![object], self.corlib_type("String")),
                }
            }
            _ => (Vec::new(), Type::Void),
        };
        let parameters = parameters
            .iter()
            .map(|ty| self.substitute(ty, &key.bindings))
            .collect();
        let return_type = self.substitute(&return_type, &key.bindings);
        (parameters, return_type)
    }

    fn function_has_this(&self, key: &FunctionKey) -> bool {
        // the behaviour entry class has exactly one instance — the program
        // itself — so its members carry no `this` and its fields are globals
        if self.is_entry_member(key.symbol) {
            return false;
        }
        match key.role {
            Role::DefaultConstructor | Role::StructEquals | Role::StructHashCode => true,
            Role::TypeTest | Role::ObjectDispatcher(_) | Role::UnhandledException => false,
            _ => !self.declarations.table.symbol(key.symbol).is_static,
        }
    }

    fn mangle_key(&self, key: &FunctionKey) -> String {
        let mut name = format!("fn_{}", self.symbol_path(key.symbol));
        match key.role {
            Role::Method => {}
            Role::Getter => name.push_str("_get"),
            Role::Setter => name.push_str("_set"),
            Role::Constructor => name.push_str("_ctor"),
            Role::DefaultConstructor => name.push_str("_defaultctor"),
            Role::StructEquals => name.push_str("_equals"),
            Role::StructHashCode => name.push_str("_hashcode"),
            Role::Dispatcher => name.push_str("_dispatch"),
            Role::GetterDispatcher => name.push_str("_get_dispatch"),
            Role::SetterDispatcher => name.push_str("_set_dispatch"),
            Role::TypeTest => name.push_str("_is"),
            Role::UnhandledException => name.push_str("_unhandled_exception"),
            Role::ObjectDispatcher(member) => {
                name.push_str("_object_");
                name.push_str(member.name());
            }
        }
        for (_, ty) in &key.bindings {
            name.push('_');
            name.push_str(&self.heap_type_component(ty));
        }
        // several symbols may share a path (overloads); disambiguate
        format!("{}_{}", name, self.functions.len())
    }

    /// Registers (and queues) a function instance; returns nothing — look it
    /// up in `self.functions`.
    pub(super) fn ensure_function(&mut self, key: &FunctionKey) {
        if self.functions.contains_key(key) {
            return;
        }
        let name = self.mangle_key(key);
        let (parameter_types, return_type) = self.function_shape(key);
        let has_this = self.function_has_this(key);

        let mut parameters = Vec::new();
        if has_this {
            let slot = self.program.add_data(DataSymbol {
                name: format!("{name}__this"),
                udon_type: "SystemObjectArray".into(),
                init: HeapInit::Null,
                export: false,
                sync: None,
            });
            parameters.push(slot);
        }
        for (index, ty) in parameter_types.iter().enumerate() {
            let udon_type = self.heap_type(ty);
            let slot = self.program.add_data(DataSymbol {
                name: format!("{name}__p{index}"),
                udon_type,
                init: HeapInit::Null,
                export: false,
                sync: None,
            });
            parameters.push(slot);
        }
        let result = if return_type == Type::Void {
            None
        } else {
            let udon_type = self.heap_type(&return_type);
            Some(self.program.add_data(DataSymbol {
                name: format!("{name}__result"),
                udon_type,
                init: HeapInit::Null,
                export: false,
                sync: None,
            }))
        };
        let return_slot = self.program.add_data(DataSymbol {
            name: format!("{name}__return"),
            udon_type: "SystemUInt32".into(),
            init: HeapInit::UInt32(0),
            export: false,
            sync: None,
        });
        let label = self.program.add_label(name.clone());

        let mut frame = parameters.clone();
        frame.push(return_slot);
        self.functions.insert(
            key.clone(),
            Function {
                label,
                parameters,
                result,
                return_slot,
                name,
                frame,
            },
        );
        self.queue.push_back(key.clone());
    }

    // ------------------------------------------------------------ compiling

    pub(super) fn compile_function(&mut self, key: &FunctionKey) {
        self.current_frame = Some(key.clone());
        self.compile_function_body(key);
        self.current_frame = None;
    }

    fn compile_function_body(&mut self, key: &FunctionKey) {
        let function = &self.functions[key];
        let label = function.label;
        let result = function.result;
        let return_slot = function.return_slot;
        let parameters = function.parameters.clone();
        let has_this = self.function_has_this(key);
        let (parameter_types, _) = self.function_shape(key);

        let symbol = self.declarations.table.symbol(key.symbol);
        let site = symbol.declarations.first();
        let file = site.map(|site| site.file).unwrap_or(FileId(0));
        let syntax = site.map(|site| site.syntax);

        let mut ctx = Ctx {
            key: key.clone(),
            file,
            locals: vec![HashMap::new()],
            this_slot: has_this.then(|| parameters[0]),
            this_type: has_this.then(|| self.this_type_of(key)),
            loop_stack: Vec::new(),
            result,
            return_slot,
            caught: Vec::new(),
        };

        self.program.code.push(Op::Label(label));

        // `[UdonExtern("...")]`: the body *is* the extern. Some of what Udon
        // offers has no .NET type to call — `VRCInstantiate` is a wrapper
        // module, not a class — so corlib names the signature directly instead
        // of the compiler growing a special case per function.
        if let Some(signature) = self.udon_extern_of(key.symbol) {
            let mut pushed = parameters[usize::from(has_this)..].to_vec();
            pushed.extend(result);
            self.call_extern(&ctx, &signature, &pushed, 0..0);
            self.program.code.push(Op::JumpIndirect(return_slot));
            return;
        }

        // bind parameter names
        let value_parameters = &parameters[usize::from(has_this)..];
        match (key.role, &syntax) {
            (Role::DefaultConstructor, _) => {
                // the implicit constructor: field initializers, then `base()`
                self.emit_field_initializers(&mut ctx);
                let chain = self.bodies.constructor_chains.get(&key.symbol).cloned();
                if let Some(chain) = chain {
                    self.emit_constructor_chain(&mut ctx, &chain, &[], 0..0);
                }
            }
            (Role::StructEquals, _) => self.emit_struct_equals(&mut ctx),
            (Role::StructHashCode, _) => self.emit_struct_hash_code(&mut ctx),
            (Role::UnhandledException, _) => self.emit_unhandled_exception_body(&mut ctx),
            (Role::Constructor, Some(SyntaxRef::Constructor(declaration))) => {
                self.bind_parameters(
                    &mut ctx,
                    declaration
                        .parameters
                        .as_ref()
                        .ok()
                        .map(|list| list.parameters),
                    value_parameters,
                    &parameter_types,
                );
                if has_this {
                    // §15.11.2: `: this(...)` hands everything (field
                    // initializers included) to the sibling; otherwise the
                    // field initializers run, then the base constructor,
                    // then the body
                    let chain = self.bodies.constructor_chains.get(&key.symbol).cloned();
                    let arguments: &[Argument] = declaration
                        .initializer
                        .as_ref()
                        .and_then(|initializer| initializer.arguments.as_ref().ok())
                        .map(|list| list.arguments)
                        .unwrap_or(&[]);
                    let span = declaration
                        .initializer
                        .as_ref()
                        .map(|initializer| initializer.span.clone())
                        .unwrap_or_else(|| declaration.name.span.clone());
                    match &chain {
                        Some(chain) if chain.kind == ConstructorChainKind::This => {
                            self.emit_constructor_chain(&mut ctx, chain, arguments, span);
                        }
                        Some(chain) => {
                            self.emit_field_initializers(&mut ctx);
                            self.emit_constructor_chain(&mut ctx, chain, arguments, span);
                        }
                        None => self.emit_field_initializers(&mut ctx),
                    }
                }
                self.emit_function_body(&mut ctx, &declaration.body);
            }
            (Role::Method, Some(SyntaxRef::Method(declaration))) => {
                self.bind_parameters(
                    &mut ctx,
                    declaration
                        .parameters
                        .as_ref()
                        .ok()
                        .map(|list| list.parameters),
                    value_parameters,
                    &parameter_types,
                );
                self.emit_function_body(&mut ctx, &declaration.body);
            }
            // a user-declared operator is a static method with a symbol for a name
            (Role::Method, Some(SyntaxRef::Operator(declaration))) => {
                if declaration.conversion.is_some() {
                    self.error(
                        &ctx,
                        "conversion operators (`implicit operator` / `explicit operator`) are \
                         not supported by the Udon backend yet",
                        declaration.span.clone(),
                    );
                    return;
                }
                self.bind_parameters(
                    &mut ctx,
                    declaration
                        .parameters
                        .as_ref()
                        .ok()
                        .map(|list| list.parameters),
                    value_parameters,
                    &parameter_types,
                );
                self.emit_function_body(&mut ctx, &declaration.body);
            }
            (Role::Getter | Role::Setter, Some(SyntaxRef::Property(declaration))) => {
                if key.role == Role::Setter
                    && let (Some(&slot), Some(ty)) =
                        (value_parameters.first(), parameter_types.first())
                {
                    ctx.locals[0].insert("value", (slot, ty.clone()));
                }
                self.emit_accessor_body(&mut ctx, &declaration.body, key.role);
            }
            (Role::Getter | Role::Setter, Some(SyntaxRef::Indexer(declaration))) => {
                let names = declaration
                    .parameters
                    .as_ref()
                    .ok()
                    .map(|list| list.parameters);
                let index_count = names.map(|list| list.len()).unwrap_or(0);
                self.bind_parameters(
                    &mut ctx,
                    names,
                    &value_parameters[..index_count.min(value_parameters.len())],
                    &parameter_types,
                );
                if key.role == Role::Setter
                    && let (Some(&slot), Some(ty)) = (
                        value_parameters.get(index_count),
                        parameter_types.get(index_count),
                    )
                {
                    ctx.locals[0].insert("value", (slot, ty.clone()));
                }
                self.emit_accessor_body(&mut ctx, &declaration.body, key.role);
            }
            (_, other) => {
                let message = format!(
                    "this member cannot be compiled to Udon yet ({:?})",
                    other.as_ref().map(std::mem::discriminant)
                );
                self.error(&ctx, message, 0..0);
            }
        }

        // fall-through return
        self.program.code.push(Op::JumpIndirect(return_slot));
    }

    fn this_type_of(&self, key: &FunctionKey) -> Type {
        // the declaring class, instantiated with this key's bindings
        let mut class = key.symbol;
        loop {
            let entry = self.declarations.table.symbol(class);
            if entry.kind.is_type() {
                break;
            }
            match entry.parent {
                Some(parent) => class = parent,
                None => break,
            }
        }
        let arguments = self
            .declarations
            .table
            .symbol(class)
            .type_parameters
            .iter()
            .map(|parameter| self.substitute(&Type::TypeParameter(*parameter), &key.bindings))
            .collect();
        Type::Named {
            target: TypeTarget::Source(class),
            arguments,
        }
    }

    fn bind_parameters(
        &mut self,
        ctx: &mut Ctx<'ast>,
        declared: Option<&'ast [men_sharp_parser::ast::Parameter<'ast, 'ast>]>,
        slots: &[DataId],
        types: &[Type],
    ) {
        let Some(declared) = declared else {
            return;
        };
        for (index, parameter) in declared.iter().enumerate() {
            if let Ok(name) = &parameter.name
                && let (Some(&slot), Some(ty)) = (slots.get(index), types.get(index))
            {
                ctx.locals[0].insert(name.value, (slot, ty.clone()));
            }
        }
    }

    fn emit_function_body(&mut self, ctx: &mut Ctx<'ast>, body: &'ast FunctionBody<'ast, 'ast>) {
        match body {
            FunctionBody::Block(block) => self.lower_block(ctx, block),
            FunctionBody::Expression {
                expression: Ok(expression),
                ..
            } => {
                let value = self.lower_expression(ctx, expression);
                if let (Some(value), Some(result)) = (value, ctx.result) {
                    self.copy(value, result);
                }
            }
            FunctionBody::None { semicolon } => {
                self.error(
                    ctx,
                    "a bodiless (abstract/extern) member cannot be called directly on Udon",
                    semicolon.clone(),
                );
            }
            _ => {}
        }
    }

    fn emit_accessor_body(
        &mut self,
        ctx: &mut Ctx<'ast>,
        body: &'ast FunctionBody<'ast, 'ast>,
        role: Role,
    ) {
        match body {
            // expression-bodied property: the getter
            FunctionBody::Expression { expression, .. } => {
                if role == Role::Getter {
                    if let Ok(expression) = expression {
                        let value = self.lower_expression(ctx, expression);
                        if let (Some(value), Some(result)) = (value, ctx.result) {
                            self.copy(value, result);
                        }
                    }
                } else {
                    self.error(ctx, "this property has no setter", 0..0);
                }
            }
            FunctionBody::Accessors(list) => {
                let wanted = match role {
                    Role::Getter => AccessorKind::Get,
                    _ => AccessorKind::Set,
                };
                let accessor = list
                    .accessors
                    .iter()
                    .find(|accessor| accessor.kind.value == wanted);
                match accessor {
                    // `{ get; set; }` with storage of its own: reached as a
                    // function only through dispatch (an override of a
                    // virtual/abstract/interface property), so the accessor
                    // is the slot read or write itself
                    Some(accessor) if matches!(accessor.body, FunctionBody::None { .. }) => {
                        self.emit_auto_accessor(ctx, role, list.span.clone());
                    }
                    Some(accessor) => self.emit_function_body(ctx, &accessor.body),
                    None => self.error(ctx, "missing accessor", list.span.clone()),
                }
            }
            _ => self.error(ctx, "unsupported accessor shape", 0..0),
        }
    }

    fn emit_auto_accessor(&mut self, ctx: &mut Ctx<'ast>, role: Role, span: Range<usize>) {
        let slot = ctx
            .this_type
            .clone()
            .and_then(|this_type| self.layout_of(&this_type))
            .and_then(|layout| layout.slots.get(&ctx.key.symbol).copied());
        let (Some(index), Some(this)) = (slot, ctx.this_slot) else {
            self.error(
                ctx,
                "a bodiless (abstract/extern) member cannot be called directly on Udon",
                span,
            );
            return;
        };
        let index = self.int_constant(index as i32);
        match role {
            Role::Getter => {
                let (_, ty) = self.function_shape(&ctx.key);
                let value = self.get_element(ctx, this, index, &ty, span);
                if let Some(result) = ctx.result {
                    self.copy(value, result);
                }
            }
            _ => {
                let Some((value, _)) = ctx.lookup("value") else {
                    self.error(ctx, "internal: setter without a value", span);
                    return;
                };
                self.set_element(ctx, this, index, value, span);
            }
        }
    }

    /// `this` field/auto-property initializers, run at the top of every
    /// constructor of the class.
    fn emit_field_initializers(&mut self, ctx: &mut Ctx<'ast>) {
        let Some(this_type) = ctx.this_type.clone() else {
            return;
        };
        let Some(layout) = self.layout_of(&this_type) else {
            return;
        };
        let Type::Named {
            target: TypeTarget::Source(class),
            ..
        } = &this_type
        else {
            return;
        };
        let members: Vec<SymbolId> = self.declarations.table.symbol(*class).members.to_vec();
        for member in members {
            let Some(&slot_index) = layout.slots.get(&member) else {
                continue;
            };
            let symbol = self.declarations.table.symbol(member);
            let Some(site) = symbol.declarations.first() else {
                continue;
            };
            let initializer = match &site.syntax {
                SyntaxRef::Field { declarator, .. } => match &declarator.initializer {
                    Some(InitializerValue::Expression(value)) => Some(value),
                    _ => None,
                },
                SyntaxRef::Property(property) => match &property.initializer {
                    Some(InitializerValue::Expression(value)) => Some(value),
                    _ => None,
                },
                _ => None,
            };
            let object = ctx.this_slot.expect("constructors have this");
            let Some(initializer) = initializer else {
                // no initializer: C# guarantees the default value, and a
                // fresh object[] element is null — which is not 0, and an
                // extern given it for an Int32 throws. Reference types keep
                // the null they already have
                let field_type = match self.signatures.members.get(&member) {
                    Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => {
                        self.substitute(ty, &ctx.key.bindings)
                    }
                    _ => continue,
                };
                if !self.is_reference_type(&field_type) {
                    let value = self.default_value_in(ctx, &field_type, 0..0);
                    let index = self.int_constant(slot_index as i32);
                    self.set_element(ctx, object, index, value, 0..0);
                }
                continue;
            };
            if let Some(value) = self.owned_value(ctx, initializer) {
                let index = self.int_constant(slot_index as i32);
                self.set_element(ctx, object, index, value, 0..0);
            }
        }
    }

    /// The base (or sibling) constructor call at the top of a constructor,
    /// on the object under construction.
    fn emit_constructor_chain(
        &mut self,
        ctx: &mut Ctx<'ast>,
        chain: &ConstructorChain,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: Range<usize>,
    ) {
        let Some(this) = ctx.this_slot else {
            return;
        };
        let target_type = self.substitute(&chain.target_type, &ctx.key.bindings);
        // the entry class is the program itself: its chain has nothing to run
        if self.behaviour_in_type(&target_type).is_some() {
            return;
        }
        match &chain.call {
            Some(call) => {
                let Some(values) = self.constructor_arguments(ctx, call, arguments) else {
                    return;
                };
                if let MemberOrigin::Source(constructor) = call.origin {
                    let key = FunctionKey {
                        symbol: constructor,
                        role: Role::Constructor,
                        bindings: self.bindings_for(ctx, constructor, &call.declaring_type, &[]),
                    };
                    self.call_function(ctx, &key, Some(this), &values, &[], span);
                }
            }
            None => {
                // the target declares no constructor: its synthesized one
                let Type::Named {
                    target: TypeTarget::Source(class),
                    arguments: class_arguments,
                } = &target_type
                else {
                    return;
                };
                let parameters = &self.declarations.table.symbol(*class).type_parameters;
                let bindings = parameters
                    .iter()
                    .copied()
                    .zip(class_arguments.iter().cloned())
                    .collect();
                let key = FunctionKey {
                    symbol: *class,
                    role: Role::DefaultConstructor,
                    bindings,
                };
                self.call_function(ctx, &key, Some(this), &[], &[], span);
            }
        }
    }

    // ---------------------------------------------------------------- calls

    /// Emits a call to a compiled function instance; returns the slot holding
    /// the (copied) result.
    pub(super) fn call_function(
        &mut self,
        ctx: &mut Ctx<'ast>,
        key: &FunctionKey,
        this: Option<DataId>,
        arguments: &[DataId],
        // (argument index, where its value goes back): the callee's `ref`/
        // `out` parameters, whose slots are copied home after the call
        by_ref: &[(usize, Place)],
        span: Range<usize>,
    ) -> Option<DataId> {
        self.ensure_function(key);
        self.call_edges
            .entry(ctx.key.clone())
            .or_default()
            .insert(key.clone());

        let function = &self.functions[key];
        let label = function.label;
        let return_slot = function.return_slot;
        let result = function.result;
        let parameters = function.parameters.clone();
        let name = function.name.clone();

        // the receiver has to match the callee's shape before anything is
        // copied: a behaviour's members *are* the program's globals and take
        // no `this`, every other instance member needs one
        let wants_this = self.function_has_this(key);
        if wants_this != this.is_some() {
            let member = self.declarations.table.symbol(key.symbol).name;
            let message = if wants_this {
                format!("`{member}` needs an instance to be called on")
            } else {
                format!(
                    "`{member}` belongs to the behaviour itself — there is one instance, \
                     so it cannot be called on another object"
                )
            };
            self.error(ctx, message, span);
            return None;
        }

        let expected = usize::from(this.is_some()) + arguments.len();
        if parameters.len() != expected {
            self.error(
                ctx,
                format!(
                    "internal: `{name}` expects {} arguments, {expected} were provided",
                    parameters.len()
                ),
                span,
            );
            return None;
        }

        // placeholder around the whole call: if this edge turns out to be part
        // of a cycle, the callee's frame is saved here and restored after —
        // decided once the call graph is complete (resolve_frame_markers)
        let marker = self.frame_markers.len() as u32;
        self.frame_markers.push((ctx.key.clone(), key.clone()));
        self.program.code.push(Op::SaveFrame(marker));

        let mut sources: Vec<DataId> = Vec::with_capacity(expected);
        sources.extend(this);
        sources.extend_from_slice(arguments);
        for (source, parameter) in sources.iter().zip(&parameters) {
            self.copy(*source, *parameter);
        }

        let continuation = self.program.add_label(format!("ret_{}", self.temp_counter));
        self.temp_counter += 1;
        let constant = self.program.add_data(DataSymbol {
            name: format!("__retaddr_{}", self.temp_counter),
            udon_type: "SystemUInt32".into(),
            init: HeapInit::CodeAddress(continuation),
            export: false,
            sync: None,
        });
        self.copy(constant, return_slot);
        self.program.code.push(Op::Jump(Target::Label(label)));
        self.program.code.push(Op::Label(continuation));

        // `ref`/`out` results: the callee wrote its own parameter slots.
        // Rescue them into scratch slots *before* a recursive restore rewinds
        // the frame, write them into their places *after* it — a scratch is
        // deliberately outside every frame, and nothing runs in between.
        let mut returned: Vec<(DataId, Place)> = Vec::new();
        for (argument_index, place) in by_ref {
            let parameter = parameters[usize::from(this.is_some()) + argument_index];
            let udon_type = self.program.data[parameter.0].udon_type.clone();
            let scratch = self.scratch_slot(&udon_type);
            self.copy(parameter, scratch);
            returned.push((scratch, place.clone()));
        }

        self.program.code.push(Op::RestoreFrame(marker));

        for (scratch, place) in returned {
            self.write_place(ctx, place, scratch, span.clone());
        }

        // the callee may have returned early with an exception pending: it
        // continues unwinding from here — to this function's handler, or on
        // out of it
        self.emit_pending_check(ctx, &span);

        result.map(|result| {
            let symbol = &self.program.data[result.0];
            let udon_type = symbol.udon_type.clone();
            let temp = self.temp(&udon_type);
            self.copy(result, temp);
            temp
        })
    }

    /// A temp that belongs to *no* function's frame: for values that must
    /// survive a recursive frame restore (they are dead again by the next
    /// call, so nothing ever needs to save them).
    fn scratch_slot(&mut self, udon_type: &str) -> DataId {
        let saved = self.current_frame.take();
        let slot = self.temp(udon_type);
        self.current_frame = saved;
        slot
    }

    /// The bindings for a source member's instance: the declaring type's
    /// parameters bound to its (already substituted) arguments, plus the
    /// method's own parameters bound to the call's type arguments.
    pub(super) fn bindings_for(
        &self,
        ctx: &Ctx,
        member: SymbolId,
        declaring_type: &Type,
        type_arguments: &[Type],
    ) -> Vec<(SymbolId, Type)> {
        let mut bindings = Vec::new();
        if let Type::Named {
            target: TypeTarget::Source(class),
            arguments,
        } = declaring_type
        {
            let parameters = &self.declarations.table.symbol(*class).type_parameters;
            for (parameter, argument) in parameters.iter().zip(arguments) {
                bindings.push((*parameter, self.substitute(argument, &ctx.key.bindings)));
            }
        }
        let own = &self.declarations.table.symbol(member).type_parameters;
        for (parameter, argument) in own.iter().zip(type_arguments) {
            bindings.push((*parameter, self.substitute(argument, &ctx.key.bindings)));
        }
        bindings
    }

    fn declared_modifiers(&self, symbol: SymbolId) -> Vec<Modifier> {
        self.declarations
            .table
            .symbol(symbol)
            .declarations
            .iter()
            .flat_map(|site| match &site.syntax {
                SyntaxRef::Method(declaration) => declaration.modifiers,
                SyntaxRef::Property(declaration) => declaration.modifiers,
                SyntaxRef::Indexer(declaration) => declaration.modifiers,
                _ => &[],
            })
            .map(|modifier| modifier.value)
            .collect()
    }

    pub(super) fn is_virtual(&self, symbol: SymbolId) -> bool {
        self.is_interface_member(symbol)
            || self.declared_modifiers(symbol).iter().any(|modifier| {
                matches!(
                    modifier,
                    Modifier::Virtual | Modifier::Abstract | Modifier::Override
                )
            })
    }

    pub(super) fn is_interface_member(&self, symbol: SymbolId) -> bool {
        self.declarations
            .table
            .symbol(symbol)
            .parent
            .is_some_and(|parent| {
                self.declarations.table.symbol(parent).kind == SymbolKind::Interface
            })
    }

    /// Declared without a body: an abstract or interface member. Never a
    /// dispatch target — only a stub may stand in front of it.
    pub(super) fn is_bodiless(&self, symbol: SymbolId) -> bool {
        let without_body = !self
            .declarations
            .table
            .symbol(symbol)
            .declarations
            .iter()
            .any(|site| site.syntax.has_body());
        (self.is_interface_member(symbol) && without_body)
            || self
                .declared_modifiers(symbol)
                .iter()
                .any(|modifier| matches!(modifier, Modifier::Abstract | Modifier::Extern))
    }

    /// `sealed`, a struct, or otherwise a type no subtype can be created of:
    /// a virtual call on it binds statically.
    pub(super) fn is_final_type(&self, ty: &Type) -> bool {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = ty
        else {
            return false;
        };
        let entry = self.declarations.table.symbol(*symbol);
        match entry.kind {
            SymbolKind::Struct | SymbolKind::RecordStruct => true,
            SymbolKind::Class | SymbolKind::Record => entry.declarations.iter().any(|site| {
                matches!(&site.syntax, SyntaxRef::Class(declaration)
                    if declaration.modifiers.iter().any(|modifier| modifier.value == Modifier::Sealed))
            }),
            _ => false,
        }
    }

    /// The type that declares `member`, instantiated per `bindings`.
    pub(super) fn declaring_type_of(
        &self,
        member: SymbolId,
        bindings: &[(SymbolId, Type)],
    ) -> Type {
        let mut owner = member;
        while let Some(parent) = self.declarations.table.symbol(owner).parent {
            owner = parent;
            if self.declarations.table.symbol(owner).kind.is_type() {
                break;
            }
        }
        let arguments = self
            .declarations
            .table
            .symbol(owner)
            .type_parameters
            .iter()
            .map(|parameter| self.substitute(&Type::TypeParameter(*parameter), bindings))
            .collect();
        Type::Named {
            target: TypeTarget::Source(owner),
            arguments,
        }
    }

    // ------------------------------------------------------------- dispatch

    /// A call through a virtual method becomes a call to a dispatcher — a
    /// synthesized function that switches on the receiver's type id.
    /// The stub that dispatches `member` (a method body, getter or setter
    /// per `target`) on the runtime type of a receiver statically typed
    /// `declaring_type` — a class, an abstract class or an interface.
    pub(super) fn dispatcher_for(
        &mut self,
        ctx: &Ctx,
        member: SymbolId,
        declaring_type: &Type,
        type_arguments: &[Type],
        target: Role,
    ) -> FunctionKey {
        let bindings = self.bindings_for(ctx, member, declaring_type, type_arguments);
        let role = match target {
            Role::Getter => Role::GetterDispatcher,
            Role::Setter => Role::SetterDispatcher,
            _ => Role::Dispatcher,
        };
        let key = FunctionKey {
            symbol: member,
            role,
            bindings: bindings.clone(),
        };
        if !self.dispatchers.contains_key(&key) {
            // frame slots for the dispatcher itself
            self.ensure_dispatcher_frame(&key);
            let receiver = self.substitute(declaring_type, &ctx.key.bindings);
            let name = self.declarations.table.symbol(member).name.to_string();
            self.dispatchers.insert(
                key.clone(),
                Dispatcher {
                    name,
                    target,
                    receiver,
                    emitted_for: Vec::new(),
                },
            );
        }
        key
    }

    /// For a call on a receiver whose runtime type is already known — a
    /// struct, a sealed class — the implementation itself, so no stub is
    /// needed (the static dispatch a monomorphized `T : IShape` allows).
    pub(super) fn direct_implementation(
        &mut self,
        receiver_type: &Type,
        member: SymbolId,
        bindings: &[(SymbolId, Type)],
        target: Role,
    ) -> Option<FunctionKey> {
        if !self.is_final_type(receiver_type) {
            return None;
        }
        let shape_key = FunctionKey {
            symbol: member,
            role: target,
            bindings: bindings.to_vec(),
        };
        let (parameter_types, return_type) = self.function_shape(&shape_key);
        let name = self.declarations.table.symbol(member).name.to_string();
        let contract = self.contract_interface(member, bindings);
        self.implementation_on(
            receiver_type,
            &name,
            &parameter_types,
            &return_type,
            target,
            contract.as_ref().map(|contract| (member, contract)),
        )
    }

    /// When `member` is an interface member: that interface, instantiated —
    /// the contract an explicit implementation must name to count.
    fn contract_interface(&self, member: SymbolId, bindings: &[(SymbolId, Type)]) -> Option<Type> {
        if !self.is_interface_member(member) {
            return None;
        }
        Some(self.declaring_type_of(member, bindings))
    }

    /// Dispatcher frames live in `self.functions` (so `call_function` treats
    /// them uniformly) but are never queued for body compilation.
    fn ensure_dispatcher_frame(&mut self, key: &FunctionKey) {
        if self.functions.contains_key(key) {
            return;
        }
        self.ensure_function(key);
        // remove from the compile queue: the body is synthesized separately
        self.queue.retain(|queued| queued != key);
    }

    /// Makes sure the override of every instantiated subtype is scheduled.
    /// Returns true when this enqueued new work.
    pub(super) fn ensure_dispatcher_impls(&mut self) -> bool {
        let mut changed = false;
        let keys: Vec<FunctionKey> = self.dispatchers.keys().cloned().collect();
        for key in keys {
            let dispatcher = &self.dispatchers[&key];
            let receiver = dispatcher.receiver.clone();
            let name = dispatcher.name.clone();
            if key.role == Role::TypeTest {
                // nothing to call: the body enumerates types when emitted
                continue;
            }
            if let Role::ObjectDispatcher(member) = key.role {
                changed |= self.ensure_object_dispatcher_impls(&key, member.name());
                continue;
            }
            let candidates: Vec<Type> = self
                .type_order
                .iter()
                .filter(|ty| self.is_subtype(ty, &receiver))
                .cloned()
                .collect();
            for ty in candidates {
                let already = self.dispatchers[&key].emitted_for.contains(&ty);
                if already {
                    continue;
                }
                if self.emitted_dispatchers.contains(&key) {
                    let message = format!(
                        "internal: `{}` was instantiated after the dispatch of `{}` on `{}` \
                         was emitted",
                        self.display_type(&ty),
                        name,
                        self.display_type(&receiver)
                    );
                    let ctx = self.dispatcher_ctx(&key);
                    self.error(&ctx, message, 0..0);
                }
                if self.behaviour_in_type(&ty).is_some() {
                    // a behaviour is a program reference, not an object[]:
                    // there is no type id to read, and no way to call into
                    // it but by event name
                    let message = format!(
                        "`{}` cannot be reached through `{}`: a behaviour is another Udon \
                         program, which has no virtual dispatch — call it through a variable \
                         of its own type",
                        self.display_type(&ty),
                        self.display_type(&receiver)
                    );
                    let ctx = self.dispatcher_ctx(&key);
                    self.error(&ctx, message, 0..0);
                } else if let Some(implementation) = self.override_for(&ty, &name, &key) {
                    self.ensure_function(&implementation);
                    changed = true;
                }
                self.dispatchers
                    .get_mut(&key)
                    .expect("dispatcher exists")
                    .emitted_for
                    .push(ty);
            }
        }
        changed
    }

    /// The function instance implementing `name` for exact type `ty`,
    /// signature-compatible with the dispatcher.
    fn override_for(
        &mut self,
        ty: &Type,
        name: &str,
        dispatcher: &FunctionKey,
    ) -> Option<FunctionKey> {
        let target = self.dispatchers[dispatcher].target;
        let (parameter_types, return_type) = self.function_shape(dispatcher);
        let contract = self.contract_interface(dispatcher.symbol, &dispatcher.bindings);
        self.implementation_on(
            ty,
            name,
            &parameter_types,
            &return_type,
            target,
            contract
                .as_ref()
                .map(|contract| (dispatcher.symbol, contract)),
        )
    }

    /// The member of `ty` (or the nearest base) implementing `name` with
    /// this shape: a method for `Method`, a property/indexer accessor for
    /// `Getter`/`Setter`. For an interface `contract`, an explicit
    /// implementation of exactly that interface (`int IShape.Area()`) wins
    /// over a public member of the same name (§18.6.5), though ordinary
    /// lookup hides it; a bodiless (abstract, interface) member never
    /// implements anything.
    fn implementation_on(
        &mut self,
        ty: &Type,
        name: &str,
        parameter_types: &[Type],
        return_type: &Type,
        target: Role,
        contract: Option<(SymbolId, &Type)>,
    ) -> Option<FunctionKey> {
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let wanted_kind = match target {
            Role::Method => &[SymbolKind::Method][..],
            _ => &[SymbolKind::Property, SymbolKind::Indexer][..],
        };
        let mut candidates = Vec::new();
        if let Some((_, contract)) = contract {
            candidates.extend(self.explicit_implementations(ty, name, contract));
        }
        candidates.extend(system.members_named(ty, name));
        for candidate in candidates {
            let MemberOrigin::Source(symbol) = candidate.origin else {
                continue;
            };
            if !wanted_kind.contains(&candidate.kind) {
                continue;
            }
            if !Self::shape_fits(
                target,
                candidate.signature.as_ref(),
                parameter_types,
                return_type,
            ) {
                continue;
            }
            if self.is_bodiless(symbol) {
                // nothing to jump to: an abstract base's slot, or the
                // interface member itself when a class does not implement it
                // (the checker reports that)
                return None;
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
                role: target,
                bindings,
            });
        }
        // nothing on the class: the interface's own default implementation
        let (member, contract) = contract?;
        self.default_implementation(
            ty,
            member,
            contract,
            name,
            parameter_types,
            return_type,
            target,
        )
    }

    /// Whether a candidate's signature is the dispatched shape: a method
    /// for `Method`, a property/indexer accessor for `Getter`/`Setter`
    /// (an indexer's getter takes the indices, its setter the indices plus
    /// the value).
    fn shape_fits(
        target: Role,
        signature: Option<&MemberSignature>,
        parameter_types: &[Type],
        return_type: &Type,
    ) -> bool {
        match (target, signature) {
            (Role::Method, Some(MemberSignature::Function(signature))) => {
                signature.parameters.len() == parameter_types.len()
                    && signature
                        .parameters
                        .iter()
                        .zip(parameter_types)
                        .all(|(a, b)| a.parameter_type == *b)
            }
            (Role::Getter, Some(MemberSignature::Property(property))) => {
                parameter_types.is_empty() && property == return_type
            }
            (Role::Setter, Some(MemberSignature::Property(property))) => {
                parameter_types.len() == 1 && parameter_types[0] == *property
            }
            (Role::Getter, Some(MemberSignature::Function(signature))) => {
                signature.parameters.len() == parameter_types.len()
                    && signature
                        .parameters
                        .iter()
                        .zip(parameter_types)
                        .all(|(a, b)| a.parameter_type == *b)
                    && signature.return_type == *return_type
            }
            (Role::Setter, Some(MemberSignature::Function(signature))) => {
                signature.parameters.len() + 1 == parameter_types.len()
                    && signature
                        .parameters
                        .iter()
                        .zip(parameter_types)
                        .all(|(a, b)| a.parameter_type == *b)
                    && parameter_types.last() == Some(&signature.return_type)
            }
            _ => false,
        }
    }

    /// The default implementation (C# 8) `ty` inherits for `member` of
    /// `contract`: the most specific re-implementation (`void IA.G() { }`
    /// in an interface deriving from `IA`) among the interfaces `ty`
    /// implements, else the member's own body. Two unrelated interfaces
    /// re-implementing it is the C# ambiguity error (CS8705).
    #[allow(clippy::too_many_arguments)]
    fn default_implementation(
        &mut self,
        ty: &Type,
        member: SymbolId,
        contract: &Type,
        name: &str,
        parameter_types: &[Type],
        return_type: &Type,
        target: Role,
    ) -> Option<FunctionKey> {
        let bindings_of = |generator: &Self, interface: &Type| -> Vec<(SymbolId, Type)> {
            match interface {
                Type::Named {
                    target: TypeTarget::Source(symbol),
                    arguments,
                } => generator
                    .declarations
                    .table
                    .symbol(*symbol)
                    .type_parameters
                    .iter()
                    .copied()
                    .zip(arguments.iter().cloned())
                    .collect(),
                _ => Vec::new(),
            }
        };

        // re-implementations in derived interfaces
        let mut found: Vec<(Type, SymbolId)> = Vec::new();
        for interface in self.interface_closure(ty) {
            let Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } = &interface
            else {
                continue;
            };
            let bindings = bindings_of(self, &interface);
            let members: Vec<SymbolId> = self.declarations.table.symbol(*symbol).members.to_vec();
            for candidate in members {
                let entry = self.declarations.table.symbol(candidate);
                if !entry.is_explicit_implementation
                    || entry.name != name
                    || self.is_bodiless(candidate)
                {
                    continue;
                }
                let names_contract = self
                    .signatures
                    .explicit_interfaces
                    .get(&candidate)
                    .is_some_and(|named| self.substitute(named, &bindings) == *contract);
                if !names_contract {
                    continue;
                }
                let signature = self
                    .signatures
                    .members
                    .get(&candidate)
                    .map(|signature| self.substitute_member(signature, &bindings));
                if Self::shape_fits(target, signature.as_ref(), parameter_types, return_type) {
                    found.push((interface.clone(), candidate));
                }
            }
        }
        let competing = found.clone();
        if found.len() > 1 {
            // the most derived one, when there is one that derives from all
            found.retain(|(interface, _)| {
                competing
                    .iter()
                    .all(|(other, _)| other == interface || self.implements(interface, other))
            });
        }
        match found.len() {
            1 => {
                let (interface, candidate) = found.remove(0);
                let bindings = bindings_of(self, &interface);
                return Some(FunctionKey {
                    symbol: candidate,
                    role: target,
                    bindings,
                });
            }
            0 if competing.is_empty() => {}
            _ => {
                let found = competing;
                let (file, span) = self.declaration_site(member);
                let interfaces: Vec<String> = found
                    .iter()
                    .map(|(interface, _)| self.display_type(interface))
                    .collect();
                self.errors.push(CodegenError {
                    message: format!(
                        "`{}` inherits conflicting default implementations of `{}.{}` from {} \
                         — implement it on the type (CS8705)",
                        self.display_type(ty),
                        self.display_type(contract),
                        name,
                        interfaces.join(" and ")
                    ),
                    file,
                    span,
                });
                return None;
            }
        }

        // the member's own body
        if self.is_bodiless(member) {
            return None;
        }
        Some(FunctionKey {
            symbol: member,
            role: target,
            bindings: bindings_of(self, contract),
        })
    }

    /// Every interface `ty` implements, instantiated, directly or through
    /// a base class or another interface.
    fn interface_closure(&self, ty: &Type) -> Vec<Type> {
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let mut out: Vec<Type> = Vec::new();
        let mut visited: HashSet<Type> = HashSet::new();
        let mut queue = vec![ty.clone()];
        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if system.is_interface(&current) && &current != ty {
                out.push(current.clone());
            }
            queue.extend(system.interfaces_of(&current));
            if let Some(base) = system.base_of(&current)
                && (self.is_source_class(&base) || system.is_interface(&base))
            {
                queue.push(base);
            }
        }
        out
    }

    /// `int IShape.Area()` members named `name` implementing `contract` on
    /// `ty` and its base classes: excluded from ordinary lookup, so gathered
    /// here for dispatch.
    fn explicit_implementations(
        &self,
        ty: &Type,
        name: &str,
        contract: &Type,
    ) -> Vec<men_sharp_semantics::MemberCandidate> {
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let mut out = Vec::new();
        let mut current = Some(ty.clone());
        while let Some(class_type) = current {
            let Type::Named {
                target: TypeTarget::Source(class),
                arguments,
            } = &class_type
            else {
                break;
            };
            let entry = self.declarations.table.symbol(*class);
            let bindings: Vec<(SymbolId, Type)> = entry
                .type_parameters
                .iter()
                .copied()
                .zip(arguments.iter().cloned())
                .collect();
            for &member in &entry.members {
                let member_entry = self.declarations.table.symbol(member);
                if !member_entry.is_explicit_implementation || member_entry.name != name {
                    continue;
                }
                let implements_contract = self
                    .signatures
                    .explicit_interfaces
                    .get(&member)
                    .is_some_and(|interface| self.substitute(interface, &bindings) == *contract);
                if !implements_contract {
                    continue;
                }
                let signature = self
                    .signatures
                    .members
                    .get(&member)
                    .map(|signature| self.substitute_member(signature, &bindings));
                out.push(men_sharp_semantics::MemberCandidate {
                    origin: MemberOrigin::Source(member),
                    kind: member_entry.kind,
                    is_static: member_entry.is_static,
                    accessibility: member_entry.accessibility,
                    arity: member_entry.arity,
                    signature,
                    declaring_type: class_type.clone(),
                });
            }
            current = system
                .base_of(&class_type)
                .filter(|base| self.is_source_class(base));
        }
        out
    }

    fn substitute_member(
        &self,
        signature: &MemberSignature,
        bindings: &[(SymbolId, Type)],
    ) -> MemberSignature {
        match signature {
            MemberSignature::Function(function) => {
                MemberSignature::Function(self.substitute_signature(function, bindings))
            }
            MemberSignature::Property(ty) => {
                MemberSignature::Property(self.substitute(ty, bindings))
            }
            MemberSignature::Field(ty) => MemberSignature::Field(self.substitute(ty, bindings)),
            MemberSignature::Event(ty) => MemberSignature::Event(self.substitute(ty, bindings)),
        }
    }

    pub(super) fn is_subtype(&self, ty: &Type, of: &Type) -> bool {
        if ty == of {
            return true;
        }
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        if system.is_interface(of) {
            return self.implements(ty, of);
        }
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = ty
        else {
            return false;
        };
        let entry = self.declarations.table.symbol(*symbol);
        let bindings: Vec<(SymbolId, Type)> = entry
            .type_parameters
            .iter()
            .copied()
            .zip(arguments.iter().cloned())
            .collect();
        let base = self
            .signatures
            .base_types
            .get(symbol)
            .into_iter()
            .flatten()
            .find(|base| self.is_source_class(base))
            .cloned();
        match base {
            Some(base) => {
                let base = self.substitute(&base, &bindings);
                self.is_subtype(&base, of)
            }
            None => false,
        }
    }

    /// Does `ty` implement `interface` — directly, through a base class, or
    /// through an interface that extends it?
    pub(super) fn implements(&self, ty: &Type, interface: &Type) -> bool {
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        let mut visited: HashSet<Type> = HashSet::new();
        let mut queue = vec![ty.clone()];
        while let Some(current) = queue.pop() {
            if &current == interface {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            queue.extend(system.interfaces_of(&current));
            if let Some(base) = system.base_of(&current)
                && (self.is_source_class(&base) || system.is_interface(&base))
            {
                queue.push(base);
            }
        }
        false
    }

    /// A context for diagnostics raised while synthesizing a dispatcher.
    pub(super) fn dispatcher_ctx(&self, key: &FunctionKey) -> Ctx<'ast> {
        let function = &self.functions[key];
        Ctx {
            key: key.clone(),
            file: FileId(0),
            locals: Vec::new(),
            this_slot: None,
            this_type: None,
            loop_stack: Vec::new(),
            result: function.result,
            return_slot: function.return_slot,
            caught: Vec::new(),
        }
    }

    /// Emits every dispatcher body not emitted yet: read `this[0]`, compare
    /// against each instantiated subtype's id, tail-jump into the chosen
    /// override. Returns whether it emitted anything.
    pub(super) fn emit_dispatcher_bodies(&mut self) -> bool {
        let mut keys: Vec<FunctionKey> = self
            .dispatchers
            .keys()
            .filter(|key| !self.emitted_dispatchers.contains(key))
            .cloned()
            .collect();
        // deterministic output: the map's order is not
        keys.sort_by_key(|key| self.functions[key].name.clone());
        let emitted = !keys.is_empty();
        for key in keys {
            self.emitted_dispatchers.insert(key.clone());
            match key.role {
                Role::TypeTest => {
                    self.emit_type_test_body(&key);
                    continue;
                }
                Role::ObjectDispatcher(_) => {
                    self.emit_object_dispatcher_body(&key);
                    continue;
                }
                _ => {}
            }
            let function = &self.functions[&key];
            let label = function.label;
            let this_slot = function.parameters[0];
            let parameters = function.parameters.clone();
            let return_slot = function.return_slot;
            let result = function.result;

            self.program.code.push(Op::Label(label));
            // the dispatcher's own temps belong to its frame, like any body's
            self.current_frame = Some(key.clone());

            let mut ctx = Ctx {
                key: key.clone(),
                file: FileId(0),
                locals: vec![HashMap::new()],
                this_slot: Some(this_slot),
                this_type: None,
                loop_stack: Vec::new(),
                result,
                return_slot,
                caught: Vec::new(),
            };

            // type_id = (int) this[0]
            let zero = self.int_constant(0);
            let boxed = self.temp("SystemObject");
            self.call_extern(
                &ctx,
                "SystemObjectArray.__Get__SystemInt32__SystemObject",
                &[this_slot, zero, boxed],
                0..0,
            );
            let type_id = self.temp("SystemInt32");
            self.copy(boxed, type_id);

            let dispatcher = &self.dispatchers[&key];
            let targets: Vec<(Type, i32)> = dispatcher
                .emitted_for
                .iter()
                .filter_map(|ty| {
                    self.layouts
                        .get(ty)
                        .map(|layout| (ty.clone(), layout.type_id))
                })
                .collect();
            let name = dispatcher.name.clone();

            let condition = self.temp("SystemBoolean");
            for (ty, id) in targets {
                let Some(implementation) = self.override_for(&ty, &name, &key) else {
                    continue;
                };
                let id_constant = self.int_constant(id);
                let skip = self
                    .program
                    .add_label(format!("dispatch_skip_{}", self.temp_counter));
                self.temp_counter += 1;
                self.call_extern(
                    &ctx,
                    "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean",
                    &[type_id, id_constant, condition],
                    0..0,
                );
                self.program.code.push(Op::Push(condition));
                self.program.code.push(Op::JumpIfFalse(Target::Label(skip)));

                // an ordinary call rather than a tail jump: this records the
                // dispatcher → override edge and carries the frame-save
                // markers, so recursion through virtual dispatch is seen like
                // any other cycle instead of silently corrupting frames
                let arguments: Vec<DataId> = parameters[1..].to_vec();
                // the override's `ref`/`out` results come back into the
                // dispatcher's own parameter slots, where the real caller's
                // write-back then reads them
                let (parameter_types, _) = self.function_shape(&key);
                let by_ref: Vec<(usize, Place)> = match self.signatures.members.get(&key.symbol) {
                    Some(MemberSignature::Function(signature)) => signature
                        .parameters
                        .iter()
                        .enumerate()
                        .filter(|(_, parameter)| {
                            matches!(
                                parameter.passing,
                                ParameterPassing::Ref | ParameterPassing::Out
                            )
                        })
                        .map(|(index, _)| {
                            let ty = parameter_types.get(index).cloned().unwrap_or(Type::Error);
                            (index, Place::Slot(parameters[1 + index], ty))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let value = self.call_function(
                    &mut ctx,
                    &implementation,
                    Some(this_slot),
                    &arguments,
                    &by_ref,
                    0..0,
                );
                if let (Some(mine), Some(value)) = (result, value) {
                    self.copy(value, mine);
                }
                self.program.code.push(Op::JumpIndirect(return_slot));
                self.program.code.push(Op::Label(skip));
            }

            // no type matched: return default
            self.program.code.push(Op::JumpIndirect(return_slot));
            self.current_frame = None;
        }
        emitted
    }

    // ------------------------------------------------------ object plumbing

    pub(super) fn set_element(
        &mut self,
        ctx: &Ctx,
        object: DataId,
        index: DataId,
        value: DataId,
        span: Range<usize>,
    ) {
        self.call_extern(
            ctx,
            "SystemObjectArray.__Set__SystemInt32_SystemObject__SystemVoid",
            &[object, index, value],
            span,
        );
    }

    pub(super) fn get_element(
        &mut self,
        ctx: &Ctx,
        object: DataId,
        index: DataId,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let out = self.temp_for(ty);
        self.call_extern(
            ctx,
            "SystemObjectArray.__Get__SystemInt32__SystemObject",
            &[object, index, out],
            span,
        );
        out
    }

    /// The extern signature for a resolved external call.
    pub(super) fn external_signature(
        &mut self,
        ctx: &Ctx,
        call: &ResolvedCall,
        span: &Range<usize>,
    ) -> Option<String> {
        let MemberOrigin::External { member, .. } = &call.origin else {
            return None;
        };
        let declaring = self.substitute(&call.declaring_type, &ctx.key.bindings);
        let Some(owner) = self.extern_type_name(&declaring) else {
            self.error(
                ctx,
                "this call's declaring type cannot be represented on Udon",
                span.clone(),
            );
            return None;
        };
        let name = member.name.replace('.', "");
        let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
        let mut parts: Vec<String> = Vec::new();
        for parameter in &signature.parameters {
            match self.extern_type_name(&parameter.parameter_type) {
                Some(mut part) => {
                    // `ref`/`out` parameters are spelled with a `Ref` suffix in
                    // the whitelist: `UnityEngineRaycastHitRef`
                    if matches!(
                        parameter.passing,
                        ParameterPassing::Ref | ParameterPassing::Out
                    ) {
                        part.push_str("Ref");
                    }
                    parts.push(part);
                }
                None => {
                    self.error(
                        ctx,
                        "a parameter type of this call cannot be represented on Udon",
                        span.clone(),
                    );
                    return None;
                }
            }
        }
        let return_part = match self.extern_type_name(&signature.return_type) {
            Some(part) => part,
            None => {
                self.error(
                    ctx,
                    "the return type of this call cannot be represented on Udon",
                    span.clone(),
                );
                return None;
            }
        };
        let middle = if parts.is_empty() {
            String::new()
        } else {
            format!("__{}", parts.join("_"))
        };
        // Udon has no generics: a generic method is one extern named `…__T`
        // that takes its type argument as an ordinary `System.Type` value.
        // The caller supplies that value; see emit_call.
        if !call.type_arguments.is_empty() {
            return Some(format!("{owner}.__{name}{middle}__T"));
        }
        Some(format!("{owner}.__{name}{middle}__{return_part}"))
    }

    pub(super) fn substitute_signature(
        &self,
        signature: &FunctionSignature,
        bindings: &[(SymbolId, Type)],
    ) -> FunctionSignature {
        let substituted = MemberSignature::Function(signature.clone()).map(&|ty| {
            if let Type::TypeParameter(symbol) = &ty
                && let Some((_, concrete)) =
                    bindings.iter().find(|(parameter, _)| parameter == symbol)
            {
                return concrete.clone();
            }
            ty
        });
        match substituted {
            MemberSignature::Function(signature) => signature,
            _ => unreachable!(),
        }
    }
}
