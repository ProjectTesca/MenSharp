//! The message catalogs: one TOML file per language, compiled in.
//!
//! The files are flat — `[section]` headers and `key = "text"` lines, with
//! `"""` for text that spans lines — so they are read by the small parser
//! below rather than a TOML library: the compiler has no other use for
//! one, and the subset is what a translator needs. A key is looked up as
//! `section.key`. A language that lacks a key falls back to English, and
//! English to the key itself, so a missing translation is never a crash,
//! only a line of English (or a key) in the output.

use rustc_hash::FxHashMap as HashMap;

use crate::Message;

/// Language codes with a catalog, English first.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", include_str!("../../../messages/en.toml")),
    ("ja", include_str!("../../../messages/ja.toml")),
];

/// Joins the values of a list argument until the catalog says how.
pub(crate) const LIST_SEPARATOR_MARK: &str = "\u{1f}";

/// `ja_JP.UTF-8` → `ja`, `en-US` → `en`; `None` when no catalog matches.
pub fn language_from_locale(locale: &str) -> Option<&'static str> {
    let code = locale
        .split(['_', '-', '.', '@'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    LANGUAGES
        .iter()
        .map(|(language, _)| *language)
        .find(|language| *language == code)
}

pub struct Catalog {
    entries: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl Catalog {
    /// The catalog for a language code, English behind it. An unknown code
    /// is English.
    pub fn for_language(language: &str) -> Catalog {
        let english = LANGUAGES[0].1;
        let text = LANGUAGES
            .iter()
            .find(|(code, _)| *code == language)
            .map(|(_, text)| *text)
            .unwrap_or(english);
        Catalog {
            entries: parse(text),
            fallback: parse(english),
        }
    }

    /// Every key the language file defines, for the tests that keep the
    /// files in step.
    pub fn keys_of(language: &str) -> Vec<String> {
        LANGUAGES
            .iter()
            .find(|(code, _)| *code == language)
            .map(|(_, text)| parse(text).into_keys().collect())
            .unwrap_or_default()
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.entries
            .get(key)
            .or_else(|| self.fallback.get(key))
            .map(String::as_str)
    }

    /// The message's text in this language, placeholders filled.
    pub fn render(&self, message: &Message) -> String {
        let template: String = if message.literal {
            message.key.to_string()
        } else {
            match self.text(&message.key) {
                Some(text) => text.to_string(),
                // no entry anywhere: the key is better than nothing
                None => message.key.to_string(),
            }
        };
        let separator = self.text("ui.list_separator").unwrap_or(", ").to_string();
        fill(&template, &message.args, &separator)
    }
}

/// `{name}` → the argument; `{{` and `}}` → braces.
fn fill(
    template: &str,
    args: &[(std::borrow::Cow<'static, str>, String)],
    separator: &str,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("{{") {
            out.push('{');
            rest = after;
            continue;
        }
        let Some(close) = rest.find('}') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let name = &rest[1..close];
        match args.iter().find(|(candidate, _)| candidate == name) {
            Some((_, value)) => out.push_str(&value.replace(LIST_SEPARATOR_MARK, separator)),
            None => out.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(&rest.replace("}}", "}"));
    out
}

/// The flat TOML subset the catalogs are written in.
fn parse(text: &str) -> HashMap<String, String> {
    let mut entries = HashMap::default();
    let mut section = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        let parsed = if let Some(rest) = value.strip_prefix("\"\"\"") {
            // multi-line basic string: runs to the closing `"""`, a newline
            // right after the opening one dropped, as TOML has it
            let mut body = String::new();
            let mut current = rest.to_string();
            let mut first = true;
            loop {
                if let Some(end) = current.find("\"\"\"") {
                    body.push_str(&current[..end]);
                    break;
                }
                if !(first && current.is_empty()) {
                    body.push_str(&current);
                    body.push('\n');
                }
                first = false;
                match lines.next() {
                    Some(next) => current = next.to_string(),
                    None => break,
                }
            }
            unescape(&body)
        } else if let Some(rest) = value.strip_prefix('"') {
            let end = rest.rfind('"').unwrap_or(rest.len());
            unescape(&rest[..end])
        } else {
            value.to_string()
        };
        let full = if section.is_empty() {
            key.to_string()
        } else {
            format!("{section}.{key}")
        };
        entries.insert(full, parsed);
    }
    entries
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            // `\` at a line end: the newline and leading whitespace of the
            // next line are dropped (TOML's line-ending backslash)
            Some('\n') => {
                let rest = chars.as_str();
                let trimmed = rest.trim_start();
                chars = trimmed.chars();
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_strings() {
        let entries = parse(
            "# comment\n[semantics]\ntype_mismatch = \"expected `{expected}`, found `{found}`\"\n\
             long = \"\"\"\nfirst\nsecond\"\"\"\n[ui]\nlist_separator = \", \"\n",
        );
        assert_eq!(
            entries["semantics.type_mismatch"],
            "expected `{expected}`, found `{found}`"
        );
        assert_eq!(entries["semantics.long"], "first\nsecond");
        assert_eq!(entries["ui.list_separator"], ", ");
    }

    #[test]
    fn fills_placeholders_and_lists() {
        let separator = ", ";
        let message = Message::key("x")
            .arg("expected", "int")
            .list("missing", &["A".to_string(), "B".to_string()]);
        assert_eq!(
            fill(
                "{expected}: {missing} {{literal}}",
                &message.args,
                separator
            ),
            "int: A, B {literal}"
        );
    }

    #[test]
    fn every_language_defines_the_english_keys() {
        let english = Catalog::keys_of("en");
        for (language, _) in LANGUAGES {
            let keys = Catalog::keys_of(language);
            let missing: Vec<&String> = english.iter().filter(|key| !keys.contains(key)).collect();
            assert!(missing.is_empty(), "{language} lacks {missing:?}");
            let extra: Vec<&String> = keys.iter().filter(|key| !english.contains(key)).collect();
            assert!(
                extra.is_empty(),
                "{language} has keys English lacks: {extra:?}"
            );
        }
    }

    #[test]
    fn locale_codes_map_to_catalogs() {
        assert_eq!(language_from_locale("ja_JP.UTF-8"), Some("ja"));
        assert_eq!(language_from_locale("en-US"), Some("en"));
        assert_eq!(language_from_locale("de_DE"), None);
    }
}
