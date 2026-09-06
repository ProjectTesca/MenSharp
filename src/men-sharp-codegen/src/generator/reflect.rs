//! `MenSharp.Reflection.Reflect` — static reflection, answered here.
//!
//! Udon has no `System.Reflection`, and an M# object is an `object[]` that
//! carries nothing but a type id. What exists instead is this compiler,
//! which knows every field of every type it compiles and monomorphizes
//! generic code per type argument — so a question about a type argument
//! can be answered while the caller is being compiled, and the call
//! lowered to its answer: a boolean constant (`IsArray<T>()`), a fresh
//! object (`New<T>()`), or one call per field (`VisitFields`), each to the
//! visitor's generic `Visit<F>` instantiated for that field's type. Nothing
//! is looked up at run time; a question with no answer (`VisitFields` on an
//! engine type, `New<T>` without a parameterless constructor) is an error
//! naming the type, where the code is built.
//!
//! The constants feed `if`: a branch on a constant condition lowers only
//! the branch taken (see `lower_if`), which is what lets generic code say
//! `if (Reflect.IsArray<T>()) { ... element ... } else { ... }` and name,
//! in each arm, things that exist only for some `T`.

use super::*;

const REFLECT_PATH: [&str; 3] = ["MenSharp", "Reflection", "Reflect"];
const FIELD_INFO_PATH: [&str; 3] = ["MenSharp", "Reflection", "FieldInfo"];
const LIST_PATH: [&str; 4] = ["System", "Collections", "Generic", "List"];

/// One storage slot of a type, as the visitor sees it.
struct ReflectedField {
    member: SymbolId,
    index: usize,
    ty: Type,
}

