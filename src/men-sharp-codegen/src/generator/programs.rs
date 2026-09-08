//! Programs talking to programs.
//!
//! Two behaviours are two Udon programs with no memory in common. What one
//! can do to another is what the runtime offers by *name*: write a heap
//! variable (`SetProgramVariable`), read one (`GetProgramVariable`), raise an
//! event (`SendCustomEvent`). So a call across the boundary is a protocol,
//! not a jump: the arguments go into named variables of the callee, the
//! event runs the body, and the result (and any `ref`/`out` parameter) is
//! read back from named variables afterwards.
//!
//! The names are UdonSharp's — `__0_amount__param`, `__0_Slide`,
//! `__0___0_Count__ret` — computed by the very algorithm its compiler uses
//! (`CompilationContext.BuildMethodLayout`): a counter per name, inherited
//! down the class chain, members in declaration order. Sharing the scheme is
//! what lets M# code call an UdonSharp behaviour typed, and lets UdonSharp
//! code (which only knows strings) reach an M# program by the names its own
//! author would guess.
//!
//! An UdonSharp behaviour is known here through its *source*, read for
//! declarations only (`Declarations::foreign_files`): the class is a
//! reference to a program UdonSharp compiled, never something to compile.

use rustc_hash::FxHashMap as HashMap;
use std::ops::Range;

use men_sharp_parser::ast::{AccessorKind, FunctionBody, Modifier};
use men_sharp_semantics::{
    Accessibility, ExternalTypeKind, MemberSignature, ParameterPassing, ResolvedCall,
    ResolvedMember, SymbolId, SymbolKind, SyntaxRef, Type, TypeTarget,
};

use super::*;

/// The .NET name of UdonSharp's behaviour base class — a class deriving from
/// it (through metadata when `UdonSharp.Runtime.dll` is referenced, or
/// through a source declaration of that name) is a program of UdonSharp's.
pub(super) const UDONSHARP_BEHAVIOUR: &str = "UdonSharp.UdonSharpBehaviour";

const RECEIVER: &str = BEHAVIOUR_EXTERN_TYPE;

/// What a layout is filed under: a method, or one accessor of a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LayoutKey {
    Method(SymbolId),
    Getter(SymbolId),
    Setter(SymbolId),
}

/// How one member of a behaviour is reached from another program: the event
/// that runs it, the variables that carry its parameters, the variable its
/// result is left in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExportLayout {
    pub event: String,
    pub parameters: Vec<String>,
    pub result: Option<String>,
}

/// UdonSharp's `GetUniqueID`: `__{n}_{id}`, with `n` how many times this id
/// was asked for before.
fn unique_id(counters: &mut HashMap<String, u32>, id: &str) -> String {
    let count = counters.entry(id.to_string()).or_insert(0);
    let found = *count;
    *count += 1;
    format!("__{found}_{id}")
}

impl<'a, 'ast> Generator<'a, 'ast> {
    // ------------------------------------------------------- what is a program

    /// Is this class an UdonSharp behaviour — derived from
    /// `UdonSharp.UdonSharpBehaviour`? Such a class is a program of
    /// UdonSharp's: M# reaches it by name and never compiles it.
    pub(super) fn is_foreign_behaviour_class(&self, class: SymbolId) -> bool {
        if self.declarations.table.symbol(class).kind != SymbolKind::Class {
            return false;
        }
        let mut current = class;
        for _ in 0..64 {
            let mut next = None;
            for base in self
                .signatures
                .base_types
                .get(&current)
                .into_iter()
                .flatten()
            {
                match base {
                    Type::Named {
                        target: TypeTarget::External(id),
                        ..
                    } => {
                        if self.external.display_name(*id) == UDONSHARP_BEHAVIOUR {
                            return true;
                        }
                    }
                    Type::Named {
                        target: TypeTarget::Source(symbol),
                        ..
                    } if self.declarations.table.symbol(*symbol).kind == SymbolKind::Class => {
                        if self.display_path(*symbol) == UDONSHARP_BEHAVIOUR {
                            return true;
                        }
                        next = Some(*symbol);
                    }
                    _ => {}
                }
            }
            match next {
                Some(base) => current = base,
                None => return false,
            }
        }
        false
    }

