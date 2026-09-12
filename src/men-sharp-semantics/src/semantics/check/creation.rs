//! Making and indexing things: `new`, initializers, arrays and slices.

use super::{
    ArgumentShape, CallArgument, Checker, Meaning, MethodGroup, ResolvedMember, ResolvedTarget,
};
use crate::error::SemanticErrorKind;
use crate::semantics::resolve::apply_suffixes;
use crate::symbol::SymbolKind;
use crate::types::external::INDEXER_LOOKUP_NAME;
use crate::types::infer::best_common_type;
use crate::types::lookup::MemberCandidate;
use crate::types::{MemberSignature, Type, TypeTarget};
use men_sharp_parser::ast::{
    Argument, ArgumentValue, EntityID, Expression, InitializerValue, UnaryOperator,
};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    // ------------------------------------------------- initializer lists

    /// `{ 1, 2 }` where a value was expected: C# reads it as
    /// `new T[] { 1, 2 }`, and only for an array type (CS0622).
    pub(super) fn check_array_initializer(
        &mut self,
        initializer: &'ast men_sharp_parser::ast::Initializer<'ast, 'ast>,
        declared: &Type,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};

        let element = match declared {
            Type::Error => return,
            Type::Array { element, rank: 1 } => (**element).clone(),
            Type::Array { element, rank } => {
                let element = (**element).clone();
                self.check_rectangular_initializer(initializer, &element, *rank);
                return;
            }
            other => {
                let kind = SemanticErrorKind::TypeMismatch {
                    expected: self.describe(other),
                    found: "array initializer".to_string(),
                };
                self.error(kind, initializer.span());
                return;
            }
        };
        let elements = match initializer {
            Initializer::Collection { elements, .. } => *elements,
            // `= { }`: nothing between the braces reads as an object
            // initializer, and is the empty array
            Initializer::Object { elements: [], .. } => &[][..],
            Initializer::Object { .. } => {
                let kind = SemanticErrorKind::TypeMismatch {
                    expected: self.describe(declared),
                    found: "object initializer".to_string(),
                };
                self.error(kind, initializer.span());
                return;
            }
        };
        for written in elements {
            match written {
                CollectionElement::Expression(expression) => {
                    let literal = Self::is_integer_literal(expression);
                    let ty = self.check_expression_expecting(expression, Some(&element));
                    self.require_convertible(&ty, &element, literal, expression.span());
                }
                // `{ { 1, 2 }, { 3, 4 } }`: a jagged or rectangular array
                CollectionElement::Nested(nested) => {
                    self.error(SemanticErrorKind::UnsupportedExpression, nested.span());
                }
            }
        }
    }

    /// `{ { 1, 2 }, { 3, 4 } }` for a `T[,]`: `rank` levels of braces,
    /// every row at one level as long as its siblings, and the leaves
    /// convertible to `T`.
    fn check_rectangular_initializer(
        &mut self,
        initializer: &'ast men_sharp_parser::ast::Initializer<'ast, 'ast>,
        element: &Type,
        rank: u32,
    ) {
        use men_sharp_parser::ast::Initializer;
        let elements = match initializer {
            Initializer::Collection { elements, .. } => *elements,
            Initializer::Object { elements: [], .. } => &[][..],
            Initializer::Object { .. } => {
                let kind = SemanticErrorKind::TypeMismatch {
                    expected: format!(
                        "{}[{}]",
                        self.describe(element),
                        ",".repeat(rank as usize - 1)
                    ),
                    found: "object initializer".to_string(),
                };
                self.error(kind, initializer.span());
                return;
            }
        };
        // one expected length per level, fixed by the first row seen there
        let mut lengths: Vec<Option<usize>> = vec![None; rank as usize];
        self.check_rectangular_level(
            elements,
            element,
            rank,
            0,
            &mut lengths,
            &initializer.span(),
        );
    }

    fn check_rectangular_level(
        &mut self,
        elements: &'ast [men_sharp_parser::ast::CollectionElement<'ast, 'ast>],
        element: &Type,
        rank: u32,
        depth: usize,
        lengths: &mut [Option<usize>],
        span: &Range<usize>,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};
        match lengths[depth] {
            Some(expected) if expected != elements.len() => {
                self.error(SemanticErrorKind::RaggedArrayInitializer, span.clone());
            }
            Some(_) => {}
            None => lengths[depth] = Some(elements.len()),
        }
        let innermost = depth + 1 == rank as usize;
        for written in elements {
            match (written, innermost) {
                (CollectionElement::Expression(expression), true) => {
                    let literal = Self::is_integer_literal(expression);
                    let ty = self.check_expression_expecting(expression, Some(element));
                    self.require_convertible(&ty, element, literal, expression.span());
                }
                (CollectionElement::Nested(Initializer::Collection { elements, span }), false) => {
                    self.check_rectangular_level(elements, element, rank, depth + 1, lengths, span);
                }
                // a leaf where a row was due, or a row where a leaf was
                (CollectionElement::Expression(expression), false) => {
                    self.error(SemanticErrorKind::RaggedArrayInitializer, expression.span());
                }
                (CollectionElement::Nested(nested), _) => {
                    self.error(SemanticErrorKind::RaggedArrayInitializer, nested.span());
                }
            }
        }
    }

    /// `a[i]`, `text[i]`: every index has to be an `int`.
    fn require_integer_indices(&mut self, arguments: &[CallArgument<'ast>]) {
        let int32 = self.corlib("Int32");
        for argument in arguments {
            self.require_convertible(
                &argument.value_type(),
                &int32,
                argument.is_integer_literal,
                argument.span.clone(),
            );
        }
    }

    // ------------------------------------------------------------- new / []

    pub(super) fn check_new(
        &mut self,
        new_expression: &'ast men_sharp_parser::ast::NewExpression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Meaning<'ast> {
        // `new[] { ... }`: the element type is the best common type of the elements
        if new_expression.created_type.is_none() && !new_expression.array_suffixes.is_empty() {
            use men_sharp_parser::ast::{CollectionElement, Initializer};
            let mut element_types = Vec::new();
            if let Some(Initializer::Collection { elements, .. }) = &new_expression.initializer {
                for element in *elements {
                    if let CollectionElement::Expression(expression) = element {
                        element_types.push(self.check_expression(expression));
                    }
                }
            }
            let system = self.system();
            let element = match best_common_type(&system, &element_types) {
                Some(element) => element,
                None => {
                    self.error(
                        SemanticErrorKind::TypeAnnotationNeeded,
                        new_expression.span.clone(),
                    );
                    Type::Error
                }
            };
            let ty = Type::Array {
                element: Box::new(element),
                rank: 1,
            };
            // `new[]` writes no type; the code generator reads it from here
            self.expression_types
                .insert(EntityID::from(new_expression), ty.clone());
            return Meaning::Value(ty);
        }

        // array creation
        if new_expression.is_array_creation() {
            let element = match &new_expression.created_type {
                Some(created_type) => self.resolve_type(created_type),
                None => Type::Error,
            };
            for size in new_expression.array_sizes {
                self.check_expression(size);
            }
            let ty = if new_expression.array_sizes.is_empty() {
                // `new int[] { ... }`: the written type already carries its ranks
                element
            } else {
                let inner = apply_suffixes(element, new_expression.array_suffixes);
                Type::Array {
                    element: Box::new(inner),
                    rank: new_expression.array_sizes.len() as u32,
                }
            };
            self.check_initializer(&new_expression.initializer, &ty);
            return Meaning::Value(ty);
        }

        let ty = match (&new_expression.created_type, expected) {
            (Some(created_type), _) => self.resolve_type(created_type),
            // target-typed `new(...)` takes the context's type; it writes no
            // type of its own, so the code generator reads it from here
            (None, Some(expected)) => {
                let expected = match expected {
                    // `MyStruct? m = new();` makes the value, not the null
                    Type::Nullable(inner) => (**inner).clone(),
                    other => other.clone(),
                };
                self.expression_types
                    .insert(EntityID::from(new_expression), expected.clone());
                expected
            }
            (None, None) => {
                self.error(
                    SemanticErrorKind::TypeAnnotationNeeded,
                    new_expression.span.clone(),
                );
                return Meaning::Error;
            }
        };
        if matches!(ty, Type::Error) {
            return Meaning::Error;
        }

        // constructor overloads declared on the type itself
        if let Type::Named {
            target: TypeTarget::Source(class),
            ..
        } = &ty
            && self.is_abstract_type(*class)
        {
            let kind = SemanticErrorKind::CannotInstantiateAbstractType {
                type_name: self.describe(&ty),
            };
            self.error(kind, new_expression.span.clone());
        }

        let constructors: Vec<MemberCandidate> = self
            .system()
            .members_named(&ty, ".ctor")
            .into_iter()
            .filter(|candidate| {
                candidate.kind == SymbolKind::Constructor
                    && !candidate.is_static
                    && candidate.declaring_type == ty
            })
            .collect();

        let arguments = new_expression
            .arguments
            .as_ref()
            .map(|list| self.check_arguments(list.arguments))
            .unwrap_or_default();

        // a struct always has a parameterless constructor (§16.4.9), whether
        // or not one is declared next to the others
        let implicit_struct_default = arguments.is_empty()
            && self.is_struct(&ty)
            && !constructors.iter().any(|candidate| {
                matches!(
                    &candidate.signature,
                    Some(MemberSignature::Function(function)) if function.parameters.is_empty()
                )
            });
        if constructors.is_empty() || implicit_struct_default {
            // the implicit parameterless constructor
            if !arguments.is_empty() {
                self.error(
                    SemanticErrorKind::NoMatchingOverload,
                    new_expression.span.clone(),
                );
            }
        } else {
            let receiver_display = self.describe(&ty);
            let group = MethodGroup {
                candidates: constructors,
                explicit_arguments: Vec::new(),
                via_type: false,
                name: ".ctor",
                receiver: None,
                allow_extensions: false,
                receiver_display,
                span: new_expression.span.clone(),
            };
            self.resolve_call(
                group,
                arguments,
                &new_expression.span,
                Some(EntityID::from(new_expression)),
            );
        }

        self.check_initializer(&new_expression.initializer, &ty);
        Meaning::Value(ty)
    }

    fn check_initializer(
        &mut self,
        initializer: &'ast Option<men_sharp_parser::ast::Initializer<'ast, 'ast>>,
        ty: &Type,
    ) {
        if let Some(initializer) = initializer {
            self.check_initializer_value(initializer, ty);
        }
    }

    pub(super) fn check_initializer_value(
        &mut self,
        initializer: &'ast men_sharp_parser::ast::Initializer<'ast, 'ast>,
        ty: &Type,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer, InitializerTarget};
        match initializer {
            Initializer::Object { elements, .. } => {
                for element in *elements {
                    // `new D { [k] = v }` is `d[k] = v`: the indexer binds like
                    // any element access, recorded on the element
                    if let InitializerTarget::Index { arguments, span } = &element.target {
                        let call_arguments: Vec<CallArgument<'ast>> = arguments
                            .iter()
                            .map(|argument| self.plain_argument(argument))
                            .collect();
                        let element_type = match self.index_with(
                            ty.clone(),
                            call_arguments,
                            span,
                            Some(EntityID::from(element)),
                        ) {
                            Meaning::Value(element_type) => element_type,
                            _ => Type::Error,
                        };
                        if let Ok(InitializerValue::Expression(value)) = &element.value {
                            let literal = Self::is_integer_literal(value);
                            let value_type =
                                self.check_expression_expecting(value, Some(&element_type));
                            self.require_convertible(
                                &value_type,
                                &element_type,
                                literal,
                                value.span(),
                            );
                        }
                        continue;
                    }
                    if let InitializerTarget::Member(name) = &element.target {
                        // `new T { X = v }` is `t.X = v`: the member binds
                        // like any other write, recorded on the element for
                        // the code generator
                        let member = self
                            .system()
                            .members_named(ty, name.value)
                            .into_iter()
                            .find_map(|candidate| match &candidate.signature {
                                Some(MemberSignature::Field(member_type))
                                | Some(MemberSignature::Property(member_type))
                                    if !candidate.is_static =>
                                {
                                    Some((member_type.clone(), candidate))
                                }
                                _ => None,
                            });
                        let Some((member_type, candidate)) = member else {
                            let kind = SemanticErrorKind::UnknownMember {
                                type_name: self.describe(ty),
                            };
                            self.error(kind, name.span.clone());
                            continue;
                        };
                        self.targets.insert(
                            EntityID::from(element),
                            ResolvedTarget::Member(ResolvedMember {
                                origin: candidate.origin,
                                kind: candidate.kind,
                                is_static: candidate.is_static,
                                declaring_type: candidate.declaring_type,
                                member_type: member_type.clone(),
                            }),
                        );
                        if let Ok(InitializerValue::Expression(value)) = &element.value {
                            let literal = Self::is_integer_literal(value);
                            let value_type =
                                self.check_expression_expecting(value, Some(&member_type));
                            self.require_convertible(
                                &value_type,
                                &member_type,
                                literal,
                                value.span(),
                            );
                        }
                    }
                }
            }
            Initializer::Collection { elements, .. } => {
                if let Type::Array { element, rank } = ty
                    && *rank > 1
                {
                    let element = (**element).clone();
                    self.check_rectangular_initializer(initializer, &element, *rank);
                    return;
                }
                if let Type::Array { element, .. } = ty {
                    for item in *elements {
                        if let CollectionElement::Expression(expression) = item {
                            let value_type = self.check_expression(expression);
                            let literal = Self::is_integer_literal(expression);
                            self.require_convertible(
                                &value_type,
                                element,
                                literal,
                                expression.span(),
                            );
                        }
                    }
                    return;
                }
                if matches!(ty, Type::Error | Type::Dynamic) {
                    return;
                }
                // `new C { a, b }` is `Add(a); Add(b);` (§12.8.17.4): each
                // element is one overload resolution against the type's `Add`,
                // recorded on the element node for the code generator
                for item in *elements {
                    self.check_collection_element(item, ty);
                }
            }
        }
    }

    fn check_collection_element(
        &mut self,
        item: &'ast men_sharp_parser::ast::CollectionElement<'ast, 'ast>,
        collection: &Type,
    ) {
        use men_sharp_parser::ast::{CollectionElement, Initializer};
        let (arguments, span): (Vec<CallArgument<'ast>>, Range<usize>) = match item {
            CollectionElement::Expression(expression) => {
                (vec![self.plain_argument(expression)], expression.span())
            }
            // `{ k, v }`: one `Add` with several arguments
            CollectionElement::Nested(Initializer::Collection { elements, span }) => {
                let mut arguments = Vec::with_capacity(elements.len());
                for inner in *elements {
                    match inner {
                        CollectionElement::Expression(expression) => {
                            arguments.push(self.plain_argument(expression));
                        }
                        CollectionElement::Nested(initializer) => {
                            self.error(
                                SemanticErrorKind::UnsupportedExpression,
                                initializer.span(),
                            );
                            return;
                        }
                    }
                }
                (arguments, span.clone())
            }
            CollectionElement::Nested(initializer) => {
                self.error(SemanticErrorKind::UnsupportedExpression, initializer.span());
                return;
            }
        };
        let candidates: Vec<MemberCandidate> = self
            .system()
            .members_named(collection, "Add")
            .into_iter()
            .filter(|candidate| candidate.kind == SymbolKind::Method && !candidate.is_static)
            .collect();
        if candidates.is_empty() {
            let kind = SemanticErrorKind::UnknownMember {
                type_name: self.describe(collection),
            };
            self.error(kind, span);
            return;
        }
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: false,
            name: "Add",
            receiver: Some(collection.clone()),
            allow_extensions: false,
            receiver_display: self.describe(collection),
            span: span.clone(),
        };
        self.resolve_call(group, arguments, &span, Some(EntityID::from(item)));
    }

    /// A by-value argument built from a bare expression (no `Argument` node:
    /// collection-initializer elements).
    fn plain_argument(&mut self, expression: &'ast Expression<'ast, 'ast>) -> CallArgument<'ast> {
        CallArgument {
            shape: ArgumentShape::Value(self.check_expression(expression)),
            name: None,
            expression: Some(expression),
            modifier: None,
            is_integer_literal: Self::is_integer_literal(expression),
            out_declaration: None,
            is_receiver: false,
            span: expression.span(),
        }
    }

    pub(super) fn index(
        &mut self,
        receiver: Type,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        // `a[^1]` and `a[1..3]`: Udon has no `Index`/`Range` values, so
        // these are read as syntax where they are written and lowered in
        // place — on arrays and strings, the two C# gives them to natively
        if let [argument] = arguments
            && argument.name.is_none()
            && argument.modifier.is_none()
            && let ArgumentValue::Expression(expression) = &argument.value
        {
            match expression {
                Expression::Range(range) => return self.check_slice(&receiver, range, span),
                Expression::Unary(unary) if unary.operator.value == UnaryOperator::IndexFromEnd => {
                    self.check_index_from_end(unary);
                    if !self.is_indexable_from_end(&receiver) {
                        let kind = SemanticErrorKind::NotIndexable {
                            type_name: self.describe(&receiver),
                        };
                        self.error(kind, unary.span.clone());
                        return Meaning::Error;
                    }
                    let call_arguments = vec![CallArgument {
                        shape: ArgumentShape::Value(self.corlib("Int32")),
                        name: None,
                        expression: None,
                        modifier: None,
                        is_integer_literal: false,
                        out_declaration: None,
                        is_receiver: false,
                        span: argument.span.clone(),
                    }];
                    return self.index_with(receiver, call_arguments, span, node);
                }
                _ => {}
            }
        }
        let call_arguments = self.check_arguments(arguments);
        self.index_with(receiver, call_arguments, span, node)
    }

    /// `^k`: an `int` counted back from the end. Only where the compiler
    /// knows the length without asking a member — arrays and strings.
    fn check_index_from_end(
        &mut self,
        unary: &'ast men_sharp_parser::ast::UnaryExpression<'ast, 'ast>,
    ) {
        self.expression_types
            .insert(EntityID::from(unary), self.corlib("Int32"));
        if let Ok(operand) = &unary.operand {
            let literal = Self::is_integer_literal(operand);
            let int32 = self.corlib("Int32");
            let ty = self.check_expression_expecting(operand, Some(&int32));
            self.require_convertible(&ty, &int32, literal, operand.span());
        }
    }

    /// An array or a string: what `^i` and `i..j` are lowered for.
    fn is_indexable_from_end(&self, receiver: &Type) -> bool {
        matches!(receiver, Type::Array { rank: 1, .. } | Type::Error)
            || self.system().is_string(receiver)
    }

    /// `a[1..^1]`: a slice, whose type is the sliced type itself.
    fn check_slice(
        &mut self,
        receiver: &Type,
        range: &'ast men_sharp_parser::ast::RangeExpression<'ast, 'ast>,
        span: &Range<usize>,
    ) -> Meaning<'ast> {
        for endpoint in [&range.start, &range.end].into_iter().flatten() {
            match endpoint {
                Expression::Unary(unary) if unary.operator.value == UnaryOperator::IndexFromEnd => {
                    self.check_index_from_end(unary);
                }
                other => {
                    let literal = Self::is_integer_literal(other);
                    let int32 = self.corlib("Int32");
                    let ty = self.check_expression_expecting(other, Some(&int32));
                    self.require_convertible(&ty, &int32, literal, other.span());
                }
            }
        }
        if !self.is_indexable_from_end(receiver) {
            let kind = SemanticErrorKind::NotIndexable {
                type_name: self.describe(receiver),
            };
            self.error(kind, span.clone());
            return Meaning::Error;
        }
        Meaning::Value(receiver.clone())
    }

    fn index_with(
        &mut self,
        receiver: Type,
        call_arguments: Vec<CallArgument<'ast>>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        match &receiver {
            Type::Array { element, rank } => {
                self.require_integer_indices(&call_arguments);
                if call_arguments.len() != *rank as usize {
                    let kind = SemanticErrorKind::WrongNumberOfIndices { expected: *rank };
                    self.error(kind, span.clone());
                    return Meaning::Error;
                }
                Meaning::Value((**element).clone())
            }
            Type::Error => Meaning::Error,
            // `text[i]`: `string` has an indexer, but no member the lookup
            // below would find — the code generator lowers it in place
            _ if self.system().is_string(&receiver) => {
                self.require_integer_indices(&call_arguments);
                Meaning::Value(self.corlib("Char"))
            }
            _ => {
                // indexers: `this[]` from source, and the provider answers
                // the same name with metadata's parameterised properties —
                // by whatever name `[IndexerName]` gave them
                let candidates = self.system().members_named(&receiver, INDEXER_LOOKUP_NAME);
                let indexers: Vec<MemberCandidate> = candidates
                    .into_iter()
                    .filter(|candidate| {
                        matches!(candidate.signature, Some(MemberSignature::Function(_)))
                    })
                    .collect();

                if indexers.is_empty() {
                    let kind = SemanticErrorKind::NotIndexable {
                        type_name: self.describe(&receiver),
                    };
                    self.error(kind, span.clone());
                    return Meaning::Error;
                }

                let receiver_display = self.describe(&receiver);
                let group = MethodGroup {
                    candidates: indexers,
                    explicit_arguments: Vec::new(),
                    via_type: false,
                    name: "this[]",
                    receiver: None,
                    allow_extensions: false,
                    receiver_display,
                    span: span.clone(),
                };
                Meaning::Value(self.resolve_call(group, call_arguments, span, node))
            }
        }
    }
}
