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
    AccessorKind, Argument, ArgumentModifier, ArgumentValue, AssignmentOperator, BinaryOperator,
    Block, EntityID, Expression, ForInitializer, FunctionBody, InitializerValue, InterpolationPart,
    LiteralExpression, PrimaryExpression, PrimaryLeft, PrimaryRight, Statement, TypeRefBase,
    UnaryOperator,
};
use men_sharp_semantics::{
    Accessibility, BodyCheck, ConstructorChain, ConstructorChainKind, Declarations, ExternalTypes,
    FileId, ForeachEnumeration, MemberOrigin, MemberSignature, ResolvedCall, ResolvedMember,
    ResolvedTarget, Signatures, SymbolId, SymbolKind, SyntaxRef, Type, TypeTarget,
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
    /// The file the entry class was declared in. The driver turns this into
    /// [`men_sharp_asm::Program::source`]; codegen only knows file ids.
    pub source_file: Option<FileId>,
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
        static_init_emitted: HashSet::new(),
        static_init_phase: false,
        static_constructors: Vec::new(),
        layouts: HashMap::new(),
        type_order: Vec::new(),
        exception_state: None,
        line_starts: HashMap::new(),
        last_source_mark: None,
        dispatchers: HashMap::new(),
        emitted_dispatchers: HashSet::new(),
        export_layouts: HashMap::new(),
        call_edges: HashMap::new(),
        temp_counter: 0,
        entry_class: None,
        entry_chain: Vec::new(),
        marker: behaviour_marker(declarations),
        entry_file: None,
        entry: None,
        lambdas: HashMap::new(),
        local_functions: HashMap::new(),
        delegate_shapes: Vec::new(),
        invokers: HashMap::new(),
        thunks: HashMap::new(),
        thunk_queue: VecDeque::new(),
        multicast_thunks: HashMap::new(),
        thunk_addresses: HashMap::new(),
        current_frame: None,
        frame_markers: Vec::new(),
        external_callers: HashSet::new(),
        async_snapshots: Vec::new(),
        resume_thunks: HashMap::new(),
        self_behaviour: None,
        incoming_resume: None,
    };
    generator.run(entry_path);
    CodegenOutput {
        source_file: generator.entry_file,
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
    /// The synthesized field-wise `bool Equals(object)` of a struct that
    /// declares none. `symbol` is the struct.
    StructEquals,
    /// The synthesized field-wise `int GetHashCode()` of a struct that
    /// declares none. `symbol` is the struct.
    StructHashCode,
    /// The synthesized virtual-dispatch stub for `symbol`: compares the
    /// receiver's type id and jumps to the right override. A role of its own so
    /// it never collides with the method's own body — which it would otherwise
    /// do whenever the declaring class is itself instantiated, or reached
    /// through `base.`, making the stub dispatch to itself forever.
    Dispatcher,
    /// The function a lambda expression compiles to. `symbol` is the member
    /// whose body the lambda is written in; the id is the lambda node's.
    /// See `delegates`.
    Lambda(EntityID),
    /// The function a local function compiles to. `symbol` is the member
    /// whose body it is written in; the id is its declaration node's. It
    /// takes the boxes of what it captures before its own parameters —
    /// there is no delegate, the call is direct. See `delegates`.
    LocalFunction(EntityID),
    /// The stub every call of a delegate of one shape goes through: takes
    /// the delegate and the arguments, jumps to the address the delegate
    /// holds. `symbol` is the entry class; the index names the shape. See
    /// `delegates`.
    DelegateInvoker(u32),
    /// The dispatch stub of a virtual/abstract/interface property or indexer
    /// getter (`symbol` is the property).
    GetterDispatcher,
    /// ... and setter.
    SetterDispatcher,
    /// `bool (object)`: is the value an object of type `symbol` (with these
    /// bindings) or a subtype — the runtime test behind casts, `is` and
    /// `as`. Synthesized after the fixpoint like a dispatcher, since it
    /// enumerates every instantiated subtype.
    TypeTest,
    /// The one function every uncaught exception ends in: reports it and
    /// halts the behaviour. `symbol` is the root namespace.
    UnhandledException,
    /// `Equals`, `GetHashCode` or `ToString` of one tuple shape, built
    /// element by element: a tuple has no members to call, so the object
    /// dispatcher jumps here when it meets one. `symbol` is the entry
    /// class; the id is the shape's type id (see `type_order`).
    TupleMember(ObjectMember, u32),
    /// The stub behind `Equals`, `GetHashCode` or `ToString` on a receiver
    /// whose runtime type is open (`object`, a non-sealed class, an
    /// interface): finds the user's override by type id, else falls back to
    /// the `System.Object` extern. `symbol` is the root namespace — there is
    /// one stub per member in the whole program.
    ObjectDispatcher(ObjectMember),
}

/// The `System.Object` members every type may override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ObjectMember {
    Equals,
    GetHashCode,
    ToString,
}

impl ObjectMember {
    fn of(name: &str) -> Option<Self> {
        match name {
            "Equals" => Some(Self::Equals),
            "GetHashCode" => Some(Self::GetHashCode),
            "ToString" => Some(Self::ToString),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Equals => "Equals",
            Self::GetHashCode => "GetHashCode",
            Self::ToString => "ToString",
        }
    }
}

/// A compiled (or scheduled) function instance.
struct Function {
    label: LabelId,
    /// `this` first when the function has one.
    parameters: Vec<DataId>,
    result: Option<DataId>,
    return_slot: DataId,
    name: String,
    /// Every mutable slot this function's one static frame consists of:
    /// parameters, the return address, and each temporary its body allocates.
    /// What a re-entrant call has to save to leave the outer activation
    /// intact. The result slot is deliberately absent — its value only
    /// matters between the callee writing it and the caller's immediate
    /// copy-out, and no call happens in that window.
    frame: Vec<DataId>,
}

/// Layout of one instantiated class: `object[]` size and field slot indices.
#[derive(Debug, Clone)]
struct Layout {
    type_id: i32,
    size: usize,
    /// Field or auto-property symbol → element index.
    slots: HashMap<SymbolId, usize>,
}

/// One `[FieldChangeCallback]` field: the `_onVarChange_<slot>` entry the
/// runtime raises when `SetProgramVariable` or network sync writes the field.
struct FieldCallback {
    /// `_onVarChange_<slot>`.
    name: String,
    field_slot: DataId,
    /// `_old_<slot>` — where the runtime leaves the previous value.
    old_slot_name: String,
    udon_type: String,
    setter: FunctionKey,
}

/// One exported event entry: the stub that initializes statics, hands over
/// the event's arguments and jumps into the method body.
struct EventEntry {
    name: String,
    key: FunctionKey,
    arguments: Vec<EventArgument>,
    /// `OnOwnershipRequest`: Udon reads the result back from `__returnValue`.
    returns_value: bool,
    /// A custom event with a result: the variable another program reads it
    /// from afterwards, `(name, heap type)`.
    result_slot: Option<(String, String)>,
}

/// One value an event hands over: the stub copies the named slot — written
/// by the runtime for a built-in event, by the calling program for a custom
/// one — into the function's parameter.
struct EventArgument {
    slot: String,
    udon_type: String,
    /// `ref`/`out`: the parameter's final value goes back into the slot for
    /// the caller to read.
    write_back: bool,
}

/// See [`Generator::exception_state`].
#[derive(Clone, Copy)]
struct ExceptionState {
    exception: DataId,
    pending: DataId,
}

/// A synthesized virtual-call dispatcher: one per (root method, bindings).
struct Dispatcher {
    /// The member name used to find overrides on each instantiated subtype.
    name: String,
    /// What the stub stands in for: a method body, a getter or a setter.
    target: Role,
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
    /// Static fields whose initializer has been emitted into the static
    /// initializer body already.
    static_init_emitted: HashSet<SymbolId>,
    /// Set while the static initializer body is being emitted: a static
    /// field first met there gets its initializer emitted on the spot,
    /// before the read that met it — C#'s "initialized before first use".
    static_init_phase: bool,
    /// Every static constructor in the compilation, in declaration order;
    /// the static initializer runs them after the field initializers.
    static_constructors: Vec<FunctionKey>,
    layouts: HashMap<Type, Layout>,
    type_order: Vec<Type>,
    /// The heap slots exceptions travel in: the exception itself and the
    /// "one is pending" flag every call checks after returning. Made on
    /// first use.
    exception_state: Option<ExceptionState>,
    /// Byte offsets where each line starts, per file — built on first use,
    /// for the `File.cs:line:column` in stack traces.
    line_starts: HashMap<FileId, Vec<usize>>,
    /// The last source mark emitted, so a run of externs from one statement
    /// shares one entry.
    last_source_mark: Option<(FileId, usize, FunctionKey)>,
    dispatchers: HashMap<FunctionKey, Dispatcher>,
    /// Dispatchers (and type tests) whose body has been emitted: their
    /// subtype list is closed, so a type instantiated afterwards is an
    /// internal error rather than a silently missing branch.
    emitted_dispatchers: HashSet<FunctionKey>,
    /// Per behaviour class: how other programs reach each of its methods
    /// and accessors — the names UdonSharp would use. See `programs`.
    export_layouts: HashMap<SymbolId, HashMap<programs::LayoutKey, programs::ExportLayout>>,
    call_edges: HashMap<FunctionKey, HashSet<FunctionKey>>,
    temp_counter: usize,
    /// Set when the entry class is a `MenSharpBehaviour` subclass: its
    /// instance fields live in named heap slots and its instance methods have
    /// no `this` — the behaviour is the program.
    entry_class: Option<SymbolId>,
    /// The entry class and every base class up to (excluding)
    /// `MenSharpBehaviour`, most derived first. All of their instance members
    /// belong to the one instance the program is.
    entry_chain: Vec<SymbolId>,
    /// `MenSharp.MenSharpBehaviour` itself, when the compilation declares it.
    marker: Option<SymbolId>,
    /// The file the entry class was declared in, once `run` has found it.
    entry_file: Option<FileId>,
    /// The entry class itself.
    entry: Option<SymbolId>,
    /// Every lambda that became a function, by its key. See `delegates`.
    lambdas: HashMap<FunctionKey, delegates::LambdaInfo<'ast>>,
    /// Every local function that became a function, by its key: registered
    /// when the block that declares it is lowered, so a call written above
    /// the declaration finds it too. See `delegates`.
    local_functions: HashMap<FunctionKey, delegates::LocalFunctionInfo<'ast>>,
    /// The delegate shapes met so far, indexed by `Role::DelegateInvoker`.
    delegate_shapes: Vec<delegates::DelegateShape>,
    /// The invoker function of each shape.
    invokers: HashMap<delegates::DelegateShape, FunctionKey>,
    /// The thunk that enters `(target, shape)` from an invoker, once made.
    thunks: HashMap<(FunctionKey, u32), LabelId>,
    /// Thunks registered but not emitted yet.
    thunk_queue: VecDeque<delegates::Thunk>,
    /// Per shape index: the thunk that calls a multicast delegate's list
    /// in turn, once made.
    multicast_thunks: HashMap<u32, LabelId>,
    /// The `SystemUInt32` constant holding each thunk's code address: what
    /// a delegate carries in element 0.
    thunk_addresses: HashMap<LabelId, DataId>,
    /// The function whose body is being compiled right now; every temp
    /// allocated while set joins that function's frame.
    current_frame: Option<FunctionKey>,
    /// (caller, callee) per source-level call site, indexed by the id inside
    /// `Op::SaveFrame`/`Op::RestoreFrame`. Emitted for every call, expanded
    /// into real save/restore code — or nothing — once the graph is complete.
    frame_markers: Vec<(FunctionKey, FunctionKey)>,
    /// Functions that call into another program (`SendCustomEvent`,
    /// `SetProgramVariable`): while such a call is in flight, everything on
    /// the current call chain can be re-entered from outside, which no static
    /// analysis of *this* program can see — those ancestors get a runtime
    /// re-entry guard instead.
    external_callers: HashSet<FunctionKey>,
    /// (function, slot) per `Op::SnapshotFrame`/`Op::RestoreSnapshot`
    /// marker: the function whose frame the snapshot in `slot` copies.
    /// Expanded with the frame markers, once every frame is complete. See
    /// `tasks`.
    async_snapshots: Vec<(FunctionKey, DataId)>,
    /// The thunk that resumes each `async` function from a continuation
    /// delegate, once made. See `tasks`.
    resume_thunks: HashMap<FunctionKey, LabelId>,
    /// The slot holding the program's own UdonBehaviour, once made.
    self_behaviour: Option<DataId>,
    /// The exported slot another program leaves a continuation of ours in,
    /// once made. See `tasks`.
    incoming_resume: Option<DataId>,
}

