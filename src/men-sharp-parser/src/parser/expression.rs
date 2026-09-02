use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        AnonymousMethodExpression, Argument, ArgumentList, ArgumentModifier, ArgumentValue,
        AsExpression, AssignmentExpression, AssignmentOperator, AwaitExpression, BinaryExpression,
        BinaryOperator, CastExpression, CheckedKind, CollectionElement,
        CollectionExpressionElement, ConditionalExpression, DeclarationExpression, Expression,
        Initializer, InitializerTarget, InitializerValue, IsExpression, LambdaBody,
        LambdaExpression, LambdaParameters, LiteralExpression, MemberSeparator, Modifier,
        NewExpression, ObjectInitializerElement, Parameter, ParameterList, ParameterModifier,
        PostfixOperator, PrimaryExpression, PrimaryLeft, PrimaryRight, RangeExpression,
        RefExpression, Spanned, StackallocExpression, SwitchExpression, SwitchExpressionArm,
        ThrowExpression, TupleElement, TypeRef, TypeRefBase, TypeSuffix, UnaryExpression,
        UnaryOperator, WithExpression,
    },
    error::{ParseErrorKind, recover_until, recover_until_balanced},
    lexer::{Lexer, TokenKind},
    parser::{
        BumpVec, Errors, GreaterRun, ParserLexer, alloc_slice, consume_tokens,
        declaration::{parse_attribute_sections, parse_modifiers, parse_parameter_list},
        interpolation::parse_interpolated_string,
        pattern::{parse_pattern, parse_variable_designation},
        peek_greater_run,
        query::parse_query_expression,
        statement::parse_block,
        types::{parse_generics_info, parse_type, parse_type_after_is, predefined_type},
    },
};

/// Token kinds a broken expression can safely be skipped up to.
pub(crate) const EXPRESSION_RECOVERY: &[TokenKind] = &[
    TokenKind::Semicolon,
    TokenKind::Comma,
    TokenKind::ParenthesisRight,
    TokenKind::BracketRight,
    TokenKind::BraceRight,
    TokenKind::BraceLeft,
];

type Level<'input, 'allocator> =
    fn(&mut Lexer<'input>, &mut Errors, &'allocator Bump) -> Option<Expression<'input, 'allocator>>;

pub(crate) fn parse_expression_or_recover<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    until: &[TokenKind],
) -> Result<Expression<'input, 'allocator>, ()> {
    match parse_expression(lexer, errors, allocator) {
        Some(expression) => Ok(expression),
        None => {
            errors.push(recover_until(
                lexer,
                until,
                ParseErrorKind::MissingExpression,
            ));
            Err(())
        }
    }
}

/// The lowest precedence level: lambdas, `throw`, and assignment (right associative).
pub(crate) fn parse_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    if lexer.kind() == TokenKind::From
        && let Some(query) = parse_query_expression(lexer, errors, allocator)
    {
        return Some(Expression::Query(allocator.alloc(query)));
    }

    if let Some(anonymous_method) = parse_anonymous_method(lexer, errors, allocator) {
        return Some(Expression::AnonymousMethod(
            allocator.alloc(anonymous_method),
        ));
    }

    if let Some(lambda) = parse_lambda(lexer, errors, allocator) {
        return Some(Expression::Lambda(allocator.alloc(lambda)));
    }

    if lexer.kind() == TokenKind::Throw {
        let anchor = lexer.cast_anchor();
        let throw_keyword = lexer.take_span();
        let value = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

        return Some(Expression::Throw(allocator.alloc(ThrowExpression {
            throw_keyword,
            value,
            span: anchor.elapsed(lexer),
        })));
    }

    let anchor = lexer.cast_anchor();
    let target = parse_conditional(lexer, errors, allocator)?;

    let Some(operator) = eat_assignment_operator(lexer) else {
        return Some(target);
    };

    // right associative: `a = b = c` is `a = (b = c)`
    let value = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

    Some(Expression::Assignment(allocator.alloc(
        AssignmentExpression {
            target,
            operator,
            value,
            span: anchor.elapsed(lexer),
        },
    )))
}

fn eat_assignment_operator(lexer: &mut Lexer<'_>) -> Option<Spanned<AssignmentOperator>> {
    let operator = match lexer.kind() {
        TokenKind::Equal => AssignmentOperator::Assign,
        TokenKind::PlusEqual => AssignmentOperator::Add,
        TokenKind::MinusEqual => AssignmentOperator::Subtract,
        TokenKind::AsteriskEqual => AssignmentOperator::Multiply,
        TokenKind::SlashEqual => AssignmentOperator::Divide,
        TokenKind::PercentEqual => AssignmentOperator::Modulo,
        TokenKind::AmpersandEqual => AssignmentOperator::BitwiseAnd,
        TokenKind::VerticalLineEqual => AssignmentOperator::BitwiseOr,
        TokenKind::CircumflexEqual => AssignmentOperator::BitwiseXor,
        TokenKind::LeftShiftEqual => AssignmentOperator::LeftShift,
        TokenKind::DoubleQuestionEqual => AssignmentOperator::Coalesce,
        // `>>=` and `>>>=` arrive as a run of `>` tokens ending in `>=`
        TokenKind::GreaterThan | TokenKind::GreaterThanEqual => {
            let (run, span, count) = peek_greater_run(lexer)?;
            let operator = match run {
                GreaterRun::RightShiftAssign => AssignmentOperator::RightShift,
                GreaterRun::UnsignedRightShiftAssign => AssignmentOperator::UnsignedRightShift,
                _ => return None,
            };
            consume_tokens(lexer, count);
            return Some(Spanned::new(operator, span));
        }
        _ => return None,
    };

    Some(Spanned::new(operator, lexer.take_span()))
}

fn parse_conditional<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let condition = parse_coalesce(lexer, errors, allocator)?;

    let Some(question_mark) = lexer.eat(TokenKind::QuestionMark) else {
        return Some(condition);
    };

    let then_value = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

    let colon = lexer.eat(TokenKind::Colon);
    if colon.is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::MissingColonInConditional,
        ));
    }

    let else_value = match colon {
        Some(_) => parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY),
        None => Err(()),
    };

    Some(Expression::Conditional(allocator.alloc(
        ConditionalExpression {
            condition,
            question_mark,
            then_value,
            colon,
            else_value,
            span: anchor.elapsed(lexer),
        },
    )))
}

