//! Body checking against real assemblies: gameplay-shaped code through the whole
//! pipeline — parse, declarations, signatures, then a type for every expression.
//!
//! Tests pass vacuously (with a note on stderr) when the machine has no .NET
//! runtime or Unity editor installed.

use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};
use men_sharp_semantics::SemanticErrorKind;

fn dotnet_shared_dir() -> Option<std::path::PathBuf> {
    let base = std::path::Path::new("/usr/share/dotnet/shared/Microsoft.NETCore.App");
    let mut versions: Vec<_> = std::fs::read_dir(base).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    versions.pop().map(|entry| entry.path())
}

fn unity_data_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let editors = std::path::Path::new(&home).join("Unity/Hub/Editor");
    for editor in std::fs::read_dir(editors).ok()?.flatten() {
        let path = editor.path().join("Editor/Data");
        if path
            .join("Managed/UnityEngine/UnityEngine.CoreModule.dll")
            .exists()
        {
            return Some(path);
        }
    }
    None
}

#[test]
fn a_udonsharp_shaped_class_checks_cleanly_against_unity() {
    let Some(unity) = unity_data_dir() else {
        eprintln!("skipped: no Unity editor on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(unity.join("Managed/UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
        std::fs::read(unity.join("Managed/UnityEngine/UnityEngine.PhysicsModule.dll")).unwrap(),
        std::fs::read(unity.join("MonoBleedingEdge/lib/mono/unityaot-linux/mscorlib.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "door.cs",
        r#"
        using UnityEngine;
        using System.Collections.Generic;

        namespace Game
        {
            public class Door : MonoBehaviour
            {
                public Vector3 openOffset;
                public float speed = 1.5f;
                public Transform[] hinges;
                private List<string> visitors = new List<string>();
                private bool open;

                public void Interact(GameObject player)
                {
                    Debug.Log("interacted: " + player.name);
                    visitors.Add(player.name);

                    var target = transform.position + openOffset * speed;
                    transform.position = target;

                    foreach (var hinge in hinges)
                    {
                        hinge.Rotate(Vector3.up, speed * Time.deltaTime);
                    }

                    var rigidbody = GetComponent<Rigidbody>();
                    if (rigidbody != null && !open)
                    {
                        rigidbody.mass = 2f;
                        open = true;
                    }

                    if (player.transform.position.y > 1.0f)
                    {
                        Debug.LogWarning("player is airborne");
                    }
                }

                public int VisitorCount => visitors.Count;

                public string DescribeVisitor(int index)
                {
                    if (index < 0 || index >= visitors.Count) { return "nobody"; }
                    return visitors[index];
                }
            }
        }
        "#,
    )]);

    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    assert_eq!(declarations.errors, vec![]);
    assert_eq!(signatures.errors, vec![]);
    assert_eq!(bodies.errors, vec![], "the class should check cleanly");
    // every expression in those bodies got a type
    eprintln!("typed expressions: {}", bodies.expression_types.len());
    assert!(bodies.expression_types.len() > 40);
}

#[test]
fn real_mistakes_are_caught_with_real_types() {
    let Some(unity) = unity_data_dir() else {
        eprintln!("skipped: no Unity editor on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(unity.join("Managed/UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
        std::fs::read(unity.join("MonoBleedingEdge/lib/mono/unityaot-linux/mscorlib.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "broken.cs",
        r#"
        using UnityEngine;

        public class Broken : MonoBehaviour
        {
            void Run()
            {
                int x = transform.position;
                transform.position = 1;
                Debug.Log();
                var v = Vector3.up + 1;
                gameObject.Frobnicate();
            }
        }
        "#,
    )]);

    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let kinds: Vec<&SemanticErrorKind> = bodies.errors.iter().map(|error| &error.kind).collect();
    assert_eq!(kinds.len(), 5, "{kinds:?}");

    // int x = transform.position; — Vector3 into int, both named with real names
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "int".to_string(),
            found: "UnityEngine.Vector3".to_string(),
        }
    );
    // transform.position = 1;
    assert!(matches!(kinds[1], SemanticErrorKind::TypeMismatch { .. }));
    // Debug.Log() has no zero-argument overload
    assert!(matches!(kinds[2], SemanticErrorKind::NoMatchingOverload));
    // Vector3 + int has no operator
    assert!(matches!(
        kinds[3],
        SemanticErrorKind::InvalidOperator { .. }
    ));
    // no such member on GameObject
    assert!(matches!(kinds[4], SemanticErrorKind::UnknownMember { .. }));
}

