//! End-to-end code generation: M# source → checked → Udon assembly → run in
//! the emulator → assert on heap values. Skips (with a note) when no .NET
//! runtime provides the reference assemblies.

use men_sharp_asm::{Emulator, Value};
use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};

fn dotnet_shared_dir() -> Option<std::path::PathBuf> {
    for root in ["/usr/share/dotnet", "/usr/lib/dotnet"] {
        let base = std::path::Path::new(root).join("shared/Microsoft.NETCore.App");
        if let Ok(entries) = std::fs::read_dir(base) {
            let mut versions: Vec<_> = entries.flatten().collect();
            versions.sort_by_key(|entry| entry.file_name());
            if let Some(version) = versions.pop() {
                return Some(version.path());
            }
        }
    }
    None
}

/// Compile `source` (with the mini-corlib, as every real compilation has
/// it: the runtime checks throw its exception types), run entry `event`,
/// and return the finished emulator.
fn run(source: &str, event: &str) -> Option<Emulator> {
    run_with_corlib(source, event)
}

/// [`run`] with the mini-corlib (`List<T>`, ...) compiled in.
fn run_with_corlib(source: &str, event: &str) -> Option<Emulator> {
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    run_sources(sources, event)
}

fn run_sources(sources: Vec<SourceCode>, event: &str) -> Option<Emulator> {
    let (program, result) = run_sources_result(sources, event)?;
    Some(result.unwrap_or_else(|error| panic!("emulator error: {error:?}\n{program}")))
}

/// [`run_sources`] that hands back the emulator's verdict instead of
/// panicking on it — for programs expected to halt.
fn run_sources_result(
    sources: Vec<SourceCode>,
    event: &str,
) -> Option<(String, Result<Emulator, men_sharp_asm::EmulatorError>)> {
    let (program, assembled) = build(sources)?;
    let mut emulator = Emulator::new(&program, &assembled);
    let result = emulator.run(&assembled, event).map(|()| emulator);
    Some((program.dump(), result))
}

/// Runs `event`, then moves the clock on by each `(seconds, frames)` step
/// in turn, delivering the delayed events that fall due — how a program
/// that awaits is driven.
fn run_stepping(source: &str, event: &str, steps: &[(f32, i32)]) -> Option<Emulator> {
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let (program, assembled) = build(sources)?;
    let mut emulator = Emulator::new(&program, &assembled);
    let dump = program.dump();
    emulator
        .run(&assembled, event)
        .unwrap_or_else(|error| panic!("emulator error: {error:?}\n{dump}"));
    for (seconds, frames) in steps {
        emulator
            .advance(&assembled, *seconds, *frames)
            .unwrap_or_else(|error| panic!("emulator error: {error:?}\n{dump}"));
    }
    Some(emulator)
}

/// Compiles `sources` (adding the mini-corlib when absent) to an assembled
/// program, asserting that every phase is clean.
fn build(
    mut sources: Vec<SourceCode>,
) -> Option<(men_sharp_asm::Program, men_sharp_asm::Assembled)> {
    let dir = dotnet_shared_dir()?;
    if !sources
        .iter()
        .any(|source| source.name.starts_with("corlib/"))
    {
        sources.extend(Compiler::corlib_sources());
    }
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(declarations.errors, vec![], "declaration errors");
    assert_eq!(signatures.errors, vec![], "signature errors");
    assert_eq!(bodies.errors, vec![], "type errors");

    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );

    let assembled = output.program.assemble().unwrap();
    Some((output.program, assembled))
}

fn int_of(emulator: &Emulator, name: &str) -> i32 {
    match emulator.value_of(name) {
        Some(Value::Int32(value)) => *value,
        other => panic!("{name} = {other:?}"),
    }
}

fn string_of(emulator: &Emulator, name: &str) -> String {
    match emulator.value_of(name) {
        Some(Value::Str(value)) => value.to_string(),
        other => panic!("{name} = {other:?}"),
    }
}

#[test]
fn arithmetic_lands_in_a_static_field() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    result = 40 + 2;
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn loops_conditions_and_locals() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    int total = 0;
                    for (int i = 1; i <= 10; i++)
                    {
                        if (i % 2 == 0) { total += i; }
                    }
                    while (total < 40) { total = total + 5; }
                    result = total;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    // evens 2..10 = 30, then 35, 40
    assert_eq!(int_of(&emulator, "result"), 40);
}

#[test]
fn static_method_calls_pass_arguments_and_return() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;

                private static int Add(int a, int b) { return a + b; }
                private static int Twice(int x) => Add(x, x);

                public static void Main()
                {
                    result = Twice(Add(10, 11));
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn objects_fields_constructors_and_instance_methods() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Counter
            {
                private int value;
                public int Bonus = 7;

                public Counter(int start) { value = start; }

                public void Add(int amount) { value += amount; }
                public int Value => value + Bonus;
            }

            public class Program
            {
                public static int result;
                public static void Main()
                {
                    var counter = new Counter(30);
                    counter.Add(5);
                    result = counter.Value;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn generic_classes_monomorphize() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Box<T>
            {
                private T item;
                public void Put(T value) { item = value; }
                public T Take() { return item; }
            }

            public class Program
            {
                public static int result;
                public static string text;
                public static void Main()
                {
                    var numbers = new Box<int>();
                    numbers.Put(42);
                    result = numbers.Take();

                    var words = new Box<string>();
                    words.Put("hello");
                    text = words.Take();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
    assert_eq!(string_of(&emulator, "text"), "hello");
}

#[test]
fn virtual_calls_dispatch_on_the_runtime_type() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Animal
            {
                public virtual int Legs() { return 0; }
            }
            public class Dog : Animal
            {
                public override int Legs() { return 4; }
            }
            public class Bird : Animal
            {
                public override int Legs() { return 2; }
            }

            public class Program
            {
                public static int result;
                public static void Main()
                {
                    Animal a = new Dog();
                    Animal b = new Bird();
                    result = a.Legs() * 10 + b.Legs();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn arrays_and_foreach() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    var values = new int[4] { 20, 10, 8, 4 };
                    int total = 0;
                    foreach (var value in values)
                    {
                        total += value;
                    }
                    result = total + values.Length - values[3];
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn strings_and_interpolation() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static string text;
                public static void Main()
                {
                    int count = 3;
                    string name = "door";
                    text = $"{name}: {count + 1} visits";
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "text"), "door: 4 visits");
}

#[test]
fn recursion_just_works() {
    // static frames plus a save/restore stack woven in around the calls that
    // can come back — no attribute, no configuration, like C#
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                private static int Fib(int n)
                {
                    if (n < 2) { return n; }
                    return Fib(n - 1) + Fib(n - 2);
                }
                public static void Main() { result = Fib(10); }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 55);
}

#[test]
fn mutual_recursion_just_works() {
    // the cycle spans two functions, so each save site protects its callee
    // and the chain unwinds cleanly
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static bool even;
                public static bool odd;
                private static bool IsEven(int n) { if (n == 0) { return true; } return IsOdd(n - 1); }
                private static bool IsOdd(int n) { if (n == 0) { return false; } return IsEven(n - 1); }
                public static void Main()
                {
                    even = IsEven(10);
                    odd = IsOdd(10);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert!(matches!(
        emulator.value_of("even"),
        Some(Value::Boolean(true))
    ));
    assert!(matches!(
        emulator.value_of("odd"),
        Some(Value::Boolean(false))
    ));
}

#[test]
fn recursion_through_virtual_dispatch_just_works() {
    // the cycle runs through the synthesized dispatcher: Node.Sum calls
    // this.Next().Sum() on a base-typed reference, which dispatches back
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Node
            {
                public int value;
                public Node next;
                public virtual int Sum()
                {
                    if (next == null) { return value; }
                    return value + next.Sum();
                }
            }

            public class DoubleNode : Node
            {
                public override int Sum()
                {
                    if (next == null) { return value * 2; }
                    return value * 2 + next.Sum();
                }
            }

            public class Program
            {
                public static int result;
                public static void Main()
                {
                    var a = new Node();
                    var b = new DoubleNode();
                    var c = new Node();
                    a.value = 1;
                    b.value = 2;
                    c.value = 3;
                    a.next = b;
                    b.next = c;
                    result = a.Sum();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    // 1 + 2*2 + 3, the middle node through its override
    assert_eq!(int_of(&emulator, "result"), 8);
}

#[test]
fn the_corlib_list_works_end_to_end() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using System.Collections.Generic;

        namespace Game
        {
            public class Program
            {
                public static int result;
                public static string text;

                public static void Main()
                {
                    var numbers = new List<int>();
                    for (int i = 1; i <= 10; i++) { numbers.Add(i); }
                    numbers.RemoveAt(0);
                    numbers[0] = numbers[0] + 40;

                    int total = 0;
                    for (int i = 0; i < numbers.Count; i++) { total += numbers[i]; }

                    var words = new List<string>();
                    words.Add("grow");
                    words.Add("ing");
                    text = words[0] + words[1];

                    // 2..10 summed = 54, +40 = 94
                    result = total;
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(declarations.errors, vec![], "declaration errors");
    assert_eq!(signatures.errors, vec![], "signature errors");
    assert_eq!(bodies.errors, vec![], "type errors");

    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );

    let assembled = output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&output.program, &assembled);
    emulator.run(&assembled, "Main").unwrap();
    assert_eq!(int_of(&emulator, "result"), 94);
    assert_eq!(string_of(&emulator, "text"), "growing");
}

#[test]
fn the_shipped_demo_runs_in_the_emulator() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let unity = std::path::Path::new(&home).join(
        "Unity/Hub/Editor/6000.5.0f1/Editor/Data/Managed/UnityEngine/UnityEngine.CoreModule.dll",
    );
    if !unity.exists() {
        eprintln!("skipped: no Unity editor on this machine");
        return;
    }

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(unity).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let demo = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/udon_demo.cs");
    let mut sources = vec![SourceCode::new(
        "udon_demo.cs",
        std::fs::read_to_string(demo).unwrap(),
    )];
    sources.extend(Compiler::corlib_sources());

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Demo", "Greeter"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );

    let assembled = output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&output.program, &assembled);
    emulator.run(&assembled, "_start").unwrap();
    assert_eq!(int_of(&emulator, "total"), 3);
    assert_eq!(
        emulator.log,
        vec![
            "hello beatrice (visit #1)".to_string(),
            "hello claude (visit #2)".to_string(),
            "total visits: 3".to_string(),
        ]
    );
}

/// Compile exactly these sources — the caller decides whether the mini-corlib
/// is among them — and return the named behaviour's program, codegen errors
/// and all.
fn compile_behaviour(
    sources: Vec<SourceCode>,
    class_path: &str,
) -> Option<men_sharp_compiler::UdonBehaviourProgram> {
    let dir = dotnet_shared_dir()?;
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let found: Vec<&String> = programs.iter().map(|program| &program.class_path).collect();
    let index = programs
        .iter()
        .position(|program| program.class_path == class_path)
        .unwrap_or_else(|| panic!("behaviour {class_path} was not discovered; found {found:?}"));
    Some(programs.into_iter().nth(index).unwrap())
}

/// The class paths of every behaviour program the compilation produces.
fn compile_behaviours(sources: Vec<SourceCode>) -> Vec<String> {
    let Some(dir) = dotnet_shared_dir() else {
        return Vec::new();
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    compiler
        .generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files)
        .into_iter()
        .map(|program| program.class_path)
        .collect()
}

/// Compile with the corlib included (MenSharpBehaviour lives there) and run
/// one behaviour program.
fn run_behaviour(source: &str, class_path: &str, event: &str) -> Option<Emulator> {
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let program = compile_behaviour(sources, class_path)?;
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );

    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    emulator.run(&assembled, event).unwrap_or_else(|error| {
        panic!(
            "emulator error: {error:?}\n{}",
            program.output.program.dump()
        )
    });
    Some(emulator)
}

#[test]
fn behaviour_instance_fields_become_public_variables() {
    let Some(emulator) = run_behaviour(
        r#"
        using MenSharp;

        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public int openCount;
                public float speed = 1.5f;
                public string label = "front door";

                private int Bump()
                {
                    openCount = openCount + 1;
                    return openCount;
                }

                public void Interact()
                {
                    Bump();
                    this.Bump();
                    this.openCount += 40;
                }
            }
        }
        "#,
        "Game.Door",
        "_interact",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    // two bumps + 40
    assert_eq!(int_of(&emulator, "openCount"), 42);
    assert_eq!(string_of(&emulator, "label"), "front door");
    match emulator.value_of("speed") {
        Some(Value::Single(value)) => assert_eq!(*value, 1.5),
        other => panic!("speed = {other:?}"),
    }
}

#[test]
fn every_behaviour_in_the_compilation_is_discovered() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour { public void Interact() { } }
            public class Lamp : MenSharpBehaviour { public void Interact() { } }
            public class Helper { }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let names: Vec<&str> = programs.iter().map(|p| p.class_path.as_str()).collect();
    // ... plus the holder of the statics they share
    assert_eq!(names, vec!["Game.Door", "Game.Lamp", "MenSharp.Statics"]);
    for program in &programs {
        assert!(
            program.output.errors.is_empty(),
            "{:#?}",
            program.output.errors
        );
    }
}

#[test]
fn constructing_a_behaviour_is_an_error() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public void Interact() { var other = new Door(); }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    assert!(
        programs[0]
            .output
            .errors
            .iter()
            .any(|error| error.message.to_string().contains("cannot be constructed")),
        "{:#?}",
        programs[0].output.errors
    );
}

#[test]
fn inspector_values_survive_field_initializers() {
    // the Unity inspector applies public variables after the heap loads;
    // literal field initializers must be baked heap defaults, not runtime
    // code, or they would overwrite the inspector's values on the first event
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public int bonus = 7;
                public string label = "default";
                public int result;

                public void Interact()
                {
                    result = bonus;
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program = &programs[0];
    assert!(
        program.output.errors.is_empty(),
        "{:#?}",
        program.output.errors
    );

    let assembled = program.output.program.assemble().unwrap();

    // without inspector overrides: the baked defaults
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    emulator.run(&assembled, "_interact").unwrap();
    assert_eq!(int_of(&emulator, "result"), 7);
    assert_eq!(string_of(&emulator, "label"), "default");

    // with inspector overrides applied before the first event: they win
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    assert!(emulator.set_value("bonus", Value::Int32(300)));
    assert!(emulator.set_value("label", Value::Str("from inspector".into())));
    emulator.run(&assembled, "_interact").unwrap();
    assert_eq!(int_of(&emulator, "result"), 300);
    assert_eq!(string_of(&emulator, "label"), "from inspector");
}

/// A stand-in for the corlib behaviour base class. The real one declares
/// `gameObject`/`transform` as `UnityEngine` types, which need Unity's
/// assemblies; the rule the generator applies — *members declared on
/// `MenSharpBehaviour` itself become self references* — does not care which
/// type they have, so a corlib-free compilation can test it anywhere.
const SELF_REFERENCE_BASE: &str = r#"
    namespace MenSharp
    {
        public class MenSharpBehaviour
        {
            public object gameObject { get; }
        }
    }
"#;

#[test]
fn self_references_become_this_initialized_slots() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public object held;

                        public void Interact()
                        {
                            held = gameObject;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );

    let slot = program
        .output
        .program
        .data
        .iter()
        .find(|symbol| symbol.name == "__this_gameObject")
        .expect("no self-reference slot was emitted");
    // never exported: the inspector must not offer to override what the
    // behaviour is attached to
    assert!(!slot.export);

    let text = program.output.program.to_uasm().unwrap();
    assert!(
        text.contains("__this_gameObject: %SystemObject, this"),
        "{text}"
    );

    // and it survives into the running program as its own kind of value
    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    emulator.run(&assembled, "_interact").unwrap();
    match emulator.value_of("held") {
        Some(Value::SelfComponent(udon_type)) => assert_eq!(&**udon_type, "SystemObject"),
        other => panic!("held = {other:?}"),
    }
}

#[test]
fn assigning_to_a_self_reference_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public object other;

                        public void Interact()
                        {
                            gameObject = other;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages.iter().any(|message| message.contains("read-only")),
        "{messages:?}"
    );
}

#[test]
fn base_calls_bind_to_the_base_implementation() {
    // the trap: an override calling `base.M()` must not go through the
    // dispatcher, or it re-enters itself forever
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Animal
            {
                public int tag;
                public virtual int Legs() { return 2; }
            }

            public class Dog : Animal
            {
                public override int Legs() { return base.Legs() * 2; }
                public int Tag() { return base.tag + 1; }
            }

            public class Program
            {
                public static int result;
                public static int tagged;
                public static void Main()
                {
                    Animal a = new Dog();
                    result = a.Legs();
                    var dog = new Dog();
                    dog.tag = 40;
                    tagged = dog.Tag();
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    // virtual call finds Dog.Legs, whose `base.Legs()` reaches Animal's 2
    assert_eq!(int_of(&emulator, "result"), 4);
    assert_eq!(int_of(&emulator, "tagged"), 41);
}

#[test]
fn a_virtual_method_dispatches_to_its_own_class_too() {
    // the dispatcher used to occupy the declaring method's own key, so an
    // instantiated base class made the stub dispatch to itself
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Animal
            {
                public virtual int Legs() { return 2; }
            }
            public class Dog : Animal
            {
                public override int Legs() { return 4; }
            }

            public class Program
            {
                public static int result;
                public static void Main()
                {
                    Animal plain = new Animal();
                    Animal dog = new Dog();
                    result = plain.Legs() * 10 + dog.Legs();
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 24);
}

#[test]
fn base_reaches_a_self_reference_without_a_second_slot() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public object held;
                        public object alsoHeld;

                        public void Interact()
                        {
                            held = gameObject;
                            alsoHeld = base.gameObject;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let slots = program
        .output
        .program
        .data
        .iter()
        .filter(|symbol| symbol.name == "__this_gameObject")
        .count();
    assert_eq!(slots, 1, "`this` and `base` must share one slot");
}

#[test]
fn a_behaviour_inherits_variables_events_and_overrides() {
    // one behaviour, one instance: the leaf class owns everything its bases
    // declare, so an inherited event calling a virtual method must land on
    // the leaf's override, and that override's `base` call on the base body
    let Some(emulator) = run_behaviour(
        r#"
        using MenSharp;

        namespace Game
        {
            public class Interactable : MenSharpBehaviour
            {
                public int useCount;
                public virtual int Cost() { return 1; }
                public void Interact() { useCount += Cost(); }
            }

            public class Door : Interactable
            {
                public int extra = 2;
                public override int Cost() { return base.Cost() + extra; }
            }
        }
        "#,
        "Game.Door",
        "_interact",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    // Interact is inherited, Cost resolves to Door's override, base.Cost() to
    // Interactable's body: 1 + 2
    assert_eq!(int_of(&emulator, "useCount"), 3);
    // and the base class's public variable is the leaf program's too
    assert_eq!(int_of(&emulator, "extra"), 2);
}

#[test]
fn the_base_behaviour_still_compiles_on_its_own() {
    let Some(emulator) = run_behaviour(
        r#"
        using MenSharp;

        namespace Game
        {
            public class Interactable : MenSharpBehaviour
            {
                public int useCount;
                public virtual int Cost() { return 1; }
                public void Interact() { useCount += Cost(); }
            }

            public class Door : Interactable
            {
                public override int Cost() { return 5; }
            }
        }
        "#,
        "Game.Interactable",
        "_interact",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    // attached as itself, Interactable is the whole instance — Door's
    // override belongs to a different program
    assert_eq!(int_of(&emulator, "useCount"), 1);
}

#[test]
fn a_public_variable_hidden_by_a_derived_class_is_an_error() {
    // exported slots are named after the member, so two of one name would
    // collapse into a single inspector variable
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Middle : MenSharp.MenSharpBehaviour
                    {
                        public int shared;
                    }

                    public class Leaf : Middle
                    {
                        public new int shared;
                        public void Interact() { shared = 1; }
                    }
                }
                "#,
            ),
        ],
        "Game.Leaf",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages.iter().any(|message| message.contains("hides")),
        "{messages:?}"
    );
}

#[test]
fn calling_a_behaviour_member_on_an_object_is_a_plain_error() {
    // reachable only through `new Behaviour()`, which is itself an error —
    // but it used to surface as an "internal:" argument-count mismatch
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public void Interact()
                        {
                            var other = new Door();
                            other.Interact();
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        !messages.iter().any(|message| message.contains("internal:")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("cannot be constructed")),
        "{messages:?}"
    );
}

#[test]
fn a_program_records_the_file_it_came_from() {
    // the Unity inspector groups behaviours by source file to offer the ones
    // drag-and-drop cannot add; Unity itself cannot answer that question, so
    // the sidecar carries it
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "Doors.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public void Interact() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert_eq!(program.output.program.source.as_deref(), Some("Doors.cs"));
    let meta = program.output.program.to_meta_json().unwrap();
    assert!(meta.contains("\"source\": \"Doors.cs\""), "{meta}");
}

#[test]
fn one_behaviour_reaches_another_by_name() {
    // Udon gives two programs no shared memory: a field access becomes
    // Get/SetProgramVariable and a call becomes SendCustomEvent
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public bool isOpen;
                        public void Open() { isOpen = true; }
                    }

                    public class Switch : MenSharp.MenSharpBehaviour
                    {
                        public Door door;
                        public Door[] doors;

                        public void Interact()
                        {
                            door.isOpen = false;
                            door.Open();
                            bool seen = door.isOpen;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Switch",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );

    let text = program.output.program.to_uasm().unwrap();
    // a `this` heap reference may only resolve to a GameObject, a Transform
    // or an UdonBehaviour: an interface-typed slot is refused at load time and
    // takes the whole program with it, so a scalar reference is the concrete
    // type even though the externs name the interface
    assert!(text.contains("door: %VRCUdonUdonBehaviour"), "{text}");
    assert!(
        text.contains("doors: %VRCUdonCommonInterfacesIUdonEventReceiverArray"),
        "{text}"
    );
    assert!(text.contains("__SetProgramVariable__"), "{text}");
    assert!(text.contains("__GetProgramVariable__"), "{text}");
    assert!(text.contains("__SendCustomEvent__"), "{text}");

    // and the names it addresses them by are the source's own
    let meta = program.output.program.to_meta_json().unwrap();
    for name in ["isOpen", "Open"] {
        assert!(meta.contains(&format!("\"value\": \"{name}\"")), "{meta}");
    }
}

const CROSS_PROGRAM_CALLS: &str = r#"
namespace Game
{
    public class Door : MenSharp.MenSharpBehaviour
    {
        private int level;
        public void Slide(int amount) { level += amount; }
        public int Count() { return level; }
        public int Add(int a, int b) { return a + b; }
        public bool Take(int amount, out int rest) { rest = level - amount; return rest >= 0; }
        public int Level
        {
            get { return level; }
            set { level = value; }
        }
        public int Twice => level * 2;
    }

    public class Switch : MenSharp.MenSharpBehaviour
    {
        public Door door;
        public void Interact()
        {
            door.Slide(2);
            int n = door.Count();
            int sum = door.Add(n, 3);
            bool ok = door.Take(1, out int rest);
            door.Level = sum + rest;
            int level = door.Level + door.Twice;
        }
    }
}
"#;

/// A method call on another behaviour is UdonSharp's protocol: the
/// arguments are written into the callee's parameter variables, the event
/// runs, the result (and every `ref`/`out` argument) is read back — under the
/// names UdonSharp's own compiler would use, so both sides agree.
#[test]
fn a_call_across_programs_carries_arguments_and_results() {
    let sources = || {
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new("test.cs", CROSS_PROGRAM_CALLS),
        ]
    };
    let Some(switch) = compile_behaviour(sources(), "Game.Switch") else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        switch.output.errors.is_empty(),
        "{:#?}",
        switch.output.errors
    );
    let text = switch.output.program.to_uasm().unwrap();
    let meta = switch.output.program.to_meta_json().unwrap();
    // the caller's side: the names it writes and raises
    for name in [
        "__0_amount__param", // Slide(int amount)
        "__0_Slide",         // a method with parameters is mangled
        "Count",             // one without is not
        "__0_Count__ret",    // its result variable
        "__0_a__param",
        "__0_b__param",
        "__0_Add",
        "__0___0_Add__ret",  // the mangled name's result
        "__1_amount__param", // Take's `amount`: the second of that name
        "__0_rest__param",
        "__0_Take",
        "__0___0_Take__ret",
        "get_Level", // accessors are events of their own
        "__0_get_Level__ret",
        "__0_value__param",
        "__0_set_Level",
        "get_Twice",
        "__0_get_Twice__ret",
    ] {
        assert!(
            meta.contains(&format!("\"value\": \"{name}\"")),
            "{name} missing:\n{meta}"
        );
    }
    assert!(text.contains("__SetProgramVariable__"), "{text}");
    assert!(text.contains("__GetProgramVariable__"), "{text}");
    assert!(text.contains("__SendCustomEvent__"), "{text}");

    // the callee's side: the events and variables under the same names
    let Some(door) = compile_behaviour(sources(), "Game.Door") else {
        return;
    };
    assert!(door.output.errors.is_empty(), "{:#?}", door.output.errors);
    let text = door.output.program.to_uasm().unwrap();
    for event in [
        "__0_Slide",
        "Count",
        "__0_Add",
        "__0_Take",
        "get_Level",
        "__0_set_Level",
        "get_Twice",
    ] {
        assert!(
            text.contains(&format!(".export {event}\n")),
            "event {event} missing:\n{text}"
        );
    }
    for variable in [
        "__0_amount__param: %SystemInt32",
        "__0_Count__ret: %SystemInt32",
        "__0___0_Add__ret: %SystemInt32",
        "__1_amount__param: %SystemInt32",
        "__0_rest__param: %SystemInt32",
        "__0___0_Take__ret: %SystemBoolean",
        "__0_get_Level__ret: %SystemInt32",
        "__0_value__param: %SystemInt32",
        "__0_get_Twice__ret: %SystemInt32",
    ] {
        assert!(
            text.contains(variable),
            "variable {variable} missing:\n{text}"
        );
        // parameter and result variables are not public variables
        let name = variable.split(':').next().unwrap();
        assert!(
            !text.contains(&format!(".export {name}\n")),
            "{name} must not be exported:\n{text}"
        );
    }
    // the built-in event keeps its own protocol
    assert!(
        text.contains(".export _interact\n") || !text.contains("_interact"),
        "{text}"
    );
}

/// An UdonSharp behaviour, known through its source (declarations only): M#
/// code reaches its public fields, properties and methods by the names
/// UdonSharp exported them under. Its bodies are never looked at.
#[test]
fn an_udonsharp_behaviour_is_reached_by_its_export_names() {
    let sources = || {
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::foreign(
                "Assets/Vendor/UCounter.cs",
                r#"
                namespace UdonSharp { public class UdonSharpBehaviour { } }
                public enum Mode { Slow, Fast }
                public class UCounter : UdonSharp.UdonSharpBehaviour
                {
                    public int count;
                    private int hidden;
                    public Mode mode;
                    public void Bump(int by) { this body is not C# at all; }
                    public int Read() { return count; }
                    public int Level { get; set; }
                    public int Rate { get { return 1; } }
                    public static class Util { }
                }
                public static class Helper
                {
                    public static int Twice(int x) { return x * 2; }
                }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Switch.cs",
                r#"
                namespace Game
                {
                    public class Switch : MenSharp.MenSharpBehaviour
                    {
                        public UCounter counter;
                        public UCounter[] counters;
                        public void Interact()
                        {
                            counter.count = 1;
                            counter.mode = Mode.Fast;
                            counter.Bump(2);
                            int n = counter.Read();
                            counter.Level = n;
                            int total = counter.Level + counter.Rate + counters.Length;
                        }
                    }
                }
                "#,
            ),
        ]
    };
    let Some(switch) = compile_behaviour(sources(), "Game.Switch") else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        switch.output.errors.is_empty(),
        "{:#?}",
        switch.output.errors
    );
    let text = switch.output.program.to_uasm().unwrap();
    let meta = switch.output.program.to_meta_json().unwrap();
    // a reference to an UdonSharp behaviour is a program reference
    assert!(text.contains("counter: %VRCUdonUdonBehaviour"), "{text}");
    assert!(
        text.contains("counters: %VRCUdonCommonInterfacesIUdonEventReceiverArray"),
        "{text}"
    );
    for name in [
        "count",
        "mode",
        "__0_by__param",
        "__0_Bump",
        "Read",
        "__0_Read__ret",
        // every property of an UdonSharp behaviour goes through its accessors
        "get_Level",
        "__0_get_Level__ret",
        "__0_value__param",
        "__0_set_Level",
        "get_Rate",
        "__0_get_Rate__ret",
    ] {
        assert!(
            meta.contains(&format!("\"value\": \"{name}\"")),
            "{name} missing:\n{meta}"
        );
    }
    // the UdonSharp program itself is not something M# compiles
    let found = compile_behaviours(sources());
    assert_eq!(
        found,
        vec!["Game.Switch".to_string(), "MenSharp.Statics".to_string()],
        "{found:?}"
    );
}

fn exported_int(emulator: &Emulator, name: &str) -> i32 {
    match emulator.value_of(name) {
        Some(men_sharp_asm::Value::Int32(value)) => *value,
        other => panic!("{name}: {other:?}"),
    }
}

/// What an UdonSharp (library) source declares besides its behaviours is
/// compiled into the program that uses it, the way UdonSharp itself
/// inlines a helper into each behaviour that calls it — a static helper, an
/// enum, a struct: ordinary code, checked and generated on use.
#[test]
fn an_udonsharp_helper_class_is_compiled_into_the_program() {
    let Some(emulator) = run_sources(
        vec![
            SourceCode::foreign(
                "Assets/Vendor/Helper.cs",
                r#"
                public enum Speed { Slow = 1, Fast = 3 }
                public static class Helper
                {
                    public static int Twice(int x) { return x * 2; }
                    public static int Scale(int x, Speed speed) { return x * (int)speed; }
                }
                public struct Pair
                {
                    public int a;
                    public int b;
                    public int Sum() { return a + b; }
                }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Program.cs",
                r#"
                namespace Game
                {
                    public class Program
                    {
                        public static int total;
                        public static void Main()
                        {
                            Pair pair = new Pair();
                            pair.a = Helper.Twice(2);
                            pair.b = Helper.Scale(5, Speed.Fast);
                            total = pair.Sum();
                        }
                    }
                }
                "#,
            ),
        ],
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert_eq!(exported_int(&emulator, "total"), 4 + 15);
}

/// A library's errors are its own: nothing is reported for a file the user
/// did not write — until M# code uses the broken declaration, which is an
/// error there, naming the reason. The rest of the library stays usable.
#[test]
fn a_broken_library_member_is_an_error_only_when_used() {
    let library = SourceCode::foreign(
        "Assets/Vendor/Helper.cs",
        r#"
        public static class Helper
        {
            public static int Fine(int x) { return x + 1; }
            public static int Broken(int x) { return x + undefined_name; }
        }
        public class Broken2 { public int x = this is not C#; }
        "#,
    );
    // using only what works: no error at all
    let Some(emulator) = run_sources(
        vec![
            library.clone(),
            SourceCode::new(
                "Assets/MenSharp/Program.cs",
                r#"
                namespace Game
                {
                    public class Program
                    {
                        public static int total;
                        public static void Main() { total = Helper.Fine(41); }
                    }
                }
                "#,
            ),
        ],
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert_eq!(exported_int(&emulator, "total"), 42);
}

#[test]
fn a_broken_library_member_message_names_the_reason() {
    let sources = vec![
        SourceCode::foreign(
            "Assets/Vendor/Helper.cs",
            r#"
            public static class Helper
            {
                public static int Broken(int x) { return x + undefined_name; }
            }
            "#,
        ),
        SourceCode::new(
            "Assets/MenSharp/Program.cs",
            r#"
            namespace Game
            {
                public class Program
                {
                    public static int total;
                    public static void Main() { total = Helper.Broken(1); }
                }
            }
            "#,
        ),
    ];
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let mut sources = sources;
    sources.extend(Compiler::corlib_sources());
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    // the library's own error is not among the reported ones
    assert_eq!(bodies.errors, vec![], "library errors must not be reported");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    let messages: Vec<String> = output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages.iter().any(|message| message
            .contains("`Helper.Broken` is used from MenSharp code")
            && message.contains("cannot compile it")),
        "{messages:?}"
    );
}

/// `#if` is evaluated: a library file's editor-only block (the UdonSharp
/// convention `#if !COMPILER_UDONSHARP && UNITY_EDITOR`) and an UdonSharp-only
/// block are what UdonSharp's compiler would see; the user's own files see
/// `COMPILER_MENSHARP`.
#[test]
fn conditional_compilation_follows_the_defines() {
    let Some(emulator) = run_sources(
        vec![
            SourceCode::foreign(
                "Assets/Vendor/Helper.cs",
                r#"
                #if !COMPILER_UDONSHARP && UNITY_EDITOR
                using UnityEditor;
                [CustomEditor(typeof(Helper))]
                public class HelperEditor : Editor { this would not parse }
                #endif
                public static class Helper
                {
                #if COMPILER_UDONSHARP
                    public static int Value() { return 1; }
                #else
                    public static int Value() { return 2; }
                #endif
                }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Program.cs",
                r#"
                namespace Game
                {
                    public class Program
                    {
                        public static int total;
                        public static void Main()
                        {
                #if COMPILER_MENSHARP
                            total = Helper.Value() + 10;
                #else
                            total = -1;
                #endif
                        }
                    }
                }
                "#,
            ),
        ],
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert_eq!(exported_int(&emulator, "total"), 11);
}

/// A library type under a name the user's code declares too is dropped —
/// the user's wins, silently, as a library's would in any C# project.
#[test]
fn the_users_type_wins_over_a_library_type_of_the_same_name() {
    let Some(emulator) = run_sources(
        vec![
            SourceCode::foreign(
                "Assets/Vendor/Helper.cs",
                r#"
                public static class Helper { public static int Value() { return 1; } }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Program.cs",
                r#"
                public static class Helper { public static int Value() { return 2; } }
                namespace Game
                {
                    public class Program
                    {
                        public static int total;
                        public static void Main() { total = Helper.Value(); }
                    }
                }
                "#,
            ),
        ],
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert_eq!(exported_int(&emulator, "total"), 2);
}

/// A class deriving from an engine class (a plain MonoBehaviour in a
/// library, say) is neither a program nor an object Udon can hold: using it
/// as a type is an error naming the base, not a silent `object[]`.
#[test]
fn an_engine_derived_library_class_cannot_be_used_as_a_type() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::foreign(
                "Assets/Vendor/Placer.cs",
                r#"
                public class Placer : System.Collections.ArrayList { public int slot; }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Switch.cs",
                r#"
                namespace Game
                {
                    public class Switch : MenSharp.MenSharpBehaviour
                    {
                        public Placer placer;
                        public void Interact() { placer.slot = 1; }
                    }
                }
                "#,
            ),
        ],
        "Game.Switch",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("derives from `System.Collections.ArrayList`")),
        "{messages:?}"
    );
}

#[test]
fn a_private_member_of_another_behaviour_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        internal int secret;
                        public void Open() { }
                    }

                    public class Switch : MenSharp.MenSharpBehaviour
                    {
                        public Door door;
                        public void Interact() { door.secret = 1; }
                    }
                }
                "#,
            ),
        ],
        "Game.Switch",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("is not public")),
        "{messages:?}"
    );
}

#[test]
fn synced_variables_get_a_sync_directive() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    [UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
                    public class Counter : MenSharp.MenSharpBehaviour
                    {
                        [UdonSynced] public int total;
                        [UdonSynced(UdonSyncMode.Linear)] public float dial;
                        public int local;
                        public void Interact() { total = total + 1; }
                        public void OnDeserialization() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Counter",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );

    let text = program.output.program.to_uasm().unwrap();
    // exported *and* synced: the inspector needs the first, the network the
    // second, and a public synced variable needs both
    assert!(
        text.contains("    .export total\n    .sync total, none"),
        "{text}"
    );
    assert!(text.contains("    .sync dial, linear"), "{text}");
    assert!(!text.contains(".sync local"), "{text}");

    // OnDeserialization is an Udon event, not a custom one
    assert!(text.contains(".export _onDeserialization"), "{text}");

    // and the behaviour-wide mode rides in the sidecar, since it is a setting
    // on the component rather than part of the program
    let meta = program.output.program.to_meta_json().unwrap();
    assert!(meta.contains("\"syncMode\": \"manual\""), "{meta}");
}

#[test]
fn an_unknown_on_method_stays_a_custom_event() {
    // rewriting every `OnSomething` into `_onSomething` would invent events
    // Udon never raises
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Bell : MenSharp.MenSharpBehaviour
                    {
                        public void OnMyOwnThing() { }
                        public void OnPlayerJoined() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Bell",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let events: Vec<&str> = program
        .output
        .program
        .entry_points
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert!(events.contains(&"OnMyOwnThing"), "{events:?}");
    assert!(events.contains(&"_onPlayerJoined"), "{events:?}");
}

#[test]
fn attributes_that_would_change_behaviour_are_errors() {
    // `[UdonSynced]` used to be dropped on the floor: the program compiled,
    // ran, and simply never synchronised
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Counter : MenSharp.MenSharpBehaviour
                    {
                        [FieldChangeCallback(nameof(total))] public int total;
                        [Header("looks only")] public int shown;
                        public void Interact() { total = total + 1; }
                    }
                }
                "#,
            ),
        ],
        "Game.Counter",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("[FieldChangeCallback]")),
        "{messages:?}"
    );
    // cosmetic attributes stay silent: ignoring them costs nothing
    assert!(
        !messages.iter().any(|message| message.contains("[Header]")),
        "{messages:?}"
    );
}