/// `??` is right associative.
fn parse_coalesce<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let left = parse_logical_or(lexer, errors, allocator)?;

    let Some(span) = lexer.eat(TokenKind::DoubleQuestion) else {
        return Some(left);
    };
    let operator = Spanned::new(BinaryOperator::Coalesce, span);

    let right = match parse_coalesce(lexer, errors, allocator) {
        Some(right) => Ok(right),
        None => {
            // `x ?? throw new Exception()`
            match parse_expression(lexer, errors, allocator) {
                Some(right) => Ok(right),
                None => {
                    errors.push(recover_until(
                        lexer,
                        EXPRESSION_RECOVERY,
                        ParseErrorKind::MissingRightOperand,
                    ));
                    Err(())
                }
            }
        }
    };

    Some(Expression::Binary(allocator.alloc(BinaryExpression {
        left,
        operator,
        right,
        span: anchor.elapsed(lexer),
    })))
}

/// Shared driver for the left associative binary levels.
fn parse_binary_level<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    operators: &[(TokenKind, BinaryOperator)],
    next: Level<'input, 'allocator>,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut left = next(lexer, errors, allocator)?;

    loop {
        let kind = lexer.kind();
        let Some((_, operator)) = operators.iter().find(|(token, _)| *token == kind) else {
            break;
        };
        let operator = Spanned::new(*operator, lexer.take_span());

        let right = match next(lexer, errors, allocator) {
            Some(right) => Ok(right),
            None => {
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingRightOperand,
                ));
                Err(())
            }
        };
        let failed = right.is_err();

        left = Expression::Binary(allocator.alloc(BinaryExpression {
            left,
            operator,
            right,
            span: anchor.elapsed(lexer),
        }));

        if failed {
            break;
        }
    }

    Some(left)
}

fn parse_logical_or<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[(TokenKind::DoubleVerticalLine, BinaryOperator::LogicalOr)],
        parse_logical_and,
    )
}

fn parse_logical_and<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[(TokenKind::DoubleAmpersand, BinaryOperator::LogicalAnd)],
        parse_bitwise_or,
    )
}

fn parse_bitwise_or<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[(TokenKind::VerticalLine, BinaryOperator::BitwiseOr)],
        parse_bitwise_xor,
    )
}

fn parse_bitwise_xor<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[(TokenKind::Circumflex, BinaryOperator::BitwiseXor)],
        parse_bitwise_and,
    )
}

fn parse_bitwise_and<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[(TokenKind::Ampersand, BinaryOperator::BitwiseAnd)],
        parse_equality,
    )
}

fn parse_equality<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[
            (TokenKind::DoubleEqual, BinaryOperator::Equal),
            (TokenKind::ExclamationEqual, BinaryOperator::NotEqual),
        ],
        parse_relational,
    )
}

/// `<`, `>`, `<=`, `>=`, `is` and `as` share one level.
fn parse_relational<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut left = parse_shift(lexer, errors, allocator)?;

    loop {
        match lexer.kind() {
            TokenKind::Is => {
                let is_keyword = lexer.take_span();
                let pattern = match parse_pattern(lexer, errors, allocator) {
                    Some(pattern) => Ok(pattern),
                    None => {
                        errors.push(recover_until(
                            lexer,
                            EXPRESSION_RECOVERY,
                            ParseErrorKind::MissingPattern,
                        ));
                        Err(())
                    }
                };

                left = Expression::Is(allocator.alloc(IsExpression {
                    value: left,
                    is_keyword,
                    pattern,
                    span: anchor.elapsed(lexer),
                }));
            }
            TokenKind::As => {
                let as_keyword = lexer.take_span();
                let target_type = match parse_type_after_is(lexer, errors, allocator) {
                    Some(target_type) => Ok(target_type),
                    None => {
                        errors.push(recover_until(
                            lexer,
                            EXPRESSION_RECOVERY,
                            ParseErrorKind::MissingType,
                        ));
                        Err(())
                    }
                };

                left = Expression::As(allocator.alloc(AsExpression {
                    value: left,
                    as_keyword,
                    target_type,
                    span: anchor.elapsed(lexer),
                }));
            }
            kind => {
                let operator = match kind {
                    TokenKind::LessThan => BinaryOperator::LessThan,
                    TokenKind::LessThanEqual => BinaryOperator::LessThanEqual,
                    // a lone `>` or `>=`; a longer run belongs to the shift or
                    // assignment level, so leave it there
                    TokenKind::GreaterThan | TokenKind::GreaterThanEqual => {
                        match peek_greater_run(lexer) {
                            Some((GreaterRun::Greater, _, _)) => BinaryOperator::GreaterThan,
                            Some((GreaterRun::GreaterEqual, _, _)) => {
                                BinaryOperator::GreaterThanEqual
                            }
                            _ => break,
                        }
                    }
                    _ => break,
                };
                let operator = Spanned::new(operator, lexer.take_span());

                let right = match parse_shift(lexer, errors, allocator) {
                    Some(right) => Ok(right),
                    None => {
                        errors.push(recover_until(
                            lexer,
                            EXPRESSION_RECOVERY,
                            ParseErrorKind::MissingRightOperand,
                        ));
                        Err(())
                    }
                };
                let failed = right.is_err();

                left = Expression::Binary(allocator.alloc(BinaryExpression {
                    left,
                    operator,
                    right,
                    span: anchor.elapsed(lexer),
                }));

                if failed {
                    break;
                }
            }
        }
    }

    Some(left)
}

fn parse_shift<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut left = parse_additive(lexer, errors, allocator)?;

    loop {
        let operator = match lexer.kind() {
            TokenKind::LeftShift => Spanned::new(BinaryOperator::LeftShift, lexer.take_span()),
            TokenKind::GreaterThan => {
                let Some((run, span, count)) = peek_greater_run(lexer) else {
                    break;
                };
                let operator = match run {
                    GreaterRun::RightShift => BinaryOperator::RightShift,
                    GreaterRun::UnsignedRightShift => BinaryOperator::UnsignedRightShift,
                    _ => break,
                };
                consume_tokens(lexer, count);
                Spanned::new(operator, span)
            }
            _ => break,
        };

        let right = match parse_additive(lexer, errors, allocator) {
            Some(right) => Ok(right),
            None => {
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingRightOperand,
                ));
                Err(())
            }
        };
        let failed = right.is_err();

        left = Expression::Binary(allocator.alloc(BinaryExpression {
            left,
            operator,
            right,
            span: anchor.elapsed(lexer),
        }));

        if failed {
            break;
        }
    }

    Some(left)
}

fn parse_additive<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[
            (TokenKind::Plus, BinaryOperator::Add),
            (TokenKind::Minus, BinaryOperator::Subtract),
        ],
        parse_multiplicative,
    )
}