/// Per-function compilation state.
struct Ctx<'ast> {
    key: FunctionKey,
    file: FileId,
    locals: Vec<HashMap<String, Local>>,
    /// The locals and parameters of this body that some lambda captures:
    /// declared in a box (a one-element `object[]`) the lambda shares,
    /// instead of a slot of their own. See `delegates`.
    boxed: Vec<String>,
    this_slot: Option<DataId>,
    this_type: Option<Type>,
    /// What `break`/`continue` bind to, innermost last — and the `try`
    /// regions in between, which exceptions unwind to.
    loop_stack: Vec<BreakFrame<'ast>>,
    result: Option<DataId>,
    return_slot: DataId,
    /// The exception each enclosing `catch` block caught, innermost last:
    /// what a bare `throw;` rethrows.
    caught: Vec<DataId>,
    /// Set inside an `async` body: its task and what completes it. See
    /// `tasks`.
    async_state: Option<tasks::AsyncCtx>,
    /// Set inside an iterator body: the `Iterator<T>` it yields into. See
    /// `tasks`.
    iterator_state: Option<tasks::IteratorCtx>,
}

impl Ctx<'_> {
    fn lookup(&self, name: &str) -> Option<Local> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

/// Where a local variable or parameter lives.
#[derive(Clone)]
struct Local {
    /// Its own slot — or, when `boxed`, the slot holding its box.
    slot: DataId,
    ty: Type,
    /// Captured by a lambda: the value is element 0 of the `object[]` in
    /// `slot`, which the lambda's closure shares.
    boxed: bool,
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
    /// `base` — like `Pending`, but the member named next binds statically:
    /// `base.M()` has to reach the base implementation, not re-enter the
    /// override that is calling it. Only carried across a method group, so
    /// `base.M().N()` dispatches `N` normally.
    Base {
        receiver: Option<(DataId, Type)>,
    },
    Error,
}

impl Piece {
    /// The receiver a following member access, call or index should use.
    fn receiver(self) -> Option<(DataId, Type)> {
        match self {
            Piece::Value(slot, ty) => Some((slot, ty)),
            Piece::Pending { receiver } | Piece::Base { receiver } => receiver,
            Piece::Void | Piece::Error => None,
        }
    }
}

/// One enclosing construct `break` can leave. `continue` skips over `Switch`
/// frames to the nearest loop, as C# does.
enum BreakFrame<'ast> {
    Loop {
        continue_target: LabelId,
        break_target: LabelId,
    },
    Switch {
        break_target: LabelId,
    },
    /// A `try` region: where an exception raised (or returned into) this
    /// region goes, and what every exit out of it has to run first.
    Try {
        handler: LabelId,
        finally: Option<FinallyAction<'ast>>,
    },
}

/// What leaving a `try` region runs on the way out.
#[derive(Clone)]
enum FinallyAction<'ast> {
    /// The `Dispose()` call is the big variant, and one lives in every
    /// `foreach` frame; boxing it keeps `BreakFrame` small.
    /// The `finally` block as it was written.
    Block(&'ast Block<'ast, 'ast>),
    /// A `foreach` whose enumerator can be disposed: §13.9.5 wraps the loop
    /// in exactly this `try`/`finally`, and it is what runs an iterator's
    /// pending `finally` blocks when the loop is left early.
    Dispose(Box<Disposal>),
}

/// See [`FinallyAction::Dispose`].
#[derive(Clone)]
struct Disposal {
    call: ResolvedCall,
    enumerator: DataId,
    enumerator_type: Type,
}

