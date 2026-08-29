//! Recursive descent parser for MenSharp.
//!
//! # Shape
//!
//! Every rule is a free function with the same signature:
//!
//! ```ignore
//! fn parse_x<'input, 'allocator>(
//!     lexer: &mut Lexer<'input>,
//!     errors: &mut Errors,
//!     allocator: &'allocator Bump,
//! ) -> Option<X<'input, 'allocator>>
//! ```
//!
//! `None` means *this rule does not apply here*; the lexer is left where it started so the
//! caller can try something else. A rule that has committed never returns `None`: it fills
//! the hole with `Err(())`, records a [`ParseError`], and keeps going. That is what lets the
//! parser return a usable tree for broken input instead of stopping at the first mistake.
//!
//! Backtracking is free: [`Lexer::cast_anchor`] / [`Lexer::back_to_anchor`] just move a byte
//! offset, which is what makes the speculative parses C# needs -- casts vs. parenthesised
//! expressions, declarations vs. expressions, lambdas vs. tuples -- cheap enough to do inline.

use std::ops::Range;

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{CompilationUnit, Documents, Ident, LiteralText},
    error::{ParseError, ParseErrorKind, recover_until},
    lexer::{GetKind, Lexer, Token, TokenKind},
    parser::declaration::{parse_namespace_members, parse_using_directives},
};

pub(crate) mod declaration;
pub(crate) mod expression;
pub(crate) mod interpolation;
pub(crate) mod pattern;
pub(crate) mod query;
pub(crate) mod statement;
pub(crate) mod types;

/// Errors are collected in a plain `std` vec: they outlive the arena, so the caller can keep
/// them after the tree is dropped.
pub type Errors = std::vec::Vec<ParseError>;

pub(crate) type BumpVec<'allocator, T> = Vec<T, &'allocator Bump>;

pub fn parse_compilation_unit<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator CompilationUnit<'input, 'allocator> {
    let anchor = lexer.cast_anchor();

    let extern_aliases = declaration::parse_extern_aliases(lexer, errors, allocator);
    let usings = parse_using_directives(lexer, errors, allocator);
    let attributes = declaration::parse_attribute_sections(lexer, errors, allocator);
    let members = parse_namespace_members(lexer, errors, allocator, &[]);

    // `parse_namespace_members` stops at anything that cannot begin a member -- a stray
    // `}` for instance. Nothing else will look at those tokens, so report them here.
    if lexer.current().is_some() {
        errors.push(recover_until(
            lexer,
            &[],
            ParseErrorKind::InvalidNamespaceMember,
        ));
    }

    allocator.alloc(CompilationUnit {
        extern_aliases,
        usings,
        attributes,
        members,
        span: anchor.elapsed(lexer),
    })
}

/// Moves a bump `Vec` into the arena and hands back a slice of it.
///
/// The `Vec`'s buffer already lives in the arena; this only parks the three-word header
/// there too, so the resulting `&'allocator [T]` outlives the local. An empty vec costs
/// nothing at all.
pub(crate) fn alloc_slice<'allocator, T>(
    allocator: &'allocator Bump,
    vec: BumpVec<'allocator, T>,
) -> &'allocator [T] {
    if vec.is_empty() {
        &[]
    } else {
        allocator.alloc(vec).as_slice()
    }
}

/// Small conveniences on top of the lexer, shared by every rule.
pub(crate) trait ParserLexer<'input> {
    /// The kind of the token at the cursor, or [`TokenKind::None`] at end of input.
    fn kind(&mut self) -> TokenKind;

    /// The kind `offset` tokens after the cursor, without moving it.
    fn lookahead(&mut self, offset: usize) -> TokenKind;

    /// Consumes the token at the cursor and returns its span.
    fn take_span(&mut self) -> Range<usize>;

    /// Consumes the token at the cursor if it has `kind`, and returns its span.
    fn eat(&mut self, kind: TokenKind) -> Option<Range<usize>>;

    /// Consumes an identifier, including contextual keywords used as names.
    /// A verbatim identifier (`@class`) yields the name without its `@`.
    fn eat_ident(&mut self) -> Option<Ident<'input>>;

    /// The text and span of the token at the cursor, consumed.
    fn take_text(&mut self) -> LiteralText<'input>;

    /// Whether the token at the cursor starts exactly where the previous one ended.
    fn is_adjacent_to_previous(&mut self, previous: &Token<'_>) -> bool;
}