fn parse_multiplicative<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    parse_binary_level(
        lexer,
        errors,
        allocator,
        &[
            (TokenKind::Asterisk, BinaryOperator::Multiply),
            (TokenKind::Slash, BinaryOperator::Divide),
            (TokenKind::Percent, BinaryOperator::Modulo),
        ],
        parse_switch_or_with,
    )
}

/// `expr switch { ... }` and `expr with { ... }`, both of which bind tighter than
/// any binary operator but looser than unary.
fn parse_switch_or_with<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut value = parse_range(lexer, errors, allocator)?;

    loop {
        // both keywords need a `{` behind them; `with` is only contextually a keyword
        if lexer.lookahead(1) != TokenKind::BraceLeft {
            break;
        }

        match lexer.kind() {
            TokenKind::Switch => {
                let switch_keyword = lexer.take_span();
                let arms = parse_switch_expression_arms(lexer, errors, allocator);

                value = Expression::Switch(allocator.alloc(SwitchExpression {
                    value,
                    switch_keyword,
                    arms,
                    span: anchor.elapsed(lexer),
                }));
            }
            TokenKind::With => {
                let with_keyword = lexer.take_span();
                let initializer = match parse_initializer(lexer, errors, allocator) {
                    Some(initializer) => Ok(initializer),
                    None => {
                        errors.push(recover_until(
                            lexer,
                            EXPRESSION_RECOVERY,
                            ParseErrorKind::UnclosedInitializer,
                        ));
                        Err(())
                    }
                };

                value = Expression::With(allocator.alloc(WithExpression {
                    value,
                    with_keyword,
                    initializer,
                    span: anchor.elapsed(lexer),
                }));
            }
            _ => break,
        }
    }

    Some(value)
}

#[allow(clippy::type_complexity)]
fn parse_switch_expression_arms<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<&'allocator [SwitchExpressionArm<'input, 'allocator>], ()> {
    if lexer.eat(TokenKind::BraceLeft).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedSwitchExpression,
        ));
        return Err(());
    }

    let mut arms = Vec::new_in(allocator);

    loop {
        if lexer.kind() == TokenKind::BraceRight || lexer.kind() == TokenKind::None {
            break;
        }

        let anchor = lexer.cast_anchor();

        let Some(pattern) = parse_pattern(lexer, errors, allocator) else {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, TokenKind::BraceRight],
                ParseErrorKind::MissingSwitchExpressionArm,
            ));
            if lexer.eat(TokenKind::Comma).is_some() {
                continue;
            }
            break;
        };

        let guard = lexer
            .eat(TokenKind::When)
            .map(|_| parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY))
            .and_then(Result::ok);

        let fat_arrow = lexer.eat(TokenKind::FatArrow);
        if fat_arrow.is_none() {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, TokenKind::BraceRight],
                ParseErrorKind::MissingFatArrowInSwitchArm,
            ));
        }

        let value = match fat_arrow {
            Some(_) => parse_expression_or_recover(
                lexer,
                errors,
                allocator,
                &[TokenKind::Comma, TokenKind::BraceRight],
            ),
            None => Err(()),
        };

        arms.push(SwitchExpressionArm {
            pattern,
            guard,
            fat_arrow,
            value,
            span: anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedSwitchExpression,
        ));
    }

    Ok(alloc_slice(allocator, arms))
}

/// `a..b`, `a..`, `..b` and `..`.
fn parse_range<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    if let Some(dot_dot) = lexer.eat(TokenKind::DotDot) {
        let end = parse_unary(lexer, errors, allocator);

        return Some(Expression::Range(allocator.alloc(RangeExpression {
            start: None,
            dot_dot,
            end,
            span: anchor.elapsed(lexer),
        })));
    }

    let start = parse_unary(lexer, errors, allocator)?;

    let Some(dot_dot) = lexer.eat(TokenKind::DotDot) else {
        return Some(start);
    };
    let end = parse_unary(lexer, errors, allocator);

    Some(Expression::Range(allocator.alloc(RangeExpression {
        start: Some(start),
        dot_dot,
        end,
        span: anchor.elapsed(lexer),
    })))
}

fn parse_unary<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    // `var (a, b)` on the left of an assignment. Only this shape is checked here; the
    // typed form `(int a, string b)` is recognised element by element inside the tuple.
    if lexer.kind() == TokenKind::Var
        && lexer.lookahead(1) == TokenKind::ParenthesisLeft
        && let Some(declaration) = parse_declaration_expression(lexer, errors, allocator)
    {
        return Some(Expression::Declaration(allocator.alloc(declaration)));
    }

    // `ref x`, as in `ref int y = ref x;`
    if let Some(ref_keyword) = lexer.eat(TokenKind::Ref) {
        let value = match parse_unary(lexer, errors, allocator) {
            Some(value) => Ok(value),
            None => {
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::MissingExpression,
                ));
                Err(())
            }
        };

        return Some(Expression::Ref(allocator.alloc(RefExpression {
            ref_keyword,
            value,
            span: anchor.elapsed(lexer),
        })));
    }

    let operator = match lexer.kind() {
        TokenKind::Plus => UnaryOperator::Plus,
        TokenKind::Minus => UnaryOperator::Minus,
        TokenKind::Exclamation => UnaryOperator::Not,
        TokenKind::Tilde => UnaryOperator::BitwiseNot,
        TokenKind::DoublePlus => UnaryOperator::PreIncrement,
        TokenKind::DoubleMinus => UnaryOperator::PreDecrement,
        TokenKind::Circumflex => UnaryOperator::IndexFromEnd,
        // at operand position these cannot be the binary `&` and `*`
        TokenKind::Ampersand => UnaryOperator::AddressOf,
        TokenKind::Asterisk => UnaryOperator::Dereference,
        TokenKind::Await if can_start_expression(lexer.lookahead(1)) => {
            let await_keyword = lexer.take_span();
            let value = match parse_unary(lexer, errors, allocator) {
                Some(value) => Ok(value),
                None => {
                    errors.push(recover_until(
                        lexer,
                        EXPRESSION_RECOVERY,
                        ParseErrorKind::MissingExpression,
                    ));
                    Err(())
                }
            };

            return Some(Expression::Await(allocator.alloc(AwaitExpression {
                await_keyword,
                value,
                span: anchor.elapsed(lexer),
            })));
        }
        TokenKind::ParenthesisLeft => {
            if let Some(cast) = parse_cast(lexer, errors, allocator) {
                return Some(Expression::Cast(allocator.alloc(cast)));
            }
            return parse_primary(lexer, errors, allocator);
        }
        _ => return parse_primary(lexer, errors, allocator),
    };
    let operator = Spanned::new(operator, lexer.take_span());

    let operand = match parse_unary(lexer, errors, allocator) {
        Some(operand) => Ok(operand),
        None => {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingExpression,
            ));
            Err(())
        }
    };

    Some(Expression::Unary(allocator.alloc(UnaryExpression {
        operator,
        operand,
        span: anchor.elapsed(lexer),
    })))
}

