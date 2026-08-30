//! Differential testing against Roslyn: every file in `tests/corpus` is fed to
//! both the real C# compiler (`csc`) and the M# front end, with the same
//! reference assemblies, and the verdicts must agree.
//!
//! - `corpus/ok`: csc accepts → M# must report **zero** errors.
//! - `corpus/cs0411`: csc rejects with CS0411 → M# must report
//!   `CannotInferTypeArguments` (the only place that error is allowed).
//! - `corpus/pending`: csc accepts but M# has a known gap. Reported, not
//!   failed — unless a file starts checking cleanly, in which case the test
//!   demands it graduates to `corpus/ok` (a ratchet).
//!
//! Tests pass vacuously (with a note on stderr) when neither a .NET SDK nor a
//! Unity editor is installed. Set `MENSHARP_REQUIRE_DIFFERENTIAL=1` (CI does)
//! to turn a missing toolchain into a failure.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};
use men_sharp_semantics::{SemanticError, SemanticErrorKind};

/// The reference closure shared by both compilers. Corpus files must not need
/// anything outside this list.
const REFERENCES: &[&str] = &[
    "System.Private.CoreLib.dll",
    "System.Runtime.dll",
    "System.Linq.dll",
    "System.Collections.dll",
    "System.Console.dll",
];

fn require_toolchain() -> bool {
    std::env::var("MENSHARP_REQUIRE_DIFFERENTIAL").is_ok_and(|v| v == "1")
}

fn skip(reason: &str) {
    if require_toolchain() {
        panic!("MENSHARP_REQUIRE_DIFFERENTIAL=1 but {reason}");
    }
    eprintln!("skipped: {reason}");
}

/// Latest `shared/Microsoft.NETCore.App/<version>` under a dotnet root.
fn shared_runtime_dir(root: &Path) -> Option<PathBuf> {
    let base = root.join("shared/Microsoft.NETCore.App");
    let mut versions: Vec<_> = std::fs::read_dir(base).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    versions.pop().map(|entry| entry.path())
}

fn dotnet_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(root) = std::env::var("DOTNET_ROOT") {
        roots.push(PathBuf::from(root));
    }
    roots.push(PathBuf::from("/usr/share/dotnet"));
    roots.push(PathBuf::from("/usr/lib/dotnet"));
    if let Ok(home) = std::env::var("HOME") {
        let editors = Path::new(&home).join("Unity/Hub/Editor");
        if let Ok(entries) = std::fs::read_dir(editors) {
            for editor in entries.flatten() {
                roots.push(editor.path().join("Editor/Data/DotNetSdk"));
            }
        }
    }
    roots
}

/// The runtime directory holding the reference dlls both compilers load.
fn reference_dir() -> Option<PathBuf> {
    dotnet_roots()
        .iter()
        .filter_map(|root| shared_runtime_dir(root))
        .find(|dir| REFERENCES.iter().all(|name| dir.join(name).exists()))
}

struct Roslyn {
    dotnet: PathBuf,
    csc_dll: PathBuf,
}

/// Locate a Roslyn `csc.dll` and a `dotnet` host to run it with.
/// `MENSHARP_DOTNET` / `MENSHARP_CSC_DLL` override the probe.
fn find_roslyn() -> Option<Roslyn> {
    if let Ok(dll) = std::env::var("MENSHARP_CSC_DLL") {
        let dotnet = std::env::var("MENSHARP_DOTNET").unwrap_or_else(|_| "dotnet".to_string());
        return Some(Roslyn {
            dotnet: PathBuf::from(dotnet),
            csc_dll: PathBuf::from(dll),
        });
    }
    for root in dotnet_roots() {
        let dotnet = root.join("dotnet");
        if !dotnet.exists() {
            continue;
        }
        let Ok(sdks) = std::fs::read_dir(root.join("sdk")) else {
            continue;
        };
        let mut sdks: Vec<_> = sdks.flatten().map(|entry| entry.path()).collect();
        sdks.sort();
        for sdk in sdks.into_iter().rev() {
            let csc_dll = sdk.join("Roslyn/bincore/csc.dll");
            if csc_dll.exists() {
                return Some(Roslyn { dotnet, csc_dll });
            }
        }
    }
    None
}

struct CscVerdict {
    file: PathBuf,
    success: bool,
    output: String,
}

/// Compile every file with csc, one process per file, all in flight at once.
fn csc_verdicts(roslyn: &Roslyn, refs_dir: &Path, files: &[PathBuf]) -> Vec<CscVerdict> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out_dir = std::env::temp_dir().join(format!(
        "mensharp-differential-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&out_dir).unwrap();

    let children: Vec<_> = files
        .iter()
        .map(|file| {
            let out = out_dir.join(format!(
                "{}.dll",
                file.file_stem().unwrap().to_string_lossy()
            ));
            let mut command = Command::new(&roslyn.dotnet);
            command
                .arg("exec")
                .arg(&roslyn.csc_dll)
                .args([
                    "-nologo",
                    "-noconfig",
                    "-nostdlib",
                    "-t:library",
                    "-warn:0",
                    "-langversion:12",
                ])
                .arg(format!("-out:{}", out.display()));
            for reference in REFERENCES {
                command.arg(format!("-r:{}", refs_dir.join(reference).display()));
            }
            command
                .arg(file)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to spawn csc")
        })
        .collect();

    let verdicts = files
        .iter()
        .zip(children)
        .map(|(file, child)| {
            let output = child.wait_with_output().expect("csc did not run");
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            CscVerdict {
                file: file.clone(),
                success: output.status.success(),
                output: text,
            }
        })
        .collect();

    let _ = std::fs::remove_dir_all(&out_dir);
    verdicts
}

fn corpus_files(group: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(group);
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("corpus directory missing")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "cs"))
        .collect();
    files.sort();
    files
}

