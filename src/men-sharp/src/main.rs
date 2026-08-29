//! The `men-sharp` command line.
//!
//! For now this is a thin driver over `men-sharp-compiler`: it parses the given
//! files, collects declarations, and prints the diagnostics and the symbol tree.
//! It exists so the pipeline can be exercised end to end from a shell; real
//! compilation output comes later.
//!
//! ```text
//! men-sharp [--threads N] <file.cs>...
//! ```

use std::process::ExitCode;

use men_sharp_compiler::{Compiler, CompilerSettings, ParsedFile, SourceCode};
use men_sharp_semantics::{Declarations, SymbolId, SymbolKind};

fn main() -> ExitCode {
    let mut thread_count = None;
    let mut paths = Vec::new();

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--threads" => {
                let Some(count) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("--threads needs a number");
                    return ExitCode::FAILURE;
                };
                thread_count = Some(count);
            }
            "--help" | "-h" => {
                println!("usage: men-sharp [--threads N] <file.cs>...");
                return ExitCode::SUCCESS;
            }
            path => paths.push(path.to_string()),
        }
    }

    if paths.is_empty() {
        eprintln!("usage: men-sharp [--threads N] <file.cs>...");
        return ExitCode::FAILURE;
    }

    let mut sources = Vec::with_capacity(paths.len());
    for path in &paths {
        match std::fs::read_to_string(path) {
            Ok(text) => sources.push(SourceCode::new(path.as_str(), text)),
            Err(error) => {
                eprintln!("{path}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

    let compiler = match Compiler::new(CompilerSettings { thread_count }) {
        Ok(compiler) => compiler,
        Err(error) => {
            eprintln!("failed to build the thread pool: {error}");
            return ExitCode::FAILURE;
        }
    };

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);

    let error_count = report(&files, &declarations);

    println!("symbols ({} threads):", compiler.thread_count());
    print_symbol(&declarations, &files, declarations.table.root(), 0);

    if error_count > 0 {
        eprintln!("{error_count} error(s)");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Prints every parse and semantic error as `file:line:column: message`.
fn report(files: &[ParsedFile], declarations: &Declarations) -> usize {
    let mut count = 0;

    for file in files {
        for error in file.ast.errors() {
            let (line, column) = line_column(file.ast.source(), error.span.start);
            eprintln!(
                "{}:{line}:{column}: syntax error: {:?}",
                file.name, error.kind
            );
            count += 1;
        }
    }

    for error in &declarations.errors {
        let file = &files[error.file.0 as usize];
        let (line, column) = line_column(file.ast.source(), error.span.start);
        eprintln!("{}:{line}:{column}: error: {:?}", file.name, error.kind);
        count += 1;
    }

    count
}

fn line_column(source: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(source.len());
    let before = &source[..byte];
    let line = before.matches('\n').count() + 1;
    let column = before.chars().rev().take_while(|c| *c != '\n').count() + 1;
    (line, column)
}

fn print_symbol(declarations: &Declarations, files: &[ParsedFile], id: SymbolId, depth: usize) {
    let symbol = declarations.table.symbol(id);

    if symbol.parent.is_some() {
        let indent = "  ".repeat(depth);
        let arity = match symbol.arity {
            0 => String::new(),
            arity => format!("`{arity}"),
        };
        let sites = symbol
            .declarations
            .iter()
            .map(|site| files[site.file.0 as usize].name.as_ref())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{indent}{:?} {}{arity}  [{sites}]",
            symbol.kind, symbol.name
        );
    }

    // member bodies are not interesting at declaration level; stop at member kinds
    if symbol.parent.is_some() && !(symbol.kind.is_type() || symbol.kind == SymbolKind::Namespace) {
        return;
    }

    let depth = if symbol.parent.is_some() {
        depth + 1
    } else {
        depth
    };
    for &member in &symbol.members {
        print_symbol(declarations, files, member, depth);
    }
}
