//! Verification against real assemblies found on this machine: the .NET runtime's
//! core library and, when a Unity editor is installed, UnityEngine.CoreModule.dll —
//! the very assembly MenSharp will consume in production.
//!
//! Each test locates its input at runtime and passes vacuously (with a note on
//! stderr) when the assembly is not present, so the suite still runs on machines
//! with neither .NET nor Unity.

use men_sharp_dotnet::{DotNetAssembly, TypeSig, TypeToken, Variance};

/// The newest installed Microsoft.NETCore.App runtime directory.
fn dotnet_shared_dir() -> Option<std::path::PathBuf> {
    let base = std::path::Path::new("/usr/share/dotnet/shared/Microsoft.NETCore.App");
    let mut versions: Vec<_> = std::fs::read_dir(base).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    versions.pop().map(|entry| entry.path())
}

fn unity_core_module() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let editors = std::path::Path::new(&home).join("Unity/Hub/Editor");
    for editor in std::fs::read_dir(editors).ok()?.flatten() {
        let path = editor
            .path()
            .join("Editor/Data/Managed/UnityEngine/UnityEngine.CoreModule.dll");
        if path.exists() {
            return Some(path);
        }
    }
    None
}

macro_rules! load_or_skip {
    ($path:expr) => {{
        let Some(path) = $path else {
            eprintln!("skipped: assembly not found on this machine");
            return;
        };
        std::fs::read(path).unwrap()
    }};
}

#[test]
fn core_library_types_and_signatures() {
    let bytes =
        load_or_skip!(dotnet_shared_dir().map(|dir| dir.join("System.Private.CoreLib.dll")));
    let assembly = DotNetAssembly::parse(&bytes).unwrap();

    assert_eq!(assembly.name, "System.Private.CoreLib");

    // System.String: a sealed public class with an int Length property
    let string = assembly.find_type("System", "String").unwrap();
    let string = assembly.type_definition(string);
    assert!(string.is_public());
    assert!(string.is_sealed());
    assert!(!string.is_value_type);

    let length = string
        .properties
        .iter()
        .find(|property| property.name == "Length")
        .unwrap();
    assert_eq!(length.signature.property_type, TypeSig::Int32);
    assert!(length.getter.is_some());
    assert!(length.setter.is_none());

    // System.Int32: a value type
    let int32 = assembly.find_type("System", "Int32").unwrap();
    assert!(assembly.type_definition(int32).is_value_type);

    // List<T>: generic arity in the name, an Add(T) method, a nested Enumerator
    let list = assembly
        .find_type("System.Collections.Generic", "List`1")
        .unwrap();
    let list_definition = assembly.type_definition(list);
    assert_eq!(list_definition.name_and_arity(), ("List", 1));
    assert_eq!(list_definition.generic_parameters.len(), 1);
    assert_eq!(list_definition.generic_parameters[0].name, "T");

    let add = list_definition
        .methods
        .iter()
        .find(|method| method.name == "Add" && method.signature.parameters.len() == 1)
        .unwrap();
    assert!(add.is_public());
    assert!(!add.is_static());
    assert_eq!(add.signature.parameters[0], TypeSig::TypeParameter(0));
    assert_eq!(add.signature.return_type, TypeSig::Void);
    assert_eq!(add.parameters[0].name, "item");

    let enumerator = list_definition
        .nested_types
        .iter()
        .map(|&nested| assembly.type_definition(nested))
        .find(|nested| nested.name == "Enumerator")
        .unwrap();
    assert!(enumerator.is_value_type);
    assert_eq!(enumerator.enclosing_type, Some(list));

    // an enum with its constant values
    let day = assembly.find_type("System", "DayOfWeek").unwrap();
    let day = assembly.type_definition(day);
    assert!(day.is_enum);
    let wednesday = day
        .fields
        .iter()
        .find(|field| field.name == "Wednesday")
        .unwrap();
    assert!(wednesday.is_literal());
    assert_eq!(
        wednesday.constant,
        Some(men_sharp_dotnet::Constant::Int32(3))
    );

    // variance: IEnumerable<out T>
    let enumerable = assembly
        .find_type("System.Collections.Generic", "IEnumerable`1")
        .unwrap();
    let enumerable = assembly.type_definition(enumerable);
    assert!(enumerable.is_interface());
    assert_eq!(
        enumerable.generic_parameters[0].variance,
        Variance::Covariant
    );

    // Math.Max(int, int): static with two parameters
    let math = assembly.find_type("System", "Math").unwrap();
    let max = assembly
        .type_definition(math)
        .methods
        .iter()
        .find(|method| {
            method.name == "Max" && method.signature.parameters == [TypeSig::Int32, TypeSig::Int32]
        })
        .unwrap();
    assert!(max.is_static());
    assert_eq!(max.signature.return_type, TypeSig::Int32);
}

