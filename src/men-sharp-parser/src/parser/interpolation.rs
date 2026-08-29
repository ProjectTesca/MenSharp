//! The inside of an interpolated string.
//!
//! The lexer already delimits the whole literal, nesting included, so `$"{f("}")}"` arrives
//! as one token. This module takes that token apart: the text around the holes, and for each
//! hole an expression, an optional alignment and an optional format string.
//!
//! # Why the holes are parsed at all
//!
//! A hole is an expression, and expressions reach outside the literal. They read variables
//! (so definite assignment and closure capture depend on them), they call methods (so
//! evaluation order and side effects depend on them), and they can assign or `await`. Above
//! all, the compiler has to lower the literal into concatenation, and it cannot emit code
//! for text it never parsed.
//!
//! # How the three parts are separated
//!
//! By the first `,` and the first `:` that are not nested inside brackets, which is the
//! rule the C# compiler uses. It is also why C# needs parentheses around a conditional in
//! a hole: in `{a ? b : c}` the `:` ends the expression. Splitting the same way keeps this
//! parser from accepting code that the C# language service would reject.
//!
//! `::` is stepped over, so a namespace alias qualifier does not look like a format
//! separator.

use std::ops::Range;

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{Expression, InterpolatedString, InterpolationHole, InterpolationPart, LiteralText},
    error::{ParseError, ParseErrorKind},
    lexer::{Lexer, scan},
    parser::{Errors, alloc_slice, expression::parse_expression},
};

/// Splits an interpolated string token into its parts.
pub(crate) fn parse_interpolated_string<'input, 'allocator>(
    token: LiteralText<'input>,
    unsafe_depth: usize,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator InterpolatedString<'input, 'allocator> {
    let text = token.value;
    let base = token.span.start;
    let bytes = text.as_bytes();

    // `$`, `@` and the opening quote run, in whatever order they were written
    let mut index = 0;
    let mut dollars = 0usize;
    let mut is_verbatim = false;
    while index < bytes.len() && (bytes[index] == b'$' || bytes[index] == b'@') {
        if bytes[index] == b'$' {
            dollars += 1;
        } else {
            is_verbatim = true;
        }
        index += 1;
    }

    let quote_start = index;
    while bytes.get(index) == Some(&b'"') {
        index += 1;
    }
    let quote_count = index - quote_start;
    let is_raw = quote_count >= 3;

    // the closing delimiter is the same width as the opening one, when it is there at all
    let content_start = index;
    let content_end = text.len().saturating_sub(if text.ends_with('"') {
        if is_raw { quote_count } else { 1 }
    } else {
        0
    });
    let content_end = content_end.max(content_start);

    // in a raw string a hole opens on as many braces as there are dollars
    let brace_width = if is_raw { dollars.max(1) } else { 1 };

    let content = &text[content_start..content_end];
    let content_base = base + content_start;

    let mut parts = Vec::new_in(allocator);
    let mut text_start = 0usize;
    let mut cursor = 0usize;
    let content_bytes = content.as_bytes();

    while cursor < content_bytes.len() {
        match content_bytes[cursor] {
            b'\\' if !is_verbatim && !is_raw => {
                // an escape can hide a brace, so step over both bytes
                cursor += 2;
            }
            b'{' => {
                let run = brace_run(content_bytes, cursor);

                // doubled braces are a literal brace, not a hole; in a raw string a run
                // narrower than the dollar count is literal too
                if !is_raw && run >= 2 {
                    cursor += 2;
                    continue;
                }
                if run < brace_width {
                    cursor += run;
                    continue;
                }

                if text_start < cursor {
                    parts.push(text_part(content, content_base, text_start, cursor));
                }

                let hole_open = cursor + brace_width;
                let (hole_end, after) = scan_hole(content, hole_open, brace_width);

                parts.push(InterpolationPart::Hole(parse_hole(
                    &content[hole_open..hole_end],
                    content_base + hole_open,
                    content_base + cursor..content_base + after,
                    unsafe_depth,
                    errors,
                    allocator,
                )));

                cursor = after;
                text_start = cursor;
            }
            b'}' if !is_raw => {
                // `}}` is a literal brace; a lone `}` is malformed but harmless to keep
                cursor += if brace_run(content_bytes, cursor) >= 2 {
                    2
                } else {
                    1
                };
            }
            _ => cursor += 1,
        }
    }

    if text_start < content_bytes.len() {
        parts.push(text_part(
            content,
            content_base,
            text_start,
            content_bytes.len(),
        ));
    }

    allocator.alloc(InterpolatedString {
        parts: alloc_slice(allocator, parts),
        is_verbatim,
        is_raw,
        text: token.clone(),
        span: token.span,
    })
}

fn text_part<'input, 'allocator>(
    content: &'input str,
    base: usize,
    start: usize,
    end: usize,
) -> InterpolationPart<'input, 'allocator> {
    let end = end.min(content.len());
    let start = start.min(end);

    InterpolationPart::Text(LiteralText::new(
        &content[start..end],
        base + start..base + end,
    ))
}

/// How many braces of the same kind run from `index`.
fn brace_run(bytes: &[u8], index: usize) -> usize {
    let brace = bytes[index];
    let mut end = index;
    while bytes.get(end) == Some(&brace) {
        end += 1;
    }
    end - index
}

/// Walks from just inside a hole to its closing braces.
///
/// Returns where the hole's content ends and where the whole hole ends. Nested literals
/// and comments are stepped over with the lexer's own scanners, so a brace inside a
/// string cannot close the hole.
fn scan_hole(content: &str, start: usize, brace_width: usize) -> (usize, usize) {
    let bytes = content.as_bytes();

    let mut index = start;
    let mut depth = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' | b'@' | b'$' | b'/' => {
                let nested = scan::literal_or_comment_length(&content[index..]);
                index += if nested == 0 { 1 } else { nested };
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                index += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    index += 1;
                    continue;
                }

                let run_start = index;
                let run = brace_run(bytes, index);
                if run >= brace_width {
                    return (run_start, run_start + brace_width);
                }
                index += run;
            }
            _ => index += 1,
        }
    }

    // unterminated: the lexer already reported the literal, so just take what is there
    (bytes.len(), bytes.len())
}