/// `(T)x` -- told apart from a parenthesised expression the way C# does it: the tokens
/// inside must parse as a type, and what follows must be able to start an operand.
///
/// `(a) - b` is therefore a subtraction, while `(int) - b` is a cast, because a
/// predefined type on the left removes the ambiguity.
fn parse_cast<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<CastExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    lexer.eat(TokenKind::ParenthesisLeft)?;

    let Some(target_type) = parse_type(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        lexer.back_to_anchor(anchor);
        return None;
    }

    let following = lexer.kind();
    let unambiguous = matches!(target_type.base, TypeRefBase::Predefined(_))
        || !target_type.suffixes.is_empty()
        || has_generic_argument(&target_type);

    let is_cast = can_start_expression(following)
        || (unambiguous
            && matches!(
                following,
                TokenKind::Plus | TokenKind::Minus | TokenKind::Asterisk | TokenKind::Ampersand
            ));

    if !is_cast {
        lexer.back_to_anchor(anchor);
        return None;
    }

    let value = match parse_unary(lexer, errors, allocator) {
        Some(value) => Ok(value),
        None => {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingExpression,
            ));
            Err(())
        }
    };

    Some(CastExpression {
        target_type,
        value,
        span: anchor.elapsed(lexer),
    })
}

fn has_generic_argument(type_ref: &TypeRef<'_, '_>) -> bool {
    match &type_ref.base {
        TypeRefBase::Name(name) => name
            .segments
            .iter()
            .any(|segment| segment.generics.is_some()),
        _ => false,
    }
}

/// `var (a, b)` or `int x` where an expression is expected.
///
/// Restored if the tokens do not end in a designation followed by something a declaration
/// can be followed by, which is what keeps the call `f(x)` from reading as `f` declaring
/// `(x)`.
fn parse_declaration_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<DeclarationExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let Some(variable_type) = parse_type(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    // This is a trial parse: a designation that only got through by error
    // recovery is not a designation. `(F(1, 1) ? a : b)` otherwise reads as
    // the type `F` declaring the broken `(1` — recovery stops right at the
    // `)`, which is exactly what a declaration may be followed by
    let errors_before = errors.len();
    let Some(designation) = parse_variable_designation(lexer, errors, allocator) else {
        errors.truncate(errors_before);
        lexer.back_to_anchor(anchor);
        return None;
    };
    if errors.len() > errors_before {
        errors.truncate(errors_before);
        lexer.back_to_anchor(anchor);
        return None;
    }

    if !matches!(
        lexer.kind(),
        TokenKind::Equal | TokenKind::Comma | TokenKind::ParenthesisRight
    ) {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(DeclarationExpression {
        variable_type,
        designation,
        span: anchor.elapsed(lexer),
    })
}

/// Whether a token can begin an operand. Used both to disambiguate casts and to decide
/// whether a contextual `await` is an operator or a plain name.
pub(crate) fn can_start_expression(kind: TokenKind) -> bool {
    if kind.can_be_identifier() || predefined_type(kind).is_some() {
        return true;
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
            | TokenKind::This
            | TokenKind::Base
            | TokenKind::New
            | TokenKind::Typeof
            | TokenKind::Sizeof
            | TokenKind::Default
            | TokenKind::Checked
            | TokenKind::Unchecked
            | TokenKind::Throw
            | TokenKind::Delegate
            | TokenKind::Stackalloc
            | TokenKind::Ampersand
            | TokenKind::Asterisk
            | TokenKind::ParenthesisLeft
            | TokenKind::Tilde
            | TokenKind::Exclamation
    )
}

// ---------------------------------------------------------------------------
// primary expressions
// ---------------------------------------------------------------------------

fn parse_primary<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Expression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let left = parse_primary_left(lexer, errors, allocator)?;
    let chain = parse_primary_chain(lexer, errors, allocator);

    Some(Expression::Primary(allocator.alloc(PrimaryExpression {
        left,
        chain,
        span: anchor.elapsed(lexer),
    })))
}