/// Unity's managed assemblies, when this machine has them. The generic
/// externs all live on Unity types, so a test for them cannot run without.
fn unity_managed_dir() -> Option<std::path::PathBuf> {
    let hub = std::path::Path::new(&std::env::var("HOME").ok()?).join("Unity/Hub/Editor");
    let mut versions: Vec<_> = std::fs::read_dir(hub).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    for version in versions {
        let managed = version.path().join("Editor/Data/Managed");
        if managed
            .join("UnityEngine/UnityEngine.CoreModule.dll")
            .exists()
        {
            return Some(managed);
        }
    }
    None
}

#[test]
fn a_generic_extern_passes_its_type_as_a_value() {
    // Udon has no generics: `GetComponent<T>` is one extern named `…__T` that
    // takes typeof(T) as an ordinary parameter, after the receiver
    let (Some(dotnet), Some(unity)) = (dotnet_shared_dir(), unity_managed_dir()) else {
        eprintln!("skipped: needs both a .NET runtime and a Unity install");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dotnet.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(unity.join("UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        namespace Game
        {
            public class Bouncer : MenSharpBehaviour
            {
                public void Interact()
                {
                    UnityEngine.Transform found = gameObject.GetComponent<UnityEngine.Transform>();
                    if (found == null) { return; }
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources_for(&references));
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program = programs
        .iter()
        .find(|program| program.class_path == "Game.Bouncer")
        .expect("Game.Bouncer");
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );

    let text = program.output.program.to_uasm().unwrap();
    assert!(
        text.contains("EXTERN, \"UnityEngineGameObject.__GetComponent__T\""),
        "{text}"
    );
    // `Transform == null` binds to the operator UnityEngine.Object declares,
    // which is only found by looking up the base chain — and it is the one
    // that knows about destroyed objects, so the exact extern matters
    assert!(
        text.contains(
            "EXTERN, \"UnityEngineObject.__op_Equality__UnityEngineObject_\
             UnityEngineObject__SystemBoolean\""
        ),
        "{text}"
    );
    let meta = program.output.program.to_meta_json().unwrap();
    assert!(
        meta.contains("\"kind\": \"Type\", \"value\": \"UnityEngine.Transform\""),
        "{meta}"
    );
}

#[test]
fn a_type_constant_carries_its_dotnet_name() {
    // only the Unity importer can make a real System.Type, so what the
    // sidecar carries is the name it resolves
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Probe : MenSharp.MenSharpBehaviour
                    {
                        public object held;
                        public void Interact() { held = typeof(string); }
                    }
                }
                "#,
            ),
        ],
        "Game.Probe",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let meta = program.output.program.to_meta_json().unwrap();
    assert!(
        meta.contains("\"kind\": \"Type\", \"value\": \"System.String\""),
        "{meta}"
    );
}

#[test]
fn only_public_fields_become_public_variables() {
    // a private field is an implementation detail; exporting it would put it
    // in the UdonBehaviour's variable table, where the inspector — or anything
    // on the network — can write it
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Spawner : MenSharp.MenSharpBehaviour
                    {
                        public int shown;
                        private int hidden;
                        public void Interact() { hidden = hidden + 1; shown = hidden; }
                    }
                }
                "#,
            ),
        ],
        "Game.Spawner",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    assert!(text.contains("    .export shown"), "{text}");
    assert!(!text.contains(".export hidden"), "{text}");
    // it still has storage, it is just not part of the surface
    assert!(text.contains("hidden: %SystemInt32"), "{text}");
}

#[test]
fn out_arguments_reach_an_extern_and_come_back() {
    // An extern takes every parameter by heap address, so a variable's own
    // slot *is* the reference: `int.TryParse(s, out n)` has the extern write
    // straight into `n`. All three call-site spellings land in the same place.
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int direct;
                public static int declared;
                public static int inferred;
                public static bool ok;
                public static bool bad;
                public static void Main()
                {
                    int n;
                    ok = int.TryParse("40", out n);
                    direct = n;
                    if (int.TryParse("1", out int m)) { declared = m; }
                    bad = int.TryParse("not a number", out var broken);
                    inferred = broken + 1;
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "direct"), 40);
    assert_eq!(int_of(&emulator, "declared"), 1);
    // TryParse leaves 0 behind on failure
    assert_eq!(int_of(&emulator, "inferred"), 1);
    assert!(matches!(
        emulator.value_of("ok"),
        Some(Value::Boolean(true))
    ));
    assert!(matches!(
        emulator.value_of("bad"),
        Some(Value::Boolean(false))
    ));
}

#[test]
fn an_out_argument_writes_back_into_an_array_element() {
    // an array element has no heap slot of its own, so the extern writes a
    // stand-in temporary which is then copied home
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    int[] numbers = new int[2];
                    int.TryParse("42", out numbers[1]);
                    result = numbers[1];
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 42);
}

#[test]
fn an_out_argument_to_a_source_method_just_works() {
    // used to be a compile error; static frames make it a copy-back
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                private static void Fill(out int x) { x = 1; }
                public static void Main() { int n; Fill(out n); result = n; }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 1);
}

#[test]
fn an_out_argument_of_the_wrong_type_does_not_resolve() {
    // `ref`/`out` write through the reference: no conversion may sit in
    // between, so `out float` never matches `out int` (§12.6.4.2)
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static void Main()
                {
                    float wrong;
                    int.TryParse("1", out wrong);
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert!(!bodies.errors.is_empty(), "out float matched out int");
}

#[test]
fn a_byref_extern_parameter_is_spelled_with_a_ref_suffix() {
    // `Physics.Raycast(ray, out hit)`: the whitelist spells the out parameter
    // with a `Ref` suffix — UnityEngineRaycastHitRef — and the pushed slot is
    // the destination variable itself
    let (Some(dotnet), Some(unity)) = (dotnet_shared_dir(), unity_managed_dir()) else {
        eprintln!("skipped: needs both a .NET runtime and a Unity install");
        return;
    };
    let physics = unity.join("UnityEngine/UnityEngine.PhysicsModule.dll");
    if !physics.exists() {
        eprintln!("skipped: no UnityEngine.PhysicsModule.dll");
        return;
    }
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dotnet.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(unity.join("UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
        std::fs::read(physics).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        using UnityEngine;
        namespace Game
        {
            public class Scanner : MenSharpBehaviour
            {
                public Ray ray;
                public bool hitSomething;
                public float distance;

                public void Interact()
                {
                    if (Physics.Raycast(ray, out RaycastHit hit))
                    {
                        hitSomething = true;
                        distance = hit.distance;
                    }
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources_for(&references));
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program = programs
        .iter()
        .find(|program| program.class_path == "Game.Scanner")
        .expect("Game.Scanner");
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    assert!(
        text.contains(
            "EXTERN, \"UnityEnginePhysics.__Raycast__UnityEngineRay_\
             UnityEngineRaycastHitRef__SystemBoolean\""
        ),
        "{text}"
    );
}

#[test]
fn a_built_in_event_receives_its_arguments() {
    // Before raising `_midiNoteOn`, the runtime writes each argument into the
    // slot named after event and parameter (midiNoteOnChannel, ...); the
    // entry stub hands them to the method as its parameters.
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static int seenChannel;
                public static int seenNumber;
                public static int seenVelocity;

                public static void MidiNoteOn(int channel, int number, int velocity)
                {
                    seenChannel = channel;
                    seenNumber = number;
                    seenVelocity = velocity;
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );

    let text = output.program.to_uasm().unwrap();
    assert!(text.contains(".export _midiNoteOn"), "{text}");
    assert!(text.contains("midiNoteOnChannel: %SystemInt32"), "{text}");

    let assembled = output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&output.program, &assembled);
    assert!(emulator.set_value("midiNoteOnChannel", Value::Int32(9)));
    assert!(emulator.set_value("midiNoteOnNumber", Value::Int32(60)));
    assert!(emulator.set_value("midiNoteOnVelocity", Value::Int32(127)));
    emulator
        .run(&assembled, "_midiNoteOn")
        .unwrap_or_else(|error| panic!("emulator error: {error:?}\n{}", output.program.dump()));
    assert_eq!(int_of(&emulator, "seenChannel"), 9);
    assert_eq!(int_of(&emulator, "seenNumber"), 60);
    assert_eq!(int_of(&emulator, "seenVelocity"), 127);
}

#[test]
fn an_event_with_the_wrong_parameters_is_an_error() {
    // Unity fires an event named like a built-in regardless of its parameter
    // list, so a mismatch would run with unset values — U# refuses it, and so
    // does this compiler
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static void MidiNoteOn(int channel) { }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.iter().any(|error| error
            .message
            .to_string()
            .contains("built-in event `_midiNoteOn`")),
        "{:#?}",
        output.errors
    );
}

#[test]
fn a_parameterized_method_is_not_an_event() {
    // no event can carry arguments to it, so exporting it would run the body
    // with the parameters never written — it stays an internal function
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public int total;
                        public void Interact() { TakeDamage(5); }
                        public void TakeDamage(int amount) { total += amount; }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    assert!(text.contains(".export _interact"), "{text}");
    assert!(!text.contains(".export TakeDamage"), "{text}");
}

#[test]
fn serialize_field_exports_a_private_field() {
    // Unity's own serialization rule, which is also what UdonSharp exports:
    // `public` opts in, `[SerializeField]` opts a private field in,
    // `[NonSerialized]` opts a public field out
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Spawner : MenSharp.MenSharpBehaviour
                    {
                        [SerializeField] private int hidden = 3;
                        [NonSerialized] public int scratch;
                        public int shown;
                        public void Interact() { shown = hidden + scratch; }
                    }
                }
                "#,
            ),
        ],
        "Game.Spawner",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    assert!(text.contains("    .export hidden"), "{text}");
    assert!(text.contains("    .export shown"), "{text}");
    assert!(!text.contains(".export scratch"), "{text}");
}

#[test]
fn a_field_change_callback_routes_the_write_through_the_setter() {
    // the runtime writes the new value into the field, the previous one into
    // `_old_<slot>`, and raises `_onVarChange_<slot>`; the stub hands the new
    // value to the property setter with the field restored to its old value
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        [SerializeField] [FieldChangeCallback(nameof(Level))]
                        private int _level;
                        public int delta;

                        public int Level
                        {
                            get { return _level; }
                            set { delta = value - _level; _level = value; }
                        }

                        public void Interact() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    assert!(text.contains(".export _onVarChange__level"), "{text}");
    assert!(text.contains("_old__level: %SystemInt32"), "{text}");

    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    // what the runtime does before raising the event
    assert!(emulator.set_value("_level", Value::Int32(7)));
    assert!(emulator.set_value("_old__level", Value::Int32(3)));
    emulator
        .run(&assembled, "_onVarChange__level")
        .unwrap_or_else(|error| {
            panic!(
                "emulator error: {error:?}\n{}",
                program.output.program.dump()
            )
        });
    // delta = new - old proves the setter saw the field's OLD value
    assert_eq!(int_of(&emulator, "delta"), 4);
    assert_eq!(int_of(&emulator, "_level"), 7);
}

#[test]
fn a_field_change_callback_without_the_property_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        [FieldChangeCallback("Missing")] public int value;
                        public void Interact() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.iter().any(|error| error
            .message
            .to_string()
            .contains("no property of that name")),
        "{:#?}",
        program.output.errors
    );
}

#[test]
fn a_function_that_calls_out_gets_a_reentry_guard() {
    // no analysis of THIS program can see a SendCustomEvent round trip that
    // comes back through another program — so a function that can be live
    // during such a call carries a runtime guard: re-entering it logs an
    // error and aborts instead of silently corrupting its frame
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "test.cs",
                r#"
                namespace Game
                {
                    public class Other : MenSharp.MenSharpBehaviour
                    {
                        public void Poke() { }
                    }

                    public class Door : MenSharp.MenSharpBehaviour
                    {
                        public Other other;
                        public bool fire;
                        public int count;

                        public void Interact()
                        {
                            if (fire) { other.Poke(); }
                            count = count + 1;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    let flag = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("__active_"))
        .and_then(|line| line.split(':').next())
        .expect("a guard flag in the data section")
        .to_string();

    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    // ordinary dispatches pass the guard and clear it again
    emulator.run(&assembled, "_interact").unwrap();
    emulator.run(&assembled, "_interact").unwrap();
    assert_eq!(int_of(&emulator, "count"), 2);
    // what the runtime state looks like mid-flight: the function is active
    assert!(emulator.set_value(&flag, Value::Boolean(true)));
    emulator.run(&assembled, "_interact").unwrap();
    assert_eq!(int_of(&emulator, "count"), 2, "the body must not have run");
    assert!(
        emulator.log.iter().any(|line| line.contains("re-entered")),
        "{:?}",
        emulator.log
    );
}

#[test]
fn enums_work_as_values() {
    // a source enum is its underlying Int32: members are baked constants,
    // comparisons are Int32 externs, casts to and from int are no-ops
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public enum DoorState { Closed, Open, Locked = 10 }

            public class Program
            {
                public static int result;
                public static DoorState state;

                public static void Main()
                {
                    state = DoorState.Open;
                    if (state == DoorState.Open) { result = 1; }
                    int raw = (int)state;
                    DoorState back = (DoorState)raw;
                    if (back != DoorState.Closed) { result = result + 2; }
                    result = result + (int)DoorState.Locked;
                    if (DoorState.Closed < DoorState.Open) { result = result + 100; }
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 113);
    assert_eq!(int_of(&emulator, "state"), 1);
}

#[test]
fn switch_over_enum_and_int() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public enum DoorState { Closed, Open, Locked = 10 }

            public class Program
            {
                public static int byEnum;
                public static int byDefault;
                public static int byInt;

                public static void Main()
                {
                    DoorState state = DoorState.Open;
                    switch (state)
                    {
                        case DoorState.Closed:
                            byEnum = 100;
                            break;
                        case DoorState.Open:
                            byEnum = 200;
                            break;
                        default:
                            byEnum = 300;
                            break;
                    }
                    switch (DoorState.Locked)
                    {
                        case DoorState.Closed:
                        case DoorState.Open:
                            byDefault = 1;
                            break;
                        default:
                            byDefault = 2;
                            break;
                    }
                    switch (7)
                    {
                        case 3:
                            byInt = 30;
                            break;
                        case 7:
                            byInt = 70;
                            break;
                    }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "byEnum"), 200);
    assert_eq!(int_of(&emulator, "byDefault"), 2);
    assert_eq!(int_of(&emulator, "byInt"), 70);
}

#[test]
fn break_leaves_the_switch_and_continue_still_reaches_the_loop() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int total;

                public static void Main()
                {
                    for (int i = 0; i < 5; i++)
                    {
                        switch (i)
                        {
                            case 2:
                                total = total + 10;
                                break;
                            case 3:
                                continue;
                            default:
                                total = total + 1;
                                break;
                        }
                        total = total + 100;
                    }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    // i=0,1,4: +1+100 each; i=2: +10+100; i=3: continue skips the +100
    assert_eq!(int_of(&emulator, "total"), 413);
}

#[test]
fn external_const_fields_are_baked_values() {
    // `int.MaxValue` has no extern — it is a metadata constant, and now a
    // baked heap value
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                public static void Main() { result = int.MaxValue; }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), i32::MAX);
}

#[test]
fn external_enums_are_boxed_constants() {
    // `KeyCode.Space` has no extern either: its value comes from metadata,
    // and the heap slot must hold the real boxed enum (an Int32 would throw
    // when an extern unboxes it), so the sidecar carries type and value
    let (Some(dotnet), Some(unity)) = (dotnet_shared_dir(), unity_managed_dir()) else {
        eprintln!("skipped: needs both a .NET runtime and a Unity install");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dotnet.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(unity.join("UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        using UnityEngine;
        namespace Game
        {
            public class Keys : MenSharpBehaviour
            {
                public KeyCode held;
                public int seen;

                public void Interact()
                {
                    held = KeyCode.Space;
                    if (held == KeyCode.Space) { seen = 1; }
                    switch (held)
                    {
                        case KeyCode.A:
                            seen = 10;
                            break;
                        case KeyCode.Space:
                            seen = seen + 20;
                            break;
                    }
                    if (held != KeyCode.A) { seen = seen + 100; }
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources_for(&references));
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program = programs
        .iter()
        .find(|program| program.class_path == "Game.Keys")
        .expect("Game.Keys");
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let meta = program.output.program.to_meta_json().unwrap();
    // KeyCode.Space == 32
    assert!(
        meta.contains("\"kind\": \"Enum\", \"value\": \"UnityEngine.KeyCode#32\""),
        "{meta}"
    );

    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    emulator
        .run(&assembled, "_interact")
        .unwrap_or_else(|error| {
            panic!(
                "emulator error: {error:?}\n{}",
                program.output.program.dump()
            )
        });
    assert_eq!(int_of(&emulator, "seen"), 121);
}

#[test]
fn source_methods_take_out_and_ref_arguments() {
    // static frames make this direct: the callee writes its own parameter
    // slot, and the call copies it back into the argument's place
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int high;
                public static int low;
                public static bool ok;
                public static int bumped;

                private static bool Split(int value, out int tens, out int ones)
                {
                    tens = value / 10;
                    ones = value % 10;
                    return value >= 10;
                }

                private static void Bump(ref int x)
                {
                    x = x + 1;
                }

                public static void Main()
                {
                    ok = Split(42, out int h, out int l);
                    high = h;
                    low = l;

                    int[] numbers = new int[2];
                    numbers[1] = 7;
                    Bump(ref numbers[1]);
                    bumped = numbers[1];
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "high"), 4);
    assert_eq!(int_of(&emulator, "low"), 2);
    assert!(matches!(
        emulator.value_of("ok"),
        Some(Value::Boolean(true))
    ));
    assert_eq!(int_of(&emulator, "bumped"), 8);
}

#[test]
fn out_arguments_survive_recursion() {
    // the write-back rescues the callee's parameter into a scratch slot
    // before the recursive frame restore rewinds it — this is the ordering
    // that breaks if the write-back moves to either side of the restore
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;

                private static void Count(int n, out int total)
                {
                    if (n == 0) { total = 0; return; }
                    Count(n - 1, out int rest);
                    total = rest + n;
                }

                public static void Main()
                {
                    Count(5, out int sum);
                    result = sum;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 15);
}

#[test]
fn out_arguments_cross_virtual_dispatch() {
    // the override writes its own slot; the dispatcher hands the value back
    // through its slot; the caller's write-back reads it from there
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Source
            {
                public virtual bool TryGet(out int value) { value = 1; return true; }
            }

            public class Doubled : Source
            {
                public override bool TryGet(out int value) { value = 2; return true; }
            }

            public class Program
            {
                public static int result;
                public static void Main()
                {
                    Source source = new Doubled();
                    if (source.TryGet(out int value)) { result = value; }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 2);
}

#[test]
fn generic_out_parameters_monomorphize() {
    // the TryGetComponent shape: a generic method whose out parameter's type
    // is the method's own type parameter
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int number;
                public static string text;

                private static bool First<T>(T[] items, out T first)
                {
                    first = items[0];
                    return items.Length > 0;
                }

                public static void Main()
                {
                    int[] numbers = new int[2];
                    numbers[0] = 7;
                    numbers[1] = 8;
                    if (First(numbers, out int n)) { number = n; }
                    string[] words = new string[1];
                    words[0] = "hi";
                    if (First(words, out string w)) { text = w; }
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "number"), 7);
    assert_eq!(string_of(&emulator, "text"), "hi");
}

#[test]
fn array_initializers_and_default_expressions() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public enum Mood { Sad, Happy }

            public class Program
            {
                public static int sum;
                public static int sized;
                public static string joined;
                public static int zero;
                public static bool no;
                public static string none;
                public static int moodZero;

                private static T Pick<T>(T[] items, int index)
                {
                    return items[index];
                }

                public static void Main()
                {
                    // explicit element type, size from the element count
                    int[] numbers = new int[] { 7, 8, 9 };
                    sum = numbers[0] + numbers[1] + numbers[2];

                    // written size plus initializer
                    int[] pair = new int[2] { 40, 2 };
                    sized = pair[0] + pair[1];

                    // element type inferred from the elements
                    var words = new[] { "a", "b" };
                    joined = words[0] + words[1];

                    // bare `default` takes its type from the context
                    int i = default;
                    zero = i;
                    bool flag = default;
                    no = flag;
                    none = default(string);
                    Mood mood = default;
                    moodZero = (int)mood;

                    // and through a generic, where T decides what it means
                    sum = sum + Pick(numbers, 1);
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "sum"), 24 + 8);
    assert_eq!(int_of(&emulator, "sized"), 42);
    assert_eq!(string_of(&emulator, "joined"), "ab");
    assert_eq!(int_of(&emulator, "zero"), 0);
    assert!(matches!(
        emulator.value_of("no"),
        Some(Value::Boolean(false))
    ));
    assert!(matches!(emulator.value_of("none"), Some(Value::Null)));
    assert_eq!(int_of(&emulator, "moodZero"), 0);
}

#[test]
fn foreach_walks_a_list_a_string_and_a_user_enumerator() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            // the enumerator pattern on a class of your own: no interface,
            // no List — foreach binds to GetEnumerator/MoveNext/Current
            public class Countdown
            {
                private int from;
                public Countdown(int from) { this.from = from; }
                public Ticker GetEnumerator() { return new Ticker(from); }
            }
            public class Ticker
            {
                private int next;
                public Ticker(int from) { next = from + 1; }
                public bool MoveNext() { next--; return next > 0; }
                public int Current { get { return next; } }
            }

            public class Program
            {
                public static int sum;
                public static int vowels;
                public static int countdown;
                public static int nested;
                public static int skipped;
                public static string joined;

                public static void Main()
                {
                    var numbers = new List<int>();
                    for (int i = 1; i <= 5; i++) { numbers.Add(i); }

                    // 1+2+3+4+5
                    foreach (var n in numbers) { sum += n; }

                    // continue skips 3, break stops at 5: 1+2+4 = 7
                    foreach (int n in numbers)
                    {
                        if (n == 3) { continue; }
                        if (n == 5) { break; }
                        skipped += n;
                    }

                    // strings iterate by char
                    foreach (char c in "education")
                    {
                        if (c == 'a' || c == 'e' || c == 'i' || c == 'o' || c == 'u') { vowels++; }
                    }

                    // 3+2+1
                    foreach (var t in new Countdown(3)) { countdown += t; }

                    // nested foreach over the same list: 5 * 15 = 75
                    foreach (var a in numbers)
                    {
                        foreach (var b in numbers) { nested += b; }
                    }

                    var words = new List<string>();
                    words.Add("for");
                    words.Add("each");
                    joined = "";
                    foreach (var w in words) { joined = joined + w; }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "sum"), 15);
    assert_eq!(int_of(&emulator, "skipped"), 7);
    assert_eq!(int_of(&emulator, "vowels"), 5);
    assert_eq!(int_of(&emulator, "countdown"), 6);
    assert_eq!(int_of(&emulator, "nested"), 75);
    assert_eq!(string_of(&emulator, "joined"), "foreach");
}

#[test]
fn foreach_survives_recursion() {
    // the enumerator lives in a temp of the recursing function's frame, so an
    // inner activation walking the same list must not disturb the outer walk
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static int result;
                static List<int> items;

                // depth 0: 1+2+3 = 6; depth 1: three times that = 18; depth 2: 54
                static int Walk(int depth)
                {
                    int total = 0;
                    foreach (var x in items)
                    {
                        if (depth == 0) { total += x; }
                        else { total += Walk(depth - 1); }
                    }
                    return total;
                }

                public static void Main()
                {
                    items = new List<int>();
                    items.Add(1);
                    items.Add(2);
                    items.Add(3);
                    result = Walk(2);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 54);
}

#[test]
fn foreach_over_something_without_an_enumerator_is_an_error() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Bag { public int Count; }
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    foreach (var x in new Bag()) { result += 1; }
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert!(
        bodies.errors.iter().any(|error| matches!(
            error.kind,
            men_sharp_semantics::SemanticErrorKind::NotEnumerable { .. }
        )),
        "{:#?}",
        bodies.errors
    );
}

#[test]
fn collection_initializers_are_add_calls() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            // any type with an `Add` takes a collection initializer; a
            // `{ k, v }` element picks the two-argument overload
            public class Pairs
            {
                public int keys;
                public int values;
                public void Add(int key, int value) { keys += key; values += value; }
                public void Add(int both) { keys += both; values += both; }
            }

            public class Program
            {
                public static int sum;
                public static int count;
                public static string joined;
                public static int keys;
                public static int values;
                static List<int> field = new List<int> { 10, 20 };

                public static void Main()
                {
                    var names = new List<string>
                    {
                        "a",
                        "b"
                    };
                    joined = "";
                    foreach (var name in names) { joined = joined + name; }
                    count = names.Count;

                    var numbers = new List<int> { 1, 2, 3 };
                    foreach (var n in numbers) { sum += n; }
                    foreach (var n in field) { sum += n; }

                    var pairs = new Pairs { { 1, 100 }, 5, { 2, 200 } };
                    keys = pairs.keys;
                    values = pairs.values;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "joined"), "ab");
    assert_eq!(int_of(&emulator, "count"), 2);
    assert_eq!(int_of(&emulator, "sum"), 36);
    assert_eq!(int_of(&emulator, "keys"), 8);
    assert_eq!(int_of(&emulator, "values"), 305);
}

#[test]
fn a_collection_initializer_needs_a_matching_add() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Bag { }
            public class Program
            {
                public static void Main()
                {
                    var a = new Bag { 1 };
                    var b = new List<int> { "not an int" };
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let kinds: Vec<_> = bodies.errors.iter().map(|error| &error.kind).collect();
    assert_eq!(kinds.len(), 2, "{kinds:#?}");
    assert!(
        matches!(
            kinds[0],
            men_sharp_semantics::SemanticErrorKind::UnknownMember { .. }
        ),
        "{kinds:#?}"
    );
    assert!(
        matches!(
            kinds[1],
            men_sharp_semantics::SemanticErrorKind::NoMatchingOverload
        ),
        "{kinds:#?}"
    );
}

#[test]
fn object_initializers_set_fields_and_properties() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Box
            {
                public int width;
                public int height = 1;
                private int depth;
                // a property with a real setter body, not just an auto-property
                public int Depth
                {
                    get { return depth; }
                    set { depth = value * 2; }
                }
                public int Volume => width * height * depth;
            }

            public class Program
            {
                public static int volume;
                public static int untouched;

                public static void Main()
                {
                    var box = new Box { width = 3, Depth = 5 };
                    volume = box.Volume;          // 3 * 1 * 10
                    untouched = new Box { width = 7 }.height;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "volume"), 30);
    assert_eq!(int_of(&emulator, "untouched"), 1);
}

#[test]
fn engine_structs_take_initializers_field_writes_and_default() {
    // `new Vector3 { x = 1f }`, `new Vector3()`, `v.x = 5f`, `default`:
    // all on an engine struct, which Udon spells differently from a class
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let Some(managed) = unity_managed_dir() else {
        eprintln!("skipped: no Unity editor on this machine");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(managed.join("UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        using MenSharp;
        using UnityEngine;
        namespace Game
        {
            public class Mover : MenSharpBehaviour
            {
                public float result;
                public void Interact()
                {
                    var a = new Vector3 { x = 1f, y = 2f };
                    var b = new Vector3(1f, 2f, 3f) { z = 9f };
                    var c = new Vector3();
                    Vector3 d = default;
                    a.x = 5f;
                    result = a.x + b.z + c.y + d.z;
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources_for(&references));
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");

    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program = &programs[0].output;
    assert!(
        program.errors.is_empty(),
        "codegen errors: {:#?}",
        program.errors
    );

    let text = program.program.to_uasm().unwrap();
    // struct field setters carry no `__SystemVoid`
    assert!(
        text.contains("\"UnityEngineVector3.__set_x__SystemSingle\""),
        "{text}"
    );
    assert!(
        text.contains("\"UnityEngineVector3.__set_z__SystemSingle\""),
        "{text}"
    );
    assert!(
        !text.contains("__set_x__SystemSingle__SystemVoid"),
        "{text}"
    );
    // `new Vector3()` / `default` are a struct-typed slot, never a null object
    assert!(
        text.contains("__const_0_UnityEngineVector3: %UnityEngineVector3, null"),
        "{text}"
    );
    // the default constant is only ever copied from; the writes land on a copy
    let pushes_of_default = text.matches("PUSH, __const_0_UnityEngineVector3").count();
    assert_eq!(pushes_of_default, 3, "{text}"); // a, c, d
}

#[test]
fn the_corlib_dictionary_works_end_to_end() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static int count;
                public static int found;
                public static int missing;
                public static int removed;
                public static int reAdded;
                public static int pairSum;
                public static string keysJoined;
                public static int valueSum;
                public static int grown;
                public static int intKeyed;
                public static bool hasValue;

                public static void Main()
                {
                    var ages = new Dictionary<string, int>
                    {
                        { "ann", 30 },      // Add(k, v)
                        { "bob", 41 },
                    };
                    // the index form is an object initializer (C# does not
                    // let the two mix in one pair of braces)
                    var seed = new Dictionary<string, int> { ["cy"] = 52 };
                    ages.Add("cy", seed["cy"]);
                    ages["dee"] = 63;
                    ages["ann"] = 31;       // overwrite, not a new entry
                    count = ages.Count;     // 4

                    found = ages["cy"];     // 52
                    missing = ages.ContainsKey("zed") ? 1 : 0;   // 0
                    int age;
                    if (ages.TryGetValue("bob", out age)) { found += age; }   // 93
                    hasValue = ages.ContainsValue(63);

                    removed = ages.Remove("bob") ? ages.Count : -1;   // 3
                    ages["bob"] = 42;       // reuses the freed entry
                    reAdded = ages["bob"] + ages.Count;   // 46

                    foreach (var pair in ages) { pairSum += pair.Value; }   // 31+52+63+42 = 188
                    keysJoined = "";
                    foreach (var key in ages.Keys) { keysJoined = keysJoined + key; }
                    foreach (var v in ages.Values) { valueSum += v; }   // 188

                    // growth: far past the initial capacity, every key still found
                    var squares = new Dictionary<int, int>();
                    for (int i = 0; i < 100; i++) { squares[i] = i * i; }
                    for (int i = 0; i < 100; i++) { grown += squares[i]; }   // 328350
                    intKeyed = squares.Count;   // 100
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "count"), 4);
    assert_eq!(int_of(&emulator, "found"), 93);
    assert_eq!(int_of(&emulator, "missing"), 0);
    assert!(matches!(
        emulator.value_of("hasValue"),
        Some(Value::Boolean(true))
    ));
    assert_eq!(int_of(&emulator, "removed"), 3);
    assert_eq!(int_of(&emulator, "reAdded"), 46);
    assert_eq!(int_of(&emulator, "pairSum"), 188);
    // the re-added "bob" took the freed entry back, so it enumerates where it
    // was — the same order the real Dictionary gives
    assert_eq!(string_of(&emulator, "keysJoined"), "annbobcydee");
    assert_eq!(int_of(&emulator, "valueSum"), 188);
    assert_eq!(int_of(&emulator, "grown"), 328350);
    assert_eq!(int_of(&emulator, "intKeyed"), 100);
}

#[test]
fn the_corlib_list_covers_the_rest_of_the_api() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static string inserted;
                public static string ranged;
                public static string removed;
                public static string reversed;
                public static string found;
                public static string searched;
                public static string copied;
                public static int capacity;
                public static string mutated;

                static string Join(List<int> list)
                {
                    string text = "";
                    foreach (int n in list) { text = text + n + ","; }
                    return text;
                }

                public static void Main()
                {
                    var list = new List<int> { 10, 20, 30 };
                    list.Insert(0, 5);              // 5,10,20,30
                    list.Insert(4, 40);             // append through Insert
                    list.InsertRange(2, new int[] { 11, 12 });   // 5,10,11,12,20,30,40
                    list.InsertRange(7, list);      // itself: doubled
                    inserted = Join(list) + list.Count;

                    var range = list.GetRange(2, 3);    // 11,12,20
                    ranged = Join(range) + range.Count;

                    list.RemoveRange(7, 7);         // back to the first half
                    bool gone = list.Remove(11);    // 5,10,12,20,30,40
                    bool absent = list.Remove(99);
                    removed = Join(list) + gone + absent;

                    list.Reverse();                 // 40,30,20,12,10,5
                    list.Reverse(1, 3);             // 40,12,20,30,10,5
                    reversed = Join(list);

                    found = list.IndexOf(20) + " " + list.IndexOf(20, 3) + " " + list.LastIndexOf(40)
                        + " " + list.FindLast(n => n < 15) + " " + list.FindLastIndex(n => n > 25)
                        + " " + Join(list.FindAll(n => n % 20 == 0)) + " " + list.FindIndex(2, n => n == 10);

                    list.Sort();                    // 5,10,12,20,30,40
                    searched = list.BinarySearch(20) + " " + list.BinarySearch(21) + " " + list.BinarySearch(1);

                    var target = new int[8];
                    list.CopyTo(target, 1);
                    list.CopyTo(4, target, 7, 1);
                    copied = "";
                    foreach (int n in target) { copied = copied + n + ","; }

                    var sized = new List<int>(3);
                    int before = sized.Capacity;
                    sized.EnsureCapacity(10);
                    int ensured = sized.Capacity;
                    sized.Add(1);
                    sized.TrimExcess();
                    capacity = before * 10000 + ensured * 100 + sized.Capacity;

                    // clearing then refilling reuses the storage
                    list.Clear();
                    list.Add(7);
                    mutated = Join(list) + list.Count;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "inserted"),
        "5,10,11,12,20,30,40,5,10,11,12,20,30,40,14"
    );
    assert_eq!(string_of(&emulator, "ranged"), "11,12,20,3");
    assert_eq!(
        string_of(&emulator, "removed"),
        "5,10,12,20,30,40,TrueFalse"
    );
    assert_eq!(string_of(&emulator, "reversed"), "40,12,20,30,10,5,");
    // `~low` for a missing 21 (between 20 at 3 and 30 at 4) is ~4 = -5; for
    // 1 it is ~0 = -1
    assert_eq!(string_of(&emulator, "found"), "2 -1 0 5 3 40,20, 4");
    assert_eq!(string_of(&emulator, "searched"), "3 -5 -1");
    assert_eq!(string_of(&emulator, "copied"), "0,5,10,12,20,30,40,30,");
    assert_eq!(int_of(&emulator, "capacity"), 3 * 10000 + 10 * 100 + 1);
    assert_eq!(string_of(&emulator, "mutated"), "7,1");
}

#[test]
fn the_corlib_hashset_works_end_to_end() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Program
            {
                public static string added;
                public static string walked;
                public static string sets;
                public static string queries;
                public static string nulls;
                public static string grown;
                public static string linq;

                static string Join(HashSet<int> set)
                {
                    string text = "";
                    foreach (int n in set) { text = text + n + ","; }
                    return text;
                }

                public static void Main()
                {
                    var set = new HashSet<int> { 3, 1, 2 };
                    bool fresh = set.Add(4);
                    bool dup = set.Add(3);
                    bool had = set.Remove(1);
                    bool lacked = set.Remove(9);
                    added = set.Count + " " + fresh + dup + had + lacked + " " + set.Contains(2) + set.Contains(1);

                    set.Add(5);     // takes the slot 1 freed
                    walked = Join(set);

                    var a = new HashSet<int>(new int[] { 1, 2, 3, 4 });
                    var b = new HashSet<int>(new int[] { 3, 4, 5, 5 });
                    a.UnionWith(b);
                    string union = Join(a);
                    a.IntersectWith(new int[] { 2, 3, 4, 5, 6 });
                    string intersect = Join(a);
                    a.ExceptWith(new int[] { 3 });
                    string except = Join(a);
                    a.SymmetricExceptWith(new int[] { 4, 7, 7 });
                    string symmetric = Join(a);
                    a.UnionWith(a);
                    a.IntersectWith(a);
                    string self = Join(a);
                    a.ExceptWith(a);
                    sets = union + " " + intersect + " " + except + " " + symmetric + " " + self + " " + a.Count;

                    var small = new HashSet<int>(new int[] { 1, 2 });
                    var big = new HashSet<int>(new int[] { 1, 2, 3 });
                    queries = small.IsSubsetOf(big) + "" + small.IsProperSubsetOf(big) + small.IsSubsetOf(small)
                        + small.IsProperSubsetOf(small) + " " + big.IsSupersetOf(small) + big.IsProperSupersetOf(small)
                        + big.IsProperSupersetOf(big) + " " + small.Overlaps(new int[] { 2, 9 }) + small.Overlaps(new int[] { 9 })
                        + " " + small.SetEquals(new int[] { 2, 1, 1 }) + small.SetEquals(big)
                        + " " + big.RemoveWhere(n => n > 1) + Join(big);

                    var words = new HashSet<string>();
                    words.Add("a");
                    words.Add(null);
                    bool nullAgain = words.Add(null);
                    string seen = "";
                    foreach (string w in words) { seen = seen + (w == null ? "null" : w) + ","; }
                    string actual;
                    bool got = words.TryGetValue("a", out actual);
                    nulls = words.Count + " " + nullAgain + " " + words.Contains(null) + " " + seen + " " + got + actual
                        + " " + words.Remove(null) + words.Count;

                    var many = new HashSet<int>();
                    for (int i = 0; i < 200; i++) { many.Add(i * 7); }
                    int hits = 0;
                    for (int i = 0; i < 200; i++) { if (many.Contains(i * 7)) { hits++; } }
                    grown = many.Count + " " + hits + " " + many.Contains(3);

                    var distinct = new int[] { 5, 5, 6 }.ToHashSet();
                    string fromLinq = "";
                    foreach (int n in new int[] { 1, 2, 2, 3, 1 }.Distinct()) { fromLinq = fromLinq + n; }
                    linq = distinct.Count + " " + fromLinq + " " + distinct.Sum();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "added"),
        "3 TrueFalseTrueFalse TrueFalse"
    );
    // insertion order, with 5 in the entry 1 vacated
    assert_eq!(string_of(&emulator, "walked"), "3,5,2,4,");
    // 7 takes the entry 4 vacated just before it, so it walks before 5
    assert_eq!(
        string_of(&emulator, "sets"),
        "1,2,3,4,5, 2,3,4,5, 2,4,5, 2,7,5, 2,7,5, 0"
    );
    assert_eq!(
        string_of(&emulator, "queries"),
        "TrueTrueTrueFalse TrueTrueFalse TrueFalse TrueFalse 21,"
    );
    assert_eq!(
        string_of(&emulator, "nulls"),
        "2 False True a,null, Truea True1"
    );
    assert_eq!(string_of(&emulator, "grown"), "200 200 False");
    assert_eq!(string_of(&emulator, "linq"), "2 123 11");
}

