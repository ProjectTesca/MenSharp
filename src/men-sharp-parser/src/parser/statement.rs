use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        Block, BreakStatement, CatchClause, CatchFilter, CheckedKind, CheckedStatement,
        ContinueStatement, DoWhileStatement, Documents, ExpressionStatement, FinallyClause,
        FixedStatement, ForInitializer, ForStatement, ForeachStatement, GotoStatement, GotoTarget,
        IfStatement, LabeledStatement, LocalVariableDeclaration, LockStatement, MethodDeclaration,
        ReturnStatement, Spanned, Statement, SwitchLabel, SwitchSection, SwitchStatement,
        ThrowStatement, TryStatement, UnsafeStatement, UsingResource, UsingStatement,
        VariableDeclarator, WhileStatement, YieldKind, YieldStatement,
    },
    error::{ParseErrorKind, error_here, recover_until, recover_until_balanced},
    lexer::{Lexer, TokenKind},
    parser::{
        Errors, ParserLexer, alloc_slice,
        declaration::{
            has_unsafe, parse_attribute_sections, parse_function_body, parse_modifiers,
            parse_parameter_list,
        },
        expression::{parse_expression, parse_expression_or_recover, parse_initializer_value},
        pattern::parse_pattern,
        skip_documents,
        types::{parse_generics_define, parse_type, parse_type_parameter_constraints},
    },
};

const STATEMENT_RECOVERY: &[TokenKind] = &[
    TokenKind::Semicolon,
    TokenKind::BraceRight,
    TokenKind::BraceLeft,
];

pub(crate) fn parse_block<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Block<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    lexer.eat(TokenKind::BraceLeft)?;

    let statements = parse_statements(lexer, errors, allocator);

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedBlock,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    Some(Block {
        statements,
        span: anchor.elapsed(lexer),
    })
}

/// Statements up to the closing `}` of the enclosing block, or end of input.
fn parse_statements<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [Statement<'input, 'allocator>] {
    let mut statements = Vec::new_in(allocator);

    loop {
        skip_documents(lexer);

        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        match parse_statement(lexer, errors, allocator) {
            Some(statement) => statements.push(statement),
            None => {
                // nothing here parses; report it and make sure we still move forward
                let before = lexer.cast_anchor();
                errors.push(recover_until_balanced(
                    lexer,
                    STATEMENT_RECOVERY,
                    ParseErrorKind::InvalidStatement,
                ));

                if before.elapsed(lexer).is_empty()
                    && lexer.eat(TokenKind::Semicolon).is_none()
                    && lexer.next().is_none()
                {
                    break;
                }
            }
        }
    }

    alloc_slice(allocator, statements)
}

