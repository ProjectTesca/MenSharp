//! Turning diagnostics into what a person reads.
//!
//! A [`Diagnostic`] says where and what in no particular language and no
//! particular shape; a [`Reporter`] gives it both. One layout serves every
//! reader: the message, then the source lines the error touches with the
//! offending part marked *inside* the line, then the other places involved
//! and the hints — a hint with an edit shows the line as it would read
//! after the change, the change marked. Nothing is drawn under or beside
//! the source, so the layout survives a proportional font and a console
//! that trims leading whitespace, which Unity's does. What differs per
//! [`Format`] is only the markup:
//!
//! - **rich** — a terminal: ANSI colour and bold.
//! - **unity** — for Unity's console, which lists the first two lines of
//!   an entry and shows the rest when it is selected: the heading and the
//!   marked source line first, then, set apart by blank lines, the full
//!   text as the terminal shows it, with Unity's rich text tags
//!   (`<color><b>`). The lines after the first are indented four spaces so
//!   the editor driver keeps them together as one entry.
//! - **short** — plain text for other tools: one
//!   `file(line,column): error: message` line a console can click on, the
//!   rest indented as above.
//!
//! Where the text comes from is the caller's business: a [`Sources`] hands
//! back a file's name and its text by the index a diagnostic carries. So is
//! *whether* to colour and in which language — a reporter is told, and
//! reads no environment of its own. Nothing here does I/O: a reporter
//! returns a `String`.

use std::ops::Range;

use crate::{Catalog, Diagnostic, Edit, Message, Replacement};

/// The files a set of diagnostics points into, by the index they carry.
pub trait Sources {
    fn name(&self, file: u32) -> &str;
    fn source(&self, file: u32) -> &str;
}

/// The markup a rendered diagnostic is dressed in; see the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rich,
    Short,
    Unity,
}

/// What a marked part of a line is: the error itself, another place the
/// error involves, or the change a hint makes.
#[derive(Clone, Copy)]
enum Mark {
    Error,
    Label,
    Hint,
}

impl Mark {
    fn ansi(self) -> &'static str {
        match self {
            Mark::Error => "1;31",
            Mark::Label => "1;33",
            Mark::Hint => "1;36",
        }
    }

    fn unity(self) -> &'static str {
        match self {
            Mark::Error => "#ff6b6b",
            Mark::Label => "#ffd166",
            Mark::Hint => "#5ad1e6",
        }
    }
}

pub struct Reporter<'a> {
    sources: &'a dyn Sources,
    catalog: Catalog,
    format: Format,
    color: bool,
}

impl<'a> Reporter<'a> {
    pub fn new(sources: &'a dyn Sources, language: &str, format: Format, color: bool) -> Self {
        Reporter {
            sources,
            catalog: Catalog::for_language(language),
            format,
            color,
        }
    }

    /// A `ui.*` line with `{count}` filled in.
    pub fn count_line(&self, key: &'static str, count: usize) -> String {
        self.catalog.render(&Message::key(key).arg("count", count))
    }

    /// Every diagnostic, in reporting order, as one text.
    pub fn render(&self, diagnostics: &mut [Diagnostic]) -> String {
        crate::sort(diagnostics);
        let mut out = String::new();
        for diagnostic in diagnostics.iter() {
            out.push_str(&self.render_one(diagnostic));
            out.push('\n');
        }
        out
    }

    fn file_name(&self, file: u32) -> &str {
        self.sources.name(file)
    }

    fn source(&self, file: u32) -> &str {
        self.sources.source(file)
    }

    fn text(&self, message: &Message) -> String {
        self.catalog.render(message)
    }

    fn mark(&self, text: &str, mark: Mark) -> String {
        match self.format {
            Format::Rich if self.color => format!("\x1b[{}m{text}\x1b[0m", mark.ansi()),
            Format::Rich | Format::Short => text.to_string(),
            Format::Unity => format!("<color={}><b>{text}</b></color>", mark.unity()),
        }
    }

    /// Lines that belong to the diagnostic above them: indented in the
    /// driver formats, which is how a driver tells them apart.
    fn detail(&self, line: &str) -> String {
        match self.format {
            Format::Rich => format!("{line}\n"),
            Format::Short | Format::Unity => format!("    {line}\n"),
        }
    }