#[test]
fn the_corlib_queue_and_stack_work_end_to_end() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Program
            {
                public static string queue;
                public static string wrapped;
                public static string queueEmpty;
                public static string stack;
                public static string stackEmpty;
                public static string cleared;

                public static void Main()
                {
                    var q = new Queue<int>();
                    q.Enqueue(1);
                    q.Enqueue(2);
                    q.Enqueue(3);
                    int first = q.Dequeue();
                    q.Enqueue(4);
                    string order = "";
                    foreach (int n in q) { order = order + n; }
                    int peeked;
                    bool hasPeek = q.TryPeek(out peeked);
                    queue = first + " " + order + " " + q.Peek() + hasPeek + peeked + " " + q.Count + " " + q.Contains(3) + q.Contains(1);

                    // wrap around the ring many times, growing while wrapped
                    var ring = new Queue<int>(2);
                    int total = 0;
                    for (int i = 0; i < 50; i++)
                    {
                        ring.Enqueue(i);
                        ring.Enqueue(i + 100);
                        total += ring.Dequeue();
                    }
                    string rest = "";
                    foreach (int n in ring.ToArray()) { rest = rest + n + ","; }
                    ring.TrimExcess();
                    wrapped = total + " " + ring.Count + " " + rest.Length + " " + ring.Sum() + " " + ring.Peek();

                    var empty = new Queue<string>(new string[] { "x" });
                    string taken = empty.Dequeue();
                    string missing;
                    bool none = empty.TryDequeue(out missing);
                    string caught = "";
                    try { empty.Peek(); } catch (InvalidOperationException e) { caught = e.Message; }
                    queueEmpty = taken + " " + none + (missing == null) + " " + caught;

                    var s = new Stack<int>();
                    s.Push(1);
                    s.Push(2);
                    s.Push(3);
                    int top = s.Pop();
                    s.Push(4);
                    string down = "";
                    foreach (int n in s) { down = down + n; }
                    string array = "";
                    foreach (int n in s.ToArray()) { array = array + n; }
                    int popped;
                    bool hasPop = s.TryPop(out popped);
                    int peek;
                    bool hasTop = s.TryPeek(out peek);
                    stack = top + " " + down + " " + array + " " + hasPop + popped + " " + hasTop + peek + " " + s.Count + " " + s.Contains(1) + s.Contains(4);

                    var drained = new Stack<int>(new int[] { 1, 2 });
                    drained.Pop();
                    drained.Pop();
                    int nothing;
                    bool got = drained.TryPop(out nothing);
                    string failed = "";
                    try { drained.Pop(); } catch (InvalidOperationException e) { failed = e.Message; }
                    stackEmpty = got + "" + nothing + " " + failed;

                    s.Clear();
                    q.Clear();
                    s.Push(9);
                    q.Enqueue(9);
                    cleared = s.Count + "" + q.Count + s.Peek() + q.Peek();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "queue"), "1 234 2True2 3 TrueFalse");
    // the dequeues alternate the two streams: 0..24 and 100..124 came out
    // (3100), 25..49 and 125..149 stayed (4350), printed in 75 + 100 chars
    assert_eq!(string_of(&emulator, "wrapped"), "3100 50 175 4350 25");
    assert_eq!(
        string_of(&emulator, "queueEmpty"),
        "x FalseTrue Queue empty."
    );
    assert_eq!(
        string_of(&emulator, "stack"),
        "3 421 421 True4 True2 2 TrueFalse"
    );
    assert_eq!(string_of(&emulator, "stackEmpty"), "False0 Stack empty.");
    assert_eq!(string_of(&emulator, "cleared"), "1199");
}

#[test]
fn comparers_flow_through_the_collections_and_linq() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            // a comparer of one's own: by length, then by the string
            public class ByLength : IComparer<string>
            {
                public int Compare(string x, string y)
                {
                    if (x.Length != y.Length) { return x.Length.CompareTo(y.Length); }
                    return string.Compare(x, y, StringComparison.Ordinal);
                }
            }

            // an equality comparer of one's own: ints modulo 10
            public class ModTen : IEqualityComparer<int>
            {
                public bool Equals(int x, int y) { return x % 10 == y % 10; }
                public int GetHashCode(int obj) { return obj % 10; }
            }

            public class Program
            {
                public static string sorted;
                public static string searched;
                public static string dictionary;
                public static string set;
                public static string ordered;
                public static string sets;
                public static string queries;
                public static string grouped;
                public static string defaults;

                static string Join(IEnumerable<string> items)
                {
                    string text = "";
                    foreach (string item in items) { text = text + item + ","; }
                    return text;
                }

                static string JoinInts(IEnumerable<int> items)
                {
                    string text = "";
                    foreach (int item in items) { text = text + item + ","; }
                    return text;
                }

                public static void Main()
                {
                    var words = new List<string> { "pear", "Fig", "apple", "kiwi", "fig" };
                    words.Sort(new ByLength());
                    string byLength = Join(words);
                    words.Sort(StringComparer.OrdinalIgnoreCase);
                    string ignoringCase = Join(words);
                    words.Sort(Comparer<string>.Create((a, b) => string.Compare(b, a, StringComparison.Ordinal)));
                    string reversed = Join(words);
                    words.Sort(Comparer<string>.Default);
                    string plain = Join(words);
                    words.Sort(1, 3, new ByLength());
                    sorted = byLength + " " + ignoringCase + " " + reversed + " " + plain + " " + Join(words);

                    words.Sort(new ByLength());        // Fig,fig,kiwi,pear,apple
                    searched = words.BinarySearch("pear", new ByLength()) + " " + words.BinarySearch("zz", new ByLength())
                        + " " + words.BinarySearch(0, 2, "fig", new ByLength()) + " " + words.BinarySearch("Fig", null);

                    var ages = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
                    ages["Ann"] = 30;
                    ages["ANN"] = 31;       // same key
                    ages.Add("bob", 41);
                    int found;
                    bool got = ages.TryGetValue("BOB", out found);
                    string keys = "";
                    foreach (var key in ages.Keys) { keys = keys + key + ","; }
                    var ordinal = new Dictionary<string, int>(StringComparer.Ordinal);
                    ordinal["a"] = 1;
                    ordinal["A"] = 2;
                    dictionary = ages.Count + " " + ages["ann"] + " " + got + found + " " + keys + " " + ages.Remove("BoB") + ages.Count
                        + " " + ordinal.Count + " " + ages.Comparer.Equals("x", "X") + ordinal.Comparer.Equals("x", "X");

                    var tens = new HashSet<int>(new int[] { 1, 11, 2, 22, 3 }, new ModTen());
                    bool again = tens.Add(21);
                    tens.UnionWith(new int[] { 4, 14 });
                    tens.IntersectWith(new int[] { 12, 13, 14, 15 });   // keeps 2, 3, 4 by mod 10
                    var lower = new HashSet<string>(StringComparer.InvariantCultureIgnoreCase) { "A", "b" };
                    set = JoinInts(tens) + " " + again + " " + tens.Contains(33) + tens.Contains(5)
                        + " " + lower.Add("a") + lower.Contains("B") + lower.Count + " " + lower.SetEquals(new string[] { "a", "B" });

                    string[] mixed = new string[] { "b", "A", "a", "B", "cc", "C" };
                    ordered = Join(mixed.OrderBy(s => s, StringComparer.OrdinalIgnoreCase))
                        + " " + Join(mixed.OrderByDescending(s => s, new ByLength()).ThenBy(s => s, StringComparer.Ordinal))
                        + " " + Join(mixed.OrderBy(s => s.Length).ThenByDescending(s => s, StringComparer.OrdinalIgnoreCase));

                    sets = Join(mixed.Distinct(StringComparer.OrdinalIgnoreCase))
                        + " " + Join(new string[] { "x", "Y" }.Union(new string[] { "X", "z" }, StringComparer.OrdinalIgnoreCase))
                        + " " + Join(mixed.Intersect(new string[] { "c", "b" }, StringComparer.OrdinalIgnoreCase))
                        + " " + Join(mixed.Except(new string[] { "a", "cc" }, StringComparer.OrdinalIgnoreCase))
                        + " " + JoinInts(new int[] { 5, 15, 6 }.Distinct(new ModTen()));

                    queries = mixed.Contains("CC", StringComparer.OrdinalIgnoreCase) + "" + mixed.Contains("CC", StringComparer.Ordinal)
                        + " " + new string[] { "a", "B" }.SequenceEqual(new string[] { "A", "b" }, StringComparer.OrdinalIgnoreCase)
                        + new string[] { "a", "B" }.SequenceEqual(new string[] { "A", "b" }, EqualityComparer<string>.Default)
                        + " " + mixed.ToHashSet(StringComparer.OrdinalIgnoreCase).Count
                        + " " + new string[] { "k", "K" }.ToDictionary(s => s + "!", s => s.Length, StringComparer.Ordinal).Count;

                    string groups = "";
                    foreach (var group in mixed.GroupBy(s => s, StringComparer.OrdinalIgnoreCase))
                    {
                        groups = groups + group.Key + ":" + group.Count() + ",";
                    }
                    string chosen = "";
                    foreach (var group in mixed.GroupBy(s => s, s => s.ToUpperInvariant(), StringComparer.OrdinalIgnoreCase))
                    {
                        chosen = chosen + Join(group).Length + ",";
                    }
                    grouped = groups + " " + chosen;

                    var byDefault = Comparer<int>.Default;
                    var equality = EqualityComparer<string>.Default;
                    var culture = StringComparer.CurrentCultureIgnoreCase;
                    defaults = byDefault.Compare(3, 5) + " " + byDefault.Compare(5, 5) + " " + equality.Equals("a", "a") + equality.Equals("a", null)
                        + " " + (equality.GetHashCode("hi") == "hi".GetHashCode()) + " " + culture.Equals("Ab", "aB") + " " + (culture.GetHashCode("Ab") == culture.GetHashCode("aB"))
                        + " " + (StringComparer.Ordinal.Compare("a", "B") > 0) + " " + (StringComparer.OrdinalIgnoreCase.Compare("a", "B") < 0)
                        + " " + StringComparer.Ordinal.GetHashCode(null);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    // the default order of strings is the culture's (`fig` before `Fig`);
    // the last sort touches only positions 1..3, by length then ordinal
    assert_eq!(
        string_of(&emulator, "sorted"),
        "Fig,fig,kiwi,pear,apple, apple,Fig,fig,kiwi,pear, pear,kiwi,fig,apple,Fig, apple,fig,Fig,kiwi,pear, apple,Fig,fig,kiwi,pear,"
    );
    // two-letter "zz" would go first by length: ~0 = -1; "fig" is found
    // within the first two; "Fig" by the default order is first
    assert_eq!(string_of(&emulator, "searched"), "3 -1 1 0");
    assert_eq!(
        string_of(&emulator, "dictionary"),
        "2 31 True41 Ann,bob, True1 2 TrueFalse"
    );
    // by mod 10: {1, 2, 3} then 21 is a repeat; 4 joins; intersect keeps 2..4
    assert_eq!(
        string_of(&emulator, "set"),
        "2,3,4, False TrueFalse FalseTrue2 True"
    );
    // stable within equal keys: A before a, b before B; the length-then-
    // ordinal comparer never ties, so its ThenBy changes nothing
    assert_eq!(
        string_of(&emulator, "ordered"),
        "A,a,b,B,C,cc, cc,b,a,C,B,A, C,b,B,A,a,cc,"
    );
    // Intersect and Except yield each element once under the comparer: the
    // `B` after `b` and the `a` after `A` are repeats
    assert_eq!(
        string_of(&emulator, "sets"),
        "b,A,cc,C, x,Y,z, b,C, b,C, 5,6,"
    );
    assert_eq!(string_of(&emulator, "queries"), "TrueFalse TrueFalse 4 2");
    assert_eq!(
        string_of(&emulator, "grouped"),
        "b:2,A:2,cc:1,C:1, 4,4,3,2,"
    );
    assert_eq!(
        string_of(&emulator, "defaults"),
        "-1 0 TrueFalse True True True True True 0"
    );
}

#[test]
fn user_structs_work_as_dictionary_and_set_keys() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public enum Kind { Wall, Floor }

            public struct Cell
            {
                public int x;
                public int y;
                public Cell(int x, int y) { this.x = x; this.y = y; }
            }

            // every field kind: enum, float, string, a nested struct, a class
            public struct Tile
            {
                public Kind kind;
                public float height;
                public string name;
                public Cell at;
                public Marker marker;
            }

            public class Marker { public int id; }

            public struct Pair<T>
            {
                public T first;
                public T second;
            }

            public record struct Point(int X, int Y);

            public class Program
            {
                public static string basics;
                public static string copied;
                public static string tiles;
                public static string generic;
                public static string records;
                public static string set;
                public static string linq;
                public static string walked;

                public static void Main()
                {
                    var grid = new Dictionary<Cell, string>();
                    grid[new Cell(1, 2)] = "a";
                    grid[new Cell(1, 2)] = "b";          // same key, overwritten
                    grid.Add(new Cell(2, 1), "c");        // x/y swapped: another key
                    string got;
                    bool found = grid.TryGetValue(new Cell(1, 2), out got);
                    bool has = grid.ContainsKey(new Cell(2, 1));
                    bool lacks = grid.ContainsKey(new Cell(3, 3));
                    bool removed = grid.Remove(new Cell(2, 1));
                    basics = grid.Count + " " + found + got + " " + has + lacks + " " + removed + grid.Count;

                    // the stored key is a copy: mutating the variable after
                    // the insert changes neither the entry nor its lookup
                    Cell key = new Cell(5, 5);
                    var byKey = new Dictionary<Cell, int>();
                    byKey[key] = 1;
                    key.x = 6;
                    bool oldStillThere = byKey.ContainsKey(new Cell(5, 5));
                    bool newAbsent = !byKey.ContainsKey(key);
                    Cell first = new Cell(0, 0);
                    foreach (var pair in byKey) { first = pair.Key; }
                    first.y = 9;                          // a copy out, too
                    copied = oldStillThere + "" + newAbsent + " " + byKey.ContainsKey(new Cell(5, 5)) + " " + first.x + first.y;

                    var shared = new Marker { id = 1 };
                    var t1 = new Tile { kind = Kind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
                    var t2 = new Tile { kind = Kind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
                    var t3 = new Tile { kind = Kind.Wall, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
                    var t4 = new Tile { kind = Kind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = new Marker { id = 1 } };
                    var t5 = new Tile { kind = Kind.Floor, height = 0.5f, name = "n", at = new Cell(1, 2), marker = shared };
                    var byTile = new Dictionary<Tile, int>();
                    byTile[t1] = 1;
                    byTile[t2] = 2;     // equal in every field: same entry
                    byTile[t3] = 3;     // enum differs
                    byTile[t4] = 4;     // class field is another reference
                    byTile[t5] = 5;     // nested struct differs
                    tiles = byTile.Count + " " + byTile[t1] + " " + byTile[new Tile { kind = Kind.Wall, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared }];

                    var pairs = new Dictionary<Pair<string>, int>();
                    pairs[new Pair<string> { first = "a", second = "b" }] = 1;
                    pairs[new Pair<string> { first = "a", second = "b" }] = 2;
                    pairs[new Pair<string> { first = "b", second = "a" }] = 3;
                    generic = pairs.Count + " " + pairs[new Pair<string> { first = "a", second = "b" }];

                    var points = new Dictionary<Point, string>();
                    points[new Point(1, 1)] = "p";
                    points[new Point(1, 1)] = "q";
                    records = points.Count + " " + points[new Point(1, 1)] + " " + points.ContainsKey(new Point(1, 2));

                    var cells = new HashSet<Cell>();
                    cells.Add(new Cell(1, 1));
                    bool dup = cells.Add(new Cell(1, 1));
                    cells.Add(new Cell(2, 2));
                    set = cells.Count + " " + dup + " " + cells.Contains(new Cell(2, 2)) + cells.Remove(new Cell(1, 1)) + cells.Count;

                    var list = new List<Cell> { new Cell(1, 1), new Cell(2, 2), new Cell(1, 1), new Cell(3, 3) };
                    int groups = list.GroupBy(c => c).Count();
                    int distinct = list.Distinct().Count();
                    bool contains = list.Contains(new Cell(2, 2));
                    int index = list.IndexOf(new Cell(3, 3));
                    linq = groups + " " + distinct + " " + contains + " " + index + " " + list.Where(c => c.Equals(new Cell(1, 1))).Count();

                    walked = "";
                    foreach (var (at, tag) in grid) { walked = walked + at.x + at.y + tag; }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    // the first count is read after the removal, like the last
    assert_eq!(string_of(&emulator, "basics"), "1 Trueb TrueFalse True1");
    assert_eq!(string_of(&emulator, "copied"), "TrueTrue True 59");
    assert_eq!(string_of(&emulator, "tiles"), "4 2 3");
    assert_eq!(string_of(&emulator, "generic"), "2 2");
    assert_eq!(string_of(&emulator, "records"), "1 q False");
    assert_eq!(string_of(&emulator, "set"), "2 False TrueTrue1");
    assert_eq!(string_of(&emulator, "linq"), "3 3 True 3 2");
    assert_eq!(string_of(&emulator, "walked"), "12b");
}

#[test]
fn an_invoked_property_rebinds_to_the_extension_method_of_that_name() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Bag
            {
                public int Count = 7;                    // a field of the same name
                public Func<int, int> Twice = n => n * 2; // a delegate-typed field
            }

            public static class BagExtensions
            {
                public static int Count(this Bag bag, int extra) { return bag.Count + extra; }
            }

            public class Program
            {
                public static int counted;
                public static int all;
                public static int field;
                public static int delegated;
                public static int property;

                public static void Main()
                {
                    var list = new List<int> { 1, 2, 3 };
                    counted = list.Count(n => n > 1);    // the extension, not the property
                    all = list.Count();                  // the extension with no argument
                    property = list.Count;               // still the property
                    var bag = new Bag();
                    field = bag.Count(10);               // the extension over a field
                    delegated = bag.Twice(4);            // a delegate field is called itself
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "counted"), 2);
    assert_eq!(int_of(&emulator, "all"), 3);
    assert_eq!(int_of(&emulator, "property"), 3);
    assert_eq!(int_of(&emulator, "field"), 17);
    assert_eq!(int_of(&emulator, "delegated"), 8);
}

#[test]
fn index_initializers_write_through_the_indexer() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Grid
            {
                private int[] cells = new int[16];
                public int this[int x, int y]
                {
                    get { return cells[y * 4 + x]; }
                    set { cells[y * 4 + x] = value; }
                }
            }
            public class Program
            {
                public static int result;
                public static void Main()
                {
                    var grid = new Grid { [1, 2] = 7, [3, 3] = 5 };
                    result = grid[1, 2] * 10 + grid[3, 3];
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 75);
}

#[test]
fn structs_are_copied_where_csharp_copies_them() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public struct Point
            {
                public int x;
                public int y;
                public Point(int x, int y) { this.x = x; this.y = y; }
                public void Move(int dx) { x += dx; }
                public int Sum() { return x + y; }
            }

            public struct Line
            {
                public Point a;   // a struct inside a struct
                public Point b;
            }

            public class Holder
            {
                public Point pos;           // default is an instance, not null
                public Point Prop { get; set; }
                public Point Copy() { return pos; }   // returns a copy
            }

            public class Program
            {
                public static int assigned;
                public static int byValue;
                public static int inPlace;
                public static int elementInPlace;
                public static int nested;
                public static int fieldDefault;
                public static int arrayDefaults;
                public static int viaProperty;
                public static int listCopy;
                public static int boxed;
                public static int returned;

                static void Bump(Point p) { p.x = 100; }
                static Point Make() { Point p = new Point(5, 6); return p; }

                public static void Main()
                {
                    // b = a copies: writing b leaves a alone
                    Point a = new Point(1, 2);
                    Point b = a;
                    b.x = 9;
                    assigned = a.x * 10 + b.x;          // 19

                    // by value: the callee's writes stay in the callee
                    Bump(a);
                    byValue = a.x;                      // 1

                    // a method on a variable works in place
                    a.Move(3);
                    inPlace = a.x;                      // 4

                    // an array element is a variable: in place too
                    var points = new Point[2];
                    points[0].x = 7;
                    points[0].Move(1);
                    Point taken = points[0];
                    taken.x = 0;
                    elementInPlace = points[0].x;       // 8

                    // nested structs copy all the way down
                    Line l1 = default;
                    l1.a.x = 1;
                    Line l2 = l1;
                    l2.a.x = 2;
                    nested = l1.a.x * 10 + l2.a.x;      // 12

                    // a struct field of a class holds an instance from the start
                    var holder = new Holder();
                    holder.pos.y = 5;
                    fieldDefault = holder.pos.y + holder.pos.x;   // 5

                    // new S[n] is n defaults, each its own
                    var many = new Point[3];
                    many[1].x = 4;
                    arrayDefaults = many[0].x + many[1].x + many[2].x;   // 4

                    // a property returns a copy; the getter result is not the field
                    holder.Prop = new Point(1, 1);
                    Point fromProp = holder.Prop;
                    fromProp.x = 50;
                    Point again = holder.Copy();
                    again.y = 50;
                    viaProperty = holder.Prop.x + holder.pos.y;   // 1 + 5 = 6

                    // List<T> stores copies and hands out copies
                    var list = new List<Point>();
                    list.Add(a);
                    a.x = 0;
                    Point got = list[0];
                    got.x = 0;
                    listCopy = list[0].x;               // 4

                    // boxing copies
                    Point p = new Point(2, 3);
                    object o = p;
                    p.x = 0;
                    Point back = (Point)o;
                    boxed = back.x;                     // 2

                    returned = Make().Sum();            // 11
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "assigned"), 19);
    assert_eq!(int_of(&emulator, "byValue"), 1);
    assert_eq!(int_of(&emulator, "inPlace"), 4);
    assert_eq!(int_of(&emulator, "elementInPlace"), 8);
    assert_eq!(int_of(&emulator, "nested"), 12);
    assert_eq!(int_of(&emulator, "fieldDefault"), 5);
    assert_eq!(int_of(&emulator, "arrayDefaults"), 4);
    assert_eq!(int_of(&emulator, "viaProperty"), 6);
    assert_eq!(int_of(&emulator, "listCopy"), 4);
    assert_eq!(int_of(&emulator, "boxed"), 2);
    assert_eq!(int_of(&emulator, "returned"), 11);
}

#[test]
fn structs_get_field_wise_equals_and_hash_code() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public struct Cell
            {
                public int row;
                public int col;
                public string tag;
            }

            public struct Wrapped
            {
                public Cell inner;   // nested: equality recurses
            }

            // a struct that declares its own: honoured over the synthesized one
            public struct Loose
            {
                public int value;
                public override bool Equals(object other) { return true; }
                public override int GetHashCode() { return 7; }
            }

            public class Program
            {
                public static bool equal;
                public static bool different;
                public static bool sameHash;
                public static bool notNull;
                public static bool notInt;
                public static bool nestedEqual;
                public static int dictionary;
                public static bool declared;

                public static void Main()
                {
                    Cell a = new Cell { row = 1, col = 2, tag = "x" };
                    Cell b = new Cell { row = 1, col = 2, tag = "x" };
                    Cell c = new Cell { row = 1, col = 3, tag = "x" };
                    equal = a.Equals(b);
                    different = !a.Equals(c);
                    sameHash = a.GetHashCode() == b.GetHashCode();
                    notNull = !a.Equals(null);
                    notInt = !a.Equals(5);

                    Wrapped w1 = new Wrapped { inner = a };
                    Wrapped w2 = new Wrapped { inner = b };
                    nestedEqual = w1.Equals(w2);

                    // two equal-valued keys are one entry
                    var counts = new Dictionary<Cell, int>();
                    counts[a] = 10;
                    counts[b] = 20;
                    counts[c] = 1;
                    dictionary = counts.Count * 100 + counts[a];   // 2 * 100 + 20

                    Loose l1 = new Loose { value = 1 };
                    Loose l2 = new Loose { value = 2 };
                    declared = l1.Equals(l2) && l1.GetHashCode() == 7;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    for name in [
        "equal",
        "different",
        "sameHash",
        "notNull",
        "notInt",
        "nestedEqual",
        "declared",
    ] {
        assert!(
            matches!(emulator.value_of(name), Some(Value::Boolean(true))),
            "{name} = {:?}",
            emulator.value_of(name)
        );
    }
    assert_eq!(int_of(&emulator, "dictionary"), 220);
}

#[test]
fn writing_a_member_of_a_struct_copy_is_an_error() {
    // `holder.Prop.x = 1` changes a copy the getter returned — CS1612
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public struct Point { public int x; }
            public class Holder { public Point Prop { get; set; } }
            public class Program
            {
                public static void Main()
                {
                    var holder = new Holder();
                    holder.Prop.x = 1;
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output
            .errors
            .iter()
            .any(|error| error.message.to_string().contains("CS1612")),
        "{:#?}",
        output.errors
    );
}

#[test]
fn a_call_with_arguments_inside_a_parenthesized_conditional_is_not_a_declaration() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int result;
                static bool F(int a, int b) => a == b;
                static bool H(int a) => a > 0;
                public static void Main()
                {
                    result = 1 + (F(1, 1) ? 1 : 0) + (H(1) ? 10 : 0) + (F(1, 2) ? 100 : 0);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 12);
}

#[test]
fn named_arguments_bind_by_name_and_evaluate_in_written_order() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Box
            {
                public int width;
                public int height;
                public Box(int width, int height) { this.width = width; this.height = height; }
            }

            public class Program
            {
                public static int reordered;
                public static int mixed;
                public static int order;
                public static int constructed;
                public static int parsed;
                public static int trace;

                static int Sub(int a, int b) => a - b;
                static int Three(int a, int b, int c) => a * 100 + b * 10 + c;
                static int Tick(int value) { trace = trace * 10 + value; return value; }

                public static void Main()
                {
                    reordered = Sub(b: 1, a: 5);                 // 4, not -4
                    mixed = Three(1, c: 3, b: 2);                // 123
                    // written order is evaluation order, whatever the names say
                    order = Three(c: Tick(3), a: Tick(1), b: Tick(2));   // 123, trace 312
                    constructed = new Box(height: 2, width: 7).width * 10
                        + new Box(height: 2, width: 7).height;   // 72
                    int value;
                    if (int.TryParse(result: out value, s: "41")) { parsed = value + 1; }   // 42
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "reordered"), 4);
    assert_eq!(int_of(&emulator, "mixed"), 123);
    assert_eq!(int_of(&emulator, "order"), 123);
    assert_eq!(int_of(&emulator, "trace"), 312);
    assert_eq!(int_of(&emulator, "constructed"), 72);
    assert_eq!(int_of(&emulator, "parsed"), 42);
}

#[test]
fn a_named_argument_must_name_a_parameter_once() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                static int Sub(int a, int b) => a - b;
                public static void Main()
                {
                    int x = Sub(a: 1, c: 2);      // no parameter c
                    int y = Sub(a: 1, a: 2);      // a twice
                    int z = Sub(b: 1, 2);         // positional after a moved name
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let overload_errors = bodies
        .errors
        .iter()
        .filter(|error| {
            matches!(
                error.kind,
                men_sharp_semantics::SemanticErrorKind::NoMatchingOverload
            )
        })
        .count();
    assert_eq!(overload_errors, 3, "{:#?}", bodies.errors);
}

#[test]
fn optional_parameters_take_their_defaults_at_the_call_site() {
    let Some(emulator) = run(
        r##"
        namespace Game
        {
            public enum Mode { Fast, Safe = 7 }

            public struct Point { public int x; }

            public class Box
            {
                public int size;
                public string label;
                public Box(int size = 4, string label = "box") { this.size = size; this.label = label; }
            }

            public class Program
            {
                public static int sum;
                public static int skipped;
                public static int negative;
                public static int enumDefault;
                public static string text;
                public static int structDefault;
                public static int nullDefault;
                public static int constructed;
                public static string constructedLabel;
                public static int preferred;

                const int Base = 100;
                static int Add(int a, int b = 2, int c = Base) => a + b + c;
                static int Neg(int a, int b = -3) => a + b;
                static int ModeOf(Mode mode = Mode.Safe) => (int)mode;
                static string Tag(string t, string prefix = "#") => prefix + t;
                static int PointX(Point p = default) => p.x + 1;
                static int Len(string s = null) => s == null ? -1 : s.Length;
                static int Pick(int a) => a * 10;
                static int Pick(int a, int b = 1) => a + b;

                public static void Main()
                {
                    sum = Add(1);                     // 1 + 2 + 100 = 103
                    skipped = Add(1, c: 5);           // 1 + 2 + 5 = 8
                    negative = Neg(1);                // -2
                    enumDefault = ModeOf();           // 7
                    text = Tag("x");                  // "#x"
                    structDefault = PointX();         // 1
                    nullDefault = Len();              // -1
                    var box = new Box(label: "lid");
                    constructed = box.size;           // 4
                    constructedLabel = box.label;     // "lid"
                    preferred = Pick(1);              // the overload without defaults: 10
                }
            }
        }
        "##,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "sum"), 103);
    assert_eq!(int_of(&emulator, "skipped"), 8);
    assert_eq!(int_of(&emulator, "negative"), -2);
    assert_eq!(int_of(&emulator, "enumDefault"), 7);
    assert_eq!(string_of(&emulator, "text"), "#x");
    assert_eq!(int_of(&emulator, "structDefault"), 1);
    assert_eq!(int_of(&emulator, "nullDefault"), -1);
    assert_eq!(int_of(&emulator, "constructed"), 4);
    assert_eq!(string_of(&emulator, "constructedLabel"), "lid");
    assert_eq!(int_of(&emulator, "preferred"), 10);
}

#[test]
fn an_extern_optional_parameter_is_baked_as_its_metadata_constant() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static int count;
                public static void Main()
                {
                    // Split(char separator, StringSplitOptions options = None)
                    count = "a,b".Split(',').Length;
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );
    let text = output.program.to_uasm().unwrap();
    assert!(
        text.contains(
            "SystemString.__Split__SystemChar_SystemStringSplitOptions__SystemStringArray"
        ),
        "{text}"
    );
    // the omitted enum argument is the real boxed enum value, built by the importer
    let meta = output.program.to_meta_json().unwrap();
    assert!(meta.contains("System.StringSplitOptions#0"), "{meta}");
}

#[test]
fn a_default_value_must_be_a_constant() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                static int Now() => 1;
                static int F(int a = Now()) => a;
                public static void Main() { int x = F(); }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert!(
        bodies.errors.iter().any(|error| matches!(
            error.kind,
            men_sharp_semantics::SemanticErrorKind::UnsupportedExpression
        )),
        "{:#?}",
        bodies.errors
    );
}

#[test]
fn params_arguments_are_gathered_into_an_array() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public struct Point { public int x; }

            public class Program
            {
                public static int none;
                public static int several;
                public static int passedArray;
                public static int normalFormWins;
                public static int withLeading;
                public static int generic;
                public static int structCopies;
                public static int named;

                static int Sum(params int[] values)
                {
                    int total = 0;
                    foreach (var v in values) { total += v; }
                    return total;
                }
                static int Sum(int single) => single * 100;
                static int Weighted(int factor, params int[] values) => factor * Sum(values);
                static int Count<T>(params T[] items) => items.Length;
                static int FirstX(params Point[] points) { points[0].x = 99; return points.Length; }

                public static void Main()
                {
                    none = Sum();                          // 0 (empty array)
                    several = Sum(1, 2, 3);                // 6
                    passedArray = Sum(new int[] { 4, 5 }); // 9: normal form, the array itself
                    normalFormWins = Sum(7);               // 700: the non-params overload
                    withLeading = Weighted(2, 1, 2, 3);    // 12
                    generic = Count("a", "b", "c");        // 3
                    Point p = new Point { x = 1 };
                    structCopies = FirstX(p) * 10 + p.x;   // the callee got a copy: 11
                    named = Weighted(factor: 3, 1, 1);     // 6
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "none"), 0);
    assert_eq!(int_of(&emulator, "several"), 6);
    assert_eq!(int_of(&emulator, "passedArray"), 9);
    assert_eq!(int_of(&emulator, "normalFormWins"), 700);
    assert_eq!(int_of(&emulator, "withLeading"), 12);
    assert_eq!(int_of(&emulator, "generic"), 3);
    assert_eq!(int_of(&emulator, "structCopies"), 11);
    assert_eq!(int_of(&emulator, "named"), 6);
}

#[test]
fn an_extern_params_call_passes_one_array() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static string joined;
                public static void Main()
                {
                    joined = string.Join(",", "a", "b", "c");
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );
    let text = output.program.to_uasm().unwrap();
    assert!(
        text.contains("\"SystemString.__Join__SystemString_SystemStringArray__SystemString\""),
        "{text}"
    );
    assert!(
        text.contains("\"SystemStringArray.__ctor__SystemInt32__SystemStringArray\""),
        "{text}"
    );
}

