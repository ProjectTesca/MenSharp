//! The lowering machine: checked M# → Udon assembly.
//!
//! ## The model
//!
//! Udon has no user types, no call stack and no exceptions, so this module
//! invents the missing machinery on top of the heap:
//!
//! - **Objects** are `object[]`: slot 0 holds an `int` type-id, fields (and
//!   auto-property backing stores) follow, base-class fields first.
//! - **Generics** are monomorphized: each distinct instantiation of a method
//!   or type compiles separately, Rust-style. The unit of compilation is a
//!   [`FunctionKey`] — a member symbol plus concrete bindings for every type
//!   parameter in scope.
//! - **Calls** use static frames: every function instance owns heap slots for
//!   its parameters, locals, temporaries, result and return address. The
//!   caller fills the argument slots, stores a return-address constant, jumps;
//!   the callee ends with `JUMP_INDIRECT` on its return slot. Recursion would
//!   corrupt these frames, so cycles in the call graph are reported as errors
//!   (a real stack for recursive cliques is future work).
//! - **Virtual calls** read the type-id and dispatch through a synthesized
//!   dispatcher function that compares against every instantiated subtype —
//!   exact ids, because every object stores its exact type.
//! - Everything else — arithmetic included — is an `EXTERN` against the
//!   whitelist; every emitted signature is validated against the embedded SDK
//!   dump at generation time, so "compiles" means "every call exists in Udon".
//!
//! The semantic phases already did the hard part: [`BodyCheck::targets`] says
//! what every name, call and index bound to, and [`BodyCheck::expression_types`]
//! gives every expression's type (with type parameters still free — this module
//! substitutes per instantiation).

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;

use men_sharp_asm::{
    DataId, DataSymbol, EntryPoint, HALT_ADDRESS, HeapInit, LabelId, Op, Program, Target,
};
use men_sharp_parser::ast::{
    Argument, ArgumentValue, AssignmentOperator, BinaryOperator, Block, EntityID, Expression,
    ForInitializer, FunctionBody, InitializerValue, InterpolationPart, LiteralExpression,
    PrimaryExpression, PrimaryLeft, PrimaryRight, Statement, UnaryOperator,
};
use men_sharp_semantics::{
    Accessibility, BodyCheck, Declarations, ExternalTypes, FileId, MemberOrigin, MemberSignature,
    ResolvedCall, ResolvedMember, ResolvedTarget, Signatures, SymbolId, SymbolKind, SyntaxRef,
    Type, TypeTarget,
};

use crate::externs::{UdonNodes, mangle_dotnet_name};

// ---------------------------------------------------------------------- API

#[derive(Debug, Clone)]
pub struct CodegenError {
    pub message: String,
    pub file: FileId,
    pub span: Range<usize>,
}

pub struct CodegenOutput {
    pub program: Program,
    pub errors: Vec<CodegenError>,
}

/// Lowers the checked compilation to one Udon program.
///
/// `entry_path` names the entry class (namespace path + class name). Every
/// public static method of that class becomes an exported entry point under
/// its own name, and every static field of it becomes an exported heap
/// variable — which is also how tests observe results.
pub fn generate(
    declarations: &Declarations,
    signatures: &Signatures,
    bodies: &BodyCheck,
    external: &dyn ExternalTypes,
    nodes: &UdonNodes,
    entry_path: &[&str],
) -> CodegenOutput {
    let mut generator = Generator {
        declarations,
        signatures,
        bodies,
        external,
        nodes,
        program: Program::default(),
        errors: Vec::new(),
        constants: HashMap::new(),
        functions: HashMap::new(),
        queue: VecDeque::new(),
        statics: HashMap::new(),
        static_init: Vec::new(),
        layouts: HashMap::new(),
        type_order: Vec::new(),
        dispatchers: HashMap::new(),
        call_edges: HashMap::new(),
        temp_counter: 0,
    };
    generator.run(entry_path);
    CodegenOutput {
        program: generator.program,
        errors: generator.errors,
    }
}

// ------------------------------------------------------------------- keys

/// One monomorphized function instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionKey {
    symbol: SymbolId,
    role: Role,
    /// Concrete types for every type parameter in scope (declaring types' and
    /// the method's own), in a canonical order.
    bindings: Vec<(SymbolId, Type)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    Method,
    Getter,
    Setter,
    Constructor,
    /// The synthesized parameterless constructor of a class with none declared
    /// (runs field initializers). `symbol` is the class.
    DefaultConstructor,
}