pub(crate) fn parse_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<Statement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    match lexer.kind() {
        TokenKind::BraceLeft => {
            return parse_block(lexer, errors, allocator).map(Statement::Block);
        }
        TokenKind::Semicolon => {
            return Some(Statement::Empty {
                semicolon: lexer.take_span(),
            });
        }
        TokenKind::If => return parse_if(lexer, errors, allocator).map(Statement::If),
        TokenKind::While => return parse_while(lexer, errors, allocator).map(Statement::While),
        TokenKind::Do => return parse_do_while(lexer, errors, allocator).map(Statement::DoWhile),
        TokenKind::For => return parse_for(lexer, errors, allocator).map(Statement::For),
        TokenKind::Foreach => {
            return parse_foreach(lexer, errors, allocator).map(Statement::Foreach);
        }
        TokenKind::Switch if lexer.lookahead(1) == TokenKind::ParenthesisLeft => {
            return parse_switch(lexer, errors, allocator).map(Statement::Switch);
        }
        TokenKind::Try => return parse_try(lexer, errors, allocator).map(Statement::Try),
        TokenKind::Lock => return parse_lock(lexer, errors, allocator).map(Statement::Lock),
        TokenKind::Checked | TokenKind::Unchecked if lexer.lookahead(1) == TokenKind::BraceLeft => {
            let checked = lexer.kind() == TokenKind::Checked;
            let kind = Spanned::new(
                if checked {
                    CheckedKind::Checked
                } else {
                    CheckedKind::Unchecked
                },
                lexer.take_span(),
            );
            let block = parse_block(lexer, errors, allocator).ok_or(());

            return Some(Statement::Checked(CheckedStatement {
                kind,
                block,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Unsafe if lexer.lookahead(1) == TokenKind::BraceLeft => {
            let unsafe_keyword = lexer.take_span();

            lexer.unsafe_depth += 1;
            let block = parse_block(lexer, errors, allocator).ok_or(());
            lexer.unsafe_depth -= 1;

            return Some(Statement::Unsafe(UnsafeStatement {
                unsafe_keyword,
                block,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Fixed => {
            let fixed_keyword = lexer.take_span();

            // `fixed` is only legal inside `unsafe`, but a file that gets that wrong should
            // still parse, so that the error is about `unsafe` rather than about syntax
            lexer.unsafe_depth += 1;

            if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
                errors.push(recover_until_balanced(
                    lexer,
                    STATEMENT_RECOVERY,
                    ParseErrorKind::MissingParenthesisLeft,
                ));
            }

            let declaration =
                match parse_local_variable_declaration(lexer, errors, allocator, None, false) {
                    Some(declaration) => Ok(declaration),
                    None => {
                        errors.push(recover_until_balanced(
                            lexer,
                            &[TokenKind::ParenthesisRight],
                            ParseErrorKind::MissingUsingResource,
                        ));
                        Err(())
                    }
                };

            if lexer.eat(TokenKind::ParenthesisRight).is_none() {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
                    ParseErrorKind::UnclosedParenthesis,
                ));
                lexer.eat(TokenKind::ParenthesisRight);
            }

            let body = parse_embedded_statement(lexer, errors, allocator);
            lexer.unsafe_depth -= 1;

            return Some(Statement::Fixed(FixedStatement {
                fixed_keyword,
                declaration,
                body,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Using => {
            // `using (resource) statement` vs. the declaration form `using var x = ...;`
            if lexer.lookahead(1) == TokenKind::ParenthesisLeft {
                return parse_using(lexer, errors, allocator).map(Statement::Using);
            }

            let using_keyword = lexer.take_span();
            let declaration = parse_local_variable_declaration(
                lexer,
                errors,
                allocator,
                Some(using_keyword),
                true,
            );

            return match declaration {
                Some(declaration) => Some(Statement::LocalVariable(declaration)),
                None => {
                    errors.push(recover_until(
                        lexer,
                        STATEMENT_RECOVERY,
                        ParseErrorKind::MissingUsingResource,
                    ));
                    lexer.eat(TokenKind::Semicolon);
                    None
                }
            };
        }
        TokenKind::Return => {
            let return_keyword = lexer.take_span();
            let value = (lexer.kind() != TokenKind::Semicolon)
                .then(|| parse_expression(lexer, errors, allocator))
                .flatten();
            let semicolon = eat_semicolon(lexer, errors);

            return Some(Statement::Return(ReturnStatement {
                return_keyword,
                value,
                semicolon,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Throw => {
            let throw_keyword = lexer.take_span();
            let value = (lexer.kind() != TokenKind::Semicolon)
                .then(|| parse_expression(lexer, errors, allocator))
                .flatten();
            let semicolon = eat_semicolon(lexer, errors);

            return Some(Statement::Throw(ThrowStatement {
                throw_keyword,
                value,
                semicolon,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Yield if matches!(lexer.lookahead(1), TokenKind::Return | TokenKind::Break) => {
            let yield_keyword = lexer.take_span();

            let is_return = lexer.kind() == TokenKind::Return;
            let kind = Spanned::new(
                if is_return {
                    YieldKind::Return
                } else {
                    YieldKind::Break
                },
                lexer.take_span(),
            );

            let value = is_return
                .then(|| parse_expression(lexer, errors, allocator))
                .flatten();
            let semicolon = eat_semicolon(lexer, errors);

            return Some(Statement::Yield(YieldStatement {
                yield_keyword,
                kind,
                value,
                semicolon,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Break => {
            let break_keyword = lexer.take_span();
            let semicolon = eat_semicolon(lexer, errors);

            return Some(Statement::Break(BreakStatement {
                break_keyword,
                semicolon,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Continue => {
            let continue_keyword = lexer.take_span();
            let semicolon = eat_semicolon(lexer, errors);

            return Some(Statement::Continue(ContinueStatement {
                continue_keyword,
                semicolon,
                span: anchor.elapsed(lexer),
            }));
        }
        TokenKind::Goto => return parse_goto(lexer, errors, allocator).map(Statement::Goto),
        _ => {}
    }

    // `label:`
    if lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::Colon {
        let label = lexer.eat_ident().unwrap();
        let colon = lexer.take_span();
        let statement = parse_embedded_statement(lexer, errors, allocator);

        return Some(Statement::Labeled(LabeledStatement {
            label,
            colon,
            statement,
            span: anchor.elapsed(lexer),
        }));
    }

    if let Some(local_function) = parse_local_function(lexer, errors, allocator) {
        return Some(Statement::LocalFunction(local_function));
    }

    if let Some(declaration) =
        parse_local_variable_declaration(lexer, errors, allocator, None, true)
    {
        return Some(Statement::LocalVariable(declaration));
    }

    let expression = parse_expression(lexer, errors, allocator)?;
    let semicolon = eat_semicolon(lexer, errors);

    Some(Statement::Expression(ExpressionStatement {
        expression,
        semicolon,
        span: anchor.elapsed(lexer),
    }))
}

/// A missing `;` is reported in place rather than skipped to, because the next statement
/// almost always starts right there and skipping would swallow it.
fn eat_semicolon(lexer: &mut Lexer<'_>, errors: &mut Errors) -> Option<std::ops::Range<usize>> {
    match lexer.eat(TokenKind::Semicolon) {
        Some(semicolon) => Some(semicolon),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingSemicolon));
            None
        }
    }
}

/// The single statement that follows `if (...)`, `while (...)` and friends.
fn parse_embedded_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<&'allocator Statement<'input, 'allocator>, ()> {
    match parse_statement(lexer, errors, allocator) {
        Some(statement) => Ok(allocator.alloc(statement)),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                STATEMENT_RECOVERY,
                ParseErrorKind::MissingStatement,
            ));
            Err(())
        }
    }
}

/// `( expression )`, shared by `if`, `while`, `lock` and `switch`.
fn parse_parenthesized_condition<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<crate::ast::Expression<'input, 'allocator>, ()> {
    if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingParenthesisLeft,
        ));
        return Err(());
    }

    let condition = parse_expression_or_recover(
        lexer,
        errors,
        allocator,
        &[TokenKind::ParenthesisRight, TokenKind::Semicolon],
    );

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
            ParseErrorKind::UnclosedParenthesis,
        ));
        lexer.eat(TokenKind::ParenthesisRight);
    }

    condition
}

fn parse_if<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<IfStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let if_keyword = lexer.eat(TokenKind::If)?;

    let condition = parse_parenthesized_condition(lexer, errors, allocator);
    let then_branch = parse_embedded_statement(lexer, errors, allocator);

    let else_keyword = lexer.eat(TokenKind::Else);
    let else_branch = else_keyword
        .is_some()
        .then(|| parse_embedded_statement(lexer, errors, allocator).ok())
        .flatten();

    Some(IfStatement {
        if_keyword,
        condition,
        then_branch,
        else_keyword,
        else_branch,
        span: anchor.elapsed(lexer),
    })
}

fn parse_while<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<WhileStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let while_keyword = lexer.eat(TokenKind::While)?;

    let condition = parse_parenthesized_condition(lexer, errors, allocator);
    let body = parse_embedded_statement(lexer, errors, allocator);

    Some(WhileStatement {
        while_keyword,
        condition,
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_do_while<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<DoWhileStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let do_keyword = lexer.eat(TokenKind::Do)?;

    let body = parse_embedded_statement(lexer, errors, allocator);

    let while_keyword = lexer.eat(TokenKind::While);
    if while_keyword.is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingWhileInDoWhile,
        ));
    }

    let condition = match while_keyword {
        Some(_) => parse_parenthesized_condition(lexer, errors, allocator),
        None => Err(()),
    };
    let semicolon = lexer.eat(TokenKind::Semicolon);

    Some(DoWhileStatement {
        do_keyword,
        body,
        while_keyword,
        condition,
        semicolon,
        span: anchor.elapsed(lexer),
    })
}

fn parse_for<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<ForStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let for_keyword = lexer.eat(TokenKind::For)?;

    if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingParenthesisLeft,
        ));
    }

    let initializer = if lexer.kind() == TokenKind::Semicolon {
        None
    } else if let Some(declaration) =
        parse_local_variable_declaration(lexer, errors, allocator, None, false)
    {
        Some(ForInitializer::Declaration(declaration))
    } else {
        let mut expressions = Vec::new_in(allocator);
        while let Some(expression) = parse_expression(lexer, errors, allocator) {
            expressions.push(expression);

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Some(ForInitializer::Expressions(alloc_slice(
            allocator,
            expressions,
        )))
    };

    if lexer.eat(TokenKind::Semicolon).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::Semicolon, TokenKind::ParenthesisRight],
            ParseErrorKind::MissingSemicolon,
        ));
        lexer.eat(TokenKind::Semicolon);
    }

    let condition = (lexer.kind() != TokenKind::Semicolon)
        .then(|| parse_expression(lexer, errors, allocator))
        .flatten();

    if lexer.eat(TokenKind::Semicolon).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::Semicolon, TokenKind::ParenthesisRight],
            ParseErrorKind::MissingSemicolon,
        ));
        lexer.eat(TokenKind::Semicolon);
    }

    let mut incrementors = Vec::new_in(allocator);
    if lexer.kind() != TokenKind::ParenthesisRight {
        while let Some(expression) = parse_expression(lexer, errors, allocator) {
            incrementors.push(expression);

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
    }

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
            ParseErrorKind::UnclosedParenthesis,
        ));
        lexer.eat(TokenKind::ParenthesisRight);
    }

    let body = parse_embedded_statement(lexer, errors, allocator);

    Some(ForStatement {
        for_keyword,
        initializer,
        condition,
        incrementors: alloc_slice(allocator, incrementors),
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_foreach<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<ForeachStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let foreach_keyword = lexer.eat(TokenKind::Foreach)?;

    if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingParenthesisLeft,
        ));
    }

    let variable_type = match parse_type(lexer, errors, allocator) {
        Some(variable_type) => Ok(variable_type),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::In, TokenKind::ParenthesisRight],
                ParseErrorKind::MissingForeachVariable,
            ));
            Err(())
        }
    };

    let name = match super::pattern::parse_variable_designation(lexer, errors, allocator) {
        Some(designation) => Ok(designation),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::In, TokenKind::ParenthesisRight],
                ParseErrorKind::MissingForeachVariable,
            ));
            Err(())
        }
    };

    let in_keyword = lexer.eat(TokenKind::In);
    if in_keyword.is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::ParenthesisRight],
            ParseErrorKind::MissingInInForeach,
        ));
    }

    let collection = parse_expression_or_recover(
        lexer,
        errors,
        allocator,
        &[TokenKind::ParenthesisRight, TokenKind::Semicolon],
    );

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
            ParseErrorKind::UnclosedParenthesis,
        ));
        lexer.eat(TokenKind::ParenthesisRight);
    }

    let body = parse_embedded_statement(lexer, errors, allocator);

    Some(ForeachStatement {
        foreach_keyword,
        variable_type,
        name,
        in_keyword,
        collection,
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_switch<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<SwitchStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let switch_keyword = lexer.eat(TokenKind::Switch)?;

    let value = parse_parenthesized_condition(lexer, errors, allocator);

    if lexer.eat(TokenKind::BraceLeft).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::UnclosedSwitchBody,
        ));

        return Some(SwitchStatement {
            switch_keyword,
            value,
            sections: Err(()),
            span: anchor.elapsed(lexer),
        });
    }

    let mut sections = Vec::new_in(allocator);

    loop {
        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        let section_anchor = lexer.cast_anchor();
        let mut labels = Vec::new_in(allocator);

        loop {
            let label_anchor = lexer.cast_anchor();

            match lexer.kind() {
                TokenKind::Case => {
                    let case_keyword = lexer.take_span();

                    let pattern = match parse_pattern(lexer, errors, allocator) {
                        Some(pattern) => Ok(pattern),
                        None => {
                            errors.push(recover_until_balanced(
                                lexer,
                                &[TokenKind::Colon, TokenKind::BraceRight],
                                ParseErrorKind::MissingCaseLabel,
                            ));
                            Err(())
                        }
                    };

                    let guard = lexer
                        .eat(TokenKind::When)
                        .map(|_| {
                            parse_expression_or_recover(
                                lexer,
                                errors,
                                allocator,
                                &[TokenKind::Colon, TokenKind::BraceRight],
                            )
                        })
                        .and_then(Result::ok);

                    if lexer.eat(TokenKind::Colon).is_none() {
                        errors.push(recover_until_balanced(
                            lexer,
                            &[TokenKind::Colon, TokenKind::BraceRight],
                            ParseErrorKind::MissingColonInSwitchLabel,
                        ));
                        lexer.eat(TokenKind::Colon);
                    }

                    labels.push(SwitchLabel::Case {
                        case_keyword,
                        pattern,
                        guard,
                        span: label_anchor.elapsed(lexer),
                    });
                }
                TokenKind::Default => {
                    let default_keyword = lexer.take_span();

                    if lexer.eat(TokenKind::Colon).is_none() {
                        errors.push(recover_until_balanced(
                            lexer,
                            &[TokenKind::Colon, TokenKind::BraceRight],
                            ParseErrorKind::MissingColonInSwitchLabel,
                        ));
                        lexer.eat(TokenKind::Colon);
                    }

                    labels.push(SwitchLabel::Default {
                        default_keyword,
                        span: label_anchor.elapsed(lexer),
                    });
                }
                _ => break,
            }
        }

        if labels.is_empty() {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Case, TokenKind::Default, TokenKind::BraceRight],
                ParseErrorKind::MissingCaseLabel,
            ));

            if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
                break;
            }
            continue;
        }

        let mut statements = Vec::new_in(allocator);
        loop {
            skip_documents(lexer);

            if matches!(
                lexer.kind(),
                TokenKind::Case | TokenKind::Default | TokenKind::BraceRight | TokenKind::None
            ) {
                break;
            }

            match parse_statement(lexer, errors, allocator) {
                Some(statement) => statements.push(statement),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Case, TokenKind::Default, TokenKind::BraceRight],
                        ParseErrorKind::InvalidStatement,
                    ));
                    break;
                }
            }
        }

        sections.push(SwitchSection {
            labels: alloc_slice(allocator, labels),
            statements: alloc_slice(allocator, statements),
            span: section_anchor.elapsed(lexer),
        });
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedSwitchBody,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    Some(SwitchStatement {
        switch_keyword,
        value,
        sections: Ok(alloc_slice(allocator, sections)),
        span: anchor.elapsed(lexer),
    })
}

