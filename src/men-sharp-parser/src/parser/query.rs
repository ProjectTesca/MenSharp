//! LINQ query expressions.
//!
//! The clause keywords (`from`, `where`, `select`, ...) are all contextual, so they are
//! ordinary identifiers everywhere else. Two things keep that from being a problem:
//!
//! - a query is only entered after a complete `from [type] name in` header parses, which
//!   an identifier named `from` can never produce;
//! - inside a query, each clause's expression stops of its own accord at the next keyword,
//!   because the lexer gives every contextual keyword its own [`TokenKind`] and none of
//!   them is an operator. `from x in xs where p select e` needs no lookahead at all.

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        FromClause, JoinClause, LetClause, OrderByClause, OrderDirection, Ordering, QueryBody,
        QueryClause, QueryContinuation, QueryExpression, SelectOrGroupClause, Spanned,
        WhereQueryClause,
    },
    error::{ParseErrorKind, error_here, recover_until},
    lexer::{Lexer, TokenKind},
    parser::{
        Errors, ParserLexer, alloc_slice,
        expression::{EXPRESSION_RECOVERY, parse_expression, parse_expression_or_recover},
        types::parse_type,
    },
};

/// Parses a query expression, restoring the cursor if `from` turns out to be a plain name.
pub(crate) fn parse_query_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<QueryExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let from = parse_from_clause(lexer, errors, allocator)?;
    let body = parse_query_body(lexer, errors, allocator);

    Some(QueryExpression {
        from,
        body: allocator.alloc(body),
        span: anchor.elapsed(lexer),
    })
}

/// `from [type] name in source`
///
/// The header up to `in` is parsed strictly: anything missing means this was never a
/// query, so the cursor goes back and `from` stays an identifier. Only the source
/// expression, once `in` is confirmed, is parsed with error recovery.
fn parse_from_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<FromClause<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let from_keyword = lexer.eat(TokenKind::From)?;

    let Some((element_type, name)) = parse_range_variable(lexer, errors, allocator) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let Some(in_keyword) = lexer.eat(TokenKind::In) else {
        lexer.back_to_anchor(anchor);
        return None;
    };

    let source = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

    Some(FromClause {
        from_keyword,
        element_type,
        name,
        in_keyword,
        source,
        span: anchor.elapsed(lexer),
    })
}

/// The `[type] name` of a `from` or `join` clause. `from x` and `from int x` both occur,
/// and a lone identifier parses as a type first, so the typed reading is tried and undone.
#[allow(clippy::type_complexity)]
fn parse_range_variable<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<(
    Option<crate::ast::TypeRef<'input, 'allocator>>,
    crate::ast::Ident<'input>,
)> {
    let anchor = lexer.cast_anchor();

    let element_type = parse_type(lexer, errors, allocator);
    if let Some(name) = lexer.eat_ident() {
        return Some((element_type, name));
    }

    lexer.back_to_anchor(anchor);
    lexer.eat_ident().map(|name| (None, name))
}

fn parse_query_body<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> QueryBody<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let mut clauses = Vec::new_in(allocator);

    loop {
        let clause = match lexer.kind() {
            TokenKind::From => match parse_from_clause(lexer, errors, allocator) {
                Some(clause) => QueryClause::From(clause),
                None => break,
            },
            TokenKind::Let => QueryClause::Let(parse_let_clause(lexer, errors, allocator)),
            TokenKind::Where => QueryClause::Where(parse_where_clause(lexer, errors, allocator)),
            TokenKind::Join => QueryClause::Join(parse_join_clause(lexer, errors, allocator)),
            TokenKind::Orderby => {
                QueryClause::OrderBy(parse_order_by_clause(lexer, errors, allocator))
            }
            _ => break,
        };

        clauses.push(clause);
    }

    let select_or_group = parse_select_or_group(lexer, errors, allocator);

    let continuation = lexer.eat(TokenKind::Into).map(|into_keyword| {
        let continuation_anchor = lexer.cast_anchor();

        let name = match lexer.eat_ident() {
            Some(name) => Ok(name),
            None => {
                errors.push(error_here(lexer, ParseErrorKind::MissingQueryContinuation));
                Err(())
            }
        };
        let body = parse_query_body(lexer, errors, allocator);

        QueryContinuation {
            into_keyword,
            name,
            body: allocator.alloc(body),
            span: continuation_anchor.elapsed(lexer),
        }
    });

    QueryBody {
        clauses: alloc_slice(allocator, clauses),
        select_or_group,
        continuation,
        span: anchor.elapsed(lexer),
    }
}