impl<'input> ParserLexer<'input> for Lexer<'input> {
    fn kind(&mut self) -> TokenKind {
        self.current().get_kind()
    }

    fn lookahead(&mut self, offset: usize) -> TokenKind {
        let anchor = self.cast_anchor();

        let mut kind = TokenKind::None;
        for _ in 0..=offset {
            kind = self.current().get_kind();
            if kind == TokenKind::None {
                break;
            }
            self.next();
        }

        self.back_to_anchor(anchor);
        kind
    }

    fn take_span(&mut self) -> Range<usize> {
        self.next().map(|token| token.span).unwrap_or_default()
    }

    fn eat(&mut self, kind: TokenKind) -> Option<Range<usize>> {
        (self.kind() == kind).then(|| self.take_span())
    }

    fn eat_ident(&mut self) -> Option<Ident<'input>> {
        if !self.kind().can_be_identifier() {
            return None;
        }

        let token = self.next().unwrap();
        let text = token.text.strip_prefix('@').unwrap_or(token.text);

        Some(Ident::new(text, token.span))
    }

    fn take_text(&mut self) -> LiteralText<'input> {
        match self.next() {
            Some(token) => LiteralText::new(token.text, token.span),
            None => LiteralText::new("", 0..0),
        }
    }

    fn is_adjacent_to_previous(&mut self, previous: &Token<'_>) -> bool {
        self.current()
            .map(|token| previous.is_adjacent_to(&token))
            .unwrap_or(false)
    }
}

/// Collects the `///` and `/** */` comments sitting in front of a declaration.
///
/// Plain comments never reach the parser -- the lexer files them under `Lexer::comments` --
/// so anything seen here is documentation.
pub(crate) fn parse_documents<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    allocator: &'allocator Bump,
) -> Documents<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let mut documents = Vec::new_in(allocator);

    while lexer.kind().is_doc_comment() {
        documents.push(lexer.take_text());
    }

    Documents {
        documents: alloc_slice(allocator, documents),
        span: anchor.elapsed(lexer),
    }
}

/// Discards doc comments that ended up somewhere the grammar cannot attach them.
pub(crate) fn skip_documents(lexer: &mut Lexer<'_>) {
    while lexer.kind().is_doc_comment() {
        lexer.next();
    }
}

/// `>>`, `>>>`, `>>=` and `>>>=` are not lexed as single tokens, because that would make
/// `List<List<int>>` ambiguous. Instead the lexer emits a run of adjacent `>` / `>=`
/// tokens and this classifies the run, so each precedence level can consume only the
/// operator that belongs to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GreaterRun {
    /// `>`
    Greater,
    /// `>=`
    GreaterEqual,
    /// `>>`
    RightShift,
    /// `>>>`
    UnsignedRightShift,
    /// `>>=`
    RightShiftAssign,
    /// `>>>=`
    UnsignedRightShiftAssign,
}

/// Classifies the run of adjacent `>`-family tokens at the cursor without consuming it.
/// Returns the run, its combined span, and how many tokens it spans.
pub(crate) fn peek_greater_run(lexer: &mut Lexer<'_>) -> Option<(GreaterRun, Range<usize>, usize)> {
    let anchor = lexer.cast_anchor();

    let mut tokens: std::vec::Vec<Token> = std::vec::Vec::with_capacity(3);
    while tokens.len() < 3 {
        let Some(token) = lexer.current() else {
            break;
        };

        if !matches!(
            token.kind,
            TokenKind::GreaterThan | TokenKind::GreaterThanEqual
        ) {
            break;
        }
        if let Some(previous) = tokens.last()
            && !previous.is_adjacent_to(&token)
        {
            break;
        }

        let is_last = token.kind == TokenKind::GreaterThanEqual;
        tokens.push(token);
        lexer.next();

        // `>=` can only ever close a run
        if is_last {
            break;
        }
    }

    lexer.back_to_anchor(anchor);

    let kinds: std::vec::Vec<_> = tokens.iter().map(|token| token.kind).collect();
    let run = match kinds.as_slice() {
        [TokenKind::GreaterThan] => GreaterRun::Greater,
        [TokenKind::GreaterThanEqual] => GreaterRun::GreaterEqual,
        [TokenKind::GreaterThan, TokenKind::GreaterThan] => GreaterRun::RightShift,
        [TokenKind::GreaterThan, TokenKind::GreaterThanEqual] => GreaterRun::RightShiftAssign,
        [
            TokenKind::GreaterThan,
            TokenKind::GreaterThan,
            TokenKind::GreaterThan,
        ] => GreaterRun::UnsignedRightShift,
        [
            TokenKind::GreaterThan,
            TokenKind::GreaterThan,
            TokenKind::GreaterThanEqual,
        ] => GreaterRun::UnsignedRightShiftAssign,
        _ => return None,
    };

    let span = tokens.first().unwrap().span.start..tokens.last().unwrap().span.end;

    Some((run, span, tokens.len()))
}

/// Consumes `count` tokens, as reported by [`peek_greater_run`].
pub(crate) fn consume_tokens(lexer: &mut Lexer<'_>, count: usize) {
    for _ in 0..count {
        lexer.next();
    }
}
