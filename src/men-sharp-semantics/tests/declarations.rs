//! Collecting and merging declarations: what the source says it declares.

mod common;

use common::{declarations, find};
use men_sharp_semantics::{FileId, SemanticErrorKind, SymbolKind, SyntaxRef};

#[test]
pub fn partial_class_across_files_is_one_symbol() {
    declarations!(
        declarations,
        "namespace App { public partial class Player { void Move() {} } }",
        "namespace App { public partial class Player { int health; } }",
    );

    assert_eq!(declarations.errors, vec![]);

    let player = find(&declarations, "App.Player");
    let symbol = declarations.table.symbol(player);
    assert_eq!(symbol.declarations.len(), 2);
    assert!(symbol.is_partial);
    assert_eq!(symbol.members_named("Move").len(), 1);
    assert_eq!(symbol.members_named("health").len(), 1);
}

#[test]
pub fn namespaces_are_open_across_files_and_spellings() {
    declarations!(
        declarations,
        "namespace A.B { class First {} }",
        "namespace A { namespace B { class Second {} } }",
        "namespace A.B;\nclass Third {}",
    );

    assert_eq!(declarations.errors, vec![]);

    let b = find(&declarations, "A.B");
    let symbol = declarations.table.symbol(b);
    assert_eq!(symbol.kind, SymbolKind::Namespace);
    assert_eq!(symbol.declarations.len(), 3);
    assert_eq!(symbol.members.len(), 3);
    assert_eq!(declarations.table.fully_qualified_name(b), "A.B");
}

#[test]
pub fn duplicate_type_is_reported_and_still_merged() {
    declarations!(
        declarations,
        "class Foo { void A() {} }",
        "class Foo { void B() {} }",
    );

    assert_eq!(declarations.errors.len(), 1);
    let error = &declarations.errors[0];
    assert!(matches!(
        error.kind,
        SemanticErrorKind::DuplicateTypeDefinition {
            first_file: FileId(0),
            ..
        }
    ));
    assert_eq!(error.file, FileId(1));

    // recovery: both declarations' members stay visible
    let foo = declarations.table.symbol(find(&declarations, "Foo"));
    assert_eq!(foo.declarations.len(), 2);
    assert_eq!(foo.members_named("A").len(), 1);
    assert_eq!(foo.members_named("B").len(), 1);
}

#[test]
pub fn partial_declarations_must_agree_on_kind() {
    declarations!(
        declarations,
        "partial class Foo {}",
        "partial struct Foo {}",
    );

    assert_eq!(declarations.errors.len(), 1);
    assert!(matches!(
        declarations.errors[0].kind,
        SemanticErrorKind::PartialKindMismatch { .. }
    ));
}

#[test]
pub fn generic_arity_separates_types() {
    declarations!(
        declarations,
        "class Foo {} class Foo<T> {} class Foo<T, U> {}",
    );

    assert_eq!(declarations.errors, vec![]);

    let root = declarations.table.symbol(declarations.table.root());
    let family = root.members_named("Foo");
    assert_eq!(family.len(), 3);

    let arities: Vec<u32> = family
        .iter()
        .map(|&id| declarations.table.symbol(id).arity)
        .collect();
    assert_eq!(arities, vec![0, 1, 2]);
    assert_eq!(declarations.table.fully_qualified_name(family[2]), "Foo`2");
}

#[test]
pub fn overloads_share_a_name_but_fields_do_not() {
    declarations!(
        declarations,
        r#"
        class Ok { void F() {} void F(int x) {} void F<T>() {} }
        class Bad { int x; string x; }
        class Clash { int y; void y() {} }
        "#,
    );

    assert_eq!(declarations.errors.len(), 2);
    assert!(
        declarations
            .errors
            .iter()
            .all(|error| matches!(error.kind, SemanticErrorKind::DuplicateMemberName { .. }))
    );

    let ok = declarations.table.symbol(find(&declarations, "Ok"));
    assert_eq!(ok.members_named("F").len(), 3);
}

#[test]
pub fn members_of_a_partial_type_clash_across_files() {
    declarations!(
        declarations,
        "partial class Foo { int value; }",
        "partial class Foo { string value; }",
    );

    assert_eq!(declarations.errors.len(), 1);
    let error = &declarations.errors[0];
    assert!(matches!(
        error.kind,
        SemanticErrorKind::DuplicateMemberName {
            first_file: FileId(0),
            ..
        }
    ));
    assert_eq!(error.file, FileId(1));
}

#[test]
pub fn enum_members_and_type_parameters_reject_duplicates() {
    declarations!(
        declarations,
        "enum Color { Red, Green, Red } class Box<T, T> {}",
    );

    assert_eq!(declarations.errors.len(), 2);
    assert!(
        declarations
            .errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::DuplicateMemberName { .. }))
    );
    assert!(
        declarations
            .errors
            .iter()
            .any(|error| matches!(error.kind, SemanticErrorKind::DuplicateTypeParameter { .. }))
    );
}

#[test]
pub fn explicit_interface_implementations_do_not_clash() {
    declarations!(
        declarations,
        r#"
        interface IFoo { void Run(); }
        class Foo : IFoo { void IFoo.Run() {} public void Run() {} }
        "#,
    );

    assert_eq!(declarations.errors, vec![]);
}

#[test]
pub fn namespace_and_type_may_not_share_a_name() {
    declarations!(
        declarations,
        "namespace Thing { class Inner {} }",
        "class Thing {}",
    );

    assert_eq!(declarations.errors.len(), 1);
    assert!(matches!(
        declarations.errors[0].kind,
        SemanticErrorKind::TypeNamespaceConflict { .. }
    ));
}

#[test]
pub fn special_member_names_follow_metadata_conventions() {
    declarations!(
        declarations,
        r#"
        class Foo
        {
            Foo() {}
            ~Foo() {}
            public int this[int index] => index;
            public static Foo operator +(Foo a, Foo b) => a;
            public static implicit operator int(Foo a) => 0;
        }
        "#,
    );

    assert_eq!(declarations.errors, vec![]);

    let foo = declarations.table.symbol(find(&declarations, "Foo"));
    for name in [".ctor", "Finalize", "this[]", "op_Addition", "op_Implicit"] {
        assert_eq!(foo.members_named(name).len(), 1, "missing {name}");
    }
}

#[test]
pub fn entity_ids_lead_back_to_symbols() {
    declarations!(
        declarations,
        "namespace App { class Program { void Main() {} } }"
    );

    let program = find(&declarations, "App.Program");
    let symbol = declarations.table.symbol(program);
    let entity = symbol.declarations[0].syntax.entity_id();
    assert_eq!(declarations.symbol_of(entity), Some(program));

    let main = symbol.members_named("Main")[0];
    let SyntaxRef::Method(method) = declarations.table.symbol(main).declarations[0].syntax else {
        panic!("expected method syntax");
    };
    assert_eq!(method.name.value, "Main");
}
