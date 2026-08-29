//! The `men-sharp` command line.
//!
//! For now this is a thin driver over `men-sharp-compiler`: it parses the given
//! files, collects declarations, resolves signatures against the referenced dlls,
//! and prints the diagnostics and the symbol tree with resolved types. It exists so
//! the pipeline can be exercised end to end from a shell; real compilation output
//! comes later.
//!
//! ```text
//! men-sharp [--threads N] [--reference lib.dll]... <file.cs>...
//! ```

use std::process::ExitCode;

use men_sharp_compiler::{Compiler, CompilerSettings, ParsedFile, ReferenceSet, SourceCode};
use men_sharp_semantics::{
    BodyCheck, Declarations, MemberSignature, Signatures, SymbolId, SymbolKind, Type, TypeTarget,
};

fn main() -> ExitCode {
    let mut thread_count = None;
    let mut paths = Vec::new();
    let mut reference_paths = Vec::new();

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
            "--reference" | "-r" => {
                let Some(path) = arguments.next() else {
                    eprintln!("--reference needs a dll path");
                    return ExitCode::FAILURE;
                };
                reference_paths.push(path);
            }
            "--help" | "-h" => {
                println!("usage: men-sharp [--threads N] [--reference lib.dll]... <file.cs>...");
                return ExitCode::SUCCESS;
            }
            path => paths.push(path.to_string()),
        }
    }

    if paths.is_empty() {
        eprintln!("usage: men-sharp [--threads N] [--reference lib.dll]... <file.cs>...");
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

    let mut reference_bytes = Vec::with_capacity(reference_paths.len());
    for path in &reference_paths {
        match std::fs::read(path) {
            Ok(bytes) => reference_bytes.push(bytes),
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

    let references = match compiler.load_references(&reference_bytes) {
        Ok(references) => references,
        Err(error) => {
            eprintln!("{}: {}", reference_paths[error.reference], error.error);
            return ExitCode::FAILURE;
        }
    };

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let error_count = report(&files, &declarations, &signatures, &bodies);
    println!("checked {} expressions", bodies.expression_types.len());

    println!("symbols ({} threads):", compiler.thread_count());
    let printer = Printer {
        declarations: &declarations,
        signatures: &signatures,
        references: &references,
        files: &files,
    };
    printer.print(declarations.table.root(), 0);

    if error_count > 0 {
        eprintln!("{error_count} error(s)");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Prints every parse and semantic error as `file:line:column: message`.
fn report(
    files: &[ParsedFile],
    declarations: &Declarations,
    signatures: &Signatures,
    bodies: &BodyCheck,
) -> usize {
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

    for error in declarations
        .errors
        .iter()
        .chain(&signatures.errors)
        .chain(&bodies.errors)
    {
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

struct Printer<'a> {
    declarations: &'a Declarations<'a>,
    signatures: &'a Signatures,
    references: &'a ReferenceSet<'a>,
    files: &'a [ParsedFile],
}

impl Printer<'_> {
    fn print(&self, id: SymbolId, depth: usize) {
        let symbol = self.declarations.table.symbol(id);

        if symbol.parent.is_some() {
            let indent = "  ".repeat(depth);
            let arity = match symbol.arity {
                0 => String::new(),
                arity => format!("`{arity}"),
            };
            let signature = self
                .signatures
                .members
                .get(&id)
                .map(|member| format!("  : {}", self.member(member)))
                .unwrap_or_default();
            let bases = self
                .signatures
                .base_types
                .get(&id)
                .filter(|bases| !bases.is_empty())
                .map(|bases| {
                    format!(
                        "  : {}",
                        bases
                            .iter()
                            .map(|base| self.type_name(base))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .unwrap_or_default();
            let sites = symbol
                .declarations
                .iter()
                .map(|site| self.files[site.file.0 as usize].name.as_ref())
                .collect::<Vec<_>>()
                .join(", ");

            println!(
                "{indent}{:?} {}{arity}{bases}{signature}  [{sites}]",
                symbol.kind, symbol.name
            );
        }

        if symbol.parent.is_some()
            && !(symbol.kind.is_type() || symbol.kind == SymbolKind::Namespace)
        {
            return;
        }

        let depth = if symbol.parent.is_some() {
            depth + 1
        } else {
            depth
        };
        for &member in &symbol.members {
            self.print(member, depth);
        }
    }

    fn member(&self, member: &MemberSignature) -> String {
        match member {
            MemberSignature::Field(field_type) => self.type_name(field_type),
            MemberSignature::Property(property_type) => self.type_name(property_type),
            MemberSignature::Event(event_type) => self.type_name(event_type),
            MemberSignature::Function(function) => {
                let parameters = function
                    .parameters
                    .iter()
                    .map(|parameter| self.type_name(&parameter.parameter_type))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "({parameters}) -> {}",
                    self.type_name(&function.return_type)
                )
            }
        }
    }

    fn type_name(&self, resolved: &Type) -> String {
        match resolved {
            Type::Named { target, arguments } => {
                let name = match target {
                    TypeTarget::Source(symbol) => {
                        self.declarations.table.fully_qualified_name(*symbol)
                    }
                    TypeTarget::External(id) => self.references.display_name(*id),
                };
                if arguments.is_empty() {
                    name
                } else {
                    format!(
                        "{name}<{}>",
                        arguments
                            .iter()
                            .map(|argument| self.type_name(argument))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Type::TypeParameter(symbol) => self.declarations.table.symbol(*symbol).name.to_string(),
            Type::ExternalTypeParameter { index, .. } => format!("!{index}"),
            Type::ExternalMethodTypeParameter(index) => format!("!!{index}"),
            Type::Array { element, rank } => {
                format!(
                    "{}[{}]",
                    self.type_name(element),
                    ",".repeat(*rank as usize - 1)
                )
            }
            Type::Pointer(element) => format!("{}*", self.type_name(element)),
            Type::Nullable(element) => format!("{}?", self.type_name(element)),
            Type::ByRef { readonly, element } => format!(
                "ref {}{}",
                if *readonly { "readonly " } else { "" },
                self.type_name(element)
            ),
            Type::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| match &element.name {
                        Some(name) => format!("{} {name}", self.type_name(&element.element)),
                        None => self.type_name(&element.element),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Dynamic => "dynamic".to_string(),
            Type::Void => "void".to_string(),
            Type::Null => "null".to_string(),
            Type::Infer => "var".to_string(),
            Type::Error => "<error>".to_string(),
        }
    }
}
