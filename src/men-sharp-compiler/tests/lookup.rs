//! Member lookup and cross-assembly resolution against real assemblies.
//!
//! These tests exercise the whole chain: dll bytes -> metadata -> semantic types ->
//! inheritance-walking member lookup. Each locates its inputs at runtime and passes
//! vacuously (with a note on stderr) when the machine has no .NET runtime or Unity
//! editor installed.

use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};
use men_sharp_semantics::{
    ExternalTypes, MemberOrigin, MemberSignature, SymbolKind, Type, TypeSystem, TypeTarget,
};

fn dotnet_shared_dir() -> Option<std::path::PathBuf> {
    let base = std::path::Path::new("/usr/share/dotnet/shared/Microsoft.NETCore.App");
    let mut versions: Vec<_> = std::fs::read_dir(base).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    versions.pop().map(|entry| entry.path())
}

fn unity_managed_dir() -> Option<std::path::PathBuf> {
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

fn external(ty: &Type) -> men_sharp_semantics::ExternalTypeId {
    match ty {
        Type::Named {
            target: TypeTarget::External(id),
            ..
        } => *id,
        other => panic!("expected an external named type, got {other:?}"),
    }
}

#[test]
fn generic_members_instantiate_against_the_real_core_library() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap()];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new("empty.cs", "class Empty {}")]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &references,
    };

    let int32 = Type::Named {
        target: TypeTarget::External(references.find_type(&["System"], "Int32", 0).unwrap()),
        arguments: Vec::new(),
    };
    let list_of_int = Type::Named {
        target: TypeTarget::External(
            references
                .find_type(&["System", "Collections", "Generic"], "List", 1)
                .unwrap(),
        ),
        arguments: vec![int32.clone()],
    };

    // List<int>.Add(T) instantiates to Add(int)
    let add: Vec<_> = system
        .members_named(&list_of_int, "Add")
        .into_iter()
        .filter(|candidate| candidate.declaring_type == list_of_int)
        .collect();
    assert_eq!(add.len(), 1);
    let Some(MemberSignature::Function(function)) = &add[0].signature else {
        panic!("Add should be a function");
    };
    assert_eq!(function.parameters[0].parameter_type, int32);
    assert_eq!(function.return_type, Type::Void);

    // Count is an instance int property
    let count = system.members_named(&list_of_int, "Count");
    assert!(count.iter().any(|candidate| {
        candidate.signature == Some(MemberSignature::Property(int32.clone()))
            && !candidate.is_static
    }));

    // ToString comes from System.Object, several bases up the chain
    let to_string = system.members_named(&list_of_int, "ToString");
    assert!(!to_string.is_empty());
    assert!(to_string.iter().any(|candidate| {
        references.display_name(external(&candidate.declaring_type)) == "System.Object"
    }));

    // int.TryParse(string, out int): the out parameter comes through
    let try_parse = system.members_named(&int32, "TryParse");
    assert!(try_parse.iter().any(|candidate| {
        let Some(MemberSignature::Function(function)) = &candidate.signature else {
            return false;
        };
        candidate.is_static
            && function.parameters.len() == 2
            && function.parameters[1].passing == men_sharp_semantics::ParameterPassing::Out
            && function.parameters[1].parameter_type == int32
    }));
}

#[test]
fn unity_hierarchy_walks_across_module_boundaries() {
    let Some(unity) = unity_managed_dir() else {
        eprintln!("skipped: no Unity editor on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let bytes = vec![
        std::fs::read(unity.join("Managed/UnityEngine/UnityEngine.CoreModule.dll")).unwrap(),
        std::fs::read(unity.join("MonoBleedingEdge/lib/mono/unityaot-linux/mscorlib.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    // a source class deriving from a Unity type, so the chain crosses from
    // source symbols into metadata
    let files = compiler.parse(vec![SourceCode::new(
        "door.cs",
        r#"
        using UnityEngine;

        public class Door : MonoBehaviour
        {
            public int uses;
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    assert_eq!(signatures.errors, vec![]);

    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &references,
    };

    let root = declarations.table.root();
    let door = Type::Named {
        target: TypeTarget::Source(declarations.table.symbol(root).members_named("Door")[0]),
        arguments: Vec::new(),
    };

    // its own field first
    let uses = system.members_named(&door, "uses");
    assert_eq!(uses.len(), 1);
    assert!(matches!(uses[0].origin, MemberOrigin::Source(_)));

    // `transform` is a property on UnityEngine.Component, two metadata bases up
    let transform = system.members_named(&door, "transform");
    assert_eq!(transform.len(), 1);
    assert_eq!(
        references.display_name(external(&transform[0].declaring_type)),
        "UnityEngine.Component"
    );
    let Some(MemberSignature::Property(transform_type)) = &transform[0].signature else {
        panic!("transform should be a property");
    };
    assert_eq!(
        references.display_name(external(transform_type)),
        "UnityEngine.Transform"
    );

    // `GetInstanceID` sits even higher, on UnityEngine.Object
    let get_id = system.members_named(&door, "GetInstanceID");
    assert!(!get_id.is_empty());
    assert_eq!(
        references.display_name(external(&get_id[0].declaring_type)),
        "UnityEngine.Object"
    );

    // and `name`, whose base chain passes a TypeRef into mscorlib and back
    let name = system.members_named(&door, "name");
    assert_eq!(
        references.display_name(external(&name[0].declaring_type)),
        "UnityEngine.Object"
    );
    let Some(MemberSignature::Property(name_type)) = &name[0].signature else {
        panic!("name should be a property");
    };
    assert_eq!(
        references.display_name(external(name_type)),
        "System.String"
    );

    // GetComponent<T>() reports its own generic parameter as the return type
    let get_component = system.members_named(&door, "GetComponent");
    assert!(get_component.iter().any(|candidate| {
        candidate.kind == SymbolKind::Method
            && candidate.arity == 1
            && candidate.signature
                == Some(MemberSignature::Function(
                    men_sharp_semantics::FunctionSignature {
                        return_type: Type::ExternalMethodTypeParameter(0),
                        parameters: vec![],
                    },
                ))
    }));
}

#[test]
fn type_forwarders_are_chased_through_facades() {
    let Some(dir) = dotnet_shared_dir() else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    // System.Console.dll refers to types as living in the System.Runtime facade,
    // which only *forwards* them to System.Private.CoreLib
    let bytes = vec![
        std::fs::read(dir.join("System.Console.dll")).unwrap(),
        std::fs::read(dir.join("System.Runtime.dll")).unwrap(),
        std::fs::read(dir.join("System.Private.CoreLib.dll")).unwrap(),
    ];
    let references = compiler.load_references(&bytes).unwrap();

    let files = compiler.parse(vec![SourceCode::new("empty.cs", "class Empty {}")]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);
    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &references,
    };

    let console = Type::Named {
        target: TypeTarget::External(references.find_type(&["System"], "Console", 0).unwrap()),
        arguments: Vec::new(),
    };
    let string = references.find_type(&["System"], "String", 0).unwrap();

    // WriteLine(string)'s parameter type token points into System.Runtime; it must
    // land on the CoreLib definition of System.String via the forwarder
    let write_line = system.members_named(&console, "WriteLine");
    assert!(write_line.iter().any(|candidate| {
        let Some(MemberSignature::Function(function)) = &candidate.signature else {
            return false;
        };
        function.parameters.len() == 1
            && function.parameters[0].parameter_type
                == Type::Named {
                    target: TypeTarget::External(string),
                    arguments: Vec::new(),
                }
    }));
}