/// An assignable location.
#[derive(Clone)]
enum Place {
    Slot(DataId, Type),
    /// `gameObject`/`transform`: a slot Udon fills in with what the behaviour
    /// is attached to. Reads like a slot; assignment is rejected, matching the
    /// read-only properties `UnityEngine.Component` declares.
    SelfReference {
        slot: DataId,
        ty: Type,
        name: String,
    },
    /// A computed, read-only value (`x.HasValue`, `x.Value`): already in a
    /// slot; assignment is rejected naming `what` it was.
    ReadOnly {
        slot: DataId,
        ty: Type,
        what: String,
    },
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
    /// A public variable of *another* behaviour, reached by name. Udon gives
    /// two programs no shared memory, only `GetProgramVariable` /
    /// `SetProgramVariable` over a string — so this is what a field access
    /// across the boundary becomes.
    ProgramVariable {
        receiver: DataId,
        name: String,
        ty: Type,
    },
    /// A property with a body on *another* behaviour: reached through its
    /// accessor events, `(event, variable)` for each — the getter's result
    /// variable, the setter's value parameter.
    ProgramAccessor {
        receiver: DataId,
        getter: Option<(String, String)>,
        setter: Option<(String, String)>,
        name: String,
        ty: Type,
    },
    /// External property: `__get_X`/`__set_X` externs.
    ExternalProperty {
        receiver: Option<DataId>,
        owner: String,
        name: String,
        ty: Type,
    },
    /// External indexer (`list[0]`, `dictionary[key]`, `vector[1]`):
    /// `__get_Item`/`__set_Item` externs, whose names carry the index types.
    ExternalIndexer {
        receiver: DataId,
        owner: String,
        /// The metadata name — `Item`, unless `[IndexerName]` renamed it.
        name: String,
        /// Each index, already converted to its parameter type, with that
        /// type: the extern's name is built from them.
        indices: Vec<(DataId, Type)>,
        ty: Type,
    },
    Error,
}

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------------- driving

    fn run(&mut self, entry_path: &[&str]) {
        // heap slots 0 and 1: the program's identity, which is what the VM's
        // heap dump shows first when an extern throws and halts it — the
        // Unity side maps the id back to this program's sidecar
        self.emit_program_identity(entry_path);
        let Some(entry) = self.find_symbol(entry_path) else {
            self.errors.push(CodegenError {
                message: format!("entry class `{}` was not found", entry_path.join(".")),
                file: FileId(0),
                span: 0..0,
            });
            return;
        };

        // a behaviour declared in a library file: outside the MenSharp
        // sources, so nothing checks it and UdonSharp's own compiler reads
        // it — not a program to emit, an error to fix
        if is_behaviour_class(self.declarations, self.signatures, entry) {
            let (file, span) = self.declaration_site(entry);
            if self.declarations.is_foreign(file) {
                let name = entry_path.join(".");
                self.errors.push(CodegenError {
                    message: format!(
                        "`{name}` inherits MenSharpBehaviour but is outside the MenSharp \
                         sources: not under Assets/MenSharp, and not in an assembly \
                         definition that references ProjectTesca.MenSharp.Runtime. Move it \
                         to Assets/MenSharp, or give its folder an assembly definition \
                         (MenSharp > Create Package…, or MenSharp > Create Assembly Definition)"
                    ),
                    file,
                    span,
                });
                return;
            }
        }

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
            self.entry_chain = self.behaviour_chain(entry);
        }
        self.entry_file = Some(self.declaration_site(entry).0);
        self.entry = Some(entry);

        // static fields of the entry class become exported, observable slots;
        // on a behaviour, instance fields (and auto-properties) do too — the
        // behaviour has exactly one instance, so its fields are the program's
        // public variables
        self.collect_statics(entry, true);
        if self.entry_class.is_some() {
            self.collect_entry_instance_fields(entry);
            // most derived first: a leaf may override the mode its base set
            for class in self.entry_chain.clone() {
                if let Some(mode) = self.behaviour_sync_mode(class) {
                    self.program.sync_mode = Some(mode);
                    break;
                }
            }
        }

        // public methods are events (instance ones only on a behaviour; static
        // ones always, which is also what tests use). A behaviour exports its
        // inherited events too — walking the chain most-derived first means an
        // override claims the event name before the method it overrides.
        let mut entries: Vec<EventEntry> = Vec::new();
        let mut claimed: HashSet<String> = HashSet::new();
        let classes = if self.entry_chain.is_empty() {
            vec![entry]
        } else {
            self.entry_chain.clone()
        };
        for class in classes {
            let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
            for member in members {
                let symbol = self.declarations.table.symbol(member);
                let eligible = symbol.kind == SymbolKind::Method
                    && symbol.accessibility == Accessibility::Public
                    && (symbol.is_static || self.entry_class.is_some());
                if !eligible {
                    continue;
                }
                let method_name = symbol.name.to_string();
                let event = self.nodes.event(&method_name).cloned();
                let name = match &event {
                    Some(_) => udon_event_name(&method_name),
                    None => method_name.clone(),
                };
                if claimed.contains(&name) {
                    continue;
                }
                let parameters = match self.signatures.members.get(&member) {
                    Some(MemberSignature::Function(signature)) => signature.parameters.clone(),
                    _ => Vec::new(),
                };
                let mut arguments = Vec::new();
                match &event {
                    // a built-in event may take its documented arguments —
                    // the runtime writes them into slots named after event and
                    // parameter before raising it — or none, ignoring them
                    Some(event) if !parameters.is_empty() => {
                        let matches = parameters.len() == event.parameters.len()
                            && parameters.iter().zip(&event.parameters).all(
                                |(parameter, expected)| {
                                    self.matches_event_type(
                                        &parameter.parameter_type,
                                        &expected.dotnet_type,
                                    )
                                },
                            );
                        if !matches {
                            let expected = event
                                .parameters
                                .iter()
                                .map(|parameter| {
                                    format!("{} {}", parameter.dotnet_type, parameter.name)
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            let (file, span) = self.declaration_site(member);
                            self.errors.push(CodegenError {
                                message: format!(
                                    "`{method_name}` is the built-in event `{name}`: declare it \
                                     with exactly ({expected}), or with no parameters to ignore \
                                     the event's arguments"
                                ),
                                file,
                                span,
                            });
                            continue;
                        }
                        for parameter in &event.parameters {
                            arguments.push(EventArgument {
                                slot: event_argument_slot(&method_name, &parameter.name),
                                udon_type: event_slot_type(&parameter.dotnet_type),
                                write_back: false,
                            });
                        }
                    }
                    // a custom event with parameters or a result: exported
                    // under the names other programs use to reach it (see
                    // `programs`) — the stub reads the arguments from, and
                    // leaves the result in, the variables of its layout
                    None if !parameters.is_empty() || return_type_of(self, member) => {
                        let exported = self.exported_member_layouts(member);
                        for (layout, key, passing) in exported {
                            self.push_layout_entry(
                                &mut entries,
                                &mut claimed,
                                layout,
                                key,
                                passing,
                            );
                        }
                        continue;
                    }
                    _ => {}
                }
                let key = FunctionKey {
                    symbol: member,
                    role: Role::Method,
                    bindings: Vec::new(),
                };
                self.ensure_function(&key);
                claimed.insert(name.clone());
                entries.push(EventEntry {
                    name,
                    key,
                    arguments,
                    // the one event whose *result* Udon reads back, from
                    // `__returnValue` (UdonSharp does the same copy)
                    returns_value: method_name == "OnOwnershipRequest",
                    result_slot: None,
                });
            }
            // public properties with bodies: their accessors are events too,
            // so another program can read and write them
            if self.entry_class.is_some() {
                let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
                for member in members {
                    if self.declarations.table.symbol(member).kind != SymbolKind::Property {
                        continue;
                    }
                    let exported = self.exported_member_layouts(member);
                    for (layout, key, passing) in exported {
                        self.push_layout_entry(&mut entries, &mut claimed, layout, key, passing);
                    }
                }
            }
        }
        // `[FieldChangeCallback]` fields each get an `_onVarChange_…` entry;
        // collected before the queue drains so their setters get compiled
        let callbacks = self.collect_field_callbacks();
        // `[NetworkCallable]` methods: validated whether or not they were
        // exported (a private one is an error, not silence), and recorded
        // with the variables their arguments arrive in
        self.record_network_callables();

        // a behaviour may legitimately be all public variables and no events —
        // an explicit entry class was asked for by name, so it must have one
        if entries.is_empty() && self.entry_class.is_none() {
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

        // static constructors run at startup whether or not anything else
        // refers to their class; compile them with everything else so their
        // bodies take part in the dispatch fixpoint below
        self.schedule_static_constructors();
        // where every uncaught exception ends: compiled with everything
        // else, since it dispatches `ToString` and `Message`
        let unhandled = self.unhandled_key();
        self.ensure_function(&unhandled);
        // every event drains the continuation queue when its body is done,
        // and the resume event feeds the timed ones into it (see `tasks`)
        self.scheduler_key("__Drain");
        self.scheduler_key("__OnResume");

        // fixpoint: draining the queue may register new types, which may make
        // dispatchers incomplete, which enqueues more functions, ...
        // ... and a dispatcher body may itself schedule functions (the
        // fallbacks it calls) or meet a cast that needs a type test, so the
        // whole thing repeats until nothing is left to emit
        loop {
            loop {
                while let Some(key) = self.queue.pop_front() {
                    self.compile_function(&key);
                }
                // a thunk emits no new function, so this never re-fills the
                // queue — but the invoker it jumps from may still be queued
                self.emit_thunks();
                if !self.ensure_dispatcher_impls() {
                    break;
                }
            }
            if !self.emit_dispatcher_bodies() && self.queue.is_empty() {
                break;
            }
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

        for entry in &entries {
            let name = &entry.name;
            let key = &entry.key;
            self.begin_entry_stub(name, init_label, init_return, initialized);

            // the event's arguments: the runtime wrote them into the named
            // slots before raising the event; hand them to the function
            let function = &self.functions[key];
            let offset = function.parameters.len() - entry.arguments.len();
            let parameter_slots: Vec<DataId> = function.parameters[offset..].to_vec();
            let mut write_backs: Vec<(DataId, DataId)> = Vec::new();
            for (argument, parameter) in entry.arguments.iter().zip(parameter_slots) {
                let slot = self.program.add_data(DataSymbol {
                    name: argument.slot.clone(),
                    udon_type: argument.udon_type.clone(),
                    init: HeapInit::Null,
                    export: false,
                    sync: None,
                });
                self.copy(slot, parameter);
                if argument.write_back {
                    write_backs.push((parameter, slot));
                }
            }

            let function = &self.functions[key];
            let (callee_return, callee_label, callee_result) =
                (function.return_slot, function.label, function.result);
            // the body returns here so an exception it left pending can be
            // reported; then the event's result (if any) goes to
            // `__returnValue`, where Udon reads it, and the event halts
            let done = self.program.add_label(format!("event_{name}__done"));
            let return_to_stub =
                self.code_address_constant(format!("__ret_body_{name}"), Some(done));
            self.copy(return_to_stub, callee_return);
            self.program
                .code
                .push(Op::Jump(Target::Label(callee_label)));
            self.program.code.push(Op::Label(done));
            self.emit_unhandled_check(name);
            // `ref`/`out` parameters and the result, where the caller reads
            // them back
            for (parameter, slot) in write_backs {
                self.copy(parameter, slot);
            }
            if let (Some(result), Some((slot_name, udon_type))) =
                (callee_result, entry.result_slot.clone())
            {
                let slot = self.program.add_data(DataSymbol {
                    name: slot_name,
                    udon_type,
                    init: HeapInit::Null,
                    export: false,
                    sync: None,
                });
                self.copy(result, slot);
            }
            if let Some(result) = callee_result.filter(|_| entry.returns_value) {
                let return_value = self.program.add_data(DataSymbol {
                    name: "__returnValue".into(),
                    udon_type: "SystemObject".into(),
                    init: HeapInit::Null,
                    export: false,
                    sync: None,
                });
                self.copy(result, return_value);
            }
            // continuations of tasks the body completed run now, with the
            // body's frames all returned from
            self.emit_scheduler_call("__Drain", name);
            self.program
                .code
                .push(Op::Jump(Target::Address(HALT_ADDRESS)));
        }

        // the event `SendCustomEventDelayed…` raises for timed continuations
        if self.scheduler_key("__OnResume").is_some() {
            let name = tasks::RESUME_EVENT;
            self.begin_entry_stub(name, init_label, init_return, initialized);
            self.emit_scheduler_call("__OnResume", name);
            self.emit_scheduler_call("__Drain", name);
            self.program
                .code
                .push(Op::Jump(Target::Address(HALT_ADDRESS)));
        }

        for callback in &callbacks {
            self.begin_entry_stub(&callback.name, init_label, init_return, initialized);
            let function = &self.functions[&callback.setter];
            let (setter_label, setter_return, parameter) = (
                function.label,
                function.return_slot,
                function.parameters.last().copied(),
            );
            let Some(parameter) = parameter else {
                // a setter always takes its value; nothing sane to do without
                continue;
            };
            // the runtime already wrote the new value into the field and the
            // previous one into `_old_…`: hand the new value to the setter,
            // put the old value back, and let the setter decide what sticks
            self.copy(callback.field_slot, parameter);
            let old = self.program.add_data(DataSymbol {
                name: callback.old_slot_name.clone(),
                udon_type: callback.udon_type.clone(),
                init: HeapInit::Null,
                export: false,
                sync: None,
            });
            self.copy(old, callback.field_slot);
            let halt = self.code_address_constant(format!("__halt_{}", callback.name), None);
            self.copy(halt, setter_return);
            self.program
                .code
                .push(Op::Jump(Target::Label(setter_label)));
        }

        // the shared static initializer body
        self.emit_static_initializer(init_label, init_return, initialized);
        // a static initializer's expression may call functions scheduled here
        while let Some(key) = self.queue.pop_front() {
            self.compile_function(&key);
        }
        self.emit_thunks();

        self.resolve_frame_markers(init_label);
    }

    /// Opens one exported entry point: label, `.export`, and the check that
    /// runs the static initializer once before anything else.
    fn begin_entry_stub(
        &mut self,
        name: &str,
        init_label: LabelId,
        init_return: DataId,
        initialized: DataId,
    ) {
        let label = self.program.add_label(format!("event_{name}"));
        self.program.entry_points.push(EntryPoint {
            name: name.to_string(),
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
        // a static constructor may have thrown
        self.emit_unhandled_check(&format!("{name}__init"));
    }

    /// `[FieldChangeCallback(nameof(Prop))]` on behaviour fields: when
    /// `SetProgramVariable` or network sync writes the field, the runtime
    /// leaves the previous value in `_old_<slot>` and raises
    /// `_onVarChange_<slot>` — one entry per such field, validated the way
    /// UdonSharp validates them.
    fn collect_field_callbacks(&mut self) -> Vec<FieldCallback> {
        let mut callbacks = Vec::new();
        let mut claimed: HashSet<SymbolId> = HashSet::new();
        for class in self.entry_chain.clone() {
            let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
            for member in members {
                let symbol = self.declarations.table.symbol(member);
                if symbol.kind != SymbolKind::Field || symbol.is_static {
                    continue;
                }
                let Some(property_name) = self.field_change_callback_of(member) else {
                    continue;
                };
                let (file, span) = self.declaration_site(member);
                // the property may live anywhere in the chain, like any member
                let property = self
                    .entry_chain
                    .clone()
                    .into_iter()
                    .flat_map(|class| {
                        self.declarations
                            .table
                            .symbol(class)
                            .members_named(&property_name)
                            .to_vec()
                    })
                    .find(|&candidate| {
                        self.declarations.table.symbol(candidate).kind == SymbolKind::Property
                    });
                let Some(property) = property else {
                    self.errors.push(CodegenError {
                        message: format!(
                            "`[FieldChangeCallback]` names `{property_name}`, but this \
                             behaviour has no property of that name"
                        ),
                        file,
                        span,
                    });
                    continue;
                };
                if !self.property_has_setter(property) {
                    self.errors.push(CodegenError {
                        message: format!(
                            "`{property_name}` has no setter — the callback is a call to \
                             it with the value that was written"
                        ),
                        file,
                        span,
                    });
                    continue;
                }
                let field_type = self.signatures.members.get(&member);
                let property_type = self.signatures.members.get(&property);
                let types_match = matches!(
                    (field_type, property_type),
                    (
                        Some(MemberSignature::Field(field)),
                        Some(MemberSignature::Property(property))
                    ) if field == property
                );
                if !types_match {
                    self.errors.push(CodegenError {
                        message: format!(
                            "`{property_name}` must have the same type as the field: its \
                             setter receives the value that was written"
                        ),
                        file,
                        span,
                    });
                    continue;
                }
                if !claimed.insert(property) {
                    self.errors.push(CodegenError {
                        message: format!(
                            "two fields point their `[FieldChangeCallback]` at \
                             `{property_name}`; a change could not say which field it was"
                        ),
                        file,
                        span,
                    });
                    continue;
                }
                let Some(slot) = self.statics.get(&member).copied() else {
                    continue;
                };
                let slot_name = self.program.data[slot.0].name.clone();
                let udon_type = self.program.data[slot.0].udon_type.clone();
                let setter = FunctionKey {
                    symbol: property,
                    role: Role::Setter,
                    bindings: Vec::new(),
                };
                self.ensure_function(&setter);
                callbacks.push(FieldCallback {
                    name: format!("_onVarChange_{slot_name}"),
                    field_slot: slot,
                    old_slot_name: format!("_old_{slot_name}"),
                    udon_type,
                    setter,
                });
            }
        }
        callbacks
    }

    /// The property name from `[FieldChangeCallback(nameof(Prop))]` (or a
    /// plain string), `None` when the member has no such attribute.
    fn field_change_callback_of(&mut self, member: SymbolId) -> Option<String> {
        for sections in self.attribute_sections(member) {
            for attribute in sections.iter().flat_map(|section| section.attributes) {
                let name = attribute_name(attribute)
                    .map(|spelling| spelling.strip_suffix("Attribute").unwrap_or(spelling));
                if name != Some("FieldChangeCallback") {
                    continue;
                }
                let spelling = attribute
                    .arguments
                    .as_ref()
                    .and_then(|list| list.arguments.first())
                    .and_then(|argument| match &argument.value {
                        ArgumentValue::Expression(expression) => string_literal_of(expression)
                            .or_else(|| nameof_argument(expression).map(str::to_string)),
                        _ => None,
                    });
                if spelling.is_none() {
                    let (file, span) = self.declaration_site(member);
                    self.errors.push(CodegenError {
                        message: "`[FieldChangeCallback]` takes the property to call: write \
                                  `[FieldChangeCallback(nameof(Property))]`"
                            .into(),
                        file,
                        span,
                    });
                }
                return spelling;
            }
        }
        None
    }

    /// Does this source property declare a `set` accessor?
    fn property_has_setter(&self, property: SymbolId) -> bool {
        self.declarations
            .table
            .symbol(property)
            .declarations
            .iter()
            .any(|site| match &site.syntax {
                SyntaxRef::Property(property) => match &property.body {
                    FunctionBody::Accessors(accessors) => accessors
                        .accessors
                        .iter()
                        .any(|accessor| accessor.kind.value == AccessorKind::Set),
                    _ => false,
                },
                _ => false,
            })
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

        // an initializer's expression may meet further static fields; those
        // join the list as it is walked (and are emitted inline where met,
        // see `ensure_static`), so this is an index loop, not an iterator
        self.static_init_phase = true;
        let mut index = 0;
        while index < self.static_init.len() {
            let (field, file) = self.static_init[index];
            index += 1;
            if self.static_init_emitted.insert(field) {
                self.emit_static_field_initializer(field, file);
            }
        }
        self.static_init_phase = false;

        // then every static constructor body (§15.12): after its class's
        // field initializers — all of them ran above — and once
        let constructors = self.static_constructors.clone();
        for key in constructors {
            let (file, span) = self.declaration_site(key.symbol);
            let mut ctx = Ctx {
                key: key.clone(),
                file,
                locals: vec![HashMap::new()],
                boxed: Vec::new(),
                this_slot: None,
                this_type: None,
                loop_stack: Vec::new(),
                result: None,
                return_slot: init_return, // unused
                caught: Vec::new(),
                async_state: None,
                iterator_state: None,
            };
            self.call_function(&mut ctx, &key, None, &[], &[], span);
        }
        self.program.code.push(Op::JumpIndirect(init_return));
    }

    /// Every `static T()` in the compilation, queued for compilation and
    /// remembered for the static initializer. A generic class's static
    /// constructor would need one run per instantiation, which nothing
    /// here models yet — an error rather than a silent skip.
    fn schedule_static_constructors(&mut self) {
        let mut found = Vec::new();
        for (symbol, entry) in self.declarations.table.iter() {
            if entry.kind != SymbolKind::Constructor || !entry.is_static {
                continue;
            }
            let Some(owner) = entry.parent else {
                continue;
            };
            if !self
                .declarations
                .table
                .symbol(owner)
                .type_parameters
                .is_empty()
            {
                let (file, span) = self.declaration_site(symbol);
                self.errors.push(CodegenError {
                    message: "a static constructor of a generic class is not supported by the \
                              Udon backend yet"
                        .into(),
                    file,
                    span,
                });
                continue;
            }
            found.push(FunctionKey {
                symbol,
                role: Role::Constructor,
                bindings: Vec::new(),
            });
        }
        for key in &found {
            self.ensure_function(key);
        }
        self.static_constructors = found;
    }

    fn emit_static_field_initializer(&mut self, field: SymbolId, file: FileId) {
        let symbol = self.declarations.table.symbol(field);
        let Some(site) = symbol.declarations.first() else {
            return;
        };
        let SyntaxRef::Field { declarator, .. } = site.syntax else {
            return;
        };
        let Some(written) = &declarator.initializer else {
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
            boxed: Vec::new(),
            this_slot: None,
            this_type: None,
            loop_stack: Vec::new(),
            result: None,
            return_slot: slot, // unused
            caught: Vec::new(),
            async_state: None,
            iterator_state: None,
        };
        self.emit_function_start_mark(&ctx);
        match written {
            InitializerValue::Expression(value) => {
                if let Some(value_slot) = self.lower_expression(&mut ctx, value) {
                    let _ = ty;
                    self.copy(value_slot, slot);
                }
            }
            // `static int[] Steps = { 1, 2 };`
            InitializerValue::Nested(nested) => {
                if let Some(array) =
                    self.lower_array_shorthand(&mut ctx, &ty, nested, nested.span())
                {
                    self.copy(array, slot);
                }
            }
        }
    }

    /// Which functions each function can (transitively) call.
    fn call_closure(&self) -> HashMap<FunctionKey, HashSet<FunctionKey>> {
        let mut closure = HashMap::new();
        for start in self.call_edges.keys() {
            let mut seen: HashSet<FunctionKey> = HashSet::new();
            let mut stack: Vec<&FunctionKey> = self.call_edges[start].iter().collect();
            while let Some(next) = stack.pop() {
                if seen.insert(next.clone())
                    && let Some(more) = self.call_edges.get(next)
                {
                    stack.extend(more.iter());
                }
            }
            closure.insert(start.clone(), seen);
        }
        closure
    }

    /// Runs after every body is emitted, when the call graph is complete.
    /// Turns the `SaveFrame`/`RestoreFrame` placeholders into what each call
    /// site actually needs:
    ///
    /// - An edge the call could come back through (the callee reaches the
    ///   caller) saves the callee's whole static frame to a heap stack before
    ///   the call and restores it after — so recursion, direct or mutual or
    ///   through virtual dispatch, just works, with no cost on any other call.
    /// - Everything else: the placeholder disappears.
    ///
    /// It also weaves a re-entry guard into every function that can be live
    /// while control is outside this program (an ancestor of a
    /// `SendCustomEvent`/`SetProgramVariable` call): another program calling
    /// back into an active function would corrupt its frame silently — no
    /// analysis of *this* program can see that cycle, so it is caught at
    /// runtime with an error instead.
    fn resolve_frame_markers(&mut self, init_label: LabelId) {
        let closure = self.call_closure();

        let expand: Vec<bool> = self
            .frame_markers
            .iter()
            .map(|(caller, callee)| {
                callee == caller
                    || closure
                        .get(callee)
                        .is_some_and(|reachable| reachable.contains(caller))
            })
            .collect();
        let any_recursion = expand.iter().any(|&needed| needed);

        let guarded: Vec<FunctionKey> = self
            .functions
            .keys()
            .filter(|function| {
                self.external_callers.contains(*function)
                    || closure.get(*function).is_some_and(|reachable| {
                        reachable
                            .iter()
                            .any(|callee| self.external_callers.contains(callee))
                    })
            })
            .cloned()
            .collect();

        if !any_recursion && guarded.is_empty() && self.async_snapshots.is_empty() {
            self.program
                .code
                .retain(|op| !matches!(op, Op::SaveFrame(_) | Op::RestoreFrame(_)));
            return;
        }

        // one guard per function: the `am I already running?` flag and the
        // message logged when the answer is yes
        let mut guards: HashMap<FunctionKey, (DataId, DataId)> = HashMap::new();
        let mut label_guards: HashMap<LabelId, FunctionKey> = HashMap::new();
        let mut return_guards: HashMap<DataId, FunctionKey> = HashMap::new();
        for key in guarded {
            let function = &self.functions[&key];
            let (label, return_slot, name) =
                (function.label, function.return_slot, function.name.clone());
            let flag = self.program.add_data(DataSymbol {
                name: format!("__active_{name}"),
                udon_type: "SystemBoolean".into(),
                init: HeapInit::Boolean(false),
                export: false,
                sync: None,
            });
            let path = self.display_path(key.symbol);
            let message = self.string_constant(&format!(
                "MenSharp: `{path}` was re-entered while it was still running — a \
                 SendCustomEvent/SetProgramVariable call it made came back into it through \
                 another program. The event was aborted to avoid corrupting its variables; \
                 restructure the calls so they do not loop back."
            ));
            // saved and restored with the rest of the frame, so a legitimate
            // recursive activation starts `not running` and the outer one
            // gets its state back
            self.functions
                .get_mut(&key)
                .expect("guarded function exists")
                .frame
                .push(flag);
            guards.insert(key.clone(), (flag, message));
            label_guards.insert(label, key.clone());
            return_guards.insert(return_slot, key);
        }

        let frame_of: HashMap<FunctionKey, Vec<DataId>> = self
            .frame_markers
            .iter()
            .zip(&expand)
            .filter(|(_, needed)| **needed)
            .map(|((_, callee), _)| (callee.clone(), self.functions[callee].frame.clone()))
            .collect();
        // what an `await` copies out of (and a resume copies back into) a
        // suspended function: its frame, complete by now
        let snapshot_frames: HashMap<FunctionKey, Vec<DataId>> = self
            .async_snapshots
            .iter()
            .map(|(key, _)| (key.clone(), self.functions[key].frame.clone()))
            .collect();

        const SET: &str = "SystemObjectArray.__Set__SystemInt32_SystemObject__SystemVoid";
        const GET: &str = "SystemObjectArray.__Get__SystemInt32__SystemObject";
        const ADD: &str = "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32";
        const SUB: &str = "SystemInt32.__op_Subtraction__SystemInt32_SystemInt32__SystemInt32";
        const CTOR: &str = "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray";
        const LOG_ERROR: &str = "UnityEngineDebug.__LogError__SystemObject__SystemVoid";

        let stack = self.program.add_data(DataSymbol {
            name: "__recursion_stack".into(),
            udon_type: "SystemObjectArray".into(),
            init: HeapInit::Null,
            export: false,
            sync: None,
        });
        let top = self.program.add_data(DataSymbol {
            name: "__recursion_top".into(),
            udon_type: "SystemInt32".into(),
            init: HeapInit::Int32(0),
            export: false,
            sync: None,
        });
        let one = self.int_constant(1);
        let size = self.int_constant(4096);
        let true_constant = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        let false_constant = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));

        let code = std::mem::take(&mut self.program.code);
        let mut out: Vec<Op> = Vec::with_capacity(code.len());
        for op in code {
            match op {
                Op::SaveFrame(id) => {
                    if !expand[id as usize] {
                        continue;
                    }
                    let (_, callee) = &self.frame_markers[id as usize];
                    for &slot in &frame_of[callee] {
                        // stack[top] = slot; top += 1
                        out.push(Op::Push(stack));
                        out.push(Op::Push(top));
                        out.push(Op::Push(slot));
                        out.push(Op::Extern(SET.into()));
                        out.push(Op::Push(top));
                        out.push(Op::Push(one));
                        out.push(Op::Push(top));
                        out.push(Op::Extern(ADD.into()));
                    }
                    if let Some((flag, _)) = guards.get(callee) {
                        out.push(Op::Push(false_constant));
                        out.push(Op::Push(*flag));
                        out.push(Op::Copy);
                    }
                }
                Op::SnapshotFrame(id) => {
                    let (key, slot) = self.async_snapshots[id as usize].clone();
                    let frame = snapshot_frames[&key].clone();
                    let count = self.int_constant(frame.len() as i32);
                    out.push(Op::Push(count));
                    out.push(Op::Push(slot));
                    out.push(Op::Extern(CTOR.into()));
                    for (index, source) in frame.iter().enumerate() {
                        let at = self.int_constant(index as i32);
                        out.push(Op::Push(slot));
                        out.push(Op::Push(at));
                        out.push(Op::Push(*source));
                        out.push(Op::Extern(SET.into()));
                    }
                }
                Op::RestoreSnapshot(id) => {
                    let (key, slot) = self.async_snapshots[id as usize].clone();
                    let frame = snapshot_frames[&key].clone();
                    for (index, target) in frame.iter().enumerate() {
                        let at = self.int_constant(index as i32);
                        out.push(Op::Push(slot));
                        out.push(Op::Push(at));
                        out.push(Op::Push(*target));
                        out.push(Op::Extern(GET.into()));
                    }
                }
                Op::RestoreFrame(id) => {
                    if !expand[id as usize] {
                        continue;
                    }
                    let (_, callee) = &self.frame_markers[id as usize];
                    for &slot in frame_of[callee].iter().rev() {
                        // top -= 1; slot = stack[top]
                        out.push(Op::Push(top));
                        out.push(Op::Push(one));
                        out.push(Op::Push(top));
                        out.push(Op::Extern(SUB.into()));
                        out.push(Op::Push(stack));
                        out.push(Op::Push(top));
                        out.push(Op::Push(slot));
                        out.push(Op::Extern(GET.into()));
                    }
                }
                Op::Label(label) => {
                    out.push(Op::Label(label));
                    if label == init_label && any_recursion {
                        // the stack itself, made once with the statics
                        out.push(Op::Push(size));
                        out.push(Op::Push(stack));
                        out.push(Op::Extern(CTOR.into()));
                    }
                    if let Some(key) = label_guards.get(&label) {
                        let (flag, message) = guards[key];
                        let ok = self
                            .program
                            .add_label(format!("not_reentered_{}", self.functions[key].name));
                        out.push(Op::Push(flag));
                        out.push(Op::JumpIfFalse(Target::Label(ok)));
                        out.push(Op::Push(message));
                        out.push(Op::Extern(LOG_ERROR.into()));
                        out.push(Op::Jump(Target::Address(HALT_ADDRESS)));
                        out.push(Op::Label(ok));
                        out.push(Op::Push(true_constant));
                        out.push(Op::Push(flag));
                        out.push(Op::Copy);
                    }
                }
                Op::JumpIndirect(slot) if return_guards.contains_key(&slot) => {
                    let (flag, _) = guards[&return_guards[&slot]];
                    out.push(Op::Push(false_constant));
                    out.push(Op::Push(flag));
                    out.push(Op::Copy);
                    out.push(Op::JumpIndirect(slot));
                }
                other => out.push(other),
            }
        }
        self.program.code = out;
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

    /// Everything the earlier phases produced, as the checker sees it.
    pub(super) fn type_system(&self) -> men_sharp_semantics::TypeSystem<'_, 'ast> {
        men_sharp_semantics::TypeSystem {
            declarations: self.declarations,
            signatures: self.signatures,
            external: self.external,
        }
    }

    /// The same path as [`Generator::symbol_path`], but spelled the way the
    /// user wrote it — for diagnostics, where a mangled name means nothing.
    /// A type as the user would write it, for diagnostics.
    pub(super) fn display_type(&self, ty: &Type) -> String {
        self.type_system().display(ty)
    }

    fn display_path(&self, symbol: SymbolId) -> String {
        let mut parts = Vec::new();
        let mut current = Some(symbol);
        while let Some(id) = current {
            let entry = self.declarations.table.symbol(id);
            if !entry.name.is_empty() {
                parts.push(entry.name);
            }
            current = entry.parent;
        }
        parts.reverse();
        parts.join(".")
    }

    fn constant(&mut self, udon_type: &str, repr: &str, init: HeapInit) -> DataId {
        // keyed by the value's kind too: the string literal "null" and a
        // null string slot are both spelled `null` by their callers
        let key = (
            udon_type.to_string(),
            format!("{:?}:{repr}", std::mem::discriminant(&init)),
        );
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

    /// A `%SystemType` constant naming an external type. Udon has no generics:
    /// a type argument travels as a value, so `GetComponent<Rigidbody>()`
    /// needs `typeof(Rigidbody)` sitting in the heap. The constant carries the
    /// .NET name, which is what the Unity importer can resolve back to a real
    /// `System.Type`.
    /// The declared symbol behind a type, when it is an enum from source.
    pub(super) fn source_enum(&self, ty: &Type) -> Option<SymbolId> {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if self.declarations.table.symbol(*symbol).kind == SymbolKind::Enum => Some(*symbol),
            _ => None,
        }
    }

    /// Is this an enum from a referenced assembly (`KeyCode`, `VideoError`)?
    /// Unlike a source enum — a plain `Int32` slot — its values are *boxed*
    /// enum objects, because externs unbox them by their real type.
    pub(super) fn external_enum(&self, ty: &Type) -> Option<men_sharp_semantics::ExternalTypeId> {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } if self.external.type_info(*id).kind
                == men_sharp_semantics::ExternalTypeKind::Enum =>
            {
                Some(*id)
            }
            _ => None,
        }
    }

    /// A source enum member's underlying value: an explicit integer literal,
    /// or counting up from the previous member, as C# does.
    fn enum_member_value(&mut self, member: SymbolId) -> Option<i64> {
        let parent = self.declarations.table.symbol(member).parent?;
        let members: Vec<SymbolId> = self.declarations.table.symbol(parent).members.to_vec();
        let mut value: i64 = 0;
        for candidate in members {
            let symbol = self.declarations.table.symbol(candidate);
            if symbol.kind != SymbolKind::EnumMember {
                continue;
            }
            let initializer = symbol
                .declarations
                .first()
                .and_then(|site| match &site.syntax {
                    SyntaxRef::EnumMember(node) => node.value.as_ref(),
                    _ => None,
                });
            if let Some(expression) = initializer {
                match literal_heap_init(expression, "SystemInt32") {
                    Some(HeapInit::Int32(explicit)) => value = explicit as i64,
                    _ => {
                        let (file, span) = self.declaration_site(candidate);
                        self.errors.push(CodegenError {
                            message: "an enum member's value must be an integer literal \
                                      for the Udon backend"
                                .into(),
                            file,
                            span,
                        });
                        return None;
                    }
                }
            }
            if candidate == member {
                return Some(value);
            }
            value += 1;
        }
        None
    }

    /// A member access that is a compile-time constant — a source or external
    /// enum member, or an external `const` field — as a heap slot. `None`
    /// when the member is not a constant.
    pub(super) fn member_constant(&mut self, member: &ResolvedMember) -> Option<(DataId, Type)> {
        match &member.origin {
            MemberOrigin::Source(symbol) => {
                if member.kind != SymbolKind::EnumMember {
                    return None;
                }
                let value = self.enum_member_value(*symbol)?;
                Some((self.int_constant(value as i32), member.member_type.clone()))
            }
            MemberOrigin::External {
                member: external, ..
            } => {
                let constant = external.constant.clone()?;
                let ty = member.member_type.clone();
                let slot = self.typed_constant(&constant, &ty)?;
                Some((slot, ty))
            }
            // a local function is never a constant member access
            MemberOrigin::LocalFunction(_) => None,
        }
    }

    /// A metadata constant as a heap slot of the given type: a const field's
    /// value, or an optional parameter's default.
    pub(super) fn typed_constant(
        &mut self,
        constant: &men_sharp_semantics::ExternalConstant,
        ty: &Type,
    ) -> Option<DataId> {
        use men_sharp_semantics::ExternalConstant;
        {
            {
                // an enum constant must be the real boxed value — an Int32 in
                // an enum-typed slot throws when an extern unboxes it — and
                // only the Unity importer can build one (HeapInit::EnumValue)
                if let Some(id) = self.external_enum(ty) {
                    let value = match constant {
                        ExternalConstant::Int(value) => *value,
                        ExternalConstant::UInt(value) => *value as i64,
                        _ => return None,
                    };
                    let dotnet_type = self.external.display_name(id);
                    let udon_type = self.heap_type(ty);
                    let slot = self.constant(
                        &udon_type,
                        &format!("{dotnet_type}#{value}"),
                        HeapInit::EnumValue { dotnet_type, value },
                    );
                    return Some(slot);
                }
                let slot = match constant {
                    ExternalConstant::Int(value) => match self.heap_type(ty).as_str() {
                        "SystemInt64" => self.constant(
                            "SystemInt64",
                            &value.to_string(),
                            HeapInit::Int64(*value),
                        ),
                        _ => self.int_constant(*value as i32),
                    },
                    ExternalConstant::UInt(value) => match self.heap_type(ty).as_str() {
                        "SystemUInt32" => self.constant(
                            "SystemUInt32",
                            &value.to_string(),
                            HeapInit::UInt32(*value as u32),
                        ),
                        _ => self.int_constant(*value as i32),
                    },
                    ExternalConstant::Single(value) => self.constant(
                        "SystemSingle",
                        &format!("{value:?}"),
                        HeapInit::Single(*value),
                    ),
                    ExternalConstant::Double(value) => self.constant(
                        "SystemDouble",
                        &format!("{value:?}"),
                        HeapInit::Double(*value),
                    ),
                    ExternalConstant::Boolean(value) => self.constant(
                        "SystemBoolean",
                        &value.to_string(),
                        HeapInit::Boolean(*value),
                    ),
                    ExternalConstant::Char(value) => self.constant(
                        "SystemChar",
                        &(*value as u32).to_string(),
                        HeapInit::Char(*value),
                    ),
                    ExternalConstant::String(value) => self.string_constant(value),
                };
                Some(slot)
            }
        }
    }

    pub(super) fn type_constant(&mut self, ty: &Type) -> Option<DataId> {
        let Type::Named {
            target: TypeTarget::External(id),
            ..
        } = ty
        else {
            return None;
        };
        let name = self.external.display_name(*id);
        Some(self.constant("SystemType", &name, HeapInit::TypeOf(name.clone())))
    }

    /// `typeof(object[])` — what every M# object is at runtime, and the first
    /// thing a type test checks before reading a type id out of one.
    pub(super) fn object_array_type_constant(&mut self) -> DataId {
        let name = "System.Object[]";
        self.constant("SystemType", name, HeapInit::TypeOf(name.to_string()))
    }

    /// A type's extern spelling, then its base classes' — what an operator or
    /// member declared further up is named by.
    pub(super) fn external_chain(&self, ty: &Type) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(name) = self.extern_type_name(ty) {
            names.push(name);
        }
        let Type::Named {
            target: TypeTarget::External(id),
            ..
        } = ty
        else {
            return names;
        };
        let mut current = self.external.base_type(*id);
        while let Some(base) = current {
            let Some(name) = self.extern_type_name(&base) else {
                break;
            };
            if names.contains(&name) {
                break;
            }
            names.push(name);
            current = match &base {
                Type::Named {
                    target: TypeTarget::External(id),
                    ..
                } => self.external.base_type(*id),
                _ => None,
            };
        }
        names
    }

    /// Does a value of this type live behind a reference? Structs and enums do
    /// not, and comparing two boxed copies of one by identity would be wrong.
    pub(super) fn is_reference_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => matches!(
                self.external.type_info(*id).kind,
                men_sharp_semantics::ExternalTypeKind::Class
                    | men_sharp_semantics::ExternalTypeKind::Interface
                    | men_sharp_semantics::ExternalTypeKind::Delegate
            ),
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => !matches!(
                self.declarations.table.symbol(*symbol).kind,
                SymbolKind::Struct | SymbolKind::RecordStruct | SymbolKind::Enum
            ),
            Type::Array { .. } => true,
            _ => false,
        }
    }

    /// `System.Type`, the type of a `typeof(...)` expression.
    pub(super) fn system_type(&self) -> Type {
        match self.external.find_type(&["System"], "Type", 0) {
            Some(id) => Type::Named {
                target: TypeTarget::External(id),
                arguments: Vec::new(),
            },
            None => Type::Error,
        }
    }

    /// A `%SystemString` constant — the currency of Udon's cross-program
    /// calls, which address everything by name.
    pub(super) fn string_constant(&mut self, value: &str) -> DataId {
        self.constant("SystemString", value, HeapInit::Str(value.to_string()))
    }

    fn int_constant(&mut self, value: i32) -> DataId {
        self.constant("SystemInt32", &value.to_string(), HeapInit::Int32(value))
    }

    fn temp(&mut self, udon_type: &str) -> DataId {
        self.temp_counter += 1;
        let name = format!("__t{}", self.temp_counter);
        let slot = self.program.add_data(DataSymbol {
            name,
            udon_type: udon_type.to_string(),
            init: HeapInit::Null,
            export: false,
            sync: None,
        });
        // a temp allocated while a body compiles is part of that function's
        // static frame — what a re-entrant call has to save
        if let Some(key) = &self.current_frame
            && let Some(function) = self.functions.get_mut(key)
        {
            function.frame.push(slot);
        }
        slot
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
        // a call into another program can synchronously come back into this
        // one — remember who makes them, so their ancestors get re-entry
        // guards (see resolve_frame_markers)
        if (signature.contains("SendCustomEvent") && !signature.contains("SendCustomEventDelayed"))
            || signature.contains("SetProgramVariable")
        {
            self.external_callers.insert(ctx.key.clone());
        }
        // where this extern is in the source: an extern's own exception halts
        // the VM at this address, and the sidecar table maps it back
        self.emit_source_mark(ctx, &span);
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

    /// The type the checker recorded for a node that is not a whole
    /// expression — a tuple written inline, say.
    fn type_of_node(&self, ctx: &Ctx, node: EntityID) -> Type {
        let ty = self
            .bodies
            .expression_types
            .get(&node)
            .cloned()
            .unwrap_or(Type::Error);
        self.substitute(&ty, &ctx.key.bindings)
    }

    /// The declared Udon heap type for a slot of this M# type.
    fn heap_type(&self, ty: &Type) -> String {
        match ty {
            // another program (an UdonSharp behaviour's class included): what
            // a scalar slot may hold is the concrete UdonBehaviour, the one
            // thing a `this` reference resolves into
            Type::Named { .. } if self.is_program_reference(ty) => BEHAVIOUR_HEAP_TYPE.into(),
            // a delegate is an `object[]` of the compiler's own making
            Type::Named { .. } if self.is_delegate_type(ty) => "SystemObjectArray".into(),
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } => {
                let mut name = mangle_dotnet_name(&self.external.display_name(*id));
                for argument in arguments {
                    name.push_str(&self.heap_type_component(argument));
                }
                // UdonBehaviour has no node of its own — nothing is declared
                // *on* it — but the assembler resolves the name, and it is the
                // only thing a `this` reference may be stored as
                if name == BEHAVIOUR_HEAP_TYPE || self.nodes.has_type(&name) {
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
            // a tuple is an `object[]`, like a struct
            Type::Tuple(_) => "SystemObjectArray".into(),
            Type::Array { element, rank: 1 } => {
                let name = format!("{}Array", self.heap_type_component(element));
                if self.nodes.has_type(&name) {
                    name
                } else {
                    "SystemObjectArray".into()
                }
            }
            // a `T?` of a value type: the boxed value or null (see `nullable`);
            // a reference annotation is the type itself
            Type::Nullable(inner) => {
                if self.nullable_inner(ty).is_some() {
                    "SystemObject".into()
                } else {
                    self.heap_type(inner)
                }
            }
            _ => "SystemObject".into(),
        }
    }

    fn heap_type_component(&self, ty: &Type) -> String {
        // arrays are named after the interface, which has a whitelisted array
        // type and the Get/Set/get_Length externs to go with it. An
        // UdonBehaviour[] fits one by array covariance; only a *scalar* slot
        // has to be the concrete UdonBehaviour, for `this` to resolve into it
        if !matches!(ty, Type::Array { .. }) && self.is_program_reference(ty) {
            return BEHAVIOUR_EXTERN_TYPE.into();
        }
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

    /// Does this M# type name the same thing as an event parameter's .NET
    /// full name? Arrays compare structurally: `[]` would vanish in the
    /// name mangling and collide with the element type.
    fn matches_event_type(&self, ty: &Type, dotnet: &str) -> bool {
        if let Some(element_name) = dotnet.strip_suffix("[]") {
            if let Type::Array { element, rank: 1 } = ty {
                return self.matches_event_type(element, element_name);
            }
            return false;
        }
        self.extern_type_name(ty)
            .is_some_and(|name| name == mangle_dotnet_name(dotnet))
    }

    /// The type's spelling inside an extern signature; `None` when the type
    /// cannot appear there (user types, unresolved parameters).
    fn extern_type_name(&self, ty: &Type) -> Option<String> {
        // a behaviour appears in extern signatures as the interface
        if !matches!(ty, Type::Array { .. }) && self.is_program_reference(ty) {
            return Some(BEHAVIOUR_EXTERN_TYPE.into());
        }
        if let Type::Named {
            target: TypeTarget::External(id),
            ..
        } = ty
            && mangle_dotnet_name(&self.external.display_name(*id)) == BEHAVIOUR_HEAP_TYPE
        {
            return Some(BEHAVIOUR_EXTERN_TYPE.into());
        }
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
        // a tuple carries a type id like a struct, so a boxed one is still
        // recognisable — it just has no members, only positions
        if let Type::Tuple(elements) = ty {
            let type_id = self.type_order.len() as i32;
            let layout = Layout {
                type_id,
                size: elements.len() + 1,
                slots: HashMap::new(),
            };
            self.layouts.insert(ty.clone(), layout.clone());
            self.type_order.push(ty.clone());
            return Some(layout);
        }
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = ty
        else {
            return None;
        };
        let symbol = *symbol;
        // a type that cannot be compiled, or that is an engine object in
        // disguise, has no layout worth building: the error names why
        if let Some(reason) = self.uncompilable_reason(symbol) {
            let (file, span) = self.declaration_site(symbol);
            let name = self.display_path(symbol);
            self.errors.push(CodegenError {
                message: format!(
                    "`{name}` is used from MenSharp code, but MenSharp cannot compile it: \
                     {reason}"
                ),
                file,
                span,
            });
            return None;
        }
        if self.declarations.table.symbol(symbol).kind == SymbolKind::Class
            && !self.is_program_reference(ty)
            && let Some(base) = self.engine_base_of(symbol)
        {
            let (file, span) = self.declaration_site(symbol);
            let name = self.display_path(symbol);
            self.errors.push(CodegenError {
                message: format!(
                    "`{name}` derives from `{base}`, an engine class: Udon can neither create \
                     nor hold such an object, so the class cannot be used from a program \
                     (an UdonSharp behaviour can — through a reference to it)"
                ),
                file,
                span,
            });
            return None;
        }
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
                SymbolKind::Field | SymbolKind::Event => true,
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
        // `{ get; }` on an abstract or interface property declares no storage:
        // the accessor is dispatched, and the implementing type owns the value
        if self.is_bodiless(symbol) {
            return false;
        }
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
            if !matches!(symbol.kind, SymbolKind::Field | SymbolKind::Event) || !symbol.is_static {
                continue;
            }
            self.ensure_static(member, export);
        }
    }

    /// One exported entry from a member's export layout: the event under the
    /// layout's name, its arguments from the layout's parameter variables,
    /// its result into the layout's result variable.
    fn push_layout_entry(
        &mut self,
        entries: &mut Vec<EventEntry>,
        claimed: &mut HashSet<String>,
        layout: programs::ExportLayout,
        key: FunctionKey,
        passing: Vec<men_sharp_semantics::ParameterPassing>,
    ) {
        if !claimed.insert(layout.event.clone()) {
            return;
        }
        self.ensure_function(&key);
        let (parameter_types, return_type) = self.function_shape(&key);
        let has_this = self.function_has_this(&key);
        let value_parameters = &parameter_types[usize::from(has_this)..];
        let arguments = layout
            .parameters
            .iter()
            .zip(value_parameters)
            .enumerate()
            .map(|(index, (slot, ty))| EventArgument {
                slot: slot.clone(),
                udon_type: self.heap_type(ty),
                write_back: matches!(
                    passing.get(index),
                    Some(
                        men_sharp_semantics::ParameterPassing::Ref
                            | men_sharp_semantics::ParameterPassing::Out
                    )
                ),
            })
            .collect();
        let result_slot = layout
            .result
            .filter(|_| return_type != Type::Void)
            .map(|name| (name, self.heap_type(&return_type)));
        entries.push(EventEntry {
            name: layout.event,
            key,
            arguments,
            returns_value: false,
            result_slot,
        });
    }

    /// A behaviour class and the classes it inherits from, most derived first,
    /// stopping before `MenSharpBehaviour` itself (whose members are self
    /// references, not storage).
    fn behaviour_chain(&self, entry: SymbolId) -> Vec<SymbolId> {
        let mut chain = Vec::new();
        let mut current = Some(entry);
        while let Some(class) = current {
            if Some(class) == self.marker || chain.contains(&class) {
                break;
            }
            chain.push(class);
            current = self
                .signatures
                .base_types
                .get(&class)
                .into_iter()
                .flatten()
                .find_map(|base| match base {
                    Type::Named {
                        target: TypeTarget::Source(symbol),
                        ..
                    } => Some(*symbol),
                    _ => None,
                });
        }
        chain
    }

    /// On a behaviour entry class, instance fields and auto-properties become
    /// exported heap slots — the program's public variables. They reuse the
    /// `statics` machinery: the behaviour has exactly one instance, so its
    /// inherited fields are just as much the program's state as its own.
    ///
    /// Declared base-first so the inspector lists inherited variables above
    /// the ones the leaf class adds.
    fn collect_entry_instance_fields(&mut self, class: SymbolId) {
        let mut chain = self.behaviour_chain(class);
        chain.reverse();
        let mut exported: HashMap<String, SymbolId> = HashMap::new();
        for class in chain {
            let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
            for member in members {
                let symbol = self.declarations.table.symbol(member);
                if symbol.is_static {
                    continue;
                }
                let stores_value = match symbol.kind {
                    SymbolKind::Field | SymbolKind::Event => true,
                    SymbolKind::Property => self.is_auto_property(member),
                    _ => false,
                };
                if !stores_value {
                    continue;
                }
                // exported slots are named after the member, so two members
                // with one name would collide into one public variable
                if self.is_public_variable(member)
                    && let Some(&first) = exported.get(symbol.name)
                {
                    let owner = self
                        .declarations
                        .table
                        .symbol(first)
                        .parent
                        .map(|parent| self.declarations.table.symbol(parent).name)
                        .unwrap_or("a base class");
                    let name = symbol.name;
                    let (file, span) = self.declaration_site(member);
                    self.errors.push(CodegenError {
                        message: format!(
                            "`{name}` hides the `{name}` declared in `{owner}`: a behaviour's \
                             public variables share one namespace on Udon, so the name must \
                             be unique across the class hierarchy"
                        ),
                        file,
                        span,
                    });
                    continue;
                }
                exported.insert(symbol.name.to_string(), member);
                let export = self.is_public_variable(member);
                self.ensure_static(member, export);
            }
        }
    }

    /// The `.sync` mode `[UdonSynced]` asks for, or `None` when the member is
    /// not synced. The mode is read from how the argument is *spelled*
    /// (`UdonSyncMode.Linear`): attributes are not resolved as expressions, and
    /// an unrecognised spelling is reported rather than guessed at.
    fn sync_mode_of(&mut self, member: SymbolId) -> Option<String> {
        for sections in self.attribute_sections(member) {
            for attribute in sections.iter().flat_map(|section| section.attributes) {
                if attribute_name(attribute) != Some("UdonSynced") {
                    continue;
                }
                let argument = attribute
                    .arguments
                    .as_ref()
                    .and_then(|list| list.arguments.first());
                let Some(argument) = argument else {
                    return Some("none".into());
                };
                let spelling = match &argument.value {
                    ArgumentValue::Expression(expression) => last_name_of(expression),
                    _ => None,
                };
                return match spelling {
                    Some("None") | None => Some("none".into()),
                    Some("Linear") => Some("linear".into()),
                    Some("Smooth") => Some("smooth".into()),
                    Some(other) => {
                        let (file, span) = self.declaration_site(member);
                        self.errors.push(CodegenError {
                            message: format!(
                                "`{other}` is not a sync mode; use UdonSyncMode.None, \
                                 UdonSyncMode.Linear or UdonSyncMode.Smooth"
                            ),
                            file,
                            span,
                        });
                        Some("none".into())
                    }
                };
            }
        }
        None
    }

    /// The extern signature a corlib function stands for, from
    /// `[UdonExtern("Owner.__Name__Params__Ret")]`.
    fn udon_extern_of(&self, member: SymbolId) -> Option<String> {
        for sections in self.attribute_sections(member) {
            for attribute in sections.iter().flat_map(|section| section.attributes) {
                if attribute_name(attribute) != Some("UdonExtern") {
                    continue;
                }
                let argument = attribute
                    .arguments
                    .as_ref()
                    .and_then(|list| list.arguments.first())?;
                let ArgumentValue::Expression(expression) = &argument.value else {
                    return None;
                };
                return string_literal_of(expression);
            }
        }
        None
    }

    /// The behaviour-wide sync mode from `[UdonBehaviourSyncMode(...)]`, or
    /// `None` to leave the UdonBehaviour's setting alone.
    fn behaviour_sync_mode(&mut self, class: SymbolId) -> Option<String> {
        for sections in self.attribute_sections(class) {
            for attribute in sections.iter().flat_map(|section| section.attributes) {
                if attribute_name(attribute) != Some("UdonBehaviourSyncMode") {
                    continue;
                }
                let spelling = attribute
                    .arguments
                    .as_ref()
                    .and_then(|list| list.arguments.first())
                    .and_then(|argument| match &argument.value {
                        ArgumentValue::Expression(expression) => last_name_of(expression),
                        _ => None,
                    });
                return match spelling {
                    Some("Continuous") => Some("continuous".into()),
                    Some("Manual") => Some("manual".into()),
                    Some("None") => Some("none".into()),
                    other => {
                        let (file, span) = self.declaration_site(class);
                        self.errors.push(CodegenError {
                            message: format!(
                                "`{}` is not a behaviour sync mode; use \
                                 BehaviourSyncMode.Continuous, .Manual or .None",
                                other.unwrap_or("(nothing)")
                            ),
                            file,
                            span,
                        });
                        None
                    }
                };
            }
        }
        None
    }

    /// Every attribute section written on a member, across its declarations.
    fn attribute_sections(
        &self,
        member: SymbolId,
    ) -> Vec<&'ast [men_sharp_parser::ast::AttributeSection<'ast, 'ast>]> {
        self.declarations
            .table
            .symbol(member)
            .declarations
            .iter()
            .filter_map(|site| match &site.syntax {
                SyntaxRef::Field { field, .. } => Some(field.attributes),
                SyntaxRef::Property(property) => Some(property.attributes),
                SyntaxRef::Method(method) => Some(method.attributes),
                SyntaxRef::Class(class) => Some(class.attributes),
                _ => None,
            })
            .collect()
    }

    /// Does this behaviour member become a public variable — something the
    /// inspector shows, the proxy's values transfer into, and anything on the
    /// network may write? The rule is Unity's own serialization rule, which
    /// is also what UdonSharp exports: `public` opts in, `[SerializeField]`
    /// opts a non-public field in, `[NonSerialized]` opts a public field out.
    /// A plain private field is an implementation detail: exporting it would
    /// put it in the UdonBehaviour's variable table, where it can be
    /// overwritten by everything from the inspector to another program.
    pub(super) fn is_public_variable(&self, member: SymbolId) -> bool {
        if self.has_attribute(member, "NonSerialized") {
            return false;
        }
        // a delegate is this program's own code addresses: nothing the
        // inspector could set, nothing another program could use
        let ty = match self.signatures.members.get(&member) {
            Some(MemberSignature::Field(ty))
            | Some(MemberSignature::Property(ty))
            | Some(MemberSignature::Event(ty)) => ty.clone(),
            _ => Type::Error,
        };
        // ... and a `T?` is a boxed value or null in an `object` slot, which
        // the inspector has no editor for either (Unity does not serialize
        // nullable fields at all)
        if self.is_delegate_type(&ty) || self.nullable_inner(&ty).is_some() {
            return false;
        }
        // ... and a task holds continuations, which are code addresses too
        if self.is_task_type(&ty) {
            return false;
        }
        // ... and Unity serializes neither an array of arrays nor a tuple,
        // so those fields are the program's own, not the inspector's
        if matches!(ty, Type::Tuple(_)) {
            return false;
        }
        if let Type::Array { element, .. } = &ty
            && matches!(**element, Type::Array { .. })
        {
            return false;
        }
        self.declarations.table.symbol(member).accessibility == Accessibility::Public
            || self.has_attribute(member, "SerializeField")
    }

    /// Is an attribute of this name (with or without the `Attribute` suffix)
    /// written on the member? By spelling, like every attribute here.
    fn has_attribute(&self, member: SymbolId, name: &str) -> bool {
        self.attribute_sections(member).iter().any(|sections| {
            sections
                .iter()
                .flat_map(|section| section.attributes)
                .any(|attribute| {
                    attribute_name(attribute)
                        .map(|spelling| spelling.strip_suffix("Attribute").unwrap_or(spelling))
                        == Some(name)
                })
        })
    }

    /// Where a symbol was declared, for diagnostics that have no expression to
    /// point at.
    pub(super) fn declaration_site(&self, symbol: SymbolId) -> (FileId, Range<usize>) {
        match self.declarations.table.symbol(symbol).declarations.first() {
            Some(site) => {
                let span = match &site.syntax {
                    SyntaxRef::Field { declarator, .. } => declarator.span.clone(),
                    SyntaxRef::Property(property) => property.span.clone(),
                    SyntaxRef::Method(method) => method.span.clone(),
                    _ => 0..0,
                };
                (site.file, span)
            }
            None => (FileId(0), 0..0),
        }
    }

    /// Does this member belong to the one instance the behaviour program is —
    /// its entry class or any class that entry class inherits from?
    pub(super) fn is_entry_member(&self, member: SymbolId) -> bool {
        let Some(parent) = self.declarations.table.symbol(member).parent else {
            return false;
        };
        // MenSharpBehaviour's own members count too: `RequestSerialization()`
        // is a method on the program, not on an object, so like everything in
        // the entry chain it is compiled without a `this`. (Its storage is
        // still a self reference — see self_reference_slot, which runs first.)
        self.entry_chain.contains(&parent)
            || (Some(parent) == self.marker && !self.entry_chain.is_empty())
    }

    /// The most derived declaration of a behaviour member. A behaviour has
    /// exactly one instance, so its dynamic type is known at compile time:
    /// a virtual call on it resolves here instead of through a dispatcher.
    pub(super) fn entry_override(&self, member: SymbolId) -> SymbolId {
        let entry = self.declarations.table.symbol(member);
        let (name, kind) = (entry.name, entry.kind);
        let wanted = self.signatures.members.get(&member);
        for &class in &self.entry_chain {
            for &candidate in &self.declarations.table.symbol(class).members {
                if candidate == member {
                    return member;
                }
                let symbol = self.declarations.table.symbol(candidate);
                if symbol.name != name || symbol.kind != kind || symbol.is_static {
                    continue;
                }
                let found = self.signatures.members.get(&candidate);
                let compatible = match (wanted, found) {
                    (
                        Some(MemberSignature::Function(wanted)),
                        Some(MemberSignature::Function(found)),
                    ) => {
                        wanted.parameters.len() == found.parameters.len()
                            && wanted
                                .parameters
                                .iter()
                                .zip(&found.parameters)
                                .all(|(a, b)| a.parameter_type == b.parameter_type)
                    }
                    (
                        Some(MemberSignature::Property(wanted)),
                        Some(MemberSignature::Property(found)),
                    ) => wanted == found,
                    _ => false,
                };
                if compatible {
                    return candidate;
                }
            }
        }
        member
    }

    /// The heap slot for a member declared directly on `MenSharpBehaviour`
    /// (`gameObject`, `transform`), or `None` for anything else.
    ///
    /// Udon has no `this` and no extern that returns a program's own object,
    /// so these are not calls: each gets a private slot whose initial value is
    /// the assembler's `this` literal, and the UdonBehaviour fills it with the
    /// matching thing on its own GameObject before the first event. From there
    /// on it is an ordinary reference — `gameObject.SetActive(false)` is the
    /// same extern it would be on any other GameObject.
    pub(super) fn self_reference_slot(&mut self, member: SymbolId) -> Option<DataId> {
        let marker = self.marker?;
        let symbol = self.declarations.table.symbol(member);
        if symbol.parent != Some(marker) {
            return None;
        }
        if let Some(&slot) = self.statics.get(&member) {
            return Some(slot);
        }
        let ty = match self.signatures.members.get(&member) {
            Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => ty.clone(),
            _ => Type::Error,
        };
        let name = format!("__this_{}", symbol.name);
        let udon_type = self.heap_type(&ty);
        let slot = self.program.add_data(DataSymbol {
            name,
            udon_type,
            init: HeapInit::SelfReference,
            export: false,
            sync: None,
        });
        self.statics.insert(member, slot);
        Some(slot)
    }

    fn ensure_static(&mut self, field: SymbolId, export: bool) -> DataId {
        if let Some(&slot) = self.statics.get(&field) {
            return slot;
        }
        let ty = match self.signatures.members.get(&field) {
            Some(MemberSignature::Field(ty))
            | Some(MemberSignature::Property(ty))
            | Some(MemberSignature::Event(ty)) => ty.clone(),
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
                SyntaxRef::Field { declarator, .. } | SyntaxRef::Event { declarator, .. } => {
                    declarator.initializer.as_ref()
                }
                SyntaxRef::Property(property) => property.initializer.as_ref(),
                _ => None,
            });

        // literal initializers bake into the heap default instead of running
        // as code. This matters for exported behaviour fields: the inspector's
        // public-variable values are applied *after* the heap loads, so a
        // baked default lets them win — runtime initializer code would
        // overwrite them on the first event
        // `= { 1, 2 }` builds an array, so it always runs at startup
        let baked = match initializer {
            Some(InitializerValue::Expression(expression)) => {
                literal_heap_init(expression, &udon_type)
            }
            _ => None,
        };
        let runs_at_startup = initializer.is_some() && baked.is_none();

        let sync = self.sync_mode_of(field);
        let slot = self.program.add_data(DataSymbol {
            name,
            udon_type,
            init: baked.unwrap_or(HeapInit::Null),
            export,
            sync,
        });
        self.statics.insert(field, slot);
        if runs_at_startup && let Some(file) = file {
            self.static_init.push((field, file));
            // met while the static initializer is being emitted (by another
            // initializer's expression): initialize it right here, ahead of
            // the read that is being lowered
            if self.static_init_phase && self.static_init_emitted.insert(field) {
                self.emit_static_field_initializer(field, file);
            }
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

/// The last segment of an attribute's name, so `[MenSharp.UdonSynced]` and
/// `[UdonSynced]` read the same.
fn attribute_name<'a>(attribute: &'a men_sharp_parser::ast::Attribute<'a, 'a>) -> Option<&'a str> {
    let TypeRefBase::Name(name) = &attribute.name.base else {
        return None;
    };
    let spelling = name.segments.last()?.name.value;
    Some(spelling.strip_suffix("Attribute").unwrap_or(spelling))
}

