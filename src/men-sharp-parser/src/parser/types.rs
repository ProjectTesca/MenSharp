use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        ConstraintBound, GenericsDefine, GenericsInfo, GenericsParameter, NameSegment, NameType,
        PredefinedType, Spanned, TupleType, TupleTypeElement, TypeParameterConstraint, TypeRef,
        TypeRefBase, TypeSuffix, Variance,
    },
    error::{ParseErrorKind, recover_until},
    lexer::{Lexer, TokenKind},
    parser::{Errors, ParserLexer, alloc_slice, declaration::parse_attribute_sections},
};

/// The built-in type keyword for `kind`, if it is one.
pub(crate) fn predefined_type(kind: TokenKind) -> Option<PredefinedType> {
    Some(match kind {
        TokenKind::Bool => PredefinedType::Bool,
        TokenKind::Byte => PredefinedType::Byte,
        TokenKind::Sbyte => PredefinedType::Sbyte,
        TokenKind::Short => PredefinedType::Short,
        TokenKind::Ushort => PredefinedType::Ushort,
        TokenKind::Int => PredefinedType::Int,
        TokenKind::Uint => PredefinedType::Uint,
        TokenKind::Long => PredefinedType::Long,
        TokenKind::Ulong => PredefinedType::Ulong,
        TokenKind::Char => PredefinedType::Char,
        TokenKind::Float => PredefinedType::Float,
        TokenKind::Double => PredefinedType::Double,
        TokenKind::Decimal => PredefinedType::Decimal,
        TokenKind::Str => PredefinedType::String,
        TokenKind::Object => PredefinedType::Object,
        TokenKind::Void => PredefinedType::Void,
        TokenKind::Nint => PredefinedType::Nint,
        TokenKind::Nuint => PredefinedType::Nuint,
        TokenKind::Dynamic => PredefinedType::Dynamic,
        _ => return None,
    })
}

/// Parses a type reference, restoring the cursor if the tokens do not form one.
///
/// Because almost any expression prefix also parses as a type, callers use this
/// speculatively and check what follows before committing.
pub(crate) fn parse_type<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<TypeRef<'input, 'allocator>> {
    parse_type_in(lexer, errors, allocator, false)
}

/// [`parse_type`] for the type after `is` / `as`, which may sit at the end
/// of an expression: there a trailing `?` is the conditional operator when
/// what follows it can start an expression (`x is T ? a : b`), and the
/// nullable suffix only otherwise (`x is T? t`) — C#'s own disambiguation.
pub(crate) fn parse_type_after_is<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<TypeRef<'input, 'allocator>> {
    parse_type_in(lexer, errors, allocator, true)
}

fn parse_type_in<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    after_is: bool,
) -> Option<TypeRef<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    // `ref T` / `ref readonly T`
    if let Some(ref_keyword) = lexer.eat(TokenKind::Ref) {
        let readonly_keyword = lexer.eat(TokenKind::Readonly);

        let Some(element) = parse_type(lexer, errors, allocator) else {
            lexer.back_to_anchor(anchor);
            return None;
        };

        let span = anchor.elapsed(lexer);
        return Some(TypeRef {
            base: TypeRefBase::Ref {
                ref_keyword,
                readonly_keyword,
                element: allocator.alloc(element),
                span: span.clone(),
            },
            suffixes: &[],
            span,
        });
    }

    let base = if let Some(predefined) = predefined_type(lexer.kind()) {
        TypeRefBase::Predefined(Spanned::new(predefined, lexer.take_span()))
    } else if lexer.kind() == TokenKind::Var {
        TypeRefBase::Var(lexer.take_span())
    } else if lexer.kind() == TokenKind::ParenthesisLeft {
        TypeRefBase::Tuple(parse_tuple_type(lexer, errors, allocator)?)
    } else {
        TypeRefBase::Name(parse_name_type(lexer, errors, allocator)?)
    };

    // `int* p` is unambiguous whatever the context, because a type keyword cannot be
    // multiplied. `Foo* p` reads exactly like `Foo * p`, so it needs an `unsafe` scope.
    let allow_pointer = matches!(base, TypeRefBase::Predefined(_)) || lexer.unsafe_depth > 0;
    let suffixes = parse_type_suffixes(lexer, allocator, allow_pointer, after_is);

    Some(TypeRef {
        base,
        suffixes,
        span: anchor.elapsed(lexer),
    })
}

/// Parses a type in a position where one is required, recovering to `until` if absent.
pub(crate) fn parse_type_or_recover<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    until: &[TokenKind],
) -> Result<TypeRef<'input, 'allocator>, ()> {
    match parse_type(lexer, errors, allocator) {
        Some(type_ref) => Ok(type_ref),
        None => {
            errors.push(recover_until(lexer, until, ParseErrorKind::MissingType));
            Err(())
        }
    }
}