fn parse_try<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<TryStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let try_keyword = lexer.eat(TokenKind::Try)?;

    let block = parse_block(lexer, errors, allocator).ok_or(());

    let mut catches = Vec::new_in(allocator);
    while lexer.kind() == TokenKind::Catch {
        let catch_anchor = lexer.cast_anchor();
        let catch_keyword = lexer.take_span();

        let mut exception_type = None;
        let mut name = None;

        if lexer.eat(TokenKind::ParenthesisLeft).is_some() {
            exception_type = parse_type(lexer, errors, allocator);
            name = lexer.eat_ident();

            if lexer.eat(TokenKind::ParenthesisRight).is_none() {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
                    ParseErrorKind::UnclosedParenthesis,
                ));
                lexer.eat(TokenKind::ParenthesisRight);
            }
        }

        let filter = lexer.eat(TokenKind::When).map(|when_keyword| {
            let filter_anchor = lexer.cast_anchor();
            let condition = parse_parenthesized_condition(lexer, errors, allocator);

            CatchFilter {
                when_keyword,
                condition,
                span: filter_anchor.elapsed(lexer),
            }
        });

        catches.push(CatchClause {
            catch_keyword,
            exception_type,
            name,
            filter,
            block: parse_block(lexer, errors, allocator).ok_or(()),
            span: catch_anchor.elapsed(lexer),
        });
    }

    let finally_clause = lexer.eat(TokenKind::Finally).map(|finally_keyword| {
        let finally_anchor = lexer.cast_anchor();

        FinallyClause {
            finally_keyword,
            block: parse_block(lexer, errors, allocator).ok_or(()),
            span: finally_anchor.elapsed(lexer),
        }
    });

    if catches.is_empty() && finally_clause.is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingCatchOrFinally,
        ));
    }

    Some(TryStatement {
        try_keyword,
        block,
        catches: alloc_slice(allocator, catches),
        finally_clause,
        span: anchor.elapsed(lexer),
    })
}