#[test]
fn facade_assemblies_expose_forwarders() {
    let bytes = load_or_skip!(dotnet_shared_dir().map(|dir| dir.join("System.Runtime.dll")));
    let assembly = DotNetAssembly::parse(&bytes).unwrap();

    // System.Runtime defines almost nothing; System.String reaches it as a forwarder
    let string = assembly
        .forwarders
        .iter()
        .find(|forwarder| forwarder.namespace == "System" && forwarder.name == "String")
        .unwrap();
    assert_eq!(string.assembly, "System.Private.CoreLib");
}

#[test]
fn unity_core_module_if_installed() {
    let bytes = load_or_skip!(unity_core_module());
    let assembly = DotNetAssembly::parse(&bytes).unwrap();

    assert_eq!(assembly.name, "UnityEngine.CoreModule");

    // UnityEngine.Debug.Log(object): the canonical extern
    let debug = assembly.find_type("UnityEngine", "Debug").unwrap();
    let log = assembly
        .type_definition(debug)
        .methods
        .iter()
        .find(|method| method.name == "Log" && method.signature.parameters == [TypeSig::Object])
        .unwrap();
    assert!(log.is_static());
    assert_eq!(log.signature.return_type, TypeSig::Void);

    // Vector3: a struct with float fields and a static get-only `up` property
    let vector3 = assembly.find_type("UnityEngine", "Vector3").unwrap();
    let vector3_definition = assembly.type_definition(vector3);
    assert!(vector3_definition.is_value_type);
    for field in ["x", "y", "z"] {
        let field = vector3_definition
            .fields
            .iter()
            .find(|candidate| candidate.name == field)
            .unwrap();
        assert_eq!(field.field_type, TypeSig::Single);
        assert!(field.is_public());
        assert!(!field.is_static());
    }
    let up = vector3_definition
        .properties
        .iter()
        .find(|property| property.name == "up")
        .unwrap();
    assert!(up.getter.is_some());
    assert!(up.setter.is_none());

    // Transform.position: a Vector3-typed property on a class
    let transform = assembly.find_type("UnityEngine", "Transform").unwrap();
    let position = assembly
        .type_definition(transform)
        .properties
        .iter()
        .find(|property| property.name == "position")
        .unwrap();
    match &position.signature.property_type {
        TypeSig::Named {
            token: TypeToken::Definition(definition),
            ..
        } => assert_eq!(*definition, vector3),
        other => panic!("unexpected position type: {other:?}"),
    }

    // GameObject.GetComponent<T>(): a generic method returning its own parameter
    let game_object = assembly.find_type("UnityEngine", "GameObject").unwrap();
    let get_component = assembly
        .type_definition(game_object)
        .methods
        .iter()
        .find(|method| {
            method.name == "GetComponent" && method.signature.generic_parameter_count == 1
        })
        .unwrap();
    assert_eq!(
        get_component.signature.return_type,
        TypeSig::MethodTypeParameter(0)
    );
}

#[test]
fn extension_attribute_is_detected_on_linq() {
    let bytes = load_or_skip!(dotnet_shared_dir().map(|dir| dir.join("System.Linq.dll")));
    let assembly = DotNetAssembly::parse(&bytes).unwrap();

    let enumerable = assembly.find_type("System.Linq", "Enumerable").unwrap();
    let enumerable = assembly.type_definition(enumerable);

    // the static class and its methods both carry ExtensionAttribute
    assert!(enumerable.is_extension);
    let where_method = enumerable
        .methods
        .iter()
        .find(|method| method.name == "Where")
        .unwrap();
    assert!(where_method.is_extension);

    // an ordinary type does not
    let bytes =
        load_or_skip!(dotnet_shared_dir().map(|dir| dir.join("System.Private.CoreLib.dll")));
    let corelib = DotNetAssembly::parse(&bytes).unwrap();
    let string = corelib.find_type("System", "String").unwrap();
    assert!(!corelib.type_definition(string).is_extension);
}
