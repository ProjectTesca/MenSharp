//! The MenSharp compiler driver.
//!
//! This crate owns orchestration and nothing else: which files are compiled, in what
//! order the phases run, and on how many threads. The phases themselves live in
//! `men-sharp-parser` and `men-sharp-semantics` as synchronous functions that know
//! nothing about threads — parallelism is applied here, from the outside.
//!
//! The pipeline so far:
//!
//! 1. **Parse** — one task per file, embarrassingly parallel ([`Compiler::parse`]).
//! 2. **Collect declarations** — one task per file, then a sequential merge into one
//!    symbol table ([`Compiler::collect_declarations`]). The barrier is forced by C#
//!    itself: namespaces are open across files and `partial` types span files.
//!
//! Later phases (signature resolution, then per-method-body type checking) slot in
//! after the merge; method bodies are again embarrassingly parallel.
//!
//! Threads come from a dedicated [`rayon`] pool sized by [`CompilerSettings`], not
//! from rayon's implicit global pool, so two [`Compiler`]s with different settings
//! never fight over configuration.
//!
//! ```
//! use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};
//!
//! let compiler = Compiler::new(CompilerSettings { thread_count: Some(2), ..Default::default() }).unwrap();
//! let files = compiler.parse(vec![SourceCode::new("main.ms", "class Program {}")]);
//! let declarations = compiler.collect_declarations(&files);
//! assert!(declarations.errors.is_empty());
//! ```

use std::sync::Arc;

use men_sharp_asm::AssembleError;
use men_sharp_dotnet::DotNetAssembly;
use men_sharp_parser::MenSharpAST;
use men_sharp_semantics::{
    BodyCheck, Declarations, ExternalTypes, FileId, Signatures, check_file, collect_file,
    merge_declarations, resolve_file,
};
use rayon::prelude::*;

pub mod references;

pub use references::{ReferenceError, ReferenceSet};

/// Settings the driver is constructed with. Kept as a plain struct so growing it
/// (optimization level, language version, ...) never changes any signature.
#[derive(Debug, Clone, Default)]
pub struct CompilerSettings {
    /// Worker threads for the parallel phases. `None` means one per available core.
    pub thread_count: Option<usize>,
    /// Conditional compilation symbols (`#if NAME`), on top of the ones the
    /// compiler always defines: `COMPILER_MENSHARP` for every file, and
    /// `COMPILER_UDONSHARP` for foreign (UdonSharp) files, which were written
    /// for that compiler's eyes.
    pub defines: Vec<String>,
}

/// One input file: a display name (usually the path) and its text.
#[derive(Debug, Clone)]
pub struct SourceCode {
    pub name: Arc<str>,
    pub text: Arc<str>,
    /// A foreign source: an UdonSharp script, read for the declarations of
    /// its behaviours (what M# code can reach by name) and nothing more —
    /// its bodies are UdonSharp's to compile, and so are its errors.
    pub foreign: bool,
}

impl SourceCode {
    pub fn new(name: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
            foreign: false,
        }
    }

    /// An UdonSharp source: declarations only. See [`SourceCode::foreign`].
    pub fn foreign(name: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
            foreign: true,
        }
    }
}

/// A parsed file. Its [`FileId`] is its index in the vec [`Compiler::parse`] returned.
#[derive(Debug)]
pub struct ParsedFile {
    pub name: Arc<str>,
    pub ast: MenSharpAST,
    /// See [`SourceCode::foreign`].
    pub foreign: bool,
}

pub struct Compiler {
    settings: CompilerSettings,
    pool: rayon::ThreadPool,
}

impl Compiler {
    pub fn new(settings: CompilerSettings) -> Result<Self, rayon::ThreadPoolBuildError> {
        let mut builder = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("men-sharp-{index}"))
            // so a profile names the worker a span ran on
            .start_handler(|index| timescope::register_thread_name(format!("men-sharp-{index}")));

