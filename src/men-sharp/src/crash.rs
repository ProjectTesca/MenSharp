//! Crash reports. A bug in the compiler surfaces as a panic; the report it
//! leaves on stderr is what a user pastes into an issue, and what the Unity
//! package shows in the console — so it has to be self-contained (version,
//! platform, message, location, backtrace) and it has to be recognisable
//! among the diagnostics on the same stream. The [`MARKER`] line does the
//! latter: the package treats everything from that line on as the report.
//!
//! The backtrace is captured whether or not `RUST_BACKTRACE` is set: nobody
//! reruns a crashed compile from Unity with an environment variable, so the
//! first report has to be the complete one.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::panic::PanicHookInfo;
use std::sync::Mutex;

/// The first line of a report starts with this. The Unity package looks for
/// it; keep it in sync with `MenSharpCompiler.cs`.
pub const MARKER: &str = "[mensharp crash]";

/// Frames below this many are all the report keeps: past that the trace is
/// runtime and thread-pool plumbing, and a console entry has a size limit.
const MAX_BACKTRACE_LINES: usize = 80;

/// Installs the panic hook that writes reports to stderr.
pub fn install() {
    std::panic::set_hook(Box::new(report));
}

fn report(info: &PanicHookInfo<'_>) {
    // one report at a time: two worker threads failing together (a
    // parallel codegen over a corpus that trips the same bug twice) must not
    // interleave their lines
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _guard = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut text = String::new();
    let _ = writeln!(
        text,
        "{MARKER} men-sharp {} ({} {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    match info.location() {
        Some(location) => {
            let _ = writeln!(
                text,
                "thread '{thread}' panicked at {}:{}:{}:",
                location.file(),
                location.line(),
                location.column()
            );
        }
        None => {
            let _ = writeln!(text, "thread '{thread}' panicked:");
        }
    }
    let _ = writeln!(text, "{}", info.payload_as_str().unwrap_or("(no message)"));
    let _ = writeln!(text, "stack backtrace:");
    for line in trimmed_backtrace(&Backtrace::force_capture().to_string()) {
        let _ = writeln!(text, "{line}");
    }
    eprint!("{text}");
}

/// The frames of a captured backtrace between the panic machinery at the top
/// and the runtime's entry at the bottom: the ones that say where the
/// compiler was.
fn trimmed_backtrace(backtrace: &str) -> Vec<&str> {
    let lines: Vec<&str> = backtrace.lines().collect();
    // a frame line ("   3: some::function") and, with debug info, its
    // "             at file:line" line; the panic hook's own frames and the
    // panic runtime's come first, and the last of them is the frame that
    // raised the panic's caller's caller
    let start = lines
        .iter()
        .rposition(|line| {
            let line = line.trim_start();
            line.contains("rust_begin_unwind")
                || line.contains("std::panicking::")
                || line.contains("core::panicking::")
        })
        .map(|index| {
            // skip that frame's own "at" line too
            let mut next = index + 1;
            if lines
                .get(next)
                .is_some_and(|line| line.trim_start().starts_with("at "))
            {
                next += 1;
            }
            next
        })
        .unwrap_or(0);
    let end = lines
        .iter()
        .position(|line| line.contains("__rust_begin_short_backtrace"))
        .unwrap_or(lines.len());
    let end = end.max(start);
    let mut kept: Vec<&str> = lines[start..end].to_vec();
    if kept.is_empty() {
        // nothing recognisable (a stripped binary, a platform without
        // symbols): better the raw trace than none
        kept = lines;
    }
    if kept.len() > MAX_BACKTRACE_LINES {
        kept.truncate(MAX_BACKTRACE_LINES);
        kept.push("   ... (truncated)");
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_frames_between_the_panic_and_the_runtime() {
        let raw = "\
   0: std::backtrace::Backtrace::force_capture
   1: men_sharp::crash::report
   2: std::panicking::rust_panic_with_hook
   3: std::panicking::begin_panic_handler
             at /rustc/abc/library/std/src/panicking.rs:100:1
   4: core::panicking::panic_fmt
             at /rustc/abc/library/core/src/panicking.rs:50:1
   5: men_sharp_codegen::generator::Generator::lower
             at ./src/men-sharp-codegen/src/generator.rs:10:5
   6: men_sharp_compiler::Compiler::generate
   7: std::sys::backtrace::__rust_begin_short_backtrace
   8: std::rt::lang_start
";
        assert_eq!(
            trimmed_backtrace(raw),
            vec![
                "   5: men_sharp_codegen::generator::Generator::lower",
                "             at ./src/men-sharp-codegen/src/generator.rs:10:5",
                "   6: men_sharp_compiler::Compiler::generate",
            ]
        );
    }

    #[test]
    fn falls_back_to_the_raw_trace_when_nothing_is_recognisable() {
        let raw = "   0: <unknown>\n   1: <unknown>\n";
        assert_eq!(
            trimmed_backtrace(raw),
            vec!["   0: <unknown>", "   1: <unknown>"]
        );
    }
}
