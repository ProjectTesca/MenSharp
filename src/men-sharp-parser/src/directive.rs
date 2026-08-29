//! Preprocessor directives.
//!
//! # Why these are not part of the syntax tree
//!
//! A directive may sit between any two tokens, including in the middle of an expression.
//! Threading them through the grammar would mean every rule has to know about them, for no
//! benefit: they do not nest with the syntax, they nest with each other. So the lexer
//! treats them as trivia, collects them in source order, and this module gives them
//! structure afterwards.
//!
//! # What this does and does not do
//!
//! This splits a directive line into a kind and the text that follows it. It does **not**
//! evaluate anything: `#if UNITY_EDITOR` is recorded as [`DirectiveKind::If`] with the
//! argument `"UNITY_EDITOR"`, and deciding whether that branch is live is a preprocessing
//! pass's job, since it needs the set of defined symbols, which is a build input rather
//! than a property of the file.
//!
//! One consequence is worth stating plainly: because branches are not evaluated, the code
//! inside *every* `#if` arm is parsed, including arms that would be excluded. For a front
//! end whose next step is to reject directives it cannot support, that is the right
//! trade-off; a build that needs real conditional compilation must run a pass over
//! [`Directive`]s first.

use std::ops::Range;

use crate::{ast::Spanned, lexer::Token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive<'input> {
    pub kind: Spanned<DirectiveKind>,
    /// Everything after the directive name, trimmed, if there was any.
    ///
    /// For `#if` and `#elif` this is the unevaluated condition; for `#pragma` the pragma
    /// body; for `#region` the label.
    pub argument: Option<Spanned<&'input str>>,
    /// The whole line, `#` included.
    pub text: &'input str,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectiveKind {
    If,
    Elif,
    Else,
    Endif,
    Define,
    Undef,
    Region,
    EndRegion,
    Warning,
    Error,
    Line,
    Pragma,
    Nullable,
    /// A `#` followed by something that is not a directive name.
    Unknown,
}

impl DirectiveKind {
    fn from_name(name: &str) -> Self {
        match name {
            "if" => DirectiveKind::If,
            "elif" => DirectiveKind::Elif,
            "else" => DirectiveKind::Else,
            "endif" => DirectiveKind::Endif,
            "define" => DirectiveKind::Define,
            "undef" => DirectiveKind::Undef,
            "region" => DirectiveKind::Region,
            "endregion" => DirectiveKind::EndRegion,
            "warning" => DirectiveKind::Warning,
            "error" => DirectiveKind::Error,
            "line" => DirectiveKind::Line,
            "pragma" => DirectiveKind::Pragma,
            "nullable" => DirectiveKind::Nullable,
            _ => DirectiveKind::Unknown,
        }
    }

    /// Whether this directive opens, continues or closes a conditional section.
    pub fn is_conditional(&self) -> bool {
        matches!(
            self,
            DirectiveKind::If | DirectiveKind::Elif | DirectiveKind::Else | DirectiveKind::Endif
        )
    }
}

/// Splits a [`TokenKind::Directive`](crate::lexer::TokenKind::Directive) token into its
/// name and argument, keeping every span anchored in the original source.
pub fn parse_directive<'input>(token: &Token<'input>) -> Directive<'input> {
    let text = token.text;
    let start = token.span.start;

    // skip `#` and any space between it and the name, as in `#  if`
    let after_hash = 1 + text[1..].len() - text[1..].trim_start().len();
    let rest = &text[after_hash..];

    let name_length = rest
        .find(|char: char| !char.is_alphanumeric() && char != '_')
        .unwrap_or(rest.len());
    let name = &rest[..name_length];

    let kind = Spanned::new(
        DirectiveKind::from_name(name),
        start + after_hash..start + after_hash + name_length,
    );

    let tail = &rest[name_length..];
    let trimmed = tail.trim();
    let argument = (!trimmed.is_empty()).then(|| {
        let offset = after_hash + name_length + (tail.len() - tail.trim_start().len());
        Spanned::new(trimmed, start + offset..start + offset + trimmed.len())
    });

    Directive {
        kind,
        argument,
        text,
        span: token.span.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        directive::{DirectiveKind, parse_directive},
        lexer::{Lexer, TokenKind},
    };

    fn directives(source: &str) -> Vec<(DirectiveKind, Option<&str>)> {
        let mut lexer = Lexer::new(source);
        lexer.by_ref().for_each(drop);

        lexer
            .directives
            .iter()
            .map(|token| {
                let directive = parse_directive(token);
                (directive.kind.value, directive.argument.map(|a| a.value))
            })
            .collect()
    }

    #[test]
    fn directives_are_collected_as_trivia() {
        let source = "#if UNITY_EDITOR\nusing UnityEditor;\n#endif\nclass A { }";
        let mut lexer = Lexer::new(source);

        // the token stream itself never sees them
        assert!(
            lexer
                .by_ref()
                .all(|token| token.kind != TokenKind::Directive)
        );
        assert_eq!(lexer.directives.len(), 2);
    }

    #[test]
    fn kinds_and_arguments() {
        assert_eq!(
            directives(
                "#if A && !B\n#elif C\n#else\n#endif\n#region Fields\n#endregion\n\
                 #define FOO\n#undef FOO\n#pragma warning disable 0649\n#nullable enable\n\
                 #warning careful\n#error nope\n#line 42\n#frobnicate\n#"
            ),
            vec![
                (DirectiveKind::If, Some("A && !B")),
                (DirectiveKind::Elif, Some("C")),
                (DirectiveKind::Else, None),
                (DirectiveKind::Endif, None),
                (DirectiveKind::Region, Some("Fields")),
                (DirectiveKind::EndRegion, None),
                (DirectiveKind::Define, Some("FOO")),
                (DirectiveKind::Undef, Some("FOO")),
                (DirectiveKind::Pragma, Some("warning disable 0649")),
                (DirectiveKind::Nullable, Some("enable")),
                (DirectiveKind::Warning, Some("careful")),
                (DirectiveKind::Error, Some("nope")),
                (DirectiveKind::Line, Some("42")),
                (DirectiveKind::Unknown, None),
                (DirectiveKind::Unknown, None),
            ]
        );
    }

    #[test]
    fn indentation_and_spacing_are_tolerated() {
        assert_eq!(
            directives("    #  if  DEBUG  \n    #endif"),
            vec![
                (DirectiveKind::If, Some("DEBUG")),
                (DirectiveKind::Endif, None),
            ]
        );
    }

    #[test]
    fn spans_index_back_into_the_source() {
        let source = "#if UNITY_EDITOR\nclass A { }";
        let mut lexer = Lexer::new(source);
        lexer.by_ref().for_each(drop);

        let directive = parse_directive(&lexer.directives[0]);
        assert_eq!(&source[directive.span.clone()], "#if UNITY_EDITOR");
        assert_eq!(&source[directive.kind.span.clone()], "if");
        assert_eq!(
            &source[directive.argument.unwrap().span.clone()],
            "UNITY_EDITOR"
        );
    }

    /// A `#` that is not the first thing on its line is not a directive.
    #[test]
    fn a_hash_mid_line_is_not_a_directive() {
        let mut lexer = Lexer::new("a # b");
        let kinds: Vec<_> = lexer.by_ref().map(|token| token.kind).collect();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Identifier,
                TokenKind::Hash,
                TokenKind::Identifier
            ]
        );
        assert!(lexer.directives.is_empty());
    }
}
