//! Unicode escapes and formatting characters in identifiers (C# §6.4.3):
//! `int \u0061 = 1;` declares `a`, `\U00000061` is the same `a`, and
//! `a\u200Db` — or `a` followed by a literal zero-width joiner and `b` — is
//! `ab`, because two identifiers are the same once every escape is turned
//! into its character and every formatting character (Unicode `Cf`) is
//! dropped. An escaped keyword stays an identifier: `\u0069nt` names a
//! variable `int`, never the type.
//!
//! The lexer works on `&str` slices of the source, so an identifier that
//! reads differently from how it is spelled has to be spelled that way in
//! the text the lexer sees. This pass rewrites every such identifier in
//! place — its normalized form, then spaces up to the original length, so
//! the file keeps exactly the byte offsets and line structure of the
//! original and every span still points into the user's source (as
//! [`crate::preprocess`] does for inactive `#if` arms). A normalized name
//! that would lex as a keyword gets the `@` prefix, which the parser strips
//! again. Every escape frees at least three bytes, so the room is always
//! there. Strings, characters and comments are stepped over untouched:
//! their escapes are their own.
//!
//! Not rewritten: an escape inside an interpolation hole (`$"{\u0061}"`),
//! whose text the lexer hands to a sub-lexer later.

use std::borrow::Cow;

use regex::Regex;

use crate::lexer::{Lexer, scan};

/// The source with every escaped or formatting-character-bearing identifier
/// spelled in its normalized form. Returns the input unchanged (no copy)
/// when nothing needed rewriting.
pub fn unescape_identifiers(source: &str) -> Cow<'_, str> {
    if source.is_ascii() && !source.contains("\\u") && !source.contains("\\U") {
        return Cow::Borrowed(source);
    }
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut changed = false;
    let mut index = 0;
    while index < bytes.len() {
        let rest = &source[index..];
        // what an identifier cannot be inside of is stepped over whole
        let skip = match bytes[index] {
            b'/' => [
                scan::block_doc_comment(rest),
                scan::block_comment(rest),
                scan::line_doc_comment(rest),
                scan::line_comment(rest),
            ]
            .into_iter()
            .max()
            .unwrap_or(0),
            b'"' => scan::raw_string(rest).max(scan::string(rest)),
            b'\'' => scan::char(rest),
            b'$' => scan::interpolated_string(rest),
            b'@' if rest.starts_with("@\"") || rest.starts_with("@$") => {
                scan::verbatim_string(rest).max(scan::interpolated_string(rest))
            }
            b'#' => scan::directive(rest),
            _ => 0,
        };
        if skip > 0 {
            out.push_str(&rest[..skip]);
            index += skip;
            continue;
        }
        if let Some(identifier) = ExtendedIdentifier::scan(rest) {
            let run = &rest[..identifier.length];
            match identifier.normalized(run) {
                Some(normalized) => {
                    out.push_str(&normalized);
                    out.extend(std::iter::repeat_n(' ', run.len() - normalized.len()));
                    changed = true;
                }
                None => out.push_str(run),
            }
            index += identifier.length;
            continue;
        }
        let character = rest.chars().next().expect("index is on a char boundary");
        out.push(character);
        index += character.len_utf8();
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(source)
    }
}

/// An identifier as written: its characters with every escape decoded, and
/// whether anything about the spelling differs from the reading.
struct ExtendedIdentifier {
    /// Bytes of source the identifier occupies, `@` and escapes included.
    length: usize,
    verbatim: bool,
    characters: Vec<char>,
    has_escape: bool,
    has_formatting: bool,
}

impl ExtendedIdentifier {
    /// The identifier `rest` starts with, escapes allowed anywhere in it;
    /// `None` when `rest` starts with something else.
    fn scan(rest: &str) -> Option<Self> {
        let verbatim = rest.starts_with('@');
        let mut position = usize::from(verbatim);
        let mut characters = Vec::new();
        let mut has_escape = false;
        let mut has_formatting = false;
        loop {
            let remaining = &rest[position..];
            if let Some((character, width)) = escape_at(remaining) {
                if !is_identifier_char(character, characters.is_empty()) {
                    break;
                }
                has_escape = true;
                has_formatting |= is_formatting(character);
                characters.push(character);
                position += width;
                continue;
            }
            let Some(character) = remaining.chars().next() else {
                break;
            };
            if !is_identifier_char(character, characters.is_empty()) {
                break;
            }
            has_formatting |= is_formatting(character);
            characters.push(character);
            position += character.len_utf8();
        }
        if characters.is_empty() {
            return None;
        }
        Some(Self {
            length: position,
            verbatim,
            characters,
            has_escape,
            has_formatting,
        })
    }