    /// Does a value of this type refer to another *program* — a behaviour of
    /// the user's, an UdonSharp behaviour, or an array of either? Such a
    /// value is not an object: it has no type id, its members are reached by
    /// name, and a slot holding one is typed as what Udon lets programs
    /// talk through.
    pub(super) fn is_program_reference(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => {
                Some(*symbol) != self.marker
                    && (is_behaviour_class(self.declarations, self.signatures, *symbol)
                        || self.is_foreign_behaviour_class(*symbol)
                        || self.display_path(*symbol) == UDONSHARP_BEHAVIOUR)
            }
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.external.display_name(*id) == UDONSHARP_BEHAVIOUR,
            Type::Array { element, .. } => self.is_program_reference(element),
            Type::Nullable(inner) => self.is_program_reference(inner),
            _ => false,
        }
    }

    /// Why a library declaration (or the type it belongs to) cannot be
    /// compiled, if it cannot: the first error the checker found in it.
    pub(super) fn uncompilable_reason(&self, symbol: SymbolId) -> Option<String> {
        let mut current = Some(symbol);
        while let Some(id) = current {
            if let Some(reason) = self.bodies.uncompilable.get(&id) {
                return Some(reason.clone());
            }
            let entry = self.declarations.table.symbol(id);
            if entry.kind == SymbolKind::Namespace {
                break;
            }
            current = entry.parent;
        }
        None
    }

    /// A class of the user's (or of a library) deriving from an engine
    /// class — `MonoBehaviour`, `ScriptableObject` — is not something a
    /// program can make or hold: Udon has no `new` for it, and an instance
    /// Unity made is not an `object[]`. Only a behaviour (a program) is
    /// reachable, and that is handled as a program reference.
    pub(super) fn engine_base_of(&self, class: SymbolId) -> Option<String> {
        let mut current = class;
        for _ in 0..64 {
            let mut next = None;
            for base in self
                .signatures
                .base_types
                .get(&current)
                .into_iter()
                .flatten()
            {
                match base {
                    Type::Named {
                        target: TypeTarget::External(id),
                        ..
                    } => {
                        let name = self.external.display_name(*id);
                        if self.external.type_info(*id).kind == ExternalTypeKind::Class
                            && !matches!(
                                name.as_str(),
                                "System.Object" | "System.ValueType" | "System.Enum"
                            )
                        {
                            return Some(name);
                        }
                    }
                    Type::Named {
                        target: TypeTarget::Source(symbol),
                        ..
                    } if self.declarations.table.symbol(*symbol).kind == SymbolKind::Class => {
                        next = Some(*symbol);
                    }
                    _ => {}
                }
            }
            current = next?;
        }
        None
    }

    // ----------------------------------------------------------- the layout

    /// The source classes from `class` up to (excluding) the behaviour base,
    /// base first: the order the export names are assigned in.
    fn program_class_chain(&self, class: SymbolId) -> Vec<SymbolId> {
        let mut chain = Vec::new();
        let mut current = Some(class);
        while let Some(symbol) = current {
            if Some(symbol) == self.marker
                || self.display_path(symbol) == UDONSHARP_BEHAVIOUR
                || chain.contains(&symbol)
            {
                break;
            }
            chain.push(symbol);
            current = self
                .signatures
                .base_types
                .get(&symbol)
                .into_iter()
                .flatten()
                .find_map(|base| match base {
                    Type::Named {
                        target: TypeTarget::Source(base),
                        ..
                    } if self.declarations.table.symbol(*base).kind == SymbolKind::Class => {
                        Some(*base)
                    }
                    _ => None,
                });
        }
        chain.reverse();
        chain
    }

    /// The member this one overrides, in a source base class — or `None`
    /// when it overrides nothing of the user's (an event of the base
    /// behaviour, or nothing at all).
    fn overridden_member(&self, member: SymbolId) -> Option<SymbolId> {
        let modifiers = self.declared_modifiers(member);
        if !modifiers.contains(&Modifier::Override) {
            return None;
        }
        let entry = self.declarations.table.symbol(member);
        let parent = entry.parent?;
        let wanted = self.signatures.members.get(&member);
        let mut chain = self.program_class_chain(parent);
        chain.reverse(); // most derived first
        for &class in chain.iter().skip_while(|&&class| class != parent).skip(1) {
            for &candidate in &self.declarations.table.symbol(class).members {
                let symbol = self.declarations.table.symbol(candidate);
                if symbol.name != entry.name || symbol.kind != entry.kind || symbol.is_static {
                    continue;
                }
                let compatible = match (wanted, self.signatures.members.get(&candidate)) {
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
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// The declaration whose layout a member uses: an override runs under
    /// the names of what it overrides (UdonSharp does the same, so the
    /// caller's static type does not change the names).
    fn root_declaration(&self, member: SymbolId) -> SymbolId {
        let mut current = member;
        for _ in 0..64 {
            match self.overridden_member(current) {
                Some(base) => current = base,
                None => break,
            }
        }
        current
    }

    /// How another program reaches `member` (a method or a property
    /// accessor's owner): its export names. `None` for members with no
    /// layout — fields, constructors, operators.
    pub(super) fn export_layout(&mut self, member: SymbolId) -> Option<ExportLayout> {
        let root = self.root_declaration(member);
        let class = self.declarations.table.symbol(root).parent?;
        if !self.export_layouts.contains_key(&class) {
            let layouts = self.build_export_layouts(class);
            self.export_layouts.insert(class, layouts);
        }
        self.export_layouts
            .get(&class)?
            .get(&LayoutKey::Method(root))
            .cloned()
    }

    /// The getter's and setter's layouts of a property (or indexer).
    pub(super) fn accessor_layouts(
        &mut self,
        property: SymbolId,
    ) -> (Option<ExportLayout>, Option<ExportLayout>) {
        let root = self.root_declaration(property);
        let Some(class) = self.declarations.table.symbol(root).parent else {
            return (None, None);
        };
        if !self.export_layouts.contains_key(&class) {
            let layouts = self.build_export_layouts(class);
            self.export_layouts.insert(class, layouts);
        }
        let layouts = &self.export_layouts[&class];
        (
            layouts.get(&LayoutKey::Getter(root)).cloned(),
            layouts.get(&LayoutKey::Setter(root)).cloned(),
        )
    }

    /// UdonSharp's `BuildLayout` for a class: every method and accessor of
    /// the chain, base first, in declaration order, each drawing its names
    /// from counters the bases already advanced.
    fn build_export_layouts(&self, class: SymbolId) -> HashMap<LayoutKey, ExportLayout> {
        let mut layouts = HashMap::default();
        let mut counters: HashMap<String, u32> = HashMap::default();
        for owner in self.program_class_chain(class) {
            let members: Vec<SymbolId> = self.declarations.table.symbol(owner).members.to_vec();
            for member in members {
                let symbol = self.declarations.table.symbol(member);
                match symbol.kind {
                    SymbolKind::Method => {
                        // an override runs under its base's names
                        if self.overridden_member(member).is_some() {
                            continue;
                        }
                        let Some(MemberSignature::Function(signature)) =
                            self.signatures.members.get(&member)
                        else {
                            continue;
                        };
                        let parameters: Vec<String> = signature
                            .parameters
                            .iter()
                            .enumerate()
                            .map(|(index, parameter)| {
                                parameter
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| format!("arg{index}"))
                            })
                            .collect();
                        let layout = self.build_member_layout(
                            &mut counters,
                            symbol.name,
                            &parameters,
                            signature.return_type == Type::Void,
                            symbol.is_static,
                            self.has_attribute(member, "NetworkCallable"),
                        );
                        layouts.insert(LayoutKey::Method(member), layout);
                    }
                    SymbolKind::Property => {
                        if self.overridden_member(member).is_some() {
                            continue;
                        }
                        let (has_get, has_set) = self.declared_accessors(member);
                        let name = if self.is_indexer(member) {
                            "Item".to_string()
                        } else {
                            symbol.name.to_string()
                        };
                        let indices: Vec<String> = self.indexer_parameter_names(member);
                        // Roslyn lists a property's accessors getter first,
                        // whichever was written first
                        if has_get {
                            let layout = self.build_member_layout(
                                &mut counters,
                                &format!("get_{name}"),
                                &indices,
                                false,
                                symbol.is_static,
                                false,
                            );
                            layouts.insert(LayoutKey::Getter(member), layout);
                        }
                        if has_set {
                            let mut parameters = indices.clone();
                            parameters.push("value".into());
                            let layout = self.build_member_layout(
                                &mut counters,
                                &format!("set_{name}"),
                                &parameters,
                                true,
                                symbol.is_static,
                                false,
                            );
                            layouts.insert(LayoutKey::Setter(member), layout);
                        }
                    }
                    _ => {}
                }
            }
        }
        layouts
    }

    /// UdonSharp's `BuildMethodLayout` for one method.
    fn build_member_layout(
        &self,
        counters: &mut HashMap<String, u32>,
        name: &str,
        parameters: &[String],
        is_void: bool,
        is_static: bool,
        network_callable: bool,
    ) -> ExportLayout {
        let event_node = if is_static {
            None
        } else {
            self.nodes.event(name)
        };
        let (event, parameters) = if let Some(node) = event_node {
            // a built-in event: its arguments arrive in the runtime's own
            // slots, nothing to name
            let event = udon_event_name(name);
            let parameters = node
                .parameters
                .iter()
                .map(|parameter| event_argument_slot(name, &parameter.name))
                .collect();
            (event, parameters)
        } else {
            // other scripts call a network callable by its written name, so
            // it is never mangled; a method with parameters is, so that
            // overloads stay apart
            let event = if network_callable || parameters.is_empty() {
                name.to_string()
            } else {
                unique_id(counters, name)
            };
            let parameters = parameters
                .iter()
                .map(|parameter| unique_id(counters, &format!("{parameter}__param")))
                .collect();
            (event, parameters)
        };
        let result = (!is_void).then(|| unique_id(counters, &format!("{event}__ret")));
        ExportLayout {
            event,
            parameters,
            result,
        }
    }

    /// Which accessors a property declares, from its syntax.
    fn declared_accessors(&self, property: SymbolId) -> (bool, bool) {
        let mut has_get = false;
        let mut has_set = false;
        for site in &self.declarations.table.symbol(property).declarations {
            let body = match site.syntax {
                SyntaxRef::Property(node) => &node.body,
                SyntaxRef::Indexer(node) => &node.body,
                _ => continue,
            };
            match body {
                FunctionBody::Accessors(list) => {
                    for accessor in list.accessors {
                        match accessor.kind.value {
                            AccessorKind::Get => has_get = true,
                            AccessorKind::Set | AccessorKind::Init => has_set = true,
                            _ => {}
                        }
                    }
                }
                // `int X => 1;`
                FunctionBody::Expression { .. } => has_get = true,
                _ => {}
            }
        }
        (has_get, has_set)
    }

    fn is_indexer(&self, property: SymbolId) -> bool {
        self.declarations
            .table
            .symbol(property)
            .declarations
            .iter()
            .any(|site| matches!(site.syntax, SyntaxRef::Indexer(_)))
    }

    fn indexer_parameter_names(&self, property: SymbolId) -> Vec<String> {
        match self.signatures.members.get(&property) {
            Some(MemberSignature::Function(signature)) => signature
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    parameter
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("index{index}"))
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    // ----------------------------------------------------------- the calls

    /// A public member of another program, or an error saying why not.
    fn require_public_member(
        &mut self,
        ctx: &Ctx<'ast>,
        symbol: SymbolId,
        span: &Range<usize>,
    ) -> bool {
        let entry = self.declarations.table.symbol(symbol);
        if entry.accessibility == Accessibility::Public {
            return true;
        }
        let name = entry.name.to_string();
        self.error(
            ctx,
            Message::key("codegen.name_is_not_public_so_it_is").arg("name", name),
            span.clone(),
        );
        false
    }

    /// `other.Slide(2)`, `int n = other.Count()`: a method call on another
    /// behaviour. The arguments are written into the callee's parameter
    /// variables, the event runs the body, and the result (and every `ref`/
    /// `out` argument) is read back — the protocol UdonSharp follows, under
    /// the names it would use.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cross_program_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        receiver: DataId,
        values: &[DataId],
        by_ref: &[(usize, Place)],
        span: Range<usize>,
    ) -> Piece {
        if !self.require_public_member(ctx, symbol, &span) {
            return Piece::Error;
        }
        // a delegate is code addresses of the program that made it: it
        // means nothing to another program
        let crosses_delegate = self.is_delegate_type(&call.signature.return_type)
            || call
                .signature
                .parameters
                .iter()
                .any(|parameter| self.is_delegate_type(&parameter.parameter_type));
        if crosses_delegate {
            let name = self.declarations.table.symbol(symbol).name;
            self.error(
                ctx,
                Message::key("codegen.name_takes_or_returns_a_delegate_which").arg("name", name),
                span,
            );
            return Piece::Error;
        }
        // A task *may* cross: it is an object[] whose field positions are
        // fixed by the mini-corlib both programs compile, and every piece of
        // it that would need this program's code addresses — running a
        // continuation, rethrowing the exception as itself — checks the
        // owner first. See the `tasks` module.
        let Some(layout) = self.export_layout(symbol) else {
            self.error(
                ctx,
                Message::key("codegen.this_member_cannot_be_reached_on_another"),
                span,
            );
            return Piece::Error;
        };
        let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
        if layout.parameters.len() != values.len() {
            self.error(
                ctx,
                "internal: argument count does not match the callee's export layout",
                span,
            );
            return Piece::Error;
        }
        let return_type = signature.return_type.clone();
        if return_type != Type::Void && layout.result.is_none() {
            self.error(
                ctx,
                "internal: a returning method without a result variable",
                span,
            );
            return Piece::Error;
        }

        for (name, value) in layout.parameters.iter().zip(values) {
            self.set_program_variable(ctx, receiver, name, *value, span.clone());
        }
        let event = self.string_constant(&layout.event);
        self.call_extern(
            ctx,
            &format!("{RECEIVER}.__SendCustomEvent__SystemString__SystemVoid"),
            &[receiver, event],
            span.clone(),
        );
        // `ref`/`out`: the callee left the final value in the parameter
        // variable
        for (index, place) in by_ref {
            let Some(name) = layout.parameters.get(*index) else {
                continue;
            };
            let ty = signature
                .parameters
                .get(*index)
                .map(|parameter| parameter.parameter_type.clone())
                .unwrap_or(Type::Error);
            let value = self.get_program_variable(ctx, receiver, name, &ty, span.clone());
            self.write_place(ctx, place.clone(), value, span.clone());
        }
        match layout.result {
            Some(name) if return_type != Type::Void => {
                let value = self.get_program_variable(ctx, receiver, &name, &return_type, span);
                Piece::Value(value, return_type)
            }
            _ => Piece::Void,
        }
    }

    /// A property of another behaviour. A field or auto-property of a
    /// behaviour of the user's is a variable, read and written by name; a
    /// property with a body — and *every* property of an UdonSharp
    /// behaviour, whose backing storage is its own — goes through its
    /// accessor events.
    pub(super) fn program_member_place(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: &ResolvedMember,
        symbol: SymbolId,
        receiver: DataId,
        receiver_type: &Type,
        span: Range<usize>,
    ) -> Place {
        if !self.require_public_member(ctx, symbol, &span) {
            return Place::Error;
        }
        let entry = self.declarations.table.symbol(symbol);
        let name = entry.name.to_string();
        let ty = self.substitute(&member.member_type, &ctx.key.bindings);
        let foreign = match receiver_type {
            Type::Named {
                target: TypeTarget::Source(class),
                ..
            } => self.is_foreign_behaviour_class(*class),
            _ => true,
        };
        if self.is_delegate_type(&ty) {
            self.error(
                ctx,
                Message::key("codegen.name_is_a_delegate_which_cannot_cross").arg("name", name),
                span,
            );
            return Place::Error;
        }
        if self.is_task_type(&ty) {
            self.error(
                ctx,
                Message::key("codegen.name_is_a_task_typed_field_and").arg("name", name),
                span,
            );
            return Place::Error;
        }
        let by_name = match member.kind {
            SymbolKind::Field | SymbolKind::Event => true,
            SymbolKind::Property => !foreign && self.is_auto_property(symbol),
            _ => false,
        };
        if by_name {
            return Place::ProgramVariable { receiver, name, ty };
        }
        if member.kind != SymbolKind::Property {
            self.error(
                ctx,
                Message::key("codegen.name_cannot_be_reached_on_another_behaviour")
                    .arg("name", name),
                span,
            );
            return Place::Error;
        }
        if self.is_indexer(symbol) {
            self.error(
                ctx,
                Message::key("codegen.an_indexer_of_another_behaviour_is_not"),
                span,
            );
            return Place::Error;
        }
        let (getter, setter) = self.accessor_layouts(symbol);
        Place::ProgramAccessor {
            receiver,
            getter: getter.and_then(|layout| layout.result.map(|result| (layout.event, result))),
            setter: setter.and_then(|layout| {
                layout
                    .parameters
                    .last()
                    .cloned()
                    .map(|parameter| (layout.event, parameter))
            }),
            name,
            ty,
        }
    }

    /// `other.Prop`: raise the getter's event, then read what it left.
    pub(super) fn read_program_accessor(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: DataId,
        getter: Option<(String, String)>,
        name: &str,
        ty: &Type,
        span: Range<usize>,
    ) -> Option<DataId> {
        let Some((event, result)) = getter else {
            self.error(
                ctx,
                Message::key("codegen.name_has_no_getter_to_raise_on").arg("name", name),
                span,
            );
            return None;
        };
        let event = self.string_constant(&event);
        self.call_extern(
            ctx,
            &format!("{RECEIVER}.__SendCustomEvent__SystemString__SystemVoid"),
            &[receiver, event],
            span.clone(),
        );
        Some(self.get_program_variable(ctx, receiver, &result, ty, span))
    }

    /// `other.Prop = v`: hand the value over, then raise the setter's event.
    pub(super) fn write_program_accessor(
        &mut self,
        ctx: &mut Ctx<'ast>,
        receiver: DataId,
        setter: Option<(String, String)>,
        name: &str,
        value: DataId,
        span: Range<usize>,
    ) {
        let Some((event, parameter)) = setter else {
            self.error(
                ctx,
                Message::key("codegen.name_has_no_setter_to_raise_on").arg("name", name),
                span,
            );
            return;
        };
        self.set_program_variable(ctx, receiver, &parameter, value, span.clone());
        let event = self.string_constant(&event);
        self.call_extern(
            ctx,
            &format!("{RECEIVER}.__SendCustomEvent__SystemString__SystemVoid"),
            &[receiver, event],
            span,
        );
    }

    pub(super) fn set_program_variable(
        &mut self,
        ctx: &Ctx<'ast>,
        receiver: DataId,
        name: &str,
        value: DataId,
        span: Range<usize>,
    ) {
        let key = self.string_constant(name);
        self.call_extern(
            ctx,
            &format!("{RECEIVER}.__SetProgramVariable__SystemString_SystemObject__SystemVoid"),
            &[receiver, key, value],
            span,
        );
    }

    /// The variable comes back boxed as `object`; the value inside is already
    /// of the right runtime type, so copying it into a typed slot is all the
    /// conversion Udon needs.
    pub(super) fn get_program_variable(
        &mut self,
        ctx: &Ctx<'ast>,
        receiver: DataId,
        name: &str,
        ty: &Type,
        span: Range<usize>,
    ) -> DataId {
        let boxed = self.temp("SystemObject");
        let key = self.string_constant(name);
        self.call_extern(
            ctx,
            &format!("{RECEIVER}.__GetProgramVariable__SystemString__SystemObject"),
            &[receiver, key, boxed],
            span,
        );
        let out = self.temp_for(ty);
        self.copy(boxed, out);
        out
    }

    // --------------------------------------------------------- the surface

    /// The events a behaviour of the user's exports for other programs to
    /// call, beyond its parameterless public methods: public methods with
    /// parameters, and the accessors of public properties with bodies — each
    /// under its layout's names, so an UdonSharp (or M#) caller finds them
    /// where it expects.
    pub(super) fn exported_member_layouts(
        &mut self,
        member: SymbolId,
    ) -> Vec<(ExportLayout, FunctionKey, Vec<ParameterPassing>)> {
        let symbol = self.declarations.table.symbol(member);
        let (kind, is_static, is_public) = (
            symbol.kind,
            symbol.is_static,
            symbol.accessibility == Accessibility::Public,
        );
        if !is_public || is_static {
            return Vec::new();
        }
        match kind {
            SymbolKind::Method => {
                let Some(MemberSignature::Function(signature)) =
                    self.signatures.members.get(&member).cloned()
                else {
                    return Vec::new();
                };
                let passing = signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.passing)
                    .collect();
                match self.export_layout(member) {
                    Some(layout) => vec![(
                        layout,
                        FunctionKey {
                            symbol: member,
                            role: Role::Method,
                            bindings: Vec::new(),
                        },
                        passing,
                    )],
                    None => Vec::new(),
                }
            }
            SymbolKind::Property if !self.is_auto_property(member) && !self.is_indexer(member) => {
                let (getter, setter) = self.accessor_layouts(member);
                let mut out = Vec::new();
                if let Some(layout) = getter {
                    out.push((
                        layout,
                        FunctionKey {
                            symbol: member,
                            role: Role::Getter,
                            bindings: Vec::new(),
                        },
                        Vec::new(),
                    ));
                }
                if let Some(layout) = setter {
                    out.push((
                        layout,
                        FunctionKey {
                            symbol: member,
                            role: Role::Setter,
                            bindings: Vec::new(),
                        },
                        vec![ParameterPassing::Value],
                    ));
                }
                out
            }
            _ => Vec::new(),
        }
    }
}