#[test]
fn abstract_members_dispatch_on_the_runtime_type() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public abstract class Shape
            {
                public abstract int Area();
                public abstract string Name { get; }
                public abstract int Level { get; set; }
                public virtual int Twice() => Area() * 2;
                public virtual int Bonus => 1;
                public int Describe() => Name.Length * 100 + Area();
            }

            public class Circle : Shape
            {
                private int r;
                public Circle(int r) { this.r = r; }
                public override int Area() => 3 * r * r;
                public override string Name => "circle";
                public override int Level { get; set; }
                public override int Bonus => 5;
            }

            public class Square : Shape
            {
                private int s;
                public Square(int s) { this.s = s; }
                public override int Area() => s * s;
                public override string Name => "sq";
                public override int Level { get; set; }
                public override int Twice() => Area() * 2 + 1;
            }

            public class Program
            {
                public static int areas;
                public static int twice;
                public static int names;
                public static int levels;
                public static int bonus;
                public static int described;

                public static void Main()
                {
                    Shape[] shapes = new Shape[] { new Circle(2), new Square(3) };
                    foreach (Shape shape in shapes)
                    {
                        areas += shape.Area();          // 12 + 9 = 21
                        twice += shape.Twice();         // 24 + 19 = 43
                        names += shape.Name.Length;     // 6 + 2 = 8
                        shape.Level = shape.Area();     // abstract setter
                        levels += shape.Level;          // 21
                        bonus += shape.Bonus;           // 5 + 1 = 6 (virtual property)
                    }
                    described = shapes[0].Describe();   // 612
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "areas"), 21);
    assert_eq!(int_of(&emulator, "twice"), 43);
    assert_eq!(int_of(&emulator, "names"), 8);
    assert_eq!(int_of(&emulator, "levels"), 21);
    assert_eq!(int_of(&emulator, "bonus"), 6);
    assert_eq!(int_of(&emulator, "described"), 612);
}

#[test]
fn interface_calls_dispatch_on_classes_and_structs() {
    let Some(emulator) = run_with_corlib(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public interface IDescribable { string Describe(); }
            public interface IShape : IDescribable
            {
                int Area();
                string Name { get; }
                int this[int scale] { get; }
            }

            public class Circle : IShape
            {
                public int r;
                public Circle(int r) { this.r = r; }
                public int Area() => 3 * r * r;
                public string Name => "circle";
                public int this[int scale] => Area() * scale;
                public string Describe() => Name + Area();
            }

            public struct Unit : IShape
            {
                public int size;
                public int Area() => size;
                public string Name => "unit";
                public int this[int scale] => size * scale;
                // explicit: reachable only through the interface
                string IDescribable.Describe() => "u" + size;
            }

            public class Holder { public IShape shape; }

            public class Program
            {
                public static int viaInterface;
                public static int viaInherited;
                public static int list;
                public static int genericClass;
                public static int genericStruct;
                public static int boxedCopy;
                public static int field;
                public static int indexer;
                public static string explicitImpl;

                static int Total<T>(T shape) where T : IShape => shape.Area() + shape.Name.Length;

                public static void Main()
                {
                    IShape c = new Circle(2);
                    viaInterface = c.Area() + c.Name.Length;        // 12 + 6 = 18
                    IDescribable d = c;
                    viaInherited = d.Describe().Length;             // "circle12" = 8

                    var shapes = new List<IShape> { new Circle(1), new Unit { size = 7 } };
                    foreach (var s in shapes) { list += s.Area(); } // 3 + 7 = 10

                    genericClass = Total(new Circle(1));            // 3 + 6 = 9
                    Unit u = new Unit { size = 4 };
                    genericStruct = Total(u);                       // 4 + 4 = 8

                    // a struct behind an interface is a boxed copy
                    IShape boxed = u;
                    u.size = 100;
                    boxedCopy = boxed.Area();                       // 4

                    var holder = new Holder { shape = new Unit { size = 6 } };
                    field = holder.shape.Area();                    // 6
                    indexer = c[10] + holder.shape[2];              // 120 + 12 = 132
                    explicitImpl = ((IDescribable)holder.shape).Describe();   // "u6"
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "viaInterface"), 18);
    assert_eq!(int_of(&emulator, "viaInherited"), 8);
    assert_eq!(int_of(&emulator, "list"), 10);
    assert_eq!(int_of(&emulator, "genericClass"), 9);
    assert_eq!(int_of(&emulator, "genericStruct"), 8);
    assert_eq!(int_of(&emulator, "boxedCopy"), 4);
    assert_eq!(int_of(&emulator, "field"), 6);
    assert_eq!(int_of(&emulator, "indexer"), 132);
    assert_eq!(string_of(&emulator, "explicitImpl"), "u6");
}

#[test]
fn abstract_and_interface_contracts_are_checked() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public interface IShape { int Area(); string Name { get; } }
            public abstract class Base { public abstract int F(); }
            public class NoArea : IShape { public string Name => "x"; }      // CS0535
            public class NoF : Base { }                                      // CS0534
            public class Fine : Base, IShape
            {
                public override int F() => 1;
                public int Area() => 2;
                public string Name => "fine";
            }
            public class Program
            {
                public static void Main()
                {
                    Base b = new Base();                                     // CS0144
                    Fine f = new Fine();
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let mut missing: Vec<String> = bodies
        .errors
        .iter()
        .filter_map(|error| match &error.kind {
            men_sharp_semantics::SemanticErrorKind::MissingImplementation { type_name, member } => {
                Some(format!("{type_name}: {member}"))
            }
            _ => None,
        })
        .collect();
    missing.sort();
    assert_eq!(
        missing,
        vec!["Game.NoArea: Game.IShape.Area", "Game.NoF: Game.Base.F"],
        "{:#?}",
        bodies.errors
    );
    assert!(
        bodies.errors.iter().any(|error| matches!(
            error.kind,
            men_sharp_semantics::SemanticErrorKind::CannotInstantiateAbstractType { .. }
        )),
        "{:#?}",
        bodies.errors
    );
}

#[test]
fn constructors_chain_to_the_base() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class A
            {
                public int v = 5;
                public int w;
                public int fromVirtual;
                public A() { w = 1; fromVirtual = F(); }
                public A(int x) { w = x; }
                public virtual int F() { return 1; }
            }
            public class B : A
            {
                public int own = 7;
                public B() { }                                  // implicit base()
                public B(int x) : base(x) { own = x * 2; }
                public B(int x, int y) : this(x) { own += y; }  // this(x) → base(x)
                public override int F() { return 2; }
            }
            public class C : A { }                              // implicit constructor, implicit base()
            public abstract class S
            {
                public int v;
                protected S(int x) { v = x + Extra(); }
                protected abstract int Extra();
            }
            public class T : S
            {
                public T() : base(1) { }
                protected override int Extra() { return 10; }
            }
            public class Program
            {
                public static int implicitBase;
                public static int explicitBase;
                public static int viaThis;
                public static int noConstructor;
                public static int virtualFromBase;
                public static int abstractBase;
                public static void Main()
                {
                    var b = new B();
                    implicitBase = b.v * 100 + b.w * 10 + b.own;      // 517
                    var b2 = new B(3);
                    explicitBase = b2.v * 100 + b2.w * 10 + b2.own;   // 536
                    var b3 = new B(3, 4);
                    viaThis = b3.v * 100 + b3.w * 10 + b3.own;        // 540
                    var c = new C();
                    noConstructor = c.v * 10 + c.w;                   // 51
                    virtualFromBase = b.fromVirtual;                  // 2
                    abstractBase = new T().v;                         // 11
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "implicitBase"), 517);
    assert_eq!(int_of(&emulator, "explicitBase"), 536);
    assert_eq!(int_of(&emulator, "viaThis"), 540);
    assert_eq!(int_of(&emulator, "noConstructor"), 51);
    assert_eq!(int_of(&emulator, "virtualFromBase"), 2);
    assert_eq!(int_of(&emulator, "abstractBase"), 11);
}

#[test]
fn a_missing_base_constructor_is_an_error() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class A { public A(int x) { } }
            public class B : A { }                          // CS7036: no base()
            public class C : A { public C() { } }           // CS7036 again
            public class D : A { public D() : base("x") { } } // no such overload
            public class Program { public static void Main() { } }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let count = bodies
        .errors
        .iter()
        .filter(|error| {
            matches!(
                error.kind,
                men_sharp_semantics::SemanticErrorKind::NoMatchingBaseConstructor { .. }
            )
        })
        .count();
    assert_eq!(count, 3, "{:#?}", bodies.errors);
}

#[test]
fn static_constructors_run_at_startup_after_field_initializers() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public static class Cfg
            {
                public static int seed;
                public static int fromInitializer = Other.Base * 2; // non-literal: runs at startup
                static Cfg() { seed = 42 + fromInitializer; }
            }
            public static class Other
            {
                public static int Base = Compute();
                static int Compute() { return 10; }
            }
            public class Program
            {
                public static int seed;
                public static int fromInitializer;
                public static void Main()
                {
                    seed = Cfg.seed;
                    fromInitializer = Cfg.fromInitializer;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "fromInitializer"), 20);
    assert_eq!(int_of(&emulator, "seed"), 62);
}

#[test]
fn explicit_implementations_win_for_their_own_interface() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public interface IA { int F(); }
            public interface IB { int F(); }
            public class C : IA, IB
            {
                int IA.F() { return 1; }
                int IB.F() { return 2; }
                public int F() { return 3; }
            }
            public sealed class D : IA, IB
            {
                int IB.F() { return 2; }
                public int F() { return 3; }   // implements IA
            }
            public class Program
            {
                public static int result;
                public static int viaSealed;
                public static void Main()
                {
                    var c = new C();
                    IA a = c;
                    IB b = c;
                    result = a.F() * 100 + b.F() * 10 + c.F();
                    var d = new D();
                    IA da = d;
                    IB db = d;
                    viaSealed = da.F() * 100 + db.F() * 10 + d.F();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 123);
    assert_eq!(int_of(&emulator, "viaSealed"), 323);
}

#[test]
fn object_members_dispatch_through_object_receivers() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class A
            {
                public int v;
                public override bool Equals(object o) { var a = o as A; return a != null && a.v == v; }
                public override int GetHashCode() { return v; }
                public override string ToString() { return "A" + v; }
            }
            public class B : A { }                     // inherits the overrides
            public class Plain { }                     // reference identity, "Game.Plain"
            public struct P { public int x; }          // synthesized Equals, type name
            public class Program
            {
                public static int equalsViaObject;
                public static int equalsViaBase;
                public static int hashViaObject;
                public static string toStringViaObject;
                public static string concat;
                public static string plain;
                public static string structName;
                public static int structEquals;
                public static int plainIdentity;
                public static string boxedInt;
                public static string nullConcat;
                public static void Main()
                {
                    object o = new A { v = 2 };
                    equalsViaObject = o.Equals(new A { v = 2 }) ? 1 : 0;
                    A a = new B { v = 3 };
                    equalsViaBase = a.Equals(new B { v = 3 }) ? 1 : 0;
                    hashViaObject = o.GetHashCode();
                    toStringViaObject = o.ToString();
                    concat = "x" + a + "y";
                    object p = new Plain();
                    plain = p.ToString();
                    object s = new P { x = 1 };
                    structName = s.ToString();
                    structEquals = s.Equals(new P { x = 1 }) ? 1 : 0;
                    plainIdentity = (p.Equals(new Plain()) ? 1 : 0) + (p.Equals(p) ? 10 : 0);
                    object i = 5;
                    boxedInt = i.ToString();
                    A none = null;
                    nullConcat = "n" + none;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "equalsViaObject"), 1);
    assert_eq!(int_of(&emulator, "equalsViaBase"), 1);
    assert_eq!(int_of(&emulator, "hashViaObject"), 2);
    assert_eq!(string_of(&emulator, "toStringViaObject"), "A2");
    assert_eq!(string_of(&emulator, "concat"), "xA3y");
    assert_eq!(string_of(&emulator, "plain"), "Game.Plain");
    assert_eq!(string_of(&emulator, "structName"), "Game.P");
    assert_eq!(int_of(&emulator, "structEquals"), 1);
    assert_eq!(int_of(&emulator, "plainIdentity"), 10);
    assert_eq!(string_of(&emulator, "boxedInt"), "5");
    assert_eq!(string_of(&emulator, "nullConcat"), "n");
}

#[test]
fn casts_is_and_as_test_the_runtime_type() {
    let Some(emulator) = run_with_corlib(
        r#"
        namespace Game
        {
            public interface IShape { int Area(); }
            public abstract class S : IShape { public abstract int Area(); }
            public class C : S { public int side; public override int Area() { return side * side; } }
            public class D : S { public override int Area() { return 0; } }
            public struct P : IShape { public int x; public int Area() { return x; } }
            public class Program
            {
                public static int asAndIs;
                public static int pattern;
                public static int conditional;
                public static int upcastAndNull;
                public static int externals;
                public static int viaInterface;
                public static int unboxCopies;
                public static void Main()
                {
                    S s = new C { side = 3 };
                    var c = s as C;
                    var d = s as D;
                    asAndIs = (c == null ? 0 : c.side) * 100 + (d == null ? 1 : 0) * 10 + ((s is C) ? 1 : 0);
                    if (s is C found) { pattern = found.side; }
                    if (s is D wrong) { pattern += 100; }
                    conditional = s is C ? 1 : 0;                 // `C ?` is the conditional, not C?
                    object none = null;
                    C fromNull = (C)none;                         // null passes a cast
                    S up = (S)new C { side = 2 };                 // upcast: no test
                    upcastAndNull = (fromNull == null ? 1 : 0) + up.Area() * 10;
                    object boxed = 5;
                    object text = "t";
                    externals = ((boxed is int) ? 1 : 0) + ((text is string) ? 10 : 0) + ((boxed is string) ? 100 : 0) + ((none is int) ? 1000 : 0);
                    IShape shape = new P { x = 7 };
                    var list = new System.Collections.Generic.List<IShape> { new C { side = 2 }, shape };
                    foreach (var item in list)
                    {
                        if (item is P p) { viaInterface += p.x; }
                        if (item is S ss) { viaInterface += ss.Area() * 10; }
                    }
                    P unboxed = (P)shape;
                    unboxed.x = 1;
                    unboxCopies = ((P)shape).x * 10 + unboxed.x;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "asAndIs"), 311);
    assert_eq!(int_of(&emulator, "pattern"), 3);
    assert_eq!(int_of(&emulator, "conditional"), 1);
    assert_eq!(int_of(&emulator, "upcastAndNull"), 41);
    assert_eq!(int_of(&emulator, "externals"), 11);
    assert_eq!(int_of(&emulator, "viaInterface"), 47);
    assert_eq!(int_of(&emulator, "unboxCopies"), 71);
}

#[test]
fn a_failed_cast_halts_the_program() {
    let Some((program, result)) = run_sources_result(
        vec![SourceCode::new(
            "test.cs",
            r#"
            namespace Game
            {
                public class S { }
                public class C : S { public int side = 3; }
                public class D : S { }
                public class Program
                {
                    public static int before;
                    public static int after;
                    public static void Main()
                    {
                        before = 1;
                        S s = new D();
                        C c = (C)s;
                        after = c.side;
                    }
                }
            }
            "#,
        )],
        "Main",
    ) else {
        return;
    };
    match result {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(
                message.contains("InvalidCastException") && message.contains("Game.C"),
                "{message}"
            );
        }
        Err(other) => panic!("expected a halt, got {other:?}\n{program}"),
        Ok(emulator) => panic!(
            "the cast was not checked: after = {:?}\n{program}",
            emulator.value_of("after")
        ),
    }
}

#[test]
fn external_types_are_tested_at_runtime_too() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static int tests;
                public static string asString;
                public static int asNull;
                public static void Main()
                {
                    object text = "t";
                    object number = 5;
                    object none = null;
                    tests = ((text is string) ? 1 : 0) + ((text is object) ? 10 : 0)
                        + ((number is int) ? 100 : 0) + ((number is string) ? 1000 : 0)
                        + ((none is object) ? 10000 : 0) + ((text is int) ? 100000 : 0);
                    asString = text as string;
                    var missing = number as string;
                    asNull = missing == null ? 1 : 0;
                    string cast = (string)text;      // checked, passes
                    string fromNull = (string)none;  // null passes
                    asString = cast + (fromNull == null ? "!" : "?");
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "tests"), 111);
    assert_eq!(string_of(&emulator, "asString"), "t!");
    assert_eq!(int_of(&emulator, "asNull"), 1);
}

#[test]
fn a_failed_cast_to_an_external_type_halts_too() {
    let Some((program, result)) = run_sources_result(
        vec![SourceCode::new(
            "test.cs",
            r#"
            namespace Game
            {
                public class Program
                {
                    public static int after;
                    public static void Main()
                    {
                        object number = 5;
                        string text = (string)number;
                        after = text.Length;
                    }
                }
            }
            "#,
        )],
        "Main",
    ) else {
        return;
    };
    match result {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(message.contains("InvalidCastException"), "{message}");
        }
        Err(other) => panic!("expected a halt, got {other:?}\n{program}"),
        Ok(emulator) => panic!(
            "the cast was not checked: after = {:?}\n{program}",
            emulator.value_of("after")
        ),
    }
}

#[test]
fn user_defined_operators_are_called() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public struct V
            {
                public int x; public int y;
                public V(int x, int y) { this.x = x; this.y = y; }
                public static V operator +(V a, V b) { return new V(a.x + b.x, a.y + b.y); }
                public static V operator -(V a, V b) { return new V(a.x - b.x, a.y - b.y); }
                public static V operator -(V a) { return new V(-a.x, -a.y); }
                public static V operator *(V a, int k) { return new V(a.x * k, a.y * k); }
                public static V operator *(int k, V a) { return new V(a.x * k, a.y * k); }
                public static bool operator ==(V a, V b) { return a.x == b.x && a.y == b.y; }
                public static bool operator !=(V a, V b) { return !(a == b); }
                public static bool operator <(V a, V b) { return a.x < b.x; }
                public static bool operator >(V a, V b) { return a.x > b.x; }
                public static V operator ++(V a) { return new V(a.x + 1, a.y + 1); }
                public static bool operator !(V a) { return a.x == 0 && a.y == 0; }
                public override bool Equals(object o) { return o is V v && v == this; }
                public override int GetHashCode() { return x * 31 + y; }
            }
            public class Money
            {
                public int cents;
                public Money(int c) { cents = c; }
                public static Money operator +(Money a, Money b) { return new Money(a.cents + b.cents); }
                public static bool operator ==(Money a, Money b)
                {
                    if ((object)a == null) { return (object)b == null; }
                    if ((object)b == null) { return false; }
                    return a.cents == b.cents;
                }
                public static bool operator !=(Money a, Money b) { return !(a == b); }
                public override bool Equals(object o) { return o is Money m && m == this; }
                public override int GetHashCode() { return cents; }
            }
            public class Program
            {
                public static int arithmetic;
                public static int comparisons;
                public static int mutation;
                public static int classes;
                public static void Main()
                {
                    var a = new V(1, 2);
                    var b = new V(10, 20);
                    var c = a + b; var d = b - a; var e = -a; var f = a * 3; var g = 2 * a;
                    arithmetic = c.x * 1000 + d.y * 10 + e.x + f.y + g.x;              // 11187
                    comparisons = (a == new V(1, 2) ? 1 : 0) + (a != b ? 10 : 0)
                        + (a < b ? 100 : 0) + (a > b ? 1000 : 0) + (!new V(0, 0) ? 10000 : 0);   // 10111
                    var h = a; h += b;                       // (11, 22)
                    var i = a; i++; ++i;                     // (3, 4)
                    var j = a; var k = j++;                  // j = (2, 3), k = (1, 2): the old value
                    mutation = h.x * 1000 + i.y * 100 + j.x * 10 + k.y;   // 11422
                    Money m = null;
                    Money n = new Money(5);
                    classes = (m == null ? 1 : 0) + (n == null ? 0 : 10) + (n != m ? 100 : 0)
                        + (n + new Money(1) == new Money(6) ? 1000 : 0)
                        + (n.Equals(new Money(5)) ? 10000 : 0);   // 11111
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "arithmetic"), 11187);
    assert_eq!(int_of(&emulator, "comparisons"), 10111);
    assert_eq!(int_of(&emulator, "mutation"), 11422);
    assert_eq!(int_of(&emulator, "classes"), 11111);
}

#[test]
fn comparison_operators_must_come_in_pairs() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public struct V
            {
                public int x;
                public static bool operator ==(V a, V b) { return a.x == b.x; }   // CS0216: no !=
                public static bool operator <(V a, V b) { return a.x < b.x; }
                public static bool operator >(V a, V b) { return a.x > b.x; }
            }
            public class Program { public static void Main() { } }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let pairs: Vec<String> = bodies
        .errors
        .iter()
        .filter_map(|error| match &error.kind {
            men_sharp_semantics::SemanticErrorKind::OperatorRequiresPair { operator, missing } => {
                Some(format!("{operator} needs {missing}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(pairs, vec!["== needs !="], "{:#?}", bodies.errors);
}

#[test]
fn default_interface_methods_run_unless_the_type_implements_them() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public interface IGreeter
            {
                string Name { get; }
                string Greet() { return "Hi " + Name; }         // default
                int Twice(int x) { return Helper(x) * 2; }      // calls through `this`
                int Helper(int x) { return x + 1; }
                string Title => "Mx";                           // default property
            }
            public interface IPolite : IGreeter
            {
                string IGreeter.Greet() { return "Good day " + Name; }   // re-implementation
            }
            public class Plain : IGreeter { public string Name => "plain"; }
            public class Custom : IGreeter
            {
                public string Name => "custom";
                public string Greet() { return "Yo " + Name; }
                public int Helper(int x) { return x + 10; }
            }
            public class Polite : IPolite { public string Name => "polite"; }
            public struct Unit : IGreeter { public int n; public string Name => "unit" + n; }
            public class Program
            {
                public static string greetings;
                public static int twice;
                public static string statically;
                public static void Main()
                {
                    IGreeter a = new Plain();
                    IGreeter b = new Custom();
                    IGreeter c = new Polite();
                    IGreeter d = new Unit { n = 3 };
                    greetings = a.Greet() + "|" + b.Greet() + "|" + c.Greet() + "|" + d.Greet() + "|" + a.Title;
                    twice = a.Twice(1) * 100 + b.Twice(1);      // 4*100 + 22
                    statically = Via(new Unit { n = 7 }) + Via(new Plain());
                }
                static string Via<T>(T g) where T : IGreeter { return g.Greet(); }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "greetings"),
        "Hi plain|Yo custom|Good day polite|Hi unit3|Mx"
    );
    assert_eq!(int_of(&emulator, "twice"), 422);
    assert_eq!(string_of(&emulator, "statically"), "Hi unit7Hi plain");
}

#[test]
fn a_default_method_is_not_a_member_of_the_class() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public interface IA { void G() { } }
            public class C : IA { }
            public class Program
            {
                public static void Main() { var c = new C(); c.G(); }   // CS1061: only through IA
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert!(
        bodies.errors.iter().any(|error| matches!(
            error.kind,
            men_sharp_semantics::SemanticErrorKind::UnknownMember { .. }
        )),
        "{:#?}",
        bodies.errors
    );
}

#[test]
fn conflicting_default_implementations_are_an_error() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let sources = vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public interface IA { int G() { return 1; } }
            public interface IB : IA { int IA.G() { return 2; } }
            public interface IC : IA { int IA.G() { return 3; } }
            public class Both : IB, IC { }                            // CS8705
            public interface ID : IB { int IA.G() { return 4; } }
            public class MostDerived : ID, IB { }                     // fine: ID re-implements IB's
            public class Program
            {
                public static int result;
                public static void Main() { IA a = new MostDerived(); result = a.G(); IA b = new Both(); result += b.G(); }
            }
        }
        "#,
    )];
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output
            .errors
            .iter()
            .any(|error| error.message.to_string().contains("CS8705")
                && error.message.to_string().contains("Game.Both")),
        "{:#?}",
        output.errors
    );
}

#[test]
fn is_patterns_beyond_types_match_as_in_csharp() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public enum Color { Red, Green, Blue }
            public abstract class Shape { public int Id; public abstract int Area(); }
            public class Circle : Shape
            {
                public int Radius; public Color Tint; public Circle Inner;
                public override int Area() { return 3 * Radius * Radius; }
            }
            public class Square : Shape { public int Side; public override int Area() { return Side * Side; } }
            public struct P { public int x; public int y; }
            public class Program
            {
                public static int constants;
                public static int relational;
                public static int properties;
                static int B(bool b, int bit) { return b ? bit : 0; }
                public static void Main()
                {
                    int n = 7; string s = "abc"; object o = 5; object nothing = null; Color c = Color.Green;
                    constants = B(n is 7, 1) + B(n is not 8, 2) + B(s is "abc", 4) + B(nothing is null, 8)
                        + B(o is not null, 16) + B(c is Color.Green, 32) + B(o is 5, 64)
                        + B(o is "5", 128) + B(nothing is 5, 256);                       // 127
                    relational = B(n is > 5, 1) + B(n is >= 7 and < 10, 2) + B(n is < 3 or > 6, 4)
                        + B(n is not (> 0 and < 5), 8) + B(o is > 3, 16) + B(s is > 3, 32)
                        + B(n is var anything && anything == 7, 64) + B(n is (7), 256);   // 351
                    Shape sh = new Circle { Id = 1, Radius = 2, Tint = Color.Blue, Inner = new Circle { Radius = 1 } };
                    Shape none = null;
                    P p = new P { x = 3, y = -1 };
                    properties = B(sh is Circle { Radius: 2 }, 1) + B(sh is Circle { Radius: > 5 }, 2)
                        + B(sh is { Id: 1 }, 4)
                        + B(sh is Circle { Tint: Color.Blue, Inner: { Radius: 1 } } found && found.Radius == 2, 8)
                        + B(none is { }, 16) + B(sh is not Square, 32) + B(p is { x: > 0, y: < 0 }, 64)
                        + B(sh is Circle { Inner: not null } c2 && c2.Area() == 12, 128)
                        + B(sh is Square { Side: 2 } or Circle { Radius: 2 }, 256)
                        + B(o is int i && i == 5, 512) + B(sh is Circle { Inner: { Inner: null } }, 1024);   // 2029
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "constants"), 127);
    assert_eq!(int_of(&emulator, "relational"), 351);
    assert_eq!(int_of(&emulator, "properties"), 2029);
}

#[test]
fn switch_statements_and_expressions_take_patterns() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public enum Color { Red, Green, Blue }
            public abstract class Shape { public abstract int Area(); }
            public class Circle : Shape { public int Radius; public override int Area() { return 3 * Radius * Radius; } }
            public class Square : Shape { public int Side; public override int Area() { return Side * Side; } }
            public class Program
            {
                public static int statements;
                public static int constants;
                public static string expressions;
                static int Classify(Shape s)
                {
                    switch (s)
                    {
                        case null: return -1;
                        case Circle { Radius: > 5 } big: return 100 + big.Radius;
                        case Circle c when c.Radius == 2: return 20;
                        case Circle c: return 10 + c.Radius;
                        case Square { Side: 0 }: return 0;
                        default: return 1;
                    }
                }
                static string Describe(object o) => o switch
                {
                    null => "null",
                    int i when i < 0 => "negative",
                    int i => "int" + i,
                    string s => "str:" + s,
                    Circle { Radius: var rad } => "circle" + rad,
                    _ => "other",
                };
                static int Old(Color c) { switch (c) { case Color.Red: return 1; case Color.Blue: return 3; default: return 2; } }
                static int Str(string s) { switch (s) { case "a": return 1; case "b": return 2; } return 0; }
                public static void Main()
                {
                    statements = Classify(null) * 10000 + Classify(new Circle { Radius = 7 }) * 100
                        + Classify(new Circle { Radius = 2 }) + Classify(new Circle { Radius = 3 });   // 733
                    constants = Old(Color.Red) * 100 + Old(Color.Green) * 10 + Old(Color.Blue) + Str("b") * 1000
                        + Classify(new Square { Side = 0 }) * 10000 + Classify(new Square { Side = 4 }) * 100000;   // 102123
                    int n = 5;
                    string size = n switch { < 3 => "small", >= 3 and < 10 => "medium", _ => "large" };
                    expressions = Describe(null) + Describe(-3) + Describe(4) + Describe("x")
                        + Describe(new Circle { Radius = 9 }) + Describe(1.5f) + "|" + size;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "statements"), 733);
    assert_eq!(int_of(&emulator, "constants"), 102123);
    assert_eq!(
        string_of(&emulator, "expressions"),
        "nullnegativeint4str:xcircle9other|medium"
    );
}

#[test]
fn a_switch_expression_with_no_matching_arm_halts() {
    let Some((program, result)) = run_sources_result(
        vec![SourceCode::new(
            "test.cs",
            r#"
            namespace Game
            {
                public class Program
                {
                    public static int after;
                    public static void Main()
                    {
                        int n = 5;
                        after = n switch { 1 => 10, 2 => 20 };
                    }
                }
            }
            "#,
        )],
        "Main",
    ) else {
        return;
    };
    match result {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(message.contains("SwitchExpressionException"), "{message}");
        }
        Err(other) => panic!("expected a halt, got {other:?}\n{program}"),
        Ok(emulator) => panic!(
            "no halt: after = {:?}\n{program}",
            emulator.value_of("after")
        ),
    }
}

#[test]
fn exceptions_are_thrown_caught_and_finalized() {
    let Some(emulator) = run(
        r#"
        using System;
        namespace Game
        {
            public class DoorLocked : InvalidOperationException
            {
                public int Code;
                public DoorLocked(int code) : base("locked " + code) { Code = code; }
            }
            public class Program
            {
                public static int caught;
                public static int values;
                public static string trace;
                static string log = "";
                static void Deep(int n) { if (n == 0) { throw new DoorLocked(42); } Deep(n - 1); }
                static int Fact(int n) { if (n < 0) throw new ArgumentException("neg"); return n <= 1 ? 1 : n * Fact(n - 1); }
                static int WithFinally(bool boom)
                {
                    try { log += "[t"; if (boom) throw new Exception("boom"); log += "n"; return 1; }
                    finally { log += "f]"; }
                }
                static int Loop()
                {
                    int hits = 0;
                    for (int i = 0; i < 3; i++)
                    {
                        try { if (i == 1) continue; if (i == 2) break; hits += 10; }
                        finally { hits += 1; }
                    }
                    return hits;   // 10+1, +1, +1
                }
                public static void Main()
                {
                    // by type, through recursion, with `when`
                    try { Deep(3); caught = -1; }
                    catch (DoorLocked e) when (e.Code == 41) { caught = 1; }
                    catch (DoorLocked e) when (e.Code == 42) { caught = e.Code; }
                    catch (Exception) { caught = 2; }
                    // nested, rethrow, finally order, base type
                    try
                    {
                        try { Fact(-1); }
                        catch (ArgumentException) { log += "a"; throw; }
                        finally { log += "F"; }
                    }
                    catch (Exception e) { log += "o:" + e.Message; }
                    // finally around return, normal and exceptional
                    int w1 = WithFinally(false);
                    int w2 = 0;
                    try { WithFinally(true); } catch (Exception e) { w2 = e.Message == "boom" ? 7 : 0; }
                    values = w1 * 10 + w2 + Loop() * 100 + Fact(4) * 10000;   // 241317
                    // the compiler's own checks
                    int[] arr = new int[2];
                    string checks = "";
                    try { arr[5] = 1; } catch (IndexOutOfRangeException) { checks += "I"; }
                    try { int x = arr[-1]; } catch (IndexOutOfRangeException) { checks += "i"; }
                    DoorLocked none = null;
                    try { int c = none.Code; } catch (NullReferenceException) { checks += "N"; }
                    try { none.ToString(); checks += "?"; } catch (NullReferenceException) { checks += "n"; }
                    int zero = arr[0];
                    try { int q = 10 / zero; } catch (DivideByZeroException) { checks += "D"; }
                    try { int q = 10 % zero; } catch (DivideByZeroException) { checks += "d"; }
                    object o = "str";
                    try { var d = (DoorLocked)o; } catch (InvalidCastException e) { checks += "C" + (e.Message.Length > 0 ? "m" : ""); }
                    try { int s = zero switch { 1 => 1 }; } catch (System.Runtime.CompilerServices.SwitchExpressionException) { checks += "S"; }
                    try { throw new ArgumentNullException("p"); } catch (ArgumentException e) { checks += "A" + e.Message.Length; }
                    var list = new System.Collections.Generic.List<int> { 1 };
                    try { list[3] = 1; } catch (ArgumentOutOfRangeException) { checks += "L"; }
                    var map = new System.Collections.Generic.Dictionary<string, int> { { "a", 1 } };
                    try { int v = map["b"]; } catch (System.Collections.Generic.KeyNotFoundException) { checks += "K"; }
                    try { map.Add("a", 2); } catch (ArgumentException) { checks += "k"; }
                    trace = log + "|" + checks + "|" + (10 / 2);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "caught"), 42);
    assert_eq!(int_of(&emulator, "values"), 241317);
    assert_eq!(
        string_of(&emulator, "trace"),
        "aFo:neg[tnf][tf]|IiNnDdCmSA21LKk|5"
    );
}

#[test]
fn an_uncaught_exception_reports_and_halts() {
    let Some((program, result)) = run_sources_result(
        vec![SourceCode::new(
            "test.cs",
            r#"
            using System;
            namespace Game
            {
                public class Program
                {
                    public static int before;
                    public static int after;
                    public static int finalized;
                    static void F() { throw new InvalidOperationException("door is locked"); }
                    public static void Main()
                    {
                        before = 1;
                        try { F(); } finally { finalized = 1; }
                        after = 2;
                    }
                }
            }
            "#,
        )],
        "Main",
    ) else {
        return;
    };
    match result {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(
                message.contains("System.InvalidOperationException: door is locked"),
                "{message}"
            );
        }
        Err(other) => panic!("expected a halt, got {other:?}\n{program}"),
        Ok(emulator) => panic!(
            "no halt: after = {:?}\n{program}",
            emulator.value_of("after")
        ),
    }
}

#[test]
fn throw_takes_an_exception_and_rethrow_needs_a_catch() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public class Program
            {
                public static void Main()
                {
                    throw "text";        // CS0155
                }
                static void G() { throw; }   // CS0156
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    let kinds: Vec<String> = bodies
        .errors
        .iter()
        .map(|error| format!("{:?}", error.kind))
        .filter(|kind| kind.contains("ThrowNeedsException") || kind.contains("RethrowOutsideCatch"))
        .collect();
    assert_eq!(kinds.len(), 2, "{:#?}", bodies.errors);
}

#[test]
fn exceptions_carry_their_site_and_a_stack_trace() {
    let Some(emulator) = run_sources(
        vec![SourceCode::new(
            "Assets/MenSharp/Door.cs",
            r#"
using System;
namespace Game
{
    class DoorLocked : InvalidOperationException { public DoorLocked() : base("locked") { } }
    class Door
    {
        public void Open(int n)
        {
            if (n == 0) { throw new DoorLocked(); }
            Open(n - 1);
        }
    }
    public class Program
    {
        public static string text;
        public static string rethrown;
        public static string generated;
        static void Middle() { new Door().Open(1); }
        public static void Main()
        {
            try { Middle(); }
            catch (DoorLocked e) { text = e.ToString(); }
            try { try { Middle(); } catch (Exception) { throw; } }
            catch (Exception e) { rethrown = e.StackTrace; }
            int[] a = new int[1];
            try { a[3] = 1; } catch (Exception e) { generated = e.ToString(); }
        }
    }
}
"#,
        )],
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "text"),
        "Game.DoorLocked: locked\n\
         \x20  at Game.Door.Open in Assets/MenSharp/Door.cs:10:27\n\
         \x20  at Game.Door.Open in Assets/MenSharp/Door.cs:11:17\n\
         \x20  at Game.Program.Middle in Assets/MenSharp/Door.cs:19:47\n\
         \x20  at Game.Program.Main in Assets/MenSharp/Door.cs:22:25"
    );
    // `throw;` keeps the site and trace; the outer catch sees one more frame
    assert_eq!(
        string_of(&emulator, "rethrown"),
        "   at Game.Door.Open in Assets/MenSharp/Door.cs:10:27\n\
         \x20  at Game.Door.Open in Assets/MenSharp/Door.cs:11:17\n\
         \x20  at Game.Program.Middle in Assets/MenSharp/Door.cs:19:47\n\
         \x20  at Game.Program.Main in Assets/MenSharp/Door.cs:24:31"
    );
    assert_eq!(
        string_of(&emulator, "generated"),
        "System.IndexOutOfRangeException: Index was outside the bounds of the array.\n\
         \x20  at Game.Program.Main in Assets/MenSharp/Door.cs:27:19"
    );
}

