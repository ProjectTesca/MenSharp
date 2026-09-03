//! Conditional compilation: `#if` / `#elif` / `#else` / `#endif`, `#define`
//! and `#undef`, evaluated against a set of defined symbols *before* lexing.
//!
//! The lexer treats every directive as trivia and never decides which arm
//! of an `#if` is live (see [`crate::directive`]) — that needs the set of
//! defined symbols, a build input. This pass supplies it the simplest way
//! there is: the text of every inactive arm is replaced by spaces, newlines
//! kept, so the file the lexer sees has exactly the byte offsets and line
//! structure of the original and every span still points into the user's
//! source. The directive lines themselves stay, so they are still recorded.
//!
//! Conditions are C#'s (§6.5.4): identifiers, `true`/`false`, `!`, `&&`,
//! `||`, `==`, `!=`, parentheses. A malformed condition counts as false; an
//! unbalanced `#endif` is ignored; an unclosed `#if` runs to the end.

use std::collections::HashSet;

/// The source with every inactive conditional arm blanked out.
///
/// Returns the input unchanged (no copy) when it has no `#if` at all.
pub fn preprocess<'a>(source: &'a str, defines: &[&str]) -> std::borrow::Cow<'a, str> {
    if !source.contains("#if") && !source.contains("#define") {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut defined: HashSet<&str> = defines.iter().copied().collect();
    let mut out = Vec::with_capacity(source.len());
    let mut stack: Vec<Frame> = Vec::new();

    for line in split_lines(source) {
        let active = stack.last().is_none_or(|frame| frame.active);
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim_start();
            let (name, argument) = split_directive(rest);
            match name {
                "if" => {
                    let taken = active && evaluate(argument, &defined);
                    stack.push(Frame {
                        parent_active: active,
                        taken,
                        active: taken,
                    });
                }
                "elif" => {
                    if let Some(frame) = stack.last_mut() {
                        if frame.taken {
                            frame.active = false;
                        } else {
                            frame.active = frame.parent_active && evaluate(argument, &defined);
                            frame.taken |= frame.active;
                        }
                    }
                }
                "else" => {
                    if let Some(frame) = stack.last_mut() {
                        frame.active = frame.parent_active && !frame.taken;
                        frame.taken = true;
                    }
                }
                "endif" => {
                    stack.pop();
                }
                "define" if active => {
                    if let Some(symbol) = identifier(argument) {
                        defined.insert(symbol);
                    }
                }
                "undef" if active => {
                    if let Some(symbol) = identifier(argument) {
                        defined.remove(symbol);
                    }
                }
                _ => {}
            }
            // the directive line itself stays: the lexer records it
            out.extend_from_slice(line.as_bytes());
            continue;
        }
        if active {
            out.extend_from_slice(line.as_bytes());
        } else {
            out.extend(line.bytes().map(|byte| match byte {
                b'\n' | b'\r' => byte,
                _ => b' ',
            }));
        }
    }
    // every replaced byte is ASCII and every kept byte came from a `str`
    std::borrow::Cow::Owned(String::from_utf8(out).expect("blanking keeps UTF-8 valid"))
}

struct Frame {
    parent_active: bool,
    taken: bool,
    active: bool,
}

/// Lines with their terminators kept, so joining them gives the input back.
fn split_lines(source: &str) -> impl Iterator<Item = &str> {
    let mut rest = source;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = match rest.find('\n') {
            Some(index) => index + 1,
            None => rest.len(),
        };
        let (line, tail) = rest.split_at(end);
        rest = tail;
        Some(line)
    })
}

/// `"if UNITY_EDITOR // note"` → `("if", "UNITY_EDITOR")`.
fn split_directive(rest: &str) -> (&str, &str) {
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (name, argument) = rest.split_at(name_end);
    let argument = argument.trim();
    let argument = match argument.find("//") {
        Some(index) => argument[..index].trim(),
        None => argument,
    };
    (name, argument)
}

fn identifier(argument: &str) -> Option<&str> {
    let end = argument
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(argument.len());
    (end > 0).then(|| &argument[..end])
}

/// Evaluates a `#if` condition. Anything malformed is false.
fn evaluate(condition: &str, defined: &HashSet<&str>) -> bool {
    let tokens = tokenize(condition);
    let mut parser = ConditionParser {
        tokens: &tokens,
        position: 0,
        defined,
    };
    let value = parser.or();
    match value {
        Some(value) if parser.position == tokens.len() => value,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Identifier(&'a str),
    Not,
    And,
    Or,
    Equal,
    NotEqual,
    Open,
    Close,
    Bad,
}

fn tokenize(condition: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = condition.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b' ' | b'\t' => index += 1,
            b'(' => {
                tokens.push(Token::Open);
                index += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                index += 1;
            }
            b'!' if bytes.get(index + 1) == Some(&b'=') => {
                tokens.push(Token::NotEqual);
                index += 2;
            }
            b'!' => {
                tokens.push(Token::Not);
                index += 1;
            }
            b'=' if bytes.get(index + 1) == Some(&b'=') => {
                tokens.push(Token::Equal);
                index += 2;
            }
            b'&' if bytes.get(index + 1) == Some(&b'&') => {
                tokens.push(Token::And);
                index += 2;
            }
            b'|' if bytes.get(index + 1) == Some(&b'|') => {
                tokens.push(Token::Or);
                index += 2;
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                if index == start {
                    tokens.push(Token::Bad);
                    index += 1;
                } else {
                    tokens.push(Token::Identifier(&condition[start..index]));
                }
            }
        }
    }
    tokens
}

