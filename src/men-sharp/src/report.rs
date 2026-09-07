//! What the compiler found, gathered for the reporter — and the answers
//! the environment gives.
//!
//! Every phase leaves language-neutral diagnostics (see the
//! `men-sharp-diagnostics` crate); this module collects them into one
//! list, tells the diagnostics crate's `Reporter` where the files are
//! ([`Files`]), and decides
//! from the environment what the reporter is not allowed to decide for
//! itself: the language, the format and whether to colour. The rendering
//! is the diagnostics crate's.
//!
//! Nothing is printed while the phases run: the list is complete and
//! sorted before the first line goes out, so threads cannot interleave it
//! and the same sources always produce the same text.

use std::io::IsTerminal;

use men_sharp_codegen::CodegenError;
use men_sharp_compiler::ParsedFile;
use men_sharp_diagnostics::{Diagnostic, Format, Phase, Sources, language_from_locale};
use men_sharp_semantics::{BodyCheck, Declarations, SemanticError, Signatures};

/// Rich on a terminal, short when the output is captured.
pub fn format_from_environment() -> Format {
    if std::io::stderr().is_terminal() {
        Format::Rich
    } else {
        Format::Short
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

/// The compilation's files, as the reporter asks about them: by the index
/// a diagnostic carries.
pub struct Files<'a>(pub &'a [ParsedFile]);

impl Sources for Files<'_> {
    fn name(&self, file: u32) -> &str {
        &self.0[file as usize].name
    }

    fn source(&self, file: u32) -> &str {
        self.0[file as usize].ast.source()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};
    use men_sharp_diagnostics::Reporter;

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
        Reporter::new(&Files(&files), language, format, false).render(&mut diagnostics)
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
}