impl<'a, 'ast> Generator<'a, 'ast> {
    /// A call to one of `Reflect`'s intrinsics, lowered in place; `None`
    /// for any other call.
    pub(super) fn try_reflect_intrinsic(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        values: &[DataId],
        source_by_ref: &[(usize, Place)],
        span: Range<usize>,
    ) -> Option<Piece> {
        let entry = self.declarations.table.symbol(symbol);
        let parent = entry.parent?;
        if self.display_path(parent) != REFLECT_PATH.join(".") {
            return None;
        }
        let name = entry.name;
        let type_arguments: Vec<Type> = call
            .type_arguments
            .iter()
            .map(|argument| self.substitute(argument, &ctx.key.bindings))
            .collect();
        let boolean = self.corlib_type("Boolean");
        let piece = match name {
            "VisitFields" => {
                self.reflect_visit_fields(ctx, &type_arguments, values, source_by_ref, span)
            }
            "VisitElementType" => {
                self.reflect_visit_element_type(ctx, &type_arguments, values, span)
            }
            "New" => self.reflect_new(ctx, &type_arguments, span),
            "Unsupported" => {
                let display = type_arguments
                    .first()
                    .map(|ty| self.describe_type(ty))
                    .unwrap_or_default();
                let what = values
                    .first()
                    .and_then(|slot| match &self.program.data[slot.0].init {
                        HeapInit::Str(text) => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "this code".to_string());
                self.error(
                    ctx,
                    Message::key("codegen.reflect_what_does_not_support_display")
                        .arg("what", what)
                        .arg("display", display),
                    span,
                );
                Piece::Error
            }
            "IsObject" | "IsArray" | "IsList" | "IsNullable" | "IsEnum" | "Is" => {
                let answer = match (name, type_arguments.as_slice()) {
                    ("IsObject", [ty]) => self.is_reflectable_object(ty),
                    ("IsArray", [ty]) => matches!(ty, Type::Array { rank: 1, .. }),
                    ("IsList", [ty]) => self.list_element_type(ty).is_some(),
                    ("IsNullable", [ty]) => self.nullable_inner(ty).is_some(),
                    ("IsEnum", [ty]) => self.is_enum_type(ty),
                    ("Is", [left, right]) => left == right,
                    _ => {
                        self.error(ctx, "internal: Reflect intrinsic arity", span);
                        return Some(Piece::Error);
                    }
                };
                Piece::Value(self.bool_constant(answer), boolean)
            }
            _ => return None,
        };
        Some(piece)
    }

    // ------------------------------------------------------------- queries

    /// A class, struct or record of the compilation's own: what has a
    /// layout to walk.
    fn is_reflectable_object(&self, ty: &Type) -> bool {
        self.is_source_class(ty) && !self.is_program_reference(ty)
    }

    fn is_enum_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } => self.declarations.table.symbol(*symbol).kind == SymbolKind::Enum,
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => self.external.type_info(*id).kind == men_sharp_semantics::ExternalTypeKind::Enum,
            _ => false,
        }
    }

    /// `E` when `ty` is the corlib's `List<E>`.
    fn list_element_type(&self, ty: &Type) -> Option<Type> {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = ty
        else {
            return None;
        };
        if self.display_path(*symbol) != LIST_PATH.join(".") || arguments.len() != 1 {
            return None;
        }
        Some(arguments[0].clone())
    }

    /// What `VisitElementType` hands on: the element of an array, the `E`
    /// of a `List<E>`, the `E` of an `E?`.
    fn element_type_of(&self, ty: &Type) -> Option<Type> {
        if let Type::Array { element, rank: 1 } = ty {
            return Some((**element).clone());
        }
        if let Some(inner) = self.nullable_inner(ty) {
            return Some(inner);
        }
        self.list_element_type(ty)
    }

    /// The storage slots of an object type in layout order: base class
    /// first, then declaration order — each with its type as this
    /// instantiation sees it. Events hold a slot too but are not fields.
    fn reflected_fields(&mut self, ty: &Type) -> Vec<ReflectedField> {
        let Some(layout) = self.layout_of(ty) else {
            return Vec::new();
        };
        let mut fields = Vec::new();
        // walk the chain so that a field inherited from a generic base is
        // substituted with *that* base's arguments, not the leaf's
        let mut current = Some(ty.clone());
        while let Some(this_type) = current {
            let Type::Named {
                target: TypeTarget::Source(class),
                arguments,
            } = &this_type
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
                let Some(&index) = layout.slots.get(&member) else {
                    continue;
                };
                let member_type = match self.signatures.members.get(&member) {
                    Some(MemberSignature::Field(ty)) | Some(MemberSignature::Property(ty)) => ty,
                    _ => continue,
                };
                fields.push(ReflectedField {
                    member,
                    index,
                    ty: self.substitute(member_type, &bindings),
                });
            }
            current = self.type_system().base_of(&this_type);
        }
        fields.sort_by_key(|field| field.index);
        fields
    }

    /// The visitor's `Visit` with `type_arity` type parameters and
    /// `parameter_count` parameters, with the bindings of its declaring
    /// type: what one call per field is instantiated from. A bodiless
    /// candidate (the interface's own) does not count — the visitor has to
    /// be a concrete class or struct.
    fn visitor_method(
        &mut self,
        visitor: &Type,
        type_arity: u32,
        parameter_count: usize,
    ) -> Option<(SymbolId, Vec<(SymbolId, Type)>)> {
        let candidates = self.type_system().members_named(visitor, "Visit");
        for candidate in candidates {
            let MemberOrigin::Source(symbol) = candidate.origin else {
                continue;
            };
            if candidate.kind != SymbolKind::Method
                || candidate.is_static
                || candidate.arity != type_arity
            {
                continue;
            }
            let Some(MemberSignature::Function(signature)) = &candidate.signature else {
                continue;
            };
            if signature.parameters.len() != parameter_count || self.is_bodiless(symbol) {
                continue;
            }
            let bindings = match &candidate.declaring_type {
                Type::Named {
                    target: TypeTarget::Source(class),
                    arguments,
                } => self
                    .declarations
                    .table
                    .symbol(*class)
                    .type_parameters
                    .iter()
                    .copied()
                    .zip(arguments.iter().cloned())
                    .collect(),
                _ => Vec::new(),
            };
            return Some((symbol, bindings));
        }
        None
    }

    // ---------------------------------------------------------- intrinsics

    /// `Reflect.VisitFields(ref target, visitor)`: one call of the visitor's
    /// `Visit<F>(info, ref field)` per storage slot of `T`.
    fn reflect_visit_fields(
        &mut self,
        ctx: &mut Ctx<'ast>,
        type_arguments: &[Type],
        values: &[DataId],
        source_by_ref: &[(usize, Place)],
        span: Range<usize>,
    ) -> Piece {
        let ([target_type, visitor_type], [target, visitor]) = (type_arguments, values) else {
            self.error(ctx, "internal: VisitFields shape", span);
            return Piece::Error;
        };
        let (target, visitor) = (*target, *visitor);
        if !self.is_reflectable_object(target_type) {
            let display = self.describe_type(target_type);
            self.error(
                ctx,
                Message::key("codegen.reflect_visit_fields_needs_an_m_type")
                    .arg("display", display),
                span,
            );
            return Piece::Error;
        }
        let Some((visit, class_bindings)) = self.visitor_method(visitor_type, 1, 2) else {
            let display = self.describe_type(visitor_type);
            self.error(
                ctx,
                Message::key("codegen.reflect_visitor_needs_a_visit_method")
                    .arg("display", display)
                    .arg("shape", "Visit<TField>(FieldInfo, ref TField)"),
                span,
            );
            return Piece::Error;
        };
        let Some(type_parameter) = self
            .declarations
            .table
            .symbol(visit)
            .type_parameters
            .first()
            .copied()
        else {
            return Piece::Error;
        };

        for field in self.reflected_fields(target_type) {
            let Some(info) = self.field_info(ctx, field.member, span.clone()) else {
                return Piece::Error;
            };
            let index = self.int_constant(field.index as i32);
            let current = self.get_element(ctx, target, index, &field.ty, span.clone());
            let mut bindings = class_bindings.clone();
            bindings.push((type_parameter, field.ty.clone()));
            let key = FunctionKey {
                symbol: visit,
                role: Role::Method,
                bindings,
            };
            let place = Place::Field {
                object: target,
                index,
                ty: field.ty.clone(),
            };
            self.call_function(
                ctx,
                &key,
                Some(visitor),
                &[info, current],
                &[(1, place)],
                span.clone(),
            );
        }
        // `ref target` written through something other than a local: the
        // stand-in value goes back where it came from, as after any call
        for (_, place) in source_by_ref {
            self.write_place(ctx, place.clone(), target, span.clone());
        }
        Piece::Void
    }

    /// `Reflect.VisitElementType<T>(visitor)`: `visitor.Visit<E>()` for the
    /// element type of `T`.
    fn reflect_visit_element_type(
        &mut self,
        ctx: &mut Ctx<'ast>,
        type_arguments: &[Type],
        values: &[DataId],
        span: Range<usize>,
    ) -> Piece {
        let ([target_type, visitor_type], [visitor]) = (type_arguments, values) else {
            self.error(ctx, "internal: VisitElementType shape", span);
            return Piece::Error;
        };
        let visitor = *visitor;
        let Some(element) = self.element_type_of(target_type) else {
            let display = self.describe_type(target_type);
            self.error(
                ctx,
                Message::key("codegen.reflect_display_has_no_element_type").arg("display", display),
                span,
            );
            return Piece::Error;
        };
        let Some((visit, mut bindings)) = self.visitor_method(visitor_type, 1, 0) else {
            let display = self.describe_type(visitor_type);
            self.error(
                ctx,
                Message::key("codegen.reflect_visitor_needs_a_visit_method")
                    .arg("display", display)
                    .arg("shape", "Visit<TType>()"),
                span,
            );
            return Piece::Error;
        };
        let Some(type_parameter) = self
            .declarations
            .table
            .symbol(visit)
            .type_parameters
            .first()
            .copied()
        else {
            return Piece::Error;
        };
        bindings.push((type_parameter, element));
        let key = FunctionKey {
            symbol: visit,
            role: Role::Method,
            bindings,
        };
        self.call_function(ctx, &key, Some(visitor), &[], &[], span);
        Piece::Void
    }

    /// `Reflect.New<T>()`: a class or record through its parameterless
    /// constructor, a struct at its defaults.
    fn reflect_new(
        &mut self,
        ctx: &mut Ctx<'ast>,
        type_arguments: &[Type],
        span: Range<usize>,
    ) -> Piece {
        let [ty] = type_arguments else {
            self.error(ctx, "internal: New shape", span);
            return Piece::Error;
        };
        if !self.is_reflectable_object(ty) {
            let display = self.describe_type(ty);
            self.error(
                ctx,
                Message::key("codegen.reflect_new_needs_an_m_type").arg("display", display),
                span,
            );
            return Piece::Error;
        }
        if self.is_source_struct(ty) {
            let slot = self.allocate_default_struct(ctx, ty, span);
            return Piece::Value(slot, ty.clone());
        }
        let Type::Named {
            target: TypeTarget::Source(class),
            arguments,
        } = ty
        else {
            return Piece::Error;
        };
        let class = *class;
        let bindings: Vec<(SymbolId, Type)> = self
            .declarations
            .table
            .symbol(class)
            .type_parameters
            .iter()
            .copied()
            .zip(arguments.iter().cloned())
            .collect();
        // the declared constructors: the parameterless one, or none at all
        // (then the synthesized default runs the field initializers)
        let constructors: Vec<SymbolId> = self
            .declarations
            .table
            .symbol(class)
            .members_named(".ctor")
            .iter()
            .copied()
            .filter(|&ctor| {
                let entry = self.declarations.table.symbol(ctor);
                entry.kind == SymbolKind::Constructor && !entry.is_static
            })
            .collect();
        let parameterless = constructors.iter().copied().find(|&ctor| {
            matches!(
                self.signatures.members.get(&ctor),
                Some(MemberSignature::Function(signature)) if signature.parameters.is_empty()
            )
        });
        let key = match (parameterless, constructors.is_empty()) {
            (Some(ctor), _) => FunctionKey {
                symbol: ctor,
                role: Role::Constructor,
                bindings,
            },
            (None, true) => FunctionKey {
                symbol: class,
                role: Role::DefaultConstructor,
                bindings,
            },
            (None, false) => {
                let display = self.describe_type(ty);
                self.error(
                    ctx,
                    Message::key("codegen.reflect_new_display_needs_a_parameterless")
                        .arg("display", display),
                    span,
                );
                return Piece::Error;
            }
        };
        let Some(object) = self.allocate_object(ctx, ty, span.clone()) else {
            return Piece::Error;
        };
        self.call_function(ctx, &key, Some(object), &[], &[], span);
        Piece::Value(object, ty.clone())
    }

    // ----------------------------------------------------------- FieldInfo

    /// A `FieldInfo` describing `member`: its name, accessibility and the
    /// attributes written on it that the checker resolved to classes of
    /// the compilation's own, each constructed as written.
    fn field_info(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: SymbolId,
        span: Range<usize>,
    ) -> Option<DataId> {
        let Some(info_symbol) = self.find_symbol(&FIELD_INFO_PATH) else {
            self.error(
                ctx,
                "internal: MenSharp.Reflection.FieldInfo is missing",
                span,
            );
            return None;
        };
        let info_type = Type::Named {
            target: TypeTarget::Source(info_symbol),
            arguments: Vec::new(),
        };
        let entry = self.declarations.table.symbol(member);
        let name = self.string_constant(entry.name);
        let is_public = self.bool_constant(entry.accessibility == Accessibility::Public);
        let attributes = self.attribute_objects(ctx, member, span.clone());

        let ctor = self
            .declarations
            .table
            .symbol(info_symbol)
            .members_named(".ctor")
            .iter()
            .copied()
            .find(|&ctor| {
                matches!(
                    self.signatures.members.get(&ctor),
                    Some(MemberSignature::Function(signature)) if signature.parameters.len() == 3
                )
            })?;
        let object = self.allocate_object(ctx, &info_type, span.clone())?;
        let key = FunctionKey {
            symbol: ctor,
            role: Role::Constructor,
            bindings: Vec::new(),
        };
        self.call_function(
            ctx,
            &key,
            Some(object),
            &[name, is_public, attributes],
            &[],
            span,
        );
        Some(object)
    }

    /// The attributes on `member` as an `object[]`, one constructed object
    /// per attribute the checker bound to a source class (see
    /// `BodyCheck::attribute_constructions`); the others — engine and SDK
    /// attributes, which have no representation here — are left out.
    fn attribute_objects(
        &mut self,
        ctx: &mut Ctx<'ast>,
        member: SymbolId,
        span: Range<usize>,
    ) -> DataId {
        let sections = self.attribute_sections(member);
        let mut built = Vec::new();
        for attribute in sections
            .iter()
            .flat_map(|sections| sections.iter())
            .flat_map(|section| section.attributes.iter())
        {
            {
                let id = EntityID::from(attribute);
                let Some(ty) = self.bodies.attribute_types.get(&id).cloned() else {
                    continue;
                };
                let arguments = attribute
                    .arguments
                    .as_ref()
                    .map(|list| list.arguments)
                    .unwrap_or(&[]);
                // the construction the checker bound — or, for a class
                // that declares no constructor, the synthesized default
                let (key, values) = match self.bodies.targets.get(&id).cloned() {
                    Some(ResolvedTarget::Call(call)) => {
                        let MemberOrigin::Source(ctor) = call.origin else {
                            continue;
                        };
                        let Some(values) = self.constructor_arguments(ctx, &call, arguments) else {
                            continue;
                        };
                        let key = FunctionKey {
                            symbol: ctor,
                            role: Role::Constructor,
                            bindings: self.bindings_for(ctx, ctor, &call.declaring_type, &[]),
                        };
                        (key, values)
                    }
                    _ => {
                        let Type::Named {
                            target: TypeTarget::Source(class),
                            ..
                        } = &ty
                        else {
                            continue;
                        };
                        let key = FunctionKey {
                            symbol: *class,
                            role: Role::DefaultConstructor,
                            bindings: Vec::new(),
                        };
                        (key, Vec::new())
                    }
                };
                let Some(object) = self.allocate_object(ctx, &ty, span.clone()) else {
                    continue;
                };
                self.call_function(ctx, &key, Some(object), &values, &[], span.clone());
                built.push(object);
            }
        }
        let object_type = self.corlib_type("Object");
        let array_type = Type::Array {
            element: Box::new(object_type),
            rank: 1,
        };
        let size = self.int_constant(built.len() as i32);
        let array = self.allocate_array(ctx, &array_type, size, span.clone());
        for (position, object) in built.into_iter().enumerate() {
            let index = self.int_constant(position as i32);
            self.set_element(ctx, array, index, object, span.clone());
        }
        array
    }
}