/// The address → source table marks where every function starts and where
/// the compiler's own halt is, so the Unity side neither attributes a halt
/// in generated code to the function that happens to precede it, nor
/// explains M#'s own stop after an unhandled exception as an engine error.
#[test]
fn the_source_table_marks_function_starts_and_the_halt() {
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let mut sources = vec![SourceCode::new(
        "Assets/MenSharp/Program.cs",
        r#"
        using System;
        namespace Game
        {
            abstract class Shape { public abstract int Area(); }
            class Circle : Shape { public int r = 2; public override int Area() { return 3 * r * r; } }
            public class Program
            {
                static int total;
                public static void Main()
                {
                    Shape shape = new Circle();
                    total = shape.Area();
                    throw new InvalidOperationException("boom");
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let meta = output.program.to_meta_json().unwrap();

    // one entry per line: (function, kind)
    let entries: Vec<(String, String)> = meta
        .lines()
        .filter(|line| line.trim_start().starts_with("{\"address\""))
        .map(|line| {
            let field = |name: &str| {
                let key = format!("\"{name}\": \"");
                let start = line.find(&key).map(|at| at + key.len());
                start.map_or(String::new(), |start| {
                    line[start..][..line[start..].find('"').unwrap()].to_string()
                })
            };
            (field("function"), field("kind"))
        })
        .collect();

    // every function's first entry is its start mark, positions follow
    let mut seen = std::collections::HashSet::new();
    for (function, kind) in &entries {
        if seen.insert(function.clone()) {
            assert_eq!(kind, "function", "{function} starts with {kind:?}\n{meta}");
        }
    }
    let main = |kind: &str| {
        entries
            .iter()
            .filter(|(function, k)| function == "Game.Program.Main" && k == kind)
            .count()
    };
    assert_eq!(main("function"), 1, "{meta}");
    assert!(main("") >= 3, "{meta}");
    // synthesized functions have a start too: the default constructor,
    // the unhandled exception report — and the compiler's own halt is
    // marked as such, inside the latter
    let report = "the unhandled exception report";
    assert!(
        entries
            .iter()
            .any(|(f, k)| f == "Game.Circle.Circle" && k == "function"),
        "{meta}"
    );
    assert!(
        entries.iter().any(|(f, k)| f == report && k == "function"),
        "{meta}"
    );
    assert!(
        entries.iter().any(|(f, k)| f == report && k == "halt"),
        "{meta}"
    );
}

/// `GetComponent<Door>()` for a program type: the UdonBehaviours on the
/// object are asked for their identity — `__program_id` against the ids of
/// `Door` and every subclass for a MenSharp program, `__refl_typeids` /
/// `__refl_typeid` for an UdonSharp one — through the corlib's searches.
#[test]
fn a_program_is_found_on_a_game_object_by_its_id() {
    let mut sources = vec![
        SourceCode::foreign(
            "Assets/Vendor/UCounter.cs",
            r#"
            namespace UdonSharp { public class UdonSharpBehaviour { } }
            public class UCounter : UdonSharp.UdonSharpBehaviour { public int count; }
            "#,
        ),
        SourceCode::new(
            "Assets/MenSharp/Switch.cs",
            r#"
            using MenSharp.Internal;
            namespace Game
            {
                public class Door : MenSharp.MenSharpBehaviour { public int opened; }
                public class SlidingDoor : Door { }
                public class Switch : MenSharp.MenSharpBehaviour
                {
                    public object target;
                    public int found;
                    public void Interact()
                    {
                        Door door = Programs.GetComponent<Door>(target);
                        Door[] doors = Programs.GetComponents<Door>(target);
                        UCounter counter = Programs.GetComponentInChildren<UCounter>(target, true);
                        if (door != null) { found = doors.Length; door.opened = 1; }
                        if (counter != null) { counter.count = 2; }
                    }
                }
            }
            "#,
        ),
    ];
    sources.extend(Compiler::corlib_sources());
    let Some(program) = compile_behaviour(sources, "Game.Switch") else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "{:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    let meta = program.output.program.to_meta_json().unwrap();
    // the engine call, with typeof(UdonBehaviour)
    assert!(
        text.contains(
            "UnityEngineComponent.__GetComponents__SystemType__UnityEngineComponentArray"
        ),
        "{text}"
    );
    assert!(
        text.contains("UnityEngineComponent.__GetComponentsInChildren__SystemType_SystemBoolean__UnityEngineComponentArray"),
        "{text}"
    );
    assert!(meta.contains("VRC.Udon.UdonBehaviour"), "{meta}");
    // the identities asked for
    for name in ["__program_id", "__refl_typeids", "__refl_typeid"] {
        assert!(
            meta.contains(&format!("\"value\": \"{name}\"")),
            "{name} missing:\n{meta}"
        );
    }
    // the ids of Door and of its subclass: FNV-1a of the class path, as the
    // programs' own heap slot 0 holds it
    let fnv = |path: &str| {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        (hash & 0x7fff_ffff_ffff_ffff) as i64
    };
    for path in ["Game.Door", "Game.SlidingDoor"] {
        let id = fnv(path).to_string();
        assert!(
            meta.contains(&format!("\"value\": \"{id}\"")),
            "{path} id missing:\n{meta}"
        );
    }
    // and not the id of an unrelated program (its own slot 0 aside)
    let unrelated = format!(
        "SystemInt64\", \"kind\": \"Int64\", \"value\": \"{}\"",
        fnv("Game.Switch")
    );
    assert!(!meta.contains(&unrelated), "{meta}");
    // the result is a program reference
    assert!(text.contains("%VRCUdonUdonBehaviour"), "{text}");
}

/// `[NetworkCallable]`: the event keeps its written name, its parameters
/// arrive in the layout's variables, and the sidecar carries the metadata
/// the runtime serializes the arguments by. Anything the rules refuse is an
/// error naming the method.
#[test]
fn a_network_callable_event_carries_its_metadata() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new(
                "base.cs",
                r#"
                namespace MenSharp
                {
                    public class MenSharpBehaviour
                    {
                        public object gameObject { get; }
                        public void SendCustomEvent(string eventName) { }
                    }
                }
                namespace VRC.SDK3.UdonNetworkCalling
                {
                    public class NetworkCallableAttribute : System.Attribute
                    {
                        public NetworkCallableAttribute() { }
                        public NetworkCallableAttribute(int maxEventsPerSecond) { }
                    }
                }
                "#,
            ),
            SourceCode::new(
                "Assets/MenSharp/Turret.cs",
                r#"
                using VRC.SDK3.UdonNetworkCalling;
                namespace Game
                {
                    public enum Mode { Slow, Fast }
                    public class Turret : MenSharp.MenSharpBehaviour
                    {
                        public int hits;
                        public Turret other;
                        [NetworkCallable]
                        public void Hit(int damage, string by) { hits += damage; }
                        [NetworkCallable(5)]
                        public void Aim(float[] at, Mode mode) { }
                        public void Interact() { other.SendCustomEvent(nameof(Hit)); }
                    }
                }
                "#,
            ),
        ],
        "Game.Turret",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "{:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    let meta = program.output.program.to_meta_json().unwrap();
    // never mangled, parameters in the layout's variables
    assert!(text.contains(".export Hit\n"), "{text}");
    assert!(text.contains(".export Aim\n"), "{text}");
    assert!(text.contains("__0_damage__param: %SystemInt32"), "{text}");
    assert!(text.contains("__0_at__param: %SystemSingleArray"), "{text}");
    // the metadata: variable names and .NET types, the rate
    assert!(
        meta.contains(r#"{"event": "Hit", "maxEventsPerSecond": 0, "parameters": [ {"name": "__0_damage__param", "type": "System.Int32"}, {"name": "__0_by__param", "type": "System.String"} ]}"#),
        "{meta}"
    );
    assert!(
        meta.contains(r#"{"event": "Aim", "maxEventsPerSecond": 5, "parameters": [ {"name": "__0_at__param", "type": "System.Single[]"}, {"name": "__0_mode__param", "type": "System.Int32"} ]}"#),
        "{meta}"
    );
    // MenSharpBehaviour's own operation on another behaviour is the extern
    // on that program
    assert!(
        text.contains(
            "VRCUdonCommonInterfacesIUdonEventReceiver.__SendCustomEvent__SystemString__SystemVoid"
        ),
        "{text}"
    );
}

#[test]
fn what_a_network_callable_cannot_be_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "Assets/MenSharp/Turret.cs",
                r#"
                namespace Game
                {
                    public class Ammo { public int count; }
                    public class Turret : MenSharp.MenSharpBehaviour
                    {
                        [NetworkCallable] private void Hidden(int x) { }
                        [NetworkCallable] public static void Shared(int x) { }
                        [NetworkCallable] public virtual void Open(int x) { }
                        [NetworkCallable] public void Reload(Ammo ammo) { }
                        [NetworkCallable] public void Interact() { }
                    }
                }
                "#,
            ),
        ],
        "Game.Turret",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    for expected in [
        "must be public: `Hidden`",
        "cannot be static: `Shared`",
        "cannot be virtual, abstract or an override: `Open`",
        "network callable `Reload` has a type the network cannot carry",
        "`Interact` is a built-in event",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(expected)),
            "{expected}: {messages:?}"
        );
    }
}

/// An implicit numeric conversion is real code on Udon: an `Int32`
/// constant copied into a `Single` slot stays an `Int32`, and the first
/// extern reading it throws. So `float f = 1`, `Half(3)`, `new V(1, 2)`,
/// `return 1` in a float method and `{ 1, 2 }` in a float array all
/// convert.
#[test]
fn integer_values_convert_where_a_float_is_expected() {
    let mut sources = vec![SourceCode::new(
        "test.cs",
        r#"
        namespace Game
        {
            public struct V { public float x; public V(float a, float b) { x = a + b; } }
            public class Program : MenSharp.MenSharpBehaviour
            {
                public float total;
                public float[] samples;
                static float Half(float x) { return x / 2; }
                static float One() { return 1; }
                static float Sum(params float[] values) { return 1; }
                public void Interact()
                {
                    float f = 1;
                    total = 2;
                    V v = new V(1, 2);
                    samples = new float[] { 1, 2 };
                    total = Half(3) + f + v.x + One() + Sum(1, 2);
                }
            }
        }
        "#,
    )];
    sources.extend(Compiler::corlib_sources());
    let Some(program) = compile_behaviour(sources, "Game.Program") else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "{:#?}",
        program.output.errors
    );
    let text = program.output.program.to_uasm().unwrap();
    let conversions = text
        .matches("SystemConvert.__ToSingle__SystemInt32__SystemSingle")
        .count();
    // f, total, V(1, 2), the two array elements, Half(3), One's return, Sum's two
    assert!(conversions >= 10, "{conversions} conversions:\n{text}");
}

/// A behaviour placed where MenSharp does not look for sources — outside
/// Assets/MenSharp, in no assembly of its own — is read as a library and
/// would be fed to UdonSharp's C# 7.3 pass: an error saying where it
/// belongs, not a silently missing program.
#[test]
fn a_behaviour_outside_the_mensharp_sources_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::foreign(
                "Assets/MyGimmick/Door.cs",
                r#"
                public class Door : MenSharp.MenSharpBehaviour
                {
                    public void Interact() { }
                }
                "#,
            ),
        ],
        "Door",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages.iter().any(|message| message
            .contains("`Door` inherits MenSharpBehaviour but is outside the MenSharp sources")),
        "{messages:?}"
    );
    assert_eq!(
        program.output.errors[0].file,
        men_sharp_semantics::FileId(1),
        "{:?}",
        program.output.errors
    );
}

