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
        entry_class: None,
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
    /// Set when the entry class is a `MenSharpBehaviour` subclass: its
    /// instance fields live in named heap slots and its instance methods have
    /// no `this` — the behaviour is the program.
    entry_class: Option<SymbolId>,
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

        // a MenSharpBehaviour subclass gets the behaviour treatment: its
        // instance members become the program's surface
        if is_behaviour_class(self.declarations, self.signatures, entry) {
            if self.declarations.table.symbol(entry).arity > 0 {
                self.errors.push(CodegenError {
                    message: "a behaviour class cannot be generic".into(),
                    file: FileId(0),
                    span: 0..0,
                });
                return;
            }
            self.entry_class = Some(entry);
        }

        // static fields of the entry class become exported, observable slots;
        // on a behaviour, instance fields (and auto-properties) do too — the
        // behaviour has exactly one instance, so its fields are the program's
        // public variables
        self.collect_statics(entry, true);
        if self.entry_class.is_some() {
            self.collect_entry_instance_fields(entry);
        }

        // public methods of the entry class are events (instance ones only on
        // a behaviour; static ones always, which is also what tests use)
        let mut entries = Vec::new();
        let members: Vec<SymbolId> = self.declarations.table.symbol(entry).members.to_vec();
        for member in members {
            let symbol = self.declarations.table.symbol(member);
            let eligible = symbol.kind == SymbolKind::Method
                && symbol.accessibility == Accessibility::Public
                && (symbol.is_static || self.entry_class.is_some());
            if eligible {
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
                    "entry class `{}` has no public methods to export as events",
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

    /// On a behaviour entry class, instance fields and auto-properties become
    /// exported heap slots — the program's public variables. They reuse the
    /// `statics` machinery: the behaviour has exactly one instance.
    fn collect_entry_instance_fields(&mut self, class: SymbolId) {
        let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
        for member in members {
            let symbol = self.declarations.table.symbol(member);
            if symbol.is_static {
                continue;
            }
            let stores_value = match symbol.kind {
                SymbolKind::Field => true,
                SymbolKind::Property => self.is_auto_property(member),
                _ => false,
            };
            if stores_value {
                self.ensure_static(member, true);
            }
        }
    }

    /// Does this member symbol belong to the behaviour entry class?
    pub(super) fn is_entry_member(&self, member: SymbolId) -> bool {
        let Some(entry) = self.entry_class else {
            return false;
        };
        self.declarations.table.symbol(member).parent == Some(entry)
    }

    fn ensure_static(&mut self, field: SymbolId, export: bool) -> DataId {
        if let Some(&slot) = self.statics.get(&field) {
            return slot;
        }
        let ty = match self.signatures.members.get(&field) {
            Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => ty.clone(),
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
        let initializer = symbol
            .declarations
            .first()
            .and_then(|site| match &site.syntax {
                SyntaxRef::Field { declarator, .. } => declarator.initializer.as_ref(),
                SyntaxRef::Property(property) => property.initializer.as_ref(),
                _ => None,
            })
            .and_then(|initializer| match initializer {
                InitializerValue::Expression(expression) => Some(expression),
                _ => None,
            });

        // literal initializers bake into the heap default instead of running
        // as code. This matters for exported behaviour fields: the inspector's
        // public-variable values are applied *after* the heap loads, so a
        // baked default lets them win — runtime initializer code would
        // overwrite them on the first event
        let baked = initializer.and_then(|expression| literal_heap_init(expression, &udon_type));
        let runs_at_startup = initializer.is_some() && baked.is_none();

        let slot = self.program.add_data(DataSymbol {
            name,
            udon_type,
            init: baked.unwrap_or(HeapInit::Null),
            export,
            sync: None,
        });
        self.statics.insert(field, slot);
        if runs_at_startup && let Some(file) = file {
            self.static_init.push((field, file));
        }
        slot
    }
}

/// A literal (possibly negated) initializer as a heap default, when its kind
/// matches the slot's declared Udon type. Anything else returns `None` and
/// stays runtime-initialized.
fn literal_heap_init(expression: &Expression, udon_type: &str) -> Option<HeapInit> {
    fn literal_of<'e>(expression: &'e Expression) -> Option<(&'e LiteralExpression<'e, 'e>, bool)> {
        match expression {
            Expression::Primary(primary) if primary.chain.is_empty() => match &primary.left {
                PrimaryLeft::Literal(literal) => Some((literal, false)),
                _ => None,
            },
            Expression::Unary(unary) if unary.operator.value == UnaryOperator::Minus => {
                let operand = unary.operand.as_ref().ok()?;
                let (literal, negated) = literal_of(operand)?;
                Some((literal, !negated))
            }
            _ => None,
        }
    }

    let (literal, negated) = literal_of(expression)?;
    match (literal, udon_type) {
        (LiteralExpression::Integer(text), "SystemInt32") => {
            let raw: String = text.value.chars().filter(|c| *c != '_').collect();
            let value = raw.parse::<i64>().ok()?;
            let value = if negated { -value } else { value };
            i32::try_from(value).ok().map(HeapInit::Int32)
        }
        (LiteralExpression::Real(text), "SystemSingle") => {
            let raw: String = text.value.chars().filter(|c| *c != '_').collect();
            let value = raw.strip_suffix(['f', 'F'])?.parse::<f32>().ok()?;
            Some(HeapInit::Single(if negated { -value } else { value }))
        }
        (LiteralExpression::Real(text), "SystemDouble") => {
            let raw: String = text.value.chars().filter(|c| *c != '_').collect();
            let trimmed = raw.trim_end_matches(['d', 'D']);
            let value = trimmed.parse::<f64>().ok()?;
            Some(HeapInit::Double(if negated { -value } else { value }))
        }
        (LiteralExpression::True(_), "SystemBoolean") if !negated => Some(HeapInit::Boolean(true)),
        (LiteralExpression::False(_), "SystemBoolean") if !negated => {
            Some(HeapInit::Boolean(false))
        }
        (LiteralExpression::String(text), "SystemString") if !negated => {
            let inner = text
                .value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))?;
            // escape-free strings only; anything fancier initializes at runtime
            if inner.contains('\\') {
                return None;
            }
            Some(HeapInit::Str(inner.to_string()))
        }
        _ => None,
    }
}

