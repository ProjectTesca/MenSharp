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
//! let compiler = Compiler::new(CompilerSettings { thread_count: Some(2) }).unwrap();
//! let files = compiler.parse(vec![SourceCode::new("main.ms", "class Program {}")]);
//! let declarations = compiler.collect_declarations(&files);
//! assert!(declarations.errors.is_empty());
//! ```

use std::sync::Arc;

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
}

/// One input file: a display name (usually the path) and its text.
#[derive(Debug, Clone)]
pub struct SourceCode {
    pub name: Arc<str>,
    pub text: Arc<str>,
}

impl SourceCode {
    pub fn new(name: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

/// A parsed file. Its [`FileId`] is its index in the vec [`Compiler::parse`] returned.
#[derive(Debug)]
pub struct ParsedFile {
    pub name: Arc<str>,
    pub ast: MenSharpAST,
}

pub struct Compiler {
    settings: CompilerSettings,
    pool: rayon::ThreadPool,
}

impl Compiler {
    pub fn new(settings: CompilerSettings) -> Result<Self, rayon::ThreadPoolBuildError> {
        let mut builder =
            rayon::ThreadPoolBuilder::new().thread_name(|index| format!("men-sharp-{index}"));

        if let Some(count) = settings.thread_count {
            builder = builder.num_threads(count);
        }

        Ok(Self {
            settings,
            pool: builder.build()?,
        })
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
        self.pool.install(|| {
            sources
                .into_par_iter()
                .map(|source| ParsedFile {
                    name: source.name,
                    ast: MenSharpAST::parse(source.text),
                })
                .collect()
        })
    }

    /// Collects declarations from every file in parallel, then merges them into one
    /// symbol table. The result borrows the parsed files, which is what ties the
    /// symbol table's lifetime to the syntax trees it points into.
    pub fn collect_declarations<'ast>(&self, files: &'ast [ParsedFile]) -> Declarations<'ast> {
        let collected = self.pool.install(|| {
            files
                .par_iter()
                .enumerate()
                .map(|(index, file)| collect_file(FileId(index as u32), file.ast.ast()))
                .collect()
        });

        merge_declarations(collected)
    }

    /// Parses every referenced dll in parallel and indexes them as an external
    /// type provider. The byte buffers stay with the caller, which is what ties
    /// the provider's lifetime to them.
    pub fn load_references<'data>(
        &self,
        references: &'data [Vec<u8>],
    ) -> Result<ReferenceSet<'data>, ReferenceError> {
        let assemblies: Vec<Result<DotNetAssembly, ReferenceError>> = self.pool.install(|| {
            references
                .par_iter()
                .enumerate()
                .map(|(index, bytes)| {
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
        let per_file: Vec<Signatures> = self.pool.install(|| {
            (0..declarations.files.len())
                .into_par_iter()
                .map(|index| resolve_file(declarations, external, index))
                .collect()
        });

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
        let per_file: Vec<BodyCheck> = self.pool.install(|| {
            (0..declarations.files.len())
                .into_par_iter()
                .map(|index| check_file(declarations, signatures, external, index))
                .collect()
        });

        let mut all = BodyCheck::default();
        for check in per_file {
            all.merge(check);
        }
        all.errors
            .sort_by_key(|error| (error.file, error.span.start, error.span.end));
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
        Self::corlib_sources_with_unity(unity)
    }

    fn corlib_sources_with_unity(unity: bool) -> Vec<SourceCode> {
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
        vec![
            SourceCode::new("corlib/List.cs", include_str!("../../../corlib/List.cs")),
            SourceCode::new(
                "corlib/Dictionary.cs",
                include_str!("../../../corlib/Dictionary.cs"),
            ),
            behaviour,
        ]
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
    pub fn generate_udon_behaviours(
        &self,
        declarations: &Declarations<'_>,
        signatures: &Signatures,
        bodies: &BodyCheck,
        external: &(dyn ExternalTypes + Sync),
        files: &[ParsedFile],
    ) -> Vec<UdonBehaviourProgram> {
        men_sharp_codegen::behaviour_classes(declarations, signatures)
            .into_iter()
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
    }
}

/// One behaviour's compiled program, tagged with its class path (`Demo.Door`).
pub struct UdonBehaviourProgram {
    pub class_path: String,
    pub output: men_sharp_codegen::CodegenOutput,
}