/// The text of a string literal written as an attribute argument.
fn string_literal_of<'a>(expression: &'a Expression<'a, 'a>) -> Option<String> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if !primary.chain.is_empty() {
        return None;
    }
    let PrimaryLeft::Literal(literal) = &primary.left else {
        return None;
    };
    let LiteralExpression::String(text) = literal else {
        return None;
    };
    // attribute arguments carry the written text, quotes and all
    let inner = text
        .value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text.value);
    Some(inner.to_string())
}

/// The last identifier in an expression written as a name (`UdonSyncMode.Linear`
/// -> `Linear`). Attribute arguments are never type-checked, so this reads the
/// syntax; anything else returns `None` and is reported by the caller.
/// The name inside `nameof(...)`, when the expression is exactly that.
/// Attributes are read off the syntax, so the operand is a spelling here, not
/// a resolved member.
fn nameof_argument<'a>(expression: &'a Expression<'a, 'a>) -> Option<&'a str> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if !primary.chain.is_empty() {
        return None;
    }
    let PrimaryLeft::Nameof {
        value: Ok(value), ..
    } = &primary.left
    else {
        return None;
    };
    last_name_of(value)
}

fn last_name_of<'a>(expression: &'a Expression<'a, 'a>) -> Option<&'a str> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if let Some(PrimaryRight::Member { name, .. }) = primary.chain.last() {
        return name.as_ref().ok().map(|name| name.value);
    }
    match &primary.left {
        PrimaryLeft::Identifier { name, .. } => Some(name.value),
        _ => None,
    }
}