/// Is this class a `MenSharp.MenSharpBehaviour` subclass?
fn is_behaviour_class(
    declarations: &Declarations,
    signatures: &Signatures,
    class: SymbolId,
) -> bool {
    let Some(base_marker) = behaviour_marker(declarations) else {
        return false;
    };
    let mut current = class;
    loop {
        if current == base_marker {
            return true;
        }
        let base = signatures
            .base_types
            .get(&current)
            .into_iter()
            .flatten()
            .find_map(|base| match base {
                Type::Named {
                    target: TypeTarget::Source(symbol),
                    ..
                } => Some(*symbol),
                _ => None,
            });
        match base {
            Some(base) => current = base,
            None => return false,
        }
    }
}

fn behaviour_marker(declarations: &Declarations) -> Option<SymbolId> {
    let root = declarations.table.root();
    let namespace = *declarations
        .table
        .symbol(root)
        .members_named("MenSharp")
        .first()?;
    declarations
        .table
        .symbol(namespace)
        .members_named("MenSharpBehaviour")
        .first()
        .copied()
}

/// Every `MenSharp.MenSharpBehaviour` subclass in the compilation, as dotted
/// paths — the auto-discovered entry set.
pub fn behaviour_classes(declarations: &Declarations, signatures: &Signatures) -> Vec<String> {
    let Some(marker) = behaviour_marker(declarations) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![(declarations.table.root(), String::new())];
    while let Some((symbol_id, path)) = stack.pop() {
        let symbol = declarations.table.symbol(symbol_id);
        for &member in &symbol.members {
            let child = declarations.table.symbol(member);
            let child_path = if path.is_empty() {
                child.name.to_string()
            } else {
                format!("{path}.{}", child.name)
            };
            match child.kind {
                SymbolKind::Namespace => stack.push((member, child_path)),
                SymbolKind::Class
                    if member != marker && is_behaviour_class(declarations, signatures, member) =>
                {
                    found.push(child_path);
                }
                _ => {}
            }
        }
    }
    found.sort();
    found
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
