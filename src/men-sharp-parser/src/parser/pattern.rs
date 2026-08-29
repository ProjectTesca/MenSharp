//! Patterns, as used by `is`, `case` and switch expression arms.
//!
//! A bare name is always parsed as [`Pattern::Declaration`], even when it is really a
//! constant such as `Color.Red`. The parser cannot tell a type from a constant without
//! name resolution, so that call is left to a later pass. Literals are unambiguous and
//! do become [`Pattern::Constant`].

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        Pattern, PositionalSubpattern, PropertySubpattern, RelationalOperator, Spanned,
        VariableDesignation,
    },
    error::{ParseErrorKind, recover_until_balanced},
    lexer::{Lexer, TokenKind},
    parser::{
        Errors, ParserLexer, alloc_slice,
        expression::EXPRESSION_RECOVERY,
        types::{parse_type, predefined_type},
    },
};

pub(crate) fn parse_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Pattern<'input, 'allocator>> {
    parse_or_pattern(lexer, errors, allocator)
}

fn parse_or_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Pattern<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut left = parse_and_pattern(lexer, errors, allocator)?;

    while let Some(or_keyword) = lexer.eat(TokenKind::Or) {
        let right = match parse_and_pattern(lexer, errors, allocator) {
            Some(right) => Ok(&*allocator.alloc(right)),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingPattern,
                ));
                Err(())
            }
        };
        let failed = right.is_err();

        left = Pattern::Or {
            left: allocator.alloc(left),
            or_keyword,
            right,
            span: anchor.elapsed(lexer),
        };

        if failed {
            break;
        }
    }

    Some(left)
}

fn parse_and_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Pattern<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut left = parse_unary_pattern(lexer, errors, allocator)?;

    while let Some(and_keyword) = lexer.eat(TokenKind::And) {
        let right = match parse_unary_pattern(lexer, errors, allocator) {
            Some(right) => Ok(&*allocator.alloc(right)),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingPattern,
                ));
                Err(())
            }
        };
        let failed = right.is_err();

        left = Pattern::And {
            left: allocator.alloc(left),
            and_keyword,
            right,
            span: anchor.elapsed(lexer),
        };

        if failed {
            break;
        }
    }

    Some(left)
}

fn parse_unary_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Pattern<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    if let Some(not_keyword) = lexer.eat(TokenKind::Not) {
        let pattern = match parse_unary_pattern(lexer, errors, allocator) {
            Some(pattern) => Ok(&*allocator.alloc(pattern)),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingPattern,
                ));
                Err(())
            }
        };

        return Some(Pattern::Not {
            not_keyword,
            pattern,
            span: anchor.elapsed(lexer),
        });
    }

    parse_primary_pattern(lexer, errors, allocator)
}