    fn render_one(&self, diagnostic: &Diagnostic) -> String {
        let mut out = String::new();
        let message = self.text(&diagnostic.message);
        let (line, column) = line_column(self.source(diagnostic.file), diagnostic.span.start);
        let heading = self.mark(&format!("[{}]", diagnostic.heading), Mark::Error);
        let excerpt =
            self.marked_lines(self.source(diagnostic.file), &diagnostic.span, Mark::Error);
        match self.format {
            Format::Unity => {
                // the console's list shows an entry's first two lines: the
                // message and the line it is about; the rest is the detail
                out.push_str(&format!("{message}\n"));
                if let Some(first) = excerpt.lines().next() {
                    out.push_str(first);
                    out.push('\n');
                }
                // a blank line sets the full text apart from the summary
                out.push_str(&self.detail(""));
                out.push_str(&self.detail(&format!("{heading} {message}")));
                out.push_str(&self.detail(&format!(
                    "  --> {}:{line}:{column}",
                    self.file_name(diagnostic.file)
                )));
            }
            Format::Rich => {
                out.push_str(&format!("{heading} {message}\n"));
                out.push_str(&self.detail(&format!(
                    "  --> {}:{line}:{column}",
                    self.file_name(diagnostic.file)
                )));
            }
            Format::Short => {
                out.push_str(&format!(
                    "{}({line},{column}): error: {message}\n",
                    self.file_name(diagnostic.file)
                ));
            }
        }
        out.push_str(&excerpt);

        for label in &diagnostic.labels {
            let (line, column) = line_column(self.source(label.file), label.span.start);
            out.push_str(&self.detail(&format!(
                "  {}:{line}:{column}: {}",
                self.file_name(label.file),
                self.text(&label.message)
            )));
            out.push_str(&self.marked_lines(self.source(label.file), &label.span, Mark::Label));
        }

        for hint in &diagnostic.hints {
            let heading = self.catalog.text("ui.hint_heading").unwrap_or("Hint");
            let heading = match self.format {
                Format::Rich | Format::Unity => self.mark(&format!("[{heading}]"), Mark::Hint),
                Format::Short => format!("{heading}:"),
            };
            out.push_str(&self.detail(&format!("{heading} {}", self.text(&hint.message))));
            if let Some(edit) = &hint.edit {
                let (edited, span) = apply(self.source(edit.file), edit);
                out.push_str(&self.marked_lines(&edited, &span, Mark::Hint));
            }
        }
        out
    }

    /// Every source line `span` touches, numbered, with the part inside
    /// the span marked; an empty span marks the character at its position.
    fn marked_lines(&self, source: &str, span: &Range<usize>, mark: Mark) -> String {
        let start = span.start.min(source.len());
        let end = span.end.clamp(start, source.len());
        let mut out = String::new();
        let mut offset = 0;
        for (index, line) in source.split_inclusive('\n').enumerate() {
            let line_start = offset;
            offset += line.len();
            let text = line.trim_end_matches(['\n', '\r']);
            let text_end = line_start + text.len();
            let touches = if start == end {
                (line_start..=text_end).contains(&start)
            } else {
                start < text_end.max(line_start + 1) && end > line_start
            };
            if !touches {
                continue;
            }
            let from = start.clamp(line_start, text_end) - line_start;
            let to = end.clamp(line_start, text_end) - line_start;
            let (before, rest) = text.split_at(from);
            let (inside, after) = rest.split_at(to - from);
            let inside = if inside.is_empty() { "▏" } else { inside };
            let rendered = format!(
                "{:>4} │ {}{}{}",
                index + 1,
                before.replace('\t', "    "),
                self.mark(&inside.replace('\t', "    "), mark),
                after.replace('\t', "    ")
            );
            out.push_str(&self.detail(&rendered));
        }
        out
    }
}

/// The source with the edit applied, and where the change sits in it.
fn apply(source: &str, edit: &Edit) -> (String, Range<usize>) {
    let start = edit.span.start.min(source.len());
    let end = edit.span.end.clamp(start, source.len());
    let replacement = match &edit.replacement {
        Replacement::Text(text) => text.clone(),
        Replacement::Wrap { before, after } => format!("{before}{}{after}", &source[start..end]),
    };
    let mut edited = String::with_capacity(source.len() + replacement.len());
    edited.push_str(&source[..start]);
    edited.push_str(&replacement);
    edited.push_str(&source[end..]);
    (edited, start..start + replacement.len())
}

/// 1-based line and column (in characters) of a byte offset.
pub fn line_column(source: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(source.len());
    let before = &source[..byte];
    let line = before.matches('\n').count() + 1;
    let column = before.chars().rev().take_while(|c| *c != '\n').count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_an_edit_places_the_change() {
        let (edited, span) = apply("int x = 1\n", &Edit::insert(0, 9, ";"));
        assert_eq!(edited, "int x = 1;\n");
        assert_eq!(span, 9..10);
        let (edited, span) = apply("int x = 1.5 * 2;\n", &Edit::wrap(0, 8..15, "(int)(", ")"));
        assert_eq!(edited, "int x = (int)(1.5 * 2);\n");
        assert_eq!(span, 8..22);
    }
}