fn parse_primary_left<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<PrimaryLeft<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    if let Some(literal) = parse_literal(lexer, errors, allocator) {
        return Some(PrimaryLeft::Literal(literal));
    }

    match lexer.kind() {
        TokenKind::This => return Some(PrimaryLeft::This(lexer.take_span())),
        TokenKind::Base => return Some(PrimaryLeft::Base(lexer.take_span())),
        TokenKind::New => return parse_new(lexer, errors, allocator),
        TokenKind::Typeof => {
            let typeof_keyword = lexer.take_span();
            let target_type = parse_parenthesized_type(lexer, errors, allocator);

            return Some(PrimaryLeft::Typeof {
                typeof_keyword,
                target_type,
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::Sizeof => {
            let sizeof_keyword = lexer.take_span();
            let target_type = parse_parenthesized_type(lexer, errors, allocator);

            return Some(PrimaryLeft::Sizeof {
                sizeof_keyword,
                target_type,
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::Nameof if lexer.lookahead(1) == TokenKind::ParenthesisLeft => {
            let nameof_keyword = lexer.take_span();
            lexer.next(); // `(`
            let value = parse_expression_or_recover(
                lexer,
                errors,
                allocator,
                &[TokenKind::ParenthesisRight, TokenKind::Semicolon],
            );
            if lexer.eat(TokenKind::ParenthesisRight).is_none() {
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::UnclosedParenthesis,
                ));
            }

            return Some(PrimaryLeft::Nameof {
                nameof_keyword,
                value,
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::Default => {
            let default_keyword = lexer.take_span();
            let target_type = (lexer.kind() == TokenKind::ParenthesisLeft)
                .then(|| parse_parenthesized_type(lexer, errors, allocator).ok())
                .flatten();

            return Some(PrimaryLeft::Default {
                default_keyword,
                target_type,
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::Checked | TokenKind::Unchecked => {
            let checked = lexer.kind() == TokenKind::Checked;
            let kind = Spanned::new(
                if checked {
                    CheckedKind::Checked
                } else {
                    CheckedKind::Unchecked
                },
                lexer.take_span(),
            );

            if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
                lexer.back_to_anchor(anchor);
                return None;
            }
            let value = parse_expression_or_recover(
                lexer,
                errors,
                allocator,
                &[TokenKind::ParenthesisRight, TokenKind::Semicolon],
            );
            if lexer.eat(TokenKind::ParenthesisRight).is_none() {
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::UnclosedParenthesis,
                ));
            }

            return Some(PrimaryLeft::Checked {
                kind,
                value,
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::Stackalloc => {
            let stackalloc_keyword = lexer.take_span();

            // `stackalloc[] { ... }` leaves the element type to inference
            let element_type = (lexer.kind() != TokenKind::BracketLeft)
                .then(|| parse_type(lexer, errors, allocator))
                .flatten();

            let mut size = None;
            if lexer.eat(TokenKind::BracketLeft).is_some() {
                size = parse_expression(lexer, errors, allocator);

                if lexer.eat(TokenKind::BracketRight).is_none() {
                    errors.push(recover_until(
                        lexer,
                        EXPRESSION_RECOVERY,
                        ParseErrorKind::UnclosedElementAccess,
                    ));
                }
            }

            let initializer = parse_initializer(lexer, errors, allocator);

            return Some(PrimaryLeft::Stackalloc(StackallocExpression {
                stackalloc_keyword,
                element_type,
                size,
                initializer,
                span: anchor.elapsed(lexer),
            }));
        }
        // `[1, 2, ..rest]` -- an attribute list never reaches here, because declarations
        // take their attributes before an expression is attempted
        TokenKind::BracketLeft => {
            lexer.next();
            let mut elements = Vec::new_in(allocator);

            while lexer.kind() != TokenKind::BracketRight && lexer.kind() != TokenKind::None {
                let element_anchor = lexer.cast_anchor();

                let element = if let Some(dot_dot) = lexer.eat(TokenKind::DotDot) {
                    let value = parse_expression_or_recover(
                        lexer,
                        errors,
                        allocator,
                        &[TokenKind::Comma, TokenKind::BracketRight],
                    );

                    CollectionExpressionElement::Spread {
                        dot_dot,
                        value,
                        span: element_anchor.elapsed(lexer),
                    }
                } else {
                    match parse_expression(lexer, errors, allocator) {
                        Some(value) => CollectionExpressionElement::Expression(value),
                        None => {
                            errors.push(recover_until_balanced(
                                lexer,
                                &[TokenKind::Comma, TokenKind::BracketRight],
                                ParseErrorKind::MissingExpression,
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
                errors.push(recover_until(
                    lexer,
                    EXPRESSION_RECOVERY,
                    ParseErrorKind::UnclosedElementAccess,
                ));
            }

            return Some(PrimaryLeft::Collection {
                elements: alloc_slice(allocator, elements),
                span: anchor.elapsed(lexer),
            });
        }
        TokenKind::ParenthesisLeft => {
            return parse_parenthesized_or_tuple(lexer, errors, allocator);
        }
        TokenKind::Global if lexer.lookahead(1) == TokenKind::DoubleColon => {
            return Some(PrimaryLeft::Global(lexer.take_span()));
        }
        _ => {}
    }

    if let Some(predefined) = predefined_type(lexer.kind()) {
        return Some(PrimaryLeft::Predefined(Spanned::new(
            predefined,
            lexer.take_span(),
        )));
    }

    let name = lexer.eat_ident()?;
    let generics = parse_generics_info_in_expression(lexer, errors, allocator);

    Some(PrimaryLeft::Identifier {
        name,
        generics,
        span: anchor.elapsed(lexer),
    })
}

/// A generic argument list is only recognised in an expression when the token after `>`
/// could not continue an ordinary comparison. Without this, `a < b, c > d` would be read
/// as the generic name `a<b, c>` followed by `d`.
fn parse_generics_info_in_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<crate::ast::GenericsInfo<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let generics = parse_generics_info(lexer, errors, allocator)?;

    let follows = matches!(
        lexer.kind(),
        TokenKind::ParenthesisLeft
            | TokenKind::ParenthesisRight
            | TokenKind::BracketRight
            | TokenKind::BraceRight
            | TokenKind::Colon
            | TokenKind::Semicolon
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::QuestionMark
            | TokenKind::DoubleEqual
            | TokenKind::ExclamationEqual
            | TokenKind::None
    );

    if !follows {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(generics)
}

fn parse_literal<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<LiteralExpression<'input, 'allocator>> {
    Some(match lexer.kind() {
        TokenKind::IntegerLiteral => LiteralExpression::Integer(lexer.take_text()),
        TokenKind::RealLiteral => LiteralExpression::Real(lexer.take_text()),
        TokenKind::CharLiteral => LiteralExpression::Char(lexer.take_text()),
        TokenKind::StringLiteral => LiteralExpression::String(lexer.take_text()),
        TokenKind::VerbatimStringLiteral => LiteralExpression::VerbatimString(lexer.take_text()),
        TokenKind::RawStringLiteral => LiteralExpression::RawString(lexer.take_text()),
        TokenKind::InterpolatedStringLiteral => {
            let unsafe_depth = lexer.unsafe_depth;
            LiteralExpression::InterpolatedString(parse_interpolated_string(
                lexer.take_text(),
                unsafe_depth,
                errors,
                allocator,
            ))
        }
        TokenKind::True => LiteralExpression::True(lexer.take_span()),
        TokenKind::False => LiteralExpression::False(lexer.take_span()),
        TokenKind::Null => LiteralExpression::Null(lexer.take_span()),
        _ => return None,
    })
}

fn parse_parenthesized_type<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<TypeRef<'input, 'allocator>, ()> {
    if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::MissingParenthesisLeft,
        ));
        return Err(());
    }

    let target_type = match parse_type(lexer, errors, allocator) {
        Some(target_type) => Ok(target_type),
        None => {
            errors.push(recover_until(
                lexer,
                &[TokenKind::ParenthesisRight, TokenKind::Semicolon],
                ParseErrorKind::MissingType,
            ));
            Err(())
        }
    };

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedParenthesis,
        ));
    }

    target_type
}

/// `(expr)` or the tuple `(a, b)` / `(x: a, y: b)`.
fn parse_parenthesized_or_tuple<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<PrimaryLeft<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    lexer.eat(TokenKind::ParenthesisLeft)?;

    let mut elements = Vec::new_in(allocator);

    loop {
        let element_anchor = lexer.cast_anchor();

        let name = (lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::Colon)
            .then(|| {
                let name = lexer.eat_ident().unwrap();
                lexer.next(); // `:`
                name
            });

        // `(int a, string b) = t` -- each element declares its own variable
        let value = match parse_declaration_expression(lexer, errors, allocator) {
            Some(declaration) => Expression::Declaration(allocator.alloc(declaration)),
            None => match parse_expression(lexer, errors, allocator) {
                Some(value) => value,
                None => break,
            },
        };

        elements.push(TupleElement {
            name,
            value,
            span: element_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedParenthesis,
        ));
    }

    let span = anchor.elapsed(lexer);

    if elements.len() == 1 && elements[0].name.is_none() {
        let expression = elements.pop().unwrap().value;
        return Some(PrimaryLeft::Parenthesized { expression, span });
    }

    Some(PrimaryLeft::Tuple {
        elements: alloc_slice(allocator, elements),
        span,
    })
}

fn parse_primary_chain<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [PrimaryRight<'input, 'allocator>] {
    let mut chain = Vec::new_in(allocator);

    loop {
        let anchor = lexer.cast_anchor();

        let separator = match lexer.kind() {
            TokenKind::Dot => Some(MemberSeparator::Dot),
            TokenKind::QuestionDot => Some(MemberSeparator::NullConditionalDot),
            TokenKind::DoubleColon => Some(MemberSeparator::DoubleColon),
            TokenKind::Arrow => Some(MemberSeparator::Arrow),
            _ => None,
        };

        if let Some(separator) = separator {
            let separator = Spanned::new(separator, lexer.take_span());

            let name = match lexer.eat_ident() {
                Some(name) => Ok(name),
                None => {
                    errors.push(recover_until(
                        lexer,
                        EXPRESSION_RECOVERY,
                        ParseErrorKind::MissingMemberNameAfterSeparator,
                    ));
                    Err(())
                }
            };
            let generics = parse_generics_info_in_expression(lexer, errors, allocator);
            let failed = name.is_err();

            chain.push(PrimaryRight::Member {
                separator,
                name,
                generics,
                span: anchor.elapsed(lexer),
            });

            if failed {
                break;
            }
            continue;
        }

        match lexer.kind() {
            TokenKind::ParenthesisLeft => {
                let arguments = parse_argument_list(lexer, errors, allocator, false);
                chain.push(PrimaryRight::Invocation {
                    arguments,
                    span: anchor.elapsed(lexer),
                });
            }
            TokenKind::BracketLeft => {
                let arguments = parse_argument_list(lexer, errors, allocator, true);
                chain.push(PrimaryRight::ElementAccess {
                    null_conditional: false,
                    arguments,
                    span: anchor.elapsed(lexer),
                });
            }
            // `?[` -- written as two tokens, so require them to be touching
            TokenKind::QuestionMark if lexer.lookahead(1) == TokenKind::BracketLeft => {
                let question = lexer.current().unwrap();
                lexer.next();
                if !lexer.is_adjacent_to_previous(&question) {
                    lexer.back_to_anchor(anchor);
                    break;
                }

                let arguments = parse_argument_list(lexer, errors, allocator, true);
                chain.push(PrimaryRight::ElementAccess {
                    null_conditional: true,
                    arguments,
                    span: anchor.elapsed(lexer),
                });
            }
            TokenKind::DoublePlus | TokenKind::DoubleMinus | TokenKind::Exclamation => {
                let operator = match lexer.kind() {
                    TokenKind::DoublePlus => PostfixOperator::Increment,
                    TokenKind::DoubleMinus => PostfixOperator::Decrement,
                    _ => PostfixOperator::NullForgiving,
                };
                let operator = Spanned::new(operator, lexer.take_span());

                chain.push(PrimaryRight::Postfix {
                    operator,
                    span: anchor.elapsed(lexer),
                });
            }
            _ => break,
        }
    }

    alloc_slice(allocator, chain)
}

/// `(a, b)` for a call, or `[a, b]` for an element access.
pub(crate) fn parse_argument_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    bracket: bool,
) -> ArgumentList<'input, 'allocator> {
    let (open, close) = if bracket {
        (TokenKind::BracketLeft, TokenKind::BracketRight)
    } else {
        (TokenKind::ParenthesisLeft, TokenKind::ParenthesisRight)
    };

    let anchor = lexer.cast_anchor();
    lexer.eat(open);

    let mut arguments = Vec::new_in(allocator);

    loop {
        if lexer.kind() == close || lexer.kind() == TokenKind::None {
            break;
        }

        let argument_anchor = lexer.cast_anchor();

        let name = (lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::Colon)
            .then(|| {
                let name = lexer.eat_ident().unwrap();
                lexer.next(); // `:`
                name
            });

        let modifier = match lexer.kind() {
            TokenKind::Ref => Some(Spanned::new(ArgumentModifier::Ref, lexer.take_span())),
            TokenKind::Out => Some(Spanned::new(ArgumentModifier::Out, lexer.take_span())),
            TokenKind::In => Some(Spanned::new(ArgumentModifier::In, lexer.take_span())),
            _ => None,
        };

        let value = parse_argument_value(lexer, errors, allocator, modifier.is_some(), close);

        arguments.push(Argument {
            name,
            modifier,
            value,
            span: argument_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(close).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            if bracket {
                ParseErrorKind::UnclosedElementAccess
            } else {
                ParseErrorKind::UnclosedArgumentList
            },
        ));
        lexer.eat(close);
    }

    ArgumentList {
        arguments: alloc_slice(allocator, arguments),
        span: anchor.elapsed(lexer),
    }
}

fn parse_argument_value<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    has_modifier: bool,
    close: TokenKind,
) -> ArgumentValue<'input, 'allocator> {
    // `out int x` / `out var x` introduces a variable at the call site
    if has_modifier {
        let anchor = lexer.cast_anchor();

        if let Some(variable_type) = parse_type(lexer, errors, allocator)
            && let Some(name) = lexer.eat_ident()
            && (lexer.kind() == TokenKind::Comma || lexer.kind() == close)
        {
            return ArgumentValue::Declaration {
                variable_type,
                name,
                span: anchor.elapsed(lexer),
            };
        }

        lexer.back_to_anchor(anchor);
    }

    match parse_expression(lexer, errors, allocator) {
        Some(expression) => ArgumentValue::Expression(expression),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, close],
                ParseErrorKind::MissingExpression,
            ));
            ArgumentValue::Missing
        }
    }
}

// ---------------------------------------------------------------------------
// object creation
// ---------------------------------------------------------------------------

fn parse_new<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<PrimaryLeft<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let new_keyword = lexer.eat(TokenKind::New)?;

    // `new { A = 1 }`
    if lexer.kind() == TokenKind::BraceLeft {
        let initializer = parse_object_initializer_elements(lexer, errors, allocator);

        return Some(PrimaryLeft::AnonymousObject {
            new_keyword,
            initializers: initializer,
            span: anchor.elapsed(lexer),
        });
    }

    // `new[] { 1, 2 }`
    if lexer.kind() == TokenKind::BracketLeft {
        let array_suffixes = parse_array_suffixes(lexer, allocator);
        let initializer = parse_initializer(lexer, errors, allocator);

        return Some(PrimaryLeft::New(NewExpression {
            new_keyword,
            created_type: None,
            array_sizes: &[],
            array_suffixes,
            arguments: None,
            initializer,
            span: anchor.elapsed(lexer),
        }));
    }

    // `new(...)` -- target typed
    if lexer.kind() == TokenKind::ParenthesisLeft {
        let arguments = parse_argument_list(lexer, errors, allocator, false);
        let initializer = parse_initializer(lexer, errors, allocator);

        return Some(PrimaryLeft::New(NewExpression {
            new_keyword,
            created_type: None,
            array_sizes: &[],
            array_suffixes: &[],
            arguments: Some(arguments),
            initializer,
            span: anchor.elapsed(lexer),
        }));
    }

    let created_type = match parse_type(lexer, errors, allocator) {
        Some(created_type) => created_type,
        None => {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingTypeInNewExpression,
            ));

            return Some(PrimaryLeft::New(NewExpression {
                new_keyword,
                created_type: None,
                array_sizes: &[],
                array_suffixes: &[],
                arguments: None,
                initializer: None,
                span: anchor.elapsed(lexer),
            }));
        }
    };

    // `new int[n]` -- a `[` that survived the type parser holds sizes, not a rank
    let mut array_sizes = Vec::new_in(allocator);
    let mut array_suffixes: &[TypeSuffix] = &[];

    if lexer.kind() == TokenKind::BracketLeft {
        lexer.next();

        loop {
            match parse_expression(lexer, errors, allocator) {
                Some(size) => array_sizes.push(size),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::BracketRight],
                        ParseErrorKind::MissingArraySize,
                    ));
                }
            }

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }

        if lexer.eat(TokenKind::BracketRight).is_none() {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::UnclosedElementAccess,
            ));
        }

        array_suffixes = parse_array_suffixes(lexer, allocator);
    }

    let arguments = (lexer.kind() == TokenKind::ParenthesisLeft)
        .then(|| parse_argument_list(lexer, errors, allocator, false));

    let initializer = parse_initializer(lexer, errors, allocator);

    Some(PrimaryLeft::New(NewExpression {
        new_keyword,
        created_type: Some(created_type),
        array_sizes: alloc_slice(allocator, array_sizes),
        array_suffixes,
        arguments,
        initializer,
        span: anchor.elapsed(lexer),
    }))
}