/// The first `,` and `:` that are not nested inside brackets.
fn find_separators(hole: &str) -> (Option<usize>, Option<usize>) {
    let bytes = hole.as_bytes();

    let mut index = 0;
    let mut depth = 0usize;
    let mut comma = None;
    let mut colon = None;

    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' | b'@' | b'$' | b'/' => {
                let nested = scan::literal_or_comment_length(&hole[index..]);
                index += if nested == 0 { 1 } else { nested };
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                index += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b',' if depth == 0 && comma.is_none() && colon.is_none() => {
                comma = Some(index);
                index += 1;
            }
            b':' if depth == 0 => {
                // `::` qualifies a name; it never separates a format specifier
                if bytes.get(index + 1) == Some(&b':') {
                    index += 2;
                    continue;
                }
                if colon.is_none() {
                    colon = Some(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }

    (comma, colon)
}

fn parse_hole<'input, 'allocator>(
    hole: &'input str,
    base: usize,
    span: Range<usize>,
    unsafe_depth: usize,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> InterpolationHole<'input, 'allocator> {
    let (comma_index, colon_index) = find_separators(hole);

    let expression_end = comma_index.or(colon_index).unwrap_or(hole.len());
    let expression = parse_region(
        &hole[..expression_end],
        base,
        unsafe_depth,
        errors,
        allocator,
    );

    let alignment = comma_index.and_then(|comma| {
        let start = comma + 1;
        let end = colon_index.unwrap_or(hole.len()).max(start);

        parse_region(
            &hole[start..end],
            base + start,
            unsafe_depth,
            errors,
            allocator,
        )
        .ok()
    });

    let format = colon_index.map(|colon| {
        let start = colon + 1;
        LiteralText::new(&hole[start..], base + start..base + hole.len())
    });

    InterpolationHole {
        expression,
        comma: comma_index.map(|comma| base + comma..base + comma + 1),
        alignment,
        colon: colon_index.map(|colon| base + colon..base + colon + 1),
        format,
        span,
    }
}

/// Parses one region of a hole with a sub lexer, so its spans point back into the file.
///
/// The whole region has to be consumed: anything left over is a syntax error inside the
/// hole, which the enclosing expression parser would otherwise never see.
fn parse_region<'input, 'allocator>(
    region: &'input str,
    base: usize,
    unsafe_depth: usize,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<Expression<'input, 'allocator>, ()> {
    let mut lexer = Lexer::new_at(region, base);
    lexer.unsafe_depth = unsafe_depth;

    let Some(expression) = parse_expression(&mut lexer, errors, allocator) else {
        errors.push(ParseError {
            kind: ParseErrorKind::MissingInterpolationExpression,
            span: base..base + region.len(),
        });
        return Err(());
    };

    if let Some(token) = lexer.next() {
        errors.push(ParseError {
            kind: ParseErrorKind::InvalidInterpolationHole,
            span: token.span.start..base + region.len(),
        });
    }

    Ok(expression)
}