#[test]
fn lambdas_delegates_and_closures_work() {
    let source = r#"
        using System;
        namespace Game
        {
            public delegate int Op(int a);
            public class Counter
            {
                public int Count;
                public void Bump(int by) { Count += by; }
                public virtual int Twice(int x) { return x * 2; }
            }
            public class Program
            {
                public static int Result;
                public static int Captured;
                public static string Trace = "";
                public static int Twice(int x) { return x * 2; }
                public static int Apply(Func<int, int> f, int x) { return f(x); }
                public static void Main()
                {
                    Func<int, int> f = x => x + 1;
                    Result = f(2);                       // 3
                    Result += f.Invoke(3);               // 7
                    Op o = y => y * 3;
                    Result += o(4);                      // 19
                    Result += o.Invoke(1);               // 22
                    Func<int, int> g = Twice;
                    Result += g(5);                      // 32
                    Result += Apply(v => v * 100, 1);    // 132
                    Predicate<int> p = v => v > 3;
                    if (p(5)) Result += 1000;            // 1132
                    Action none = () => { Trace += "!"; };
                    none();
                    none();

                    int captured = 10;
                    Action bump = () => { captured++; };
                    bump();
                    bump();
                    Captured = captured;                 // 12

                    // instance method groups and closures over objects
                    var counter = new Counter();
                    Action<int> add = counter.Bump;
                    add(5);
                    add(6);
                    Func<int, int> twice = counter.Twice;
                    Result += twice(counter.Count);      // 1132 + 22 = 1154

                    // a closure made per iteration keeps its own variable
                    Func<int>[] makers = new Func<int>[3];
                    for (int i = 0; i < 3; i++)
                    {
                        int square = i * i;
                        makers[i] = () => square + i;
                    }
                    // i is shared (one variable for the loop), square is not
                    Result += makers[0]() + makers[1]() + makers[2]();  // 1154 + (0+3)+(1+3)+(4+3) = 1168

                    // nested lambdas capturing through two levels
                    int outer = 1;
                    Func<Func<int>> make = () => { int inner = 2; return () => outer + inner; };
                    Func<int> made = make();
                    outer = 5;
                    Result += made();                    // 1168 + 7 = 1175
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(int_of(&emulator, "Result"), 1175);
    assert_eq!(int_of(&emulator, "Captured"), 12);
    assert_eq!(string_of(&emulator, "Trace"), "!!");
}

#[test]
fn delegates_recurse_and_carry_generics_and_by_ref_parameters() {
    let source = r#"
        using System;
        namespace Game
        {
            public delegate void Splitter(int value, out int high, out int low);
            public delegate T Picker<T>(T a, T b);
            public class Holder<T>
            {
                public Func<T, T> Step;
                public T Value;
                public Holder(T start, Func<T, T> step) { Value = start; Step = step; }
                public void Advance() { Value = Step(Value); }
            }
            public class Program
            {
                public static int Result;
                public static int Errors;
                public static string Words = "";
                public static T Best<T>(T a, T b, Func<T, T, bool> better) { return better(a, b) ? a : b; }
                public static void Each(int[] items, Action<int> action)
                {
                    foreach (int item in items) action(item);
                }
                public static void Main()
                {
                    // recursion through a delegate the lambda itself captures
                    Func<int, int> fact = null;
                    fact = n => n <= 1 ? 1 : n * fact(n - 1);
                    Result = fact(5);                                   // 120

                    // generic methods and classes taking delegates
                    Result += Best(3, 9, (a, b) => a > b);              // 129
                    Words = Best("pear", "fig", (a, b) => a.Length < b.Length);
                    Picker<string> longer = (a, b) => a.Length >= b.Length ? a : b;
                    Words += longer("x", "yyy");                        // figyyy
                    var holder = new Holder<int>(1, v => v * 3);
                    holder.Advance();
                    holder.Advance();
                    Result += holder.Value;                             // 138

                    // out parameters through a delegate of our own
                    Splitter split = (int value, out int high, out int low) => { high = value / 10; low = value % 10; };
                    int h, l;
                    split(47, out h, out l);
                    Result += h * 100 + l;                              // 545

                    // a lambda as an argument, called from a loop
                    int sum = 0;
                    Each(new int[] { 1, 2, 3 }, v => { sum += v; });
                    Result += sum;                                      // 551

                    // a null delegate throws NullReferenceException
                    Action nothing = null;
                    try { nothing(); } catch (NullReferenceException) { Errors++; }
                    Func<int> alsoNothing = null;
                    try { Result += alsoNothing(); } catch (Exception) { Errors++; }
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(int_of(&emulator, "Result"), 551);
    assert_eq!(int_of(&emulator, "Errors"), 2);
    assert_eq!(string_of(&emulator, "Words"), "figyyy");
}

#[test]
fn lambdas_reach_this_fields_and_behaviour_fields() {
    let source = r#"
        using System;
        using MenSharp;
        namespace Game
        {
            public class Tally
            {
                public int Total;
                public Func<int, int> Scale = x => x * 2;
                public Action<int> Adder()
                {
                    return amount => { Total += Scale(amount); };
                }
            }
            public class Program : MenSharpBehaviour
            {
                public int hits;
                public string log = "";
                public Action onHit;
                public void Register()
                {
                    string prefix = "hit";
                    onHit = () => { hits++; log += prefix + hits; };
                }
                public void Fire()
                {
                    if (onHit != null) onHit();
                    var tally = new Tally();
                    Action<int> add = tally.Adder();
                    add(5);
                    add(6);
                    hits += tally.Total;
                }
            }
        }
    "#;
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let Some(program) = compile_behaviour(sources, "Game.Program") else {
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let assembled = program.output.program.assemble().unwrap();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    for event in ["Register", "Fire", "Fire"] {
        emulator.run(&assembled, event).unwrap_or_else(|error| {
            panic!(
                "emulator error: {error:?}\n{}",
                program.output.program.dump()
            )
        });
    }
    // two hits, each adding the tally 2*5 + 2*6 = 22
    assert_eq!(int_of(&emulator, "hits"), 2 + 22 * 2);
    assert_eq!(string_of(&emulator, "log"), "hit1hit24");
}

#[test]
fn delegates_combine_remove_compare_and_raise_events() {
    let source = r#"
        using System;
        namespace Game
        {
            public delegate void Handler(int a);
            public class Door
            {
                public event Action Opened;
                public static event Handler Any;
                public int Times;
                public void Open() { Times++; Opened?.Invoke(); Any?.Invoke(Times); }
            }
            public class Node
            {
                public string Name = "n";
                public Node Next;
            }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                static void A() { Log += "a"; }
                static void B() { Log += "b"; }
                public static void Main()
                {
                    Action a = A;
                    Action b = B;
                    Action c = a + b;
                    c();                                    // ab
                    c += a;
                    c();                                    // ababa
                    c -= a;
                    c();                                    // ababaab
                    c = c - b;
                    c();                                    // ababaaba
                    c -= a;
                    if (c == null) Log += "0";              // ababaaba0
                    Action none = null;
                    none += a;
                    none();                                 // ababaaba0a

                    Action a2 = A;
                    if (a == a2) Result += 1;
                    if (a != b) Result += 10;
                    Action lam = () => { };
                    Action lam2 = () => { };
                    if (lam != lam2) Result += 100;
                    if ((a + b) == (a + b)) Result += 1000; // 1111

                    var door = new Door();
                    door.Opened += () => { Result += 10000; };
                    int seen = 0;
                    Handler h = t => { seen += t; };
                    Door.Any += h;
                    door.Open();
                    door.Open();                            // 21111, seen 3
                    Door.Any -= h;
                    door.Open();                            // 31111
                    Result += seen;                         // 31114

                    var quiet = new Door();
                    quiet.Open();                           // no subscribers: nothing raised
                    Result += quiet.Times;                  // 31115

                    Node missing = null;
                    string name = missing?.Name;
                    if (name == null) Result += 1;          // 31116
                    var node = new Node();
                    if (node?.Name == "n") Result += 2;     // 31118
                    if (node.Next?.Next?.Name == null) Result += 4;  // 31122
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "ababaaba0a");
    assert_eq!(int_of(&emulator, "Result"), 31122);
}

#[test]
fn list_methods_take_delegates() {
    let source = r#"
        using System;
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Order = "";
                public static void Main()
                {
                    var list = new List<int>();
                    list.Add(5);
                    list.Add(1);
                    list.Add(4);
                    list.Add(2);
                    list.Sort((x, y) => x - y);
                    list.ForEach(v => { Order += v; });            // 1245
                    Result += list.Find(v => v > 3);               // 4
                    Result += list.FindIndex(v => v == 5) * 10;    // 34
                    if (list.Exists(v => v == 2)) Result += 100;   // 134
                    if (!list.TrueForAll(v => v > 1)) Result += 1000;   // 1134
                    Result += list.RemoveAll(v => v % 2 == 0) * 10000;  // 21134
                    var strings = list.ConvertAll(v => "<" + v + ">");
                    Order += strings[0] + strings[1];              // 1245<1><5>
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Order"), "1245<1><5>");
    assert_eq!(int_of(&emulator, "Result"), 21134);
}

/// What a delegate cannot do on Udon is an error at the use, not a program
/// that jumps into another program's addresses or dereferences a null.
#[test]
fn what_a_delegate_cannot_do_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "Assets/MenSharp/Panel.cs",
                r#"
                using System;
                namespace Game
                {
                    public class Other : MenSharp.MenSharpBehaviour
                    {
                        public Action onHit;
                        public int number;
                        public void Watch(Action callback) { }
                    }
                    public class Panel : MenSharp.MenSharpBehaviour
                    {
                        public Other other;
                        public event Action Custom { add { } remove { } }
                        public void Interact()
                        {
                            Func<string, int> parse = int.Parse;
                            other.onHit = () => { };
                            other.Watch(() => { });
                            Custom += () => { };
                            int? count = other?.number;
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Panel",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    for expected in [
        "an engine method cannot become a delegate on Udon yet",
        "`onHit` is a delegate, which cannot cross into another program",
        "`Watch` takes or returns a delegate, which cannot cross into another program",
        "an event with `add`/`remove` accessors is not supported",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(expected)),
            "missing {expected:?} in {messages:#?}"
        );
    }
}

#[test]
fn nullable_value_types_work() {
    let source = r#"
        using System;
        namespace Game
        {
            public struct Point
            {
                public int X;
                public int Y;
                public Point(int x, int y) { X = x; Y = y; }
                public static Point operator +(Point a, Point b) { return new Point(a.X + b.X, a.Y + b.Y); }
            }
            public class Box
            {
                public int Count = 3;
                public Box Next;
            }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int? Twice(int? v) { return v * 2; }
                public static void Main()
                {
                    int? a = null;
                    int? b = 5;
                    if (!a.HasValue) Result += 1;                       // 1
                    if (b.HasValue) Result += 2;                        // 3
                    Result += b.Value;                                  // 8
                    Result += a.GetValueOrDefault();                    // 8
                    Result += a.GetValueOrDefault(10);                  // 18
                    Result += a ?? 100;                                 // 118
                    int? c = a + b;
                    if (c == null) Result += 1000;                      // 1118
                    int? d = b + 1;
                    Result += d.Value;                                  // 1124
                    if (a == null) Result += 10000;                     // 11124
                    if (b != null) Result += 20000;                     // 31124
                    if (b == 5) Result += 100000;                       // 131124
                    if (a < 3) Result += 7;                             // null: false
                    if (b > 3) Result += 1000000;                       // 1131124
                    b++;
                    Result += b.Value;                                  // 1131130
                    b += 10;
                    Result += (int)b;                                   // 1131146
                    int? e = -b;
                    Result += e.Value;                                  // 1131130
                    long? wide = b;
                    if (wide == 16) Result += 3;                        // 1131133
                    Log += a.ToString() + "|" + b.ToString();           // |16
                    Log += "|" + a;                                     // |16|
                    Log += "|" + (Twice(b) ?? -1) + "|" + (Twice(a) ?? -1);   // |16||32|-1
                    object boxed = b;
                    if (boxed is int v) Result += v;                    // 1131149
                    int? back = (int?)boxed;
                    Result += back.Value;                               // 1131165
                    switch (a) { case null: Log += "N"; break; case 1: Log += "1"; break; }
                    switch (b) { case null: Log += "N"; break; case 16: Log += "S"; break; default: Log += "D"; break; }
                    if (b is int w && w == 16) Log += "W";
                    if (a is null) Log += "Z";
                    Point? p = null;
                    Point? q = new Point(1, 2);
                    if (p == null) Result += 1;                         // 1131166
                    Point sum = q.Value + new Point(3, 4);
                    Result += sum.X * 10 + sum.Y;                       // 1131212
                    Point? r = p ?? q;
                    Result += r.Value.X;                                // 1131213
                    Box none = null;
                    Box some = new Box();
                    int? n1 = none?.Count;
                    int? n2 = some?.Count;
                    if (n1 == null && n2 == 3) Result += 5;             // 1131218
                    Result += some.Next?.Count ?? 7;                    // 1131225
                    int? def = default;
                    if (def == null) Result += 1;                       // 1131226
                    int?[] arr = new int?[2];
                    arr[1] = 9;
                    if (arr[0] == null) Result += arr[1].Value;         // 1131235
                    try { Result += a.Value; } catch (InvalidOperationException) { Log += "E"; }
                    int? x = null;
                    int? y = null;
                    if (x == y) Log += "Q";
                    if (x != b) Log += "R";
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "|16||32|-1NSWZEQR");
    assert_eq!(int_of(&emulator, "Result"), 1131235);
}

#[test]
fn a_conditional_takes_the_type_its_context_wants() {
    let source = r#"
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int? Pick(bool yes) { return yes ? 1 : null; }
                public static void Main()
                {
                    bool on = Result == 0;
                    int? a = on ? 5 : null;                     // target-typed: int?
                    int? b = !on ? 5 : null;
                    if (a == 5 && b == null) Result += 1;       // 1
                    long wide = on ? 1 : 2L;                    // natural type long
                    if (wide == 1) Result += 10;                // 11
                    double d = on ? 1 : 2.5;                    // 1 converted to double
                    if (d == 1.0) Result += 100;                // 111
                    string s = on ? "yes" : null;
                    Log += s ?? "none";
                    Log += Pick(false) ?? -1;                   // yes-1
                    Result += Pick(true) ?? 0;                  // 112
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "yes-1");
    assert_eq!(int_of(&emulator, "Result"), 112);
}

#[test]
fn local_functions_run_capture_and_recurse() {
    let source = r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static int Total;
                public static string Log = "";
                public static int Field = 7;

                public int Instance = 3;
                public int UsesThis()
                {
                    int Doubled() { return Instance * 2; }
                    return Doubled();
                }

                public static void Main()
                {
                    // called before it is written, and after
                    Result = Twice(3);                          // 6
                    int Twice(int x) { return x * 2; }
                    Result += Twice(4);                         // 14

                    // an expression body, a default argument and a named one
                    int Step(int x, int by = 10) => x + by;
                    Result += Step(1);                          // 25
                    Result += Step(by: 5, x: 0);                // 30

                    // recursion, and mutual recursion between two of them
                    int Fact(int n) { return n <= 1 ? 1 : n * Fact(n - 1); }
                    Result += Fact(5);                          // 150
                    bool Even(int n) { return n == 0 ? true : Odd(n - 1); }
                    bool Odd(int n) { return n == 0 ? false : Even(n - 1); }
                    if (Even(10) && Odd(7)) Result += 1000;     // 1150

                    // capture: the variable is shared, not copied
                    int total = 0;
                    void Add(int by) { total += by; }
                    Add(2);
                    Add(3);
                    total += 1;
                    Add(4);
                    Total = total;                              // 10

                    // a variable the function shares with the body
                    string word = "hi";
                    void Shout() { Log += word + "!"; }
                    Shout();
                    word = "bye";
                    Shout();                                    // hi!bye!

                    // `ref` and `out` parameters
                    void Swap(ref int a, ref int b) { int t = a; a = b; b = t; }
                    int left = 1;
                    int right = 2;
                    Swap(ref left, ref right);
                    Result += left * 10 + right;                // 1150 + 21 = 1171
                    void Split(int value, out int high, out int low)
                    {
                        high = value / 10;
                        low = value % 10;
                    }
                    Split(48, out int h, out int l);
                    Result += h + l;                            // 1171 + 12 = 1183

                    // a static one, and one reaching a static field
                    static int Pure(int x) { return x + 1; }
                    int Reads() { return Field; }
                    Result += Pure(0) + Reads();                // 1183 + 8 = 1191

                    // one nested in another, capturing through both levels
                    int outer = 100;
                    int Outer()
                    {
                        int middle = 20;
                        int Inner() { return outer + middle; }
                        return Inner() + Inner();
                    }
                    Result += Outer();                          // 1191 + 240 = 1431

                    // a lambda calling a local function that captures
                    Func<int, int> through = n => Twice(n) + total;
                    Result += through(5);                       // 1431 + 20 = 1451

                    // a local function as a delegate
                    Func<int, int> group = Twice;
                    Result += group(6);                         // 1463

                    // callers that never name `counter` still have to hand
                    // it to the function that does
                    int counter = 0;
                    void Bump() { counter++; }
                    void BumpTwice() { Bump(); Bump(); }
                    Action raise = () => Bump();
                    BumpTwice();
                    raise();
                    Result += counter;                          // 1466

                    // called from a loop body, capturing the loop variable
                    for (int i = 0; i < 3; i++)
                    {
                        void Mark() { Log += i; }
                        Mark();
                    }                                           // hi!bye!012

                    // an instance member's local function, using `this`
                    Result += new Program().UsesThis();         // 1472
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "hi!bye!012");
    assert_eq!(int_of(&emulator, "Total"), 10);
    assert_eq!(int_of(&emulator, "Result"), 1472);
}

#[test]
fn local_functions_live_anywhere_a_block_does() {
    let source = r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int Seed = 4;

                public int Value;
                public int Doubled => Doubling();
                public Program(int start)
                {
                    int Adjust(int x) { return x + Bonus(); }
                    Value = Adjust(start);
                }
                public int Bonus() { return 2; }
                private int Doubling()
                {
                    int Twice() { return Value * 2; }
                    return Twice();
                }

                public static void Main()
                {
                    // inside a nested block, and inside a loop body
                    if (Seed > 0)
                    {
                        int Half(int x) { return x / 2; }
                        Result += Half(Seed);                   // 2
                    }
                    for (int i = 0; i < 3; i++)
                    {
                        string Tag() { return "<" + i + ">"; }
                        Log += Tag();
                    }                                           // <0><1><2>

                    // inside a lambda body, capturing the lambda's parameter
                    Func<int, int> outer = n =>
                    {
                        int Plus(int x) { return x + n; }
                        return Plus(10);
                    };
                    Result += outer(5);                         // 17

                    // its own `try`/`catch`, and a `throw` that leaves it
                    int Risky(int x)
                    {
                        try
                        {
                            if (x == 0) throw new Exception("zero");
                            return 100 / x;
                        }
                        catch (Exception e)
                        {
                            Log += e.Message;
                            return -1;
                        }
                    }
                    Result += Risky(4) + Risky(0);              // 17 + 25 - 1 = 41

                    // calling instance and static members of the class
                    var program = new Program(1);
                    Result += program.Value + program.Doubled;  // 41 + 3 + 6 = 50
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "<0><1><2>zero");
    assert_eq!(int_of(&emulator, "Result"), 50);
}

#[test]
fn a_local_function_reaching_a_variable_that_has_no_value_yet_is_an_error() {
    let Some(program) = compile_behaviour(
        vec![
            SourceCode::new("base.cs", SELF_REFERENCE_BASE),
            SourceCode::new(
                "Assets/MenSharp/Early.cs",
                r#"
                namespace Game
                {
                    public class Early : MenSharp.MenSharpBehaviour
                    {
                        public string Log;
                        public void Interact()
                        {
                            // the call runs before `word` is a variable at
                            // all — C# calls that a use before assignment
                            Show();
                            string word = "hi";
                            void Show() { Log += word; }
                            Show();
                        }
                    }
                }
                "#,
            ),
        ],
        "Game.Early",
    ) else {
        eprintln!("skipped: no .NET runtime for reference assemblies");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("uses `word` of the enclosing method")),
        "{messages:#?}"
    );
}

#[test]
fn a_string_can_be_indexed() {
    let source = r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static int Result;
                public static string Word = "hello";
                public static char At(string text, int index) { return text[index]; }
                public static void Main()
                {
                    Log += Word[0];                          // h
                    Log += Word[Word.Length - 1];            // o
                    char c = At("abc", 1);
                    Log += c;                                // b
                    if (Word[1] == 'e') Result += 1;         // 1
                    for (int i = 0; i < Word.Length; i++)
                    {
                        if (char.IsLetter(Word[i])) Result += 10;
                    }                                        // 51
                    string built = "";
                    foreach (char ch in Word) built += ch;
                    if (built == Word) Result += 100;        // 151
                    Log += Word.Substring(1, 2)[1];          // l
                    try
                    {
                        Log += Word[9];
                    }
                    catch (IndexOutOfRangeException)
                    {
                        Log += "!";
                    }
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "hobl!");
    assert_eq!(int_of(&emulator, "Result"), 151);
}

#[test]
fn an_array_can_be_written_with_braces_alone() {
    let source = r#"
        namespace Game
        {
            public class Holder
            {
                public int[] Numbers = { 1, 2, 3 };
                public string[] Names = { "a", "b" };
                public int[] Empty = { };
            }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int[] Steps = { 5, 6, 7 };
                public static float[] Ratios = { 0.5f, 1.5f };
                public static void Main()
                {
                    int[] local = { 10, 20, 30 };
                    long[] widened = { 1, 2 };            // int literals into a long[]
                    Result = local[0] + local[2];         // 40
                    Result += Steps[1];                   // 46
                    Result += local.Length + Steps.Length; // 52
                    var holder = new Holder();
                    Result += holder.Numbers[2];          // 55
                    Result += holder.Empty.Length;        // 55
                    Log += holder.Names[1];               // b
                    if (Ratios[1] == 1.5f) Result += 100; // 155
                    if (widened[1] == 2L) Result += 1000; // 1155
                    foreach (int step in Steps) Result += step;  // 1173
                    local[1] = 5;
                    Result += local[1];                   // 1178
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "b");
    assert_eq!(int_of(&emulator, "Result"), 1178);
}

#[test]
fn an_external_indexer_becomes_its_get_item_extern() {
    // no engine assemblies here, so this borrows a metadata type that has an
    // indexer and is on Udon's list: `Match.Groups[i]` / `Groups[name]`
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let regex = dir.join("System.Text.RegularExpressions.dll");
    let Ok(regex_bytes) = std::fs::read(&regex) else {
        eprintln!("skipped: no System.Text.RegularExpressions.dll");
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap(),
        regex_bytes,
    ];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        using System.Text.RegularExpressions;
        namespace Game
        {
            public class Program
            {
                public static string Result;
                public static void Main()
                {
                    Match found = Regex.Match("ab", "(?<pair>a)(b)");
                    Result = found.Groups[1].Value + found.Groups["pair"].Value;
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );
    let dump = output.program.dump();
    for expected in [
        "SystemTextRegularExpressionsGroupCollection.__get_Item__SystemInt32__\
         SystemTextRegularExpressionsGroup",
        "SystemTextRegularExpressionsGroupCollection.__get_Item__SystemString__\
         SystemTextRegularExpressionsGroup",
    ] {
        assert!(dump.contains(expected), "missing {expected} in\n{dump}");
    }
}

#[test]
fn a_conversion_operator_from_metadata_is_applied() {
    // `DateTime` -> `DateTimeOffset` is a user-defined implicit conversion,
    // and one of the few whose types are both in the core library
    let Some(dir) = dotnet_shared_dir() else {
        return;
    };
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let files = compiler.parse(vec![SourceCode::new(
        "test.cs",
        r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static void Take(DateTimeOffset moment) { }
                public static void Main()
                {
                    DateTime now = DateTime.UtcNow;
                    DateTimeOffset moment = now;   // the operator, implicitly
                    Take(now);                     // and at a call
                }
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    assert!(
        output.errors.is_empty(),
        "codegen errors: {:#?}",
        output.errors
    );
    let dump = output.program.dump();
    let expected = "SystemDateTimeOffset.__op_Implicit__SystemDateTime__SystemDateTimeOffset";
    assert_eq!(
        dump.matches(expected).count(),
        2,
        "expected both conversions in\n{dump}"
    );
}

#[test]
fn target_typed_new_takes_the_type_the_context_wants() {
    let source = r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Counter
            {
                public int Count;
                public Counter() { Count = 1; }
                public Counter(int start) { Count = start; }
            }
            public struct Point
            {
                public int X;
                public Point(int x) { X = x; }
            }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static Counter Field = new();
                public static int Sum(List<int> values)
                {
                    int total = 0;
                    foreach (int value in values) total += value;
                    return total;
                }
                public static Counter Make() => new(7);
                public static void Main()
                {
                    Counter a = new();
                    Counter b = new(5);
                    Result = a.Count + b.Count;              // 6
                    Result += Field.Count;                   // 7
                    Result += Make().Count;                  // 14
                    List<int> values = new() { 1, 2, 3 };
                    Result += Sum(values);                   // 20
                    Result += Sum(new List<int> { 4 });      // 24
                    Point p = new(9);
                    Result += p.X;                           // 33
                    Point? maybe = new(2);
                    Result += maybe.Value.X;                 // 35
                    Dictionary<string, int> map = new();
                    map["k"] = 5;
                    Result += map["k"];                      // 40
                    Log += new Counter(3).Count;             // 3
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "3");
    assert_eq!(int_of(&emulator, "Result"), 40);
}

#[test]
fn coalescing_assignment_only_runs_when_it_has_to() {
    let source = r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Box { public int Count = 1; }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static Box Made()
                {
                    Log += "made";
                    return new Box();
                }
                public static void Main()
                {
                    Box a = null;
                    a ??= Made();                    // runs: Log = "made"
                    Result += a.Count;               // 1
                    a ??= Made();                    // does not run again
                    Result += a.Count;               // 2

                    string name = null;
                    name ??= "anon";
                    name ??= "other";
                    Log += name;                     // madeanon

                    int? count = null;
                    count ??= 5;
                    Result += count.Value;           // 7
                    count ??= 9;
                    Result += count.Value;           // 12

                    Box[] boxes = new Box[1];
                    boxes[0] ??= new Box();
                    Result += boxes[0].Count;        // 13

                    var map = new Dictionary<string, Box>();
                    map["k"] = null;
                    map["k"] ??= new Box();
                    Result += map["k"].Count;        // 14
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "madeanon");
    assert_eq!(int_of(&emulator, "Result"), 14);
}

#[test]
fn indexes_from_the_end_and_ranges_slice() {
    let source = r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int[] Numbers = { 1, 2, 3, 4, 5 };
                public static string Word = "hello";
                public static int Last(int[] values) { return values[^1]; }
                public static void Main()
                {
                    Result += Numbers[^1];               // 5
                    Result += Numbers[^5];               // 6
                    Result += Last(Numbers);             // 11
                    int two = 2;
                    Result += Numbers[^two];             // 15
                    Numbers[^1] = 50;
                    Result += Numbers[4];                // 65

                    int[] middle = Numbers[1..4];        // 2 3 4
                    Result += middle.Length;             // 68
                    Result += middle[0] + middle[2];     // 74
                    int[] tail = Numbers[3..];           // 4 50
                    Result += tail[1];                   // 124
                    int[] head = Numbers[..2];           // 1 2
                    Result += head.Length;               // 126
                    int[] all = Numbers[..];
                    Result += all.Length;                // 131
                    int[] none = Numbers[2..2];
                    Result += none.Length;               // 131
                    int[] fromEnd = Numbers[^2..^1];     // 4
                    Result += fromEnd[0];                // 135

                    Log += Word[^1];                     // o
                    Log += Word[1..3];                   // el
                    Log += Word[^2..];                   // lo
                    Log += Word[..1];                    // h

                    // the copy is its own array
                    middle[0] = 99;
                    Result += Numbers[1];                // 137

                    try
                    {
                        int[] bad = Numbers[3..1];
                        Result += bad.Length;
                    }
                    catch (ArgumentOutOfRangeException)
                    {
                        Log += "!";
                    }
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "oelloh!");
    assert_eq!(int_of(&emulator, "Result"), 137);
}

#[test]
fn jagged_arrays_are_arrays_of_arrays() {
    let source = r#"
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static int[][] Table = { new int[] { 1, 2 }, new int[] { 3 } };
                public static int Wide(int[][] rows)
                {
                    int widest = 0;
                    foreach (int[] row in rows)
                    {
                        if (row != null && row.Length > widest) widest = row.Length;
                    }
                    return widest;
                }
                public static void Main()
                {
                    int[][] grid = new int[3][];         // rows start null
                    if (grid[0] == null) Result += 1;    // 1
                    grid[0] = new int[] { 10, 20 };
                    grid[1] = new int[2];
                    grid[1][0] = 30;
                    grid[1][1] = grid[0][1];             // 20
                    Result += grid[0][0] + grid[1][0] + grid[1][1];   // 61
                    Result += grid.Length + grid[0].Length;           // 66

                    int[][] written = new int[][] { new int[] { 1 }, new int[] { 2, 3 } };
                    Result += written[1][1];             // 69
                    Result += Wide(written);             // 71
                    Result += Table[0][1] + Table[1][0]; // 76

                    foreach (int[] row in written)
                    {
                        foreach (int value in row) Log += value;
                    }                                    // 123

                    string[][] words = { new string[] { "a", "b" } };
                    Log += words[0][1];                  // 123b

                    int[][][] deep = new int[1][][];
                    deep[0] = new int[1][];
                    deep[0][0] = new int[] { 7 };
                    Result += deep[0][0][0];             // 83
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "123b");
    assert_eq!(int_of(&emulator, "Result"), 83);
}

#[test]
fn tuples_carry_values_and_come_apart() {
    let source = r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static (int, string) Pair() { return (1, "a"); }
                public static (int x, int y) Point(int x, int y) { return (x, y); }
                public static int Sum((int a, int b) values) { return values.a + values.b; }
                public static void Main()
                {
                    (int, string) pair = Pair();
                    Result += pair.Item1;                    // 1
                    Log += pair.Item2;                       // a
                    var named = Point(2, 3);
                    Result += named.x * named.y;             // 7
                    Result += named.Item1;                   // 9
                    Result += Sum((4, 5));                   // 18

                    // value semantics: a copy, not a share
                    var copy = named;
                    copy.x = 100;
                    Result += named.x;                       // 20

                    // deconstruction, in all three spellings
                    var (a, b) = Pair();
                    Result += a;                             // 21
                    Log += b;                                // aa
                    int c;
                    string d;
                    (c, d) = Pair();
                    Result += c;                             // 22
                    Log += d;                                // aaa
                    (int e, string f) = Pair();
                    Result += e;                             // 23
                    Log += f;                                // aaaa
                    var (_, only) = Pair();
                    Log += only;                             // aaaaa
                    (long wide, string _) = Pair();
                    if (wide == 1L) Result += 10;            // 33

                    // nested tuples
                    ((int, int), string) nested = ((1, 2), "n");
                    var ((p, q), r) = nested;
                    Result += p + q;                         // 36
                    Log += r;                                // aaaaan
                    Result += nested.Item1.Item2;            // 38

                    // equality is element by element
                    if (Point(1, 2) == (1, 2)) Result += 100;        // 138
                    if (Point(1, 2) != (1, 3)) Result += 1000;       // 1138

                    // patterns
                    var shape = Point(0, 5);
                    if (shape is (0, var height)) Result += height;  // 1143
                    switch (shape)
                    {
                        case (0, 0): Log += "origin"; break;
                        case (0, var h): Log += "up" + h; break;
                        default: Log += "?"; break;
                    }                                        // aaaaanup5
                    string kind = shape switch
                    {
                        (0, 0) => "origin",
                        (var x, _) when x > 0 => "right",
                        _ => "left",
                    };
                    Log += kind;                             // aaaaanup5left

                    // tuples in a list, walked apart
                    var list = new List<(int, string)>();
                    list.Add((7, "z"));
                    foreach (var (number, letter) in list)
                    {
                        Result += number;                    // 1150
                        Log += letter;                       // ...z
                    }
                    foreach (var whole in list) Result += whole.Item1;   // 1157
                    int[] plain = { 1, 2 };
                    foreach (var value in plain) Result += value;        // 1160
                    Result += list[0].Item1;                 // 1167
                    (int, int) blank = default;
                    Result += blank.Item1;                   // 1167
                    var (bx, by) = blank;
                    Result += bx + by;                       // 1167
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "aaaaanup5leftz");
    assert_eq!(int_of(&emulator, "Result"), 1167);
}

#[test]
fn a_tuple_prints_and_hashes_like_its_elements() {
    let source = r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static int Result;
                public static void Main()
                {
                    var t = (1, "a");
                    Log += $"{t}";                            // (1, a)
                    Log += ((2, (3, 4))).ToString();          // (2, (3, 4))

                    // a tuple as a dictionary key: hashed and compared by
                    // its elements, not by reference
                    var map = new Dictionary<(int, int), string>();
                    map[(1, 2)] = "here";
                    map[(3, 4)] = "there";
                    if (map.ContainsKey((1, 2))) Result += 1;
                    if (!map.ContainsKey((9, 9))) Result += 10;
                    Log += map[(3, 4)];                       // there
                    if ((1, 2).Equals((1, 2))) Result += 100;
                    if (!(1, 2).Equals((1, 3))) Result += 1000;
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "(1, a)(2, (3, 4))there");
    assert_eq!(int_of(&emulator, "Result"), 1111);
}

#[test]
fn deconstruct_takes_a_type_of_your_own_apart() {
    let source = r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Point
            {
                public int X;
                public int Y;
                public Point(int x, int y) { X = x; Y = y; }
                public void Deconstruct(out int x, out int y) { x = X; y = Y; }
            }
            public struct Size
            {
                public int Width;
                public int Height;
                public Size(int width, int height) { Width = width; Height = height; }
                public void Deconstruct(out int width, out int height)
                {
                    width = Width;
                    height = Height;
                }
            }
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static void Main()
                {
                    var point = new Point(3, 4);
                    var (x, y) = point;
                    Result += x * y;                          // 12
                    int px;
                    int py;
                    (px, py) = point;
                    Result += px + py;                        // 19

                    var (width, height) = new Size(2, 5);
                    Result += width * height;                 // 29

                    // positional patterns on a type of your own
                    if (point is (3, var found)) Result += found;      // 33
                    object shape = point;
                    switch (shape)
                    {
                        case Point(0, 0): Log += "origin"; break;
                        case Point(var a, var b): Log += "p" + (a + b); break;
                        default: Log += "?"; break;
                    }                                          // p7
                    string kind = new Size(1, 1) switch
                    {
                        (1, 1) => "unit",
                        _ => "other",
                    };
                    Log += kind;                               // p7unit

                    // and the payoff: walking a dictionary
                    var ages = new Dictionary<string, int>();
                    ages["ann"] = 30;
                    foreach (var (name, age) in ages)
                    {
                        Log += name;
                        Result += age;                         // 63
                    }
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "p7unitann");
    assert_eq!(int_of(&emulator, "Result"), 63);
}

#[test]
fn list_patterns_match_an_array_by_shape() {
    let source = r#"
        namespace Game
        {
            public class Program
            {
                public static int Result;
                public static string Log = "";
                public static string Describe(int[] values)
                {
                    return values switch
                    {
                        [] => "empty",
                        [var only] => "one:" + only,
                        [1, 2] => "onetwo",
                        [var first, .., var last] => "ends:" + first + last,
                        _ => "other",
                    };
                }
                public static void Main()
                {
                    Log += Describe(new int[] { });            // empty
                    Log += Describe(new int[] { 9 });          // one:9
                    Log += Describe(new int[] { 1, 2 });       // onetwo
                    Log += Describe(new int[] { 3, 4, 5 });    // ends:35
                    Log += Describe(new int[] { 7, 8 });       // ends:78

                    int[] numbers = { 1, 2, 3, 4 };
                    if (numbers is [1, ..]) Result += 1;                 // 1
                    if (numbers is [.., 4]) Result += 10;                // 11
                    if (numbers is [_, _, _, _]) Result += 100;          // 111
                    if (numbers is not [1, 2, 3]) Result += 1000;        // 1111
                    if (numbers is [var head, .. var rest])
                    {
                        Result += head;                                  // 1112
                        Result += rest.Length;                           // 1115
                        Result += rest[0];                               // 1117
                    }
                    if (numbers is [1, .. var middle, 4]) Result += middle.Length;  // 1119
                    string[] words = { "a", "b" };
                    if (words is ["a", var second]) Log += second;       // ...b
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "Log"),
        "emptyone:9onetwoends:35ends:78b"
    );
    assert_eq!(int_of(&emulator, "Result"), 1119);
}

#[test]
fn a_tuple_keeps_itself_inside_an_object() {
    let source = r#"
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static int Result;
                public static string Show(object value) { return value.ToString(); }
                public static bool Same(object left, object right) { return left.Equals(right); }
                public static void Main()
                {
                    object boxed = (1, "a");
                    Log += Show(boxed);                       // (1, a)
                    Log += Show((2, 3));                      // (2, 3)
                    if (Same((1, 2), (1, 2))) Result += 1;
                    if (!Same((1, 2), (1, 3))) Result += 10;
                    if (!Same((1, 2), (1, 2, 3))) Result += 100;
                    if (!Same((1, 2), "no")) Result += 1000;

                    object[] mixed = new object[] { (4, 5), "text" };
                    Log += Show(mixed[0]);                    // (4, 5)
                    Log += Show(mixed[1]);                    // text

                    // a boxed tuple hashes like its elements
                    object one = (7, 8);
                    object two = (7, 8);
                    if (one.GetHashCode() == two.GetHashCode()) Result += 10000;
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "(1, a)(2, 3)(4, 5)text");
    assert_eq!(int_of(&emulator, "Result"), 11111);
}

#[test]
fn casting_back_to_a_tuple_is_checked() {
    let source = r#"
        using System;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static void Main()
                {
                    object boxed = (1, 2);
                    (int, int) back = ((int, int))boxed;
                    Log += back.Item1 + "/" + back.Item2;          // 1/2
                    object wrong = "text";
                    try
                    {
                        (int, int) bad = ((int, int))wrong;
                        Log += "|" + bad.Item1;
                    }
                    catch (InvalidCastException)
                    {
                        Log += "|caught";
                    }
                    object other = (1, 2, 3);
                    try
                    {
                        (int, int) mismatched = ((int, int))other;
                        Log += "|" + mismatched.Item1;
                    }
                    catch (InvalidCastException)
                    {
                        Log += "|shape";
                    }
                }
            }
        }
    "#;
    let Some(emulator) = run(source, "Main") else {
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "1/2|caught|shape");
}

#[test]
fn an_async_method_suspends_and_resumes_where_it_left_off() {
    let source = r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static int Result;

                public static async Task<int> Work(int n)
                {
                    Log += "a";
                    await Scheduler.Delay(1f);
                    Log += "b";
                    int doubled = n * 2;
                    await Scheduler.NextFrame();
                    Log += "c";
                    return doubled + 1;
                }

                public static async void Main()
                {
                    Log += "s";
                    int value = await Work(20);
                    Result = value;
                    Log += "e";
                }
            }
        }
        "#;
    let Some(emulator) = run_stepping(source, "Main", &[]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    // suspended at the delay: nothing after it ran
    assert_eq!(string_of(&emulator, "Log"), "sa");
    assert_eq!(int_of(&emulator, "Result"), 0);
    assert!(emulator.has_pending_events());

    let emulator = run_stepping(source, "Main", &[(0.5, 30)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "sa");

    let emulator = run_stepping(source, "Main", &[(1.0, 60)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "sab");

    let emulator = run_stepping(source, "Main", &[(1.0, 60), (0.0, 1)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "sabce");
    assert_eq!(int_of(&emulator, "Result"), 41);
    assert!(!emulator.has_pending_events());
}

/// The code generator's error messages for `source`, with the program dump
/// alongside — for what is rejected there rather than by the checker.
fn codegen_errors(source: &str) -> Option<(String, Vec<String>)> {
    let dir = dotnet_shared_dir()?;
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(declarations.errors, vec![], "declaration errors");
    assert_eq!(signatures.errors, vec![], "signature errors");
    assert_eq!(bodies.errors, vec![], "type errors");
    let output = compiler.generate_udon(
        &declarations,
        &signatures,
        &bodies,
        &references,
        &["Game", "Program"],
    );
    let messages = output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    Some((output.program.dump(), messages))
}

/// The body-check errors of `source` compiled with the mini-corlib.
fn body_errors(source: &str) -> Option<Vec<men_sharp_semantics::SemanticError>> {
    let dir = dotnet_shared_dir()?;
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(declarations.errors, vec![], "declaration errors");
    assert_eq!(signatures.errors, vec![], "signature errors");
    Some(bodies.errors)
}

#[test]
fn exceptions_travel_through_tasks_and_completion_sources_complete_them() {
    let source = r#"
        using System;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static TaskCompletionSource<string> Source;

                static async Task<int> Fails(bool now)
                {
                    if (now) throw new InvalidOperationException("now");
                    await Scheduler.Delay(1f);
                    throw new InvalidOperationException("later");
                }

                static async Task Catcher()
                {
                    try { await Fails(true); } catch (InvalidOperationException e) { Log += "1:" + e.Message + ";"; }
                    try { await Fails(false); } catch (InvalidOperationException e) { Log += "2:" + e.Message + ";"; }
                    Task t = Fails(true);
                    Log += "faulted=" + t.IsFaulted + ";";
                    try { await Scheduler.NextFrame(); Log += "body;"; } finally { Log += "finally;"; }
                }

                static async Task Waiter()
                {
                    Source = new TaskCompletionSource<string>();
                    string word = await Source.Task;
                    Log += "got " + word + ";";
                }

                public static void Main()
                {
                    Catcher();
                    Waiter();
                }

                public static void Deliver()
                {
                    Source.SetResult("mail");
                    Log += "delivered;";
                }
            }
        }
        "#;
    let Some(emulator) = run_stepping(source, "Main", &[]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "1:now;");

    let emulator = run_stepping(source, "Main", &[(1.0, 60)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "1:now;2:later;faulted=True;");

    let mut emulator = run_stepping(source, "Main", &[(1.0, 60), (0.0, 1)]).unwrap();
    assert_eq!(
        string_of(&emulator, "Log"),
        "1:now;2:later;faulted=True;body;finally;"
    );
    // the completion source: its continuation runs once the delivering
    // event's body is done
    let sources = {
        let mut sources = vec![SourceCode::new("test.cs", source)];
        sources.extend(Compiler::corlib_sources());
        sources
    };
    let (_, assembled) = build(sources).unwrap();
    emulator.run(&assembled, "Deliver").unwrap();
    assert_eq!(
        string_of(&emulator, "Log"),
        "1:now;2:later;faulted=True;body;finally;delivered;got mail;"
    );
}

#[test]
fn when_all_when_any_and_the_timing_helpers_complete_in_order() {
    let source = r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                static async Task<string> Word(string w, float delay)
                {
                    await Scheduler.Delay(delay);
                    return w;
                }

                public static async void Main()
                {
                    Task<string> a = Word("x", 1f);
                    Task<string> b = Word("y", 2f);
                    Task first = await Task.WhenAny(a, b);
                    Log += (first == a) + ";";
                    await Task.WhenAll(a, b);
                    Log += a.Result + b.Result + ";";
                    await Task.Delay(500);
                    Log += "half;";
                    await Scheduler.Yield();
                    Log += "yield;";
                    await Task.CompletedTask.OnNextFrame();
                    Log += "next;";
                    int n = await Task.FromResult(7).After(1f);
                    Log += n + ";";
                }
            }
        }
        "#;
    let Some(emulator) = run_stepping(source, "Main", &[(1.0, 60)]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "True;");
    let emulator = run_stepping(source, "Main", &[(1.0, 60), (1.0, 60)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "True;xy;");
    let emulator = run_stepping(source, "Main", &[(1.0, 60), (1.0, 60), (0.5, 30)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "True;xy;half;yield;");
    let emulator =
        run_stepping(source, "Main", &[(1.0, 60), (1.0, 60), (0.5, 30), (0.0, 1)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "True;xy;half;yield;next;");
    let emulator = run_stepping(
        source,
        "Main",
        &[(1.0, 60), (1.0, 60), (0.5, 30), (0.0, 1), (1.0, 60)],
    )
    .unwrap();
    assert_eq!(string_of(&emulator, "Log"), "True;xy;half;yield;next;7;");
    assert!(!emulator.has_pending_events());
}

#[test]
fn async_lambdas_local_functions_and_concurrent_activations_keep_their_own_state() {
    let source = r#"
        using System;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                public static async void Main()
                {
                    Func<int, Task<int>> twice = async x => { await Scheduler.Delay(1f); return x * 2; };
                    async Task<string> Local(string s) { await Scheduler.NextFrame(); return s + "!"; }
                    Task<int> t1 = twice(1);
                    Task<int> t2 = twice(2);
                    Task<string> t3 = Local("hi");
                    int sum = await t1 + await t2;
                    Log += sum + ";";
                    await t3;
                    Log += await t3 + ";";
                    int counter = 0;
                    Action bump = () => counter++;
                    await Scheduler.Run(async () => { await Scheduler.NextFrame(); bump(); bump(); });
                    Log += counter + ";";
                    await Scheduler.WaitUntil(() => counter >= 2);
                    Log += "done";
                }
            }
        }
        "#;
    let Some(emulator) = run_stepping(source, "Main", &[(0.0, 1)]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "");
    let emulator = run_stepping(source, "Main", &[(0.0, 1), (1.0, 60)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "6;hi!;");
    let emulator = run_stepping(source, "Main", &[(0.0, 1), (1.0, 60), (0.0, 1)]).unwrap();
    assert_eq!(string_of(&emulator, "Log"), "6;hi!;2;done");
}

#[test]
fn a_behaviour_awaits_and_an_awaitable_of_the_users_own_works() {
    let source = r#"
        using System;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Signal
            {
                private bool raised;
                private Action waiting;
                public void Raise() { raised = true; if (waiting != null) { Action w = waiting; waiting = null; w(); } }
                public Signal GetAwaiter() { return this; }
                public bool IsCompleted { get { return raised; } }
                public void OnCompleted(Action continuation) { waiting = continuation; }
                public string GetResult() { return "raised"; }
            }

            public class Program : MenSharpBehaviour
            {
                public string Log = "";
                public int Opens;
                private Signal signal = new Signal();

                public async void Interact()
                {
                    Opens++;
                    Log += "open;";
                    await Scheduler.Delay(2f);
                    Log += "close;";
                    Log += await signal + ";";
                }

                public void Ring()
                {
                    signal.Raise();
                    Log += "rang;";
                }
            }
        }
        "#;
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let Some(program) = compile_behaviour(sources, "Game.Program") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert!(
        program.output.errors.is_empty(),
        "codegen errors: {:#?}",
        program.output.errors
    );
    let assembled = program.output.program.assemble().unwrap();
    let dump = program.output.program.dump();
    let mut emulator = Emulator::new(&program.output.program, &assembled);
    // `Interact` is a built-in Udon event: `_interact`
    emulator
        .run(&assembled, "_interact")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    emulator
        .run(&assembled, "_interact")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(string_of(&emulator, "Log"), "open;open;");
    assert_eq!(int_of(&emulator, "Opens"), 2);
    emulator
        .advance(&assembled, 2.0, 120)
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(string_of(&emulator, "Log"), "open;open;close;close;");
    // the user's awaitable resumes both waiters synchronously from Raise —
    // the last registered continuation wins in this simple Signal
    emulator
        .run(&assembled, "Ring")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(
        string_of(&emulator, "Log"),
        "open;open;close;close;raised;rang;"
    );
}

#[test]
fn an_exception_out_of_an_async_void_is_unhandled_and_a_run_task_fault_is_logged() {
    let source = r#"
        using System;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                static async Task Boom()
                {
                    await Scheduler.NextFrame();
                    throw new InvalidOperationException("boom");
                }

                public static async void Main()
                {
                    Scheduler.Run(Boom);
                    Log += "started;";
                    await Scheduler.DelayFrames(2);
                    Log += "resumed;";
                    throw new InvalidOperationException("void");
                }
            }
        }
        "#;
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let Some((program, assembled)) = build(sources) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let dump = program.dump();
    let mut emulator = Emulator::new(&program, &assembled);
    emulator
        .run(&assembled, "Main")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(string_of(&emulator, "Log"), "started;");
    // frame 1: the forgotten task faults, and says so
    emulator
        .advance(&assembled, 0.0, 1)
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert!(
        emulator
            .log
            .iter()
            .any(|line| line.contains("Scheduler.Run") && line.contains("boom")),
        "{:#?}",
        emulator.log
    );
    // frame 2: the async void throws — nothing catches that
    match emulator.advance(&assembled, 0.0, 1) {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(
                message.contains("Unhandled exception")
                    && message.contains("InvalidOperationException")
                    && message.contains("void"),
                "{message}"
            );
        }
        other => panic!("expected the async void's exception to halt, got {other:?}\n{dump}"),
    }
    assert_eq!(string_of(&emulator, "Log"), "started;resumed;");
}

#[test]
fn async_misuse_is_rejected_by_the_checker() {
    let Some(errors) = body_errors(
        r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static int Plain() { return 1; }
                public static async int Wrong() { await Scheduler.NextFrame(); return 1; }
                public static async Task ByRef(out int x) { x = 1; await Scheduler.NextFrame(); }
                public static void Main()
                {
                    int a = await Task.FromResult(1);
                    System.Action inner = () => { await Scheduler.NextFrame(); };
                }
                public static async Task NotAwaitable()
                {
                    await Plain();
                }
            }
        }
        "#,
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    use men_sharp_semantics::SemanticErrorKind;
    let kinds: Vec<String> = errors
        .iter()
        .map(|error| format!("{:?}", error.kind))
        .collect();
    assert!(
        errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::AsyncReturnType { .. })),
        "{kinds:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::AsyncByRefParameter)),
        "{kinds:#?}"
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error.kind, SemanticErrorKind::AwaitOutsideAsync))
            .count(),
        2,
        "{kinds:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::NotAwaitable { .. })),
        "{kinds:#?}"
    );
}

#[test]
fn async_recursion_and_instance_methods_keep_every_activation_apart() {
    let source = r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Counter
            {
                public int Ticks;
                private string name;
                public Counter(string name) { this.name = name; }
                public async Task<string> Tick(int times)
                {
                    for (int i = 0; i < times; i++)
                    {
                        await Scheduler.NextFrame();
                        Ticks++;
                    }
                    return name + Ticks;
                }
            }

            public class Program
            {
                public static string Log = "";

                static async Task<int> Depth(int n)
                {
                    if (n == 0) { return 0; }
                    await Scheduler.NextFrame();
                    int below = await Depth(n - 1);
                    Log += n + ";";
                    return below + 1;
                }

                public static async void Main()
                {
                    Counter a = new Counter("a");
                    Counter b = new Counter("b");
                    Task<string> ta = a.Tick(2);
                    Task<string> tb = b.Tick(3);
                    int depth = await Depth(3);
                    Log += "depth=" + depth + ";";
                    Log += await ta + ";" + await tb + ";";
                    Log += "ticks=" + (a.Ticks + b.Ticks);
                }
            }
        }
        "#;
    let Some(emulator) = run_stepping(source, "Main", &[(0.0, 1), (0.0, 1), (0.0, 1), (0.0, 1)])
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "1;2;3;depth=3;a2;b3;ticks=5");
    assert!(!emulator.has_pending_events());
}

#[test]
fn iterators_yield_lazily_and_enumerate_again_from_the_start() {
    let source = r#"
        using System;
        using System.Collections.Generic;
        namespace Game
        {
            public class Tree
            {
                public int Value;
                public Tree Left;
                public Tree Right;
                public Tree(int value, Tree left, Tree right) { Value = value; Left = left; Right = right; }

                public IEnumerable<int> InOrder()
                {
                    if (Left != null)
                    {
                        foreach (int v in Left.InOrder()) { yield return v; }
                    }
                    yield return Value;
                    if (Right != null)
                    {
                        foreach (int v in Right.InOrder()) { yield return v; }
                    }
                }
            }

            public class Program
            {
                public static string Log = "";

                static IEnumerable<int> Evens(int count)
                {
                    Log += "start;";
                    for (int i = 0; i < count; i++)
                    {
                        Log += "make" + i + ";";
                        yield return i * 2;
                    }
                    Log += "end;";
                }

                static IEnumerable<string> Words(string prefix)
                {
                    yield return prefix + "a";
                    if (prefix == "stop") { yield break; }
                    yield return prefix + "b";
                }

                static IEnumerable<T> Repeat<T>(T item, int times)
                {
                    for (int i = 0; i < times; i++) { yield return item; }
                }

                static IEnumerator<int> Counter()
                {
                    int n = 1;
                    while (true) { yield return n; n *= 3; }
                }

                static IEnumerable<int> Broken()
                {
                    yield return 1;
                    throw new InvalidOperationException("mid");
                }

                public static void Main()
                {
                    // lazy: nothing runs until MoveNext, and each step runs
                    // exactly up to the next yield
                    IEnumerable<int> evens = Evens(2);
                    Log += "made;";
                    foreach (int e in evens) { Log += "got" + e + ";"; }
                    // the same enumerable, enumerated again from the start
                    int sum = 0;
                    foreach (int e in evens) { sum += e; }
                    foreach (int e in evens) { sum += e; }
                    Log += "sum=" + sum + ";";

                    IEnumerable<int> local(int a) { yield return a; yield return a + 1; }
                    foreach (int v in local(10)) { Log += v + ","; }
                    foreach (string w in Words("x")) { Log += w + ","; }
                    foreach (string w in Words("stop")) { Log += w + ","; }
                    foreach (string s in Repeat("r", 2)) { Log += s; }
                    Log += ";";

                    IEnumerator<int> counter = Counter();
                    counter.MoveNext();
                    counter.MoveNext();
                    counter.MoveNext();
                    Log += "counter=" + counter.Current + ";";

                    Tree tree = new Tree(2, new Tree(1, null, null), new Tree(4, new Tree(3, null, null), null));
                    foreach (int v in tree.InOrder()) { Log += v; }
                    Log += ";";

                    IEnumerator<int> broken = Broken().GetEnumerator();
                    broken.MoveNext();
                    try { broken.MoveNext(); } catch (InvalidOperationException e) { Log += "caught " + e.Message + ";"; }
                    Log += "after=" + broken.MoveNext();
                }
            }
        }
        "#;
    let Some(emulator) = run(source, "Main") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(
        string_of(&emulator, "Log"),
        "made;start;make0;got0;make1;got2;end;start;make0;make1;end;start;make0;make1;end;sum=4;10,11,xa,xb,stopa,rr;counter=9;1234;caught mid;after=False"
    );
}

#[test]
fn cancellation_stops_a_waiting_method_at_its_await() {
    let source = r#"
        using System;
        using System.Threading;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static CancellationTokenSource Blinking;
                public static int Blinks;

                static async Task Blink(CancellationToken token)
                {
                    try
                    {
                        while (true)
                        {
                            Blinks++;
                            await Scheduler.Delay(1f, token);
                        }
                    }
                    catch (OperationCanceledException)
                    {
                        Log += "stopped at " + Blinks + ";";
                    }
                }

                static async Task Guarded(CancellationToken token)
                {
                    await Scheduler.Delay(5f, token);
                    token.ThrowIfCancellationRequested();
                    Log += "unreachable;";
                }

                public static async void Main()
                {
                    Blinking = new CancellationTokenSource();
                    Blink(Blinking.Token);
                    Task none = Blink(CancellationToken.None);

                    var timed = new CancellationTokenSource();
                    timed.CancelAfter(2000);
                    Task guarded = Guarded(timed.Token);
                    var already = new CancellationTokenSource();
                    already.Cancel();
                    Task early = Guarded(already.Token);
                    try { await early; } catch (TaskCanceledException) { Log += "early canceled=" + early.IsCanceled + ";"; }
                    try { await Scheduler.WaitUntil(() => false, timed.Token); } catch (OperationCanceledException) { Log += "wait stopped;"; }
                    Log += "guarded faulted=" + guarded.IsFaulted + ";";
                }

                public static void Stop()
                {
                    Blinking.Cancel();
                    Log += "cancel called;";
                }
            }
        }
        "#;
    let sources = {
        let mut sources = vec![SourceCode::new("test.cs", source)];
        sources.extend(Compiler::corlib_sources());
        sources
    };
    let Some((program, assembled)) = build(sources) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let dump = program.dump();
    let mut emulator = Emulator::new(&program, &assembled);
    emulator
        .run(&assembled, "Main")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    // the already-cancelled token faults its awaitable at once
    assert_eq!(string_of(&emulator, "Log"), "early canceled=True;");
    assert_eq!(int_of(&emulator, "Blinks"), 2);
    emulator
        .advance(&assembled, 1.0, 60)
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(int_of(&emulator, "Blinks"), 4);
    // cancelling resumes the waiting method with the exception, before
    // its delay is up — and only that one
    emulator
        .run(&assembled, "Stop")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(
        string_of(&emulator, "Log"),
        "early canceled=True;cancel called;stopped at 4;"
    );
    emulator
        .advance(&assembled, 1.0, 60)
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    // the token-less blink goes on; CancelAfter fired at 2s: the guarded
    // task and the WaitUntil both stopped
    assert_eq!(int_of(&emulator, "Blinks"), 5);
    assert_eq!(
        string_of(&emulator, "Log"),
        "early canceled=True;cancel called;stopped at 4;wait stopped;guarded faulted=True;"
    );
}

#[test]
fn iterator_misuse_is_rejected_by_the_checker() {
    let Some(errors) = body_errors(
        r#"
        using System;
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                static int NotEnumerable() { yield return 1; }
                static IEnumerable<int> Mixed() { yield return 1; return null; }
                static IEnumerable<int> Guarded()
                {
                    try { yield return 1; } catch (Exception) { }
                }
                static IEnumerable<int> InCatch()
                {
                    try { } catch (Exception) { yield return 1; }
                }
                static IEnumerable<int> InFinally()
                {
                    try { } finally { yield break; }
                }
                public static void Main()
                {
                    Func<int> f = () => { yield return 1; };
                }
            }
        }
        "#,
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    use men_sharp_semantics::SemanticErrorKind;
    let kinds: Vec<String> = errors
        .iter()
        .map(|error| format!("{:?}", error.kind))
        .collect();
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error.kind, SemanticErrorKind::YieldOutsideIterator))
            .count(),
        2,
        "{kinds:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::ReturnInIterator)),
        "{kinds:#?}"
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error.kind, SemanticErrorKind::YieldInsideTry { .. }))
            .count(),
        3,
        "{kinds:#?}"
    );
}

#[test]
fn interpolation_applies_its_format_and_alignment() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static void Main()
                {
                    float elapsed = 1.234567f;
                    int count = 42;
                    long big = 1234567L;
                    // format specifiers
                    Log += $"{elapsed:0.0}|{elapsed:F3}|{elapsed:0}|{count:D5}|{count:X}|{big:N0}|";
                    // ... and one that keeps its optional places away
                    Log += $"{elapsed:#.##}|{count}|";
                    // alignment, right and left, and both at once
                    Log += $"[{count,5}][{count,-5}][{elapsed,8:0.00}]";
                    string missing = null;
                    Log += $"[{missing,3}]";
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(
        string_of(&emulator, "Log"),
        "1.2|1.235|1|00042|2A|1,234,567|1.23|42|[   42][42   ][    1.23][   ]"
    );
}

