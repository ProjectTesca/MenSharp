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

/// The compiler is allocation heavy in short bursts (codegen strings, per-file
/// tables) across many threads. mimalloc handles that noticeably better than
/// the system allocator, and being statically linked it adds no runtime
/// dependency to the shipped binary.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod report;

use men_sharp_diagnostics::{Format, Reporter};
use men_sharp_semantics::{
    Declarations, MemberSignature, Signatures, SymbolId, SymbolKind, Type, TypeTarget,
};
use report::Files;

const USAGE: &str = "usage: men-sharp [--threads N] [--reference lib.dll]... \
[--udonsharp other.cs]... [--define NAME]... [--profile-dir dir] \
[--emit-udon Namespace.EntryClass --out name | --emit-udon-all --out-dir dir] \
[--lang auto|en|ja] [--error-format rich|short|unity] <file.cs>...";

fn main() -> ExitCode {
    // `--profile-dir` is read ahead of everything so that every phase,
    // including reading the inputs, is on the profile
    let profile_dir = profile_dir_argument();
    timescope::enable_profile(profile_dir.is_some());
    timescope::register_thread_name("main");
    let code = run();
    if let Some(dir) = profile_dir {
        match timescope::dump_to_dir(&dir) {
            Ok(()) => println!("profile written to {dir}/summary.txt and report.html"),
            Err(error) => eprintln!("{dir}: could not write the profile: {error}"),
        }
    }
    code
}

/// The directory `--profile-dir` names, when it is given.
fn profile_dir_argument() -> Option<String> {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--profile-dir" {
            return arguments.next();
        }
    }
    None
}