        if let Some(count) = settings.thread_count {
            builder = builder.num_threads(count);
        }

        let pool = builder.build()?;
        // The Udon whitelist is parsed on first use, which is inside codegen. With
        // programs generated in parallel that parse would sit on the critical path
        // of whichever program asked first, so it is warmed while the front end runs.
        pool.spawn(|| {
            men_sharp_codegen::UdonNodes::for_unity_version(None);
        });

        Ok(Self { settings, pool })
    }

    pub fn settings(&self) -> &CompilerSettings {
        &self.settings
    }

    /// The number of worker threads actually running.
    pub fn thread_count(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// Parses every file in parallel. Order (and therefore each file's [`FileId`])
    /// matches the input.
    pub fn parse(&self, sources: Vec<SourceCode>) -> Vec<ParsedFile> {
        timescope::scope!("parse");
        self.pool.install(|| {
            sources
                .into_par_iter()
                .map(|source| {
                    timescope::scope!("parse file");
                    let mut defines: Vec<&str> =
                        self.settings.defines.iter().map(String::as_str).collect();
                    defines.push("COMPILER_MENSHARP");
                    if source.foreign {
                        defines.push("COMPILER_UDONSHARP");
                    }
                    ParsedFile {
                        name: source.name,
                        ast: MenSharpAST::parse_with_defines(source.text, &defines),
                        foreign: source.foreign,
                    }
                })
                .collect()
        })
    }

    /// Collects declarations from every file in parallel, then merges them into one
    /// symbol table. The result borrows the parsed files, which is what ties the
    /// symbol table's lifetime to the syntax trees it points into.
    pub fn collect_declarations<'ast>(&self, files: &'ast [ParsedFile]) -> Declarations<'ast> {
        timescope::scope!("collect declarations");
        let collected: Vec<_> = self.pool.install(|| {
            files
                .par_iter()
                .enumerate()
                .map(|(index, file)| {
                    timescope::scope!("collect file");
                    let mut declarations = collect_file(FileId(index as u32), file.ast.ast());
                    declarations.foreign = file.foreign;
                    declarations
                })
                .collect()
        });

        let mut declarations = merge_declarations(collected);
        // a foreign file's syntax errors are not reported, but they make the
        // declarations around them uncompilable — kept for `check_bodies`
        declarations.foreign_syntax_errors = files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.foreign)
            .flat_map(|(index, file)| {
                file.ast.errors().iter().map(move |error| {
                    (
                        FileId(index as u32),
                        error.span.clone(),
                        format!("syntax error: {:?}", error.kind),
                    )
                })
            })
            .collect();
        declarations.sources = files
            .iter()
            .map(|file| men_sharp_semantics::SourceText {
                name: file.name.clone(),
                text: std::sync::Arc::from(file.ast.source()),
            })
            .collect();
        declarations.foreign_files = files.iter().map(|file| file.foreign).collect();
        declarations
    }

    /// Parses every referenced dll in parallel and indexes them as an external
    /// type provider. The byte buffers stay with the caller, which is what ties
    /// the provider's lifetime to them.
    pub fn load_references<'data>(
        &self,
        references: &'data [Vec<u8>],
    ) -> Result<ReferenceSet<'data>, ReferenceError> {
        timescope::scope!("load references");
        let assemblies: Vec<Result<DotNetAssembly, ReferenceError>> = self.pool.install(|| {
            references
                .par_iter()
                .enumerate()
                .map(|(index, bytes)| {
                    timescope::scope!("parse reference");
                    DotNetAssembly::parse(bytes).map_err(|error| ReferenceError {
                        reference: index,
                        error,
                    })
                })
                .collect()
        });

        Ok(ReferenceSet::new(
            assemblies.into_iter().collect::<Result<Vec<_>, _>>()?,
        ))
    }

    /// Resolves every declaration-level type reference: one task per file, results
    /// merged and sorted so the outcome does not depend on the thread count.
    pub fn resolve_signatures(
        &self,
        declarations: &Declarations<'_>,
        external: &(dyn ExternalTypes + Sync),
    ) -> Signatures {
        timescope::scope!("resolve signatures");
        let per_file: Vec<Signatures> = self.pool.install(|| {
            (0..declarations.files.len())
                .into_par_iter()
                .map(|index| {
                    timescope::scope!("resolve file");
                    resolve_file(declarations, external, index)
                })
                .collect()
        });

        timescope::scope!("merge signatures");
        let mut all = Signatures::default();
        for signatures in per_file {
            all.merge(signatures);
        }
        all.errors
            .sort_by_key(|error| (error.file, error.span.start, error.span.end));
        all
    }

    /// Type-checks every member body: one task per file, results merged and sorted.
    /// This is the phase whose output (a type for every expression) code generation
    /// will consume.
    pub fn check_bodies(
        &self,
        declarations: &Declarations<'_>,
        signatures: &Signatures,
        external: &(dyn ExternalTypes + Sync),
    ) -> BodyCheck {
        timescope::scope!("check bodies");
        let per_file: Vec<BodyCheck> = self.pool.install(|| {
            (0..declarations.files.len())
                .into_par_iter()
                .map(|index| {
                    timescope::scope!("check file");
                    check_file(declarations, signatures, external, index)
                })
                .collect()
        });

        timescope::scope!("merge bodies");
        let mut all = BodyCheck::default();
        for check in per_file {
            all.merge(check);
        }
        all.errors
            .sort_by_key(|error| (error.file, error.span.start, error.span.end));
        // a foreign file is a library: its errors are not the user's to fix
        // and are not reported, but whatever they sit in cannot be compiled
        // — used from M# code, that is reported there, with the reason
        all.uncompilable =
            men_sharp_semantics::uncompilable_foreign_members(declarations, signatures, &all);
        all.errors
            .retain(|error| !declarations.is_foreign(error.file));
        all
    }

    /// The mini-corlib sources shipped inside the compiler: source ports of
    /// BCL types Udon does not whitelist (`List<T>`, ...). For the Udon
    /// target, append these to the user's sources before [`Compiler::parse`];
    /// they compile, monomorphize and tree-shake like any other code.
    ///
    /// This is the Unity-free set: the behaviour base class declares no
    /// members. Callers that reference Unity want [`Compiler::corlib_sources_for`].
    pub fn corlib_sources() -> Vec<SourceCode> {
        Self::corlib_sources_with_unity(false)
    }

    /// The mini-corlib, picked to match what the references actually offer.
    /// When UnityEngine is among them the behaviour base class gains
    /// `gameObject`/`transform`; without it those members would not resolve,
    /// so the Unity-free base class is used instead.
    pub fn corlib_sources_for(external: &dyn ExternalTypes) -> Vec<SourceCode> {
        let unity = external
            .find_type(&["UnityEngine"], "GameObject", 0)
            .is_some()
            && external
                .find_type(&["UnityEngine"], "Transform", 0)
                .is_some();
        // the std's JSON and Http lean on the VRChat SDK: VRCJson and the
        // string downloader. Without the SDK among the references they are
        // left out, and a mention of them is an ordinary unknown name
        let sdk = external
            .find_type(&["VRC", "SDK3", "Data"], "VRCJson", 0)
            .is_some()
            && external
                .find_type(&["VRC", "SDK3", "StringLoading"], "VRCStringDownloader", 0)
                .is_some();
        Self::corlib_sources_with(unity, sdk)
    }

    fn corlib_sources_with_unity(unity: bool) -> Vec<SourceCode> {
        Self::corlib_sources_with(unity, false)
    }

    fn corlib_sources_with(unity: bool, sdk: bool) -> Vec<SourceCode> {
        // exactly one of the two behaviour base classes: they declare the
        // same type, so compiling both would be a duplicate definition
        let behaviour = if unity {
            SourceCode::new(
                "corlib/MenSharpBehaviour.Unity.cs",
                include_str!("../../../corlib/MenSharpBehaviour.Unity.cs"),
            )
        } else {
            SourceCode::new(
                "corlib/MenSharpBehaviour.cs",
                include_str!("../../../corlib/MenSharpBehaviour.cs"),
            )
        };
        let mut sources = vec![
            SourceCode::new(
                "corlib/Exception.cs",
                include_str!("../../../corlib/Exception.cs"),
            ),
            SourceCode::new("corlib/List.cs", include_str!("../../../corlib/List.cs")),
            SourceCode::new(
                "corlib/Programs.cs",
                include_str!("../../../corlib/Programs.cs"),
            ),
            SourceCode::new(
                "corlib/Delegates.cs",
                include_str!("../../../corlib/Delegates.cs"),
            ),
            SourceCode::new(
                "corlib/Dictionary.cs",
                include_str!("../../../corlib/Dictionary.cs"),
            ),
            SourceCode::new(
                "corlib/HashSet.cs",
                include_str!("../../../corlib/HashSet.cs"),
            ),
            SourceCode::new("corlib/Queue.cs", include_str!("../../../corlib/Queue.cs")),
            SourceCode::new("corlib/Stack.cs", include_str!("../../../corlib/Stack.cs")),
            SourceCode::new("corlib/Tasks.cs", include_str!("../../../corlib/Tasks.cs")),
            SourceCode::new(
                "corlib/Iterators.cs",
                include_str!("../../../corlib/Iterators.cs"),
            ),
            SourceCode::new(
                "corlib/Comparers.cs",
                include_str!("../../../corlib/Comparers.cs"),
            ),
            SourceCode::new("corlib/Linq.cs", include_str!("../../../corlib/Linq.cs")),
            SourceCode::new(
                "corlib/Reflection.cs",
                include_str!("../../../corlib/Reflection.cs"),
            ),
            SourceCode::new(
                "corlib/Result.cs",
                include_str!("../../../corlib/Result.cs"),
            ),
            behaviour,
        ];
        if sdk {
            sources.push(SourceCode::new(
                "corlib/Json.cs",
                include_str!("../../../corlib/Json.cs"),
            ));
            sources.push(SourceCode::new(
                "corlib/Http.cs",
                include_str!("../../../corlib/Http.cs"),
            ));
        }
        sources
    }

    /// Lowers a fully checked compilation to one Udon program. `entry_path`
    /// names the entry class; see [`men_sharp_codegen::generate`].
    pub fn generate_udon(
        &self,
        declarations: &Declarations<'_>,
        signatures: &Signatures,
        bodies: &BodyCheck,
        external: &(dyn ExternalTypes + Sync),
        entry_path: &[&str],
    ) -> men_sharp_codegen::CodegenOutput {
        timescope::scope!("generate program");
        let nodes = men_sharp_codegen::UdonNodes::for_unity_version(None);
        men_sharp_codegen::generate(
            declarations,
            signatures,
            bodies,
            external,
            nodes,
            entry_path,
        )
    }

    /// Discovers every `MenSharp.MenSharpBehaviour` subclass and lowers each
    /// to its own Udon program — the auto-discovery path the Unity package
    /// uses (no entry list needed).
    ///
    /// Programs are generated in parallel: each one lowers the same, finished
    /// compilation and shares nothing mutable with the others. The result
    /// keeps the discovery order of [`men_sharp_codegen::behaviour_classes`].
    pub fn generate_udon_behaviours(
        &self,
        declarations: &Declarations<'_>,
        signatures: &Signatures,
        bodies: &BodyCheck,
        external: &(dyn ExternalTypes + Sync),
        files: &[ParsedFile],
    ) -> Vec<UdonBehaviourProgram> {
        timescope::scope!("generate programs");
        let class_paths = men_sharp_codegen::behaviour_classes(declarations, signatures);
        self.pool.install(|| {
            class_paths
                .into_par_iter()
                .map(|class_path| {
                    let segments: Vec<&str> = class_path.split('.').collect();
                    let mut output =
                        self.generate_udon(declarations, signatures, bodies, external, &segments);
                    // codegen deals in file ids; the names live here
                    output.program.source = output
                        .source_file
                        .and_then(|file| files.get(file.0 as usize))
                        .map(|file| file.name.to_string());
                    UdonBehaviourProgram { class_path, output }
                })
                .collect()
        })
    }

    /// Assembles every program that generated without errors, in parallel:
    /// the `.uasm` text and its meta JSON sidecar. `None` where the program
    /// has codegen errors (there is nothing to assemble). Order matches
    /// `programs`.
    pub fn emit_udon_behaviours(
        &self,
        programs: &[UdonBehaviourProgram],
    ) -> Vec<Option<Result<EmittedProgram, AssembleError>>> {
        timescope::scope!("assemble programs");
        self.pool.install(|| {
            programs
                .par_iter()
                .map(|program| {
                    Self::emit_one(program)
                        .map(|emitted| emitted.map(|(uasm, meta)| EmittedProgram { uasm, meta }))
                })
                .collect()
        })
    }

    /// [`Compiler::emit_udon_behaviours`] that also writes each program's two
    /// files into `out_dir` (`<class path>.uasm` and `<class path>.meta.json`),
    /// in parallel. Each entry is the `.uasm` path written, `None` for a
    /// program with codegen errors, or the failure. Order matches `programs`.
    pub fn write_udon_behaviours(
        &self,
        programs: &[UdonBehaviourProgram],
        out_dir: &std::path::Path,
    ) -> Vec<Option<Result<std::path::PathBuf, EmitError>>> {
        timescope::scope!("write programs");
        self.pool.install(|| {
            programs
                .par_iter()
                .map(|program| {
                    let (uasm, meta) = match Self::emit_one(program)? {
                        Ok(texts) => texts,
                        Err(error) => return Some(Err(EmitError::Assemble(error))),
                    };
                    // the class path contains dots, so extensions are appended,
                    // not swapped in (`Game.Door` must not become `Game.uasm`)
                    let uasm_path = out_dir.join(format!("{}.uasm", program.class_path));
                    let meta_path = out_dir.join(format!("{}.meta.json", program.class_path));
                    timescope::scope!("write program files");
                    let written = std::fs::write(&uasm_path, uasm)
                        .map_err(|error| EmitError::Write(uasm_path.clone(), error))
                        .and_then(|()| {
                            std::fs::write(&meta_path, meta)
                                .map_err(|error| EmitError::Write(meta_path, error))
                        });
                    Some(written.map(|()| uasm_path))
                })
                .collect()
        })
    }

    /// The two texts of one program; `None` when it has codegen errors. The
    /// `.uasm` and the sidecar are independent, so they are built side by side.
    fn emit_one(program: &UdonBehaviourProgram) -> Option<Result<(String, String), AssembleError>> {
        if !program.output.errors.is_empty() {
            return None;
        }
        timescope::scope!("assemble program");
        let (uasm, meta) = rayon::join(
            || program.output.program.to_uasm(),
            || program.output.program.to_meta_json(),
        );
        Some(uasm.and_then(|uasm| meta.map(|meta| (uasm, meta))))
    }
}

/// One behaviour's assembled output: the `.uasm` text and the meta JSON
/// sidecar the Unity importer applies alongside it.
pub struct EmittedProgram {
    pub uasm: String,
    pub meta: String,
}

/// Why a program could not be written out.
#[derive(Debug)]
pub enum EmitError {
    Assemble(AssembleError),
    /// The path that could not be written, and why.
    Write(std::path::PathBuf, std::io::Error),
}

/// One behaviour's compiled program, tagged with its class path (`Demo.Door`).
pub struct UdonBehaviourProgram {
    pub class_path: String,
    pub output: men_sharp_codegen::CodegenOutput,
}