/// `?`, `[]`, `[,]` and `*`, in written order.
///
/// A `[` that contains anything other than commas belongs to an element access or an
/// array creation size, so it is left alone. `*` is only a suffix when `allow_pointer`
/// says the position cannot be a multiplication.
fn parse_type_suffixes<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    allocator: &'allocator Bump,
    allow_pointer: bool,
    after_is: bool,
) -> &'allocator [TypeSuffix] {
    let mut suffixes = Vec::new_in(allocator);

    loop {
        match lexer.kind() {
            TokenKind::Asterisk if allow_pointer => {
                let span = lexer.take_span();
                suffixes.push(TypeSuffix::Pointer { span });
            }
            TokenKind::QuestionMark => {
                let anchor = lexer.cast_anchor();
                let span = lexer.take_span();
                if after_is && can_start_expression(lexer.kind()) {
                    // `x is T ? a : b`: the `?` belongs to the conditional
                    lexer.back_to_anchor(anchor);
                    break;
                }
                suffixes.push(TypeSuffix::Nullable { span });
            }
            TokenKind::BracketLeft => {
                let anchor = lexer.cast_anchor();
                let start = lexer.take_span().start;

                let mut rank = 1;
                while lexer.eat(TokenKind::Comma).is_some() {
                    rank += 1;
                }

                let Some(end) = lexer.eat(TokenKind::BracketRight) else {
                    lexer.back_to_anchor(anchor);
                    break;
                };

                suffixes.push(TypeSuffix::Array {
                    rank,
                    span: start..end.end,
                });
            }
            _ => break,
        }
    }

    alloc_slice(allocator, suffixes)
}

/// `global::System.Collections.Generic.List<int>`
pub(crate) fn parse_name_type<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<NameType<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let global = (lexer.kind() == TokenKind::Global
        && lexer.lookahead(1) == TokenKind::DoubleColon)
        .then(|| {
            let span = lexer.take_span();
            lexer.next(); // `::`
            span
        });

    let mut segments = Vec::new_in(allocator);

    loop {
        let segment_anchor = lexer.cast_anchor();

        let Some(name) = lexer.eat_ident() else {
            if segments.is_empty() {
                lexer.back_to_anchor(anchor);
                return None;
            }
            // trailing separator: leave it for the caller
            lexer.back_to_anchor(segment_anchor);
            break;
        };

        let generics = parse_generics_info(lexer, errors, allocator);

        segments.push(NameSegment {
            name,
            generics,
            span: segment_anchor.elapsed(lexer),
        });

        if lexer.kind() != TokenKind::Dot && lexer.kind() != TokenKind::DoubleColon {
            break;
        }
        lexer.next();
    }

    Some(NameType {
        global,
        segments: alloc_slice(allocator, segments),
        span: anchor.elapsed(lexer),
    })
}

