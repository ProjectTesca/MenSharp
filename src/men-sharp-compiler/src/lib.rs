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
    Declarations, ExternalTypes, FileId, Signatures, collect_file, merge_declarations, resolve_file,
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
}