fn run() -> ExitCode {
    timescope::scope!("men-sharp");
    let mut thread_count = None;
    let mut paths = Vec::new();
    let mut foreign_paths = Vec::new();
    let mut defines = Vec::new();
    let mut reference_paths = Vec::new();
    let mut udon_entry: Option<String> = None;
    let mut udon_all = false;
    let mut out_name = "program".to_string();
    let mut out_dir = ".".to_string();
    let mut language: Option<String> = None;
    let mut format = report::format_from_environment();

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
            // an UdonSharp script: read for the declarations of its
            // behaviours, so M# code can talk to them by name — never
            // compiled, and its errors are not reported
            "--udonsharp" => {
                let Some(path) = arguments.next() else {
                    eprintln!("--udonsharp needs a .cs path");
                    return ExitCode::FAILURE;
                };
                foreign_paths.push(path);
            }
            "--define" | "-d" => {
                let Some(name) = arguments.next() else {
                    eprintln!("--define needs a symbol name");
                    return ExitCode::FAILURE;
                };
                defines.push(name);
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
            // writes timescope's summary.txt and report.html there at exit
            "--profile-dir" => {
                if arguments.next().is_none() {
                    eprintln!("--profile-dir needs a directory");
                    return ExitCode::FAILURE;
                }
            }
            "--lang" => {
                let Some(code) = arguments.next() else {
                    eprintln!("--lang needs a language code (auto, en, ja)");
                    return ExitCode::FAILURE;
                };
                language = Some(code);
            }
            "--error-format" => match arguments.next().as_deref() {
                Some("rich") => format = Format::Rich,
                Some("short") => format = Format::Short,
                Some("unity") => format = Format::Unity,
                _ => {
                    eprintln!("--error-format needs `rich`, `short` or `unity`");
                    return ExitCode::FAILURE;
                }
            },
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

    let mut sources = Vec::with_capacity(paths.len() + foreign_paths.len());
    {
        timescope::scope!("read sources");
        for path in &paths {
            match std::fs::read_to_string(path) {
                Ok(text) => sources.push(SourceCode::new(path.as_str(), text)),
                Err(error) => {
                    eprintln!("{path}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        for path in &foreign_paths {
            match std::fs::read_to_string(path) {
                Ok(text) => sources.push(SourceCode::foreign(path.as_str(), text)),
                Err(error) => {
                    eprintln!("{path}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    let mut reference_bytes = Vec::with_capacity(reference_paths.len());
    {
        timescope::scope!("read references");
        for path in &reference_paths {
            match std::fs::read(path) {
                Ok(bytes) => reference_bytes.push(bytes),
                Err(error) => {
                    eprintln!("{path}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let compiler = match Compiler::new(CompilerSettings {
        thread_count,
        defines,
    }) {
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
        timescope::scope!("corlib sources");
        sources.extend(Compiler::corlib_sources_for(&references));
    }

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let language = report::language_from_environment(language.as_deref());
    let sources = Files(&files);
    let reporter = Reporter::new(
        &sources,
        &language,
        format,
        report::color_from_environment(),
    );
    let mut diagnostics = {
        timescope::scope!("report");
        report::collect(&files, &declarations, &signatures, &bodies)
    };
    let error_count = diagnostics.len();
    if error_count > 0 {
        eprint!("{}", reporter.render(&mut diagnostics));
    }
    println!("checked {} expressions", bodies.expression_types.len());

    if udon_all {
        if error_count > 0 {
            eprintln!(
                "{}",
                reporter.count_line("ui.errors_not_emitting", error_count)
            );
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
        timescope::scope!("emit programs");
        // every program's diagnostics are gathered before any is printed:
        // one sorted list, whatever order the programs were generated in
        let mut codegen_diagnostics = Vec::new();
        for program in &programs {
            codegen_diagnostics.extend(report::collect_codegen(&files, &program.output.errors));
        }
        if !codegen_diagnostics.is_empty() {
            eprint!("{}", reporter.render(&mut codegen_diagnostics));
        }
        let written = compiler.write_udon_behaviours(&programs, std::path::Path::new(&out_dir));
        for (program, written) in programs.iter().zip(written) {
            match written {
                Some(Ok(uasm_path)) => println!("wrote {}", uasm_path.display()),
                Some(Err(men_sharp_compiler::EmitError::Assemble(error))) => {
                    eprintln!("{}: assembly error: {error:?}", program.class_path);
                    failed += 1;
                }
                Some(Err(men_sharp_compiler::EmitError::Write(path, error))) => {
                    eprintln!("{}: {error}", path.display());
                    failed += 1;
                }
                // codegen errors, reported above
                None => failed += 1,
            }
        }
        if failed > 0 {
            eprintln!("{}", reporter.count_line("ui.programs_failed", failed));
            return ExitCode::FAILURE;
        }
        // The process is about to exit: freeing 43 programs, the checked
        // compilation and the reference metadata one allocation at a time
        // is pure cost (a few ms, on the main thread, after the last useful
        // byte was written). The OS reclaims it all at once.
        std::mem::forget(programs);
        std::mem::forget(bodies);
        std::mem::forget(signatures);
        std::mem::forget(declarations);
        std::mem::forget(files);
        std::mem::forget(references);
        std::mem::forget(reference_bytes);
        return ExitCode::SUCCESS;
    }

    if let Some(entry) = &udon_entry {
        if error_count > 0 {
            eprintln!(
                "{}",
                reporter.count_line("ui.errors_not_emitting", error_count)
            );
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
        let mut codegen_diagnostics = report::collect_codegen(&files, &output.errors);
        if !codegen_diagnostics.is_empty() {
            let count = codegen_diagnostics.len();
            eprint!("{}", reporter.render(&mut codegen_diagnostics));
            eprintln!("{}", reporter.count_line("ui.error_count", count));
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
        let blob = output.program.to_blob().expect("assembled above");
        let uasm_path = format!("{out_name}.uasm");
        let meta_path = format!("{out_name}.meta.json");
        let blob_path = format!("{out_name}.uprog");
        if let Err(error) = std::fs::write(&uasm_path, uasm) {
            eprintln!("{uasm_path}: {error}");
            return ExitCode::FAILURE;
        }
        if let Err(error) = std::fs::write(&meta_path, meta) {
            eprintln!("{meta_path}: {error}");
            return ExitCode::FAILURE;
        }
        if let Err(error) = std::fs::write(&blob_path, blob) {
            eprintln!("{blob_path}: {error}");
            return ExitCode::FAILURE;
        }
        println!("wrote {uasm_path}, {meta_path} and {blob_path}");
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
        eprintln!("{}", reporter.count_line("ui.error_count", error_count));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
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
