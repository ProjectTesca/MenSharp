//! What a positional record declares without writing it down.
//!
//! `record Point(int X, int Y);` is, to the rest of the compiler, a class
//! with two `{ get; init; }` properties, a constructor that fills them, and
//! a `Deconstruct` that reads them back — C# §15.?. Those members are
//! synthesized here as ordinary syntax, spans pointing at the parameter
//! each came from, so that name binding, overload resolution and code
//! generation need no notion of "positional" at all. Value equality,
//! `ToString` and `with` are not expressible as members and are the code
//! generator's (see `records.rs` there).
//!
//! A `record struct` makes its properties `{ get; set; }`; a `readonly
//! record struct` makes them `{ get; init; }` like a record class.

use std::ops::Range;

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        Accessor, AccessorKind, AccessorList, ArgumentList, AssignmentExpression,
        AssignmentOperator, Block, ClassKind, ConstructorDeclaration, ConstructorInitializer,
        ConstructorInitializerKind, Documents, Expression, ExpressionStatement, FunctionBody,
        Ident, MemberSeparator, MethodDeclaration, Modifier, Parameter, ParameterList,
        ParameterModifier, PredefinedType, PrimaryExpression, PrimaryLeft, PrimaryRight,
        PropertyDeclaration, Spanned, Statement, TypeMember, TypeRef, TypeRefBase,
    },
    parser::alloc_slice,
};

/// The declared members with the synthesized ones in front of them — the
/// order `ToString` prints and the order a reader expects.
pub(crate) fn with_positional_members<'input, 'allocator>(
    allocator: &'allocator Bump,
    kind: ClassKind,
    modifiers: &[Spanned<Modifier>],
    name: &Ident<'input>,
    parameters: &ParameterList<'input, 'allocator>,
    base_arguments: Option<ArgumentList<'input, 'allocator>>,
    declared: Vec<TypeMember<'input, 'allocator>, &'allocator Bump>,
) -> &'allocator [TypeMember<'input, 'allocator>] {
    let positional: std::vec::Vec<&Parameter<'input, 'allocator>> = parameters
        .parameters
        .iter()
        .filter(|parameter| parameter.parameter_type.is_some() && parameter.name.is_ok())
        .collect();

    // a `record struct` is mutable unless `readonly`
    let settable = kind == ClassKind::RecordStruct
        && !modifiers
            .iter()
            .any(|modifier| modifier.value == Modifier::Readonly);
    let setter = if settable {
        AccessorKind::Set
    } else {
        AccessorKind::Init
    };

    let mut members = Vec::new_in(allocator);
    for parameter in &positional {
        members.push(TypeMember::Property(property(allocator, parameter, setter)));
    }
    members.push(TypeMember::Constructor(constructor(
        allocator,
        name,
        parameters,
        &positional,
        base_arguments,
    )));
    if !positional.is_empty() {
        members.push(TypeMember::Method(deconstruct(
            allocator,
            name,
            &positional,
        )));
    }
    members.extend(declared);
    alloc_slice(allocator, members)
}

fn documents<'input, 'allocator>(span: &Range<usize>) -> Documents<'input, 'allocator> {
    Documents {
        documents: &[],
        span: span.clone(),
    }
}

fn public<'allocator>(
    allocator: &'allocator Bump,
    span: &Range<usize>,
) -> &'allocator [Spanned<Modifier>] {
    let mut modifiers = Vec::new_in(allocator);
    modifiers.push(Spanned::new(Modifier::Public, span.clone()));
    alloc_slice(allocator, modifiers)
}

fn named<'input>(parameter: &Parameter<'input, '_>) -> Ident<'input> {
    parameter
        .name
        .clone()
        .expect("positional parameters were filtered to named ones")
}

fn typed<'input, 'allocator>(
    parameter: &Parameter<'input, 'allocator>,
) -> TypeRef<'input, 'allocator> {
    parameter
        .parameter_type
        .clone()
        .expect("positional parameters were filtered to typed ones")
}

/// `this.X`
fn this_member<'input, 'allocator>(
    allocator: &'allocator Bump,
    name: &Ident<'input>,
) -> Expression<'input, 'allocator> {
    let span = name.span.clone();
    let mut chain = Vec::new_in(allocator);
    chain.push(PrimaryRight::Member {
        separator: Spanned::new(MemberSeparator::Dot, span.clone()),
        name: Ok(name.clone()),
        generics: None,
        span: span.clone(),
    });
    Expression::Primary(allocator.alloc(PrimaryExpression {
        left: PrimaryLeft::This(span.clone()),
        chain: alloc_slice(allocator, chain),
        span,
    }))
}

/// `X`
fn identifier<'input, 'allocator>(
    allocator: &'allocator Bump,
    name: &Ident<'input>,
) -> Expression<'input, 'allocator> {
    let span = name.span.clone();
    Expression::Primary(allocator.alloc(PrimaryExpression {
        left: PrimaryLeft::Identifier {
            name: name.clone(),
            generics: None,
            span: span.clone(),
        },
        chain: &[],
        span,
    }))
}

