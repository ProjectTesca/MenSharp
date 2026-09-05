//! Turning what the compiler found into what a person reads.
//!
//! Every phase leaves language-neutral diagnostics (see the
//! `men-sharp-diagnostics` crate); this module gathers them, sorts them
//! into one stable order, and renders each in the language asked for.
//!
//! One layout serves every reader: the message, then the source lines the
//! error touches with the offending part marked *inside* the line, then
//! the other places involved and the hints — a hint with an edit shows
//! the line as it would read after the change, the change marked. Nothing
//! is drawn under or beside the source, so the layout survives a
//! proportional font and a console that trims leading whitespace, which
//! Unity's does. What differs per format is only the markup:
//!
//! - **rich** — a terminal: ANSI colour and bold.
//! - **unity** — for Unity's console, which lists the first two lines of
//!   an entry and shows the rest when it is selected: the heading and the
//!   marked source line first, then, set apart by blank lines, the full
//!   text as the terminal shows it, with Unity's rich text tags
//!   (`<color><b>`). The
//!   lines after the first are indented four spaces so the editor driver
//!   keeps them together as one entry.
//! - **short** — plain text for other tools: one
//!   `file(line,column): error: message` line a console can click on, the
//!   rest indented as above.
//!
//! Nothing is printed while the phases run: the list is complete and
//! sorted before the first line goes out, so threads cannot interleave it
//! and the same sources always produce the same text.

use std::io::IsTerminal;
use std::ops::Range;

use men_sharp_codegen::CodegenError;
use men_sharp_compiler::ParsedFile;
use men_sharp_diagnostics::{
    Catalog, Diagnostic, Edit, Message, Phase, Replacement, language_from_locale,
};
use men_sharp_semantics::{BodyCheck, Declarations, SemanticError, Signatures};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rich,
    Short,
    Unity,
}

impl Format {
    /// Rich on a terminal, short when the output is captured.
    pub fn from_environment() -> Format {
        if std::io::stderr().is_terminal() {
            Format::Rich
        } else {
            Format::Short
        }
    }
}

/// The language to report in: `--lang` (unless it is `auto`), else
/// `MENSHARP_LANG`, else the system's preferred languages — the POSIX
/// locale variables, then what the platform reports (`sys-locale`: the
/// Windows and macOS user settings, which no environment variable
/// carries) — the first with a catalog. English when none has one.
pub fn language_from_environment(explicit: Option<&str>) -> String {
    if let Some(language) = explicit
        && language != "auto"
    {
        return language.to_string();
    }
    let mut candidates: Vec<String> = ["MENSHARP_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .collect();
    candidates.extend(sys_locale::get_locales());
    candidates
        .iter()
        .find_map(|candidate| language_from_locale(candidate))
        .unwrap_or("en")
        .to_string()
}

/// Colour when the output is a terminal and nobody said `NO_COLOR`.
pub fn color_from_environment() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

// -------------------------------------------------------------- gathering

/// The syntax and semantic diagnostics of a compilation, foreign
/// (UdonSharp) files left out — those are UdonSharp's to report.
pub fn collect(
    files: &[ParsedFile],
    declarations: &Declarations,
    signatures: &Signatures,
    bodies: &BodyCheck,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if file.foreign {
            continue;
        }
        let index = index as u32;
        for error in file.ast.errors() {
            let mut diagnostic =
                Diagnostic::new(Phase::Syntax, index, error.span.clone(), error.message());
            diagnostic.hints = error.hints(index, file.ast.source());
            out.push(diagnostic);
        }
    }
    for error in declarations
        .errors
        .iter()
        .chain(&signatures.errors)
        .chain(&bodies.errors)
    {
        if files[error.file.0 as usize].foreign {
            continue;
        }
        out.push(semantic(error));
    }
    out
}

fn semantic(error: &SemanticError) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        Phase::Semantics,
        error.file.0,
        error.span.clone(),
        error.kind.message(),
    );
    diagnostic.heading = error.kind.heading();
    diagnostic.labels = error.kind.labels();
    diagnostic.hints = error.hints.clone();
    diagnostic
}