#[test]
fn spec_inference_against_the_real_core_library() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "inference.cs",
        r#"
        using System;
        using System.Collections.Generic;

        public class Inference
        {
            // IEnumerable<T> parameters accept List<T> and T[] through lower-bound
            // inference; the probes prove what T became
            T FirstOf<T>(IEnumerable<T> items) { return default; }

            void Run(List<string> names, int[] numbers)
            {
                string a = FirstOf(names);
                int b = FirstOf(numbers);
                int wrongA = FirstOf(names);

                // List<T>.Find takes Predicate<T>: the lambda's x is a string here
                var found = names.Find(x => x.Length > 2);
                string c = found;

                // ForEach takes Action<T>
                names.ForEach(x => { var upper = x.ToUpper(); });

                // Func/Action locals with their natural types, then as targets
                Func<int, int> twice = x => x * 2;
                Comparison<string> compare = (left, right) => left.Length - right.Length;
                names.Sort(compare);

                // a lambda body error surfaces inside the lambda
                names.ForEach(x => { int broken = x; });
            }
        }
        "#,
    )]);

    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let kinds: Vec<&SemanticErrorKind> = bodies.errors.iter().map(|error| &error.kind).collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    // wrongA: T really became string
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "int".to_string(),
            found: "string".to_string(),
        }
    );
    // int broken = x; inside the ForEach lambda, where x: string
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "int".to_string(),
            found: "string".to_string(),
        }
    );
}

#[test]
fn linq_chains_flow_through_real_extension_methods() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap(),
        std::fs::read(dir.join("System.Linq.dll")).unwrap(),
        std::fs::read(dir.join("System.Runtime.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "linq.cs",
        r#"
        using System.Collections.Generic;
        using System.Linq;

        public class Query
        {
            void Run(List<string> names, int[] numbers)
            {
                // the full chain: extension lookup, receiver-driven inference,
                // and lambda bodies typed link by link
                var lengths = names.Where(x => x.Length > 2).Select(x => x.Length).ToArray();
                string probe1 = lengths;

                var total = numbers.Sum();
                string probe2 = total;

                var first = names.FirstOrDefault();
                string ok = first;

                var pairs = names.Select(x => (x, x.Length)).ToList();

                // without `using System.Linq;` this would be UnknownMember; with a
                // wrong lambda the error lands inside the lambda body
                names.Where(x => x.Missing > 0);
            }
        }
        "#,
    )]);

    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let kinds: Vec<&SemanticErrorKind> = bodies.errors.iter().map(|error| &error.kind).collect();
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    // Where(...).Select(x => x.Length).ToArray() really produced int[]
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int[]".to_string(),
        }
    );
    // Sum() over int[] produced int
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
    // x.Missing inside the lambda, where x: string
    assert!(matches!(kinds[2], SemanticErrorKind::UnknownMember { .. }));
}

#[test]
fn core_library_generics_flow_through_bodies() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "lists.cs",
        r#"
        using System;
        using System.Collections.Generic;

        public class Lists
        {
            void Run()
            {
                var numbers = new List<int>();
                numbers.Add(1);
                int first = numbers[0];
                long total = 0;
                foreach (var number in numbers) { total += number; }

                if (int.TryParse("42", out var parsed))
                {
                    total = Math.Max(parsed, first);
                }

                var map = new Dictionary<string, List<int>>();
                map["a"] = numbers;
                List<int> back = map["a"];

                numbers.Add("wrong");
                string bad = numbers[0];
            }
        }
        "#,
    )]);

    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let bodies = compiler.check_bodies(&declarations, &signatures, &references);

    let kinds: Vec<&SemanticErrorKind> = bodies.errors.iter().map(|error| &error.kind).collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    // Add("wrong") on List<int>
    assert!(matches!(kinds[0], SemanticErrorKind::NoMatchingOverload));
    // string bad = numbers[0]; — the indexer really returned int
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}