fn parse_using<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<UsingStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let using_keyword = lexer.eat(TokenKind::Using)?;

    if lexer.eat(TokenKind::ParenthesisLeft).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            STATEMENT_RECOVERY,
            ParseErrorKind::MissingParenthesisLeft,
        ));
    }

    let resource = match parse_local_variable_declaration(lexer, errors, allocator, None, false) {
        Some(declaration) => Ok(UsingResource::Declaration(declaration)),
        None => match parse_expression(lexer, errors, allocator) {
            Some(expression) => Ok(UsingResource::Expression(expression)),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::ParenthesisRight],
                    ParseErrorKind::MissingUsingResource,
                ));
                Err(())
            }
        },
    };

    if lexer.eat(TokenKind::ParenthesisRight).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[TokenKind::ParenthesisRight, TokenKind::BraceLeft],
            ParseErrorKind::UnclosedParenthesis,
        ));
        lexer.eat(TokenKind::ParenthesisRight);
    }

    let body = parse_embedded_statement(lexer, errors, allocator);

    Some(UsingStatement {
        using_keyword,
        resource,
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_lock<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<LockStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let lock_keyword = lexer.eat(TokenKind::Lock)?;

    let target = parse_parenthesized_condition(lexer, errors, allocator);
    let body = parse_embedded_statement(lexer, errors, allocator);

    Some(LockStatement {
        lock_keyword,
        target,
        body,
        span: anchor.elapsed(lexer),
    })
}

fn parse_goto<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<GotoStatement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let goto_keyword = lexer.eat(TokenKind::Goto)?;

    let target = match lexer.kind() {
        TokenKind::Case => {
            let case_keyword = lexer.take_span();
            let value = parse_expression_or_recover(lexer, errors, allocator, STATEMENT_RECOVERY);
            GotoTarget::Case {
                case_keyword,
                value,
            }
        }
        TokenKind::Default => GotoTarget::Default {
            default_keyword: lexer.take_span(),
        },
        _ => GotoTarget::Label(match lexer.eat_ident() {
            Some(label) => Ok(label),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    STATEMENT_RECOVERY,
                    ParseErrorKind::MissingLabelName,
                ));
                Err(())
            }
        }),
    };

    let semicolon = eat_semicolon(lexer, errors);

    Some(GotoStatement {
        goto_keyword,
        target,
        semicolon,
        span: anchor.elapsed(lexer),
    })
}

