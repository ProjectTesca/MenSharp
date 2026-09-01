//! Function instances: scheduling, frames, bodies, calls and virtual dispatch.

use men_sharp_parser::ast::{AccessorKind, Modifier};
use men_sharp_semantics::{FunctionSignature, ParameterPassing};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// The instantiated parameter/return shape for a function instance:
    /// parameter types (setter value last) and the return type.
    pub(super) fn function_shape(&self, key: &FunctionKey) -> (Vec<Type>, Type) {
        let member = self.signatures.members.get(&key.symbol);
        let (parameters, return_type) = match (key.role, member) {
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
            Role::Constructor | Role::DefaultConstructor => true,
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
            Role::Dispatcher => name.push_str("_dispatch"),
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
                self.emit_field_initializers(&mut ctx);
            }
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
                if declaration.initializer.is_some() {
                    self.error(
                        &ctx,
                        "constructor initializers (`: base(...)` / `: this(...)`) are not supported by the Udon backend yet",
                        declaration.span.clone(),
                    );
                }
                self.emit_field_initializers(&mut ctx);
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
                    Some(accessor) => self.emit_function_body(ctx, &accessor.body),
                    None => self.error(ctx, "missing accessor", list.span.clone()),
                }
            }
            _ => self.error(ctx, "unsupported accessor shape", 0..0),
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
            let Some(initializer) = initializer else {
                continue;
            };
            if let Some(value) = self.lower_expression(ctx, initializer) {
                let object = ctx.this_slot.expect("constructors have this");
                let index = self.int_constant(slot_index as i32);
                self.set_element(ctx, object, index, value, 0..0);
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
        self.program.code.push(Op::RestoreFrame(marker));

        result.map(|result| {
            let symbol = &self.program.data[result.0];
            let udon_type = symbol.udon_type.clone();
            let temp = self.temp(&udon_type);
            self.copy(result, temp);
            temp
        })
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
        self.declared_modifiers(symbol).iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Virtual | Modifier::Abstract | Modifier::Override
            )
        })
    }

    // ------------------------------------------------------------- dispatch

    /// A call through a virtual method becomes a call to a dispatcher — a
    /// synthesized function that switches on the receiver's type id.
    pub(super) fn dispatcher_for(
        &mut self,
        ctx: &Ctx,
        call: &ResolvedCall,
        method: SymbolId,
        declaring_type: &Type,
    ) -> FunctionKey {
        let bindings = self.bindings_for(ctx, method, declaring_type, &call.type_arguments);
        let key = FunctionKey {
            symbol: method,
            role: Role::Dispatcher,
            bindings: bindings.clone(),
        };
        if !self.dispatchers.contains_key(&key) {
            // frame slots for the dispatcher itself
            self.ensure_dispatcher_frame(&key);
            let receiver = self.substitute(declaring_type, &ctx.key.bindings);
            let name = self.declarations.table.symbol(method).name.to_string();
            self.dispatchers.insert(
                key.clone(),
                Dispatcher {
                    name,
                    receiver,
                    emitted_for: Vec::new(),
                },
            );
        }
        key
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
                if let Some(implementation) = self.override_for(&ty, &name, &key) {
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
        let (dispatch_parameters, _) = self.function_shape(dispatcher);
        let system = men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        };
        for candidate in system.members_named(ty, name) {
            let MemberOrigin::Source(symbol) = candidate.origin else {
                continue;
            };
            let Some(MemberSignature::Function(signature)) = candidate.signature else {
                continue;
            };
            if signature.parameters.len() != dispatch_parameters.len() {
                continue;
            }
            let matches = signature
                .parameters
                .iter()
                .zip(&dispatch_parameters)
                .all(|(a, b)| a.parameter_type == *b);
            if !matches {
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
        None
    }

    pub(super) fn is_subtype(&self, ty: &Type, of: &Type) -> bool {
        if ty == of {
            return true;
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

    /// Emits every dispatcher body: read `this[0]`, compare against each
    /// instantiated subtype's id, tail-jump into the chosen override.
    pub(super) fn emit_dispatcher_bodies(&mut self) {
        let keys: Vec<FunctionKey> = self.dispatchers.keys().cloned().collect();
        for key in keys {
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
                let value = self.call_function(
                    &mut ctx,
                    &implementation,
                    Some(this_slot),
                    &arguments,
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