#[test]
fn a_format_a_type_cannot_apply_is_an_error() {
    let Some((program, messages)) = codegen_errors(
        r#"
        namespace Game
        {
            public class Point { public int X; }
            public class Program
            {
                public static string Log = "";
                public static void Main()
                {
                    Point p = new Point();
                    Log = $"{p:F2}";
                    int width = 4;
                    Log += $"{p.X,width}";
                }
            }
        }
        "#,
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert!(
        messages
            .iter()
            .any(|message| message.contains("ToString(string)") && message.contains("Game.Point")),
        "{messages:#?}\n{program}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("alignment") && message.contains("constant")),
        "{messages:#?}\n{program}"
    );
}

#[test]
fn collections_and_iterators_pass_as_sequences() {
    let Some(emulator) = run(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                // one loop that takes anything enumerable
                static string Join<T>(IEnumerable<T> items)
                {
                    string text = "";
                    foreach (T item in items) { text += item + ","; }
                    return text;
                }

                static int Total(IEnumerable<int> numbers)
                {
                    int sum = 0;
                    foreach (int n in numbers) { sum += n; }
                    return sum;
                }

                static IEnumerable<int> Squares(int count)
                {
                    for (int i = 1; i <= count; i++) { yield return i * i; }
                }

                public static void Main()
                {
                    List<int> numbers = new List<int>();
                    numbers.Add(1);
                    numbers.Add(2);
                    numbers.Add(3);
                    Log += Join<int>(numbers);
                    Log += Total(numbers) + ";";

                    // the pattern-based foreach still binds to the concrete
                    // enumerator, interface or no interface
                    int direct = 0;
                    foreach (int n in numbers) { direct += n; }
                    Log += direct + ";";

                    List<string> words = new List<string>();
                    words.Add("a");
                    words.Add("b");
                    Log += Join<string>(words);

                    Dictionary<string, int> ages = new Dictionary<string, int>();
                    ages["ann"] = 30;
                    ages["bob"] = 40;
                    Log += Join<string>(ages.Keys);
                    Log += Total(ages.Values) + ";";
                    foreach (KeyValuePair<string, int> pair in ages) { Log += pair.Key + "=" + pair.Value + ";"; }

                    // an iterator is a sequence too, and it stays lazy
                    Log += Join<int>(Squares(4));
                    Log += Total(Squares(3)) + ";";

                    // ... and a sequence variable can hold any of them
                    IEnumerable<int> sequence = numbers;
                    Log += Total(sequence) + ";";
                    sequence = Squares(2);
                    Log += Total(sequence) + ";";

                    // an array is a sequence too, as in C#
                    int[] fixedNumbers = new int[] { 4, 5, 6 };
                    Log += Total(fixedNumbers) + ";";
                    Log += Join<string>(new string[] { "p", "q" });
                    sequence = fixedNumbers;
                    Log += Total(sequence) + ";";
                    foreach (int n in sequence) { Log += n; }
                    Log += ";";
                    // ... while `foreach` over the array itself still walks
                    // it by index, with nothing allocated
                    int byIndex = 0;
                    foreach (int n in fixedNumbers) { byIndex += n; }
                    Log += byIndex + ";";

                    // a string is a sequence of characters, the same way
                    IEnumerable<char> letters = "hey";
                    foreach (char c in letters) { Log += c; }
                    Log += ";" + Join<char>("ok");
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(
        string_of(&emulator, "Log"),
        "1,2,3,6;6;a,b,ann,bob,70;ann=30;bob=40;1,4,9,16,14;6;5;15;p,q,15;456;15;hey;o,k,"
    );
}

#[test]
fn yield_inside_try_finally_cleans_up_however_the_loop_is_left() {
    let Some(emulator) = run(
        r#"
        using System;
        using System.Collections.Generic;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                static IEnumerable<int> Guarded()
                {
                    Log += "open;";
                    try
                    {
                        yield return 1;
                        Log += "mid;";
                        yield return 2;
                    }
                    finally { Log += "close;"; }
                }

                static IEnumerable<int> Nested()
                {
                    try
                    {
                        try { yield return 1; }
                        finally { Log += "inner;"; }
                    }
                    finally { Log += "outer;"; }
                }

                static IEnumerable<int> Early()
                {
                    try { yield return 1; yield break; }
                    finally { Log += "efin;"; }
                }

                static string FirstOf()
                {
                    foreach (int n in Guarded()) { return "got" + n; }
                    return "none";
                }

                public static void Main()
                {
                    // run to the end: the finally runs where the body reaches it
                    foreach (int n in Guarded()) { Log += n + ";"; }
                    Log += "|";

                    // left early: the finally still runs, at the break
                    foreach (int n in Guarded()) { Log += n + ";"; break; }
                    Log += "|";

                    // ... and at a return out of the loop
                    Log += FirstOf() + ";|";

                    // ... and when an exception carries the loop away
                    try
                    {
                        foreach (int n in Guarded()) { throw new InvalidOperationException("boom"); }
                    }
                    catch (InvalidOperationException e) { Log += "caught " + e.Message + ";"; }
                    Log += "|";

                    // innermost finally first, as C# unwinds
                    foreach (int n in Nested()) { break; }
                    Log += "|";

                    // `yield break` inside the region runs it too
                    foreach (int n in Early()) { Log += "e" + n + ";"; }
                    Log += "|";

                    // never enumerated: the body never starts, so nothing to clean up
                    IEnumerable<int> unused = Guarded();
                    Log += "quiet";
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(
        string_of(&emulator, "Log"),
        // verified line for line against the same program under real .NET.
        // The third segment has no `open;close;` because `Log += FirstOf()`
        // reads `Log` before calling it, so what the call appended is
        // overwritten — C#'s compound-assignment order, not a lost finally.
        "open;1;mid;2;close;|open;1;close;|got1;|open;close;caught boom;|inner;outer;|e1;efin;|quiet"
    );
}

#[test]
fn a_compound_assignment_reads_its_target_before_the_right_side() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static int[] Cells = new int[2];
                public static int Cell0;
                public static int Reads;
                static int Index() { Log += "i"; return 0; }
                static int Next() { Log += "n"; return 1; }
                static string Side() { Log += "s"; return "r"; }
                static int Bump() { Cells[0] += 10; return 1; }
                public static void Main()
                {
                    // §12.21.4: the target is read, then the right side is
                    // evaluated, then they are combined — so what the right
                    // side writes to the target is overwritten, not added to
                    Log += Side();
                    Cells[0] = 0;
                    // ... and the index is evaluated once, before both
                    Cells[Index()] += Bump();
                    Cell0 = Cells[0];
                    // an index expression runs once for `++` as well
                    Cells[1] = 5;
                    Reads = Cells[Next()]++;
                }
            }
        }
        "#,
        "Main",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(string_of(&emulator, "Log"), "rin");
    assert_eq!(int_of(&emulator, "Cell0"), 1);
    assert_eq!(int_of(&emulator, "Reads"), 5);
    match emulator.value_of("Cells") {
        Some(Value::Array(cells)) => {
            assert!(matches!(cells.borrow()[1], Value::Int32(6)), "{:?}", cells)
        }
        other => panic!("Cells = {other:?}"),
    }
}

/// Compiles each named behaviour from `sources` and wires them into one
/// world, in the order given — a behaviour's index is its reference.
fn world_of(source: &str, classes: &[&str]) -> Option<men_sharp_asm::World> {
    let mut world = men_sharp_asm::World::new();
    for class in classes {
        let mut sources = vec![SourceCode::new("test.cs", source)];
        sources.extend(Compiler::corlib_sources());
        let program = compile_behaviour(sources, class)?;
        assert!(
            program.output.errors.is_empty(),
            "codegen errors for {class}: {:#?}",
            program.output.errors
        );
        let assembled = program.output.program.assemble().unwrap();
        let emulator = Emulator::new(&program.output.program, &assembled);
        world.add(class, assembled, emulator);
    }
    Some(world)
}

#[test]
fn two_behaviours_really_talk_to_each_other() {
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public class Lamp : MenSharpBehaviour
            {
                public int Level;
                public string Log = "";
                public void Brighten() { Level++; Log += "b"; }
                public int Add(int a, int b) { Log += "a"; return a + b; }
                public int Doubled { get { return Level * 2; } set { Level = value / 2; } }
            }

            public class Switch : MenSharpBehaviour
            {
                public Lamp lamp;
                public string Result = "";
                public void Interact()
                {
                    lamp.Brighten();
                    lamp.Brighten();
                    Result += lamp.Level + ";";
                    Result += lamp.Add(2, 3) + ";";
                    lamp.Doubled = 10;
                    Result += lamp.Level + ";" + lamp.Doubled + ";";
                    Result += lamp.Log;
                }
            }
        }
        "#;
    let Some(mut world) = world_of(source, &["Game.Lamp", "Game.Switch"]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let lamp = world.index_of("Game.Lamp").unwrap();
    let switch = world.index_of("Game.Switch").unwrap();
    world
        .program_mut(switch)
        .set_value("lamp", Value::Behaviour(lamp));

    world.raise(switch, "_interact").unwrap();

    match world.program(switch).value_of("Result") {
        Some(Value::Str(text)) => assert_eq!(&**text, "2;5;5;10;bba"),
        other => panic!("Result = {other:?}"),
    }
    match world.program(lamp).value_of("Level") {
        Some(Value::Int32(level)) => assert_eq!(*level, 5),
        other => panic!("Level = {other:?}"),
    }
}

#[test]
fn one_behaviour_awaits_another() {
    let source = r#"
        using System;
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public string Log = "";
                public async Task<int> Open(int by)
                {
                    Log += "start" + by + ";";
                    await Scheduler.Delay(1f);
                    Log += "opened;";
                    return by * 2;
                }

                public async Task Stick()
                {
                    await Scheduler.NextFrame();
                    throw new InvalidOperationException("stuck");
                }
            }

            public class Switch : MenSharpBehaviour
            {
                public Door door;
                public string Result = "";
                public string Fault = "";
                public async void Interact()
                {
                    Result += "call;";
                    int opened = await door.Open(21);
                    Result += "got" + opened + ";";
                    try { await door.Stick(); }
                    catch (RemoteTaskException e) { Result += "caught;"; Fault = e.Message; }
                    Result += "end";
                }
            }
        }
        "#;
    let Some(mut world) = world_of(source, &["Game.Door", "Game.Switch"]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let door = world.index_of("Game.Door").unwrap();
    let switch = world.index_of("Game.Switch").unwrap();
    world
        .program_mut(switch)
        .set_value("door", Value::Behaviour(door));

    let result = |world: &men_sharp_asm::World| match world.program(switch).value_of("Result") {
        Some(Value::Str(text)) => text.to_string(),
        other => panic!("Result = {other:?}"),
    };
    let door_log = |world: &men_sharp_asm::World| match world.program(door).value_of("Log") {
        Some(Value::Str(text)) => text.to_string(),
        other => panic!("Log = {other:?}"),
    };

    // the call crosses and the door starts working; the switch is suspended
    world.raise(switch, "_interact").unwrap();
    assert_eq!(result(&world), "call;");
    assert_eq!(door_log(&world), "start21;");

    // the door's delay comes up: it finishes, and hands the switch's own
    // continuation back to it rather than running it itself
    world.advance(1.0, 60).unwrap();
    assert_eq!(door_log(&world), "start21;opened;");
    // the continuation comes home on the next frame, never inside the
    // door's own event
    assert_eq!(result(&world), "call;");
    world.advance(0.0, 1).unwrap();
    assert_eq!(result(&world), "call;got42;");

    // ... and a task that faulted over there is caught over here
    world.advance(0.0, 1).unwrap();
    world.advance(0.0, 1).unwrap();
    assert_eq!(result(&world), "call;got42;caught;end");
    // the exception itself cannot cross, so what arrives is its text
    match world.program(switch).value_of("Fault") {
        Some(Value::Str(text)) => assert!(
            text.contains("InvalidOperationException") && text.contains("stuck"),
            "{text}"
        ),
        other => panic!("Fault = {other:?}"),
    }
    assert!(!world.has_pending_events());
}

#[test]
fn remote_tasks_are_ordinary_tasks_everywhere_else() {
    let source = r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Worker : MenSharpBehaviour
            {
                public int Jobs;
                public Task<int> Slow(int n)
                {
                    Jobs++;
                    return Delayed(n);
                }
                async Task<int> Delayed(int n)
                {
                    await Scheduler.Delay(1f);
                    return n;
                }
                public Task<int> Ready(int n) { return Task.FromResult(n); }
            }

            public class Boss : MenSharpBehaviour
            {
                public Worker worker;
                public string Result = "";
                public async void Interact()
                {
                    // a remote task kept in a variable, awaited later
                    Task<int> first = worker.Slow(1);
                    Task<int> second = worker.Slow(2);
                    Result += "queued" + worker.Jobs + ";";

                    // one that is already finished takes the fast path and
                    // never leaves this program
                    Result += "ready" + await worker.Ready(9) + ";";

                    // ... and the ordinary combinators work over both
                    await Task.WhenAll(first, second, Scheduler.Delay(0.5f));
                    Result += "all" + (first.Result + second.Result) + ";";
                    Result += "end";
                }
            }
        }
        "#;
    let Some(mut world) = world_of(source, &["Game.Worker", "Game.Boss"]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let worker = world.index_of("Game.Worker").unwrap();
    let boss = world.index_of("Game.Boss").unwrap();
    world
        .program_mut(boss)
        .set_value("worker", Value::Behaviour(worker));

    let result = |world: &men_sharp_asm::World| match world.program(boss).value_of("Result") {
        Some(Value::Str(text)) => text.to_string(),
        other => panic!("Result = {other:?}"),
    };

    world.raise(boss, "_interact").unwrap();
    assert_eq!(result(&world), "queued2;ready9;");

    world.advance(1.0, 60).unwrap();
    world.advance(0.0, 1).unwrap();
    assert_eq!(result(&world), "queued2;ready9;all3;end");
    assert!(!world.has_pending_events());
}

#[test]
fn a_plain_event_can_start_several_async_helpers() {
    let source = r#"
        using System.Threading.Tasks;
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public string Log = "";
                public async Task<int> Open(int by)
                {
                    Log += "start;";
                    await Scheduler.Delay(1f);
                    Log += "opened;";
                    return by * 2;
                }
            }

            public class Switch : MenSharpBehaviour
            {
                public Door door;
                public string Trace = "";
                private Task<int> opening;

                // the entry itself is NOT async: it starts helpers that are
                public void Interact()
                {
                    Trace += "click;";
                    opening = door.Open(21);
                    Watch();
                    Report();
                }

                private async void Watch()
                {
                    Trace += "watch;";
                    int opened = await opening;
                    Trace += "opened" + opened + ";";
                }

                private async void Report()
                {
                    await Scheduler.Delay(4f);
                    Trace += "report;";
                }
            }
        }
        "#;
    let Some(mut world) = world_of(source, &["Game.Door", "Game.Switch"]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let door = world.index_of("Game.Door").unwrap();
    let switch = world.index_of("Game.Switch").unwrap();
    world
        .program_mut(switch)
        .set_value("door", Value::Behaviour(door));

    let trace = |world: &men_sharp_asm::World| match world.program(switch).value_of("Trace") {
        Some(Value::Str(text)) => text.to_string(),
        other => panic!("Trace = {other:?}"),
    };

    world.raise(switch, "_interact").unwrap();
    assert_eq!(trace(&world), "click;watch;");

    // the remote task finishes and hands the continuation home
    world.advance(1.0, 60).unwrap();
    world.advance(0.0, 1).unwrap();
    assert_eq!(trace(&world), "click;watch;opened42;");

    // ... and the other helper's own delay still comes up
    world.advance(3.0, 180).unwrap();
    assert_eq!(trace(&world), "click;watch;opened42;report;");
}

#[test]
fn a_wake_up_that_arrives_early_still_leaves_a_way_back() {
    // Each delay asks the runtime for one wake-up, and the due time it is
    // measured against is read from a different clock than the one the
    // runtime counts down. A wake-up that lands a hair early finds nothing
    // due; if that were the end of it, the continuation would never run.
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static void Main() { Wait(); }
                static async void Wait()
                {
                    await Scheduler.Delay(1f);
                    Log += "woke";
                }
            }
        }
        "#;
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let Some((program, assembled)) = build(sources) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let dump = program.dump();
    let mut emulator = Emulator::new(&program, &assembled);
    emulator
        .run(&assembled, "Main")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert!(
        emulator.has_pending_events(),
        "the delay asked for a wake-up"
    );

    // the runtime's wake-up arrives, but early: drop the one it owed us and
    // raise the event by hand with no time passed
    emulator.delayed.clear();
    emulator
        .run(&assembled, "_mensharpResume")
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(string_of(&emulator, "Log"), "", "nothing was due yet");
    assert!(
        emulator.has_pending_events(),
        "the sweep left the wait behind, so it must have asked for another \
         wake-up:\n{dump}"
    );

    // ... and that one gets it home
    emulator
        .advance(&assembled, 1.0, 60)
        .unwrap_or_else(|error| panic!("{error:?}\n{dump}"));
    assert_eq!(string_of(&emulator, "Log"), "woke");
}

#[test]
fn linq_filters_projects_and_aggregates() {
    let source = r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Item
            {
                public string Name;
                public int Price;
                public float Weight;
                public Item(string name, int price, float weight) { Name = name; Price = price; Weight = weight; }
            }

            public class Program
            {
                public static string Log = "";

                static string Show<T>(IEnumerable<T> items)
                {
                    string text = "";
                    foreach (T item in items) { text += item + ","; }
                    return text;
                }

                public static void Main()
                {
                    int[] numbers = new int[] { 5, 3, 8, 1, 9, 2 };
                    Log += Show(numbers.Where(n => n % 2 == 1).Select(n => n * 10)) + ";";
                    Log += numbers.Sum() + " " + numbers.Count() + " " + numbers.Count(n => n > 4) + " "
                        + numbers.Any(n => n > 8) + " " + numbers.All(n => n > 0) + " "
                        + numbers.Min() + " " + numbers.Max() + " " + (int)(numbers.Average() * 100) + ";";
                    Log += numbers.First() + " " + numbers.First(n => n > 5) + " " + numbers.Last() + " "
                        + numbers.LastOrDefault(n => n > 100) + " " + numbers.FirstOrDefault(n => n > 100) + " "
                        + numbers.ElementAt(2) + " " + numbers.Single(n => n == 8) + ";";
                    Log += Show(numbers.Skip(2).Take(3)) + Show(numbers.TakeWhile(n => n > 2)) + Show(numbers.SkipWhile(n => n > 2)) + ";";
                    Log += Show(numbers.Concat(new int[] { 7 }).Reverse()) + Show(new int[] { 1, 2, 2, 3, 1 }.Distinct()) + numbers.Contains(8) + numbers.Contains(4) + ";";
                    Log += Show(Enumerable.Range(1, 4)) + Show(Enumerable.Repeat("x", 2)) + Enumerable.Empty<int>().Count() + ";";
                    Log += numbers.Aggregate((a, b) => a + b) + " " + numbers.Aggregate(100, (a, b) => a - b) + " "
                        + numbers.Aggregate("", (s, n) => s + n, s => s.Length) + ";";
                    Log += Show(numbers.Prepend(0).Take(2)) + Show(numbers.Append(0).Skip(5)) + Show(Enumerable.Empty<int>().DefaultIfEmpty()) + ";";
                    Log += Show(numbers.Select((n, i) => n * i)) + Show(numbers.Where((n, i) => i % 2 == 0)) + ";";

                    List<Item> items = new List<Item>();
                    items.Add(new Item("apple", 30, 1.5f));
                    items.Add(new Item("pear", 20, 0.5f));
                    items.Add(new Item("fig", 30, 0.25f));
                    Log += items.Sum(x => x.Price) + " " + items.Sum(x => x.Weight) + " " + items.Max(x => x.Price) + " "
                        + items.Min(x => x.Weight) + " " + (int)(items.Average(x => x.Price) * 10) + ";";
                    Log += items.Select(x => x.Name).Max() + " " + items.Select(x => x.Name).Min() + " "
                        + Show(items.Where(x => x.Price == 30).Select(x => x.Name)) + ";";
                    Log += items.ToList().Count + " " + items.ToDictionary(x => x.Name)["fig"].Price + " "
                        + items.ToDictionary(x => x.Name, x => x.Weight)["pear"] + ";";
                    Log += numbers.Take(2).SequenceEqual(new int[] { 5, 3 }) + " " + numbers.SequenceEqual(numbers.Reverse()) + " "
                        + Show(numbers.Zip(new string[] { "a", "b" }, (n, s) => s + n)) + ";";
                    Log += Show(new int[] { 1, 2, 3 }.Union(new int[] { 3, 4 })) + Show(new int[] { 1, 2, 3 }.Intersect(new int[] { 2, 3, 5 }))
                        + Show(new int[] { 1, 2, 3 }.Except(new int[] { 2 })) + ";";
                    Log += Show(new string[] { "ab", "cd" }.SelectMany(s => s.ToCharArray())) + "hello".Count(c => c == 'l') + Show("abc".Reverse()) + ";";
                    foreach (IGrouping<int, Item> group in items.GroupBy(x => x.Price))
                    {
                        Log += group.Key + ":" + group.Count() + ":" + Show(group.Select(x => x.Name)) + "|";
                    }
                    Log += ";";
                    try
                    {
                        numbers.Where(n => n > 100).First();
                    }
                    catch (InvalidOperationException error)
                    {
                        Log += error.Message + ";";
                    }
                }
            }
        }
    "#;
    let emulator = run(source, "Main").unwrap();
    let expected = [
        "50,30,10,90,",
        "28 6 3 True True 1 9 466",
        "5 8 2 0 0 8 8",
        "8,1,9,5,3,8,1,9,2,",
        "7,2,9,1,8,3,5,1,2,3,TrueFalse",
        "1,2,3,4,x,x,0",
        "28 72 6",
        "0,5,2,0,0,",
        "0,3,16,3,36,10,5,8,9,",
        "80 2.25 30 0.25 266",
        "pear apple apple,fig,",
        "3 30 0.5",
        "True False a5,b3,",
        "1,2,3,4,2,3,1,3,",
        "a,b,c,d,2c,b,a,",
        "30:2:apple,fig,|20:1:pear,|",
        "Sequence contains no elements",
    ]
    .join(";")
        + ";";
    assert_eq!(string_of(&emulator, "Log"), expected);
}

#[test]
fn linq_orders_stably_by_keys_and_by_the_type_itself() {
    let source = r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Item
            {
                public string Name;
                public int Price;
                public Item(string name, int price) { Name = name; Price = price; }
            }

            public class Version : IComparable<Version>
            {
                public int Major;
                public int Minor;
                public Version(int major, int minor) { Major = major; Minor = minor; }
                public int CompareTo(Version other)
                {
                    if (Major != other.Major) { return Major.CompareTo(other.Major); }
                    return Minor.CompareTo(other.Minor);
                }
                public override string ToString() { return Major + "." + Minor; }
            }

            public enum Rank { Low, Mid, High }

            public class Program
            {
                public static string Log = "";

                static string Show<T>(IEnumerable<T> items)
                {
                    string text = "";
                    foreach (T item in items) { text += item + ","; }
                    return text;
                }

                public static void Main()
                {
                    List<Item> items = new List<Item>();
                    items.Add(new Item("apple", 30));
                    items.Add(new Item("pear", 20));
                    items.Add(new Item("fig", 30));
                    items.Add(new Item("kiwi", 10));
                    // equal keys keep their order: apple stays before fig
                    Log += Show(items.OrderBy(x => x.Price).Select(x => x.Name)) + ";";
                    Log += Show(items.OrderBy(x => x.Price).ThenByDescending(x => x.Name).Select(x => x.Name)) + ";";
                    Log += Show(items.OrderByDescending(x => x.Price).ThenBy(x => x.Name).Select(x => x.Name)) + ";";
                    string[] words = new string[] { "pear", "apple", "fig", "Banana" };
                    Log += Show(words.OrderBy(s => s)) + Show(words.OrderByDescending(s => s.Length).ThenBy(s => s)) + ";";
                    Log += Show(new float[] { 2.5f, -1f, 0.5f }.OrderBy(f => f)) + Show(new char[] { 'c', 'a', 'b' }.OrderByDescending(c => c)) + ";";

                    Version[] versions = new Version[] { new Version(2, 1), new Version(1, 9), new Version(2, 0) };
                    Log += versions.Max() + " " + versions.Min() + " " + Show(versions.OrderBy(v => v)) + ";";
                    List<Version> list = new List<Version>(versions);
                    list.Sort();
                    Log += Show(list) + list.Contains(versions[1]) + list.IndexOf(versions[2]) + ";";
                    // nulls order first and are skipped by Min/Max; an empty
                    // sequence of a class gives null instead of throwing
                    Version[] withNull = new Version[] { new Version(3, 0), null, new Version(1, 0) };
                    Log += withNull.Min() + " " + Show(withNull.OrderBy(v => v).Select(v => v == null ? "null" : v.ToString()))
                        + (new Version[0].Max() == null) + ";";
                    Rank[] ranks = new Rank[] { Rank.High, Rank.Low, Rank.Mid };
                    Log += Show(ranks.OrderBy(r => r)) + ranks.Max() + ";";
                    List<int> plain = new List<int>();
                    plain.Add(3); plain.Add(1); plain.Add(2);
                    plain.Sort();
                    Log += Show(plain) + Show(new string[] { "b", "a" }.OrderBy(s => s).ToList()) + ";";
                }
            }
        }
    "#;
    let emulator = run(source, "Main").unwrap();
    let expected = [
        "kiwi,pear,apple,fig,",
        "kiwi,pear,fig,apple,",
        "apple,fig,pear,kiwi,",
        "apple,Banana,fig,pear,Banana,apple,pear,fig,",
        "-1,0.5,2.5,c,b,a,",
        "2.1 1.9 1.9,2.0,2.1,",
        "1.9,2.0,2.1,True1",
        "1.0 null,1.0,3.0,True",
        "Low,Mid,High,High",
        "1,2,3,a,b,",
    ]
    .join(";")
        + ";";
    assert_eq!(string_of(&emulator, "Log"), expected);
}

#[test]
fn linq_is_deferred_and_closes_the_source_early() {
    let source = r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        namespace Game
        {
            public class Program
            {
                public static string Log = "";

                static IEnumerable<int> Source()
                {
                    try
                    {
                        for (int i = 1; i <= 5; i++)
                        {
                            Log += "s" + i + ";";
                            yield return i;
                        }
                    }
                    finally
                    {
                        Log += "closed;";
                    }
                }

                public static void Main()
                {
                    IEnumerable<int> query = Source()
                        .Where(n => { Log += "w" + n + ";"; return n % 2 == 0; })
                        .Select(n => n * 10);
                    Log += "built;";
                    foreach (int value in query.Take(1)) { Log += "got" + value + ";"; }
                    // a second enumeration runs the chain again from the start
                    // (read into locals: `Log += query.First()` would read
                    // Log before the call and drop what the call logged)
                    int first = query.First();
                    Log += first + ";";
                    bool any = Source().Any();
                    Log += any + ";";
                }
            }
        }
    "#;
    let emulator = run(source, "Main").unwrap();
    assert_eq!(
        string_of(&emulator, "Log"),
        "built;s1;w1;s2;w2;got20;closed;s1;w1;s2;w2;closed;20;s1;closed;True;"
    );
}

#[test]
fn ordering_a_type_without_an_ordering_is_a_compile_error() {
    let source = r#"
        using System.Linq;
        namespace Game
        {
            public class Thing { public int Score; }
            public class Program
            {
                public static Thing Best;
                public static void Main()
                {
                    Thing[] things = new Thing[] { new Thing() };
                    Best = things.Max();
                }
            }
        }
    "#;
    let Some((_, messages)) = codegen_errors(source) else {
        return;
    };
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("`Game.Thing` has no ordering")
            && messages[0].contains("IComparable<Game.Thing>")
            && messages[0].contains("Max"),
        "{}",
        messages[0]
    );
}

#[test]
fn casting_a_fraction_to_an_integer_truncates_as_in_csharp() {
    let source = r#"
        namespace Game
        {
            public class Program
            {
                public static string Log = "";
                public static void Main()
                {
                    float f = 3.7f;
                    double d = -3.7;
                    double half = 2.5;
                    long big = 7;
                    // C# drops the fraction; `Convert.ToInt32` would round
                    // 3.7 up and 2.5 to the even 2
                    Log += (int)f + " " + (int)d + " " + (int)half + " " + (long)f + " " + (int)(f * 10) + " ";
                    big++;
                    float step = 1.5f;
                    step++;
                    Log += big + " " + step;
                }
            }
        }
    "#;
    let emulator = run(source, "Main").unwrap();
    assert_eq!(string_of(&emulator, "Log"), "3 -3 2 3 37 8 2.5");
}