struct ConditionParser<'a, 'b> {
    tokens: &'b [Token<'a>],
    position: usize,
    defined: &'b HashSet<&'a str>,
}

impl ConditionParser<'_, '_> {
    fn peek(&self) -> Option<&Token<'_>> {
        self.tokens.get(self.position)
    }

    fn or(&mut self) -> Option<bool> {
        let mut value = self.and()?;
        while self.peek() == Some(&Token::Or) {
            self.position += 1;
            value |= self.and()?;
        }
        Some(value)
    }

    fn and(&mut self) -> Option<bool> {
        let mut value = self.equality()?;
        while self.peek() == Some(&Token::And) {
            self.position += 1;
            value &= self.equality()?;
        }
        Some(value)
    }

    fn equality(&mut self) -> Option<bool> {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::Equal) => {
                    self.position += 1;
                    value = value == self.unary()?;
                }
                Some(Token::NotEqual) => {
                    self.position += 1;
                    value = value != self.unary()?;
                }
                _ => return Some(value),
            }
        }
    }

    fn unary(&mut self) -> Option<bool> {
        if self.peek() == Some(&Token::Not) {
            self.position += 1;
            return Some(!self.unary()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Option<bool> {
        match self.peek()? {
            Token::Identifier("true") => {
                self.position += 1;
                Some(true)
            }
            Token::Identifier("false") => {
                self.position += 1;
                Some(false)
            }
            Token::Identifier(name) => {
                let value = self.defined.contains(name);
                self.position += 1;
                Some(value)
            }
            Token::Open => {
                self.position += 1;
                let value = self.or()?;
                if self.peek() != Some(&Token::Close) {
                    return None;
                }
                self.position += 1;
                Some(value)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defined(names: &[&str]) -> HashSet<&'static str> {
        names
            .iter()
            .map(|name| Box::leak(name.to_string().into_boxed_str()) as &str)
            .collect()
    }

    #[test]
    fn conditions_follow_the_language() {
        let set = defined(&["UNITY_EDITOR", "COMPILER_UDONSHARP"]);
        assert!(evaluate("UNITY_EDITOR", &set));
        assert!(!evaluate("DEBUG", &set));
        assert!(!evaluate("!COMPILER_UDONSHARP && UNITY_EDITOR", &set));
        assert!(evaluate("!DEBUG && UNITY_EDITOR", &set));
        assert!(evaluate("DEBUG || UNITY_EDITOR", &set));
        assert!(evaluate("(DEBUG || UNITY_EDITOR) && true", &set));
        assert!(evaluate("UNITY_EDITOR == true", &set));
        assert!(evaluate("DEBUG != true", &set));
        assert!(evaluate("!(DEBUG)", &set));
        // malformed: false, never a panic
        assert!(!evaluate("UNITY_EDITOR &&", &set));
        assert!(!evaluate("(UNITY_EDITOR", &set));
        assert!(!evaluate("", &set));
        assert!(!evaluate("UNITY_EDITOR ~ DEBUG", &set));
    }

    #[test]
    fn inactive_arms_are_blanked_and_offsets_kept() {
        let source = "#if UNITY_EDITOR\nusing UnityEditor;\n#else\nusing X;\n#endif\nclass A { }\n";
        let out = preprocess(source, &[]);
        assert_eq!(out.len(), source.len());
        assert_eq!(
            &*out,
            "#if UNITY_EDITOR\n                  \n#else\nusing X;\n#endif\nclass A { }\n"
        );
        let out = preprocess(source, &["UNITY_EDITOR"]);
        assert_eq!(
            &*out,
            "#if UNITY_EDITOR\nusing UnityEditor;\n#else\n        \n#endif\nclass A { }\n"
        );
    }

    #[test]
    fn elif_nesting_define_and_multibyte_text() {
        let source = "#define A\n#if B\n1\n#elif A\n2\n#if C\n3\n#else\n4\n#endif\n#else\n5\n#endif\n#undef A\n#if A\n日本語\n#endif\nend";
        let out = preprocess(source, &[]);
        assert_eq!(out.len(), source.len());
        let kept: Vec<&str> = out
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .collect();
        assert_eq!(kept, vec!["2", "4", "end"]);
    }

    #[test]
    fn a_file_without_conditionals_is_untouched() {
        let source = "#pragma warning disable\nclass A { }";
        assert!(matches!(
            preprocess(source, &["X"]),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