/// Trailing `[]` / `[,]` with no sizes, as in `new int[n][]`.
fn parse_array_suffixes<'allocator>(
    lexer: &mut Lexer<'_>,
    allocator: &'allocator Bump,
) -> &'allocator [TypeSuffix] {
    let mut suffixes = Vec::new_in(allocator);

    while lexer.kind() == TokenKind::BracketLeft {
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

    alloc_slice(allocator, suffixes)
}

/// `{ A = 1 }` or `{ 1, 2, 3 }`, told apart by whether the first element is `name =`
/// or `[index] =`.
pub(crate) fn parse_initializer<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Initializer<'input, 'allocator>> {
    if lexer.kind() != TokenKind::BraceLeft {
        return None;
    }

    let is_object = {
        let first = lexer.lookahead(1);
        let second = lexer.lookahead(2);

        (first.can_be_identifier() && second == TokenKind::Equal)
            || first == TokenKind::BracketLeft
            || first == TokenKind::BraceRight
    };

    let anchor = lexer.cast_anchor();

    if is_object {
        let elements = parse_object_initializer_elements(lexer, errors, allocator);
        return Some(Initializer::Object {
            elements,
            span: anchor.elapsed(lexer),
        });
    }

    lexer.next(); // `{`
    let mut elements = Vec::new_in(allocator);

    loop {
        if lexer.kind() == TokenKind::BraceRight || lexer.kind() == TokenKind::None {
            break;
        }

        match parse_initializer_value(lexer, errors, allocator) {
            Some(InitializerValue::Expression(expression)) => {
                elements.push(CollectionElement::Expression(expression))
            }
            Some(InitializerValue::Nested(nested)) => {
                elements.push(CollectionElement::Nested(nested))
            }
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Comma, TokenKind::BraceRight],
                    ParseErrorKind::MissingInitializerValue,
                ));
            }
        }

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedInitializer,
        ));
    }

    Some(Initializer::Collection {
        elements: alloc_slice(allocator, elements),
        span: anchor.elapsed(lexer),
    })
}

