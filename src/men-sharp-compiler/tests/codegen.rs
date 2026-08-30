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

/// Compile `source`, run entry `event`, and return the finished emulator.
fn run(source: &str, event: &str) -> Option<Emulator> {
    let dir = dotnet_shared_dir()?;
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new("test.cs", source)]);
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
    emulator
        .run(&assembled, event)
        .unwrap_or_else(|error| panic!("emulator error: {error:?}\n{}", output.program.dump()));
    Some(emulator)
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
fn recursion_is_a_codegen_error_not_a_crash() {
    let dir = dotnet_shared_dir();
    let Some(dir) = dir else {
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
            .any(|error| error.message.contains("recursive")),
        "expected a recursion diagnostic, got {:#?}",
        output.errors
    );
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