fn parse_let_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> LetClause<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let let_keyword = lexer.take_span();

    let name = match lexer.eat_ident() {
        Some(name) => Ok(name),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingQueryClause));
            Err(())
        }
    };

    let equal = lexer.eat(TokenKind::Equal);
    let value = match equal {
        Some(_) => parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingQueryClause));
            Err(())
        }
    };

    LetClause {
        let_keyword,
        name,
        equal,
        value,
        span: anchor.elapsed(lexer),
    }
}

fn parse_where_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> WhereQueryClause<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let where_keyword = lexer.take_span();
    let condition = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

    WhereQueryClause {
        where_keyword,
        condition,
        span: anchor.elapsed(lexer),
    }
}

fn parse_join_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> JoinClause<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let join_keyword = lexer.take_span();

    let (element_type, name) = match parse_range_variable(lexer, errors, allocator) {
        Some((element_type, name)) => (element_type, Ok(name)),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingQueryClause));
            (None, Err(()))
        }
    };

    let in_keyword = expect_keyword(lexer, errors, TokenKind::In);
    let source = keyword_guarded(lexer, errors, allocator, &in_keyword);

    let on_keyword = expect_keyword(lexer, errors, TokenKind::On);
    let left_key = keyword_guarded(lexer, errors, allocator, &on_keyword);

    let equals_keyword = expect_keyword(lexer, errors, TokenKind::Equals);
    let right_key = keyword_guarded(lexer, errors, allocator, &equals_keyword);

    let into = lexer.eat(TokenKind::Into).and_then(|_| lexer.eat_ident());

    JoinClause {
        join_keyword,
        element_type,
        name,
        in_keyword,
        source,
        on_keyword,
        left_key,
        equals_keyword,
        right_key,
        into,
        span: anchor.elapsed(lexer),
    }
}

fn expect_keyword(
    lexer: &mut Lexer<'_>,
    errors: &mut Errors,
    kind: TokenKind,
) -> Option<std::ops::Range<usize>> {
    match lexer.eat(kind) {
        Some(span) => Some(span),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingQueryClause));
            None
        }
    }
}

/// Parses an expression only when the keyword that introduces it was actually there.
fn keyword_guarded<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    keyword: &Option<std::ops::Range<usize>>,
) -> Result<crate::ast::Expression<'input, 'allocator>, ()> {
    match keyword {
        Some(_) => parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY),
        None => Err(()),
    }
}

fn parse_order_by_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> OrderByClause<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let orderby_keyword = lexer.take_span();

    let mut orderings = Vec::new_in(allocator);

    loop {
        let ordering_anchor = lexer.cast_anchor();

        let Some(key) = parse_expression(lexer, errors, allocator) else {
            errors.push(recover_until(
                lexer,
                EXPRESSION_RECOVERY,
                ParseErrorKind::MissingQueryClause,
            ));
            break;
        };

        let direction = match lexer.kind() {
            TokenKind::Ascending => {
                Some(Spanned::new(OrderDirection::Ascending, lexer.take_span()))
            }
            TokenKind::Descending => {
                Some(Spanned::new(OrderDirection::Descending, lexer.take_span()))
            }
            _ => None,
        };

        orderings.push(Ordering {
            key,
            direction,
            span: ordering_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    OrderByClause {
        orderby_keyword,
        orderings: alloc_slice(allocator, orderings),
        span: anchor.elapsed(lexer),
    }
}

fn parse_select_or_group<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<SelectOrGroupClause<'input, 'allocator>, ()> {
    let anchor = lexer.cast_anchor();

    if let Some(select_keyword) = lexer.eat(TokenKind::Select) {
        let value = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

        return Ok(SelectOrGroupClause::Select {
            select_keyword,
            value,
            span: anchor.elapsed(lexer),
        });
    }

    if let Some(group_keyword) = lexer.eat(TokenKind::Group) {
        let value = parse_expression_or_recover(lexer, errors, allocator, EXPRESSION_RECOVERY);

        let by_keyword = expect_keyword(lexer, errors, TokenKind::By);
        let key = keyword_guarded(lexer, errors, allocator, &by_keyword);

        return Ok(SelectOrGroupClause::Group {
            group_keyword,
            value,
            by_keyword,
            key,
            span: anchor.elapsed(lexer),
        });
    }

    errors.push(error_here(lexer, ParseErrorKind::MissingSelectOrGroup));
    Err(())
}