/// Code generation's diagnostics; foreign files left out as above.
pub fn collect_codegen(files: &[ParsedFile], errors: &[CodegenError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .filter(|error| !files[error.file.0 as usize].foreign)
        .map(|error| {
            Diagnostic::new(
                Phase::Codegen,
                error.file.0,
                error.span.clone(),
                error.message.clone(),
            )
        })
        .collect()
}

// -------------------------------------------------------------- rendering

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
    files: &'a [ParsedFile],
    catalog: Catalog,
    format: Format,
    color: bool,
}

impl<'a> Reporter<'a> {
    pub fn new(files: &'a [ParsedFile], language: &str, format: Format, color: bool) -> Self {
        Reporter {
            files,
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
        men_sharp_diagnostics::sort(diagnostics);
        let mut out = String::new();
        for diagnostic in diagnostics.iter() {
            out.push_str(&self.render_one(diagnostic));
            out.push('\n');
        }
        out
    }

    fn file_name(&self, file: u32) -> &str {
        &self.files[file as usize].name
    }

    fn source(&self, file: u32) -> &str {
        self.files[file as usize].ast.source()
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
    use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};

    /// The .NET corlib the compiler tests use, when a runtime is installed.
    fn corelib() -> Option<Vec<u8>> {
        let roots = [
            std::path::PathBuf::from("/usr/share/dotnet/shared/Microsoft.NETCore.App"),
            std::path::PathBuf::from("/usr/lib/dotnet/shared/Microsoft.NETCore.App"),
            std::env::var_os("HOME")
                .map(|home| {
                    std::path::PathBuf::from(home).join(".dotnet/shared/Microsoft.NETCore.App")
                })
                .unwrap_or_default(),
        ];
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            let mut versions: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
            versions.sort();
            if let Some(version) = versions.last()
                && let Ok(bytes) = std::fs::read(version.join("System.Private.CoreLib.dll"))
            {
                return Some(bytes);
            }
        }
        None
    }

    /// Parses and checks `source` against the corlib, returning the
    /// rendered diagnostics; empty when no .NET runtime is installed.
    fn render(source: &str, language: &str, format: Format) -> String {
        let Some(corelib) = corelib() else {
            eprintln!("no .NET runtime: skipping");
            return String::new();
        };
        let compiler = Compiler::new(CompilerSettings::default()).unwrap();
        let bytes = [corelib];
        let references = compiler.load_references(&bytes).unwrap();
        let files = compiler.parse(vec![SourceCode::new("Assets/Test.cs", source)]);
        let declarations = compiler.collect_declarations(&files);
        let signatures = compiler.resolve_signatures(&declarations, &references);
        let bodies = compiler.check_bodies(&declarations, &signatures, &references);
        let mut diagnostics = collect(&files, &declarations, &signatures, &bodies);
        Reporter::new(&files, language, format, false).render(&mut diagnostics)
    }

    const MISSING_SEMICOLON: &str =
        "public class A\n{\n    void Run()\n    {\n        int x = 1\n    }\n}\n";

    #[test]
    fn a_missing_semicolon_shows_the_line_with_it_added() {
        let text = render(MISSING_SEMICOLON, "en", Format::Rich);
        if text.is_empty() {
            return;
        }
        assert!(
            text.contains("[SyntaxError] a statement ends with `;`\n  --> Assets/Test.cs:6:5\n"),
            "{text}"
        );
        assert!(
            text.contains("[Hint] insert `;`\n   5 │         int x = 1;\n"),
            "{text}"
        );
    }

    #[test]
    fn japanese_uses_the_japanese_catalog() {
        let text = render(MISSING_SEMICOLON, "ja", Format::Rich);
        if text.is_empty() {
            return;
        }
        assert!(
            text.contains("[SyntaxError] 文は `;` で終わります"),
            "{text}"
        );
        assert!(text.contains("[ヒント] `;` を挿入します"), "{text}");
    }

