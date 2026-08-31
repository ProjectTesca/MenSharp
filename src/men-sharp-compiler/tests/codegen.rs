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
            .any(|error| error.message.contains("recursion is not supported")),
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
    assert_eq!(names, vec!["Game.Door", "Game.Lamp"]);
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
            .any(|error| error.message.contains("cannot be constructed")),
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
    let messages: Vec<&str> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.as_str())
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
    let messages: Vec<&str> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.as_str())
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
    let messages: Vec<&str> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.as_str())
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
    assert!(
        messages
            .iter()
            .any(|message| message.contains("belongs to the behaviour itself")),
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
fn referring_to_another_behaviour_is_an_error() {
    // this used to compile: the field became an `object[]` public variable and
    // the other behaviour's method was inlined into this program, so nothing
    // said a word until it failed on the VM
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
                        public void Open() { }
                    }

                    public class Switch : MenSharp.MenSharpBehaviour
                    {
                        public Door door;
                        public Door[] doors;
                        public void Interact() { door.Open(); }
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
    let messages: Vec<&str> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.as_str())
        .collect();
    // both the single reference and the array of them
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("is a MenSharpBehaviour"))
            .count(),
        2,
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
    let messages: Vec<&str> = program
        .output
        .errors
        .iter()
        .map(|error| error.message.as_str())
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