fn parse_object_initializer_elements<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [ObjectInitializerElement<'input, 'allocator>] {
    lexer.eat(TokenKind::BraceLeft);

    let mut elements = Vec::new_in(allocator);

    loop {
        if lexer.kind() == TokenKind::BraceRight || lexer.kind() == TokenKind::None {
            break;
        }

        let anchor = lexer.cast_anchor();

        let target = if lexer.kind() == TokenKind::BracketLeft {
            let index_anchor = lexer.cast_anchor();
            lexer.next();

            let mut arguments = Vec::new_in(allocator);
            while let Some(argument) = parse_expression(lexer, errors, allocator) {
                arguments.push(argument);

                if lexer.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }

            if lexer.eat(TokenKind::BracketRight).is_none() {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::Equal, TokenKind::Comma, TokenKind::BraceRight],
                    ParseErrorKind::UnclosedElementAccess,
                ));
            }

            InitializerTarget::Index {
                arguments: alloc_slice(allocator, arguments),
                span: index_anchor.elapsed(lexer),
            }
        } else {
            match lexer.eat_ident() {
                Some(name) => InitializerTarget::Member(name),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::BraceRight],
                        ParseErrorKind::MissingInitializerValue,
                    ));
                    if lexer.eat(TokenKind::Comma).is_some() {
                        continue;
                    }
                    break;
                }
            }
        };

        let equal = lexer.eat(TokenKind::Equal);
        let value = match equal {
            Some(_) => match parse_initializer_value(lexer, errors, allocator) {
                Some(value) => Ok(value),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::BraceRight],
                        ParseErrorKind::MissingInitializerValue,
                    ));
                    Err(())
                }
            },
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Comma, TokenKind::BraceRight],
                    ParseErrorKind::MissingInitializerValue,
                ));
                Err(())
            }
        };

        elements.push(ObjectInitializerElement {
            target,
            equal,
            value,
            span: anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            EXPRESSION_RECOVERY,
            ParseErrorKind::UnclosedInitializer,
        ));
    }

    alloc_slice(allocator, elements)
}