    #[test]
    fn the_short_format_is_one_clickable_line_then_indented_detail() {
        let text = render(MISSING_SEMICOLON, "en", Format::Short);
        if text.is_empty() {
            return;
        }
        assert!(
            text.starts_with(
                "Assets/Test.cs(6,5): error: a statement ends with `;`\n       6 │     }\n"
            ),
            "{text}"
        );
        assert!(
            text.lines()
                .skip(1)
                .filter(|line| !line.is_empty())
                .all(|line| line.starts_with("    ")),
            "{text}"
        );
    }

    #[test]
    fn the_unity_format_marks_the_span_inline() {
        let text = render(
            "public class A\n{\n    void Run()\n    {\n        int a = 1.5 * 2;\n    }\n}\n",
            "en",
            Format::Unity,
        );
        if text.is_empty() {
            return;
        }
        let heading = "<color=#ff6b6b><b>[TypeError]</b></color> expected `int`, found `double`";
        let source_line = "       5 │         int a = <color=#ff6b6b><b>1.5 * 2</b></color>;";
        assert!(
            text.starts_with(&format!(
                "expected `int`, found `double`\n{source_line}\n    \n    {heading}\n      --> Assets/Test.cs:5:17\n{source_line}\n"
            )),
            "{text}"
        );
        assert!(
            text.contains("       5 │         int a = <color=#ff6b6b><b>1.5 * 2</b></color>;\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "    <color=#5ad1e6><b>[Hint]</b></color> convert explicitly with a cast\n       5 │         int a = <color=#5ad1e6><b>(int)(1.5 * 2)</b></color>;\n"
            ),
            "{text}"
        );
        // every line after the first is a continuation of the entry
        assert!(
            text.lines()
                .skip(1)
                .filter(|line| !line.is_empty())
                .all(|line| line.starts_with("    ")),
            "{text}"
        );
        // and the rich form names the kind of error the same way
        let rich = render(
            "public class A\n{\n    void Run()\n    {\n        int a = 1.5 * 2;\n    }\n}\n",
            "en",
            Format::Rich,
        );
        assert!(rich.starts_with("[TypeError] expected"), "{rich}");
    }

    #[test]
    fn the_same_sources_report_the_same_text() {
        let source = "public class A\n{\n    void Run()\n    {\n        int x = 1\n        string s = 2;\n    }\n}\n";
        let first = render(source, "en", Format::Rich);
        let second = render(source, "en", Format::Rich);
        assert_eq!(first, second);
        if first.is_empty() {
            return;
        }
        // and in position order: the syntax error on line 5 before the
        // type error on line 6
        let syntax = first.find("[SyntaxError]").unwrap();
        let semantic = first.find("[TypeError]").unwrap();
        assert!(syntax < semantic, "{first}");
    }

    /// Every `Message::key("...")` written in the compiler's sources has an
    /// English entry — a key without one would print as itself.
    #[test]
    fn every_message_key_in_the_sources_is_in_the_catalog() {
        let catalog = men_sharp_diagnostics::Catalog::for_language("en");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut missing = Vec::new();
        let mut stack = vec![root];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name == "target") {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                if path.extension().is_some_and(|extension| extension == "rs")
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    for (index, _) in text.match_indices("Message::key(\"") {
                        let rest = &text[index + "Message::key(\"".len()..];
                        let Some(end) = rest.find('"') else {
                            continue;
                        };
                        let key = &rest[..end];
                        // a real key is `section.name`; anything else is
                        // an example in a comment or a test
                        let well_formed = key.split_once('.').is_some_and(|(section, name)| {
                            !section.is_empty()
                                && !name.is_empty()
                                && key.chars().all(|c| {
                                    c.is_ascii_lowercase()
                                        || c.is_ascii_digit()
                                        || c == '_'
                                        || c == '.'
                                })
                        });
                        if !well_formed {
                            continue;
                        }
                        if catalog.text(key).is_none() {
                            missing.push(format!("{}: {key}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "keys without a catalog entry: {missing:#?}"
        );
    }

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