#[test]
fn a_nullable_annotation_on_an_unconstrained_type_parameter_is_the_parameter_itself() {
    let Some(emulator) = run(
        r#"
        #nullable enable
        namespace Game
        {
            public class Program
            {
                public static string log = "";
                // C# 9: `T?` here is `T` — `int` when T is int, not `int?`
                static T? FirstOrDefault<T>(T[] xs) => xs.Length > 0 ? xs[0] : default;
                // ... and only `where T : struct` makes it Nullable<T>
                static T? FirstOrNull<T>(T[] xs) where T : struct
                    => xs.Length > 0 ? xs[0] : null;
                static string? Maybe(bool b) => b ? "x" : null;

                public static void Main()
                {
                    int v = FirstOrDefault(new[] { 3 });
                    int none = FirstOrDefault(new int[0]);
                    string? s = FirstOrDefault(new[] { "a" });
                    string? missing = FirstOrDefault(new string[0]);
                    int? n = FirstOrNull(new int[0]);
                    int? some = FirstOrNull(new[] { 7 });
                    string t = Maybe(true)!;
                    log = v + "," + none + "," + s + "," + (missing == null) + "," + n.HasValue
                        + "," + some + "," + t.Length;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "log"), "3,0,a,True,False,7,1");
}

#[test]
fn rectangular_arrays_index_by_dimension_and_walk_in_row_major_order() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public class Program
            {
                public static string log = "";

                static int Sum(int[,] grid)
                {
                    int s = 0;
                    foreach (int v in grid) s += v;
                    return s;
                }

                public static void Main()
                {
                    int[,] a = new int[2, 3];
                    a[0, 0] = 1;
                    a[1, 2] = 6;
                    a[0, 1] += 4;
                    int[,] b = { { 1, 2, 3 }, { 4, 5, 6 } };
                    var c = new int[,] { { 7, 8 }, { 9, 10 }, { 11, 12 } };
                    string[,] names = new string[1, 2];
                    names[0, 1] = "x";
                    int[,,] cube = new int[2, 3, 4];
                    cube[1, 2, 3] = 5;
                    object boxed = b;
                    bool isGrid = boxed is int[,];
                    bool isString = boxed is string;
                    int[,] back = (int[,])boxed;
                    var copy = (int[,])b.Clone();
                    copy[0, 0] = 100;
                    long total = 0;
                    foreach (var v in cube) total += v;
                    int walked = 0;
                    foreach (var v in b) walked = walked * 10 + v;
                    log = a[0, 0] + "," + a[0, 1] + "," + a[1, 2] + "," + a.Length + ","
                        + a.GetLength(0) + "," + a.GetLength(1) + "," + a.Rank
                        + "|" + Sum(b) + "," + b.GetUpperBound(1) + "," + c[2, 1] + "," + c.Length
                        + "|" + names[0, 1] + "," + (names[0, 0] == null)
                        + "|" + cube.Length + "," + cube[1, 2, 3] + "," + total + "," + cube.Rank
                        + "|" + isGrid + "," + isString + "," + back[1, 1] + "," + copy[0, 0] + "," + b[0, 0]
                        + "|" + walked + "|" + b;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "log"),
        "1,4,6,6,2,3,2|21,2,12,6|x,True|24,5,5,3|True,False,5,100,1|123456|System.Int32[,]"
    );
}

#[test]
fn a_rectangular_index_past_its_own_dimension_throws() {
    let Some((program, result)) = run_sources_result(
        vec![SourceCode::new(
            "test.cs",
            r#"
            namespace Game
            {
                public class Program
                {
                    public static int after;
                    public static void Main()
                    {
                        int[,] a = new int[2, 3];
                        // flat index 3 exists, but column 3 does not
                        a[0, 3] = 1;
                        after = 1;
                    }
                }
            }
            "#,
        )],
        "Main",
    ) else {
        return;
    };
    match result {
        Err(men_sharp_asm::EmulatorError::Exception(message)) => {
            assert!(message.contains("IndexOutOfRangeException"), "{message}");
        }
        Err(other) => panic!("expected a halt, got {other:?}\n{program}"),
        Ok(emulator) => panic!(
            "no halt: after = {:?}\n{program}",
            emulator.value_of("after")
        ),
    }
}

#[test]
fn records_have_value_equality_to_string_with_and_deconstruct() {
    let Some(emulator) = run(
        r#"
        using System.Collections.Generic;
        namespace Game
        {
            public record Point(int X, int Y);
            public record Named(string Name, Point At)
            {
                public int Extra = 3;
                public int Twice => X2 * 2;
                public int X2 { get; set; } = 5;
                private int hidden = 9;
                public static int S = 1;
            }
            public record Empty();
            public record Plain { public int A { get; init; } }
            public abstract record Shape(string Tag);
            public record Circle(string Tag, float R) : Shape(Tag);
            public record Some<T>(T Value);
            public record struct PS(int X, int Y);
            public readonly record struct RS(int X, double D);
            public record Nested(Point Inner, bool Flag, string Nothing);

            public class Program
            {
                public static string log = "";
                static void Log(object o) { log += o; log += "\n"; }

                public static void Main()
                {
                    var p = new Point(1, 2);
                    Log(p);
                    Log(new Named("bea", p));
                    Log(new Empty());
                    Log(new Plain { A = 4 });
                    Log(new Circle("c", 2.5f));
                    Log(new Some<int>(7));
                    Log(new Some<string>("s"));
                    Log(new PS(1, 2));
                    Log(new RS(1, 2.5));
                    Log(new Nested(p, true, null));
                    Log((p == new Point(1, 2)) + " " + p.Equals(new Point(1, 3)) + " "
                        + (p.GetHashCode() == new Point(1, 2).GetHashCode()));
                    Shape s1 = new Circle("c", 1f);
                    Shape s2 = new Circle("c", 1f);
                    Log(s1 == s2);
                    var q = p with { Y = 9 };
                    Log(q + " " + p);
                    var (x, y) = q;
                    Log(x + "," + y);
                    var ps = new PS(1, 2);
                    ps.X = 5;
                    var ps2 = ps with { Y = 7 };
                    Log(ps + " " + ps2 + " " + (ps == new PS(5, 2)));
                    var d = new Dictionary<Point, string>();
                    d[new Point(1, 2)] = "a";
                    Log(d[p]);
                    object o = p;
                    Log(o.Equals(new Point(1, 2)) + " " + o);
                    Log(new Named("bea", p) with { Extra = 1 } == new Named("bea", p));
                    Log(p != q);
                    Log(p == null);
                    Point n = null;
                    Log(n == null);
                    Log(q is Point(1, var second) ? "second " + second : "no");
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    let expected = "\
Point { X = 1, Y = 2 }
Named { Name = bea, At = Point { X = 1, Y = 2 }, Extra = 3, Twice = 10, X2 = 5 }
Empty { }
Plain { A = 4 }
Circle { Tag = c, R = 2.5 }
Some { Value = 7 }
Some { Value = s }
PS { X = 1, Y = 2 }
RS { X = 1, D = 2.5 }
Nested { Inner = Point { X = 1, Y = 2 }, Flag = True, Nothing =  }
True False True
True
Point { X = 1, Y = 9 } Point { X = 1, Y = 2 }
1,9
PS { X = 5, Y = 2 } PS { X = 5, Y = 7 } True
a
True Point { X = 1, Y = 2 }
False
True
False
True
second 9
";
    assert_eq!(string_of(&emulator, "log"), expected);
}

#[test]
fn a_union_switch_dispatches_on_the_runtime_type() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            [Union] public abstract class Shape { }
            public sealed class Circle : Shape
            {
                public int R;
                public Circle(int r) { R = r; }
            }
            public sealed class Rect : Shape
            {
                public int W, H;
                public Rect(int w, int h) { W = w; H = h; }
                public void Deconstruct(out int w, out int h) { w = W; h = H; }
            }
            [Union] public abstract class Option<T> { }
            public sealed class Some<T> : Option<T>
            {
                public T Value;
                public Some(T value) { Value = value; }
            }
            public sealed class None<T> : Option<T> { }

            public class Program
            {
                public static string log = "";

                static int Area(Shape s) => s switch
                {
                    Circle c => c.R * c.R * 3,
                    Rect(var w, var h) => w * h,
                };

                static string Show(Option<int> o) => o switch
                {
                    Some<int> some => "some " + some.Value,
                    None<int> none => "none",
                };

                public static void Main()
                {
                    Shape[] shapes = { new Circle(2), new Rect(3, 4) };
                    foreach (var s in shapes)
                    {
                        log += Area(s) + ";";
                        switch (s)
                        {
                            case Circle c:
                                log += "C";
                                break;
                            case Rect r:
                                log += "R";
                                break;
                        }
                    }
                    log += Show(new Some<int>(7)) + "," + Show(new None<int>());
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "log"), "12;C12;Rsome 7,none");
}

#[test]
fn a_source_enum_prints_its_member_name() {
    let source = r#"
        using System;
        namespace Game
        {
            public enum Rank { Low, Mid = 5, High, Peak = 5 }

            [Flags]
            public enum Doors { None = 0, Front = 1, Back = 2, Side = 4, Both = 3 }

            public class Program
            {
                public static string Log = "";
                static string Show<T>(T item) { return item + ";"; }
                public static void Main()
                {
                    Rank r = Rank.High;
                    Log += r + "," + r.ToString() + "," + Rank.Low + "," + Rank.Peak + "," + (Rank)9 + ";";
                    Log += $"{r} {Rank.Mid}" + ";";
                    Log += Show(r) + Show(Rank.Low) + Show(3);
                    Doors d = Doors.Front | Doors.Side;
                    Log += d + "," + Doors.None + "," + (Doors)3 + "," + (Doors)7 + "," + (Doors)8 + "," + Doors.Back + ";";
                    object boxed = r;
                    Log += boxed + ";";
                }
            }
        }
    "#;
    let emulator = run(source, "Main").unwrap();
    assert_eq!(
        string_of(&emulator, "Log"),
        "High,High,Low,Mid,9;High Mid;High;Low;3;Front, Side,None,Both,Both, Side,8,Back;6;"
    );
}

#[test]
fn compiling_the_same_program_twice_gives_the_same_text() {
    // Unity skips a program asset whose text did not change, so the text
    // must not depend on hash-map order: dispatchers, re-entry guards and
    // the rest are emitted in a sorted order
    let source = r#"
        using System;
        using System.Collections.Generic;
        using System.Linq;
        using MenSharp;
        namespace Game
        {
            public interface IShape { float Area(); }
            public class Square : IShape { public float Side; public float Area() { return Side * Side; } }
            public class Circle : IShape { public float Radius; public float Area() { return 3f * Radius * Radius; } }
            public class Program : MenSharpBehaviour
            {
                public Program other;
                public string Log = "";
                public void Ping() { Log += "p"; }
                public void Interact()
                {
                    IShape[] shapes = new IShape[] { new Square(), new Circle() };
                    Log = shapes.Select(s => s.Area()).Sum() + "";
                    Func<int, int> twice = x => x * 2;
                    Log += twice(3);
                    // a call into another program: what gets a re-entry guard
                    other.Ping();
                }
            }
        }
    "#;
    let texts: Vec<String> = (0..3)
        .map(|_| {
            let mut sources = vec![SourceCode::new("test.cs", source)];
            sources.extend(Compiler::corlib_sources());
            let (program, _) = build(sources).expect("a .NET SDK is installed");
            program.to_uasm().unwrap() + &program.to_meta_json().unwrap()
        })
        .collect();
    assert_eq!(texts[0], texts[1]);
    assert_eq!(texts[1], texts[2]);
}

// ------------------------------------------------------------ reflection

#[test]
fn typeof_comparisons_and_reflect_queries_settle_branches_at_compile_time() {
    let Some(emulator) = run(
        r#"
        using MenSharp.Reflection;
        namespace Game
        {
            public class Point { public int x; }
            public enum Colour { Red, Green }
            public class Program
            {
                public static string kinds;
                public static string logic;

                static string Kind<T>()
                {
                    if (typeof(T) == typeof(int)) return "int";
                    else if (typeof(T) == typeof(string)) return "string";
                    else if (Reflect.IsArray<T>()) return "array";
                    else if (Reflect.IsList<T>()) return "list";
                    else if (Reflect.IsNullable<T>()) return "nullable";
                    else if (Reflect.IsEnum<T>()) return "enum";
                    else if (Reflect.IsObject<T>()) return "object";
                    else return "other";
                }

                static string Same<T, U>()
                {
                    // the other arm names nothing this T can do: it must not be lowered
                    if (Reflect.Is<T, U>() && typeof(T) != typeof(float)) return "same";
                    if (typeof(T) != typeof(U) || Reflect.Is<T, U>()) return "different";
                    return "unreachable";
                }

                public static void Main()
                {
                    kinds = Kind<int>() + "," + Kind<string>() + "," + Kind<int[]>() + ","
                        + Kind<System.Collections.Generic.List<int>>() + "," + Kind<int?>() + ","
                        + Kind<Colour>() + "," + Kind<Point>() + "," + Kind<float>();
                    logic = Same<int, int>() + "," + Same<int, string>();
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "kinds"),
        "int,string,array,list,nullable,enum,object,other"
    );
    assert_eq!(string_of(&emulator, "logic"), "same,different");
}

#[test]
fn reflect_visit_fields_walks_every_slot_with_its_static_type() {
    let Some(emulator) = run(
        r#"
        using MenSharp.Reflection;
        namespace Game
        {
            public class JsonNameAttribute : System.Attribute
            {
                public string Name;
                public JsonNameAttribute(string name) { Name = name; }
            }
            public class Tagged : System.Attribute { }

            public class Being { public string name = "ann"; }
            public class Player : Being
            {
                [JsonName("hp")] public int health = 3;
                private float speed = 1.5f;
                [Tagged] public int[] scores = new int[] { 1, 2 };
                public int Level { get; set; } = 7;
                public float Speed => speed;
            }

            public class Doubler : IFieldVisitor
            {
                public string seen = "";
                public void Visit<F>(FieldInfo field, ref F value)
                {
                    seen += field.Name + (field.IsPublic ? "+" : "-");
                    var alias = field.Attribute<JsonNameAttribute>();
                    if (alias != null) seen += "(" + alias.Name + ")";
                    if (field.Has<Tagged>()) seen += "*";
                    seen += ":" + Program.Kind<F>() + ";";
                    if (typeof(F) == typeof(int))
                    {
                        int doubled = (int)(object)value * 2;
                        value = (F)(object)doubled;
                    }
                    else if (typeof(F) == typeof(string))
                    {
                        value = (F)(object)((string)(object)value + "!");
                    }
                }
            }

            public class Program
            {
                public static string seen;
                public static string name;
                public static int health;
                public static int level;
                public static int scores;

                public static string Kind<T>()
                {
                    if (typeof(T) == typeof(int)) return "int";
                    else if (typeof(T) == typeof(string)) return "string";
                    else if (typeof(T) == typeof(float)) return "float";
                    else if (Reflect.IsArray<T>()) return "array";
                    else return "other";
                }

                public static void Main()
                {
                    var player = new Player();
                    var doubler = new Doubler();
                    Reflect.VisitFields(ref player, doubler);
                    seen = doubler.seen;
                    name = player.name;
                    health = player.health;
                    level = player.Level;
                    scores = player.scores.Length;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "seen"),
        "name+:string;health+(hp):int;speed-:float;scores+*:array;Level+:int;"
    );
    assert_eq!(string_of(&emulator, "name"), "ann!");
    assert_eq!(int_of(&emulator, "health"), 6);
    assert_eq!(int_of(&emulator, "level"), 14);
    assert_eq!(int_of(&emulator, "scores"), 2);
}

#[test]
fn reflect_new_and_element_types_and_struct_targets() {
    let Some(emulator) = run(
        r#"
        using MenSharp.Reflection;
        namespace Game
        {
            public struct Vec { public int x; public int y; }
            public class Named { public string label = "n"; public Named() { label += "!"; } }
            public record Pair(int Left, string Right);

            public class Setter : IFieldVisitor
            {
                public void Visit<F>(FieldInfo field, ref F value)
                {
                    if (typeof(F) == typeof(int)) value = (F)(object)41;
                }
            }

            public class Names : IFieldVisitor
            {
                public string seen = "";
                public void Visit<F>(FieldInfo field, ref F value) { seen += field.Name + ","; }
            }

            public class ElementKind : ITypeVisitor
            {
                public string kind = "";
                public void Visit<E>() { kind += Program.Kind<E>() + ","; }
            }

            public class Program
            {
                public static int x;
                public static int y;
                public static string label;
                public static string elements;
                public static string pair;

                public static string Kind<T>()
                {
                    if (typeof(T) == typeof(int)) return "int";
                    else if (typeof(T) == typeof(string)) return "string";
                    else if (Reflect.IsObject<T>()) return "object";
                    else return "other";
                }

                public static void Main()
                {
                    var vec = Reflect.New<Vec>();
                    vec.y = 1;
                    Reflect.VisitFields(ref vec, new Setter());
                    x = vec.x;
                    y = vec.y;
                    label = Reflect.New<Named>().label;

                    var kinds = new ElementKind();
                    Reflect.VisitElementType<int[], ElementKind>(kinds);
                    Reflect.VisitElementType<System.Collections.Generic.List<string>, ElementKind>(kinds);
                    Reflect.VisitElementType<int?, ElementKind>(kinds);
                    Reflect.VisitElementType<Named[], ElementKind>(kinds);
                    elements = kinds.kind;

                    var names = new Names();
                    var record = new Pair(1, "r");
                    Reflect.VisitFields(ref record, names);
                    pair = names.seen;
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(int_of(&emulator, "x"), 41);
    assert_eq!(int_of(&emulator, "y"), 41);
    assert_eq!(string_of(&emulator, "label"), "n!");
    assert_eq!(string_of(&emulator, "elements"), "int,string,int,object,");
    assert_eq!(string_of(&emulator, "pair"), "Left,Right,");
}

#[test]
fn reflect_reports_what_it_cannot_answer_at_compile_time() {
    let Some((_, messages)) = codegen_errors(
        r#"
        using MenSharp.Reflection;
        namespace Game
        {
            public class Named { public Named(string name) { } }
            public class Nothing : IFieldVisitor
            {
                public void Visit<F>(FieldInfo field, ref F value) { }
            }
            public class Program
            {
                static void Describe<T>()
                {
                    if (typeof(T) == typeof(int)) { }
                    else { Reflect.Unsupported<T>("Describe"); }
                }
                public static void Main()
                {
                    Describe<int>();
                    Describe<string>();
                    string text = "x";
                    Reflect.VisitFields(ref text, new Nothing());
                    Reflect.New<Named>();
                    Reflect.New<int>();
                }
            }
        }
        "#,
    ) else {
        return;
    };
    let joined = messages.join("\n");
    assert!(
        joined.contains("`Describe` does not support `string`"),
        "{joined}"
    );
    assert!(!joined.contains("does not support `int`"), "{joined}");
    assert!(
        joined.contains(
            "`Reflect.VisitFields` walks the fields of a class, struct or record written in M#; `string` is not one"
        ),
        "{joined}"
    );
    assert!(
        joined.contains("`Reflect.New<Game.Named>` needs a parameterless constructor"),
        "{joined}"
    );
    assert!(
        joined.contains(
            "`Reflect.New` creates a class, struct or record written in M#; `int` is not one"
        ),
        "{joined}"
    );
}

#[test]
fn this_inside_a_behaviour_is_its_own_udon_behaviour() {
    // a behaviour has no object of its own, but `this` as a value is the
    // program's UdonBehaviour — what `(IUdonEventReceiver)this` hands the SDK.
    // A call with such an argument used to be dropped without a word.
    let Some(emulator) = run_behaviour(
        r#"
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public object self;
                public object taken;
                public void Interact()
                {
                    self = this;
                    Take((object)this);
                }
                private void Take(object door) { taken = door; }
            }
        }
        "#,
        "Game.Door",
        "_interact",
    ) else {
        return;
    };
    for name in ["self", "taken"] {
        assert!(
            matches!(emulator.value_of(name), Some(Value::SelfComponent(_))),
            "{name} = {:?}",
            emulator.value_of(name)
        );
    }
}

#[test]
fn a_type_nested_in_a_generic_type_sees_the_outer_parameters() {
    let Some(emulator) = run(
        r#"
        using MenSharp;
        namespace Game
        {
            [Union] public abstract class Result<T, E>
            {
                public sealed class Ok : Result<T, E>
                {
                    public readonly T Value;
                    public Ok(T value) { Value = value; }
                }
                public sealed class Err : Result<T, E>
                {
                    public readonly E Error;
                    public Err(E error) { Error = error; }
                }

                // bare `Ok` here is `Result<T, E>.Ok`
                public static Result<T, E> Success(T value) { return new Ok(value); }
                public static Result<T, E> Failure(E error) { return new Err(error); }

                public bool IsOk { get { return this is Ok; } }

                public R Match<R>(System.Func<T, R> ok, System.Func<E, R> err)
                {
                    switch (this)
                    {
                        case Ok o: return ok(o.Value);
                        case Err e: return err(e.Error);
                    }
                }
            }

            public class Program
            {
                public static string log = "";
                static Result<int, string> Parse(string text)
                {
                    int value;
                    if (int.TryParse(text, out value)) return Result<int, string>.Success(value);
                    return new Result<int, string>.Err("not a number: " + text);
                }
                public static void Main()
                {
                    foreach (string text in new[] { "42", "x" })
                    {
                        Result<int, string> r = Parse(text);
                        switch (r)
                        {
                            case Result<int, string>.Ok ok: log += "ok " + ok.Value + ";"; break;
                            case Result<int, string>.Err err: log += "err " + err.Error + ";"; break;
                        }
                        log += r.IsOk + ";" + r.Match(v => v * 2, e => -1) + ";";
                    }
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "log"),
        "ok 42;True;84;err not a number: x;False;-1;"
    );
}

#[test]
fn implicit_conversion_operators_written_in_source_apply_at_conversion_sites() {
    let Some(emulator) = run(
        r#"
        namespace Game
        {
            public struct Meters
            {
                public int Value;
                public Meters(int value) { Value = value; }
                public static implicit operator Meters(int value) { return new Meters(value); }
            }

            public sealed class OkValue<T> { public readonly T Value; public OkValue(T value) { Value = value; } }
            public sealed class ErrValue<E> { public readonly E Error; public ErrValue(E error) { Error = error; } }

            public abstract class Result<T, E>
            {
                public sealed class Ok : Result<T, E> { public readonly T Value; public Ok(T value) { Value = value; } }
                public sealed class Err : Result<T, E> { public readonly E Error; public Err(E error) { Error = error; } }
                public static implicit operator Result<T, E>(OkValue<T> ok) { return new Ok(ok.Value); }
                public static implicit operator Result<T, E>(ErrValue<E> err) { return new Err(err.Error); }
                public string Show()
                {
                    switch (this)
                    {
                        case Ok ok: return "ok " + ok.Value;
                        case Err err: return "err " + err.Error;
                        default: return "?";
                    }
                }
            }

            public static class Result
            {
                public static OkValue<T> Ok<T>(T value) { return new OkValue<T>(value); }
                public static ErrValue<E> Err<E>(E error) { return new ErrValue<E>(error); }
            }

            public class Program
            {
                public static string log = "";
                static Result<int, string> Parse(string text)
                {
                    int value;
                    if (int.TryParse(text, out value)) return Result.Ok(value);   // return site
                    return Result.Err("nan " + text);
                }
                static string Describe(Result<int, string> r) { return r.Show(); }
                static int Double(Meters m) { return m.Value * 2; }
                public static void Main()
                {
                    Meters m = 21;                                         // assignment site
                    Result<int, string> viaLocal = Result.Ok(7);
                    log = Parse("42").Show() + ";" + Parse("x").Show() + ";" + viaLocal.Show() + ";"
                        + Describe(Result.Err("arg")) + ";" + Double(5) + ";" + m.Value;   // argument site
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "log"),
        "ok 42;err nan x;ok 7;err arg;10;21"
    );
}

#[test]
fn using_static_brings_a_types_static_members_into_scope() {
    let Some(emulator) = run(
        r#"
        using static Game.Result;
        using static Game.Numbers;
        namespace Game
        {
            public sealed class OkValue<T> { public readonly T Value; public OkValue(T value) { Value = value; } }
            public sealed class ErrValue<E> { public readonly E Error; public ErrValue(E error) { Error = error; } }
            public abstract class Result<T, E>
            {
                public sealed class Ok : Result<T, E> { public readonly T Value; public Ok(T value) { Value = value; } }
                public sealed class Err : Result<T, E> { public readonly E Error; public Err(E error) { Error = error; } }
                public static implicit operator Result<T, E>(OkValue<T> ok) { return new Ok(ok.Value); }
                public static implicit operator Result<T, E>(ErrValue<E> err) { return new Err(err.Error); }
                public string Show()
                {
                    switch (this)
                    {
                        case Ok ok: return "ok " + ok.Value;
                        case Err err: return "err " + err.Error;
                        default: return "?";
                    }
                }
            }
            public static class Result
            {
                public static OkValue<T> Ok<T>(T value) { return new OkValue<T>(value); }
                public static ErrValue<E> Err<E>(E error) { return new ErrValue<E>(error); }
            }
            public static class Numbers
            {
                public const int Limit = 100;
                public static int Twice(int x) { return x * 2; }
                public static int Twice(string x) { return x.Length * 2; }
            }
            public class Program
            {
                public static string log = "";
                static Result<int, string> Parse(string text)
                {
                    int value;
                    if (!int.TryParse(text, out value)) return Err("nan " + text);
                    if (value > Limit) return Err("too big");
                    return Ok(Twice(value));
                }
                public static void Main()
                {
                    log = Parse("21").Show() + ";" + Parse("x").Show() + ";" + Parse("500").Show() + ";" + Twice("abc");
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(string_of(&emulator, "log"), "ok 42;err nan x;err too big;6");
}

#[test]
fn the_std_result_and_option_types_work_end_to_end() {
    let Some(emulator) = run(
        r#"
        using MenSharp;
        using static MenSharp.Result;
        using static MenSharp.Option;
        namespace Game
        {
            public class Program
            {
                public static string log = "";
                static Result<int, string> Parse(string text)
                {
                    int value;
                    if (!int.TryParse(text, out value)) return Err("nan " + text);
                    return Ok(value);
                }
                static Option<int> Positive(int value)
                {
                    if (value > 0) return Some(value);
                    return None;
                }
                public static void Main()
                {
                    Result<int, string> good = Parse("42");
                    Result<int, string> bad = Parse("x");
                    good.Switch(v => log += "ok " + v + ";", e => log += "err " + e + ";");
                    bad.Switch(v => log += "ok " + v + ";", e => log += "err " + e + ";");
                    log += good.Match(v => v * 2, e => -1) + ";" + bad.Match(v => v * 2, e => -1) + ";";
                    int value; string error;
                    if (good.TryGet(out value, out error)) log += "got " + value + ";";
                    if (!bad.TryGet(out value, out error)) log += "failed " + error + ";";
                    log += good.Map(v => v + 1) + ";" + bad.MapError(e => e.Length) + ";";
                    log += good.AndThen(v => Parse("7")).UnwrapOr(0) + ";" + bad.UnwrapOr(9) + ";";
                    log += good.IsOk + "," + bad.IsErr + ";";
                    switch (good)
                    {
                        case Result<int, string>.Ok(var v): log += "case ok " + v + ";"; break;
                        case Result<int, string>.Err(var e): log += "case err " + e + ";"; break;
                    }
                    log += Positive(3) + ";" + Positive(-3) + ";" + Positive(5).UnwrapOr(0) + ";"
                        + Positive(-1).OkOr("neg") + ";" + Positive(2).Map(v => v * 10);
                }
            }
        }
        "#,
        "Main",
    ) else {
        return;
    };
    assert_eq!(
        string_of(&emulator, "log"),
        "ok 42;err nan x;84;-1;got 42;failed nan x;Ok(43);Err(5);7;9;True,True;case ok 42;Some(3);None;5;Err(neg);Some(20)"
    );
}

// ------------------------------------------------------- evaluation order
// (issue #1: an operand read into a variable's own slot must keep the value
// read when a later operand writes the variable)

/// Runs `body` inside `Main` of a program with static `x`, `i`, `result` and
/// the helpers the evaluation-order tests share.
fn run_order(body: &str) -> Option<Emulator> {
    run(
        &format!(
            r#"
            using System;
            namespace Game
            {{
                public class Pair
                {{
                    public int a;
                    public int b;
                    public Pair(int a, int b) {{ this.a = a; this.b = b; }}
                }}
                public class Program
                {{
                    public static int x;
                    public static int i;
                    public static int result;
                    public static int[] array;
                    static int Mutate() {{ x = 9; return 1; }}
                    static int MoveIndex() {{ i = 9; return 7; }}
                    static int Pure() {{ return 1; }}
                    static int Add(int a, int b) {{ return a * 10 + b; }}
                    static int AddRef(ref int a, int b) {{ return a * 10 + b; }}
                    public static void Main()
                    {{
                        x = 3;
                        i = 3;
                        array = new int[10];
                        {body}
                    }}
                }}
            }}
            "#
        ),
        "Main",
    )
}

#[test]
fn a_binary_operand_keeps_its_value_across_a_later_call_that_writes_it() {
    let Some(emulator) = run_order("result = x + Mutate();") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 4);
    assert_eq!(int_of(&emulator, "x"), 9);
}

#[test]
fn a_binary_operand_keeps_its_value_across_a_later_assignment_to_it() {
    let Some(emulator) = run_order("result = x + (x = 9);") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 12);
}

#[test]
fn a_binary_operand_keeps_its_value_across_a_later_increment_of_it() {
    // `x + x++`: 3 + 3, and x ends at 4
    let Some(emulator) = run_order("result = x + x++;") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 6);
    assert_eq!(int_of(&emulator, "x"), 4);
}

#[test]
fn a_local_operand_keeps_its_value_across_a_later_assignment_to_it() {
    let Some(emulator) = run_order("int y = 3; result = y + (y = 9);") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 12);
}

#[test]
fn the_right_operand_of_a_binary_sees_the_write() {
    // `Mutate() + x` reads x after the call, as C# does
    let Some(emulator) = run_order("result = Mutate() + x;") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 10);
}

#[test]
fn a_pure_right_operand_leaves_the_left_where_it_is() {
    let Some(emulator) = run_order("result = x + Pure();") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 4);
}

#[test]
fn an_argument_keeps_its_value_across_a_later_argument_that_writes_it() {
    let Some(emulator) = run_order("result = Add(x, Mutate());") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 31);
}

#[test]
fn a_ref_argument_is_the_variable_and_sees_a_later_write() {
    let Some(emulator) = run_order("result = AddRef(ref x, Mutate());") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 91);
}

#[test]
fn a_constructor_argument_keeps_its_value_across_a_later_argument() {
    let Some(emulator) = run_order("var p = new Pair(x, Mutate()); result = p.a * 10 + p.b;")
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 31);
}

#[test]
fn a_tuple_element_keeps_its_value_across_a_later_element() {
    // `(x, x++)` is (3, 3), and x ends at 4
    let Some(emulator) = run_order("var t = (x, x++); result = t.Item1 * 10 + t.Item2;") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 33);
    assert_eq!(int_of(&emulator, "x"), 4);
}

#[test]
fn an_element_assignment_uses_the_index_read_before_the_value_runs() {
    // `array[i] = MoveIndex()` writes element 3, not element 9
    let Some(emulator) = run_order("array[i] = MoveIndex(); result = array[3] * 10 + array[9];")
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 70);
}

#[test]
fn a_compound_element_assignment_uses_the_index_read_before_the_value_runs() {
    let Some(emulator) =
        run_order("array[3] = 1; array[i] += MoveIndex(); result = array[3] * 10 + array[9];")
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 80);
}

#[test]
fn a_compound_assignment_adds_to_the_value_read_before_the_right_side() {
    let Some(emulator) = run_order("x += Mutate(); result = x;") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 4);
}

#[test]
fn a_switch_tests_the_value_read_once_across_a_when_clause_that_writes_it() {
    // the `when` fails after writing x = 9; the next label still sees 3
    let Some(emulator) = run_order(
        "switch (x) { case 3 when Mutate() > 5: result = 1; break; case 3: result = 2; break; default: result = 4; break; }",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 2);
}

#[test]
fn a_switch_expression_tests_the_value_read_once_across_a_when_clause() {
    let Some(emulator) =
        run_order("result = x switch { 3 when Mutate() > 5 => 1, 3 => 2, _ => 4 };")
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 2);
}

#[test]
fn a_binary_operand_keeps_its_value_across_a_delegate_call_that_writes_it() {
    let Some(emulator) = run_order("Func<int> f = () => { x = 9; return 1; }; result = x + f();")
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "result"), 4);
}

// ------------------------------------------------------------ shared statics
// (a static field is one per compilation, however many behaviour instances
// read it — through the holder program every behaviour's `__mensharp_statics`
// names, the way the Unity package wires a scene)

/// Compiles `source` once and wires the named instances of its behaviours
/// into one world with the statics holder; an instance's index is its
/// behaviour reference (the holder is index 0).
fn statics_world(source: &str, instances: &[(&str, &str)]) -> Option<men_sharp_asm::World> {
    let dir = dotnet_shared_dir()?;
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let files = compiler.parse(sources);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);
    assert_eq!(bodies.errors, vec![], "type errors");
    let programs =
        compiler.generate_udon_behaviours(&declarations, &signatures, &bodies, &references, &files);
    let program_of = |class_path: &str| {
        programs
            .iter()
            .find(|program| program.class_path == class_path)
            .unwrap_or_else(|| panic!("no program {class_path}"))
    };

    let mut world = men_sharp_asm::World::new();
    let holder = program_of("MenSharp.Statics");
    let assembled = holder.output.program.assemble().unwrap();
    let emulator = Emulator::new(&holder.output.program, &assembled);
    let holder_index = world.add("MenSharp.Statics", assembled, emulator);
    for (name, class_path) in instances {
        let program = program_of(class_path);
        assert!(
            program.output.errors.is_empty(),
            "codegen errors for {class_path}: {:#?}",
            program.output.errors
        );
        let assembled = program.output.program.assemble().unwrap();
        let mut emulator = Emulator::new(&program.output.program, &assembled);
        // a program with no shared static to reach has no reference either
        emulator.set_value("__mensharp_statics", Value::Behaviour(holder_index));
        world.add(name, assembled, emulator);
    }
    Some(world)
}

fn world_int(world: &men_sharp_asm::World, instance: &str, name: &str) -> i32 {
    let index = world.index_of(instance).unwrap();
    int_of(world.program(index), name)
}

#[test]
fn a_static_field_is_one_for_every_behaviour_instance() {
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public static class Counter
            {
                public static int count;
                public static int Next() { count++; return count; }
            }
            public class Clicker : MenSharpBehaviour
            {
                public int seen;
                public void Interact() { seen = Counter.Next(); }
            }
            public class Resetter : MenSharpBehaviour
            {
                public void Interact() { Counter.count = 10; }
            }
        }
        "#;
    let Some(mut world) = statics_world(
        source,
        &[
            ("a", "Game.Clicker"),
            ("b", "Game.Clicker"),
            ("r", "Game.Resetter"),
        ],
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b, r) = (
        world.index_of("a").unwrap(),
        world.index_of("b").unwrap(),
        world.index_of("r").unwrap(),
    );
    world.raise(a, "_interact").unwrap();
    world.raise(b, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "seen"), 1);
    assert_eq!(world_int(&world, "b", "seen"), 2);
    // another class writes the same field
    world.raise(r, "_interact").unwrap();
    world.raise(a, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "seen"), 11);
}

#[test]
fn a_behaviour_classs_own_static_field_is_shared_too() {
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public class Door : MenSharpBehaviour
            {
                public static int opened;
                public int mine;
                public void Interact() { opened++; mine = opened; }
            }
        }
        "#;
    let Some(mut world) = statics_world(source, &[("a", "Game.Door"), ("b", "Game.Door")]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b) = (world.index_of("a").unwrap(), world.index_of("b").unwrap());
    world.raise(a, "_interact").unwrap();
    world.raise(b, "_interact").unwrap();
    world.raise(b, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "mine"), 1);
    assert_eq!(world_int(&world, "b", "mine"), 3);
}

#[test]
fn shared_static_initializers_and_constructors_run_once_and_objects_are_shared() {
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public static class Cfg
            {
                public static int calls;
                public static int value = Bump();
                public static int[] table = { 1, 2 };
                public static string name = "cfg";
                static Cfg() { calls += 100; }
                static int Bump() { calls++; return 7; }
            }
            public class Reader : MenSharpBehaviour
            {
                public int calls;
                public int value;
                public int second;
                public string name;
                public void Interact()
                {
                    calls = Cfg.calls;
                    value = Cfg.value;
                    second = Cfg.table[1];
                    name = Cfg.name;
                }
            }
            public class Writer : MenSharpBehaviour
            {
                public void Interact() { Cfg.table[1] = 9; Cfg.name = "changed"; }
            }
        }
        "#;
    let Some(mut world) = statics_world(
        source,
        &[
            ("a", "Game.Reader"),
            ("b", "Game.Reader"),
            ("w", "Game.Writer"),
        ],
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b, w) = (
        world.index_of("a").unwrap(),
        world.index_of("b").unwrap(),
        world.index_of("w").unwrap(),
    );
    world.raise(a, "_interact").unwrap();
    world.raise(w, "_interact").unwrap();
    world.raise(b, "_interact").unwrap();
    // the initializer and the constructor ran in one program only
    assert_eq!(world_int(&world, "a", "calls"), 101);
    assert_eq!(world_int(&world, "b", "calls"), 101);
    assert_eq!(world_int(&world, "a", "value"), 7);
    assert_eq!(world_int(&world, "b", "value"), 7);
    // the array is one object, and the string one field
    assert_eq!(world_int(&world, "a", "second"), 2);
    assert_eq!(world_int(&world, "b", "second"), 9);
    assert_eq!(string_of(world.program(a), "name"), "cfg");
    assert_eq!(string_of(world.program(b), "name"), "changed");
}

#[test]
fn constants_and_immutable_readonly_statics_stay_the_programs_own() {
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public static class Limits
            {
                public const int Max = 3;
                public static readonly int Floor = 4;
                public static readonly string Tag = "t";
            }
            public class User : MenSharpBehaviour
            {
                public int seen;
                public string tag;
                public void Interact() { seen = Limits.Max + Limits.Floor; tag = Limits.Tag; }
            }
        }
        "#;
    let Some(mut world) = statics_world(source, &[("a", "Game.User")]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let a = world.index_of("a").unwrap();
    world.raise(a, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "seen"), 7);
    assert_eq!(string_of(world.program(a), "tag"), "t");
    // nothing of theirs is shared, so no program ever made the array
    let holder = world.index_of("MenSharp.Statics").unwrap();
    assert!(
        matches!(
            world.program(holder).value_of("__statics"),
            Some(Value::Null)
        ),
        "{:?}",
        world.program(holder).value_of("__statics")
    );
    assert!(world.program(a).value_of("__mensharp_statics").is_none());
}

#[test]
fn a_behaviour_without_a_holder_keeps_statics_of_its_own() {
    // no scene, no holder: the program makes an array for itself, and its
    // statics work as they always did
    let Some(emulator) = run_behaviour(
        r#"
        using MenSharp;
        namespace Game
        {
            public static class Counter
            {
                public static int count;
                public static int Next() { count++; return count; }
            }
            public class Clicker : MenSharpBehaviour
            {
                public int seen;
                public void Interact() { Counter.Next(); seen = Counter.Next(); }
            }
        }
        "#,
        "Game.Clicker",
        "_interact",
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    assert_eq!(int_of(&emulator, "seen"), 2);
}

#[test]
fn a_static_event_reaches_every_behaviour_that_subscribed() {
    // a delegate is one program's code: another program that invokes it
    // hands it back to its owner (see `delegates`)
    let source = r#"
        using System;
        using MenSharp;
        namespace Game
        {
            public static class Bus
            {
                public static event Action<int> OnPing;
                public static void Ping(int value) { OnPing?.Invoke(value); }
            }
            public class Node : MenSharpBehaviour
            {
                public int seen;
                public int calls;
                public void Subscribe() { Bus.OnPing += value => { seen = value; calls++; }; }
            }
            public class Button : MenSharpBehaviour
            {
                public int value;
                public void Interact() { Bus.Ping(value); }
            }
        }
        "#;
    let Some(mut world) = statics_world(
        source,
        &[
            ("a", "Game.Node"),
            ("b", "Game.Node"),
            ("button", "Game.Button"),
        ],
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b, button) = (
        world.index_of("a").unwrap(),
        world.index_of("b").unwrap(),
        world.index_of("button").unwrap(),
    );
    world.raise(a, "Subscribe").unwrap();
    world.raise(b, "Subscribe").unwrap();
    world
        .program_mut(button)
        .set_value("value", Value::Int32(7));
    world.raise(button, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "seen"), 7);
    assert_eq!(world_int(&world, "b", "seen"), 7);
    world
        .program_mut(button)
        .set_value("value", Value::Int32(9));
    world.raise(button, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "seen"), 9);
    assert_eq!(world_int(&world, "a", "calls"), 2);
    assert_eq!(world_int(&world, "b", "calls"), 2);
}

#[test]
fn a_delegate_of_another_behaviour_returns_its_result() {
    let source = r#"
        using System;
        using MenSharp;
        namespace Game
        {
            public static class Bus
            {
                public static Func<int, int> Scale;
            }
            public class Scaler : MenSharpBehaviour
            {
                public int factor = 3;
                public void Interact() { Bus.Scale = value => value * factor; }
            }
            public class User : MenSharpBehaviour
            {
                public int result;
                public void Interact() { result = Bus.Scale(5) + 1; }
            }
        }
        "#;
    let Some(mut world) = statics_world(source, &[("s", "Game.Scaler"), ("u", "Game.User")]) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (s, u) = (world.index_of("s").unwrap(), world.index_of("u").unwrap());
    world.raise(s, "_interact").unwrap();
    world.raise(u, "_interact").unwrap();
    assert_eq!(world_int(&world, "u", "result"), 16);
}

#[test]
fn a_delegate_reached_through_a_shared_object_runs_in_its_owner() {
    // the hole a static object with an event field would have been
    let source = r#"
        using System;
        using MenSharp;
        namespace Game
        {
            public class Hub
            {
                public event Action<string> Said;
                public void Say(string text) { Said?.Invoke(text); }
            }
            public static class World { public static Hub hub = new Hub(); }
            public class Listener : MenSharpBehaviour
            {
                public string heard = "";
                public void Subscribe() { World.hub.Said += text => { heard = heard + text; }; }
            }
            public class Speaker : MenSharpBehaviour
            {
                public void Interact() { World.hub.Say("hi"); World.hub.Say("!"); }
            }
        }
        "#;
    let Some(mut world) = statics_world(source, &[("l", "Game.Listener"), ("s", "Game.Speaker")])
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (l, s) = (world.index_of("l").unwrap(), world.index_of("s").unwrap());
    world.raise(l, "Subscribe").unwrap();
    world.raise(s, "_interact").unwrap();
    assert_eq!(string_of(world.program(l), "heard"), "hi!");
}

#[test]
fn a_subscriber_can_remove_its_handler_from_a_static_event() {
    let source = r#"
        using System;
        using MenSharp;
        namespace Game
        {
            public static class Bus
            {
                public static event Action OnPing;
                public static void Ping() { OnPing?.Invoke(); }
            }
            public class Node : MenSharpBehaviour
            {
                public int hits;
                private Action handler;
                public void Subscribe() { handler = () => { hits++; }; Bus.OnPing += handler; }
                public void Unsubscribe() { Bus.OnPing -= handler; }
            }
            public class Button : MenSharpBehaviour
            {
                public void Interact() { Bus.Ping(); }
            }
        }
        "#;
    let Some(mut world) = statics_world(
        source,
        &[
            ("a", "Game.Node"),
            ("b", "Game.Node"),
            ("button", "Game.Button"),
        ],
    ) else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b, button) = (
        world.index_of("a").unwrap(),
        world.index_of("b").unwrap(),
        world.index_of("button").unwrap(),
    );
    world.raise(a, "Subscribe").unwrap();
    world.raise(b, "Subscribe").unwrap();
    world.raise(button, "_interact").unwrap();
    world.raise(a, "Unsubscribe").unwrap();
    world.raise(button, "_interact").unwrap();
    assert_eq!(world_int(&world, "a", "hits"), 1);
    assert_eq!(world_int(&world, "b", "hits"), 2);
}

#[test]
fn a_static_synced_field_is_reported() {
    let source = r#"
        using MenSharp;
        using UdonSharp;
        namespace Game
        {
            public class Score : MenSharpBehaviour
            {
                [UdonSynced] public static int total;
                public void Interact() { total++; }
            }
        }
        "#;
    let mut sources = vec![SourceCode::new("test.cs", source)];
    sources.extend(Compiler::corlib_sources());
    let Some(program) = compile_behaviour(sources, "Game.Score") else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let messages: Vec<String> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Game.Score.total") && m.contains("[UdonSynced]")),
        "{messages:?}"
    );
}

#[test]
fn a_shared_static_initializer_runs_before_the_field_it_needs_is_read() {
    // layout order is by name — `Cfg.twice` before `Other.Value` — but the
    // read of `Other.Value` inside `twice`'s initializer initializes it first
    let source = r#"
        using MenSharp;
        namespace Game
        {
            public static class Cfg
            {
                public static int seed;
                public static int twice = Other.Value * 2;
                static Cfg() { seed = 42 + twice; }
            }
            public static class Other
            {
                public static int Value = Compute();
                static int Compute() { return 10; }
            }
            public class Reader : MenSharpBehaviour
            {
                public int seed;
                public int twice;
                public void Interact() { seed = Cfg.seed; twice = Cfg.twice; }
            }
        }
        "#;
    let Some(mut world) = statics_world(source, &[("a", "Game.Reader"), ("b", "Game.Reader")])
    else {
        eprintln!("skipped: no .NET runtime");
        return;
    };
    let (a, b) = (world.index_of("a").unwrap(), world.index_of("b").unwrap());
    world.raise(b, "_interact").unwrap();
    world.raise(a, "_interact").unwrap();
    for instance in ["a", "b"] {
        assert_eq!(world_int(&world, instance, "seed"), 62, "{instance}");
        assert_eq!(world_int(&world, instance, "twice"), 20, "{instance}");
    }
}
