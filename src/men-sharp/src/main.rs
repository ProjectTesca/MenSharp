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

const USAGE: &str = "usage: men-sharp [--threads N] [--reference lib.dll]... \
[--emit-udon Namespace.EntryClass --out name | --emit-udon-all --out-dir dir] <file.cs>...";

fn main() -> ExitCode {
    let mut thread_count = None;
    let mut paths = Vec::new();
    let mut reference_paths = Vec::new();
    let mut udon_entry: Option<String> = None;
    let mut udon_all = false;
    let mut out_name = "program".to_string();
    let mut out_dir = ".".to_string();

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
            "--emit-udon" => {
                let Some(entry) = arguments.next() else {
                    eprintln!("--emit-udon needs an entry class (Namespace.Class)");
                    return ExitCode::FAILURE;
                };
                udon_entry = Some(entry);
            }
            "--emit-udon-all" => udon_all = true,
            "--out" | "-o" => {
                let Some(name) = arguments.next() else {
                    eprintln!("--out needs a name");
                    return ExitCode::FAILURE;
                };
                out_name = name;
            }
            "--out-dir" => {
                let Some(dir) = arguments.next() else {
                    eprintln!("--out-dir needs a directory");
                    return ExitCode::FAILURE;
                };
                out_dir = dir;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            path => paths.push(path.to_string()),
        }
    }

    if paths.is_empty() {
        eprintln!("{USAGE}");
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

    if udon_entry.is_some() || udon_all {
        // the Udon target compiles the mini-corlib along with user code; which
        // flavour depends on the references, so this waits for them
        sources.extend(Compiler::corlib_sources_for(&references));
    }

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let error_count = report(&files, &declarations, &signatures, &bodies);
    println!("checked {} expressions", bodies.expression_types.len());

    if udon_all {
        if error_count > 0 {
            eprintln!("{error_count} error(s); not emitting Udon assembly");
            return ExitCode::FAILURE;
        }
        let programs = compiler.generate_udon_behaviours(
            &declarations,
            &signatures,
            &bodies,
            &references,
            &files,
        );
        // no behaviours is a normal state, not a failure: sources without one
        // yet, or the last one just deleted. Failing here would stop the Unity
        // driver before it cleans up the programs that are now stale.
        if programs.is_empty() {
            println!("no MenSharpBehaviour subclasses found — nothing to emit");
            return ExitCode::SUCCESS;
        }
        let mut failed = 0usize;
        for program in &programs {
            for error in &program.output.errors {
                let file = &files[error.file.0 as usize];
                let (line, column) = line_column(file.ast.source(), error.span.start);
                eprintln!("{}:{line}:{column}: error: {}", file.name, error.message);
            }
            if !program.output.errors.is_empty() {
                failed += 1;
                continue;
            }
            let uasm = match program.output.program.to_uasm() {
                Ok(uasm) => uasm,
                Err(error) => {
                    eprintln!("{}: assembly error: {error:?}", program.class_path);
                    failed += 1;
                    continue;
                }
            };
            let meta = program
                .output
                .program
                .to_meta_json()
                .expect("assembled above");
            // the class path contains dots, so extensions are appended, not
            // swapped in (`Game.Door` must not become `Game.uasm`)
            let directory = std::path::Path::new(&out_dir);
            let uasm_path = directory.join(format!("{}.uasm", program.class_path));
            let meta_path = directory.join(format!("{}.meta.json", program.class_path));
            if let Err(error) =
                std::fs::write(&uasm_path, uasm).and_then(|()| std::fs::write(&meta_path, meta))
            {
                eprintln!("{}: {error}", uasm_path.display());
                failed += 1;
                continue;
            }
            println!("wrote {}", uasm_path.display());
        }
        if failed > 0 {
            eprintln!("{failed} program(s) failed");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    if let Some(entry) = &udon_entry {
        if error_count > 0 {
            eprintln!("{error_count} error(s); not emitting Udon assembly");
            return ExitCode::FAILURE;
        }
        let entry_path: Vec<&str> = entry.split('.').collect();
        let output = compiler.generate_udon(
            &declarations,
            &signatures,
            &bodies,
            &references,
            &entry_path,
        );
        for error in &output.errors {
            let file = &files[error.file.0 as usize];
            let (line, column) = line_column(file.ast.source(), error.span.start);
            eprintln!("{}:{line}:{column}: error: {}", file.name, error.message);
        }
        if !output.errors.is_empty() {
            eprintln!("{} error(s)", output.errors.len());
            return ExitCode::FAILURE;
        }
        let uasm = match output.program.to_uasm() {
            Ok(uasm) => uasm,
            Err(error) => {
                eprintln!("assembly error: {error:?}");
                return ExitCode::FAILURE;
            }
        };
        let meta = output.program.to_meta_json().expect("assembled above");
        let uasm_path = format!("{out_name}.uasm");
        let meta_path = format!("{out_name}.meta.json");
        if let Err(error) = std::fs::write(&uasm_path, uasm) {
            eprintln!("{uasm_path}: {error}");
            return ExitCode::FAILURE;
        }
        if let Err(error) = std::fs::write(&meta_path, meta) {
            eprintln!("{meta_path}: {error}");
            return ExitCode::FAILURE;
        }
        println!("wrote {uasm_path} and {meta_path}");
        return ExitCode::SUCCESS;
    }

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