/// `(int x, string y)` -- at least two elements, which is what tells a tuple type apart
/// from a parenthesised expression.
fn parse_tuple_type<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<TupleType<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    lexer.eat(TokenKind::ParenthesisLeft)?;

    let mut elements = Vec::new_in(allocator);

    loop {
        let element_anchor = lexer.cast_anchor();

        let Some(element_type) = parse_type(lexer, errors, allocator) else {
            lexer.back_to_anchor(anchor);
            return None;
        };
        let name = lexer.eat_ident();

        elements.push(TupleTypeElement {
            element_type,
            name,
            span: element_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if elements.len() < 2 || lexer.eat(TokenKind::ParenthesisRight).is_none() {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(TupleType {
        elements: alloc_slice(allocator, elements),
        span: anchor.elapsed(lexer),
    })
}

/// `<int, string>`, or the unbound `<>` / `<,>` used by `typeof`.
///
/// Fully speculative: `a < b && c > d` must not be mistaken for a generic name.
pub(crate) fn parse_generics_info<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<GenericsInfo<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    lexer.eat(TokenKind::LessThan)?;

    // unbound: `<>`, `<,>`, `<,,>`
    let unbound_anchor = lexer.cast_anchor();
    let mut commas = 0;
    while lexer.eat(TokenKind::Comma).is_some() {
        commas += 1;
    }
    if lexer.eat(TokenKind::GreaterThan).is_some() {
        return Some(GenericsInfo {
            types: &[],
            arity: commas + 1,
            span: anchor.elapsed(lexer),
        });
    }
    lexer.back_to_anchor(unbound_anchor);

    let mut types = Vec::new_in(allocator);

    loop {
        let Some(type_ref) = parse_type(lexer, errors, allocator) else {
            lexer.back_to_anchor(anchor);
            return None;
        };
        types.push(type_ref);

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::GreaterThan).is_none() {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(GenericsInfo {
        arity: types.len(),
        types: alloc_slice(allocator, types),
        span: anchor.elapsed(lexer),
    })
}

/// `<in TIn, out TOut>` on a declaration.
pub(crate) fn parse_generics_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<GenericsDefine<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    lexer.eat(TokenKind::LessThan)?;

    let mut parameters = Vec::new_in(allocator);

    loop {
        let parameter_anchor = lexer.cast_anchor();

        let attributes = parse_attribute_sections(lexer, errors, allocator);

        let variance = match lexer.kind() {
            TokenKind::In => Some(Spanned::new(Variance::In, lexer.take_span())),
            TokenKind::Out => Some(Spanned::new(Variance::Out, lexer.take_span())),
            _ => None,
        };

        let Some(name) = lexer.eat_ident() else {
            lexer.back_to_anchor(anchor);
            return None;
        };

        parameters.push(GenericsParameter {
            attributes,
            variance,
            name,
            span: parameter_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::GreaterThan).is_none() {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(GenericsDefine {
        parameters: alloc_slice(allocator, parameters),
        span: anchor.elapsed(lexer),
    })
}

/// Zero or more `where T : class, IFoo, new()` clauses.
pub(crate) fn parse_type_parameter_constraints<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [TypeParameterConstraint<'input, 'allocator>] {
    let mut constraints = Vec::new_in(allocator);

    while lexer.kind() == TokenKind::Where {
        let anchor = lexer.cast_anchor();
        let where_keyword = lexer.take_span();

        let target = match lexer.eat_ident() {
            Some(target) => Ok(target),
            None => {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::BraceLeft, TokenKind::Semicolon, TokenKind::Where],
                    ParseErrorKind::MissingConstraintTarget,
                ));
                Err(())
            }
        };

        if lexer.eat(TokenKind::Colon).is_none() && target.is_ok() {
            errors.push(recover_until(
                lexer,
                &[TokenKind::BraceLeft, TokenKind::Semicolon, TokenKind::Where],
                ParseErrorKind::MissingConstraintBound,
            ));
        }

        let mut bounds = Vec::new_in(allocator);
        while let Some(bound) = parse_constraint_bound(lexer, errors, allocator) {
            bounds.push(bound);

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }

        constraints.push(TypeParameterConstraint {
            where_keyword,
            target,
            bounds: alloc_slice(allocator, bounds),
            span: anchor.elapsed(lexer),
        });
    }

    alloc_slice(allocator, constraints)
}

fn parse_constraint_bound<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<ConstraintBound<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    match lexer.kind() {
        TokenKind::Class => {
            let span = lexer.take_span();
            let nullable = lexer.eat(TokenKind::QuestionMark);
            let span = match &nullable {
                Some(nullable) => span.start..nullable.end,
                None => span,
            };
            Some(ConstraintBound::Class { nullable, span })
        }
        TokenKind::Struct => Some(ConstraintBound::Struct {
            span: lexer.take_span(),
        }),
        TokenKind::Notnull => Some(ConstraintBound::NotNull {
            span: lexer.take_span(),
        }),
        TokenKind::Unmanaged => Some(ConstraintBound::Unmanaged {
            span: lexer.take_span(),
        }),
        TokenKind::New => {
            lexer.next();
            lexer.eat(TokenKind::ParenthesisLeft);
            lexer.eat(TokenKind::ParenthesisRight);
            Some(ConstraintBound::New {
                span: anchor.elapsed(lexer),
            })
        }
        _ => parse_type(lexer, errors, allocator).map(ConstraintBound::Type),
    }
}

/// Whether a token can begin an expression — what decides, after `is T`,
/// if a `?` is the conditional operator or a nullable suffix.
fn can_start_expression(kind: TokenKind) -> bool {
    predefined_type(kind).is_some()
        || matches!(
            kind,
            TokenKind::Identifier
                | TokenKind::IntegerLiteral
                | TokenKind::RealLiteral
                | TokenKind::CharLiteral
                | TokenKind::StringLiteral
                | TokenKind::VerbatimStringLiteral
                | TokenKind::InterpolatedStringLiteral
                | TokenKind::RawStringLiteral
                | TokenKind::ParenthesisLeft
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Exclamation
                | TokenKind::Tilde
                | TokenKind::DoublePlus
                | TokenKind::DoubleMinus
                | TokenKind::Ampersand
                | TokenKind::Asterisk
                | TokenKind::New
                | TokenKind::Typeof
                | TokenKind::Default
                | TokenKind::This
                | TokenKind::Base
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
                | TokenKind::Sizeof
                | TokenKind::Nameof
                | TokenKind::Checked
                | TokenKind::Unchecked
                | TokenKind::Await
                | TokenKind::Delegate
                | TokenKind::Stackalloc
                | TokenKind::Throw
                | TokenKind::Ref
        )
}
