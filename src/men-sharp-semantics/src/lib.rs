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

pub mod check;
pub mod collect;
pub mod conversions;
pub mod error;
pub mod external;
mod infer;
pub mod lookup;
pub mod merge;
pub mod resolve;
pub mod symbol;
pub mod types;

pub use check::{
    BodyCheck, ConstructorChain, ConstructorChainKind, ForeachEnumeration, ResolvedCall,
    ResolvedMember, ResolvedTarget, check_file, uncompilable_foreign_members,
};
pub use collect::{
    DeclarationNode, FileDeclarations, MemberNode, NamespaceNode, TypeNode, collect_file,
};
pub use conversions::NumericKind;
pub use error::{SemanticError, SemanticErrorKind};
pub use external::{
    ExternalConstant, ExternalMember, ExternalMemberKind, ExternalTypeInfo, ExternalTypeKind,
    ExternalTypes, NoExternalTypes,
};
pub use lookup::{MemberCandidate, MemberOrigin, TypeSystem};
pub use merge::{Declarations, SourceText, merge_declarations};
pub use resolve::{Signatures, resolve_file, resolve_signatures};
pub use symbol::{
    Accessibility, DeclarationSite, FileId, Symbol, SymbolId, SymbolKind, SymbolTable, SyntaxRef,
};
pub use types::{
    DefaultArgument, ExternalTypeId, FunctionSignature, MemberSignature, ParameterPassing,
    ParameterSignature, TupleElement, Type, TypeTarget, TypeVariance,
};

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

    // ------------------------------------------------------------ resolution

    use crate::types::{ExternalTypeId, MemberSignature, TupleElement, Type, TypeTarget};
    use crate::{ExternalTypes, resolve_signatures};

    /// A hand-written provider standing in for referenced dlls.
    struct MockExternal {
        /// (namespace, name, arity), position = type index.
        types: Vec<(&'static str, &'static str, u32)>,
        /// (parent index, name, arity), position offset by `types.len()`.
        nested: Vec<(u32, &'static str, u32)>,
    }

    impl MockExternal {
        fn corlib() -> Self {
            let mut mock = Self {
                types: vec![
                    ("System", "Int32", 0),
                    ("System", "String", 0),
                    ("System", "Boolean", 0),
                    ("System", "Object", 0),
                    ("System", "SByte", 0),
                    ("System", "Byte", 0),
                    ("System", "Int16", 0),
                    ("System", "UInt16", 0),
                    ("System", "UInt32", 0),
                    ("System", "Int64", 0),
                    ("System", "UInt64", 0),
                    ("System", "Char", 0),
                    ("System", "Single", 0),
                    ("System", "Double", 0),
                    ("System", "Decimal", 0),
                    ("System", "IntPtr", 0),
                    ("System", "UIntPtr", 0),
                    ("System", "Type", 0),
                    ("System", "Array", 0),
                    ("System", "ValueType", 0),
                    ("System", "Enum", 0),
                    ("System", "Func", 1),
                    ("System", "Func", 2),
                    ("System", "Func", 3),
                    ("System", "Action", 0),
                    ("System", "Action", 1),
                    ("System.Collections.Generic", "List", 1),
                    ("UnityEngine", "MonoBehaviour", 0),
                    ("UnityEngine", "Debug", 0),
                    ("UnityEngine.SceneManagement", "SceneManager", 0),
                ],
                nested: Vec::new(),
            };
            let list = mock
                .types
                .iter()
                .position(|&(_, name, _)| name == "List")
                .unwrap() as u32;
            mock.nested = vec![(list, "Enumerator", 1)];
            mock
        }

        fn id_of(&self, namespace: &str, name: &str) -> ExternalTypeId {
            let index = self
                .types
                .iter()
                .position(|&(ns, n, _)| ns == namespace && n == name)
                .unwrap();
            ExternalTypeId {
                assembly: 0,
                type_index: index as u32,
            }
        }
    }

    impl ExternalTypes for MockExternal {
        fn find_type(&self, namespace: &[&str], name: &str, arity: u32) -> Option<ExternalTypeId> {
            let joined = namespace.join(".");
            self.types
                .iter()
                .position(|&(ns, n, a)| ns == joined && n == name && a == arity)
                .map(|index| ExternalTypeId {
                    assembly: 0,
                    type_index: index as u32,
                })
        }

        fn find_nested_type(
            &self,
            parent: ExternalTypeId,
            name: &str,
            arity: u32,
        ) -> Option<ExternalTypeId> {
            self.nested
                .iter()
                .position(|&(p, n, a)| p == parent.type_index && n == name && a == arity)
                .map(|index| ExternalTypeId {
                    assembly: 0,
                    type_index: (self.types.len() + index) as u32,
                })
        }

        fn namespace_exists(&self, namespace: &[&str]) -> bool {
            let joined = namespace.join(".");
            self.types
                .iter()
                .any(|&(ns, ..)| ns == joined || ns.starts_with(&format!("{joined}.")))
        }

        fn type_info(&self, id: crate::types::ExternalTypeId) -> crate::ExternalTypeInfo {
            crate::ExternalTypeInfo {
                kind: crate::ExternalTypeKind::Class,
                arity: self
                    .types
                    .get(id.type_index as usize)
                    .map(|&(.., arity)| arity)
                    .unwrap_or(0),
                is_sealed: false,
                is_abstract: false,
            }
        }

        fn base_type(&self, _: crate::types::ExternalTypeId) -> Option<Type> {
            None
        }

        fn interfaces(&self, _: crate::types::ExternalTypeId) -> Vec<Type> {
            Vec::new()
        }

        fn members_named(
            &self,
            _: crate::types::ExternalTypeId,
            _: &str,
        ) -> Vec<crate::ExternalMember> {
            Vec::new()
        }

        fn display_name(&self, id: crate::types::ExternalTypeId) -> String {
            self.types
                .get(id.type_index as usize)
                .map(|&(ns, name, _)| format!("{ns}.{name}"))
                .unwrap_or_else(|| "<nested>".to_string())
        }

        fn variances(&self, _: crate::types::ExternalTypeId) -> Vec<crate::TypeVariance> {
            Vec::new()
        }

        fn extension_method_owners(
            &self,
            _: &[&str],
            _: &str,
        ) -> Vec<crate::types::ExternalTypeId> {
            Vec::new()
        }
    }

    fn external(id: ExternalTypeId) -> Type {
        Type::Named {
            target: TypeTarget::External(id),
            arguments: Vec::new(),
        }
    }

    fn member_signature(
        declarations: &Declarations,
        signatures: &crate::Signatures,
        path: &str,
        member: &str,
    ) -> MemberSignature {
        let type_symbol = find(declarations, path);
        let member = declarations.table.symbol(type_symbol).members_named(member)[0];
        signatures.members.get(&member).unwrap().clone()
    }

    #[test]
    fn predefined_and_external_types_resolve() {
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
    fn source_types_shadow_and_generics_apply() {
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
    fn method_signatures_resolve_with_their_own_type_parameters() {
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
            crate::types::ParameterPassing::Ref
        );
        assert_eq!(
            function.parameters[1].parameter_type,
            Type::TypeParameter(u)
        );
    }

    #[test]
    fn aliases_static_usings_and_nested_types() {
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
    fn suffixes_tuples_and_special_types() {
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
    fn unresolved_and_ambiguous_names_are_reported() {
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

    // --------------------------------------------------------- member lookup

    use crate::lookup::{MemberOrigin, TypeSystem};

    fn named_source(symbol: SymbolId, arguments: Vec<Type>) -> Type {
        Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        }
    }

    #[test]
    fn inherited_members_come_back_instantiated() {
        declarations!(
            declarations,
            r#"
            public class Base<T>
            {
                public T Value;
                public void SetValue(T value) {}
            }
            public class Derived : Base<int>
            {
                public string Name;
            }
            "#,
        );
        let mock = MockExternal::corlib();
        let signatures = resolve_signatures(&declarations, &mock);
        assert_eq!(signatures.errors, vec![]);

        let system = TypeSystem {
            declarations: &declarations,
            signatures: &signatures,
            external: &mock,
        };

        let derived = named_source(find(&declarations, "Derived"), Vec::new());
        let int32 = external(mock.id_of("System", "Int32"));

        // Base<T>.Value reached through Derived : Base<int> becomes int
        let value = system.members_named(&derived, "Value");
        assert_eq!(value.len(), 1);
        assert_eq!(
            value[0].signature,
            Some(MemberSignature::Field(int32.clone()))
        );
        assert_eq!(
            value[0].declaring_type,
            named_source(find(&declarations, "Base"), vec![int32.clone()])
        );

        // and so does the parameter of SetValue(T)
        let set_value = system.members_named(&derived, "SetValue");
        let Some(MemberSignature::Function(function)) = &set_value[0].signature else {
            panic!("expected a function");
        };
        assert_eq!(function.parameters[0].parameter_type, int32);

        // its own member is found on itself
        let name = system.members_named(&derived, "Name");
        assert_eq!(name.len(), 1);
        assert_eq!(name[0].declaring_type, derived);
    }

    #[test]
    fn lookup_returns_all_candidates_nearest_first() {
        declarations!(
            declarations,
            r#"
            public class A { public void F() {} }
            public class B : A { public static void F(int x) {} }
            "#,
        );
        let mock = MockExternal::corlib();
        let signatures = resolve_signatures(&declarations, &mock);
        let system = TypeSystem {
            declarations: &declarations,
            signatures: &signatures,
            external: &mock,
        };

        let b = named_source(find(&declarations, "B"), Vec::new());
        let candidates = system.members_named(&b, "F");

        assert_eq!(candidates.len(), 2);
        // B's own overload first, with its modifiers readable
        assert_eq!(candidates[0].declaring_type, b);
        assert!(candidates[0].is_static);
        assert_eq!(candidates[0].accessibility, crate::Accessibility::Public);
        assert_eq!(
            candidates[1].declaring_type,
            named_source(find(&declarations, "A"), Vec::new())
        );
        assert!(!candidates[1].is_static);
    }

    #[test]
    fn interfaces_and_type_parameters_walk_their_hierarchies() {
        declarations!(
            declarations,
            r#"
            public interface IAnimal { void Speak(); }
            public interface IDog : IAnimal { void Fetch(); }
            public class Kennel<T> where T : IDog
            {
                public void Handle(T dog) {}
            }
            "#,
        );
        let mock = MockExternal::corlib();
        let signatures = resolve_signatures(&declarations, &mock);
        assert_eq!(signatures.errors, vec![]);
        let system = TypeSystem {
            declarations: &declarations,
            signatures: &signatures,
            external: &mock,
        };

        // an interface receiver sees members of the interfaces it extends
        let dog = named_source(find(&declarations, "IDog"), Vec::new());
        assert_eq!(system.members_named(&dog, "Fetch").len(), 1);
        assert_eq!(system.members_named(&dog, "Speak").len(), 1);

        // a constrained type parameter sees its bound's members
        let kennel = find(&declarations, "Kennel");
        let t = declarations.table.symbol(kennel).type_parameters[0];
        let speak = system.members_named(&Type::TypeParameter(t), "Speak");
        assert_eq!(speak.len(), 1);
        assert!(matches!(speak[0].origin, MemberOrigin::Source(_)));
    }

    // --------------------------------------------------------- body checking

    use crate::check::check_file;

    /// Runs the full front half plus body checking over the sources.
    macro_rules! checked {
        ($check:ident, $($source:expr),+ $(,)?) => {
            declarations!(declarations, $($source),+);
            let mock = MockExternal::corlib();
            let signatures = resolve_signatures(&declarations, &mock);
            assert_eq!(signatures.errors, vec![], "signatures must resolve cleanly");
            let mut $check = crate::check::BodyCheck::default();
            for index in 0..declarations.files.len() {
                $check.merge(check_file(&declarations, &signatures, &mock, index));
            }
        };
    }

    fn error_kinds(check: &crate::check::BodyCheck) -> Vec<&SemanticErrorKind> {
        check.errors.iter().map(|error| &error.kind).collect()
    }

    #[test]
    fn locals_and_var_infer_and_convert() {
        checked!(
            check,
            r#"
            public class Body
            {
                void Run()
                {
                    var count = 1;
                    count = 2;
                    long widened = count;
                    byte narrowed = 5;
                    string wrong = count;
                }
            }
            "#,
        );

        // the probe: assigning the inferred int to string names both types
        assert_eq!(check.errors.len(), 1);
        assert_eq!(
            check.errors[0].kind,
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
    }

    #[test]
    fn members_inheritance_and_conditions() {
        checked!(
            check,
            r#"
            public class Base
            {
                public int health;
                public bool IsAlive() { return health > 0; }
            }
            public class Player : Base
            {
                public string name;

                void Update()
                {
                    if (IsAlive() && health < 100) { health = health + 1; }
                    if (health) {}
                    Missing();
                    string wrong = this.unknown;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        assert!(matches!(
            kinds[0],
            SemanticErrorKind::ConditionNotBoolean { .. }
        ));
        assert!(matches!(kinds[1], SemanticErrorKind::UnknownIdentifier));
        assert!(matches!(kinds[2], SemanticErrorKind::UnknownMember { .. }));
    }

    #[test]
    fn overloads_pick_by_argument_types() {
        checked!(
            check,
            r#"
            public class Overloads
            {
                void Take(int value) {}
                void Take(string value) {}
                string Pick(long value) { return ""; }
                int Pick(int value) { return 0; }

                void Run()
                {
                    Take(1);
                    Take("hello");
                    Take(true);
                    string probe = Pick(1);
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        // Take(true) fits neither overload
        assert!(matches!(kinds[0], SemanticErrorKind::NoMatchingOverload));
        // Pick(1) prefers the exact int overload returning int, so the string
        // probe reports int
        assert_eq!(
            *kinds[1],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
    }

    #[test]
    fn generic_methods_infer_or_ask_for_annotations() {
        checked!(
            check,
            r#"
            public class Generics
            {
                T Identity<T>(T value) { return value; }
                T Make<T>() { return default(T); }

                void Run()
                {
                    string ok = Identity("hello");
                    string probe = Identity(1);
                    string explicitly = Make<string>();
                    var impossible = Make();
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
        // Make() has nothing to infer T from: the annotate-it error
        assert!(matches!(
            kinds[1],
            SemanticErrorKind::CannotInferTypeArguments
        ));
    }

    #[test]
    fn enums_statics_and_value_flow() {
        checked!(
            check,
            r#"
            public enum Color { Red, Green }
            public class Statics
            {
                public static int counter;
                public int instance;

                static void Tick()
                {
                    counter = counter + 1;
                    instance = 2;
                    var color = Color.Red;
                    bool same = color == Color.Green;
                    string wrong = color;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        assert!(matches!(
            kinds[0],
            SemanticErrorKind::InstanceMemberInStaticContext
        ));
        assert_eq!(
            *kinds[1],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "Color".to_string(),
            }
        );
    }

    #[test]
    fn arrays_foreach_and_indexing() {
        checked!(
            check,
            r#"
            public class Arrays
            {
                void Run(int[] values)
                {
                    var first = values[0];
                    values[1] = first + 1;
                    int total = 0;
                    foreach (var value in values) { total += value; }
                    foreach (string wrong in values) {}
                    values["x"] = 1;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        // int element into a string loop variable
        assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
        // a string index into an int[] slot
        assert!(matches!(kinds[1], SemanticErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn user_defined_operators_and_string_concat() {
        checked!(
            check,
            r#"
            public struct Vec
            {
                public float x;
                public static Vec operator +(Vec a, Vec b) { return a; }
            }
            public class Ops
            {
                void Run(Vec a, Vec b, int n)
                {
                    var sum = a + b;
                    sum.x = 1.5f;
                    string label = "n = " + n;
                    var bad = a * b;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert!(matches!(
            kinds[0],
            SemanticErrorKind::InvalidOperator { .. }
        ));
    }

    #[test]
    fn out_var_and_is_patterns_bind_locals() {
        checked!(
            check,
            r#"
            public class Player {}
            public class Bindings
            {
                bool TryGet(out int value) { value = 1; return true; }

                void Run(object thing)
                {
                    if (TryGet(out var got)) { int use = got; }
                    if (thing is Player player) { Player p = player; }
                    string probe = got;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
    }

    #[test]
    fn return_types_are_enforced() {
        checked!(
            check,
            r#"
            public class Returns
            {
                int Number() { return "text"; }
                void Nothing() { return 1; }
                int Missing() { return; }
                long Widened() { return 1; }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
        assert!(matches!(kinds[1], SemanticErrorKind::ReturnValueMismatch));
        assert!(matches!(kinds[2], SemanticErrorKind::ReturnValueMismatch));
    }

    #[test]
    fn unsupported_constructs_get_scaffolding_errors() {
        checked!(
            check,
            r#"
            public class Scaffolding
            {
                void Run()
                {
                    var q = from x in "abc" select x;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert!(
            kinds
                .iter()
                .all(|kind| matches!(kind, SemanticErrorKind::UnsupportedExpression)),
            "{kinds:?}"
        );
        assert_eq!(kinds.len(), 1);
    }

    // ---------------------------------------------- spec-shaped inference

    #[test]
    fn lower_bound_inference_sees_through_interfaces() {
        checked!(
            check,
            r#"
            public interface IContainer<T> { }
            public class Box : IContainer<string> { }
            public class App
            {
                T First<T>(IContainer<T> container) { return default; }

                void Run(Box box)
                {
                    string ok = First(box);
                    int probe = First(box);
                }
            }
            "#,
        );

        // the probe proves T was fixed to string through Box : IContainer<string>
        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.Int32".to_string(),
                found: "System.String".to_string(),
            }
        );
    }

    #[test]
    fn lambdas_check_against_delegates() {
        checked!(
            check,
            r#"
            public delegate bool Filter(int value);
            public class App
            {
                int Count(Filter filter) { return 0; }

                void Run()
                {
                    var a = Count(x => x > 0);
                    Filter direct = x => x != 1;
                    Count((int x) => true);
                    Count(x => x + 1);
                    Count((string s) => true);
                    string probe = a;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        // `x + 1` is int, the delegate wants bool
        assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
        // `(string s)` disagrees with the delegate's int
        assert!(matches!(kinds[1], SemanticErrorKind::TypeMismatch { .. }));
        // the probe proves Count(...) returned int
        assert_eq!(
            *kinds[2],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
    }

    #[test]
    fn two_phase_inference_flows_through_lambda_returns() {
        checked!(
            check,
            r#"
            public delegate R Map<T, R>(T value);
            public class App
            {
                R Apply<T, R>(T value, Map<T, R> mapper) { return default; }

                void Run()
                {
                    string probe = Apply(1, x => x > 0);
                }
            }
            "#,
        );

        // T = int fixes from the first argument, the lambda's body then types as
        // bool, and R = bool comes back out — visible in the probe
        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Boolean".to_string(),
            }
        );
    }

    #[test]
    fn natural_lambda_and_best_common_types() {
        checked!(
            check,
            r#"
            public class Animal { }
            public class Dog : Animal { }
            public class App
            {
                void Run(bool flag, Animal animal, Dog dog)
                {
                    var f = (int x) => x + 1;
                    string probe_f = f;

                    var pick = flag ? dog : animal;
                    string probe_pick = pick;

                    var mixed = flag ? 1 : 2.0;
                    string probe_mixed = mixed;

                    var xs = new[] { 1, 2L };
                    string probe_xs = xs;
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 4, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Func<System.Int32, System.Int32>".to_string(),
            }
        );
        assert_eq!(
            *kinds[1],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "Animal".to_string(),
            }
        );
        assert_eq!(
            *kinds[2],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Double".to_string(),
            }
        );
        assert_eq!(
            *kinds[3],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int64[]".to_string(),
            }
        );
    }

    #[test]
    fn extension_methods_resolve_with_scoping() {
        checked!(
            check,
            r#"
            public class Animal { }
            public class Dog : Animal { }

            public static class Extensions
            {
                public static int Doubled(this int value) { return value * 2; }
                public static T LastOf<T>(this T[] items) { return default; }
                public static string Show(this Animal animal) { return "animal"; }
            }

            public class App
            {
                void Run(int[] numbers, Dog dog)
                {
                    int d = 5.Doubled();
                    string byReceiver = dog.Show();
                    string probe = numbers.LastOf();
                    dog.Missing();
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        // the probe proves LastOf inferred T = int through the receiver
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
        assert!(matches!(kinds[1], SemanticErrorKind::UnknownMember { .. }));
    }

    #[test]
    fn extension_methods_respect_using_scopes() {
        checked!(
            check,
            "namespace Lib { public static class E { public static int Twice(this int v) { return v + v; } } }",
            "using Lib;\npublic class UsesIt { void Run() { int x = 3.Twice(); } }",
            "public class LacksIt { void Run() { int y = 3.Twice(); } }",
        );

        // only the file without `using Lib;` fails
        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert!(matches!(kinds[0], SemanticErrorKind::UnknownMember { .. }));
        assert_eq!(check.errors[0].file, FileId(2));
    }

    #[test]
    fn instance_methods_win_over_extensions() {
        checked!(
            check,
            r#"
            public static class Extensions
            {
                public static string Describe(this Thing thing) { return "extension"; }
            }
            public class Thing
            {
                public int Describe() { return 1; }
            }
            public class App
            {
                void Run(Thing thing)
                {
                    string probe = thing.Describe();
                }
            }
            "#,
        );

        // the probe reports int: the instance method was chosen
        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert_eq!(
            *kinds[0],
            SemanticErrorKind::TypeMismatch {
                expected: "System.String".to_string(),
                found: "System.Int32".to_string(),
            }
        );
    }

    #[test]
    fn target_typed_new_and_default() {
        checked!(
            check,
            r#"
            public class Config
            {
                public int retries;
                public Config() { }
                public Config(int retries) { this.retries = retries; }
            }
            public class App
            {
                void Run()
                {
                    Config a = new();
                    Config b = new(3);
                    int c = default;
                    Config d = new("wrong");
                }
            }
            "#,
        );

        let kinds = error_kinds(&check);
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert!(matches!(kinds[0], SemanticErrorKind::NoMatchingOverload));
    }

    #[test]
    fn enum_members_surface_without_signatures() {
        declarations!(declarations, "public enum Color { Red, Green }");
        let mock = MockExternal::corlib();
        let signatures = resolve_signatures(&declarations, &mock);
        let system = TypeSystem {
            declarations: &declarations,
            signatures: &signatures,
            external: &mock,
        };

        let color = named_source(find(&declarations, "Color"), Vec::new());
        let red = system.members_named(&color, "Red");
        assert_eq!(red.len(), 1);
        assert_eq!(red[0].kind, SymbolKind::EnumMember);
        assert!(red[0].is_static);
        assert_eq!(red[0].signature, None);
    }

    #[test]
    fn sibling_types_resolve_across_files_in_one_namespace() {
        declarations!(
            declarations,
            "namespace Game { public partial class Player { public Weapon weapon; } }",
            "namespace Game { public class Weapon {} }",
        );
        let mock = MockExternal::corlib();
        let signatures = resolve_signatures(&declarations, &mock);
        assert_eq!(signatures.errors, vec![]);

        let weapon = find(&declarations, "Game.Weapon");
        assert_eq!(
            member_signature(&declarations, &signatures, "Game.Player", "weapon"),
            MemberSignature::Field(Type::Named {
                target: TypeTarget::Source(weapon),
                arguments: Vec::new(),
            })
        );
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