fn parse_primary_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Pattern<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    // `_`
    if lexer.kind() == TokenKind::Identifier && lexer.current().map(|token| token.text) == Some("_")
    {
        return Some(Pattern::Discard(lexer.take_span()));
    }

    // `( pattern )` or the positional `( p1, p2 )` -- see `parse_parenthesized_or_positional`
    if lexer.kind() == TokenKind::ParenthesisLeft {
        return parse_parenthesized_or_positional(lexer, errors, allocator, None, &anchor);
    }

    // `[ 1, 2, .. ]`
    if lexer.kind() == TokenKind::BracketLeft {
        return Some(parse_list_pattern(lexer, errors, allocator, &anchor));
    }

    // `> 0`, `<= 10`
    let relational = match lexer.kind() {
        TokenKind::LessThan => Some(RelationalOperator::LessThan),
        TokenKind::GreaterThan => Some(RelationalOperator::GreaterThan),
        TokenKind::LessThanEqual => Some(RelationalOperator::LessThanEqual),
        TokenKind::GreaterThanEqual => Some(RelationalOperator::GreaterThanEqual),
        _ => None,
    };
    if let Some(relational) = relational {
        let operator = Spanned::new(relational, lexer.take_span());
        let value = match super::expression::parse_expression(lexer, errors, allocator) {
            Some(value) => Ok(value),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingExpression,
                ));
                Err(())
            }
        };

        return Some(Pattern::Relational {
            operator,
            value,
            span: anchor.elapsed(lexer),
        });
    }

    // `var x` and `var (a, b)`
    if lexer.kind() == TokenKind::Var {
        let var_keyword = lexer.take_span();
        let designation = match parse_variable_designation(lexer, errors, allocator) {
            Some(designation) => Ok(designation),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingSubpatternName,
                ));
                Err(())
            }
        };

        return Some(Pattern::Var {
            var_keyword,
            designation,
            span: anchor.elapsed(lexer),
        });
    }

    // `{ X: 1 }` with no type in front
    if lexer.kind() == TokenKind::BraceLeft {
        let subpatterns = parse_property_subpatterns(lexer, errors, allocator);
        let designation = eat_designation(lexer);

        return Some(Pattern::Property {
            pattern_type: None,
            subpatterns,
            designation,
            span: anchor.elapsed(lexer),
        });
    }

    // a literal, or an expression starting with one -- unambiguously a constant
    if starts_constant_pattern(lexer.kind()) {
        let value = super::expression::parse_expression(lexer, errors, allocator)?;
        return Some(Pattern::Constant(value));
    }

    // `Type`, `Type x`, `Type { ... }`, `Type(...)`, `Type[...]`
    let pattern_type = parse_type(lexer, errors, allocator)?;

    if lexer.kind() == TokenKind::ParenthesisLeft {
        return parse_parenthesized_or_positional(
            lexer,
            errors,
            allocator,
            Some(pattern_type),
            &anchor,
        );
    }

    if lexer.kind() == TokenKind::BraceLeft {
        let subpatterns = parse_property_subpatterns(lexer, errors, allocator);
        let designation = eat_designation(lexer);

        return Some(Pattern::Property {
            pattern_type: Some(pattern_type),
            subpatterns,
            designation,
            span: anchor.elapsed(lexer),
        });
    }

    let designation = eat_designation(lexer);

    Some(Pattern::Declaration {
        pattern_type,
        designation,
        span: anchor.elapsed(lexer),
    })
}

/// `( ... )` after an optional type.
///
/// With no type in front, a single unnamed element is a parenthesised pattern -- `is (0)`
/// means the same as `is 0` -- while anything else is positional. A `{ ... }` or a name
/// after the group also forces the positional reading, since a parenthesised pattern
/// cannot carry either.
fn parse_parenthesized_or_positional<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    pattern_type: Option<crate::ast::TypeRef<'input, 'allocator>>,
    anchor: &crate::lexer::Anchor,
) -> Option<Pattern<'input, 'allocator>> {
    lexer.eat(TokenKind::ParenthesisLeft)?;

    let mut subpatterns = Vec::new_in(allocator);

    while lexer.kind() != TokenKind::ParenthesisRight && lexer.kind() != TokenKind::None {
        let subpattern_anchor = lexer.cast_anchor();

        let name = (lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::Colon)
            .then(|| {
                let name = lexer.eat_ident().unwrap();
                lexer.next(); // `:`
                name
            });

        let Some(pattern) = parse_pattern(lexer, errors, allocator) else {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, TokenKind::ParenthesisRight],
                ParseErrorKind::MissingPattern,
            ));
            break;
        };

        subpatterns.push(PositionalSubpattern {
            name,
            pattern,
            span: subpattern_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedParenthesizedPattern,
        ));
    }

    let property_subpatterns = if lexer.kind() == TokenKind::BraceLeft {
        parse_property_subpatterns(lexer, errors, allocator)
    } else {
        &[]
    };
    let designation = eat_designation(lexer);

    let is_parenthesized = pattern_type.is_none()
        && subpatterns.len() == 1
        && subpatterns[0].name.is_none()
        && property_subpatterns.is_empty()
        && designation.is_none();

    if is_parenthesized {
        let pattern = subpatterns.pop().unwrap().pattern;

        return Some(Pattern::Parenthesized {
            pattern: Ok(allocator.alloc(pattern)),
            span: anchor.elapsed(lexer),
        });
    }

    Some(Pattern::Positional {
        pattern_type,
        subpatterns: alloc_slice(allocator, subpatterns),
        property_subpatterns,
        designation,
        span: anchor.elapsed(lexer),
    })
}