    /// The spelling the lexer should see, or `None` when the one written
    /// already is it (or nothing valid can replace it).
    fn normalized(&self, run: &str) -> Option<String> {
        if !self.has_escape && !self.has_formatting {
            return None;
        }
        let mut name = String::new();
        for &character in &self.characters {
            if !is_formatting(character) {
                name.push(character);
            }
        }
        // what is left has to be an identifier still: `1x` is not
        if scan::identifier(&name) != name.len() || name.is_empty() {
            return None;
        }
        let mut spelled = String::new();
        // `int` is an identifier named `int`, which the lexer only
        // reads as one behind an `@`
        let kind = Lexer::new(&name).current().map(|token| token.kind);
        if self.verbatim || !kind.is_some_and(|kind| kind.can_be_identifier()) {
            spelled.push('@');
        }
        spelled.push_str(&name);
        (spelled.len() <= run.len()).then_some(spelled)
    }
}

/// `\uXXXX` or `\UXXXXXXXX` at the start of `rest`: the character and the
/// bytes the escape spans.
fn escape_at(rest: &str) -> Option<(char, usize)> {
    let digits = match rest.as_bytes() {
        [b'\\', b'u', ..] => 4,
        [b'\\', b'U', ..] => 8,
        _ => return None,
    };
    let hex = rest.get(2..2 + digits)?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some((char::from_u32(value)?, 2 + digits))
}

/// `[_\p{L}\p{Nl}]` for the first character, then also
/// `[\p{Mn}\p{Mc}\p{Nd}\p{Cf}]` — the identifier rule of the specification.
fn is_identifier_char(character: char, first: bool) -> bool {
    if character.is_ascii() {
        return character == '_'
            || character.is_ascii_alphabetic()
            || (!first && character.is_ascii_digit());
    }
    static START: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static PART: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let regex = if first {
        START.get_or_init(|| Regex::new(r"^[_\p{L}\p{Nl}]$").unwrap())
    } else {
        PART.get_or_init(|| Regex::new(r"^[_\p{L}\p{Nl}\p{Mn}\p{Mc}\p{Nd}\p{Cf}]$").unwrap())
    };
    regex.is_match(character.encode_utf8(&mut [0; 4]))
}

/// A formatting character (Unicode `Cf`): part of an identifier's spelling,
/// not of its identity.
fn is_formatting(character: char) -> bool {
    if character.is_ascii() {
        return false;
    }
    static FORMATTING: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    FORMATTING
        .get_or_init(|| Regex::new(r"^\p{Cf}$").unwrap())
        .is_match(character.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::unescape_identifiers;

    fn rewritten(source: &str) -> String {
        let out = unescape_identifiers(source).into_owned();
        assert_eq!(out.len(), source.len(), "the length must not change");
        out
    }

    #[test]
    fn escapes_become_their_characters_padded_to_the_same_length() {
        assert_eq!(rewritten("int \\u0061 = 1;"), "int a      = 1;");
        assert_eq!(rewritten("\\U00000061 = 2;"), "a          = 2;");
        assert_eq!(rewritten("int a\\u200Db = 3;"), "int ab       = 3;");
    }

    #[test]
    fn a_literal_formatting_character_is_dropped_too() {
        assert_eq!(rewritten("int a\u{200D}b = 4;"), "int ab    = 4;");
    }

    #[test]
    fn an_escaped_keyword_is_an_identifier_behind_an_at() {
        assert_eq!(rewritten("\\u0069nt x = 1;"), "@int     x = 1;");
        assert_eq!(rewritten("int @\\u0061 = 5;"), "int @a      = 5;");
        // a contextual keyword is an identifier already
        assert_eq!(rewritten("\\u0076ar x = 1;"), "var      x = 1;");
    }

    #[test]
    fn strings_characters_and_comments_keep_their_escapes() {
        let source = "var s = \"\\u0061\"; var c = '\\u0061'; // \\u0061\n/* \\u0061 */ var t = $\"{x}\\u0061\"; var v = @\"\\u0061\";";
        assert!(matches!(
            unescape_identifiers(source),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn what_is_no_identifier_after_decoding_is_left_alone() {
        assert_eq!(rewritten("\\u0031x = 1;"), "\\u0031x = 1;");
        assert_eq!(rewritten("a\\u0020b"), "a\\u0020b");
    }

    #[test]
    fn plain_sources_are_not_copied() {
        assert!(matches!(
            unescape_identifiers("int a = 1; // plain"),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
