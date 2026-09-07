//! Signature resolution: every written type turned into a .

mod common;

use common::{MockExternal, declarations, external, find, member_signature};
use men_sharp_semantics::types::{ExternalTypeId, MemberSignature, TupleElement, Type, TypeTarget};
use men_sharp_semantics::{SemanticErrorKind, resolve_signatures};

#[test]
pub fn predefined_and_external_types_resolve() {
    declarations!(
        declarations,
        r#"
        using UnityEngine;

        namespace Game
        {
            public class Door : MonoBehaviour
            {
                public int count;
                public string label;
                public Debug helper;
                public UnityEngine.SceneManagement.SceneManager manager;
            }
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);

    assert_eq!(signatures.errors, vec![]);

    let door = find(&declarations, "Game.Door");
    assert_eq!(
        signatures.base_types.get(&door),
        Some(&vec![external(mock.id_of("UnityEngine", "MonoBehaviour"))])
    );

    for (member, namespace, name) in [
        ("count", "System", "Int32"),
        ("label", "System", "String"),
        ("helper", "UnityEngine", "Debug"),
        ("manager", "UnityEngine.SceneManagement", "SceneManager"),
    ] {
        assert_eq!(
            member_signature(&declarations, &signatures, "Game.Door", member),
            MemberSignature::Field(external(mock.id_of(namespace, name))),
            "field {member}"
        );
    }
}

#[test]
pub fn source_types_shadow_and_generics_apply() {
    declarations!(
        declarations,
        r#"
        namespace Game
        {
            public class Item {}
            public class Bag<T>
            {
                public T content;
                public System.Collections.Generic.List<Item> items;
                public Bag<Item> nested;
            }
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    let item = find(&declarations, "Game.Item");
    let bag = find(&declarations, "Game.Bag");

    // T resolves to the type parameter symbol
    let content = member_signature(&declarations, &signatures, "Game.Bag", "content");
    let bag_symbol = declarations.table.symbol(bag);
    assert_eq!(
        content,
        MemberSignature::Field(Type::TypeParameter(bag_symbol.type_parameters[0]))
    );

    // a fully-qualified external generic with a source argument
    assert_eq!(
        member_signature(&declarations, &signatures, "Game.Bag", "items"),
        MemberSignature::Field(Type::Named {
            target: TypeTarget::External(mock.id_of("System.Collections.Generic", "List")),
            arguments: vec![Type::Named {
                target: TypeTarget::Source(item),
                arguments: Vec::new(),
            }],
        })
    );

    // a source generic applied to a source argument
    assert_eq!(
        member_signature(&declarations, &signatures, "Game.Bag", "nested"),
        MemberSignature::Field(Type::Named {
            target: TypeTarget::Source(bag),
            arguments: vec![Type::Named {
                target: TypeTarget::Source(item),
                arguments: Vec::new(),
            }],
        })
    );
}

#[test]
pub fn method_signatures_resolve_with_their_own_type_parameters() {
    declarations!(
        declarations,
        r#"
        public class Util
        {
            public U Convert<U>(int input, ref U seed) { return seed; }
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    let MemberSignature::Function(function) =
        member_signature(&declarations, &signatures, "Util", "Convert")
    else {
        panic!("expected a function signature");
    };

    let util = find(&declarations, "Util");
    let convert = declarations.table.symbol(util).members_named("Convert")[0];
    let u = declarations.table.symbol(convert).type_parameters[0];

    assert_eq!(function.return_type, Type::TypeParameter(u));
    assert_eq!(function.parameters.len(), 2);
    assert_eq!(
        function.parameters[0].parameter_type,
        external(mock.id_of("System", "Int32"))
    );
    assert_eq!(
        function.parameters[1].passing,
        men_sharp_semantics::types::ParameterPassing::Ref
    );
    assert_eq!(
        function.parameters[1].parameter_type,
        Type::TypeParameter(u)
    );
}

#[test]
pub fn aliases_static_usings_and_nested_types() {
    declarations!(
        declarations,
        r#"
        using L = System.Collections.Generic.List<int>;
        using static System.Collections.Generic.List<int>;

        public class Holder
        {
            public L list;
            public Enumerator<int> cursor;
            public Outer.Inner inner;
        }
        public class Outer
        {
            public class Inner {}
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    // the alias carries its own arguments
    assert_eq!(
        member_signature(&declarations, &signatures, "Holder", "list"),
        MemberSignature::Field(Type::Named {
            target: TypeTarget::External(mock.id_of("System.Collections.Generic", "List")),
            arguments: vec![external(mock.id_of("System", "Int32"))],
        })
    );

    // `using static` brings the external nested type into scope
    let MemberSignature::Field(Type::Named { target, .. }) =
        member_signature(&declarations, &signatures, "Holder", "cursor")
    else {
        panic!("expected a named field type");
    };
    assert_eq!(
        target,
        TypeTarget::External(ExternalTypeId {
            assembly: 0,
            type_index: mock.types.len() as u32,
        })
    );

    // a nested source type through its enclosing type
    let outer = find(&declarations, "Outer");
    let inner = declarations.table.symbol(outer).members_named("Inner")[0];
    assert_eq!(
        member_signature(&declarations, &signatures, "Holder", "inner"),
        MemberSignature::Field(Type::Named {
            target: TypeTarget::Source(inner),
            arguments: Vec::new(),
        })
    );
}

#[test]
pub fn suffixes_tuples_and_special_types() {
    declarations!(
        declarations,
        r#"
        public class Shapes
        {
            public int[] a;
            public int?[] b;
            public int[][,] c;
            public (int x, string) d;
            public dynamic e;
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    let int32 = external(mock.id_of("System", "Int32"));

    assert_eq!(
        member_signature(&declarations, &signatures, "Shapes", "a"),
        MemberSignature::Field(Type::Array {
            element: Box::new(int32.clone()),
            rank: 1,
        })
    );
    assert_eq!(
        member_signature(&declarations, &signatures, "Shapes", "b"),
        MemberSignature::Field(Type::Array {
            element: Box::new(Type::Nullable(Box::new(int32.clone()))),
            rank: 1,
        })
    );
    // int[][,]: a [] array of [,] arrays
    assert_eq!(
        member_signature(&declarations, &signatures, "Shapes", "c"),
        MemberSignature::Field(Type::Array {
            element: Box::new(Type::Array {
                element: Box::new(int32.clone()),
                rank: 2,
            }),
            rank: 1,
        })
    );
    assert_eq!(
        member_signature(&declarations, &signatures, "Shapes", "d"),
        MemberSignature::Field(Type::Tuple(vec![
            TupleElement {
                name: Some("x".into()),
                element: int32.clone(),
            },
            TupleElement {
                name: None,
                element: external(mock.id_of("System", "String")),
            },
        ]))
    );
    assert_eq!(
        member_signature(&declarations, &signatures, "Shapes", "e"),
        MemberSignature::Field(Type::Dynamic)
    );
}

#[test]
pub fn unresolved_and_ambiguous_names_are_reported() {
    declarations!(
        declarations,
        r#"
        namespace A { public class Thing {} }
        namespace B { public class Thing {} }
        namespace App
        {
            using A;
            using B;

            public class User
            {
                public Thing conflicted;
                public Missing missing;
            }
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);

    assert_eq!(signatures.errors.len(), 2);
    assert!(
        signatures
            .errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::AmbiguousTypeName))
    );
    assert!(
        signatures
            .errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::UnresolvedTypeName))
    );

    // recovery: both fields still have signatures, as Error
    assert_eq!(
        member_signature(&declarations, &signatures, "App.User", "conflicted"),
        MemberSignature::Field(Type::Error)
    );
    assert_eq!(
        member_signature(&declarations, &signatures, "App.User", "missing"),
        MemberSignature::Field(Type::Error)
    );
}