pub(crate) fn parse_initializer_value<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<InitializerValue<'input, 'allocator>> {
    if lexer.kind() == TokenKind::BraceLeft {
        return parse_initializer(lexer, errors, allocator).map(InitializerValue::Nested);
    }

    parse_expression(lexer, errors, allocator).map(InitializerValue::Expression)
}

// ---------------------------------------------------------------------------
// lambdas
// ---------------------------------------------------------------------------

/// `delegate(int x) { ... }`, `delegate { ... }` and `async delegate { ... }`.
///
/// No lookahead games needed: `delegate` is a reserved word, so in expression position it
/// can only start an anonymous method. Declarations (`public delegate int Op(int a);`) are
/// recognised earlier, at member and namespace level.
fn parse_anonymous_method<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<AnonymousMethodExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let modifiers = parse_modifiers(lexer, allocator);

    let Some(delegate_keyword) = lexer.eat(TokenKind::Delegate) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    // `delegate { ... }` omits the parameter list rather than writing an empty one
    let parameters = (lexer.kind() == TokenKind::ParenthesisLeft)
        .then(|| parse_parameter_list(lexer, errors, allocator).ok())
        .flatten();

    let body = match parse_block(lexer, errors, allocator) {
        Some(body) => Ok(body),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingLambdaBody,
            ));
            Err(())
        }
    };

    Some(AnonymousMethodExpression {
        modifiers,
        delegate_keyword,
        parameters,
        body,
        span: anchor.elapsed(lexer),
    })
}

/// `x => e`, `(x, y) => e`, `(int x) => { ... }`, with optional `async` / `static`
/// modifiers and an optional explicit return type.
///
/// Entirely speculative: anything that does not reach a `=>` is put back.
fn parse_lambda<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<LambdaExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let modifiers = parse_modifiers(lexer, allocator);
    if modifiers
        .iter()
        .any(|modifier| !matches!(modifier.value, Modifier::Async | Modifier::Static))
    {
        lexer.back_to_anchor(anchor);
        return None;
    }

    // `x => ...`
    if lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::FatArrow {
        let name = lexer.eat_ident().unwrap();
        let fat_arrow = lexer.take_span();
        let body = parse_lambda_body(lexer, errors, allocator);

        return Some(LambdaExpression {
            modifiers,
            return_type: None,
            parameters: LambdaParameters::Single(name),
            fat_arrow,
            body,
            span: anchor.elapsed(lexer),
        });
    }

    // an explicit return type, as in `int () => 0`
    let return_type = (lexer.kind() != TokenKind::ParenthesisLeft)
        .then(|| parse_type(lexer, errors, allocator))
        .flatten();

    if lexer.kind() != TokenKind::ParenthesisLeft {
        lexer.back_to_anchor(anchor);
        return None;
    }

    let Some(parameters) = parse_lambda_parameters(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let Some(fat_arrow) = lexer.eat(TokenKind::FatArrow) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let body = parse_lambda_body(lexer, errors, allocator);

    Some(LambdaExpression {
        modifiers,
        return_type,
        parameters: LambdaParameters::List(parameters),
        fat_arrow,
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_lambda_body<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<LambdaBody<'input, 'allocator>, ()> {
    if lexer.kind() == TokenKind::BraceLeft {
        return match parse_block(lexer, errors, allocator) {
            Some(block) => Ok(LambdaBody::Block(block)),
            None => Err(()),
        };
    }

    match parse_expression(lexer, errors, allocator) {
        Some(expression) => Ok(LambdaBody::Expression(expression)),
        None => {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingLambdaBody,
            ));
            Err(())
        }
    }
}

/// The `(...)` of a lambda, where each parameter may be a bare name or a typed one.
/// Returns `None` without consuming anything if the tokens are not a parameter list.
fn parse_lambda_parameters<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<ParameterList<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    lexer.eat(TokenKind::ParenthesisLeft)?;

    let mut parameters = Vec::new_in(allocator);

    loop {
        if lexer.kind() == TokenKind::ParenthesisRight {
            break;
        }

        let parameter_anchor = lexer.cast_anchor();

        let attributes = parse_attribute_sections(lexer, errors, allocator);
        let modifiers = parse_parameter_modifiers(lexer, allocator);

        // `Type name` if that parses, otherwise a bare `name`
        let typed_anchor = lexer.cast_anchor();
        let mut parameter_type = parse_type(lexer, errors, allocator);
        let mut name = lexer.eat_ident();

        if parameter_type.is_some() && name.is_none() {
            lexer.back_to_anchor(typed_anchor);
            parameter_type = None;
            name = lexer.eat_ident();
        }

        let Some(name) = name else {
            lexer.back_to_anchor(anchor);
            return None;
        };

        let default_value = lexer
            .eat(TokenKind::Equal)
            .and_then(|_| parse_expression(lexer, errors, allocator));

        parameters.push(Parameter {
            attributes,
            modifiers,
            parameter_type,
            name: Ok(name),
            default_value,
            span: parameter_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        lexer.back_to_anchor(anchor);
        return None;
    }

    Some(ParameterList {
        parameters: alloc_slice(allocator, parameters),
        span: anchor.elapsed(lexer),
    })
}

pub(crate) fn parse_parameter_modifiers<'allocator>(
    lexer: &mut Lexer<'_>,
    allocator: &'allocator Bump,
) -> &'allocator [Spanned<ParameterModifier>] {
    let mut modifiers: BumpVec<'allocator, Spanned<ParameterModifier>> = Vec::new_in(allocator);

    loop {
        let modifier = match lexer.kind() {
            TokenKind::Ref => ParameterModifier::Ref,
            TokenKind::Out => ParameterModifier::Out,
            TokenKind::In => ParameterModifier::In,
            TokenKind::Params => ParameterModifier::Params,
            TokenKind::This => ParameterModifier::This,
            TokenKind::Scoped => ParameterModifier::Scoped,
            TokenKind::Readonly => ParameterModifier::Readonly,
            _ => break,
        };

        modifiers.push(Spanned::new(modifier, lexer.take_span()));
    }

    alloc_slice(allocator, modifiers)
}
