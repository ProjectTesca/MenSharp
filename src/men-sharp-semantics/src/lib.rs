//! Declaration-level semantic analysis for MenSharp.
//!
//! This crate covers the ground between "every file is parsed" and "types can be
//! resolved": it collects what each file declares, merges the per-file views into
//! one [`symbol::SymbolTable`] (C# namespaces are open across files and `partial`
//! types span files, so no single file knows the whole shape of anything), and
//! reports the conflicts that are visible without resolving a single type.
//!
//! Everything here is synchronous and single-threaded on purpose. [`collect_file`]
//! is a pure function of one syntax tree, so the compiler driver may fan it out over
//! however many threads its settings allow; [`merge_declarations`] is the sequential
//! barrier that makes the result deterministic. This crate never learns which of the
//! two happened.
//!
//! ```
//! use men_sharp_parser::MenSharpAST;
//! use men_sharp_semantics::{collect_file, merge_declarations, FileId};
//!
//! let ast = MenSharpAST::parse("namespace App { public class Program {} }");
//! let file = collect_file(FileId(0), ast.ast());
//! let declarations = merge_declarations(vec![file]);
//!
//! let root = declarations.table.root();
//! let app = declarations.table.symbol(root).members_named("App")[0];
//! assert_eq!(declarations.table.symbol(app).members_named("Program").len(), 1);
//! ```

pub mod collect;
pub mod error;
pub mod merge;
pub mod symbol;

pub use collect::{
    DeclarationNode, FileDeclarations, MemberNode, NamespaceNode, TypeNode, collect_file,
};
pub use error::{SemanticError, SemanticErrorKind};
pub use merge::{Declarations, merge_declarations};
pub use symbol::{DeclarationSite, FileId, Symbol, SymbolId, SymbolKind, SymbolTable, SyntaxRef};

#[cfg(test)]
mod tests {
    use men_sharp_parser::MenSharpAST;

    use crate::{
        Declarations, FileId, SemanticErrorKind, SymbolId, SymbolKind, SyntaxRef, collect_file,
        merge_declarations,
    };

    /// Parses each source as one file and merges them, binding the result to `$name`.
    /// A macro because the declarations borrow the syntax trees, which must live in
    /// the caller's scope.
    macro_rules! declarations {
        ($name:ident, $($source:expr),+ $(,)?) => {
            let asts: Vec<MenSharpAST> = vec![$(MenSharpAST::parse($source)),+];
            for ast in &asts {
                assert_eq!(ast.errors(), &[], "test source must parse cleanly");
            }
            let files = asts
                .iter()
                .enumerate()
                .map(|(index, ast)| collect_file(FileId(index as u32), ast.ast()))
                .collect::<Vec<_>>();
            let $name = merge_declarations(files);
        };
    }

    /// Follows a dotted path of member names from the root.
    fn find(declarations: &Declarations, path: &str) -> SymbolId {
        let mut current = declarations.table.root();
        for segment in path.split('.') {
            let matches = declarations.table.symbol(current).members_named(segment);
            assert!(!matches.is_empty(), "no symbol named {segment} in {path}");
            current = matches[0];
        }
        current
    }

    #[test]
    fn partial_class_across_files_is_one_symbol() {
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
    fn namespaces_are_open_across_files_and_spellings() {
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
    fn duplicate_type_is_reported_and_still_merged() {
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
    fn partial_declarations_must_agree_on_kind() {
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
    fn generic_arity_separates_types() {
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
    fn overloads_share_a_name_but_fields_do_not() {
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
    fn members_of_a_partial_type_clash_across_files() {
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
    fn enum_members_and_type_parameters_reject_duplicates() {
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
            declarations.errors.iter().any(|error| matches!(
                error.kind,
                SemanticErrorKind::DuplicateTypeParameter { .. }
            ))
        );
    }

    #[test]
    fn explicit_interface_implementations_do_not_clash() {
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
    fn namespace_and_type_may_not_share_a_name() {
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
    fn special_member_names_follow_metadata_conventions() {
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
    fn entity_ids_lead_back_to_symbols() {
        declarations!(
            declarations,
            "namespace App { class Program { void Main() {} } }"
        );

        let program = find(&declarations, "App.Program");
        let symbol = declarations.table.symbol(program);
        let entity = symbol.declarations[0].syntax.entity_id();
        assert_eq!(declarations.symbol_of(entity), Some(program));

        let main = symbol.members_named("Main")[0];
        let SyntaxRef::Method(method) = declarations.table.symbol(main).declarations[0].syntax
        else {
            panic!("expected method syntax");
        };
        assert_eq!(method.name.value, "Main");
    }

    #[test]
    fn global_usings_are_visible_compilation_wide() {
        declarations!(
            declarations,
            "global using System;\nusing App.Helpers;\nclass A {}",
            "class B {}",
        );

        let globals: Vec<_> = declarations.global_usings().collect();
        assert_eq!(globals.len(), 1);
        assert_eq!(globals[0].0, FileId(0));
    }
}