/// What a reference to another behaviour is *stored* as. Udon resolves a
/// `this` heap reference to a GameObject, a Transform or an UdonBehaviour and
/// nothing else — an interface-typed slot is refused outright, taking the
/// whole program down with it — so this is the type a slot may declare.
const BEHAVIOUR_HEAP_TYPE: &str = "VRCUdonUdonBehaviour";

/// ...and what it is *called* in an extern signature, where the methods are
/// declared on the interface rather than on UdonBehaviour. Storing one and
/// naming the other is what UdonSharp does too, for the same reason.
const BEHAVIOUR_EXTERN_TYPE: &str = "VRCUdonCommonInterfacesIUdonEventReceiver";

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

/// Does this method return something? (For the event export: a parameterless
/// method with a result still needs a layout, for the result variable.)
fn return_type_of(generator: &Generator<'_, '_>, member: SymbolId) -> bool {
    matches!(
        generator.signatures.members.get(&member),
        Some(MemberSignature::Function(signature)) if signature.return_type != Type::Void
    )
}

/// Unity/VRChat lifecycle methods map to Udon's built-in event names; other
/// method names become custom events verbatim.
/// `OnPlayerJoined` → `_onPlayerJoined`: Udon spells its built-in events as
/// the method name with a leading underscore and a lower-case first letter.
/// Which names get this treatment is decided by the SDK dump's `Event_` nodes
/// (`UdonNodes::event`): everything else is a custom event under its own
/// name, and silently rewriting an unknown `OnSomething` would produce an
/// event nothing ever raises.
fn udon_event_name(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => format!("_{}{}", first.to_lowercase(), characters.as_str()),
        None => name.to_string(),
    }
}

