//! What the compiler has to say, before it is said in any language.
//!
//! A [`Diagnostic`] names *where* (a file and a byte span) and *what* (a
//! [`Message`]: a key into the message catalog plus the values that fill
//! its holes). The text a person reads is produced later by a
//! [`Catalog`] for the language they asked for — `messages/en.toml`,
//! `messages/ja.toml` — so the phases that find problems know nothing
//! about wording, and a translation is a file, not a code change.
//!
//! A diagnostic may carry [`Hint`]s: a suggestion, optionally with an
//! [`Edit`] the renderer applies to a copy of the source, so the reader
//! sees the line the way it would be written after the fix.
//!
//! [`Reporter`] renders them for a reader: colours, source excerpts and
//! the marked span, in the language and format it is told. What it is not
//! told it does not decide — which language, whether the output is a
//! terminal, where the files are — so the crate reads no environment and
//! does no I/O; that is the CLI's business, in the `men-sharp` binary.

use std::borrow::Cow;
use std::ops::Range;

mod catalog;
mod render;

pub use catalog::{Catalog, LANGUAGES, language_from_locale};
pub use render::{Format, Reporter, Sources, line_column};

/// A catalog key with the values for its placeholders — `{expected}` in
/// the template is filled from the argument named `expected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub key: Cow<'static, str>,
    pub args: Vec<(Cow<'static, str>, String)>,
    /// The key is the text itself, shown as written when the catalog has
    /// no entry for it. For messages that are not worth translating (an
    /// internal inconsistency, say) and for text already in the reader's
    /// language.
    pub literal: bool,
}

impl Message {
    pub fn key(key: impl Into<Cow<'static, str>>) -> Self {
        Message {
            key: key.into(),
            args: Vec::new(),
            literal: false,
        }
    }

    /// A message whose text is the given string.
    pub fn literal(text: impl Into<Cow<'static, str>>) -> Self {
        Message {
            key: text.into(),
            args: Vec::new(),
            literal: true,
        }
    }

    /// A message worth a catalog entry, from text written in the code:
    /// `internal:` notes and other text that stays as written.
    pub fn from_text(text: impl Into<String>) -> Self {
        Message::literal(text.into())
    }

    pub fn arg(mut self, name: impl Into<Cow<'static, str>>, value: impl ToString) -> Self {
        self.args.push((name.into(), value.to_string()));
        self
    }

    /// A list argument: joined the way the catalog's `ui.list_separator`
    /// says, at render time.
    pub fn list(mut self, name: impl Into<Cow<'static, str>>, values: &[String]) -> Self {
        self.args
            .push((name.into(), values.join(catalog::LIST_SEPARATOR_MARK)));
        self
    }
}

/// The English text: what tests and logs see without choosing a language.
impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        static ENGLISH: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
        let catalog = ENGLISH.get_or_init(|| Catalog::for_language("en"));
        f.write_str(&catalog.render(self))
    }
}

impl From<&'static str> for Message {
    fn from(text: &'static str) -> Self {
        Message::literal(text)
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Message::literal(text)
    }
}

/// A change to the source at `span`. An insertion has an empty span; a
/// deletion, an empty replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub file: u32,
    pub span: Range<usize>,
    pub replacement: Replacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replacement {
    /// The span becomes this text.
    Text(String),
    /// The span is kept, with text put before and after it: `(int)(` ...
    /// `)` around an expression.
    Wrap { before: String, after: String },
}

impl Edit {
    pub fn insert(file: u32, at: usize, text: impl Into<String>) -> Self {
        Edit {
            file,
            span: at..at,
            replacement: Replacement::Text(text.into()),
        }
    }

    pub fn wrap(
        file: u32,
        span: Range<usize>,
        before: impl Into<String>,
        after: impl Into<String>,
    ) -> Self {
        Edit {
            file,
            span,
            replacement: Replacement::Wrap {
                before: before.into(),
                after: after.into(),
            },
        }
    }
}

/// A suggestion attached to a diagnostic. With an edit, the renderer
/// shows the source as it would read after the change, the change marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub message: Message,
    pub edit: Option<Edit>,
}

impl Hint {
    pub fn text(message: Message) -> Self {
        Hint {
            message,
            edit: None,
        }
    }

    pub fn edit(message: Message, edit: Edit) -> Self {
        Hint {
            message,
            edit: Some(edit),
        }
    }
}

/// A second place the diagnostic points at (`first declared here`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub file: u32,
    pub span: Range<usize>,
    pub message: Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    Syntax,
    Semantics,
    Codegen,
}

impl Phase {
    /// The heading a diagnostic of the phase is reported under when its
    /// kind has no more specific one.
    pub fn heading(self) -> &'static str {
        match self {
            Phase::Syntax => "SyntaxError",
            Phase::Semantics => "SemanticsError",
            Phase::Codegen => "CodegenError",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: Phase,
    /// `[TypeError]`, `[NameError]`, ...: what kind of problem, in a word.
    pub heading: &'static str,
    /// Index into the compilation's file list.
    pub file: u32,
    pub span: Range<usize>,
    pub message: Message,
    pub labels: Vec<Label>,
    pub hints: Vec<Hint>,
}

impl Diagnostic {
    pub fn new(phase: Phase, file: u32, span: Range<usize>, message: Message) -> Self {
        Diagnostic {
            phase,
            heading: phase.heading(),
            file,
            span,
            message,
            labels: Vec::new(),
            hints: Vec::new(),
        }
    }

    pub fn with_heading(mut self, heading: &'static str) -> Self {
        self.heading = heading;
        self
    }

    pub fn pointing_at(mut self, file: u32, span: Range<usize>, message: Message) -> Self {
        self.labels.push(Label {
            file,
            span,
            message,
        });
        self
    }

    pub fn with_hint(mut self, hint: Hint) -> Self {
        self.hints.push(hint);
        self
    }

    /// The order diagnostics are reported in: by file, then position, then
    /// phase, then wording — a total order over what the compiler found,
    /// so the same sources always print the same list, whatever threads
    /// found what first.
    pub fn sort_key(&self) -> (u32, usize, usize, Phase, String, Vec<(String, String)>) {
        (
            self.file,
            self.span.start,
            self.span.end,
            self.phase,
            self.message.key.to_string(),
            self.message
                .args
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
        )
    }
}

/// Puts diagnostics into their reporting order.
pub fn sort(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_cached_key(Diagnostic::sort_key);
}