/// A compiled (or scheduled) function instance.
struct Function {
    label: LabelId,
    /// `this` first when the function has one.
    parameters: Vec<DataId>,
    result: Option<DataId>,
    return_slot: DataId,
    name: String,
}

/// Layout of one instantiated class: `object[]` size and field slot indices.
#[derive(Debug, Clone)]
struct Layout {
    type_id: i32,
    size: usize,
    /// Field or auto-property symbol → element index.
    slots: HashMap<SymbolId, usize>,
}

/// A synthesized virtual-call dispatcher: one per (root method, bindings).
struct Dispatcher {
    /// The method name used to find overrides on each instantiated subtype.
    name: String,
    receiver: Type,
    emitted_for: Vec<Type>,
}

struct Generator<'a, 'ast> {
    declarations: &'a Declarations<'ast>,
    signatures: &'a Signatures,
    bodies: &'a BodyCheck,
    external: &'a dyn ExternalTypes,
    nodes: &'a UdonNodes,

    program: Program,
    errors: Vec<CodegenError>,
    constants: HashMap<(String, String), DataId>,
    functions: HashMap<FunctionKey, Function>,
    queue: VecDeque<FunctionKey>,
    statics: HashMap<SymbolId, DataId>,
    /// Static fields with initializers, in declaration order.
    static_init: Vec<(SymbolId, FileId)>,
    layouts: HashMap<Type, Layout>,
    type_order: Vec<Type>,
    dispatchers: HashMap<FunctionKey, Dispatcher>,
    call_edges: HashMap<FunctionKey, HashSet<FunctionKey>>,
    temp_counter: usize,
}

/// Per-function compilation state.
struct Ctx<'ast> {
    key: FunctionKey,
    file: FileId,
    locals: Vec<HashMap<&'ast str, (DataId, Type)>>,
    this_slot: Option<DataId>,
    this_type: Option<Type>,
    /// (continue target, break target) innermost last.
    loop_stack: Vec<(LabelId, LabelId)>,
    result: Option<DataId>,
    return_slot: DataId,
}

impl Ctx<'_> {
    fn lookup(&self, name: &str) -> Option<(DataId, Type)> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

/// What a primary-expression step produced.
enum Piece {
    /// A value in a slot.
    Value(DataId, Type),
    /// A completed `void` call.
    Void,
    /// A namespace/type prefix or a method group: nothing to evaluate yet.
    /// Carries the receiver value when the group came off an instance.
    Pending {
        receiver: Option<(DataId, Type)>,
    },
    Error,
}