/// `target = value;`
fn assignment<'input, 'allocator>(
    allocator: &'allocator Bump,
    target: Expression<'input, 'allocator>,
    value: Expression<'input, 'allocator>,
    span: &Range<usize>,
) -> Statement<'input, 'allocator> {
    let expression = Expression::Assignment(allocator.alloc(AssignmentExpression {
        target,
        operator: Spanned::new(AssignmentOperator::Assign, span.clone()),
        value: Ok(value),
        span: span.clone(),
    }));
    Statement::Expression(ExpressionStatement {
        expression,
        semicolon: Some(span.clone()),
        span: span.clone(),
    })
}

/// `public T X { get; init; }` (or `set`)
fn property<'input, 'allocator>(
    allocator: &'allocator Bump,
    parameter: &Parameter<'input, 'allocator>,
    setter: AccessorKind,
) -> PropertyDeclaration<'input, 'allocator> {
    let span = parameter.span.clone();
    let mut accessors = Vec::new_in(allocator);
    for kind in [AccessorKind::Get, setter] {
        accessors.push(Accessor {
            attributes: &[],
            modifiers: &[],
            kind: Spanned::new(kind, span.clone()),
            body: FunctionBody::None {
                semicolon: span.clone(),
            },
            span: span.clone(),
        });
    }
    PropertyDeclaration {
        documents: documents(&span),
        attributes: &[],
        modifiers: public(allocator, &span),
        property_type: typed(parameter),
        explicit_interface: None,
        name: named(parameter),
        body: FunctionBody::Accessors(AccessorList {
            accessors: alloc_slice(allocator, accessors),
            span: span.clone(),
        }),
        initializer: None,
        positional: true,
        span,
    }
}

/// `public R(T X, T Y) : base(...) { this.X = X; this.Y = Y; }` — the
/// parameter list is the record's own, defaults included.
fn constructor<'input, 'allocator>(
    allocator: &'allocator Bump,
    name: &Ident<'input>,
    parameters: &ParameterList<'input, 'allocator>,
    positional: &[&Parameter<'input, 'allocator>],
    base_arguments: Option<ArgumentList<'input, 'allocator>>,
) -> ConstructorDeclaration<'input, 'allocator> {
    let span = parameters.span.clone();
    let mut statements = Vec::new_in(allocator);
    for parameter in positional {
        let name = named(parameter);
        statements.push(assignment(
            allocator,
            this_member(allocator, &name),
            identifier(allocator, &name),
            &parameter.span,
        ));
    }
    let initializer = base_arguments.map(|arguments| ConstructorInitializer {
        colon: arguments.span.clone(),
        kind: Spanned::new(ConstructorInitializerKind::Base, arguments.span.clone()),
        span: arguments.span.clone(),
        arguments: Ok(arguments),
    });
    ConstructorDeclaration {
        documents: documents(&span),
        attributes: &[],
        modifiers: public(allocator, &span),
        name: name.clone(),
        parameters: Ok(ParameterList {
            parameters: parameters.parameters,
            span: span.clone(),
        }),
        initializer,
        body: FunctionBody::Block(Block {
            statements: alloc_slice(allocator, statements),
            span: span.clone(),
        }),
        span,
    }
}

/// `public void Deconstruct(out T X, out T Y) { X = this.X; Y = this.Y; }`
fn deconstruct<'input, 'allocator>(
    allocator: &'allocator Bump,
    name: &Ident<'input>,
    positional: &[&Parameter<'input, 'allocator>],
) -> MethodDeclaration<'input, 'allocator> {
    let span = name.span.clone();
    let mut parameters = Vec::new_in(allocator);
    let mut statements = Vec::new_in(allocator);
    for parameter in positional {
        let parameter_name = named(parameter);
        let mut modifiers = Vec::new_in(allocator);
        modifiers.push(Spanned::new(ParameterModifier::Out, parameter.span.clone()));
        parameters.push(Parameter {
            attributes: &[],
            modifiers: alloc_slice(allocator, modifiers),
            parameter_type: Some(typed(parameter)),
            name: Ok(parameter_name.clone()),
            default_value: None,
            span: parameter.span.clone(),
        });
        statements.push(assignment(
            allocator,
            identifier(allocator, &parameter_name),
            this_member(allocator, &parameter_name),
            &parameter.span,
        ));
    }
    MethodDeclaration {
        documents: documents(&span),
        attributes: &[],
        modifiers: public(allocator, &span),
        return_type: TypeRef {
            base: TypeRefBase::Predefined(Spanned::new(PredefinedType::Void, span.clone())),
            suffixes: &[],
            span: span.clone(),
        },
        explicit_interface: None,
        name: Spanned::new("Deconstruct", span.clone()),
        generics: None,
        parameters: Ok(ParameterList {
            parameters: alloc_slice(allocator, parameters),
            span: span.clone(),
        }),
        constraints: &[],
        body: FunctionBody::Block(Block {
            statements: alloc_slice(allocator, statements),
            span: span.clone(),
        }),
        span,
    }
}