/// `Type name = value, other;`
///
/// Speculative: almost every declaration prefix is also a valid expression prefix, so the
/// shape is only accepted once a name has been seen followed by `=`, `,` or `;`.
/// That is what keeps `flag ? a : b;` an expression while `Nullable? x;` is a declaration.
pub(crate) fn parse_local_variable_declaration<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    using_keyword: Option<std::ops::Range<usize>>,
    expect_semicolon: bool,
) -> Option<LocalVariableDeclaration<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let attributes = parse_attribute_sections(lexer, errors, allocator);
    let const_keyword = lexer.eat(TokenKind::Const);

    let Some(variable_type) = parse_type(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let Some(first_name) = lexer.eat_ident() else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    if !matches!(
        lexer.kind(),
        TokenKind::Equal | TokenKind::Comma | TokenKind::Semicolon
    ) {
        lexer.back_to_anchor(anchor);
        return None;
    }

    let mut declarators = Vec::new_in(allocator);
    let mut name = first_name;

    loop {
        let declarator_anchor = name.span.clone();

        let initializer = lexer.eat(TokenKind::Equal).and_then(|_| {
            match parse_initializer_value(lexer, errors, allocator) {
                Some(initializer) => Some(initializer),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, TokenKind::Semicolon],
                        ParseErrorKind::MissingExpression,
                    ));
                    None
                }
            }
        });

        let end = initializer
            .as_ref()
            .map(|initializer| initializer.span().end)
            .unwrap_or(declarator_anchor.end);

        declarators.push(VariableDeclarator {
            name,
            initializer,
            span: declarator_anchor.start..end,
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }

        match lexer.eat_ident() {
            Some(next) => name = next,
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Semicolon],
                    ParseErrorKind::MissingMemberName,
                ));
                break;
            }
        }
    }

    let semicolon = expect_semicolon
        .then(|| eat_semicolon(lexer, errors))
        .flatten();

    Some(LocalVariableDeclaration {
        attributes,
        const_keyword,
        using_keyword,
        variable_type,
        declarators: alloc_slice(allocator, declarators),
        semicolon,
        span: anchor.elapsed(lexer),
    })
}