/// An assignable location.
enum Place {
    Slot(DataId, Type),
    /// `object[element_index]` — field (or auto-property store) of an object.
    Field {
        object: DataId,
        index: DataId,
        ty: Type,
    },
    /// `array[index]`.
    Element {
        array: DataId,
        index: DataId,
        element: Type,
        array_type: Type,
    },
    /// Source property/indexer with accessor bodies.
    Accessor {
        receiver: Option<DataId>,
        symbol: SymbolId,
        bindings: Vec<(SymbolId, Type)>,
        /// Indexer arguments, already evaluated.
        indices: Vec<DataId>,
        ty: Type,
    },
    /// External property: `__get_X`/`__set_X` externs.
    ExternalProperty {
        receiver: Option<DataId>,
        owner: String,
        name: String,
        ty: Type,
    },
    Error,
}

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------- driving

    fn run(&mut self, entry_path: &[&str]) {
        let Some(entry) = self.find_symbol(entry_path) else {
            self.errors.push(CodegenError {
                message: format!("entry class `{}` was not found", entry_path.join(".")),
                file: FileId(0),
                span: 0..0,
            });
            return;
        };

        // static fields of the entry class become exported, observable slots
        self.collect_statics(entry, true);

        // every public static method of the entry class is an event
        let mut entries = Vec::new();
        let members: Vec<SymbolId> = self.declarations.table.symbol(entry).members.to_vec();
        for member in members {
            let symbol = self.declarations.table.symbol(member);
            if symbol.kind == SymbolKind::Method
                && symbol.is_static
                && symbol.accessibility == Accessibility::Public
            {
                let name = udon_event_name(symbol.name);
                let key = FunctionKey {
                    symbol: member,
                    role: Role::Method,
                    bindings: Vec::new(),
                };
                self.ensure_function(&key);
                entries.push((name, key));
            }
        }
        if entries.is_empty() {
            self.errors.push(CodegenError {
                message: format!(
                    "entry class `{}` has no public static methods",
                    entry_path.join(".")
                ),
                file: FileId(0),
                span: 0..0,
            });
            return;
        }

        // fixpoint: draining the queue may register new types, which may make
        // dispatchers incomplete, which enqueues more functions, ...
        loop {
            while let Some(key) = self.queue.pop_front() {
                self.compile_function(&key);
            }
            if !self.ensure_dispatcher_impls() {
                break;
            }
        }
        self.emit_dispatcher_bodies();
        while let Some(key) = self.queue.pop_front() {
            self.compile_function(&key);
        }

        // entry stubs: initialize statics once, call the method, halt
        let initialized = self.program.add_data(DataSymbol {
            name: "__initialized".into(),
            udon_type: "SystemBoolean".into(),
            init: HeapInit::Boolean(false),
            export: false,
            sync: None,
        });
        let init_label = self.program.add_label("__static_init");
        let init_return = self.program.add_data(DataSymbol {
            name: "__static_init__return".into(),
            udon_type: "SystemUInt32".into(),
            init: HeapInit::UInt32(0),
            export: false,
            sync: None,
        });

        for (name, key) in &entries {
            let label = self.program.add_label(format!("event_{name}"));
            self.program.entry_points.push(EntryPoint {
                name: name.clone(),
                label,
            });
            self.program.code.push(Op::Label(label));

            // prime the initializer's return address, then jump into it
            // unless statics are already initialized
            let continue_label = self.program.add_label(format!("event_{name}__init_done"));
            let return_constant = self.program.add_data(DataSymbol {
                name: format!("__ret_event_{name}"),
                udon_type: "SystemUInt32".into(),
                init: HeapInit::CodeAddress(continue_label),
                export: false,
                sync: None,
            });
            self.copy(return_constant, init_return);
            self.program.code.push(Op::Push(initialized));
            self.program
                .code
                .push(Op::JumpIfFalse(Target::Label(init_label)));
            self.program.code.push(Op::Label(continue_label));

            let function = &self.functions[key];
            let (callee_return, callee_label) = (function.return_slot, function.label);
            let halt = self.code_address_constant(format!("__halt_{name}"), None);
            self.copy(halt, callee_return);
            self.program
                .code
                .push(Op::Jump(Target::Label(callee_label)));
        }

        // the shared static initializer body
        self.emit_static_initializer(init_label, init_return, initialized);

        self.check_for_recursion();
    }

    /// `JUMP_INDIRECT` on a constant that resolves to `HALT_ADDRESS` (or a
    /// label when given).
    fn code_address_constant(&mut self, name: String, label: Option<LabelId>) -> DataId {
        let init = match label {
            Some(label) => HeapInit::CodeAddress(label),
            None => HeapInit::UInt32(HALT_ADDRESS),
        };
        self.program.add_data(DataSymbol {
            name,
            udon_type: "SystemUInt32".into(),
            init,
            export: false,
            sync: None,
        })
    }

    fn emit_static_initializer(
        &mut self,
        init_label: LabelId,
        init_return: DataId,
        initialized: DataId,
    ) {
        // Rewritten entry protocol: events jump here only when !__initialized,
        // priming `init_return`. See `run` — but the simple version used
        // there jumps with fall-through semantics, so this body just runs the
        // initializers and returns.
        self.program.code.push(Op::Label(init_label));
        let true_constant = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        self.copy(true_constant, initialized);

        let inits = std::mem::take(&mut self.static_init);
        for (field, file) in &inits {
            self.emit_static_field_initializer(*field, *file);
        }
        self.static_init = inits;
        self.program.code.push(Op::JumpIndirect(init_return));
    }

    fn emit_static_field_initializer(&mut self, field: SymbolId, file: FileId) {
        let symbol = self.declarations.table.symbol(field);
        let Some(site) = symbol.declarations.first() else {
            return;
        };
        let SyntaxRef::Field { declarator, .. } = site.syntax else {
            return;
        };
        let Some(InitializerValue::Expression(value)) = &declarator.initializer else {
            return;
        };
        let slot = self.statics[&field];
        let ty = match self.signatures.members.get(&field) {
            Some(MemberSignature::Field(ty)) => ty.clone(),
            _ => Type::Error,
        };

        let mut ctx = Ctx {
            key: FunctionKey {
                symbol: field,
                role: Role::Method,
                bindings: Vec::new(),
            },
            file,
            locals: vec![HashMap::new()],
            this_slot: None,
            this_type: None,
            loop_stack: Vec::new(),
            result: None,
            return_slot: slot, // unused
        };
        if let Some(value_slot) = self.lower_expression(&mut ctx, value) {
            let _ = ty;
            self.copy(value_slot, slot);
        }
    }

    fn check_for_recursion(&mut self) {
        // DFS over the recorded call edges; any back edge is a cycle we
        // cannot run on static frames.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Done,
        }
        let mut marks: HashMap<&FunctionKey, Mark> = HashMap::new();
        let mut reported = HashSet::new();

        fn visit<'k>(
            node: &'k FunctionKey,
            edges: &'k HashMap<FunctionKey, HashSet<FunctionKey>>,
            marks: &mut HashMap<&'k FunctionKey, Mark>,
            cycles: &mut Vec<&'k FunctionKey>,
        ) {
            match marks.get(node) {
                Some(Mark::Done) => return,
                Some(Mark::Visiting) => {
                    cycles.push(node);
                    return;
                }
                None => {}
            }
            marks.insert(node, Mark::Visiting);
            if let Some(next) = edges.get(node) {
                for callee in next {
                    visit(callee, edges, marks, cycles);
                }
            }
            marks.insert(node, Mark::Done);
        }

        let mut cycles = Vec::new();
        for node in self.call_edges.keys() {
            visit(node, &self.call_edges, &mut marks, &mut cycles);
        }
        for node in cycles {
            if reported.insert(node.clone()) {
                let name = self
                    .functions
                    .get(node)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| "<unknown>".into());
                self.errors.push(CodegenError {
                    message: format!(
                        "`{name}` is recursive; recursion is not supported by the Udon backend yet"
                    ),
                    file: FileId(0),
                    span: 0..0,
                });
            }
        }
    }

    // ----------------------------------------------------------- utilities

    fn error(&mut self, ctx: &Ctx, message: impl Into<String>, span: Range<usize>) {
        self.errors.push(CodegenError {
            message: message.into(),
            file: ctx.file,
            span,
        });
    }

    fn find_symbol(&self, path: &[&str]) -> Option<SymbolId> {
        let mut current = self.declarations.table.root();
        for segment in path {
            current = *self
                .declarations
                .table
                .symbol(current)
                .members_named(segment)
                .first()?;
        }
        Some(current)
    }

    fn symbol_path(&self, symbol: SymbolId) -> String {
        let mut parts = Vec::new();
        let mut current = Some(symbol);
        while let Some(id) = current {
            let entry = self.declarations.table.symbol(id);
            if !entry.name.is_empty() {
                parts.push(entry.name.replace(['.', '[', ']'], "_"));
            }
            current = entry.parent;
        }
        parts.reverse();
        parts.join("_")
    }

    fn constant(&mut self, udon_type: &str, repr: &str, init: HeapInit) -> DataId {
        let key = (udon_type.to_string(), repr.to_string());
        if let Some(&id) = self.constants.get(&key) {
            return id;
        }
        let index = self.constants.len();
        let id = self.program.add_data(DataSymbol {
            name: format!("__const_{index}_{udon_type}"),
            udon_type: udon_type.to_string(),
            init,
            export: false,
            sync: None,
        });
        self.constants.insert(key, id);
        id
    }

    fn int_constant(&mut self, value: i32) -> DataId {
        self.constant("SystemInt32", &value.to_string(), HeapInit::Int32(value))
    }

    fn temp(&mut self, udon_type: &str) -> DataId {
        self.temp_counter += 1;
        let name = format!("__t{}", self.temp_counter);
        self.program.add_data(DataSymbol {
            name,
            udon_type: udon_type.to_string(),
            init: HeapInit::Null,
            export: false,
            sync: None,
        })
    }

    fn temp_for(&mut self, ty: &Type) -> DataId {
        let udon_type = self.heap_type(ty);
        self.temp(&udon_type)
    }

    fn copy(&mut self, source: DataId, destination: DataId) {
        self.program.code.push(Op::Push(source));
        self.program.code.push(Op::Push(destination));
        self.program.code.push(Op::Copy);
    }

    fn call_extern(
        &mut self,
        ctx: &Ctx,
        signature: &str,
        arguments: &[DataId],
        span: Range<usize>,
    ) {
        match self.nodes.extern_node(signature) {
            None => {
                self.error(ctx, format!("`{signature}` is not exposed by Udon"), span);
            }
            Some(node) if node.parameters.len() != arguments.len() => {
                self.error(
                    ctx,
                    format!(
                        "internal: `{signature}` takes {} parameters, {} were provided",
                        node.parameters.len(),
                        arguments.len()
                    ),
                    span,
                );
            }
            Some(_) => {
                for argument in arguments {
                    self.program.code.push(Op::Push(*argument));
                }
                self.program.code.push(Op::Extern(signature.to_string()));
            }
        }
    }

    // -------------------------------------------------------- type mapping

    fn substitute(&self, ty: &Type, bindings: &[(SymbolId, Type)]) -> Type {
        ty.map(&|t| match t {
            Type::TypeParameter(symbol) => bindings
                .iter()
                .find(|(parameter, _)| *parameter == symbol)
                .map(|(_, concrete)| concrete.clone())
                .unwrap_or(Type::TypeParameter(symbol)),
            other => other,
        })
    }

    fn type_of(&self, ctx: &Ctx, expression: &Expression) -> Type {
        let ty = self
            .bodies
            .expression_types
            .get(&EntityID::from(expression))
            .cloned()
            .unwrap_or(Type::Error);
        self.substitute(&ty, &ctx.key.bindings)
    }

    /// The declared Udon heap type for a slot of this M# type.
    fn heap_type(&self, ty: &Type) -> String {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } => {
                let mut name = mangle_dotnet_name(&self.external.display_name(*id));
                for argument in arguments {
                    name.push_str(&self.heap_type_component(argument));
                }
                if self.nodes.has_type(&name) {
                    name
                } else {
                    "SystemObject".into()
                }
            }
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => match self.declarations.table.symbol(*symbol).kind {
                SymbolKind::Enum => "SystemInt32".into(),
                _ => "SystemObjectArray".into(),
            },
            Type::Array { element, rank: 1 } => {
                let name = format!("{}Array", self.heap_type_component(element));
                if self.nodes.has_type(&name) {
                    name
                } else {
                    "SystemObjectArray".into()
                }
            }
            Type::Nullable(inner) => self.heap_type(inner),
            _ => "SystemObject".into(),
        }
    }

    fn heap_type_component(&self, ty: &Type) -> String {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } => {
                let mut name = mangle_dotnet_name(&self.external.display_name(*id));
                for argument in arguments {
                    name.push_str(&self.heap_type_component(argument));
                }
                name
            }
            Type::Array { element, rank: 1 } => {
                format!("{}Array", self.heap_type_component(element))
            }
            _ => "SystemObject".into(),
        }
    }

    /// The type's spelling inside an extern signature; `None` when the type
    /// cannot appear there (user types, unresolved parameters).
    fn extern_type_name(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } => {
                let mut name = mangle_dotnet_name(&self.external.display_name(*id));
                for argument in arguments {
                    name.push_str(&self.extern_type_name(argument)?);
                }
                Some(name)
            }
            Type::Array { element, rank: 1 } => {
                Some(format!("{}Array", self.extern_type_name(element)?))
            }
            Type::Void => Some("SystemVoid".into()),
            _ => None,
        }
    }

    // ------------------------------------------------------------- objects

    /// The layout (and type id) for an instantiated source class.
    fn layout_of(&mut self, ty: &Type) -> Option<Layout> {
        if let Some(layout) = self.layouts.get(ty) {
            return Some(layout.clone());
        }
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = ty
        else {
            return None;
        };
        let symbol = *symbol;
        let entry = self.declarations.table.symbol(symbol);
        let type_parameters: Vec<SymbolId> = entry.type_parameters.to_vec();
        let members: Vec<SymbolId> = entry.members.to_vec();

        // base first, so inherited field indices stay valid in subclasses
        let mut slots = HashMap::new();
        let mut size = 1usize; // slot 0: type id
        let base = self
            .signatures
            .base_types
            .get(&symbol)
            .into_iter()
            .flatten()
            .find(|base| self.is_source_class(base))
            .cloned();
        if let Some(base) = base {
            let bindings: Vec<(SymbolId, Type)> = type_parameters
                .iter()
                .copied()
                .zip(arguments.iter().cloned())
                .collect();
            let base = self.substitute(&base, &bindings);
            if let Some(base_layout) = self.layout_of(&base) {
                size = base_layout.size;
                slots.extend(base_layout.slots);
            }
        }

        for member in members {
            let member_symbol = self.declarations.table.symbol(member);
            if member_symbol.is_static {
                continue;
            }
            let kind = member_symbol.kind;
            let stores_value = match kind {
                SymbolKind::Field => true,
                SymbolKind::Property => self.is_auto_property(member),
                _ => false,
            };
            if stores_value {
                slots.insert(member, size);
                size += 1;
            }
        }

        let type_id = self.type_order.len() as i32;
        let layout = Layout {
            type_id,
            size,
            slots,
        };
        self.layouts.insert(ty.clone(), layout.clone());
        self.type_order.push(ty.clone());
        Some(layout)
    }

    fn is_source_class(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if matches!(
                self.declarations.table.symbol(*symbol).kind,
                SymbolKind::Class | SymbolKind::Struct | SymbolKind::Record | SymbolKind::RecordStruct
            )
        )
    }

    fn is_auto_property(&self, symbol: SymbolId) -> bool {
        let entry = self.declarations.table.symbol(symbol);
        entry.declarations.iter().all(|site| {
            matches!(&site.syntax, SyntaxRef::Property(property)
            if matches!(&property.body, FunctionBody::Accessors(accessors)
                if accessors.accessors.iter().all(|accessor| matches!(
                    accessor.body,
                    FunctionBody::None { .. }
                ))))
        })
    }

    fn collect_statics(&mut self, class: SymbolId, export: bool) {
        let entry = self.declarations.table.symbol(class);
        let members: Vec<SymbolId> = entry.members.to_vec();
        for member in members {
            let symbol = self.declarations.table.symbol(member);
            if symbol.kind != SymbolKind::Field || !symbol.is_static {
                continue;
            }
            self.ensure_static(member, export);
        }
    }

    fn ensure_static(&mut self, field: SymbolId, export: bool) -> DataId {
        if let Some(&slot) = self.statics.get(&field) {
            return slot;
        }
        let ty = match self.signatures.members.get(&field) {
            Some(MemberSignature::Field(ty)) => ty.clone(),
            _ => Type::Error,
        };
        let udon_type = self.heap_type(&ty);
        let symbol = self.declarations.table.symbol(field);
        let name = if export {
            symbol.name.to_string()
        } else {
            format!("static_{}", self.symbol_path(field))
        };
        let file = symbol.declarations.first().map(|site| site.file);
        let has_initializer = symbol.declarations.first().is_some_and(|site| {
            matches!(&site.syntax, SyntaxRef::Field { declarator, .. }
                if declarator.initializer.is_some())
        });
        let slot = self.program.add_data(DataSymbol {
            name,
            udon_type,
            init: HeapInit::Null,
            export,
            sync: None,
        });
        self.statics.insert(field, slot);
        if has_initializer && let Some(file) = file {
            self.static_init.push((field, file));
        }
        slot
    }
}

/// Unity/VRChat lifecycle methods map to Udon's built-in event names; other
/// method names become custom events verbatim.
fn udon_event_name(name: &str) -> String {
    match name {
        "Start" => "_start".into(),
        "Update" => "_update".into(),
        "LateUpdate" => "_lateUpdate".into(),
        "FixedUpdate" => "_fixedUpdate".into(),
        "OnEnable" => "_onEnable".into(),
        "OnDisable" => "_onDisable".into(),
        "Interact" => "_interact".into(),
        other => other.to_string(),
    }
}

mod expressions;
mod functions;