/// `[ p1, p2, ..rest ]`
fn parse_list_pattern<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    anchor: &crate::lexer::Anchor,
) -> Pattern<'input, 'allocator> {
    lexer.eat(TokenKind::BracketLeft);

    let mut elements = Vec::new_in(allocator);

    while lexer.kind() != TokenKind::BracketRight && lexer.kind() != TokenKind::None {
        let element_anchor = lexer.cast_anchor();

        // the slice pattern `..`, optionally binding what it skipped
        let element = if let Some(dot_dot) = lexer.eat(TokenKind::DotDot) {
            let pattern =
                parse_pattern(lexer, errors, allocator).map(|pattern| &*allocator.alloc(pattern));

            Pattern::Slice {
                dot_dot,
                pattern,
                span: element_anchor.elapsed(lexer),
            }
        } else {
            match parse_pattern(lexer, errors, allocator) {
                Some(pattern) => pattern,
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::BracketRight],
                        ParseErrorKind::MissingPattern,
                    ));
                    break;
                }
            }
        };

        elements.push(element);

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BracketRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedListPattern,
        ));
    }

    let designation = eat_designation(lexer);

    Pattern::List {
        elements: alloc_slice(allocator, elements),
        designation,
        span: anchor.elapsed(lexer),
    }
}

/// `x`, `_`, or the deconstructing `(a, (b, c))`.
pub(crate) fn parse_variable_designation<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<VariableDesignation<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    if lexer.eat(TokenKind::ParenthesisLeft).is_some() {
        let mut elements = Vec::new_in(allocator);

        while lexer.kind() != TokenKind::ParenthesisRight && lexer.kind() != TokenKind::None {
            let Some(element) = parse_variable_designation(lexer, errors, allocator) else {
                break;
            };
            elements.push(element);

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }

        if lexer.eat(TokenKind::ParenthesisRight).is_none() {
            errors.push(recover_until_balanced(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::UnclosedParenthesizedPattern,
            ));
        }

        return Some(VariableDesignation::Parenthesized {
            elements: alloc_slice(allocator, elements),
            span: anchor.elapsed(lexer),
        });
    }

    if lexer.kind() == TokenKind::Identifier && lexer.current().map(|token| token.text) == Some("_")
    {
        return Some(VariableDesignation::Discard(lexer.take_span()));
    }

    eat_designation(lexer).map(VariableDesignation::Single)
}

/// The name a pattern binds to, as in `is int value`.
///
/// `and`, `or` and `when` are contextual keywords, so they would otherwise be swallowed
/// as a binding name and `o is int or string` would lose its `or`.
fn eat_designation<'input>(lexer: &mut Lexer<'input>) -> Option<crate::ast::Ident<'input>> {
    if matches!(
        lexer.kind(),
        TokenKind::And | TokenKind::Or | TokenKind::When
    ) {
        return None;
    }

    lexer.eat_ident()
}

fn starts_constant_pattern(kind: TokenKind) -> bool {
    if predefined_type(kind).is_some() {
        return false;
    }

    matches!(
        kind,
        TokenKind::IntegerLiteral
            | TokenKind::RealLiteral
            | TokenKind::CharLiteral
            | TokenKind::StringLiteral
            | TokenKind::VerbatimStringLiteral
            | TokenKind::InterpolatedStringLiteral
            | TokenKind::RawStringLiteral
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
            | TokenKind::Minus
            | TokenKind::Plus
    )
}

fn parse_property_subpatterns<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [PropertySubpattern<'input, 'allocator>] {
    lexer.eat(TokenKind::BraceLeft);

    let mut subpatterns = Vec::new_in(allocator);

    loop {
        if lexer.kind() == TokenKind::BraceRight || lexer.kind() == TokenKind::None {
            break;
        }

        let anchor = lexer.cast_anchor();

        let Some(name) = lexer.eat_ident() else {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, TokenKind::BraceRight],
                ParseErrorKind::MissingSubpatternName,
            ));
            if lexer.eat(TokenKind::Comma).is_some() {
                continue;
            }
            break;
        };

        let colon = lexer.eat(TokenKind::Colon);
        let pattern = match colon {
            Some(_) => match parse_pattern(lexer, errors, allocator) {
                Some(pattern) => Ok(pattern),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::BraceRight],
                        ParseErrorKind::MissingPattern,
                    ));
                    Err(())
                }
            },
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Comma, TokenKind::BraceRight],
                    ParseErrorKind::MissingPattern,
                ));
                Err(())
            }
        };

        subpatterns.push(PropertySubpattern {
            name,
            colon,
            pattern,
            span: anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedPropertyPattern,
        ));
    }

    alloc_slice(allocator, subpatterns)
}