/// A function declared inside a method body.
fn parse_local_function<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<MethodDeclaration<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let attributes = parse_attribute_sections(lexer, errors, allocator);
    let modifiers = parse_modifiers(lexer, allocator);

    let Some(return_type) = parse_type(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let Some(name) = lexer.eat_ident() else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let generics = parse_generics_define(lexer, errors, allocator);

    if lexer.kind() != TokenKind::ParenthesisLeft {
        lexer.back_to_anchor(anchor);
        return None;
    }

    let is_unsafe = has_unsafe(modifiers);
    if is_unsafe {
        lexer.unsafe_depth += 1;
    }

    // `Type Name (...)` is also exactly the shape of a call: `await F(x);`, `new Foo(x);`.
    // What settles it is the body, so parse the parameters speculatively and roll back --
    // errors included -- unless one actually follows.
    let error_count = errors.len();
    let parameters = parse_parameter_list(lexer, errors, allocator);

    if !matches!(
        lexer.kind(),
        TokenKind::BraceLeft | TokenKind::FatArrow | TokenKind::Where
    ) {
        lexer.back_to_anchor(anchor);
        errors.truncate(error_count);
        return None;
    }

    let constraints = parse_type_parameter_constraints(lexer, errors, allocator);
    let body = parse_function_body(lexer, errors, allocator);

    if is_unsafe {
        lexer.unsafe_depth -= 1;
    }

    Some(MethodDeclaration {
        documents: empty_documents(&anchor, lexer),
        attributes,
        modifiers,
        return_type,
        explicit_interface: None,
        name,
        generics,
        parameters,
        constraints,
        body,
        span: anchor.elapsed(lexer),
    })
}

/// A local function has nowhere to hang doc comments, so it gets an empty, zero width one.
fn empty_documents<'input, 'allocator>(
    anchor: &crate::lexer::Anchor,
    lexer: &Lexer<'_>,
) -> Documents<'input, 'allocator> {
    let span = anchor.elapsed(lexer);
    Documents {
        documents: &[],
        span: span.start..span.start,
    }
}