/// The heap slot the runtime writes one event argument into before raising
/// the event: lower-cased event name plus upper-cased parameter name
/// (`OnPlayerJoined`, `player` → `onPlayerJoinedPlayer`) — the same names
/// Udon graphs and UdonSharp read.
fn event_argument_slot(event: &str, parameter: &str) -> String {
    let mut slot = String::new();
    let mut characters = event.chars();
    if let Some(first) = characters.next() {
        slot.extend(first.to_lowercase());
        slot.push_str(characters.as_str());
    }
    let mut characters = parameter.chars();
    if let Some(first) = characters.next() {
        slot.extend(first.to_uppercase());
        slot.push_str(characters.as_str());
    }
    slot
}

/// The declared Udon heap type for an event argument's .NET type. Arrays are
/// spelled out here — `[]` has no alphanumeric characters, so the general
/// mangling would silently collide with the element type.
fn event_slot_type(dotnet: &str) -> String {
    match dotnet.strip_suffix("[]") {
        Some(element) => format!("{}Array", mangle_dotnet_name(element)),
        None => mangle_dotnet_name(dotnet),
    }
}

mod comparers;
mod components;
mod delegates;
mod exceptions;
mod expressions;
mod functions;
mod network;
mod nullable;
mod patterns;
mod programs;
mod runtime;
mod structs;
mod tasks;
mod tuples;