/// Run the whole M# pipeline over one standalone file; all phases' errors.
fn men_sharp_errors(
    compiler: &Compiler,
    references: &men_sharp_compiler::ReferenceSet,
    file: &Path,
) -> Vec<SemanticError> {
    let source = std::fs::read_to_string(file).unwrap();
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let files = compiler.parse(vec![SourceCode::new(name, source)]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, references);
    let bodies = compiler.check_bodies(&declarations, &signatures, references);

    let mut errors = declarations.errors;
    errors.extend(signatures.errors);
    errors.extend(bodies.errors);
    errors
}

fn describe(errors: &[SemanticError]) -> String {
    errors
        .iter()
        .map(|error| format!("  {:?} at {:?}", error.kind, error.span))
        .collect::<Vec<_>>()
        .join("\n")
}

struct Harness {
    compiler: Compiler,
    reference_bytes: Vec<Vec<u8>>,
    refs_dir: PathBuf,
    roslyn: Roslyn,
}

fn harness() -> Option<Harness> {
    let Some(refs_dir) = reference_dir() else {
        skip("no .NET runtime with the reference closure on this machine");
        return None;
    };
    let Some(roslyn) = find_roslyn() else {
        skip("no Roslyn csc.dll found (install a .NET SDK, or set MENSHARP_CSC_DLL)");
        return None;
    };
    let reference_bytes = REFERENCES
        .iter()
        .map(|name| std::fs::read(refs_dir.join(name)).unwrap())
        .collect();
    Some(Harness {
        compiler: Compiler::new(CompilerSettings::default()).unwrap(),
        reference_bytes,
        refs_dir,
        roslyn,
    })
}

#[test]
fn ok_corpus_is_accepted_by_both_compilers() {
    let Some(harness) = harness() else { return };
    let references = harness
        .compiler
        .load_references(&harness.reference_bytes)
        .unwrap();
    let files = corpus_files("ok");
    assert!(!files.is_empty());

    let mut failures = Vec::new();
    for verdict in csc_verdicts(&harness.roslyn, &harness.refs_dir, &files) {
        if !verdict.success {
            failures.push(format!(
                "{}: csc REJECTED a corpus/ok file — fix the corpus:\n{}",
                verdict.file.display(),
                verdict.output
            ));
            continue;
        }
        let errors = men_sharp_errors(&harness.compiler, &references, &verdict.file);
        if !errors.is_empty() {
            failures.push(format!(
                "{}: csc accepts but M# reports errors:\n{}",
                verdict.file.display(),
                describe(&errors)
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn cs0411_boundary_matches_roslyn() {
    let Some(harness) = harness() else { return };
    let references = harness
        .compiler
        .load_references(&harness.reference_bytes)
        .unwrap();
    let files = corpus_files("cs0411");
    assert!(!files.is_empty());

    let mut failures = Vec::new();
    for verdict in csc_verdicts(&harness.roslyn, &harness.refs_dir, &files) {
        if verdict.success || !verdict.output.contains("CS0411") {
            failures.push(format!(
                "{}: expected csc to fail with CS0411, got:\n{}",
                verdict.file.display(),
                verdict.output
            ));
            continue;
        }
        let errors = men_sharp_errors(&harness.compiler, &references, &verdict.file);
        let inference_errors = errors
            .iter()
            .filter(|error| error.kind == SemanticErrorKind::CannotInferTypeArguments)
            .count();
        if inference_errors == 0 {
            failures.push(format!(
                "{}: Roslyn says CS0411 but M# did not report CannotInferTypeArguments:\n{}",
                verdict.file.display(),
                describe(&errors)
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn pending_corpus_tracks_known_gaps() {
    let Some(harness) = harness() else { return };
    let references = harness
        .compiler
        .load_references(&harness.reference_bytes)
        .unwrap();
    let files = corpus_files("pending");
    if files.is_empty() {
        return;
    }

    let mut failures = Vec::new();
    for verdict in csc_verdicts(&harness.roslyn, &harness.refs_dir, &files) {
        if !verdict.success {
            failures.push(format!(
                "{}: csc REJECTED a corpus/pending file — fix the corpus:\n{}",
                verdict.file.display(),
                verdict.output
            ));
            continue;
        }
        let errors = men_sharp_errors(&harness.compiler, &references, &verdict.file);
        if errors.is_empty() {
            failures.push(format!(
                "{}: gap closed! move this file from corpus/pending to corpus/ok",
                verdict.file.display()
            ));
        } else {
            eprintln!(
                "known gap {}:\n{}",
                verdict.file.display(),
                describe(&errors)
            );
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}
